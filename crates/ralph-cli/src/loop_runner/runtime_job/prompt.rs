//! 2026-09-03-0959 plan U6 (D7 / S7): materialise a stable
//! `(unit_key, job_id, hat, stage, allowed_paths,
//!  forbidden_paths, env_allowlist_keys)` slice the kernel hands
//! to the subprocess port.
//!
//! The slice is intentionally **plain data** — no trait objects,
//! no file handles. The kernel builds it once per invocation and
//! the port may inspect, log (with sanitised redaction — see
//! `dag_inspect`'s forbidden-substring list for the style), or
//! serialise it. The legacy wave worker keeps its own prompt
//! builder; this type is DAG-only.
//!
//! PMI-004②: the context also carries the FILTERED child env map
//! (`child_env`). The kernel computes it once (allowlist ∩ host
//! env) and the port's launch MUST use it as the child's ONLY
//! env source — a real port applies it with
//! `Command::env_clear().envs(&child_env)` so undeclared host
//! secrets never reach the child. Before this field existed the
//! kernel's filter result was discarded (`let _ =`), which is the
//! gap PMI-004② closed.

use std::collections::HashMap;
use std::path::PathBuf;

use super::JobDescriptor;

/// Plain-data prompt context. Stable, `Clone`, `PartialEq`,
/// `Eq` — every field is `String`, `Vec<PathBuf>`, or the
/// plain `HashMap<String, String>` child env map. The
/// descriptor's `changed_paths` is intentionally **omitted**
/// from the prompt context: it is part of the integration
/// authorisation surface (U7's concern), not the worker
/// prompt.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方(EventLoop 接线属后续 Step),item 级
/// `#[allow(dead_code)]`——promote 义务见
/// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
/// 义务清单」#1/#2,接线落地后移除。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContext {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: String,
    pub allowed_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
    pub env_allowlist_keys: Vec<String>,
    /// PMI-004②: the env-allowlist-filtered child env map. This
    /// is the **only** env surface a real port may hand to the
    /// child process. `env_allowlist_keys` stays as the declared
    /// (name-only) contract; this map is the resolved values.
    pub child_env: HashMap<String, String>,
}

#[allow(dead_code)] // 同 `PromptContext` 的 Step 1+2 promote 注释。
impl PromptContext {
    /// Read-only accessors used by tests + the kernel.
    pub fn unit_key(&self) -> &str {
        &self.unit_key
    }
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
    pub fn hat(&self) -> &str {
        &self.hat
    }
    pub fn stage(&self) -> &str {
        &self.stage
    }
}

/// Build a prompt context from a descriptor, resolving the
/// child env map from `host_env` via the allowlist. Pure with
/// respect to `(descriptor, host_env)`: same inputs always
/// yield the same context. `stage` is rendered as the stable
/// `Stage::as_str` so downstream consumers can branch on the
/// value without re-implementing the mapping.
///
/// PMI-004②: `child_env` =
/// `env_policy.filter_child_env(host_env)` — the filter's
/// output is the child's ONLY env source. The kernel no longer
/// computes it just to throw it away.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;真实
/// subprocess backend 接线前无 bin 侧消费方,item 级
/// `#[allow(dead_code)]`(同 `PromptContext` 的 promote 注释)。
#[allow(dead_code)]
pub fn build_prompt_context(
    descriptor: &JobDescriptor,
    env_policy: &super::environment::DagEnvPolicy,
    host_env: &HashMap<String, String>,
) -> PromptContext {
    let child_env = env_policy.filter_child_env(host_env);
    PromptContext {
        unit_key: descriptor.unit_key.clone(),
        job_id: descriptor.job_id.clone(),
        hat: descriptor.hat.clone(),
        stage: descriptor.stage.as_str().to_string(),
        allowed_paths: descriptor.allowed_paths.clone(),
        forbidden_paths: descriptor.forbidden_paths.clone(),
        env_allowlist_keys: descriptor.env_allowlist_keys.clone(),
        child_env,
    }
}

#[cfg(test)]
mod tests {
    use super::super::environment::DagEnvPolicy;
    use super::*;
    use crate::loop_runner::runtime_job::Stage;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Build is a pure function: same (descriptor, env) → same
    /// context.
    #[test]
    fn build_is_pure() {
        let d = JobDescriptor::new_full(
            "U6-001",
            "exec-w-1-1",
            "executor",
            Stage::Execute,
            vec![PathBuf::from("/repo/src")],
            vec![PathBuf::from("/repo/.git")],
            vec!["PATH".to_string(), "HOME".to_string()],
        );
        let host_env: HashMap<String, String> = [("PATH".to_string(), "/bin".to_string())].into();
        let policy = DagEnvPolicy::from_declared(["PATH", "HOME"]);
        let a = build_prompt_context(&d, &policy, &host_env);
        let b = build_prompt_context(&d, &policy, &host_env);
        assert_eq!(a, b);
    }

    /// Stage serialises as the stable short string, not the
    /// `Debug` form (which would leak `"Execute"` only by chance —
    /// the explicit `as_str` mapping is the contract).
    #[test]
    fn stage_uses_stable_short_string() {
        let d = JobDescriptor::new("U6-001", "exec", "executor", Stage::Review);
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let host_env: HashMap<String, String> = HashMap::new();
        let ctx = build_prompt_context(&d, &policy, &host_env);
        assert_eq!(ctx.stage(), "review");
        assert_eq!(ctx.unit_key(), "U6-001");
        assert_eq!(ctx.job_id(), "exec");
        assert_eq!(ctx.hat(), "executor");
    }

    /// Path + env fields propagate verbatim. The kernel does not
    /// touch them at build time.
    #[test]
    fn paths_and_env_propagate_verbatim() {
        let allowed = vec![PathBuf::from("/repo/a"), PathBuf::from("/repo/b")];
        let forbidden = vec![PathBuf::from("/repo/.git")];
        let env = vec!["PATH".to_string(), "RALPH_DAG".to_string()];
        let d = JobDescriptor::new_full(
            "U6-002",
            "exec-w-2-1",
            "executor",
            Stage::Execute,
            allowed.clone(),
            forbidden.clone(),
            env.clone(),
        );
        let host_env: HashMap<String, String> = [
            ("PATH".to_string(), "/a:/b".to_string()),
            ("UNRELATED".to_string(), "noise".to_string()),
        ]
        .into();
        let policy = DagEnvPolicy::from_declared(["PATH", "RALPH_DAG"]);
        let ctx = build_prompt_context(&d, &policy, &host_env);
        assert_eq!(ctx.allowed_paths, allowed);
        assert_eq!(ctx.forbidden_paths, forbidden);
        assert_eq!(ctx.env_allowlist_keys, env);
    }

    /// PMI-004②: `child_env` carries exactly the allowlist ∩
    /// host-env intersection — the filter result is the child's
    /// ONLY env source. `UNRELATED` (undeclared) must be absent.
    #[test]
    fn child_env_carries_filtered_map_not_host_env() {
        let d = JobDescriptor::new_full(
            "U6-003",
            "exec-w-3-1",
            "executor",
            Stage::Execute,
            Vec::new(),
            Vec::new(),
            vec!["PATH".to_string()],
        );
        let host_env: HashMap<String, String> = [
            ("PATH".to_string(), "/a:/b".to_string()),
            ("UNRELATED".to_string(), "noise".to_string()),
            ("FAKE_SECRET_TOKEN".to_string(), "sentinel".to_string()),
        ]
        .into();
        let policy = DagEnvPolicy::from_declared(["PATH"]);
        let ctx = build_prompt_context(&d, &policy, &host_env);
        assert_eq!(ctx.child_env.len(), 1);
        assert_eq!(ctx.child_env.get("PATH").map(String::as_str), Some("/a:/b"));
        assert!(!ctx.child_env.contains_key("UNRELATED"));
        assert!(!ctx.child_env.contains_key("FAKE_SECRET_TOKEN"));
    }
}
