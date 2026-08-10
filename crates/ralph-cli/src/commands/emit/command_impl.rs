//! Production implementation of `ralph emit` (entry points, helpers, gates).
//!
//! Lifted verbatim from `commands/emit.rs` lines 98–2215 of HEAD `7909f159`.
//! Only the file boundary changed — every item, body, and `pub`/`pub(crate)`
//! visibility is preserved so existing call sites (including `main.rs`,
//! `wave.rs`, `commands/u2_wave_system_field_tests.rs`) continue to compile.

use super::EmitArgs;
use crate::cli::{
    ColorMode, ConfigSource, HatsSource, resolve_emit_path, resolve_marker_target,
    resolve_workspace_root, urgent_steer_path_from_workspace,
};
use crate::config_resolution;
use crate::display::colors;
use crate::policy_check::{
    PolicyCheckFlags, ValidationFailure, resolve_policy_check_mode,
    resolve_policy_check_mode_with_ctx,
};
use anyhow::{Context, Result};
use ralph_core::config::HatExecutionMode;
use ralph_core::emit_schema_hint::fix_hint_for_hat_topic;
use ralph_core::preset::engine::ProtocolView;
use ralph_core::{
    RalphConfig, UrgentSteerStore, ViolationType,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
        RecoveryDiagnosisEnvelope, RecoveryJournalEntry,
    },
};
use ralph_proto::{Hat, HatId, Topic};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Plan 001 §4.3 C3: build the hat-scoped fix hint shown to the agent when
/// a pre-publish check rejects the event. The hat's authorised topics are
/// looked up via the loaded `RalphConfig::hats`. When the hat is unknown,
/// or not authorised for the topic, only the bare policy message is shown
/// — we never leak another hat's payload shape.
fn format_fix_hint(config: &RalphConfig, hat_id: Option<&str>, topic: &str) -> String {
    let Some(hat_id) = hat_id else {
        return "Set --hat <your-hat-id> (or RALPH_CURRENT_HAT) so the CLI can \
                suggest a fix."
            .to_string();
    };

    let Some(hat_config) = config.hats.get(hat_id) else {
        return format!(
            "Hat `{hat_id}` is not registered in the loaded preset; \
             pass --hat to one of the preset's registered hats."
        );
    };

    // Build a minimal `Hat` from the preset's `HatConfig`. Only the
    // `publishes` list matters for `fix_hint_for_hat_topic`; the rest
    // is dropped on the floor inside that helper.
    let publishes: Vec<Topic> = hat_config
        .publishes
        .iter()
        .map(|t| Topic::new(t.clone()))
        .collect();
    let schema = match config
        .event_loop
        .event_policy
        .as_ref()
        .and_then(|p| p.schemas.get(topic))
    {
        Some(s) => s,
        None => return String::new(),
    };
    let hat = Hat {
        id: HatId::new(hat_id),
        name: hat_config.name.clone(),
        description: String::new(),
        subscriptions: vec![],
        publishes,
        instructions: String::new(),
    };
    fix_hint_for_hat_topic(&hat, topic, schema).unwrap_or_default()
}

/// Resolve provenance values for an emitted event.
///
/// Priority: CLI flag > env var lookup > empty.
pub fn resolve_provenance<F>(
    cli_hat: Option<String>,
    cli_triggered: Option<String>,
    cli_source: Option<String>,
    env_lookup: F,
) -> (Option<String>, Option<String>, Option<String>)
where
    F: Fn(&str) -> Option<String>,
{
    let hat = cli_hat
        .or_else(|| env_lookup("RALPH_CURRENT_HAT"))
        .filter(|s| !s.is_empty());
    let triggered = cli_triggered
        .or_else(|| env_lookup("RALPH_TRIGGERED_HAT"))
        .filter(|s| !s.is_empty());
    let source = cli_source
        .or_else(|| env_lookup("RALPH_EVENT_SOURCE"))
        .filter(|s| !s.is_empty());
    (hat, triggered, source)
}

/// Plan 001 §4.3 P1-8: log + surface to stderr when the recovery
/// envelope write fails, so operators have a chance to fix the
/// underlying disk/permissions issue. The user-facing bail still
/// fires — only the audit trace is at risk.
fn record_cli_emit_rejection(
    workspace_root: &Path,
    topic: &str,
    hat: Option<&str>,
    finding: &ralph_core::PolicyFinding,
) {
    if let Err(e) = write_cli_emit_recovery_envelope(workspace_root, topic, hat, finding) {
        tracing::warn!("Failed to write CLI emit recovery envelope: {:#}", e);
        eprintln!(
            "note: could not record rejection audit to .ralph/recovery.jsonl — \
             check disk permissions. Underlying error: {e}"
        );
    }
}

/// Re-exported from [`crate::policy_check`] so existing callers
/// keep their `crate::commands::emit::PolicyCheckMode` import path.
pub use crate::policy_check::PolicyCheckMode;

/// Decides the policy-check mode based on CLI arguments and loaded config.
///
/// Thin wrapper over [`crate::policy_check::resolve_policy_check_mode`]
/// that bridges `EmitArgs` → `PolicyCheckFlags` for symmetry with the
/// shared helper. Preserved for backward compatibility with existing
/// callers and tests.
///
/// U15: the agent-context-aware helper accepts `is_agent_context` so
/// emit callers that know whether the call came from an agent (true)
/// or from the human CLI (false) can pass that signal through. Most
/// callers should prefer the ctx-aware form.
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 emit-path parity
// (CLI vs agent policy-check invocation). Pinning the signature now
// avoids churn when U15 lands.
#[allow(dead_code)]
pub fn should_policy_check_emit(args: &EmitArgs, config: Option<&RalphConfig>) -> PolicyCheckMode {
    let flags = PolicyCheckFlags {
        policy_check: args.policy_check,
        no_policy_check: args.no_policy_check,
    };
    resolve_policy_check_mode(&flags, config)
}

/// U15 ctx-aware emit policy-check resolver. Detects the operation
/// context from the live environment and forwards `is_agent_context`
/// to the shared helper.
pub fn should_policy_check_emit_with_ctx(
    args: &EmitArgs,
    config: Option<&RalphConfig>,
    workspace_root: &std::path::Path,
) -> PolicyCheckMode {
    let flags = PolicyCheckFlags {
        policy_check: args.policy_check,
        no_policy_check: args.no_policy_check,
    };
    let ctx = crate::operation_guard::OperationContext::detect(workspace_root.to_path_buf());
    resolve_policy_check_mode_with_ctx(&flags, config, ctx.is_agent_context)
}

/// Emit an event to the current run's events file with proper JSON formatting.
///
/// This command provides a deterministic way for agents to emit events without
/// risking malformed JSONL from manual echo commands. All JSON serialization
/// is handled via serde_json, ensuring proper escaping of payloads.
///
/// Events are written to the path specified in `.ralph/current-events` marker file
/// (created by `ralph run`), or falls back to `.ralph/events.jsonl` if no marker exists.
/// Heuristic: does this payload look like a JSON object or array?
///
/// Used by `emit_command_with_root` to auto-detect JSON payloads when the
/// agent omits the `--json` flag.  This prevents structured events such as
/// `work.done` from being stored as plain strings and then rejected by the
/// execution-contract validator.
pub fn looks_like_json(payload: &str) -> bool {
    let trimmed = payload.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

/// U5 (plan 2026-07-30-004): canonicalize a payload string for evaluation
/// token hashing. Parses the payload as JSON and re-serializes it so that
/// whitespace / key-order variations of the SAME logical payload produce the
/// same token (serde_json maps are key-sorted, so the output is
/// deterministic). When the payload is empty or not valid JSON, the trimmed
/// raw string is used verbatim so non-JSON payloads still get a stable token.
fn canonical_payload_for_token(payload: &str) -> String {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => serde_json::to_string(&value).unwrap_or_else(|_| trimmed.to_string()),
        Err(_) => trimmed.to_string(),
    }
}

/// U5: compute the evaluation token binding a `(hat, topic, payload)` tuple
/// to a specific Effective Execution Contract revision. Deterministic: the
/// same inputs always yield the same hex digest. Folding the contract
/// revision (the compiled contract's `contract_digest`) into the hash means a
/// token minted against a stale config is rejected as soon as the config
/// changes. The same function mints the token on `--policy-check` and
/// recomputes it for verification on apply, so the two paths cannot drift.
fn compute_policy_check_token(
    hat: &str,
    topic: &str,
    payload: &str,
    contract_revision: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let canonical = canonical_payload_for_token(payload);
    let mut hasher = Sha256::new();
    hasher.update(b"u5-policy-check-token-v1\n");
    hasher.update(hat.as_bytes());
    hasher.update(b"\n");
    hasher.update(topic.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    hasher.update(b"\n");
    hasher.update(contract_revision.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// U5 (plan 2026-07-30-004): resolved state of the unified agent-CLI
/// capability + evaluation-token gate for a single emit invocation.
///
/// The gate is ACTIVE only in an agent context (`RALPH_CURRENT_HAT` set) AND
/// only when the preset has no event-policy pipeline validating the emit
/// (`unified_active == false`). When a full `event_policy` is active, the
/// unified validation pipeline upstream IS the contract enforcement and this
/// gate is redundant; when no `event_policy` exists, this gate is the sole
/// enforcement, so it (a) denies `(hat, topic)` pairs the Effective Execution
/// Contract does not allow and (b) requires an evaluation token proving the
/// payload passed `--policy-check` against the same contract revision.
/// The gate also stands down for the orchestrator pseudo-hat `ralph`
/// (hatless loops), wave workers, and presets with no hats — see
/// [`U5Gate::resolve`].
///
/// **U3 (2026-08-03-001-fix-opac-high-confidence-gates-plan):**
/// when the execution contract fails to compile in an agent governed
/// context, the gate now produces an explicit
/// [`U5GateState::CompileFailed`] state and refuses BOTH the
/// capability check and the token check with the same stable
/// `contract_compile_failed` reason. The previous
/// `resolved: Option<…>` collapsed compile failure into a silent
/// stand-down that let the capability gate skip and the token
/// gate return `policy_check_token_mismatch` (or fall through
/// with an empty token) — the exact "fail-open" anti-pattern
/// S6 calls out.
enum U5GateState {
    /// The gate does not apply to this invocation (human CLI,
    /// pseudo-hat `ralph`, wave worker, or preset without hats).
    Inactive,
    /// The contract compiled cleanly; the gate is enforcing.
    /// Boxed to keep the enum small (clippy `large_enum_variant`);
    /// all access goes through `&self.state` pattern matches.
    Active {
        resolved: Box<ralph_core::execution_contract::ResolvedRuntimeConfig>,
    },
    /// The contract failed to compile. The gate denies with
    /// `contract_compile_failed` BEFORE evaluating capability or
    /// token, and BEFORE the emit dry-run return / disk write —
    /// no event, idempotency row, or ticket side effect occurs.
    CompileFailed { reason: String },
}

pub(super) struct U5Gate {
    state: U5GateState,
}

impl U5Gate {
    /// Resolve the gate for this invocation. `env_hat_set` is whether
    /// `RALPH_CURRENT_HAT` is present (the agent-context signal); `hat` is the
    /// resolved emitting hat; `topic` is the EFFECTIVE (post-desugar) topic.
    /// Stand-down conditions: the gate governs only REAL agent hats in
    /// governed presets. It stays inactive for:
    /// - the orchestrator pseudo-hat `ralph` (hatless loops inject
    ///   `RALPH_CURRENT_HAT=ralph`; the contract compiled there has empty
    ///   `emit_allows`, so gating would deny `LOOP_COMPLETE` and the loop
    ///   could never terminate),
    /// - wave workers (`RALPH_WAVE_WORKER` set) — their rejections are owned
    ///   by the wave-channel guard, which must surface its own reason, and
    /// - presets defining no hats (there is nothing to govern).
    pub(super) fn resolve(
        env_hat_set: bool,
        unified_active: bool,
        config: Option<&RalphConfig>,
        hat: Option<&str>,
        _topic: &str,
        _payload: &str,
    ) -> Self {
        let stands_down = hat == Some("ralph")
            || std::env::var("RALPH_WAVE_WORKER").is_ok()
            || config.is_some_and(|c| c.hats.is_empty());
        let active = env_hat_set && !unified_active && config.is_some() && !stands_down;
        if !active {
            return Self {
                state: U5GateState::Inactive,
            };
        }
        // Compile the config to obtain the contract + revision. The
        // U3 rule: agent context + compile failure is a hard deny
        // — never fall back to inactive and never let the
        // capability / token checks slip through with a half-built
        // gate.
        let cfg = config.expect("active implies config.is_some()");
        match ralph_core::execution_contract::compile(cfg.clone()) {
            Ok(resolved) => Self {
                state: U5GateState::Active {
                    resolved: Box::new(resolved),
                },
            },
            Err(error) => Self {
                state: U5GateState::CompileFailed {
                    reason: format!("contract_compile_failed: {}", error),
                },
            },
        }
    }

    /// U3: explicit compile-failure check. Both `--policy-check`
    /// and Apply must surface this with the stable
    /// `contract_compile_failed` reason and a hint that names the
    /// compilation finding. Returns `Some((reason, hint))` when
    /// the gate is in the CompileFailed state; `None` otherwise.
    pub(super) fn compile_failure(&self) -> Option<(&'static str, String)> {
        match &self.state {
            U5GateState::CompileFailed { reason } => {
                Some(("contract_compile_failed", reason.clone()))
            }
            _ => None,
        }
    }

    /// Enforce the capability decision. Returns an error message when the
    /// contract denies `(hat, topic)`; `None` means proceed.
    pub(super) fn capability_denied(&self, hat: Option<&str>, topic: &str) -> Option<String> {
        if let Some((code, hint)) = self.compile_failure() {
            return Some(format!(
                "{code}: the Effective Execution Contract could not be compiled for this emit, so neither capability nor token checks can be evaluated safely. Re-validate the preset config (`{hint}`) and try again."
            ));
        }
        let resolved = match &self.state {
            U5GateState::Active { resolved } => resolved,
            _ => return None,
        };
        let hat_id = hat?;
        use ralph_core::execution_contract::EmitDecision;
        match resolved.contract().emit_decision(hat_id, topic) {
            EmitDecision::Allow => None,
            EmitDecision::Deny => Some(format!(
                "capability_denied: hat '{hat_id}' cannot emit '{topic}' per the Effective \
                 Execution Contract. Only topics this hat publishes (or terminal topics it \
                 owns) are allowed. Run `ralph emit --schema {topic}` to inspect the contract \
                 for this topic, or emit a topic your hat is authorised for."
            )),
        }
    }

    /// Enforce the evaluation token on the apply path. Returns `(code,
    /// message)` when the token is missing or mismatched; `None` means the
    /// apply may proceed.
    ///
    /// U3: a CompileFailed gate denies with `contract_compile_failed`
    /// BEFORE the token check. The previous fall-through returned
    /// `policy_check_token_mismatch` against an empty expected token,
    /// which is misleading and gave agents no actionable signal.
    pub(super) fn token_violation(
        &self,
        hat: Option<&str>,
        topic: &str,
        payload: &str,
        provided: Option<&str>,
    ) -> Option<(&'static str, String)> {
        if let Some((code, hint)) = self.compile_failure() {
            return Some((
                code,
                format!(
                    "{code}: the Effective Execution Contract could not be compiled for this emit, so the evaluation token cannot be verified. Re-validate the preset config (`{hint}`) and try again."
                ),
            ));
        }
        let resolved = match &self.state {
            U5GateState::Active { resolved } => resolved,
            _ => return None,
        };
        let expected = hat
            .map(|hat_id| compute_policy_check_token(hat_id, topic, payload, resolved.digest()))
            .unwrap_or_default();
        match provided {
            None => Some((
                "missing_policy_check_token",
                "missing_policy_check_token: this emit runs in an agent context with no \
                 event-policy pipeline, so it requires an evaluation token proving the \
                 payload was pre-checked against the same contract revision. Run \
                 `ralph emit <topic> --policy-check -j '<payload>'` first, then re-run the \
                 emit with `--policy-check-token <token>` using the `policy_check_token` \
                 value it prints."
                    .to_string(),
            )),
            Some(provided) => {
                if provided == expected && !expected.is_empty() {
                    None
                } else {
                    Some((
                        "policy_check_token_mismatch",
                        "policy_check_token_mismatch: the supplied --policy-check-token does \
                         not match this (hat, topic, payload, contract revision). The token is \
                         bound to the exact payload and the current contract revision; if the \
                         config changed or the payload differs, re-run \
                         `ralph emit <topic> --policy-check -j '<payload>'` and use the fresh \
                         `policy_check_token` it prints."
                            .to_string(),
                    ))
                }
            }
        }
    }

    /// Resolve the evaluation token that `token_violation` would compare
    /// against. Returns an empty string when the gate is not active
    /// (used to preserve the legacy "empty expected = token_violation" shape
    /// for callers that already short-circuit on `Inactive`).
    pub(super) fn token(&self, hat: Option<&str>, topic: &str, payload: &str) -> Option<String> {
        match &self.state {
            U5GateState::Active { resolved } => hat.map(|hat_id| {
                compute_policy_check_token(hat_id, topic, payload, resolved.digest())
            }),
            _ => None,
        }
    }

    /// Resolve the contract revision for callers that need it (e.g.
    /// the JSON envelope in the `--policy-check` success path).
    pub(super) fn resolved_digest(&self) -> Option<String> {
        match &self.state {
            U5GateState::Active { resolved } => Some(resolved.digest().to_string()),
            _ => None,
        }
    }
}

/// 2026-07-27-004 plan U2 (R5-R7 / D3 / D8): apply runtime-owned
/// system-field normalization to a wave worker's payload.
///
/// Contract:
/// - When the process is bound to a registry-validated wave worker
///   context (`wave_worker == true`) AND both `wave_id_env` and
///   `slot_index_env` are present (the dispatcher injects them as a
///   single handshake — see
///   `loop_runner/wave/dispatcher.rs::spawn_worker_env`), inject the
///   `wave_id` and `slot_index` system fields into the payload.
/// - When the worker payload already contains `wave_id` or
///   `slot_index` (even with matching values), reject with
///   `system_field_owned_by_runtime` so the contract stays
///   symmetric and auditable — Agent-fillable system fields create
///   drift between the dispatcher's authoritative context and the
///   payload's stamp.
/// - When `wave_worker == false` (regular hat), pass through
///   unchanged; the system fields are not relevant to non-wave
///   payloads.
///
/// The helper is `pub(crate)` so the wave worker tests under
/// `ralph-cli/src/cli::emit_path` and `commands::emit::tests` can
/// pin the contract directly. It returns the (possibly mutated)
/// payload and never panics; rejections are surfaced via
/// `anyhow::Error` so the calling emit path can route them through
/// the existing policy-check recovery envelope.
pub(crate) fn normalize_wave_worker_system_fields(
    mut payload: serde_json::Value,
    wave_worker: bool,
    wave_id_env: Option<&str>,
    slot_index_env: Option<u32>,
) -> Result<serde_json::Value> {
    if !wave_worker {
        // R7: non-wave hat payloads are unchanged; the existing
        // payload contract still applies.
        return Ok(payload);
    }
    let Some(public_wave_id) = wave_id_env else {
        // Handshake already gated at the call site
        // (`:390`); a missing `RALPH_WAVE_ID` here means the
        // upstream guard let through a malformed process. We
        // keep this conservative and pass through unchanged so
        // the previously enforced error still surfaces.
        return Ok(payload);
    };
    let Some(slot_index) = slot_index_env else {
        return Ok(payload);
    };

    let Some(obj) = payload.as_object_mut() else {
        anyhow::bail!(
            "system_field_owned_by_runtime: wave worker payload must be a JSON object, \
             but received {payload_kind}. Refusing to inject wave_id/slot_index \
             into a non-object payload — the agent must emit a structured object.",
            payload_kind = match payload {
                serde_json::Value::Null => "null",
                serde_json::Value::Bool(_) => "bool",
                serde_json::Value::Number(_) => "number",
                serde_json::Value::String(_) => "string",
                serde_json::Value::Array(_) => "array",
                serde_json::Value::Object(_) => "object",
            }
        );
    };

    // R6 / D8: an agent that pre-stamps `wave_id` / `slot_index`
    // with a value that DISAGREES with the registry-bound
    // context is rejected as `system_field_owned_by_runtime` —
    // the runtime owns these fields. A matching value is
    // accepted (R6 specifically targets the conflict, not all
    // stamps), so workers that emit the runtime-bound identity
    // alongside their business payload — e.g. the
    // `u7_real_ralph_emit_writes_marker_signed_channel`
    // dispatcher-signed-channel regression test — keep passing.
    if let Some(Value::String(provided)) = obj.get("wave_id")
        && provided != public_wave_id
    {
        anyhow::bail!(
            "system_field_owned_by_runtime: payload 'wave_id' ({provided}) disagrees with the \
             registry-bound context ({public_wave_id}); the runtime owns this field — drop it \
             from the payload or align with RALPH_WAVE_ID"
        );
    }
    if let Some(provided) = obj.get("slot_index")
        && let Some(num) = provided.as_u64()
        && num != u64::from(slot_index)
    {
        anyhow::bail!(
            "system_field_owned_by_runtime: payload 'slot_index' ({provided}) disagrees with the \
             registry-bound context ({slot_index}); the runtime owns this field — drop it \
             from the payload or align with RALPH_WAVE_INDEX"
        );
    }

    // Always set the canonical system fields. A matching value
    // that was already in the payload is overwritten by the same
    // value (idempotent overwrite is safe); a missing field is
    // filled in. The conflict branch above guarantees we never
    // inject a value that disagrees with the registry-bound
    // context.
    obj.insert(
        "wave_id".to_string(),
        serde_json::Value::String(public_wave_id.to_string()),
    );
    obj.insert(
        "slot_index".to_string(),
        serde_json::Value::Number(serde_json::Number::from(slot_index)),
    );
    Ok(payload)
}

/// Write a recovery envelope to `.ralph/recovery.jsonl` when the CLI
/// emit precheck rejects an event. The envelope captures the rejected
/// topic, the offending hat (if known), and the policy finding so
/// operators and `ralph diagnose` have an audit trace.
///
/// Errors are returned but expected to be logged (not propagated) by
/// the caller: the original validation error must still reach the user.
fn write_cli_emit_recovery_envelope(
    workspace_root: &Path,
    topic: &str,
    source_hat: Option<&str>,
    finding: &ralph_core::PolicyFinding,
) -> Result<()> {
    let recovery_path = workspace_root.join(".ralph/recovery.jsonl");
    if let Some(parent) = recovery_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create recovery directory: {}", parent.display())
        })?;
    }

    let (severity, outcome, reason_code, source) = match &finding.violation_type {
        ralph_core::ViolationType::PayloadTypeMismatch { .. } => (
            DiagnosisSeverity::Critical,
            DiagnosisOutcome::NotRetriable,
            "payload_contract_violation".to_string(),
            DiagnosisSource::CliEmit,
        ),
        // P1#13 fix: dimension-mismatch rejections (CLI precheck
        // for wave worker's `review.dimension.done` value) are
        // surfaced as `WaveDimensionGuard` envelopes so
        // `ralph diagnose` and operators can filter for the
        // specific gate (rather than lumping them in with the
        // generic `CliEmit` bucket). The reason code is the
        // gate's own (`dimension_mismatch`), so the evidence
        // is unambiguous even when both sources appear in the
        // same recovery journal.
        _ if finding.violation_type.reason_code() == "dimension_mismatch" => (
            DiagnosisSeverity::Error,
            DiagnosisOutcome::Failed,
            finding.violation_type.reason_code().to_string(),
            DiagnosisSource::WaveDimensionGuard,
        ),
        _ => (
            DiagnosisSeverity::Error,
            DiagnosisOutcome::Failed,
            finding.violation_type.reason_code().to_string(),
            DiagnosisSource::CliEmit,
        ),
    };

    let mut builder = RecoveryDiagnosisEnvelope::builder()
        .source(source)
        .severity(severity)
        .topic(topic)
        .reason_code(reason_code)
        .message(finding.message.clone())
        .outcome(outcome)
        .safe_target(false);

    if let Some(hat) = source_hat {
        builder = builder.source_hat(hat);
    }

    if let Some(field) = finding.violation_type.field() {
        builder = builder.evidence(EvidenceRef::new(EvidenceKind::Field, field, None));
    }

    let envelope = builder.build();
    let entry = RecoveryJournalEntry::from_envelope(envelope, Vec::new());

    let line = serde_json::to_string(&entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&recovery_path)
        .with_context(|| format!("Failed to open recovery file: {}", recovery_path.display()))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn emit_command(
    color_mode: ColorMode,
    args: EmitArgs,
    hats_source: Option<&HatsSource>,
    config_sources: &[ConfigSource],
    config_was_explicit: bool,
) -> Result<()> {
    emit_command_with_root_and_hats(
        color_mode,
        args,
        None,
        hats_source,
        config_sources,
        config_was_explicit,
    )
}

/// U3 (2026-07-06-002 plan, R3): `--file` 仍是 clap 注入的默认
/// `.ralph/events.jsonl`(相对路径)或绝对形式 `<workspace>/.ralph/events.jsonl`
/// ——即未显式覆盖。`Some(non_default)` 视为显式高级场景,豁免 cwd
/// 漂移硬约束。
fn is_default_file_arg(file: &Path) -> bool {
    let rel_default = PathBuf::from(".ralph/events.jsonl");
    let bare = Path::new(".ralph/events.jsonl");
    // 兼容测试与跨上下文调用:clap default 是相对 `.ralph/events.jsonl`;
    // 显式绝对化 `<root>/.ralph/events.jsonl` 也算 default。
    file == rel_default || file == bare || file.as_os_str() == ".ralph/events.jsonl"
}

/// U3 (R3): 比较两个路径在 canonicalize 后是否指向同一目录。
/// 处理 macOS `/var → /private/var` 这类 OS 级 symlink,避免误判
/// 漂移。
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 emit-path
// macOS-aware canonical comparison (currently uses plain Eq for
// Linux dev builds).
#[allow(dead_code)]
fn paths_canonical_differ(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca != cb
}

/// U3 (R3): 把 cwd ≠ workspace_root 的拒绝信号统一封装,便于
/// `stdout` 摘要在 U5 接线时复用同一文本(error code =
/// `cwd_workspace_drift`)。
fn bail_cwd_workspace_drift(cwd: &Path, workspace_root: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "cwd_workspace_drift: refusing to emit event because the current \
         working directory ({cwd}) does not match the workspace_root \
         ({wr}), and no RALPH_EVENTS_FILE was injected by the loop runner. \
         Without an explicit --file, the event would land in \
         `{cwd}/.ralph/events.jsonl` — an orphan subtree file rather \
         than the loop-managed events file. Fix one of:\n\
         - restore the runner-injected RALPH_EVENTS_FILE env var\n\
         - `cd {wr}` before re-running\n\
         - pass an explicit `--file <absolute path under {wr}/.ralph>` \
         that hits the loop's events allowlist",
        cwd = cwd.display(),
        wr = workspace_root.display()
    );
}

/// U5 (2026-07-06-002 plan, R6): emit 路径被拒绝时,在 `bail!` 之前
/// 显式向 **stdout** 打印一行机器可读摘要,避免被前端 stderr tail
/// 截断。`code` 是稳定的错误标识(与 EmitResult.errors[].code / STDOUT
/// 摘要一致),`detail` 是单行的人类可读补充信息。
///
/// text 模式输出 `emit rejected [{code}]: {detail}`;
/// json 模式输出合法 JSON 行,便于脚本基于 `jq` 处理。
fn print_emit_reject_summary(json_mode: bool, code: &str, detail: &str) {
    if let Some(line) = format_emit_reject_summary(json_mode, code, detail) {
        println!("{line}");
    }
}

/// U5 (R6) 的纯格式化函数版本,与 `print_emit_reject_summary` 共用输出
/// 逻辑,便于 unit test 在不重定向 stdout 的情况下断言精确文本。
///
/// `pub(super)` so the `tests_reject_summary` submodule can import it.
pub(super) fn format_emit_reject_summary(
    json_mode: bool,
    code: &str,
    detail: &str,
) -> Option<String> {
    if json_mode {
        let envelope = serde_json::json!({
            "emit_rejected": true,
            "code": code,
            "detail": detail,
        });
        serde_json::to_string(&envelope).ok()
    } else {
        Some(format!("emit rejected [{}]: {}", code, detail))
    }
}

/// Isolated-mode helper: derive `triggered` from the handoff index when
/// the runner injected the hat context and the agent did not explicitly
/// request a target.
///
/// The handoff index records topics with a *unique* downstream consumer.
/// In isolated mode those topics are exactly the deterministic handoffs.
/// When a topic has multiple consumers, a wildcard subscriber, or is not
/// registered as a handoff topic, `HandoffIndex::consumer_of` returns
/// `None` and we leave `triggered` empty rather than guessing.
///
/// `pub(super)` so the `tests_integration` submodule can pin this helper
/// directly without going through `emit_command_with_root`.
pub(super) fn maybe_derive_triggered_for_isolated(
    topic: &str,
    hat: Option<&str>,
    triggered: Option<String>,
    config: Option<&RalphConfig>,
) -> Option<String> {
    let Some(cfg) = config else {
        return triggered;
    };
    if cfg.event_loop.execution_mode != HatExecutionMode::Isolated {
        return triggered;
    }
    if hat.is_none() {
        return triggered;
    }
    if triggered.is_some() {
        return triggered;
    }
    if ralph_core::event_origin::is_ralph_control_topic(topic)
        || ralph_core::is_orchestrator_diagnostic_topic(topic)
    {
        return triggered;
    }

    let index = ralph_core::workflow_contract::HandoffIndex::from_config(cfg);
    index.consumer_of(topic).and_then(|consumer| {
        (!ralph_core::event_origin::is_virtual_runtime_consumer(consumer))
            .then(|| consumer.to_string())
    })
}

/// Whether to WARN when the resolved core config file is missing.
///
/// `cli_config_explicit` is true only when the operator passed CLI
/// `-c` / `--config`. Ambient / runner-injected `RALPH_CONFIG` is
/// intentionally excluded — see `main.rs` call site comment.
///
/// When a hats source (`-H` or `RALPH_HATS_SOURCE`) supplies the
/// workflow, a missing project `ralph.yml` is the expected default
/// core layer and must stay silent.
///
/// `pub(super)` so the `tests_integration` submodule can call this
/// directly.
pub(super) fn should_warn_on_missing_default_config(
    cli_config_explicit: bool,
    hats_source: Option<&HatsSource>,
) -> bool {
    cli_config_explicit || hats_source.is_none()
}

pub(super) fn emit_command_with_root_and_hats(
    color_mode: ColorMode,
    mut args: EmitArgs,
    root: Option<&PathBuf>,
    hats_source: Option<&HatsSource>,
    config_sources: &[ConfigSource],
    config_was_explicit: bool,
) -> Result<()> {
    // Plan 001 §4.3 C1: when no `-H` is passed but the parent loop set
    // `RALPH_HATS_SOURCE` (so a backend agent can emit without the explicit
    // flag), synthesise a `HatsSource` from the env so the rest of the
    // pipeline loads the right preset's `event_policy.schemas`.
    let env_source = std::env::var("RALPH_HATS_SOURCE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| HatsSource::parse(&s));
    let hats_source_owned: Option<HatsSource>;
    let hats_source: Option<&HatsSource> = match (hats_source, env_source.as_ref()) {
        (Some(s), _) => Some(s),
        (None, Some(env)) => {
            hats_source_owned = Some(env.clone());
            hats_source_owned.as_ref()
        }
        (None, None) => None,
    };
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(root);
    let current_events_marker = workspace_root.join(".ralph/current-events");

    // U5 / R6: --schema <TOPIC> short-circuits to a read-only protocol
    // view. No event is emitted, no events file is touched, no policy
    // check or lint phase runs.
    //
    // Schema mode requires a real ralph.yml — `load_config_for_preflight_sync`
    // happily returns a default `RalphConfig` when the file is missing,
    // but operators inspecting the protocol expect the embedded view
    // of *their* preset, not an empty default.
    //
    // The schema branch sits at the very top of the handler — before
    // urgent-steer handling, before the shared `should_load_config`
    // gate, before any other validation. Operators pipe
    // `ralph emit --schema <TOPIC>` into `jq` and expect a hermetic,
    // side-effect-free read.
    if let Some(ref schema_topic) = args.schema {
        // U6 (2026-07-06-001 plan): `--schema EMIT_RESULT` 是 ralph
        // 内部协议 schema（`EmitResult` 响应 JSON 的 SSOT），不走
        // preset 协议视图——它从 `ralph_core::emit_result` 模块的
        // 嵌入式常量直出，不读 preset / ralph.yml / .ralph/。这是
        // 「协议 SSOT 收敛」的对外可观测信号。
        //
        // 必须放在 ralph.yml 检查之前；其它 topic 走原有
        // ProtocolView 渲染。
        if schema_topic == "EMIT_RESULT" {
            use ralph_core::emit_result::{EMIT_RESULT_SCHEMA_VERSION, EmitResult};
            let view = EmitResult {
                schema_version: EMIT_RESULT_SCHEMA_VERSION,
                ok: false, // placeholder；U6 只暴露 schema_version 字段
                recorded: false,
                topic: String::new(),
                phase: String::new(),
                allowed_next: vec![],
                activate_next: vec![],
                errors: vec![],
                handoff: None,
                target_path: None,
                handoff_envelope: None,
            };
            let pretty = serde_json::to_string_pretty(&view)
                .context("Failed to serialise EmitResult schema view")?;
            println!("{pretty}");
            return Ok(());
        }

        // Prefer explicitly-passed `--config` sources (so operators can
        // inspect a preset's protocol view without a ralph.yml pointing
        // at it). Fall back to the project-config discovery SSOT
        // (`-c` File → `RALPH_CONFIG` → `ralph.yml` → `ralph.yaml`)
        // so `ralph emit --schema` works for operators who keep
        // their custom project config without a `ralph.yml`
        // symlink. The resolver is only consulted when no
        // explicit sources are present; explicit sources keep
        // the historical warn-on-multiple behaviour.
        let owned_config_sources: Vec<ConfigSource>;
        let effective_config_sources: &[ConfigSource] = if config_sources.is_empty() {
            let resolved =
                config_resolution::resolve_project_config_path(&workspace_root, config_sources);
            let config_path = match resolved {
                Some(path) => path,
                None => {
                    anyhow::bail!(
                        "Cannot render protocol view for `{schema_topic}`: no project config found. \
                         The schema view is built from the loaded preset, so the workspace must have \
                         a discoverable ralph.yml / ralph.yaml, set $RALPH_CONFIG, or pass \
                         --config <preset-or-ralph.yml>."
                    );
                }
            };
            if !config_path.exists() {
                anyhow::bail!(
                    "Cannot render protocol view for `{schema_topic}`: no ralph.yml \
                     found at {}. The schema view is built from the loaded preset, \
                     so the workspace must have a discoverable ralph.yml or pass \
                     --config <preset-or-ralph.yml>.",
                    config_path.display()
                );
            }
            owned_config_sources = vec![ConfigSource::File(config_path)];
            &owned_config_sources
        } else {
            config_sources
        };
        let cfg = crate::preflight::load_config_for_preflight_sync(
            effective_config_sources,
            hats_source,
            &workspace_root,
        )
        .with_context(|| {
            format!(
                "Failed to load config for schema view of `{schema_topic}`. \
                 Fix the ralph.yml errors or remove --schema."
            )
        })?;
        // P2-#6 (002-adversarial-review): production-only env
        // read; tests must use `from_event_loop` (env-free).
        let view = ProtocolView::from_event_loop_with_feature_for_env(&cfg.event_loop);
        let pretty = super::schema_view::render_pretty(&view, schema_topic)
            .context("Failed to serialise protocol view")?;
        println!("{pretty}");
        eprintln!(
            "Tip: use the `required_fields` array above as the authoritative field list \
             for `{schema_topic}`. To precheck a payload before emitting, run \
             `ralph emit {schema_topic} --policy-check --hat <hat-id> --json '<payload>'`."
        );
        return Ok(());
    }

    if std::env::var("RALPH_WAVE_ID").is_err() {
        let urgent_steer_store = UrgentSteerStore::new(urgent_steer_path_from_workspace(root));
        if let Some(record) = urgent_steer_store
            .take()
            .context("Failed to read urgent-steer marker")?
        {
            let guidance = record
                .messages
                .iter()
                .enumerate()
                .map(|(idx, message)| format!("{}. {}", idx + 1, message))
                .collect::<Vec<_>>()
                .join("\n");

            anyhow::bail!(
                "Urgent steer is pending. Do not hand off yet.\n\n\
                 Human feedback:\n{guidance}\n\n\
                 You have now seen the steer. Address it in this turn, then rerun `ralph emit` \
                 once you are ready to hand off."
            );
        }
    }

    // Load config for policy enforcement, provenance checks, and strict-mode detection.
    // We load whenever explicit flags are set, provenance might be required, or the
    // workspace looks like a Ralph project (has .ralph) so we can honour strict configs.
    let should_load_config = args.policy_check
        || args.no_policy_check
        || args.hat.is_none()
        || std::env::var("RALPH_CURRENT_HAT").is_ok()
        || workspace_root.join(".ralph").is_dir();

    let config = if should_load_config {
        // Prefer explicitly-passed `--config` sources (so operators can
        // target a preset without a workspace ralph.yml). Fall back to
        // the project-config discovery SSOT (`-c` File → `RALPH_CONFIG`
        // → `ralph.yml` → `ralph.yaml`) so emit / policy-check honour
        // every supported input without requiring a `ralph.yml`
        // symlink.
        let owned_config_sources: Vec<ConfigSource>;
        let effective_config_sources: &[ConfigSource] = if config_sources.is_empty() {
            let resolved =
                config_resolution::resolve_project_config_path(&workspace_root, config_sources);
            let config_path = resolved.unwrap_or_else(|| workspace_root.join("ralph.yml"));
            owned_config_sources = vec![ConfigSource::File(config_path.clone())];
            &owned_config_sources
        } else {
            config_sources
        };
        let discovered_config_path = effective_config_sources
            .iter()
            .find_map(|source| {
                if let ConfigSource::File(path) = source {
                    Some(path.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| workspace_root.join("ralph.yml"));
        let warn_on_missing_default =
            should_warn_on_missing_default_config(config_was_explicit, hats_source);
        match crate::preflight::load_config_for_preflight_sync_with_missing_default_warning(
            effective_config_sources,
            hats_source,
            &workspace_root,
            warn_on_missing_default,
        ) {
            Ok(cfg) => Some(cfg),
            Err(_e) => {
                if args.policy_check {
                    anyhow::bail!(
                        "Policy check requested but config could not be loaded: {}. \
                         Fix the config or omit --policy-check.",
                        _e
                    );
                }
                // Plan 001 §4.3 C4 row 2: bail closed when a loop
                // context exists (`.ralph/` present), even if no
                // `ralph.yml` was found — the absence of ralph.yml
                // is itself a malformed-config condition under a
                // strict preset.
                if discovered_config_path.exists() || workspace_root.join(".ralph").is_dir() {
                    anyhow::bail!(
                        "Config file exists or loop context detected but config could not be loaded: {}. \
                         Fix the config, use --policy-check with a valid config, \
                         or use --unsafe-no-policy-check to bypass (if permitted).",
                        _e
                    );
                }
                None
            }
        }
    } else {
        None
    };

    // (U5 / R6 schema branch moved to the top of the handler —
    // see the early `if let Some(ref schema_topic) = args.schema`
    // block above. This duplicate is intentional-deleted to avoid
    // double-printing the protocol view.)

    // Schema mode short-circuits above. Below this point we are
    // in *emit* mode and `args.topic` is mandatory; clap cannot
    // express `required_unless_present = "schema"` on a positional
    // argument, so we enforce the precondition here.
    let topic = args
        .topic
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing event topic (required unless --schema is set)"))?;

    // Parallel Forge artifact-first handoff. CLI precheck and apply both
    // normalize the same reference-only payload, deriving the canonical plan
    // digest/count summary from the workspace-bounded artifact. The runtime
    // projector performs the same verification before creating tasks.
    if topic == "forge.plan.ready" {
        args.payload = ralph_core::parallel_forge_handoff::canonicalize_plan_ready_payload(
            &args.payload,
            &workspace_root,
        )
        .map_err(|error| anyhow::anyhow!("forge.plan.ready artifact handoff rejected: {error}"))?;
    }
    let topic = topic.as_str();

    // Determine whether policy validation is required. U15: agent
    // context (env-detected) defaults to strict policy-check, even
    // when the preset author did not flip
    // `require_policy_check_for_cli_emit`.
    //
    // U1 (2026-07-06-002 plan, R1): `workspace_root` is anchored
    // exactly once at the top of the handler via `resolve_workspace_root`
    // (line 397 — `RALPH_WORKSPACE_ROOT` > `discover_workspace_root(cwd)`
    // > cwd). All downstream gates (policy-check / scope /
    // step-handoff / write-path) reuse that single binding. Previously
    // this block shadowed the binding with `current_dir()`, which let
    // a hat process running inside a subtree cwd (e.g. `cd sorts/`)
    // rewrite the workspace anchor to the subtree and land events in
    // `sorts/.ralph/events.jsonl` — see
    // `docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md`.
    let check_mode = should_policy_check_emit_with_ctx(&args, config.as_ref(), &workspace_root);

    // Phase 2: in isolated mode the runner controls hat provenance. When the
    // agent is running inside a hat context (RALPH_CURRENT_HAT is set), the
    // CLI flag --hat is ignored and must not disagree with the environment.
    let env_hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());

    // Resolve provenance values: CLI flag > env var > empty
    let (hat, triggered, source) =
        resolve_provenance(args.hat.clone(), args.triggered, args.source, |key| {
            std::env::var(key).ok()
        });

    let hat = if config
        .as_ref()
        .is_some_and(|c| c.event_loop.execution_mode == HatExecutionMode::Isolated)
    {
        if let Some(ref env_hat) = env_hat {
            if let Some(ref cli_hat) = args.hat
                && cli_hat != env_hat
            {
                anyhow::bail!(
                    "Isolated mode hat mismatch: --hat '{}' conflicts with \
                         RALPH_CURRENT_HAT '{}'. In isolated mode the runner \
                         controls provenance; emit as '{}'.",
                    cli_hat,
                    env_hat,
                    env_hat
                );
            }
            Some(env_hat.clone())
        } else {
            hat
        }
    } else {
        hat
    };

    // 2026-07-29-006 fix U1 (correctness:C1 + adversarial:A1): the
    // `maybe_derive_triggered_for_isolated` derivation MUST run AFTER
    // the precheck rewrite below so it sees the effective topic, not
    // the bare one. Before this reorder the JSONL row recorded
    // `topic="work.failed.proposed"` (rewritten) but
    // `triggered="reporter"` (consumer of the bare `work.failed`
    // topic), and `check_envelope_triggered` only validated the hat
    // id was in topology — it never cross-checked that
    // `triggered` matched the effective topic, so the mismatch
    // slipped through silently. There is intentionally no other
    // change in this block: signature, body, surrounding gates, and
    // the on-disk `record["triggered"] = serde_json::Value::String(triggered);`
    // write at lines 1537-1538 are all left untouched.

    // The pure resolver lives in `ralph_core::config::precheck` and
    // encodes the six rules from plan U2 Approach (idempotent,
    // kill-switch aware, scope-preserving); this block just owns
    // the binding shadow so every downstream gate sees the same
    // effective topic.
    let topic_owned: String = match config.as_ref() {
        Some(cfg) => ralph_core::config::resolve_precheck_emit_topic(cfg, hat.as_deref(), topic),
        None => topic.to_string(),
    };
    let topic: &str = topic_owned.as_str();

    // 2026-07-03: isolated mode auto-derive `triggered` from the
    // topic's registered subscriber. The runner previously set
    // RALPH_TRIGGERED_HAT to the round-robin "next hat", which
    // caused events like `review.dimension.ready` to be routed to
    // the wrong hat (e.g. `shipper` instead of `dimension-reviewer`).
    // When the agent is inside a runner-injected hat context and
    // did not explicitly request a target, derive the target from
    // the preset topology so the event bus routes correctly. The
    // topic passed here must be the EFFECTIVE (rewritten) topic so
    // the JSONL row's `triggered` value matches its `topic`.
    let triggered =
        maybe_derive_triggered_for_isolated(topic, hat.as_deref(), triggered, config.as_ref());

    // Enforce provenance requirements when hat is missing.
    //
    // U1 (2026-06-17-004 plan, R1+R2): the blanket precheck is replaced
    // by `check_emit_provenance` (called below as a separate gate). The
    // smart gate allows control topics (loop.cancel, task.resume,
    // human.*) and orchestrator diagnostics (event.*) to be emitted
    // without a hat — these are produced by the loop / runtime ralph
    // pseudo-hat. Business topics still fail-closed when provenance
    // is missing.
    if hat.is_none() {
        let provenance_required = config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .map(|p| p.require_emit_provenance)
            .unwrap_or(false);
        if provenance_required {
            // Skip the bail for control / diagnostic topics — they are
            // produced by the loop itself and bypass the new
            // check_emit_provenance gate (see policy_check.rs). The
            // smart gate catches business-topic cases below.
            let is_control = ralph_core::event_origin::is_ralph_control_topic(topic);
            let is_diagnostic = ralph_core::is_orchestrator_diagnostic_topic(topic);
            if !is_control && !is_diagnostic {
                anyhow::bail!(
                    "Event provenance required: --hat <hat-id> or RALPH_CURRENT_HAT must be set."
                );
            }
        }
    }

    // U1: ralph-hat business-topic guard.
    // The builtin `ralph` hat is the orchestration fallback hat. Allowing it to
    // emit business topics (e.g. `review.passed`, `work.start`) lets a worktree
    // loop's loop runner bypass review-synthesizer / plan-gate / coordinator and
    // advance the workflow as `ralph` — this is the impersonation attack the
    // P0 origin guard already rejects at JSONL read time. Reject here too so
    // the agent gets immediate backpressure (otherwise the rejection only
    // surfaces several seconds later when the loop runner reads the JSONL).
    if let Some(hat_id) = hat.as_deref()
        && hat_id == "ralph"
        && !ralph_core::event_origin::is_ralph_control_topic(topic)
    {
        anyhow::bail!(
            "Builtin ralph hat may only emit control topics: {:?}. \
             Topic '{}' is a business topic and cannot be emitted by ralph. \
             Set --hat to a registered workflow hat (e.g. coordinator, executor, \
             review-synthesizer) instead.",
            ralph_core::event_origin::RALPH_CONTROL_TOPICS,
            topic
        );
    }

    // U6 (2026-06-21-002 plan §U6): CLI `--policy-check` always
    // routes through the unified `validate_event` pipeline. The
    // legacy `validate_event_with_hat` path below is preserved as
    // a structural fallback for diff / comparison runs and is
    // only entered when `unified_active` is false. The unified
    // path surfaces structured `reason_codes` (and per-rule
    // `suggestions`) so agents can programmatically match failures
    // U1 (plan 2026-08-08-004, D12): scope handoff consistency gate.
    // Fires for scope topics (merge.integrated, merge.stabilized,
    // postmerge.changemap.ready, redteam.plan.resolved) to verify
    // required scope fields are present, paths are under allowed roots,
    // and files are readable. Runs BEFORE the unified/skip branch below
    // and BEFORE the `--unsafe-no-policy-check` short-circuit, so it
    // is mandatory even when the agent bypasses policy enforcement.
    if let Err(err) =
        crate::policy_check::check_scope_handoff_guard(topic, &args.payload, &workspace_root)
    {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "scope_handoff".to_string(),
                context: err.message.clone(),
                referenced_fields: Vec::new(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
            evidence: None,
        };
        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
        anyhow::bail!(
            "Event rejected by scope handoff guard: {}",
            err.message
        );
    }

    // against the loop's vocabulary.
    let unified_active = check_mode != PolicyCheckMode::Skip
        && config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .is_some_and(|p| p.enabled);
    if unified_active {
        let mut report = crate::policy_check::run_policy_check_unified_with_config(
            topic,
            Some(&args.payload),
            hat.as_deref(),
            triggered.as_deref(),
            &workspace_root,
            config.as_ref(),
        )?;
        // 2026-07-09-001 plan (U4): enrich the unified
        // report's per-item `validation_errors` with
        // `field_docs` / `allowed_values` from the loaded
        // schema. The schema is whatever the unified pipeline
        // resolved for the topic; if the topic has no schema,
        // enrichment is a no-op.
        {
            let schema_lookup: Option<&ralph_core::config::EventSchema> = config
                .as_ref()
                .and_then(|c| c.event_loop.event_policy.as_ref())
                .and_then(|p| {
                    let key: &str = topic;
                    p.schemas.get(key)
                });
            let payload_value = serde_json::from_str::<serde_json::Value>(&args.payload).ok();
            let report_hat = report.hat.clone();
            report = crate::policy_check::enrich_report_with_schema(
                report,
                topic,
                report_hat.as_deref(),
                payload_value.as_ref(),
                schema_lookup,
            );
        }
        if !report.accepted {
            // Structured reason_code list — the U6 plan mandates
            // "统一后错误输出结构化 `reason_code`" so the agent can
            // programmatically match failures against the loop's
            // vocabulary. We surface the full list to stderr (one
            // per line) AND include a JSON envelope in the bail so
            // tools that parse stderr can recover the structured
            // shape.
            let codes = report.reason_codes.join(", ");
            let suggestions: Vec<String> = report
                .suggestions
                .iter()
                .enumerate()
                .filter_map(|(idx, hint)| {
                    if hint.is_empty() {
                        None
                    } else {
                        Some(format!("[{}] {}", idx, hint))
                    }
                })
                .collect();
            let suggestions_block = if suggestions.is_empty() {
                String::new()
            } else {
                format!("\n\nSuggestions:\n{}", suggestions.join("\n"))
            };
            let repair_block = crate::policy_check::render_validation_error_repair_block(
                &report.topic,
                &report.validation_errors,
            )
            .map(|block| format!("\n\n{block}"))
            .unwrap_or_default();
            // Always append the schema-discovery hint so agents query the
            // active runner context instead of guessing another preset.
            // A loop already injects RALPH_HATS_SOURCE / RALPH_CONFIG;
            // suggesting `-H builtin:<preset>` here can override that
            // authority and turn a payload error into flow_unknown_emit or
            // origin:unknown_hat.
            let schema_hint = format!(
                "\n\nTip: run `ralph emit --schema {}` to list the required \
                 fields from the active runner context. During a hat \
                 activation, do not guess or override the preset with `-H` \
                 / `--config`; the runner-injected RALPH_HATS_SOURCE and \
                 RALPH_CONFIG are authoritative. If schema lookup reports \
                 flow_unknown_emit or origin:unknown_hat, stop and report a \
                 runner-context mismatch.",
                report.topic
            );
            let suggestions_block = format!("{suggestions_block}{repair_block}{schema_hint}");
            let envelope = serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string());

            // phase / allowed_next resolved from phase authority + ledger
            // via `policy_check::build_emit_result_parts`.
            if args.output == "json" {
                let emit_result =
                    crate::policy_check::report_to_emit_result(&report, config.as_ref());
                println!(
                    "{}",
                    serde_json::to_string(&emit_result)
                        .context("Failed to serialise EmitResult JSON")?
                );
                anyhow::bail!(
                    "Event rejected by policy: reason_codes=[{}] topic='{}' hat={:?}",
                    codes,
                    report.topic,
                    report.hat
                );
            }

            eprintln!("{}", envelope);
            anyhow::bail!(
                "Event rejected by policy: reason_codes=[{}] topic='{}' hat={:?}{}",
                codes,
                report.topic,
                report.hat,
                suggestions_block
            );
        }
        // Unified path accepted: skip the legacy `validate_event_with_hat`
        // branch below so the legacy bail never double-fires on the
        // same event. The legacy path is only entered when
        // `unified_active` is false (no event_policy configured).
        tracing::info!(
            "cli emit policy check: unified pipeline accepted topic={}",
            topic
        );
    }

    if check_mode != PolicyCheckMode::Skip {
        // When the unified branch already accepted the event, skip
        // the legacy gate (U6: unified is the production path; the
        // legacy path stays only for diff runs when no event_policy
        // is configured).
        if unified_active {
            tracing::debug!(
                "cli emit: skipping legacy validate_event_with_hat (unified path active)"
            );
        } else {
            let policy = match config
                .as_ref()
                .and_then(|c| c.event_loop.event_policy.as_ref())
            {
                Some(p) if p.enabled => Some(p),
                _ => {
                    if check_mode == PolicyCheckMode::ExplicitCheck {
                        eprintln!(
                            "Warning: --policy-check was requested but no event policy is configured or enabled."
                        );
                    }
                    None
                }
            };
            if let Some(policy) = policy {
                use ralph_core::{
                    PolicyRuntimeState, check_topic_deny_rules, validate_event_with_hat,
                };
                let events_path = fs::read_to_string(&current_events_marker)
                    .map(|s| resolve_marker_target(&workspace_root, &s))
                    .unwrap_or_else(|_| args.file.clone());
                let mut state =
                PolicyRuntimeState::from_events(&events_path, policy).unwrap_or_else(|e| {
                    eprintln!(
                        "Warning: Failed to replay events for policy check: {}. Using empty state.",
                        e
                    );
                    PolicyRuntimeState::default()
                });

                // Enforce topic-deny rules at CLI emit time so a forbidden
                // (hat, topic) pair never reaches the events file.
                if let Some(decision) = check_topic_deny_rules(hat.as_deref(), topic, policy) {
                    match decision {
                        ralph_core::PolicyDecision::Accept => {}
                        ralph_core::PolicyDecision::Warn(findings) => {
                            for finding in findings {
                                eprintln!("Policy warning: {}", finding.message);
                            }
                        }
                        ralph_core::PolicyDecision::AcknowledgeAndForward(finding) => {
                            // U2 (plan 2026-07-04-004): CLI emit
                            // path sees the dedup carve-out. We
                            // surface the dedup hint as a warning
                            // and forward the event as the runtime
                            // would; `--policy-check` callers see
                            // the same hint via stderr.
                            eprintln!("Policy acknowledge+forward: {}", finding.message);
                        }
                        ralph_core::PolicyDecision::RejectWithResume(finding)
                        | ralph_core::PolicyDecision::Hold(finding)
                        | ralph_core::PolicyDecision::Block(finding)
                        | ralph_core::PolicyDecision::Ignore(finding) => {
                            record_cli_emit_rejection(
                                &workspace_root,
                                topic,
                                hat.as_deref(),
                                &finding,
                            );
                            anyhow::bail!(
                                "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                                finding.message,
                                format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), topic)
                            );
                        }
                    }
                }

                // Run schema validation with hat-aware restrictions.
                let decision = validate_event_with_hat(
                    topic,
                    Some(&args.payload),
                    policy,
                    &mut state,
                    hat.as_deref(),
                );
                match decision {
                    ralph_core::PolicyDecision::Accept => {}
                    ralph_core::PolicyDecision::Warn(findings) => {
                        for finding in findings {
                            eprintln!("Policy warning: {}", finding.message);
                        }
                    }
                    ralph_core::PolicyDecision::AcknowledgeAndForward(finding) => {
                        // U2 (plan 2026-07-04-004): CLI precheck +
                        // emit path sees the dedup carve-out. We
                        // log the dedup hint as a warning and let
                        // the event reach the bus so `--policy-check`
                        // callers can confirm the silent-success
                        // lane is active. Real emit (without
                        // --policy-check) writes to the events file
                        // as the runtime would.
                        eprintln!("Policy acknowledge+forward: {}", finding.message);
                    }
                    ralph_core::PolicyDecision::RejectWithResume(finding)
                    | ralph_core::PolicyDecision::Hold(finding)
                    | ralph_core::PolicyDecision::Block(finding)
                    | ralph_core::PolicyDecision::Ignore(finding) => {
                        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
                        anyhow::bail!(
                            "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                            finding.message,
                            format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), topic)
                        );
                    }
                }
            } // closes if let Some(policy)
        } // closes if unified_active's else
    } else if check_mode == PolicyCheckMode::Skip {
        tracing::info!("cli emit policy check skipped: no event_policy in resolved config");
    }

    // U6 (2026-06-21-002 plan §U6): the unified `validate_event`
    // pipeline is the production path and runs before the legacy
    // check above, so the legacy bail never double-fires. The
    // legacy path below only runs when no event_policy is
    // configured (diff / no-policy fallback).

    // U1 (2026-06-17-004 plan, R1+R2): CLI provenance fail-closed.
    // Fires regardless of `check_mode`: if the agent is in isolated
    // mode and forgot to pass `--hat`, the CLI must reject the event
    // BEFORE it lands in events.jsonl. Without this gate, the runtime
    // origin guard drops the event silently at JSONL read time and the
    // agent gets no actionable backpressure at the CLI boundary.
    //
    // `check_isolated_scope` below only enforces when the hat is known;
    // `check_emit_provenance` is the matching gate for the `hat = None`
    // path. Together they form the full isolated-mode CLI guard.
    if let Some(cfg) = config.as_ref()
        && let Err(err) = crate::policy_check::check_emit_provenance(hat.as_deref(), topic, cfg)
    {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "missing_provenance".to_string(),
                context: err.message.clone(),
                referenced_fields: Vec::new(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
            evidence: None,
        };
        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
        anyhow::bail!(
            "Event rejected by missing-provenance guard: {}",
            err.message
        );
    }

    // U1 (2026-06-17-003 plan): isolated mode scope precheck. When the
    // resolved preset has `event_loop.execution_mode: isolated` and the
    // caller passed a hat (`--hat` or `RALPH_CURRENT_HAT`), the hat's
    // `publishes` scope must be enforced BEFORE the event lands in
    // events.jsonl. This mirrors the loop's runtime
    // `isolated_publish_allowed` check via `HatRegistry::can_publish`,
    // giving the agent actionable backpressure at the CLI boundary
    // instead of a silent drop at the loop reader.
    //
    // Fires regardless of `check_mode`: if the agent is in isolated
    // mode and passed `--hat`, the runner's scope is the contract —
    // `--policy-check` toggles schema enforcement, not scope
    // enforcement. Without `--hat` the call defers to the origin
    // guard (which rejects unknown/missing provenance).
    if let Some(cfg) = config.as_ref()
        && let Err(err) = crate::policy_check::check_isolated_scope(hat.as_deref(), topic, cfg)
    {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "isolated_scope".to_string(),
                context: err.message.clone(),
                referenced_fields: Vec::new(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
            evidence: None,
        };
        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
        anyhow::bail!("Event rejected by isolated scope guard: {}", err.message);
    }

    // U3 (R3): wave worker dimension assignment precheck. Fires before
    // any policy / step-handoff processing so a wave worker that
    // emits the wrong dimension never reaches the events file. The
    // env var is set by the loop runner on `review.dimension.done`
    // workers; non-wave callers (env unset) pass through unchanged.
    if let Err(err) = crate::policy_check::check_wave_dimension_assignment(topic, &args.payload) {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "wave_dimension_assignment".to_string(),
                context: err.message.clone(),
                referenced_fields: Vec::new(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
            evidence: None,
        };
        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
        anyhow::bail!("Event rejected by wave dimension guard: {}", err.message);
    }

    // U1 (2026-06-17-005 plan): step handoff gate precheck at the CLI
    // boundary. Mirrors the loop-side `apply_step_handoff_gate` so an
    // agent calling `ralph emit --policy-check` (or running under
    // `require_policy_check_for_cli_emit: true`) gets the same
    // `progress_task_mismatch` backpressure before writing the event
    // to disk. The CLI is *additive* — it never replaces the loop
    // gate, it surfaces the same reason earlier.
    if ralph_core::step_handoff::progress_task_gate::is_gated_topic(topic)
        && check_mode != PolicyCheckMode::Skip
    {
        match crate::policy_check::check_step_handoff_gate(topic, &args.payload, &workspace_root) {
            Ok(()) => {}
            Err(err) => {
                record_cli_emit_rejection(
                    &workspace_root,
                    topic,
                    hat.as_deref(),
                    &ralph_core::PolicyFinding {
                        violation_type: ViolationType::SemanticGateViolation {
                            gate: "progress_task_gate".to_string(),
                            context: err.message.clone(),
                            referenced_fields: Vec::new(),
                        },
                        topic: topic.to_string(),
                        message: err.message.clone(),
                        evidence: None,
                    },
                );
                let failure = ValidationFailure {
                    ok: false,
                    error: "policy_validation_failed",
                    topic: topic.to_string(),
                    validation_errors: vec![err],
                };
                crate::policy_check::emit_policy_validation_failure(
                    &failure,
                    crate::policy_check::OutputMode::Text,
                )?;
            }
        }
    }

    // U5 (plan 2026-07-30-004): unified agent-CLI capability + evaluation
    // token gate. Resolved AFTER the policy / scope / provenance / wave /
    // step-handoff guards above so those rejections keep their existing error
    // shape, and BEFORE the dry-run return + disk write so a denied capability
    // or a missing / stale token fails both `--policy-check` and apply. Only
    // active in an agent context (`RALPH_CURRENT_HAT` set) with no event-policy
    // pipeline (see `U5Gate`); a preset with an enabled `event_policy` already
    // enforces the contract via the unified pipeline above, so this gate stands
    // down there to avoid double enforcement.
    let u5_gate = U5Gate::resolve(
        env_hat.is_some(),
        unified_active,
        config.as_ref(),
        hat.as_deref(),
        topic,
        &args.payload,
    );
    // U3 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
    // a compile failure in an agent governed path is a hard deny
    // BEFORE any event / idempotency / ticket side effect. The
    // previous behaviour let the capability gate skip (resolved
    // was None) and the token gate return a misleading
    // `policy_check_token_mismatch` against an empty expected
    // token.
    if let Some((code, err)) = u5_gate.compile_failure() {
        print_emit_reject_summary(args.output == "json", code, &err);
        anyhow::bail!("{err}");
    }
    if let Some(err) = u5_gate.capability_denied(hat.as_deref(), topic) {
        print_emit_reject_summary(args.output == "json", "capability_denied", &err);
        anyhow::bail!("{err}");
    }
    if !args.policy_check
        && let Some((code, err)) = u5_gate.token_violation(
            hat.as_deref(),
            topic,
            &args.payload,
            args.policy_check_token.as_deref(),
        )
    {
        print_emit_reject_summary(args.output == "json", code, &err);
        anyhow::bail!("{err}");
    }

    // Generate timestamp internally — agents cannot forge timestamps
    let ts = chrono::Utc::now().to_rfc3339();

    // 2026-07-27-004 plan U2 (R5-R7 / D3 / D8): capture the wave-worker
    // handshake EARLY so the payload normalisation step below runs
    // before the policy-check path inspects the payload. The values
    // mirror the late `wave_worker` block further down — we hoist
    // the read so the normalisation happens in the correct order.
    let wave_worker = std::env::var("RALPH_WAVE_WORKER").ok().as_deref() == Some("1");
    let wave_id_env = wave_worker
        .then(|| std::env::var("RALPH_WAVE_ID").ok())
        .flatten();
    let slot_index_env = wave_worker
        .then(|| {
            std::env::var("RALPH_WAVE_INDEX")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
        })
        .flatten();

    // Validate JSON payload if --json flag is set
    let payload = if args.json && !args.payload.is_empty() {
        // Validate it's valid JSON
        serde_json::from_str::<serde_json::Value>(&args.payload).context("Invalid JSON payload")?;
        args.payload
    } else {
        args.payload
    };

    // Build the event record
    // We use serde_json directly to ensure proper escaping
    let payload_value = if args.json && !payload.is_empty() {
        // Parse and embed as object
        serde_json::from_str::<serde_json::Value>(&payload)?
    } else if payload.is_empty() {
        serde_json::Value::Null
    } else if looks_like_json(&payload) {
        // Auto-detect JSON objects/arrays even without --json so that
        // agents emitting structured events (e.g. work.done) don't get
        // their payload stored as a plain string and rejected by the
        // execution contract validator.
        let p_clone = payload.clone();
        serde_json::from_str::<serde_json::Value>(&payload)
            .unwrap_or(serde_json::Value::String(p_clone))
    } else {
        serde_json::Value::String(payload.clone())
    };

    // 2026-07-27-004 plan U2 (R5-R7 / D3 / D8): wave worker
    // payload system fields are runtime-owned. Apply the
    // normalisation BEFORE the policy_check block below (which
    // inspects `payload_value`); the helper either injects
    // `wave_id`/`slot_index` or rejects an Agent hand-fill with
    // the stable `system_field_owned_by_runtime` reason.
    let payload_value = normalize_wave_worker_system_fields(
        payload_value,
        wave_worker,
        wave_id_env.as_deref(),
        slot_index_env,
    )?;

    // U2: stash the original payload string before any
    // downstream borrow consumes it; the JSON `--output`
    // EmitResult builder uses it to extract the
    // `handoff_envelope` summary when the typed
    // `emit_result_summary` flag is on.
    let payload_for_summary = payload.clone();

    // Generic guard: reject an empty task_id in any emitted event payload.
    // An empty task_id is never valid (Ralph task ids are always non-empty
    // strings like `task-{timestamp}-{hex}`), and letting it through would
    // break the step-handoff / state-projection chain.
    if let Some(Value::String(task_id)) = payload_value.get("task_id")
        && task_id.trim().is_empty()
    {
        anyhow::bail!("task_id cannot be empty in event payload for topic '{topic}'");
    }

    let mut record = serde_json::json!({
        // 2026-07-29-006 plan U3 (R2): the on-disk event must carry
        // the effective topic, not the bare user input. The
        // precheck desugar rewrites the producer's `publishes` to
        // `<X>.proposed`; the gate hat subscribes on that variant;
        // writing `<X>` would land the event on a channel no one
        // reads and bypass the gate entirely.
        "topic": topic,
        "payload": payload_value,
        "ts": ts
    });

    // U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
    // `triggered` validation. The gate runs BEFORE the record is
    // written so a malformed `triggered` value can never land
    // on disk. Missing `triggered` is allowed (R12). The
    // topology check uses the loaded preset's `hats[]` map; an
    // unknown value yields `triggered_not_in_topology` and the
    // apply path bails before writing the record.
    if let Some(cfg) = config.as_ref()
        && let Err(err) =
            crate::policy_check::check_envelope_triggered(topic, triggered.as_deref(), cfg)
    {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "envelope_triggered".to_string(),
                context: err.message.clone(),
                referenced_fields: Vec::new(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
            evidence: None,
        };
        record_cli_emit_rejection(&workspace_root, topic, hat.as_deref(), &finding);
        anyhow::bail!(
            "Event rejected by envelope-triggered guard: {}",
            err.message
        );
    }

    // Add provenance fields only when they have values (preserve old simple schema)
    if let Some(ref hat) = hat {
        record["hat"] = serde_json::Value::String(hat.clone());
    }
    if let Some(triggered) = triggered {
        record["triggered"] = serde_json::Value::String(triggered);
    }
    if let Some(source) = source {
        record["source"] = serde_json::Value::String(source);
    } else if source.is_none()
        && config
            .as_ref()
            .is_some_and(|c| c.event_loop.execution_mode == HatExecutionMode::Isolated)
        && !ralph_core::event_origin::is_ralph_control_topic(topic)
        && let Some(hat_str) = hat.as_ref()
    {
        // U7 (2026-06-17-004 plan, R7): in isolated mode, when a business topic
        // has no explicit --source and hat is known, default source to the emitting
        // hat so downstream consumers always have a stable attribution field.
        // Control topics (loop.cancel, task.resume, etc.) are unchanged.
        record["source"] = serde_json::Value::String(hat_str.clone());
    }

    // Auto-tag with wave metadata from env vars (set by loop runner on wave workers)
    if let (Ok(wave_id), Ok(wave_index_str)) = (
        std::env::var("RALPH_WAVE_ID"),
        std::env::var("RALPH_WAVE_INDEX"),
    ) && let Ok(wave_index) = wave_index_str.parse::<u32>()
    {
        record["wave_id"] = serde_json::Value::String(wave_id);
        record["wave_index"] = serde_json::Value::Number(wave_index.into());
    }

    // Resolve events file via the P6 allowlist guard BEFORE the
    // `--policy-check` early return. Dry-run must fail the same way as
    // apply when `RALPH_EVENTS_FILE` / `--file` is outside the allowlist
    // (otherwise agents get a false green on --policy-check then exit 1
    // on the real emit — see 2026-07-26-002 Open Questions).
    //
    // The guard verifies the candidate path is either the active
    // `current-candidate-events` target, the `current-events` target, or
    // the default `events.jsonl` when no marker exists.
    //
    // U2 (2026-07-06-002 plan, R2/R4): pass the resolved hat context
    // and the isolated-mode flag to `resolve_emit_path` so the
    // fail-closed routing guard can (a) refuse orphan subtree
    // candidates and (b) prefer the `current-hat-events` channel
    // over the legacy `events.jsonl` default in isolated mode.
    let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
    let isolated_mode = config
        .as_ref()
        .is_some_and(|c| c.event_loop.execution_mode == HatExecutionMode::Isolated);

    // U2 (plan 2026-07-25-003): capture the wave-worker handshake
    // (`RALPH_WAVE_ID` / `RALPH_WAVE_INDEX`) from the worker env when
    // the dispatcher marked this process as a wave worker. The path
    // shape alone is no longer enough to accept a wave channel; the
    // file's `<id>` / `<idx>` segments must match these values verbatim
    // (adversarial-01 / goal-alignment-01). We DO NOT read them when
    // `RALPH_WAVE_WORKER != "1"` so a non-wave isolated hat cannot
    // self-claim a wave context just by exporting those vars.
    //
    // 2026-07-27-004 plan U2: the values are captured earlier in
    // this function (above the payload normalisation step) and
    // kept in scope so this block can use the same names. The
    // reads match the dispatcher's `RALPH_WAVE_*` handshake
    // contract; we deliberately do not call `std::env::var`
    // again because the contract requires the worker PID to be
    // bound exactly once at spawn time.
    let wave_worker = std::env::var("RALPH_WAVE_WORKER").ok().as_deref() == Some("1");
    let wave_id_env = wave_worker
        .then(|| std::env::var("RALPH_WAVE_ID").ok())
        .flatten();
    let slot_index_env = wave_worker
        .then(|| {
            std::env::var("RALPH_WAVE_INDEX")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
        })
        .flatten();
    // Plan 2026-07-27-003 U2 (R4): wave workers MUST also have
    // their loop identity in env (set by the dispatcher at spawn
    // time) so the registry resolver can authenticate the
    // (loop, wave, slot, path) tuple against
    // `.ralph/wave-channels/<loop-id>/<wave-id>.json`. Env-only
    // self-claim (`unset RALPH_CURRENT_LOOP_ID`) refuses the
    // emit; this is the dispatcher's binding contract.
    let loop_id_env = wave_worker
        .then(|| std::env::var("RALPH_CURRENT_LOOP_ID").ok())
        .flatten()
        .filter(|s| !s.is_empty());

    // Plan 2026-07-27-003 U7 (R4): when a process declares itself a
    // wave worker via `RALPH_WAVE_WORKER=1` but the dispatcher
    // handshake (`RALPH_WAVE_ID` / `RALPH_WAVE_INDEX` / loop id) is
    // incomplete, the resolver MUST refuse the emit with a stable
    // agent-executable reason. Falling through to the legacy
    // `events.jsonl` default in this state is exactly the
    // implementation-review primary-20260727 double-ledger root
    // cause (orphan in main, no registry binding), so we fail-close
    // here rather than rely on `resolve_emit_path`'s downstream
    // match-arm guard alone — which only triggers AFTER the legacy
    // fall-through already committed to a candidate path.
    //
    // This is the test-side mirror of `scenario_02_worker_unset_events_file_emits_rejected`:
    // the worker POV has `RALPH_WAVE_WORKER=1` set but neither
    // `RALPH_WAVE_ID` nor `RALPH_WAVE_INDEX` injected.
    if wave_worker && (wave_id_env.is_none() || slot_index_env.is_none()) {
        let missing = match (wave_id_env.is_none(), slot_index_env.is_none()) {
            (true, true) => "RALPH_WAVE_ID and RALPH_WAVE_INDEX",
            (true, false) => "RALPH_WAVE_ID",
            (false, true) => "RALPH_WAVE_INDEX",
            _ => unreachable!(),
        };
        anyhow::bail!(
            "incomplete wave-worker binding: {missing} must be set when \
             RALPH_WAVE_WORKER=1. The dispatcher injects all three handshake \
             variables atomically; a partial binding means the spawn-time \
             contract was broken and the emit cannot be routed to a registry- \
             bound channel. Refusing rather than falling back to the legacy \
             events.jsonl default."
        );
    }

    // U3 (2026-07-06-002 plan, R3): cwd 漂移硬约束。当 hat 在
    // isolated 模式下运行、`RALPH_CURRENT_HAT` 已设置、未注入
    // `RALPH_EVENTS_FILE`,且 `--file` 仍是默认值(用户没显式指定
    // 落盘位置)时,如果进程的 `current_dir()` **离开** `workspace_root`
    // 子树(例如跑到无关工程目录、或其它仓库根),可能再次发生
    // `docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md`
    // 里描述的事件孤儿(`sorts/.ralph/events.jsonl`)——我们拒绝这类
    // 默认落盘并提示修复。
    //
    // 判别口径:
    // - cwd == workspace_root:不漂移(workspace_root 直发场景)。
    // - cwd 在 workspace_root 子树内(但不等于 workspace_root,
    //   e.g. `cd sorts/`):`resolve_emit_path` 仍按 workspace_root
    //   锚定解析,本 gate **不**触发;`sorts/.ralph/events.jsonl`
    //   孤儿由 U1/U2 在下一层 marker 解析 + orphan guard 拦截。
    // - cwd 在 workspace_root **外**:drift,触发本 gate(避免创建
    //   与 workspace 无关的父目录 events 文件)。
    //
    // 豁免:
    // - `RALPH_EMIT_ALLOW_CWD_DRIFT=1` 测试旁路(仅 for unit tests in
    //   the same crate that introspect drift 后行为而不希望 gate 在
    //   测试进程的 root cwd 上误触发;生产代码不应设置)。
    // - 显式非默认 `--file` 命中 allowlist 的高级场景。
    let drift_bypass_for_test =
        std::env::var("RALPH_EMIT_ALLOW_CWD_DRIFT").ok().as_deref() == Some("1");
    if !drift_bypass_for_test
        && isolated_mode
        && hat.is_some()
        && env_events_file.is_none()
        && is_default_file_arg(&args.file)
    {
        let cwd = std::env::current_dir().unwrap_or_default();
        let workspace_canon = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.clone());
        let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
        let drift_outside = cwd_canon != workspace_canon
            && !cwd_canon.starts_with(&workspace_canon)
            && !cwd_canon.starts_with(&workspace_root);
        if drift_outside {
            // U5 (R6): stdout summary before `bail!`. Even when
            // stderr 已被前端 tail 截断,agent 仍能看到
            // `emit rejected [cwd_workspace_drift]: ...` 这一行作为
            // fail-closed 信号。
            let summary = format!(
                "current_dir={} workspace_root={}",
                cwd.display(),
                workspace_root.display()
            );
            print_emit_reject_summary(args.output == "json", "cwd_workspace_drift", &summary);
            bail_cwd_workspace_drift(&cwd, &workspace_root)?;
        }
    }

    // An active per-hat marker is a runner-owned routing context. If a
    // business emit has lost its hat identity, never let path resolution fall
    // back to the main ledger. Control and diagnostic topics remain allowed
    // without a business hat, so this check must happen after the effective
    // topic is known rather than inside the topic-agnostic path resolver.
    let active_hat_channel = fs::read_to_string(workspace_root.join(".ralph/current-hat-events"))
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let is_control_or_diagnostic = ralph_core::event_origin::is_ralph_control_topic(topic)
        || ralph_core::event_origin::is_orchestrator_diagnostic_topic(topic);
    if hat.is_none() && active_hat_channel && !is_control_or_diagnostic {
        anyhow::bail!(
            "agent emit context incomplete: active current-hat-events marker exists, but the \
             emitting hat identity is missing; refusing to fall back to the main events ledger. \
             Restore RALPH_CURRENT_HAT (or pass --hat) and retry."
        );
    }

    let events_file = match resolve_emit_path(
        &workspace_root,
        &args.file,
        env_events_file.as_deref(),
        hat.as_deref(),
        isolated_mode,
        wave_id_env.as_deref(),
        slot_index_env,
        loop_id_env.as_deref(),
    ) {
        Ok(path) => path,
        Err(err) => {
            // U5 (2026-07-06-002 plan, R6): emit 失败 stdout 摘要。
            // 不依赖 stderr 截断,这里在 `bail!` 前(已经构造了 anyhow::Error)
            // 显式 print 一行机器可读 prefix + 短描述。
            // stderr 上的 tracing 不受影响(详见 emit_channel_routing_fallback_diagnostic)。
            print_emit_reject_summary(
                args.output == "json",
                "path_resolution_failed",
                &format!("{err:#}"),
            );
            // Agent-context hint: when the caller injected a target via
            // `RALPH_EVENTS_FILE` but the path is not in this loop's
            // events allowlist, the most common cause is an outer hat
            // leaking RALPH_* into a human-CLI invocation (HARD RULE 5).
            // Print an extra stderr line with an actionable fix so the
            // agent / user immediately knows `unset RALPH_*` is the fix
            // rather than guessing against the allowlist. The stdout
            // reject summary stays stable (R5 contract) so jq / CI
            // assertions are unaffected.
            if env_events_file.is_some() {
                eprintln!(
                    "hint: RALPH_EVENTS_FILE came from an outer hat context but is not \
                     in this loop's events allowlist. Most likely cause is hat env \
                     leakage. Fix:\n  \
                     unset RALPH_CURRENT_HAT RALPH_CURRENT_LOOP_ID RALPH_EVENTS_FILE \\\n    \
                     RALPH_WAVE_WORKER RALPH_TRIGGERED_HAT RALPH_HATS_SOURCE RALPH_CONFIG\n  \
                     Then re-run `ralph emit`. Subprocess spawns should call\n  \
                     `scrub_agent_runtime_env()` from crates/ralph-cli/tests/common/mod.rs."
                );
            }
            return Err(err);
        }
    };

    // P0-2 (2026-07-02-005 BP1-3): explicit `--policy-check` is a dry-run
    // probe — validation already ran above; do not append to JSONL.
    // Enforce mode (config-mandated check before write) still writes.
    // Path allowlist already ran so dry-run and apply agree on destination.
    if check_mode == PolicyCheckMode::ExplicitCheck {
        // U8 (2026-07-06-001 plan): `--output json` 路径下打印
        // EmitResult JSON（ok=true, recorded=false）作为
        // policy-check 通过的机器可读信号。text 模式保持原有 stdout
        // "Policy check passed" 行不变（向后兼容）。
        if args.output == "json" {
            let parts = crate::policy_check::build_emit_result_parts(
                topic.to_string(),
                true,
                false,
                Vec::new(),
                config.as_ref(),
                &workspace_root,
                hat.as_deref(),
                // recorded=false：契约仍省略 target_path（见 ralph-tools-emit.md）；
                // 落点已在上方 resolve_emit_path 校验，失败则不会走到这里。
                None,
                // U2: thread the payload so the
                // `handoff_envelope` summary can be extracted
                // when the typed `emit_result_summary` flag
                // is on.
                Some(payload_for_summary.as_str()),
            );
            let result = ralph_core::emit_result::EmitResult::assemble(parts);
            println!(
                "{}",
                serde_json::to_string(&result).context("Failed to serialise EmitResult JSON")?
            );
        } else if use_colors {
            println!(
                "{}✓{} Policy check passed: {} (not written to disk)",
                colors::GREEN,
                colors::RESET,
                topic
            );
        } else {
            println!("Policy check passed: {} (not written to disk)", topic);
        }
        // U5 (plan 2026-07-30-004): advertise the evaluation token so the
        // agent can apply the pre-checked payload with
        // `--policy-check-token`. Only printed when the gate is active
        // (agent context + no event-policy pipeline); otherwise
        // `u5_gate.token` is `None` and the output shape stays stable for
        // presets that already enforce the contract via the unified
        // pipeline. The token binds the exact (hat, topic, payload,
        // contract revision) that was just pre-checked.
        if let Some(token) = u5_gate.token(hat.as_deref(), topic, payload.as_str()) {
            let envelope = serde_json::json!({
                "policy_check_token": token,
                "topic": topic,
                "hat": hat,
                "contract_revision": u5_gate.resolved_digest(),
            });
            if let Ok(line) = serde_json::to_string(&envelope) {
                println!("{line}");
            }
        }
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = events_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Append to file
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    // Write as single-line JSON (JSONL format)
    let json_line = serde_json::to_string(&record)?;
    writeln!(file, "{}", json_line)?;

    // Success message
    // U9 (2026-07-06-001 plan): apply 路径下 `--output json` 打印
    // EmitResult JSON(ok=true, recorded=true) 作为 apply 成功的
    // 机器可读信号。
    //
    // U4 (2026-07-06-002 plan, R5): apply 成功 → EmitResult.target_path
    // 必须为绝对路径,脚本消费者能据此验证事件落到合法位置。text 模式
    // 同步增强为 `Event emitted: <topic> → <absolute_path>`,便于在
    // tail / CI 截断 stderr 的场景下肉眼核对(参见诊断
    // `2026-07-06-ce-executor-ralph-emit-pwd-sorts`)。
    let target_path_str = events_file.display().to_string();
    if args.output == "json" {
        let parts = crate::policy_check::build_emit_result_parts(
            topic.to_string(),
            true,
            true,
            Vec::new(),
            config.as_ref(),
            &workspace_root,
            hat.as_deref(),
            Some(target_path_str.clone()),
            // U2: thread the payload so the `handoff_envelope`
            // summary can be extracted when the typed
            // `emit_result_summary` flag is on.
            Some(payload_for_summary.as_str()),
        );
        let result = ralph_core::emit_result::EmitResult::assemble(parts);
        println!(
            "{}",
            serde_json::to_string(&result).context("Failed to serialise EmitResult JSON")?
        );
    } else if use_colors {
        println!(
            "{}✓{} Event emitted: {} → {}",
            colors::GREEN,
            colors::RESET,
            topic,
            target_path_str
        );
    } else {
        println!("Event emitted: {} → {}", topic, target_path_str);
    }

    Ok(())
}
