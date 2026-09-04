//! 2026-09-03-0959 plan U6 (R7 / R8 / S7-S10 / D7-D9 / D11-D12 /
//! E10-E12 / E14-E15): generic job kernel + per-Unit pipeline.
//!
//! This module owns the *types and rules* the runtime job kernel
//! needs to drive the per-Unit sequence `Execute → Review → Verify`
//! in `scheduler_mode: dag`. The kernel itself (subprocess launch,
//! pre-fence, deadline collection, ingress) lives in sibling
//! modules so each file stays under the HARD RULE 5000-line
//! ceiling. **No I/O, no async, no FS** — every type here is
//! pure CPU so unit tests can construct and assert against it
//! without spinning up a CLI / event loop / store.
//!
//! Layering (matches the plan §Unit 6 §17 file list):
//!   - `environment` — env allowlist (`DagEnvAllowlist` +
//!     `DagEnvPolicy`) and a `LegacyEnvPolicy` marker that
//!     documents the existing wave worker env path **without**
//!     consuming it (the wave worker is U6-out-of-scope).
//!   - `process` — `JobProcessPort` trait + test fakes.
//!   - `prompt` — `build_prompt_context(descriptor)` materialises
//!     the stable `(unit_key, job_id, hat, stage, allowed_paths,
//!     forbidden_paths, env_allowlist_keys)` slice the kernel
//!     hands to the port.
//!   - `worker` — `run_job(descriptor, port, env_policy)` runs one
//!     kernel invocation with pre-fence, heartbeat, deadline.
//!   - `result_ingress` — `submit_accepted_result(event_loop,
//!     descriptor, process_result)` constructs the typed accepted
//!     event and runs it through the real public gate
//!     (`ralph_core::event_loop::emit_schema_gate::check`).
//!
//! The pipeline that *uses* these types to advance a Unit through
//! its stages lives in the sibling module `dag_scheduler::jobs`.
//! Keeping the types here (kernel) and the orchestration logic
//! there (pipeline) lets the kernel be reused by future Units
//! without re-exporting pipeline state.

pub mod environment;
pub mod process;
pub mod prompt;
pub mod result_ingress;
pub mod worker;

// Sub-module types are reachable through their sub-module paths
// (e.g. `runtime_job::environment::DagEnvAllowlist`). Tests use
// `use super::environment::{...}` etc., so no flat re-exports are
// needed at the module level — they would be dead code.

#[cfg(test)]
mod tests;

#[cfg(test)]
use std::path::PathBuf;

/// Stages the runtime drives a Unit through in `scheduler_mode: dag`.
///
/// The transition rule is strict: `Execute → Review → Verify`.
/// `Review → Execute` and `Execute → Verify` are illegal — both
/// are fail-closed in `Stage::can_advance_to` and in
/// `JobPipeline::advance` (see `dag_scheduler::jobs`).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Execute,
    Review,
    Verify,
}

#[cfg(test)]
impl Stage {
    /// Strict next-stage gate. `Execute → Review` and
    /// `Review → Verify` are the only legal forward moves; same
    /// stage, backward moves, and skip moves (`Execute → Verify`)
    /// are rejected.
    pub fn can_advance_to(self, other: Stage) -> bool {
        matches!(
            (self, other),
            (Stage::Execute, Stage::Review) | (Stage::Review, Stage::Verify)
        )
    }

    /// Short stable string the kernel + ingress surface to logs and
    /// event payloads. Sanitised — never contains secrets.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Execute => "execute",
            Stage::Review => "review",
            Stage::Verify => "verify",
        }
    }
}

#[cfg(test)]
impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One per-Unit kernel invocation descriptor. The tuple
/// `(unit_key, job_id, hat, stage)` is the durable identity the
/// CAS guard downstream compares against (R7 / S9 / E10). Anything
/// that mutates the descriptor (e.g. promoting `Execute → Review`)
/// MUST mint a *new* `JobToken` so the old token's CAS slot is
/// closed.
///
/// The descriptor also pins the path + env policy for this
/// invocation: the kernel refuses to launch unless every declared
/// `allowed_paths` entry is reachable and every `forbidden_paths`
/// entry is absent (pre-fence, see `process::JobProcessPort`).
///
/// `attempt` is the monotonic per-Unit counter incremented on every
/// review rejection. `JobPipeline` reads it back when minting the
/// next attempt's token.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDescriptor {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: Stage,
    pub allowed_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
    pub changed_paths: Vec<PathBuf>,
    pub env_allowlist_keys: Vec<String>,
    pub attempt: u64,
}

#[cfg(test)]
impl JobDescriptor {
    /// Minimal constructor used by tests and the legacy stub. Use
    /// `new_full` when the kernel needs the path / env policy.
    pub fn new(
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
    ) -> Self {
        Self {
            unit_key: unit_key.into(),
            job_id: job_id.into(),
            hat: hat.into(),
            stage,
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
            changed_paths: Vec::new(),
            env_allowlist_keys: Vec::new(),
            attempt: 0,
        }
    }

    pub fn new_full(
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
        allowed_paths: Vec<PathBuf>,
        forbidden_paths: Vec<PathBuf>,
        env_allowlist_keys: Vec<String>,
    ) -> Self {
        Self {
            unit_key: unit_key.into(),
            job_id: job_id.into(),
            hat: hat.into(),
            stage,
            allowed_paths,
            forbidden_paths,
            changed_paths: Vec::new(),
            env_allowlist_keys,
            attempt: 0,
        }
    }

    /// Builder-style: stamp the result of U7's changed-path
    /// computation onto the descriptor so the downstream
    /// integration half (U7's concern) can authorise against it.
    /// In U6 the descriptor simply *carries* the value; the
    /// authorisation gate is wired in U7.
    pub fn with_changed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.changed_paths = paths;
        self
    }

    /// Builder-style: bump the per-Unit attempt counter on every
    /// review rejection. The pipeline mints a fresh `JobToken`
    /// with this counter so the previous token's CAS slot is
    /// closed.
    pub fn with_attempt(mut self, attempt: u64) -> Self {
        self.attempt = attempt;
        self
    }

    pub fn unit_key(&self) -> &str {
        &self.unit_key
    }
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
    pub fn hat(&self) -> &str {
        &self.hat
    }
    pub fn stage(&self) -> Stage {
        self.stage
    }
}

/// The CAS guard the kernel hands to the worker. Bound to the
/// full `(unit_key, job_id, hat, stage, attempt)` tuple so a stolen
/// or stale token cannot validate against a different slot.
///
/// The legacy 3-RED test in `runtime_job_stub.rs` uses the
/// free-floating `mint(unit_key, stage)` form which is retained
/// here as a 2-arg mint that defaults `attempt = 0`. Production
/// callers (the pipeline) use `mint_attempt` so the attempt
/// counter is explicit.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobToken {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: Stage,
    pub attempt: u64,
}

#[cfg(test)]
impl JobToken {
    /// Production mint: pins every CAS slot so the guard can
    /// reject cross-unit / cross-stage / cross-hat / cross-attempt
    /// reuse.
    pub fn mint_attempt(
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
        attempt: u64,
    ) -> Self {
        Self {
            unit_key: unit_key.into(),
            job_id: job_id.into(),
            hat: hat.into(),
            stage,
            attempt,
        }
    }

    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    /// Legacy 2-slot check matching the stub's contract: unit_key
    /// + stage. Production callers use `belongs_to_full` (or
    /// `matches(&descriptor)`) so the hat + attempt slots are
    /// also compared.
    pub fn belongs_to(&self, unit_key: &str, stage: Stage) -> bool {
        self.unit_key == unit_key && self.stage == stage
    }

    /// Full 4-slot check used by the ingress: unit_key, stage,
    /// hat, attempt must ALL match.
    pub fn belongs_to_full(&self, unit_key: &str, stage: Stage, hat: &str, attempt: u64) -> bool {
        self.unit_key == unit_key
            && self.stage == stage
            && self.hat == hat
            && self.attempt == attempt
    }

    /// Descriptor-driven check. Used by `result_ingress` so the
    /// kernel only advances state when every CAS slot agrees.
    pub fn matches(&self, descriptor: &JobDescriptor) -> bool {
        self.belongs_to_full(
            &descriptor.unit_key,
            descriptor.stage,
            &descriptor.hat,
            descriptor.attempt,
        )
    }
}

/// Result of one kernel invocation. The kernel writes one
/// `ProcessResult` per job (executor / reviewer / verifier). The
/// ingress feeds the `payload` through the real
/// `emit_schema_gate::check` — *not* a mock — and refuses to
/// advance state if the gate rejects.
///
/// `payload_bytes` is the byte length of the rendered payload
/// JSON; the ingress uses it to enforce the 64 KiB ceiling the
/// plan §7 U6 #11 mandates.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub payload: serde_json::Value,
    pub payload_bytes: usize,
    pub exit_code: Option<i32>,
    pub pid: i32,
    pub elapsed_ms: u64,
}

#[cfg(test)]
impl ProcessResult {
    pub fn new(
        payload: serde_json::Value,
        exit_code: Option<i32>,
        pid: i32,
        elapsed_ms: u64,
    ) -> Self {
        let payload_bytes = serde_json::to_vec(&payload).map(|v| v.len()).unwrap_or(0);
        Self {
            payload,
            payload_bytes,
            exit_code,
            pid,
            elapsed_ms,
        }
    }
}

/// Maximum payload size the ingress accepts. 64 KiB matches plan
/// §7 U6 #11: "result payload > 64 KiB must be refused at the
/// ingress gate with a typed error."
///
/// `#[cfg(test)]` because the only consumer is the ingress
/// module (also `#[cfg(test)]` for U6) and the `tests` mod that
/// pins the cap. U7 promotes it once the real subprocess
/// backend reads payload byte counts.
#[cfg(test)]
pub const MAX_INGRESS_PAYLOAD_BYTES: usize = 64 * 1024;

/// All typed failure modes the runtime job kernel can surface.
///
/// Sanitisation: no variant carries an env var name or value. The
/// allowlist silently drops undeclared env entries (see
/// `environment::DagEnvPolicy::filter_child_env`) so a rejection
/// can never echo back what was hidden. The only error that
/// mentions a path is `PreFenceFailed(String)` where `String` is
/// the port's own error message (test fakes keep it short and
/// sanitised).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeJobError {
    IllegalStageTransition {
        from: Stage,
        to: Stage,
    },
    TokenMismatch {
        expected_unit: String,
        given_unit: String,
        expected_stage: Stage,
        given_stage: Stage,
        expected_hat: String,
        given_hat: String,
        expected_attempt: u64,
        given_attempt: u64,
    },
    PayloadTooLarge {
        bytes: usize,
        cap: usize,
    },
    PolicyRejected {
        missing: Vec<String>,
    },
    PoolExhausted {
        stage: Stage,
        requested: u32,
        cap: u32,
    },
    GlobalCapExceeded {
        requested: u32,
        cap: u32,
    },
    PreFenceFailed(String),
    CollectFailed(String),
    Blocked {
        reason: String,
        unit_key: String,
    },
    HeartbeatTimeout {
        stage: Stage,
        elapsed_ms: u64,
        cap_ms: u64,
    },
}

#[cfg(test)]
impl std::fmt::Display for RuntimeJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeJobError::IllegalStageTransition { from, to } => {
                write!(f, "illegal stage transition: {from} -> {to}")
            }
            RuntimeJobError::TokenMismatch {
                expected_unit,
                given_unit,
                expected_stage,
                given_stage,
                expected_hat,
                given_hat,
                expected_attempt,
                given_attempt,
            } => write!(
                f,
                "token mismatch: expected ({expected_unit}, {expected_stage}, {expected_hat}, attempt={expected_attempt}), got ({given_unit}, {given_stage}, {given_hat}, attempt={given_attempt})"
            ),
            RuntimeJobError::PayloadTooLarge { bytes, cap } => write!(
                f,
                "result payload {bytes} bytes exceeds ingress cap {cap} bytes"
            ),
            RuntimeJobError::PolicyRejected { missing } => {
                write!(f, "policy rejection: missing required fields [")?;
                for (i, m) in missing.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(m)?;
                }
                f.write_str("]")
            }
            RuntimeJobError::PoolExhausted {
                stage,
                requested,
                cap,
            } => write!(
                f,
                "pool exhausted for stage {stage}: requested {requested}, cap {cap}"
            ),
            RuntimeJobError::GlobalCapExceeded { requested, cap } => {
                write!(f, "global cap exceeded: requested {requested}, cap {cap}")
            }
            RuntimeJobError::PreFenceFailed(msg) => write!(f, "pre-fence failed: {msg}"),
            RuntimeJobError::CollectFailed(msg) => write!(f, "collect failed: {msg}"),
            RuntimeJobError::Blocked { reason, unit_key } => {
                write!(f, "unit {unit_key} blocked: {reason}")
            }
            RuntimeJobError::HeartbeatTimeout {
                stage,
                elapsed_ms,
                cap_ms,
            } => write!(
                f,
                "heartbeat timeout for stage {stage}: elapsed {elapsed_ms} ms, cap {cap_ms} ms"
            ),
        }
    }
}

#[cfg(test)]
impl std::error::Error for RuntimeJobError {}
