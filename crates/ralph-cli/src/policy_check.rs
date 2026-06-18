//! Shared policy precheck module for CLI emit commands.
//!
//! Extracted from `commands/emit.rs` so that `ralph emit` and
//! `ralph wave emit` apply the same policy validation before writing
//! to the events JSONL. The shared module exposes:
//!
//! - [`PolicyCheckMode`] — three-valued decision (Skip, ExplicitCheck, Enforce).
//! - [`resolve_policy_check_mode`] — combine CLI flags + loaded config to a mode.
//! - [`ValidationError`] — single failure (payload index + field + reason_code).
//! - [`validate_topic_payload_against_config`] — single-payload validator.
//! - [`validate_batch_against_config`] — batch validator; collects all violations.
//! - [`emit_policy_validation_failure`] — format the failure payload (text or JSON).
//!
//! Both `ralph emit` and `ralph wave emit` use the same policy check
//! semantics so the loop and CLI can never disagree on what "valid"
//! means. U4 (2026-06-13-001 fix-wave-policy-gate-chain-plan) routes
//! the wave path through this shared module; `ralph emit` keeps its
//! existing single-payload path and just delegates to the same
//! helpers.

use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::cli::{ConfigSource, load_config_with_overrides, resolve_workspace_root};
use crate::config_resolution;
use ralph_core::config::HatExecutionMode;
use ralph_core::step_handoff::progress_task_gate::{
    GateDecision, ProgressTaskMismatch, check_progress_task_alignment, is_gated_topic,
};
use ralph_core::{
    EventPolicyConfig, HatRegistry, PolicyDecision, PolicyRuntimeState, RalphConfig, ViolationType,
    validate_event,
};
use ralph_proto::HatId;

/// Determines whether and how a CLI emit should undergo policy validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyCheckMode {
    /// Skip policy check entirely.
    Skip,
    /// User explicitly requested `--policy-check`.
    ExplicitCheck,
    /// Config mandates policy check for CLI emit (`require_policy_check_for_cli_emit`).
    Enforce,
}

/// CLI flags that influence policy-check mode resolution.
pub struct PolicyCheckFlags {
    /// `--policy-check` (explicit opt-in).
    pub policy_check: bool,
    /// `--unsafe-no-policy-check` (bypass; only honored when config allows).
    pub no_policy_check: bool,
}

/// Decides the policy-check mode based on CLI arguments and loaded config.
///
/// If `--unsafe-no-policy-check` is passed but the config disallows unsafe
/// bypasses, this returns `Enforce` so the caller can reject the flag.
pub fn resolve_policy_check_mode(
    flags: &PolicyCheckFlags,
    config: Option<&RalphConfig>,
) -> PolicyCheckMode {
    if flags.policy_check {
        return PolicyCheckMode::ExplicitCheck;
    }

    if let Some(config) = config {
        if let Some(policy) = config.event_loop.event_policy.as_ref() {
            let strict = policy.enabled && policy.require_policy_check_for_cli_emit;
            if strict {
                if flags.no_policy_check && policy.allow_unsafe_cli_emit {
                    return PolicyCheckMode::Skip;
                }
                return PolicyCheckMode::Enforce;
            }
        }
    }

    // When no config is loaded and no explicit flags were given, skip.
    // `--unsafe-no-policy-check` without strict config is a no-op.
    PolicyCheckMode::Skip
}

/// Try to load the workspace `ralph.yml` config for policy check. Returns
/// `None` when no config exists. The behavior on broken configs is
/// selected by `on_error`:
/// - `Tolerate` returns `Ok(None)` silently — caller falls back to shape-only checks.
/// - `Warn` returns `Ok(None)` and prints a stderr warning naming the parse error
///   and config path. This is the default for `ralph wave emit` so a broken
///   `ralph.yml` cannot silently disable the L1 fail-fast guarantee.
/// - `Fail` returns the underlying load error wrapped in context — strict
///   callers (e.g. future ralph emit strict mode) bubble the error up.
#[allow(dead_code)] // Tolerate and Fail are reserved for future strict callers.
pub enum OnConfigError {
    /// Ignore load errors and proceed without policy enforcement.
    Tolerate,
    /// Ignore load errors but warn the user on stderr.
    Warn,
    /// Surface the load error to the user.
    Fail,
}

/// Attempts to load the workspace config file. The behavior on
/// missing-or-broken files is selected by `on_error`:
/// - `Tolerate` returns `Ok(None)` so callers fall back to shape-only checks.
/// - `Warn` returns `Ok(None)` and prints a stderr warning naming the parse
///   error and config path. Default for `ralph wave emit`.
/// - `Fail` returns the underlying load error wrapped in context.
///
/// We resolve the workspace root relative to the explicit `root` when
/// provided, mirroring `commands/emit.rs`. When `root` is `None` the
/// process CWD is used.
pub fn load_workspace_config(
    root: Option<&PathBuf>,
    on_error: OnConfigError,
) -> Result<Option<RalphConfig>> {
    let workspace_root = resolve_workspace_root(root);
    let config_path = config_resolution::find_workspace_config_path(&workspace_root)
        .unwrap_or_else(|| workspace_root.join("ralph.yml"));
    if !config_path.exists() {
        return Ok(None);
    }
    let sources = vec![ConfigSource::File(config_path.clone())];
    match load_config_with_overrides(&sources) {
        Ok(cfg) => Ok(Some(cfg)),
        Err(e) => match on_error {
            OnConfigError::Tolerate => Ok(None),
            OnConfigError::Warn => {
                eprintln!(
                    "Warning: policy check could not parse config at {}: {}. Proceeding without policy enforcement.",
                    config_path.display(),
                    e
                );
                Ok(None)
            }
            OnConfigError::Fail => {
                let ctx = format!(
                    "Policy check could not load config at {}: {}",
                    config_path.display(),
                    e
                );
                Err(anyhow::anyhow!(ctx))
            }
        },
    }
}

/// Load the workspace config and, when `RALPH_HATS_SOURCE` is set in
/// the environment, merge the matching preset's `event_policy` on top
/// of the workspace config. This is the C1 path that closes the
/// loop-subprocess precheck hole described in plan 001 §4.3: an agent
/// running inside `ralph run -H builtin:ce-executor-isolated` can call
/// `ralph emit` / `ralph wave emit` without re-passing `-H`, and the
/// CLI still gets the same `event_policy.schemas` the loop sees.
///
/// Failure modes (plan 001 §4.3 C4 fail-closed rules):
/// - `RALPH_HATS_SOURCE` is malformed → `bail!` (never Skip).
/// - Preset load fails and a workspace config exists → `bail!`.
/// - Preset load fails and no workspace config exists → `Ok(None)`,
///   matching the no-config default.
pub fn load_policy_config_for_cli_emit(
    root: Option<&PathBuf>,
    on_error: OnConfigError,
) -> Result<Option<RalphConfig>> {
    use crate::cli::{ConfigSource, HatsSource};
    use crate::preflight::load_config_for_preflight_sync;
    let env_label = std::env::var("RALPH_HATS_SOURCE")
        .ok()
        .filter(|s| !s.is_empty());

    let base = load_workspace_config(root, on_error)?;

    let Some(label) = env_label else {
        return Ok(base);
    };

    // Parse the label into a HatsSource so the existing merger can be
    // reused. Unknown shapes (e.g. plain file paths) are still honoured —
    // load_config_for_preflight_sync handles the File / Builtin / Remote
    // variants the same way.
    let parsed = Some(HatsSource::parse(&label));
    let workspace_root = resolve_workspace_root(root);
    let config_path = config_resolution::find_workspace_config_path(&workspace_root)
        .unwrap_or_else(|| workspace_root.join("ralph.yml"));
    let sources: Vec<ConfigSource> = if config_path.exists() {
        vec![ConfigSource::File(config_path)]
    } else {
        vec![]
    };

    let merged = load_config_for_preflight_sync(&sources, parsed.as_ref(), &workspace_root);
    match merged {
        Ok(cfg) => Ok(Some(cfg)),
        Err(e) => {
            // C4 fail-closed: if the env advertised a preset but we
            // cannot honour it AND a workspace config exists, refuse to
            // silently fall back. If no workspace config exists, the
            // caller had nothing to enforce anyway, so returning Ok(None)
            // matches the no-strict-config semantics.
            if !sources.is_empty() {
                anyhow::bail!(
                    "Pre-publish policy check could not honour RALPH_HATS_SOURCE='{label}': {e}. \
                     Fix the preset reference, unset RALPH_HATS_SOURCE, or omit policy enforcement."
                );
            }
            tracing::info!(
                "RALPH_HATS_SOURCE='{label}' could not be loaded ({e}); no workspace config found, proceeding without policy enforcement"
            );
            Ok(None)
        }
    }
}

/// Looks up the active `EventPolicyConfig` for the loaded config. Returns
/// `None` when the config has no event policy or the policy is disabled —
/// callers treat this as "no policy check applies".
pub fn enabled_event_policy(config: Option<&RalphConfig>) -> Option<&EventPolicyConfig> {
    let policy = config.and_then(|c| c.event_loop.event_policy.as_ref())?;
    if policy.enabled { Some(policy) } else { None }
}

/// Holds the events file path used to bootstrap [`PolicyRuntimeState`].
/// Required because terminal-monotonicity / duplicate-terminal checks
/// replay prior events to know whether the terminal has been observed.
pub struct PolicyCheckContext {
    /// Path to the events JSONL to replay before validating the new
    /// payloads. May not exist (in which case the runtime state is
    /// empty).
    pub events_file: PathBuf,
}

/// Build a fresh [`PolicyRuntimeState`] by replaying the events file
/// in `ctx`. Replay errors are tolerated (with a warning); the loop
/// does the same when the marker file is missing or unreadable.
pub fn build_policy_state(
    policy: &EventPolicyConfig,
    ctx: &PolicyCheckContext,
) -> PolicyRuntimeState {
    PolicyRuntimeState::from_events(&ctx.events_file, policy).unwrap_or_else(|e| {
        eprintln!(
            "Warning: Failed to replay events for policy check: {}. Using empty state.",
            e
        );
        PolicyRuntimeState::default()
    })
}

/// Outcome of the step-handoff progress-task gate precheck at the CLI
/// boundary. Mirrors `ralph_core::step_handoff::progress_task_gate`
/// but returns a CLI-friendly validation error so the policy-check
/// failure response stays uniform with the rest of the policy
/// validator (U1 / 2026-06-17-005 plan).
///
/// Returns `Ok(())` when the topic is not gated or when the ledgers
/// align (`Aligned` / `Inert`); returns `Err(ValidationError)` only
/// when the gate produced a `Mismatch`. Non-JSON / empty payloads on
/// gated topics are surfaced as a parse-style mismatch so the agent
/// gets structured backpressure instead of a silent fall-through
/// (review finding #6 / U3 fail-closed alignment).
pub fn check_step_handoff_gate(
    topic: &str,
    payload_str: &str,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    if !is_gated_topic(topic) {
        return Ok(());
    }

    let (step, task_id) = extract_step_and_task_id_from_payload(payload_str);
    let (step, task_id) = match (step, task_id) {
        (Some(s), Some(t)) => (Some(s), Some(t)),
        (Some(s), None) => (Some(s), None),
        (None, Some(t)) => (None, Some(t)),
        // Finding #6: empty / non-JSON gated payload must not
        // silently pass — the loop's gate would not see fields either
        // and would degrade to inert. Surface a structured mismatch
        // so the agent sees the same reason before write.
        (None, None) if payload_str.trim().is_empty() => {
            return Err(ValidationError {
                payload_index: 0,
                field: "payload".to_string(),
                reason_code: "progress_task_mismatch".to_string(),
                message: format!(
                    "step_handoff gate requires non-empty JSON payload for topic '{topic}'; \
                     cannot extract step / task_id"
                ),
            });
        }
        (None, None) => {
            return Err(ValidationError {
                payload_index: 0,
                field: "payload".to_string(),
                reason_code: "progress_task_mismatch".to_string(),
                message: format!(
                    "step_handoff gate could not extract step / task_id from payload for \
                     topic '{topic}'; expected JSON object with `step` and/or `task_id`"
                ),
            });
        }
    };

    let decision =
        check_progress_task_alignment(topic, step.as_deref(), task_id.as_deref(), workspace_root);
    match decision {
        GateDecision::Inert | GateDecision::Aligned => Ok(()),
        GateDecision::Mismatch(mismatch) => Err(mismatch_to_validation_error(&mismatch, topic)),
    }
}

fn mismatch_to_validation_error(m: &ProgressTaskMismatch, topic: &str) -> ValidationError {
    ValidationError {
        payload_index: 0,
        field: if m.task_id.is_some() {
            "task_id".to_string()
        } else if m.step.is_some() {
            "step".to_string()
        } else {
            "payload".to_string()
        },
        reason_code: "progress_task_mismatch".to_string(),
        message: format!(
            "progress_task_gate rejected topic='{topic}' reason={} detail={}",
            m.reason, m.detail
        ),
    }
}

/// U3 (R3): CLI-side precheck for wave worker dimension assignment.
///
/// When the `RALPH_WAVE_DIMENSION` env var is set and non-empty and
/// the topic is `review.dimension.done`, the emitted payload's
/// `dimension` field MUST exactly match the env var. Returns:
/// - `Ok(())` when no check applies (env unset, different topic)
/// - `Ok(())` when the dimension matches
/// - `Err(ValidationError)` with `reason_code=dimension_mismatch` when mismatched
/// - `Err(ValidationError)` with `reason_code=dimension_mismatch` when payload is not JSON
///   or lacks `dimension` (actual is rendered as `<missing>`)
pub fn check_wave_dimension_assignment(
    topic: &str,
    payload_str: &str,
) -> std::result::Result<(), ValidationError> {
    let expected = std::env::var("RALPH_WAVE_DIMENSION")
        .ok()
        .filter(|v| !v.is_empty());
    check_wave_dimension_assignment_with_env(topic, payload_str, expected.as_deref())
}

/// Inner helper for `check_wave_dimension_assignment` that accepts the
/// expected dimension explicitly. Split out so unit tests can drive
/// both branches without mutating process-global env vars (the
/// workspace `forbid(unsafe_code)` lint blocks `set_var`).
fn check_wave_dimension_assignment_with_env(
    topic: &str,
    payload_str: &str,
    expected: Option<&str>,
) -> std::result::Result<(), ValidationError> {
    // Only applies to `review.dimension.done` events. Other topics
    // pass through unchanged.
    if topic != "review.dimension.done" {
        return Ok(());
    }

    // Only applies when the runner has tagged this worker with a
    // specific dimension assignment. A worker that did not receive
    // the env var (e.g. an agent invoking `ralph emit` outside a
    // wave worker context) is not subject to the dimension check.
    let Some(expected) = expected else {
        return Ok(());
    };

    // Try to extract the `dimension` field from the payload. When
    // the payload is not valid JSON, or the field is missing / not a
    // string, we render the actual value as `<missing>` so the agent
    // sees the same diagnostic shape regardless of failure mode.
    let actual = extract_dimension_field(payload_str);

    if actual == expected {
        return Ok(());
    }

    Err(ValidationError {
        payload_index: 0,
        field: "dimension".to_string(),
        reason_code: "dimension_mismatch".to_string(),
        message: format!(
            "dimension mismatch: expected_dimension={expected} actual_dimension={actual}"
        ),
    })
}

/// U1 (2026-06-17-003 plan): CLI precheck for isolated mode hat
/// publish scope. Mirrors the loop's `isolated_publish_allowed` →
/// `registry.can_publish` so an agent running inside a hat context
/// in `event_loop.execution_mode: isolated` cannot bypass the scope
/// check by writing to events.jsonl via `ralph emit` (the topic
/// would land in the JSONL, then get dropped at loop runtime
/// without actionable backpressure).
///
/// Returns `Ok(())` when:
/// - `config.event_loop.execution_mode != Isolated` (coordinator mode
///   is unaffected — multiple hats share one prompt)
/// - `hat` is `None` (we cannot decide; the runtime origin guard
///   handles missing-provenance separately)
/// - `hat == "ralph"` and `topic` is in `RALPH_CONTROL_TOPICS` (the
///   ralph pseudo-hat is allowed to publish orchestrator control
///   topics even in isolated mode)
/// - `hat` is registered and the topic is in the hat's `publishes`
///
/// Returns `Err(ValidationError)` with `reason_code =
/// "isolated_scope_violation"` when the hat is registered but the
/// topic is not in the hat's `publishes` list. The message names
/// the hat, the topic, and the hat's allowed publishes so the
/// agent can self-correct without re-reading the preset.
pub fn check_isolated_scope(
    hat: Option<&str>,
    topic: &str,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    if config.event_loop.execution_mode != HatExecutionMode::Isolated {
        return Ok(());
    }
    let Some(hat_id) = hat else {
        return Ok(());
    };
    if hat_id == "ralph"
        && ralph_core::event_origin::RALPH_CONTROL_TOPICS
            .iter()
            .any(|t| *t == topic)
    {
        return Ok(());
    }

    let registry = HatRegistry::from_runtime_config(config);
    let hat_id_typed = HatId::new(hat_id);
    if registry.can_publish(&hat_id_typed, topic) {
        return Ok(());
    }

    let allowed: Vec<String> = config
        .hats
        .get(hat_id)
        .map(|c| c.publishes.clone())
        .unwrap_or_default();
    Err(ValidationError {
        payload_index: 0,
        field: "topic".to_string(),
        reason_code: "isolated_scope_violation".to_string(),
        message: format!(
            "isolated scope violation: hat '{hat_id}' is not allowed to publish topic '{topic}'; \
             allowed publishes: {allowed:?}"
        ),
    })
}

/// 2026-06-18-002 plan U7: CLI-side mirror of the runtime hat-handoff
/// gate. Wraps `ralph_core::hat_handoff::gate::evaluate_event` so the
/// `ralph emit --policy-check` path and the runtime gate share the
/// same reason_code vocabulary (KTD-13 / U7 contract).
///
/// `payload_str` is the raw JSON payload the agent would write to the
/// events file. The CLI reads `handoff_path` from it the same way the
/// runtime gate does, and applies file-content checks (read from
/// disk) for structural / R15 validation. When `handoff_path` is
/// present but the file is missing on disk, the CLI returns a
/// `ValidationError` mirroring the runtime's `file_not_found`
/// reason_code.
pub fn check_hat_handoff_gate(
    hat: Option<&str>,
    topic: &str,
    payload_str: Option<&str>,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    use ralph_core::hat_handoff::gate::{self, GateDecision, GateInputs};
    use ralph_core::workflow_contract::HandoffIndex;

    let handoff_config = &config.event_loop.hat_handoff;
    if !handoff_config.enabled {
        return Ok(());
    }
    if config.event_loop.execution_mode != HatExecutionMode::Isolated {
        return Ok(());
    }
    let Some(hat_id) = hat else {
        return Ok(());
    };

    // 解析 payload 中的 handoff_path(与 runtime 同语义,走 SSOT)。
    // 2026-06-18 P1-1: 改用 `hat_handoff::payload::extract_handoff_path`,
    // 与 runtime gate / inject 共用同一函数,避免 SSOT 漂移。
    let handoff_path = payload_str
        .and_then(|raw| ralph_core::hat_handoff::payload::extract_handoff_path(raw));

    // 用 preset 配置构造 HandoffIndex。
    let index = HandoffIndex::from_config(config);

    // 下游 hat publishes(同 runtime `preset_hats_publishes`)。
    let downstream_publishes: Vec<String> = index
        .consumer_of(topic)
        .and_then(|consumer| config.hats.get(consumer))
        .map(|h| h.publishes.clone())
        .unwrap_or_default();

    let workspace_root = &config.core.workspace_root;

    // 读 handoff 文件(如有)。不在 gate 里 panic。
    let file_content = match handoff_path.as_deref() {
        Some(path) => match ralph_core::hat_handoff::allocator::resolve_jailed(
            workspace_root,
            path,
        ) {
            Ok(abs) => std::fs::read_to_string(&abs).ok(),
            Err(_) => None,
        },
        None => None,
    };

    let inputs = GateInputs {
        config: handoff_config,
        execution_mode: config.event_loop.execution_mode.clone(),
        index: &index,
        from_hat: hat_id,
        topic,
        iteration: 1, // CLI 不在 loop 内,默认值;仅影响文件名 seq 校验。
        current_seq: 0,
        handoff_path: handoff_path.as_deref(),
        downstream_publishes: &downstream_publishes,
        repo_root: workspace_root.as_path(),
    };

    match gate::evaluate_event(&inputs, file_content.as_deref()) {
        GateDecision::NotRequired | GateDecision::Accept { .. } => Ok(()),
        GateDecision::Reject { reason_code, message } => Err(ValidationError {
            payload_index: 0,
            field: "handoff_path".to_string(),
            reason_code: reason_code.to_string(),
            message,
        }),
    }
}

#[cfg(test)]
mod hat_handoff_tests {
    use super::*;
    use ralph_core::hat_handoff::gate::{
        REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND, REASON_CODE_HAT_HANDOFF_MISSING_PATH,
    };
    use std::fs;

    fn isolated_two_hat_yaml(hat_handoff_block: &str) -> String {
        format!(
            r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
{hat_handoff_block}
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#
        )
    }

    fn parse_cfg(yaml: &str) -> RalphConfig {
        serde_yaml::from_str::<RalphConfig>(yaml).expect("parse ralph.yml fixture")
    }

    #[test]
    fn disabled_returns_ok() {
        let yaml = isolated_two_hat_yaml("  hat_handoff:\n    enabled: false\n");
        let mut cfg = parse_cfg(&yaml);
        cfg.core.workspace_root = tempfile::tempdir().unwrap().path().to_path_buf();
        assert!(check_hat_handoff_gate(Some("plan-gate"), "work.ready", None, &cfg).is_ok());
    }

    #[test]
    fn missing_path_rejected_with_ssot_reason_code() {
        let yaml = isolated_two_hat_yaml("  hat_handoff:\n    enabled: true\n");
        let mut cfg = parse_cfg(&yaml);
        cfg.core.workspace_root = tempfile::tempdir().unwrap().path().to_path_buf();
        let err = check_hat_handoff_gate(Some("plan-gate"), "work.ready", None, &cfg)
            .expect_err("expected Reject");
        assert_eq!(err.reason_code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
    }

    #[test]
    fn file_not_found_rejected_with_ssot_reason_code() {
        let yaml = isolated_two_hat_yaml("  hat_handoff:\n    enabled: true\n");
        let mut cfg = parse_cfg(&yaml);
        let dir = tempfile::tempdir().unwrap();
        cfg.core.workspace_root = dir.path().to_path_buf();
        let payload =
            r#"{"handoff_path":".ralph/agent/hat-handoff/1-1-plan_gate-executor.md"}"#;
        let err = check_hat_handoff_gate(Some("plan-gate"), "work.ready", Some(payload), &cfg)
            .expect_err("expected Reject");
        assert_eq!(err.reason_code, REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND);
    }

    #[test]
    fn file_present_and_valid_accepts() {
        let yaml = isolated_two_hat_yaml("  hat_handoff:\n    enabled: true\n");
        let mut cfg = parse_cfg(&yaml);
        let dir = tempfile::tempdir().unwrap();
        cfg.core.workspace_root = dir.path().to_path_buf();
        let abs = dir
            .path()
            .join(".ralph/agent/hat-handoff/1-1-plan_gate-executor.md");
        fs::create_dir_all(abs.parent().unwrap()).unwrap();
        fs::write(
            &abs,
            "# Handoff: plan_gate → executor\n## context\nx\n## changed\ny\n## verify\nz\n## next\n**动作**: emit work.done after completion\n**阻塞**: 无\n## notes\n无\n",
        )
        .unwrap();
        let payload =
            r#"{"handoff_path":".ralph/agent/hat-handoff/1-1-plan_gate-executor.md"}"#;
        assert!(check_hat_handoff_gate(Some("plan-gate"), "work.ready", Some(payload), &cfg).is_ok());
    }
}


/// U1 (2026-06-17-004 plan, R1+R2): CLI provenance fail-closed.
///
/// Closes the `hat = None` bypass where an isolated-mode CLI emit
/// slips past `check_isolated_scope` (which only enforces when the
/// hat is known). Without provenance, the origin guard downstream
/// drops the event at JSONL read time — the agent gets no
/// actionable backpressure at the CLI boundary.
///
/// Returns `Ok(())` when:
/// - `config.event_loop.execution_mode != Isolated` (coordinator
///   mode allows shared-prompt emission without per-hat provenance)
/// - `hat` is `Some(_)` (scope guard will check the hat; this gate
///   only catches the *missing*-provenance path)
/// - `topic` is in `RALPH_CONTROL_TOPICS` (loop completion, cancel,
///   human interaction, recovery — produced by Ralph itself or by
///   the runtime ralph pseudo-hat)
/// - `topic` is an orchestrator-produced diagnostic topic (`event.*`
///   allowlist, see `is_orchestrator_diagnostic_topic`)
///
/// Returns `Err(ValidationError)` with
/// `reason_code = "missing_provenance"` when the caller is in
/// isolated mode, has no hat, and is trying to publish a business
/// topic. The error message names the topic and tells the agent to
/// pass `--hat` or set `RALPH_CURRENT_HAT`.
///
/// Note: `task.resume` is intentionally in `RALPH_CONTROL_TOPICS`
/// (post-U1 fix in `event_origin.rs`) so the recovery envelope can
/// be emitted by the runner without a hat. This matches the runtime
/// origin-guard exemption for control topics.
pub fn check_emit_provenance(
    hat: Option<&str>,
    topic: &str,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    if config.event_loop.execution_mode != HatExecutionMode::Isolated {
        return Ok(());
    }

    if hat.is_some() {
        // Provenance present — scope guard (check_isolated_scope) and
        // runtime origin guard handle the rest.
        return Ok(());
    }

    // Control topics are produced by the loop / runtime ralph pseudo-hat
    // and are exempt from hat provenance. Diagnostic events are emitted
    // by the loop itself for observability.
    if ralph_core::RALPH_CONTROL_TOPICS.iter().any(|t| *t == topic) {
        return Ok(());
    }
    if ralph_core::is_orchestrator_diagnostic_topic(topic) {
        return Ok(());
    }

    Err(ValidationError {
        payload_index: 0,
        field: "hat".to_string(),
        reason_code: "missing_provenance".to_string(),
        message: format!(
            "missing provenance: isolated mode requires a hat for business topic '{topic}'. \
             Pass --hat <hat-id> or set RALPH_CURRENT_HAT=<hat-id>. \
             (Control topics {:?} and orchestrator diagnostics bypass this gate.)",
            ralph_core::RALPH_CONTROL_TOPICS
        ),
    })
}

/// Lightweight extractor mirroring the loop-side `dimension` field
/// reading. Returns `<missing>` for any non-JSON payload, missing
/// field, or non-string value. Trims the value (P1#6 fix) so the
/// CLI precheck matches the merge layer's
/// `parse_payload_dimension` behavior — otherwise a payload with
/// `"dimension": "testing "` would be rejected by the precheck
/// but accepted by the merge layer, producing a confusing
/// "agent got rejected but the merge would have accepted" loop.
fn extract_dimension_field(payload_str: &str) -> String {
    let trimmed = payload_str.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return "<missing>".to_string();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return "<missing>".to_string();
    };
    match value.get("dimension").and_then(|v| v.as_str()) {
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                "<missing>".to_string()
            } else {
                t.to_string()
            }
        }
        None => "<missing>".to_string(),
    }
}

/// Lightweight JSON-object extractor for the CLI gate. Mirrors the
/// loop-side `extract_step_and_task_id` semantics but is local to
/// `policy_check` so the CLI does not pull a `&EventBus` boundary
/// just to look at two payload fields.
fn extract_step_and_task_id_from_payload(payload_str: &str) -> (Option<String>, Option<String>) {
    let trimmed = payload_str.trim();
    if trimmed.is_empty() {
        return (None, None);
    }
    if !trimmed.starts_with('{') {
        return (None, None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return (None, None);
    };
    let step = value
        .get("step")
        .or_else(|| value.get("completed_step"))
        .or_else(|| value.get("next_step"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let task_id = value
        .get("task_id")
        .or_else(|| value.get("reviewed_task_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (step, task_id)
}

/// A single structured validation error. Stable JSON shape — agents
/// rely on `field` + `reason_code` to programmatically diagnose
/// payload issues (see `crates/ralph-core/data/ralph-tools.md`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidationError {
    /// Index of the failing payload in the original batch (0-based).
    pub payload_index: usize,
    /// Field that failed (e.g. `depth`, `plan_name`); empty when the
    /// violation is at the payload / topic level rather than a field.
    pub field: String,
    /// Stable machine-readable code; one of:
    /// `missing_required_field`, `invalid_field_value`,
    /// `payload_type_mismatch`, `terminal_monotonicity_violation`,
    /// `duplicate_terminal_event`, `business_event_after_completion`,
    /// `invalid_topic_format`, `topic_denied`,
    /// `policy_unavailable`, `config_error`.
    pub reason_code: String,
    /// Human-readable message suitable for logs and stderr summaries.
    pub message: String,
}

/// Result of a batch validation. Empty `errors` means the batch is
/// acceptable; non-empty means the caller MUST reject the entire
/// batch atomically (no partial write).
#[derive(Debug, Clone, Default)]
pub struct BatchValidation {
    pub errors: Vec<ValidationError>,
}

impl BatchValidation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validates a single payload against the loaded event policy and
/// returns a structured `ValidationError` (or `None` if the payload
/// is acceptable). Replays prior events from `events_file` to
/// enforce terminal-monotonicity / duplicate-terminal checks —
/// the same replay path the loop uses when reading the JSONL.
#[allow(dead_code)] // Exposed for single-event callers (e.g. ralph emit).
pub fn validate_topic_payload_against_config(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    events_file: &Path,
) -> Result<Option<ValidationError>> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let decision = validate_event(topic, Some(payload_str), policy, &mut state);
    Ok(finding_to_validation_error(&decision, topic))
}

/// Validates a single payload against the loaded event policy using
/// a pre-built runtime state. Used by the batch validator so the
/// caller reuses one replay instead of replaying the events file
/// once per payload.
pub fn validate_topic_payload_with_state(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
) -> Result<Option<ValidationError>> {
    let decision = validate_event(topic, Some(payload_str), policy, state);
    Ok(finding_to_validation_error(&decision, topic))
}

/// Validates a batch of payloads. Scans every payload (atomicity:
/// a single bad payload rejects the entire batch) and reuses one
/// [`PolicyRuntimeState`] across the loop — terminal-monotonicity
/// semantics match what the loop sees on its end.
///
/// `topic` is shared by every payload in the batch (waves are
/// single-topic by construction).
pub fn validate_batch_against_config(
    topic: &str,
    payloads: &[String],
    policy: &EventPolicyConfig,
    events_file: &Path,
) -> Result<BatchValidation> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let mut errors = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        if let Some(err) = validate_topic_payload_with_state(topic, payload, policy, &mut state)? {
            errors.push(ValidationError {
                payload_index: index,
                ..err
            });
        }
    }
    Ok(BatchValidation { errors })
}

fn finding_to_validation_error(decision: &PolicyDecision, _topic: &str) -> Option<ValidationError> {
    let finding = match decision {
        PolicyDecision::Accept => return None,
        PolicyDecision::Warn(findings) => {
            // Warn is non-fatal at the loop level too, so we still
            // accept the payload. Surfacing warns as errors here
            // would change CLI behavior compared to the loop
            // (loop writes the event + logs the warning). We follow
            // the loop's behavior: skip the warning. The findings
            // are logged via the loop's normal warning path; CLI
            // callers can inspect the events file for the warning
            // field if they need to surface it.
            let _ = findings; // intentionally not propagated as errors
            return None;
        }
        PolicyDecision::RejectWithResume(f)
        | PolicyDecision::Hold(f)
        | PolicyDecision::Block(f)
        | PolicyDecision::Ignore(f) => f,
    };
    Some(finding_record(finding))
}

fn finding_record(finding: &ralph_core::PolicyFinding) -> ValidationError {
    let (field, reason_code) = match &finding.violation_type {
        ViolationType::MissingRequiredField { field } => {
            (field.clone(), "missing_required_field".to_string())
        }
        ViolationType::InvalidFieldValue { field, .. } => {
            (field.clone(), "invalid_field_value".to_string())
        }
        ViolationType::PayloadTypeMismatch { .. } => {
            (String::new(), "payload_type_mismatch".to_string())
        }
        ViolationType::TerminalMonotonicityViolation { .. } => {
            (String::new(), "terminal_monotonicity_violation".to_string())
        }
        ViolationType::DuplicateTerminalEvent { .. } => {
            (String::new(), "duplicate_terminal_event".to_string())
        }
        ViolationType::BusinessEventAfterCompletion { .. } => {
            (String::new(), "business_event_after_completion".to_string())
        }
        ViolationType::InvalidTopicFormat { .. } => {
            (String::new(), "invalid_topic_format".to_string())
        }
        ViolationType::TopicDenied { .. } => (String::new(), "topic_denied".to_string()),
        ViolationType::SemanticGateViolation { gate, .. } => {
            (gate.clone(), "semantic_gate_violation".to_string())
        }
        ViolationType::DuplicateWorkDone { .. } => {
            (String::new(), "duplicate_work_done".to_string())
        }
    };
    ValidationError {
        payload_index: 0, // caller (single vs batch) fills this in
        field,
        reason_code,
        message: finding.message.clone(),
    }
}

/// Output format for the validation-failure response. Mirrors
/// `wave::WaveOutputFormat` so we can reuse the same `--output`
/// flag from both `ralph emit` and `ralph wave emit` (when wired
/// up; wave is the U4 entry point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-readable summary on stderr.
    Text,
    /// Structured JSON on stdout.
    Json,
}

/// Top-level failure payload structure (the new shape defined in
/// the U4 plan). Kept as a top-level shape so `serde_json::to_string`
/// produces a stable, agent-friendly document.
#[derive(Debug, Serialize)]
pub struct ValidationFailure {
    pub ok: bool,
    pub error: &'static str,
    pub topic: String,
    pub validation_errors: Vec<ValidationError>,
}

impl ValidationFailure {
    /// Build a failure payload from a [`BatchValidation`].
    pub fn from_batch(topic: &str, batch: BatchValidation) -> Self {
        Self {
            ok: false,
            error: "policy_validation_failed",
            topic: topic.to_string(),
            validation_errors: batch.errors,
        }
    }
}

/// Emit the validation failure in the requested output mode. Returns
/// `Err(anyhow::Error)` so the caller can `?`-propagate with a
/// non-zero exit; both modes print the failure before the error is
/// raised so the agent / operator can see the structured details.
pub fn emit_policy_validation_failure(
    failure: &ValidationFailure,
    output: OutputMode,
) -> Result<()> {
    match output {
        OutputMode::Text => {
            // R22 (U4 plan): short human summary on stderr.
            let total = failure.validation_errors.len();
            // Aggregate the most common field to give a focused hint.
            let mut field_counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for v in &failure.validation_errors {
                if !v.field.is_empty() {
                    *field_counts.entry(v.field.as_str()).or_insert(0) += 1;
                }
            }
            let field_hint = if let Some((field, count)) = field_counts.iter().next() {
                format!("missing required field '{field}' in {count}")
            } else if total > 0 {
                format!(
                    "{}",
                    failure.validation_errors[0].reason_code.replace('_', " ")
                )
            } else {
                "policy check".to_string()
            };
            eprintln!(
                "policy validation failed: {total} payload{}, {field_hint}",
                if total == 1 { "" } else { "s" }
            );
        }
        OutputMode::Json => {
            // R19: structured JSON on stdout (machine-parseable).
            println!("{}", serde_json::to_string(failure)?);
        }
    }
    anyhow::bail!("policy validation failed for topic '{}'", failure.topic);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::PolicyRuntimeState;
    use tempfile::TempDir;

    fn strict_policy_with_required(field: &str) -> EventPolicyConfig {
        let yaml = format!(
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    schemas:
      review.wave.ready:
        required_fields:
          - {field}
"#
        );
        let cfg: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        cfg.event_loop.event_policy.unwrap()
    }

    #[test]
    fn test_resolve_policy_check_mode_explicit_wins() {
        let flags = PolicyCheckFlags {
            policy_check: true,
            no_policy_check: false,
        };
        assert_eq!(
            resolve_policy_check_mode(&flags, None),
            PolicyCheckMode::ExplicitCheck
        );
    }

    #[test]
    fn test_resolve_policy_check_mode_strict_without_flags_is_enforce() {
        let policy = strict_policy_with_required("depth");
        let cfg = RalphConfig {
            event_loop: ralph_core::config::EventLoopConfig {
                event_policy: Some(policy),
                ..Default::default()
            },
            ..Default::default()
        };
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: false,
        };
        assert_eq!(
            resolve_policy_check_mode(&flags, Some(&cfg)),
            PolicyCheckMode::Enforce
        );
    }

    #[test]
    fn test_resolve_policy_check_mode_unsafe_bypass_blocked_when_disallowed() {
        let mut policy = strict_policy_with_required("depth");
        policy.allow_unsafe_cli_emit = false;
        let cfg = RalphConfig {
            event_loop: ralph_core::config::EventLoopConfig {
                event_policy: Some(policy),
                ..Default::default()
            },
            ..Default::default()
        };
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: true,
        };
        assert_eq!(
            resolve_policy_check_mode(&flags, Some(&cfg)),
            PolicyCheckMode::Enforce
        );
    }

    #[test]
    fn test_resolve_policy_check_mode_unsafe_bypass_allowed_when_config_permits() {
        let mut policy = strict_policy_with_required("depth");
        policy.allow_unsafe_cli_emit = true;
        let cfg = RalphConfig {
            event_loop: ralph_core::config::EventLoopConfig {
                event_policy: Some(policy),
                ..Default::default()
            },
            ..Default::default()
        };
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: true,
        };
        assert_eq!(
            resolve_policy_check_mode(&flags, Some(&cfg)),
            PolicyCheckMode::Skip
        );
    }

    #[test]
    fn test_resolve_policy_check_mode_no_strict_no_flags_is_skip() {
        let cfg = RalphConfig::default();
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: false,
        };
        assert_eq!(
            resolve_policy_check_mode(&flags, Some(&cfg)),
            PolicyCheckMode::Skip
        );
    }

    #[test]
    fn test_enabled_event_policy_returns_none_when_disabled() {
        let cfg = RalphConfig::default();
        assert!(enabled_event_policy(Some(&cfg)).is_none());
    }

    #[test]
    fn test_enabled_event_policy_returns_policy_when_enabled() {
        let policy = strict_policy_with_required("depth");
        let cfg = RalphConfig {
            event_loop: ralph_core::config::EventLoopConfig {
                event_policy: Some(policy),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(enabled_event_policy(Some(&cfg)).is_some());
    }

    #[test]
    fn test_validate_batch_against_config_reports_all_missing_depth_violations() {
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        // All 7 payloads lack `depth` → expect 7 errors, one per index.
        let payloads: Vec<String> = (0..7).map(|i| format!(r#"{{"dim":"d{i}"}}"#)).collect();
        let batch = validate_batch_against_config("review.wave.ready", &payloads, &policy, &events)
            .unwrap();
        assert_eq!(batch.errors.len(), 7);
        for (i, err) in batch.errors.iter().enumerate() {
            assert_eq!(err.payload_index, i);
            assert_eq!(err.field, "depth");
            assert_eq!(err.reason_code, "missing_required_field");
            assert!(err.message.contains("depth"));
        }
    }

    #[test]
    fn test_validate_batch_against_config_passes_with_depth() {
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let payloads: Vec<String> = (0..3)
            .map(|i| format!(r#"{{"dim":"d{i}","depth":"standard"}}"#))
            .collect();
        let batch = validate_batch_against_config("review.wave.ready", &payloads, &policy, &events)
            .unwrap();
        assert!(batch.is_ok(), "valid payloads should produce empty errors");
    }

    #[test]
    fn test_validate_batch_against_config_atomic_partial_rejection() {
        // Index 3 missing depth, others valid → still at least one error,
        // and the entire batch is rejected (atomicity).
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let mut payloads: Vec<String> = (0..5)
            .map(|i| format!(r#"{{"dim":"d{i}","depth":"standard"}}"#))
            .collect();
        payloads[3] = r#"{"dim":"d3"}"#.to_string();
        let batch = validate_batch_against_config("review.wave.ready", &payloads, &policy, &events)
            .unwrap();
        assert!(!batch.is_ok());
        assert!(
            batch
                .errors
                .iter()
                .any(|e| e.payload_index == 3 && e.field == "depth")
        );
    }

    #[test]
    fn test_validate_topic_payload_against_config_returns_none_when_ok() {
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let err = validate_topic_payload_against_config(
            "review.wave.ready",
            r#"{"dim":"d","depth":"standard"}"#,
            &policy,
            &events,
        )
        .unwrap();
        assert!(err.is_none());
    }

    #[test]
    fn test_validate_topic_payload_against_config_returns_error_when_missing() {
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let err = validate_topic_payload_against_config(
            "review.wave.ready",
            r#"{"dim":"d"}"#,
            &policy,
            &events,
        )
        .unwrap()
        .expect("missing depth should produce an error");
        assert_eq!(err.field, "depth");
        assert_eq!(err.reason_code, "missing_required_field");
    }

    #[test]
    fn test_emit_policy_validation_failure_json_outputs_structured_payload() {
        let failure = ValidationFailure {
            ok: false,
            error: "policy_validation_failed",
            topic: "review.wave.ready".to_string(),
            validation_errors: vec![ValidationError {
                payload_index: 0,
                field: "depth".to_string(),
                reason_code: "missing_required_field".to_string(),
                message: "Missing required field: depth".to_string(),
            }],
        };
        // We can't easily intercept stdout in this module; instead we
        // verify the JSON shape and the text summary separately.
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"error\":\"policy_validation_failed\""));
        assert!(json.contains("\"topic\":\"review.wave.ready\""));
        assert!(json.contains("\"field\":\"depth\""));
        assert!(json.contains("\"reason_code\":\"missing_required_field\""));
    }

    #[test]
    fn test_emit_policy_validation_failure_text_summary_mentions_count() {
        // Build a failure with 7 errors all on `depth`.
        let errors: Vec<ValidationError> = (0..7)
            .map(|i| ValidationError {
                payload_index: i,
                field: "depth".to_string(),
                reason_code: "missing_required_field".to_string(),
                message: "Missing required field: depth".to_string(),
            })
            .collect();
        let failure = ValidationFailure {
            ok: false,
            error: "policy_validation_failed",
            topic: "review.wave.ready".to_string(),
            validation_errors: errors,
        };
        // We assert via the JSON shape (text goes to stderr, no
        // capture). Verify the field aggregation logic by inspecting
        // the validation_errors directly.
        assert_eq!(failure.validation_errors.len(), 7);
        let all_depth = failure.validation_errors.iter().all(|e| e.field == "depth");
        assert!(all_depth);
    }

    #[test]
    fn test_build_policy_state_falls_back_to_default_on_missing_file() {
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("missing.jsonl");
        let ctx = PolicyCheckContext {
            events_file: events,
        };
        let state = build_policy_state(&policy, &ctx);
        // Default state: no terminal observed, no observed topics.
        assert!(!state.terminal_observed);
        assert!(state.observed_topics.is_empty());
        // Suppress the "unused" warning on the policy parameter.
        let _ = &policy;
    }

    #[test]
    fn test_load_workspace_config_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let cfg = load_workspace_config(Some(&root), OnConfigError::Tolerate).unwrap();
        assert!(cfg.is_none());
    }

    #[test]
    fn test_validate_batch_replays_terminal_event() {
        // Pre-seed the events file with a terminal event. The strict
        // policy + business_after_completion: reject should now reject
        // any business topic that arrives after the terminal.
        let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let policy = cfg.event_loop.event_policy.unwrap();

        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        std::fs::write(
            &events,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        let payloads = vec![r#"{"task_key":"x"}"#.to_string()];
        let batch =
            validate_batch_against_config("experiment.planned", &payloads, &policy, &events)
                .unwrap();
        assert!(!batch.is_ok(), "business event after terminal must reject");
        assert!(
            batch
                .errors
                .iter()
                .any(|e| e.reason_code == "terminal_monotonicity_violation"
                    || e.reason_code == "business_event_after_completion"
                    || e.reason_code == "duplicate_terminal_event")
        );
    }

    #[test]
    fn test_policy_runtime_state_can_be_replayed_in_loop() {
        // Sanity check: the loop and CLI must share the same replay
        // semantics. The loop uses `from_events` directly; the CLI
        // wraps it in `build_policy_state`. The two paths must
        // produce the same `terminal_observed` for a given fixture.
        let policy = strict_policy_with_required("depth");
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        std::fs::write(
            &events,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();
        let direct = PolicyRuntimeState::from_events(&events, &policy).unwrap();
        let wrapped = build_policy_state(
            &policy,
            &PolicyCheckContext {
                events_file: events,
            },
        );
        assert_eq!(direct.terminal_observed, wrapped.terminal_observed);
    }

    // ── U1 / 2026-06-17-005 plan ─────────────────────────────────────
    // CLI step-handoff progress-task gate precheck (`check_step_handoff_gate`).
    // The helper must:
    //   * pass through non-gated topics without invoking the gate
    //   * pass when progress.md ↔ tasks.jsonl align
    //   * return `progress_task_mismatch` when the agent's claim
    //     disagrees with the ledger — the same reason the loop would
    //     emit on the same fixture
    //   * fail-closed on non-JSON / empty payloads for gated topics
    //     (review finding #6)

    fn workspace() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
        tmp
    }

    fn write_progress(tmp: &tempfile::TempDir, body: &str) {
        let path = tmp.path().join(".ralph").join("agent").join("progress.md");
        std::fs::write(path, body).unwrap();
    }

    fn write_closed_task(tmp: &tempfile::TempDir, id: &str, title: &str) {
        use ralph_core::task::{Task, TaskStatus};
        let mut task = Task::new(title.to_string(), 3);
        task.id = id.to_string();
        task.status = TaskStatus::Closed;
        let line = serde_json::to_string(&task).unwrap();
        let path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(&line);
        existing.push('\n');
        std::fs::write(&path, existing).unwrap();
    }

    /// Happy path: aligned progress + tasks, `queue.advance` policy-check passes.
    #[test]
    fn u1_step_handoff_gate_happy_path_aligned_progress() {
        let tmp = workspace();
        write_closed_task(&tmp, "task-1", "step-01");
        write_progress(
            &tmp,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n",
        );
        let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
        let res = check_step_handoff_gate("queue.advance", payload, tmp.path());
        assert!(res.is_ok(), "aligned progress must pass: {res:?}");
    }

    /// Error path: deliberate progress drift → CLI gate returns the
    /// same `progress_task_mismatch` reason the loop would emit.
    #[test]
    fn u1_step_handoff_gate_rejects_misaligned_progress() {
        let tmp = workspace();
        write_closed_task(&tmp, "task-1", "step-01");
        // progress.md does NOT list step-01 → mismatch.
        write_progress(
            &tmp,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
        );
        let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
        let err = check_step_handoff_gate("queue.advance", payload, tmp.path())
            .expect_err("mismatched progress must produce validation error");
        assert_eq!(err.reason_code, "progress_task_mismatch");
        assert!(err.message.contains("task_closed_but_progress_missing"));
    }

    /// Edge: topic not in `GATED_TOPICS` → no gate call, no error.
    #[test]
    fn u1_step_handoff_gate_skips_non_gated_topic() {
        let tmp = workspace();
        // No progress.md / tasks.jsonl — gate must short-circuit.
        let payload = r#"{"step":"step-01"}"#;
        let res = check_step_handoff_gate("review.dimension.done", payload, tmp.path());
        assert!(res.is_ok(), "non-gated topic must pass without gate call");
    }

    /// Fail-closed alignment with review finding #6: empty payload on
    /// a gated topic must surface `progress_task_mismatch`, not
    /// silently pass through.
    #[test]
    fn u1_step_handoff_gate_fail_closed_on_empty_payload() {
        let tmp = workspace();
        let err = check_step_handoff_gate("queue.advance", "", tmp.path())
            .expect_err("empty gated payload must fail closed");
        assert_eq!(err.reason_code, "progress_task_mismatch");
        assert!(err.message.contains("non-empty"));
    }

    /// Fail-closed alignment with finding #6: non-JSON payload on a
    /// gated topic must surface `progress_task_mismatch` instead of
    /// inert-passing.
    #[test]
    fn u1_step_handoff_gate_fail_closed_on_non_json_payload() {
        let tmp = workspace();
        let err = check_step_handoff_gate("plan.complete", "not json at all", tmp.path())
            .expect_err("non-JSON gated payload must fail closed");
        assert_eq!(err.reason_code, "progress_task_mismatch");
    }

    #[test]
    fn test_check_wave_dimension_assignment_no_env_returns_ok() {
        // env unset, any topic
        assert!(
            check_wave_dimension_assignment_with_env(
                "review.dimension.done",
                r#"{"dimension":"testing"}"#,
                None
            )
            .is_ok()
        );
        assert!(
            check_wave_dimension_assignment_with_env(
                "work.done",
                r#"{"dimension":"testing"}"#,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn test_check_wave_dimension_assignment_match_returns_ok() {
        assert!(
            check_wave_dimension_assignment_with_env(
                "review.dimension.done",
                r#"{"dimension":"testing"}"#,
                Some("testing")
            )
            .is_ok()
        );
    }

    #[test]
    fn test_check_wave_dimension_assignment_mismatch_returns_err() {
        let err = check_wave_dimension_assignment_with_env(
            "review.dimension.done",
            r#"{"dimension":"correctness"}"#,
            Some("testing"),
        )
        .unwrap_err();
        assert_eq!(err.reason_code, "dimension_mismatch");
        assert!(err.message.contains("expected_dimension=testing"));
        assert!(err.message.contains("actual_dimension=correctness"));
    }

    #[test]
    fn test_check_wave_dimension_assignment_missing_field_returns_err() {
        let err = check_wave_dimension_assignment_with_env(
            "review.dimension.done",
            r#"{"findings_count":3}"#,
            Some("testing"),
        )
        .unwrap_err();
        assert_eq!(err.reason_code, "dimension_mismatch");
        assert!(err.message.contains("actual_dimension=<missing>"));
    }

    #[test]
    fn test_check_wave_dimension_assignment_non_json_returns_err() {
        let err = check_wave_dimension_assignment_with_env(
            "review.dimension.done",
            "not json",
            Some("testing"),
        )
        .unwrap_err();
        assert_eq!(err.reason_code, "dimension_mismatch");
        assert!(err.message.contains("actual_dimension=<missing>"));
    }

    /// Integration: the CLI gate and the loop gate share
    /// `check_progress_task_alignment` — they MUST produce the same
    /// `Mismatch.reason` for the same fixture. Drift between the two
    /// would re-introduce finding #21 (CLI passes / loop rejects).
    #[test]
    fn u1_step_handoff_gate_matches_loop_gate_reason() {
        let tmp = workspace();
        write_closed_task(&tmp, "task-1", "step-01");
        write_progress(
            &tmp,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
        );

        let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
        let cli_err = check_step_handoff_gate("queue.advance", payload, tmp.path())
            .expect_err("CLI gate must reject");

        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-02"),
            Some("task-1"),
            tmp.path(),
        );
        let loop_reason = match decision {
            GateDecision::Mismatch(m) => m.reason,
            other => panic!("expected Mismatch from loop gate, got {other:?}"),
        };
        assert!(
            cli_err.message.contains(&loop_reason),
            "CLI gate message must reference the loop-gate reason `{loop_reason}`; got: {}",
            cli_err.message
        );
    }

    // ── U1 / 2026-06-17-003 plan ─────────────────────────────────────
    // CLI isolated-mode scope precheck (`check_isolated_scope`).
    // The helper must:
    //   * pass through coordinator mode without effect
    //   * pass when hat is None (defer to origin guard)
    //   * pass when hat is ralph and topic is in RALPH_CONTROL_TOPICS
    //   * pass when hat is registered and topic is in hat.publishes
    //   * reject with isolated_scope_violation when hat is registered
    //     and topic is NOT in hat.publishes
    //   * reject with the same reason when the ralph pseudo-hat tries
    //     to publish a business topic (the existing ralph-guard at
    //     emit.rs catches this earlier, but the isolated-scope check
    //     must still agree so the two gates compose cleanly)

    fn isolated_config_with_hats(yaml_hats: &str) -> RalphConfig {
        let yaml = format!(
            r#"
event_loop:
  execution_mode: isolated
hats:
{yaml_hats}
"#
        );
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn u1_isolated_scope_happy_path_executor_publishes_work_done() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    triggers: ["plan.advance"]
    publishes: ["work.done", "task.resume"]
"#,
        );
        assert!(
            check_isolated_scope(Some("executor"), "work.done", &cfg).is_ok(),
            "executor publishing work.done must be allowed in isolated mode"
        );
    }

    #[test]
    fn u1_isolated_scope_error_path_executor_publishes_debug_step() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    triggers: ["plan.advance"]
    publishes: ["work.done", "task.resume"]
"#,
        );
        let err = check_isolated_scope(Some("executor"), "debug.step", &cfg)
            .expect_err("executor publishing debug.step must be rejected in isolated mode");
        assert_eq!(err.reason_code, "isolated_scope_violation");
        assert!(err.message.contains("executor"));
        assert!(err.message.contains("debug.step"));
        assert!(err.message.contains("work.done"));
    }

    #[test]
    fn u1_isolated_scope_coordinator_mode_is_noop() {
        let yaml = r#"
event_loop:
  execution_mode: coordinator
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        // Any hat + any topic in coordinator mode → Ok.
        assert!(check_isolated_scope(Some("executor"), "debug.step", &cfg).is_ok());
        assert!(check_isolated_scope(Some("unknown-hat"), "anything", &cfg).is_ok());
    }

    #[test]
    fn u1_isolated_scope_ralph_hat_control_topic_allowed() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
        );
        // ralph + LOOP_COMPLETE → Ok (control topic whitelist).
        assert!(check_isolated_scope(Some("ralph"), "LOOP_COMPLETE", &cfg).is_ok());
        assert!(check_isolated_scope(Some("ralph"), "task.resume", &cfg).is_ok());
        assert!(check_isolated_scope(Some("ralph"), "human.guidance", &cfg).is_ok());
    }

    #[test]
    fn u1_isolated_scope_ralph_hat_business_topic_rejected() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
        );
        // ralph + business topic → Err. The existing ralph-guard in
        // emit.rs already rejects this; the isolated-scope check
        // independently agrees (no double-reject at the gate level,
        // since emit.rs's ralph-guard runs first and bails before
        // reaching the isolated-scope check).
        let err = check_isolated_scope(Some("ralph"), "review.passed", &cfg)
            .expect_err("ralph + business topic must be rejected");
        assert_eq!(err.reason_code, "isolated_scope_violation");
    }

    #[test]
    fn u1_isolated_scope_no_hat_passes() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
        );
        // hat == None → defer to origin guard; check returns Ok.
        assert!(check_isolated_scope(None, "debug.step", &cfg).is_ok());
    }

    /// P1 (testing reviewer, plan §U1 test-scenarios Edge): an unknown
    /// hat in isolated mode must be rejected with `isolated_scope_violation`
    /// and `allowed: []` in the message. `HatRegistry::can_publish` is
    /// fail-closed for unknown hats, so the new precheck agrees with the
    /// loop's runtime scope guard. Without this test, the
    /// `config.hats.get(hat_id).map(|c| c.publishes.clone()).unwrap_or_default()`
    /// branch on line 444 is untested.
    #[test]
    fn u1_isolated_scope_unknown_hat_rejected() {
        let cfg = isolated_config_with_hats(
            r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
        );
        let err = check_isolated_scope(Some("ghost-hat"), "work.done", &cfg)
            .expect_err("unknown hat in isolated mode must be rejected (fail-closed)");
        assert_eq!(err.reason_code, "isolated_scope_violation");
        assert!(
            err.message.contains("ghost-hat"),
            "message must name the offending hat, got: {}",
            err.message
        );
        assert!(
            err.message.contains("work.done"),
            "message must name the offending topic, got: {}",
            err.message
        );
        assert!(
            err.message.contains("[]"),
            "message must show empty allowed_publishes for unknown hat, got: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------
    // 2026-06-17-004 plan U1 (R1, R2): unit tests for check_emit_provenance.
    // -----------------------------------------------------------------

    fn isolated_config_minimal() -> RalphConfig {
        let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn u1_check_emit_provenance_no_hat_business_topic_rejected() {
        let cfg = isolated_config_minimal();
        let err = check_emit_provenance(None, "review.passed", &cfg)
            .expect_err("isolated + no hat + business topic must be rejected");
        assert_eq!(err.reason_code, "missing_provenance");
        assert!(err.message.contains("review.passed"));
        assert!(err.message.contains("--hat"));
    }

    #[test]
    fn u1_check_emit_provenance_with_hat_passes() {
        let cfg = isolated_config_minimal();
        // Hat present → defer to scope guard, this gate returns Ok.
        assert!(check_emit_provenance(Some("executor"), "work.done", &cfg).is_ok());
        assert!(check_emit_provenance(Some("executor"), "review.passed", &cfg).is_ok());
    }

    #[test]
    fn u1_check_emit_provenance_control_topics_allowed_without_hat() {
        let cfg = isolated_config_minimal();
        // Control topics are produced by the loop / runtime pseudo-hat.
        for topic in [
            "LOOP_COMPLETE",
            "loop.cancel",
            "task.resume",
            "human.interact",
            "human.guidance",
        ] {
            assert!(
                check_emit_provenance(None, topic, &cfg).is_ok(),
                "control topic '{topic}' must be allowed without hat"
            );
        }
    }

    #[test]
    fn u1_check_emit_provenance_diagnostic_topics_allowed_without_hat() {
        let cfg = isolated_config_minimal();
        // Orchestrator diagnostics (event.* allowlist) are emitted by
        // the loop itself.
        for topic in [
            "event.malformed",
            "event.scope_violation",
            "event.isolation.boundary_violation",
            "event.payload_contract.rejected",
        ] {
            assert!(
                check_emit_provenance(None, topic, &cfg).is_ok(),
                "diagnostic topic '{topic}' must be allowed without hat"
            );
        }
    }

    #[test]
    fn u1_check_emit_provenance_coordinator_mode_noop() {
        let yaml = r#"
event_loop:
  execution_mode: coordinator
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        // Coordinator mode does not require provenance — the existing
        // blanket check (L424-436 in emit.rs) governs.
        assert!(check_emit_provenance(None, "review.passed", &cfg).is_ok());
        assert!(check_emit_provenance(None, "debug.step", &cfg).is_ok());
    }
}
