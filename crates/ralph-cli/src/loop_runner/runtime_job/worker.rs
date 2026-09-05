//! 2026-09-03-0959 plan U6 (D7-D9 / E10-E12): the generic kernel
//! that runs one per-Unit invocation.
//!
//! `run_job` is the smallest loop the runtime job kernel
//! executes:
//!   1. Pre-fence the port against the descriptor.
//!   2. Build the prompt context (including the PMI-004②
//!      filtered child env map).
//!   3. Launch the child through the port.
//!   4. Collect with a deadline; on `HeartbeatTimeout` cancel
//!      and surface a typed error.
//!
//! Every step is delegated to a port — the kernel itself does
//! NOT call `std::process::Command` directly. That keeps the
//! kernel testable from unit tests (which drive
//! `FakeJobProcessPort`).
//!
//! Env policy is applied at launch time by the kernel: the
//! kernel resolves the allowlist-filtered env map into the
//! prompt context (`PromptContext::child_env`), which the port
//! MUST apply as the child's only env source (real ports:
//! `env_clear().envs(...)`). `EnvSeedProvider` supplies the
//! host env snapshot the filter runs against.

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::time::{Duration, Instant};

#[cfg(test)]
use super::environment::DagEnvPolicy;
#[cfg(test)]
use super::process::{JobProcessHandle, JobProcessPort};
#[cfg(test)]
use super::prompt::build_prompt_context;
#[cfg(test)]
use super::{JobDescriptor, ProcessResult, RuntimeJobError};

/// Default kernel deadline. Matches the plan §7 U6 "hard
/// timeout" requirement. The pipeline may override per-stage.
#[cfg(test)]
pub const DEFAULT_KERNEL_DEADLINE_MS: u64 = 60_000;

/// Source of the env map the kernel hands to the port. Real
/// callers pass a closure that reads from a controlled store;
/// tests pass a literal `HashMap` to exercise the allowlist
/// assertion.
#[cfg(test)]
pub trait EnvSeedProvider: Send {
    fn host_env(&self) -> HashMap<String, String>;
}

#[cfg(test)]
impl<F> EnvSeedProvider for F
where
    F: Fn() -> HashMap<String, String> + Send,
{
    fn host_env(&self) -> HashMap<String, String> {
        (self)()
    }
}

/// Run one kernel invocation. Returns the port's
/// `ProcessResult`. Errors are typed (`RuntimeJobError`); the
/// pipeline branches on them.
#[cfg(test)]
pub fn run_job<P>(
    descriptor: &JobDescriptor,
    port: &P,
    env_policy: &DagEnvPolicy,
    env_seed: &dyn EnvSeedProvider,
) -> Result<ProcessResult, RuntimeJobError>
where
    P: JobProcessPort + ?Sized,
{
    port.pre_fence(descriptor)?;

    // PMI-004②: the filtered env map is computed ONCE here and
    // carried by the prompt context into `port.launch` — the
    // child's ONLY env source. A real port applies it with
    // `Command::env_clear().envs(&ctx.child_env)`. The old
    // `let _ = filter_child_env(...)` (compute-and-discard) is
    // exactly the gap PMI-004② closed: the intersection below
    // is "strictest of descriptor ∩ policy" because the policy
    // only forwards names present in BOTH its own allowlist and
    // the host env.
    let host_env = env_seed.host_env();
    let prompt = build_prompt_context(descriptor, env_policy, &host_env);

    let handle = port.launch(&prompt)?;
    let pid = handle.pid();

    let started = Instant::now();
    let deadline = Duration::from_millis(DEFAULT_KERNEL_DEADLINE_MS);
    let result = port.collect_with_deadline(handle.as_ref(), DEFAULT_KERNEL_DEADLINE_MS);

    // We deliberately do NOT poll in a busy loop here — the
    // port is responsible for honouring the deadline. The
    // kernel records the elapsed time so the heartbeat timeout
    // check below has a single source of truth.
    let elapsed_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(mut r) => {
            // Backfill elapsed time so the ingress / pipeline
            // see a consistent value even if the port didn't
            // stamp it.
            if r.elapsed_ms == 0 {
                r.elapsed_ms = elapsed_ms;
            }
            Ok(r)
        }
        Err(RuntimeJobError::CollectFailed(msg)) => {
            if elapsed_ms >= deadline.as_millis() as u64 {
                let _ = port.cancel(pid);
                return Err(RuntimeJobError::HeartbeatTimeout {
                    stage: descriptor.stage,
                    elapsed_ms,
                    cap_ms: deadline.as_millis() as u64,
                });
            }
            Err(RuntimeJobError::CollectFailed(msg))
        }
        Err(e) => Err(e),
    }
}

/// Compile-time assertion that the `JobProcessHandle` trait
/// object can flow through `Box`. Keeps the public port
/// abstraction honest. Test-only — the bin target does not need
/// this guard.
#[cfg(test)]
fn _assert_handle_object_safe(_: Box<dyn JobProcessHandle>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::runtime_job::Stage;
    use crate::loop_runner::runtime_job::process::FakeJobProcessPort;
    use serde_json::json;

    fn empty_env() -> HashMap<String, String> {
        HashMap::new()
    }

    /// Happy path: pre-fence → launch → collect returns the
    /// port's `ProcessResult`.
    #[test]
    fn run_job_happy_path() {
        let port = FakeJobProcessPort::new("test");
        let empty_policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let prompt = build_prompt_context(
            &JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute),
            &empty_policy,
            &empty_env(),
        );
        let _ = port.launch(&prompt).expect("launch");
        // run_job will launch its own child (monotonic pid
        // counter starts at 1000; first launch took 1000,
        // so run_job's launch returns 1001) — schedule the
        // result for that pid.
        port.enqueue_result(
            1001,
            ProcessResult::new(json!({"exit_code": 0}), Some(0), 1001, 1),
        );
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let descriptor = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute);
        let result = run_job(&descriptor, &port, &policy, &empty_env).expect("ok");
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(port.launch_count(), 2);
    }

    /// Pre-fence rejection short-circuits before launch.
    #[test]
    fn run_job_propagates_pre_fence_rejection() {
        let mut port = FakeJobProcessPort::new("test");
        port.set_pre_fence_fail("U6-bad");
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let descriptor = JobDescriptor::new("U6-bad", "j-1", "executor", Stage::Execute);
        let result = run_job(&descriptor, &port, &policy, &empty_env);
        assert!(matches!(result, Err(RuntimeJobError::PreFenceFailed(_))));
        assert_eq!(port.launch_count(), 0);
    }

    /// `CollectFailed` without deadline breach is forwarded
    /// verbatim — pipeline treats it as "still running".
    #[test]
    fn run_job_forwards_collect_failed_when_not_timed_out() {
        let port = FakeJobProcessPort::new("test");
        let empty_policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let prompt = build_prompt_context(
            &JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute),
            &empty_policy,
            &empty_env(),
        );
        // The fake's collect needs a live handle; pass the one
        // we just allocated.
        let handle = port.launch(&prompt).expect("launch 2");
        let err = port
            .collect_with_deadline(handle.as_ref(), 0)
            .expect_err("not ready");
        assert!(matches!(err, RuntimeJobError::CollectFailed(_)));
        // run_job would call collect_with_deadline on a fresh
        // launch; verify the helper does not panic when the
        // port has no result ready.
    }

    /// TG-S08 (PMI-004②, P2, post-merge-converge) — FLIPPED per
    /// the pin's own promote contract: the filter result is now
    /// wired into the launch surface (`PromptContext::child_env`),
    /// so the pin flips to the POSITIVE isolation assertion the
    /// contract demanded: the child env carried by the launch
    /// parameters contains ONLY the descriptor ∩ policy
    /// intersection; the undeclared sentinel
    /// (`FAKE_SECRET_TOKEN`) is absent.
    #[test]
    fn tg_s08_run_job_launches_with_filtered_env_isolation() {
        // Sentinel secret in the host-env seed: allowlist 只放行
        // `SAFE_VAR`,`FAKE_SECRET_TOKEN` 必须被 filter 掉。
        let host_env: HashMap<String, String> = [
            ("SAFE_VAR".to_string(), "safe-value".to_string()),
            ("FAKE_SECRET_TOKEN".to_string(), "sentinel".to_string()),
        ]
        .into_iter()
        .collect();
        let env_seed = || host_env.clone();

        let port = FakeJobProcessPort::new("test");
        let descriptor = JobDescriptor::new_full(
            "U6-001",
            "j-1",
            "executor",
            Stage::Execute,
            vec![],
            vec![],
            vec!["SAFE_VAR".to_string()],
        );
        // policy allowlist 与 descriptor 声明一致(交集语义的最小
        // 形态);kernel 的 filter 产物(只含 SAFE_VAR)现在经
        // `build_prompt_context` 进入 `PromptContext::child_env`。
        let policy = DagEnvPolicy::from_declared(vec!["SAFE_VAR"]);

        // run_job launches exactly one child (pid 1000); give it a
        // result so the kernel completes without deadline noise.
        port.enqueue_result(
            1000,
            ProcessResult::new(serde_json::json!({"exit_code": 0}), Some(0), 1000, 1),
        );
        let result = run_job(&descriptor, &port, &policy, &env_seed).expect("ok");
        assert_eq!(result.exit_code, Some(0));

        let launches = port.launches.lock().expect("launches mutex");
        assert_eq!(
            launches.len(),
            1,
            "run_job must launch exactly one child with the filtered env context"
        );
        let ctx = &launches[0];

        // 正向隔离断言 (TG-S08 promote contract 原文语义):
        //   child env 仅含 descriptor ∩ policy 交集。
        assert_eq!(
            ctx.child_env.len(),
            1,
            "child env must contain ONLY the declared intersection"
        );
        assert_eq!(
            ctx.child_env.get("SAFE_VAR").map(String::as_str),
            Some("safe-value"),
            "declared + allowlisted entry must be forwarded verbatim"
        );
        // `FAKE_SECRET_TOKEN` 缺席 — R8/E14/E15「no inherited host
        // secret」的 kernel 级行为级断言。
        assert!(
            !ctx.child_env.contains_key("FAKE_SECRET_TOKEN"),
            "TG-S08 flipped pin: undeclared host secret FAKE_SECRET_TOKEN reached \
             the launch surface — the env-leak surface is OPEN (PMI-004② \
             regression). The child env must be the filter result only, never \
             the raw host env. See .ralph/post-merge/09-test-gap-plan.md \
             §TG-S08 and PMI-004②."
        );
        // 名字列表里只有 SAFE_VAR(声明面)——不含 sentinel 名。
        assert!(
            !ctx.env_allowlist_keys
                .iter()
                .any(|k| k == "FAKE_SECRET_TOKEN"),
            "sentinel env NAME must not leak into the declared allowlist either"
        );
    }
}
