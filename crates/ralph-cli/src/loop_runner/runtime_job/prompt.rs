//! 2026-09-03-0959 plan U6 (D7 / S7): materialise a stable
//! `(unit_key, job_id, hat, stage, allowed_paths,
//!  forbidden_paths, env_allowlist_keys)` slice the kernel hands
//! to the subprocess port.
//!
//! The slice is intentionally **plain data** — no trait objects,
//! no file handles, no env vars. The kernel builds it once per
//! invocation and the port may inspect, log (with sanitised
//! redaction — see `dag_inspect`'s forbidden-substring list for
//! the style), or serialise it. The legacy wave worker keeps its
//! own prompt builder; this type is DAG-only.

#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use super::JobDescriptor;

/// Plain-data prompt context. Stable, `Clone`, `PartialEq`,
/// `Eq` — every field is `String` or `Vec<PathBuf>`. The
/// descriptor's `changed_paths` is intentionally **omitted** from
/// the prompt context: it is part of the integration authorisation
/// surface (U7's concern), not the worker prompt.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContext {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: String,
    pub allowed_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
    pub env_allowlist_keys: Vec<String>,
}

#[cfg(test)]
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

/// Build a prompt context from a descriptor. Pure function: same
/// descriptor always yields the same prompt context. `stage` is
/// rendered as the stable `Stage::as_str` so downstream consumers
/// can branch on the value without re-implementing the mapping.
///
/// `#[cfg(test)]` because the only consumers are the per-module
/// `tests` mod and the integration tests in `runtime_job::tests`.
/// U7 will promote it once a real subprocess backend is wired.
#[cfg(test)]
pub fn build_prompt_context(descriptor: &JobDescriptor) -> PromptContext {
    PromptContext {
        unit_key: descriptor.unit_key.clone(),
        job_id: descriptor.job_id.clone(),
        hat: descriptor.hat.clone(),
        stage: descriptor.stage.as_str().to_string(),
        allowed_paths: descriptor.allowed_paths.clone(),
        forbidden_paths: descriptor.forbidden_paths.clone(),
        env_allowlist_keys: descriptor.env_allowlist_keys.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::runtime_job::Stage;
    use std::path::PathBuf;

    /// Build is a pure function: same descriptor → same context.
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
        let a = build_prompt_context(&d);
        let b = build_prompt_context(&d);
        assert_eq!(a, b);
    }

    /// Stage serialises as the stable short string, not the
    /// `Debug` form (which would leak `"Execute"` only by chance —
    /// the explicit `as_str` mapping is the contract).
    #[test]
    fn stage_uses_stable_short_string() {
        let d = JobDescriptor::new("U6-001", "exec", "executor", Stage::Review);
        let ctx = build_prompt_context(&d);
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
        let ctx = build_prompt_context(&d);
        assert_eq!(ctx.allowed_paths, allowed);
        assert_eq!(ctx.forbidden_paths, forbidden);
        assert_eq!(ctx.env_allowlist_keys, env);
    }
}
