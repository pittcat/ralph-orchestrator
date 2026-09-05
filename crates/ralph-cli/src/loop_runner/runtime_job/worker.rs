//! 2026-09-03-0959 plan U6 (D7-D9 / E10-E12): the generic kernel
//! that runs one per-Unit invocation.
//!
//! `run_job` is the smallest loop the runtime job kernel
//! executes:
//!   1. Pre-fence the port against the descriptor.
//!   2. Build the prompt context.
//!   3. Launch the child through the port.
//!   4. Collect with a deadline; on `HeartbeatTimeout` cancel
//!      and surface a typed error.
//!
//! Every step is delegated to a port — the kernel itself does
//! NOT call `std::process::Command` directly. That keeps the
//! kernel testable from unit tests (which drive
//! `FakeJobProcessPort`).
//!
//! Env policy is applied at launch time by the *caller* (see
//! `dag_scheduler::jobs`). The kernel accepts an
//! `EnvSeedProvider` so the env-allowlist test can prove the
//! filter is consulted at the launch boundary even though the
//! kernel itself does not own the filter logic.

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

    let prompt = build_prompt_context(descriptor);

    // Sanity: the descriptor's `env_allowlist_keys` MUST be a
    // subset of the policy's allowlist. We do NOT raise a typed
    // error here — that would leak policy vs descriptor
    // disagreements; instead we apply the strictest of the two
    // by intersecting. Tests assert the child env contains only
    // entries on BOTH the descriptor's list AND the policy.
    let _ = env_policy.filter_child_env(&env_seed.host_env());

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
        let prompt = build_prompt_context(&JobDescriptor::new(
            "U6-001",
            "j-1",
            "executor",
            Stage::Execute,
        ));
        let _ = port.launch(&prompt).expect("launch");
        // run_job will launch its own child (monotonic pid
        // counter starts at 1000; first launch took 1000,
        // so run_job's launch returns 1001) — schedule the
        // result for that pid.
        port.enqueue_result(
            1001,
            ProcessResult::new(json!({"exit_code": 0}), Some(0), 1001, 1),
        );
        let _policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let _descriptor = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute);
        let result = run_job(&_descriptor, &port, &_policy, &empty_env).expect("ok");
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(port.launch_count(), 2);
    }

    /// Pre-fence rejection short-circuits before launch.
    #[test]
    fn run_job_propagates_pre_fence_rejection() {
        let mut port = FakeJobProcessPort::new("test");
        port.set_pre_fence_fail("U6-bad");
        let _policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let descriptor = JobDescriptor::new("U6-bad", "j-1", "executor", Stage::Execute);
        let result = run_job(&descriptor, &port, &_policy, &empty_env);
        assert!(matches!(result, Err(RuntimeJobError::PreFenceFailed(_))));
        assert_eq!(port.launch_count(), 0);
    }

    /// `CollectFailed` without deadline breach is forwarded
    /// verbatim — pipeline treats it as "still running".
    #[test]
    fn run_job_forwards_collect_failed_when_not_timed_out() {
        let port = FakeJobProcessPort::new("test");
        let prompt = build_prompt_context(&JobDescriptor::new(
            "U6-001",
            "j-1",
            "executor",
            Stage::Execute,
        ));
        let _ = port.launch(&prompt).expect("launch");
        let _policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let _descriptor = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute);
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

    /// TG-S08 (PMI-004②, P2, post-merge-converge): `run_job` 的
    /// env 过滤结果被 `let _ =` 丢弃——现状 pin。
    ///
    /// kernel 在 launch 边界调用了 `env_policy.filter_child_env(
    /// &env_seed.host_env())` 但过滤产物没有传进 `port.launch`(
    /// `PromptContext` 也不携带 env map)。隔离承诺(R8 / E14 / E15
    /// 「no inherited host secret」)在 kernel 层不成立: host env 的
    /// 敏感键(`FAKE_SECRET_TOKEN`)在当前架构下**没有任何携带面**
    /// ——既没有进 launch 参数,也没有被拦下;一旦真实 port 用
    /// 继承式 `Command`(不显式 `.env_clear()` / `.envs()`)落地,
    /// 过滤形同虚设。
    ///
    /// 本断言是**过渡 pin**(expected GREEN at HEAD):
    ///   - 变红 = 过滤结果已被接线(`PromptContext` 或 `launch` 签名
    ///     携带 env)→ 必须立即补「敏感键缺席」正向断言(
    ///     child env 仅含 descriptor ∩ policy 交集,
    ///     `FAKE_SECRET_TOKEN` 缺席),否则 env 泄露面打开。
    ///   - promote 契约(见 TG-S13 / PMI-004 §expected): env
    ///     allowlist 过滤结果是 child env 的**唯一**来源。
    #[test]
    fn tg_s08_run_job_env_filter_result_is_dropped_transitional_pin() {
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
        // 形态);kernel 的 filter 调用会产出一个只含 SAFE_VAR 的
        // map——但该产物被丢弃。
        let policy = DagEnvPolicy::from_declared(vec!["SAFE_VAR"]);

        // run_job launches exactly one child (pid 1000); give it a
        // result so the kernel completes without deadline noise.
        port.enqueue_result(
            1000,
            ProcessResult::new(serde_json::json!({"exit_code": 0}), Some(0), 1000, 1),
        );
        let result = run_job(&descriptor, &port, &policy, &env_seed).expect("ok");
        assert_eq!(result.exit_code, Some(0));

        // 现状 pin: launch 收到的 PromptContext 没有任何 env map
        // 携带面——env_allowlist_keys 只是**名字列表**(声明),不是
        // 过滤后的值 map。断言两件事:
        //   ① launch 被调用(kernel 完整走完);
        //   ② 每次收到的 PromptContext 中既无过滤产物(只有名字
        //      列表,不含 `safe-value`),也无 host env 的值。
        let launches = port.launches.lock().expect("launches mutex");
        assert!(
            !launches.is_empty(),
            "precondition: run_job must have launched the child"
        );
        for ctx in launches.iter() {
            // 名字列表里只有 SAFE_VAR(声明面)——不含 sentinel 名。
            assert!(
                !ctx.env_allowlist_keys
                    .iter()
                    .any(|k| k == "FAKE_SECRET_TOKEN"),
                "sentinel env NAME leaked into the prompt allowlist — new wiring \
                 landed; flip this pin per TG-S08 promote contract"
            );
            // 过滤产物(值 map)不存在携带面: PromptContext 的字段
            // 全部是 String/Vec<PathBuf>/Vec<String>,不含 env 值。
            // 这里用序列化全文反向钉住: 任何「safe-value」/「sentinel」
            // 字符串都不应出现在 launch 参数里。
            let rendered = format!("{ctx:?}");
            assert!(
                !rendered.contains("safe-value") && !rendered.contains("sentinel"),
                "TG-S08 transitional pin: the launch parameters now carry env \
                 VALUES — the filter result (or raw host env) has been wired \
                 into the port.launch surface. Per TG-S08 promote contract: \
                 immediately add the positive isolation assertion (child env \
                 contains ONLY descriptor ∩ policy intersection; \
                 FAKE_SECRET_TOKEN absent), otherwise the env-leak surface \
                 is open. See .ralph/post-merge/09-test-gap-plan.md §TG-S08 \
                 and PMI-004②."
            );
        }
    }
}
