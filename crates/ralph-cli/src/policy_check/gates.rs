//! Gate plumbing for the CLI policy precheck (Plan 2026-08-07-002 U1).
//!
//! The `use` block below mirrors the parent `policy_check.rs` so the
//! module's items keep the same crate-internal types in scope. After
//! the module split (Plan 2026-08-07-002 U1) only a subset of these
//! imports is actually referenced here; the rest are listed under
//! `#[allow(unused_imports)]` because the original file was
//! monolithic and the public-API surface (Plan §7 U1 §4: "项级搬移、
//! 模块声明、精确导入、明确列出的最小可见性调整") requires us to
//! preserve each item's import neighborhood verbatim, even when an
//! individual submodule happens not to touch it.
#[allow(unused_imports)]
use crate::cli::{ConfigSource, load_config_with_overrides, resolve_workspace_root};
#[allow(unused_imports)]
use crate::config_resolution;
#[allow(unused_imports)]
use crate::operation_guard::OperationContext;
#[allow(unused_imports)]
use anyhow::{Context, Result};
#[allow(unused_imports)]
use ralph_core::config::HatExecutionMode;
#[allow(unused_imports)]
use ralph_core::config::{EventFieldDoc, EventSchema, PayloadType};
#[allow(deprecated, unused_imports)]
use ralph_core::step_handoff::progress_task_gate::{
    GateDecision, ProgressTaskMismatch, check_progress_task_alignment, is_gated_topic,
};
#[allow(unused_imports)]
use ralph_core::{
    EventLoopHandoffConfig, EventPolicyConfig, HatRegistry, PolicyDecision, PolicyRuntimeState,
    RalphConfig, ViolationType, validate_event, validate_event_with_options,
};
#[allow(unused_imports)]
use ralph_proto::HatId;
#[allow(unused_imports)]
use serde::Serialize;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

use super::unified::ValidationError;

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
    config_sources: &[ConfigSource],
) -> Result<Option<RalphConfig>> {
    let workspace_root = resolve_workspace_root(root);
    let resolved = if config_sources.is_empty() {
        config_resolution::resolve_project_config_path(&workspace_root, config_sources)
    } else {
        config_sources.iter().find_map(|source| match source {
            ConfigSource::File(path) if path.exists() => Some(path.clone()),
            _ => None,
        })
    };
    let config_path = match resolved {
        Some(path) => path,
        None => {
            return Ok(None);
        }
    };
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
/// running inside `ralph run -H builtin:ce-executor-pipeline` can call
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
    config_sources: &[ConfigSource],
) -> Result<Option<RalphConfig>> {
    use crate::cli::{ConfigSource, HatsSource};
    use crate::preflight::load_config_for_preflight_sync;
    let env_label = std::env::var("RALPH_HATS_SOURCE")
        .ok()
        .filter(|s| !s.is_empty());
    let workspace_root = resolve_workspace_root(root);

    let mut base = match load_workspace_config(root, on_error, config_sources)? {
        Some(cfg) => cfg,
        None => {
            // No `ralph.yml`. If `RALPH_HATS_SOURCE` is set, we
            // still want to honour the preset's
            // `event_policy.schemas` so the CLI precheck rejects
            // bad payloads — fall through to the historic
            // preset-merge path below. If neither exists, return
            // `None` (no policy enforcement possible).
            if env_label.is_none() && !workspace_root.join(".ralph/hats.yml").exists() {
                return Ok(None);
            }
            RalphConfig::default()
        }
    };

    // 2026-07-07-001 plan U5: when a workspace `.ralph/hats.yml`
    // exists alongside the loaded `ralph.yml`, deep-merge its
    // `hats:` map into `base.hats`. The previous behaviour only
    // merged `.ralph/hats.yml` when `RALPH_HATS_SOURCE` was set,
    // which meant CLI `--policy-check` saw a hat registry with
    // ONLY the builtin `ralph` hat (from `from_runtime_config`)
    // and rejected every user-defined hat (e.g. `coordinator`)
    // via `OriginRule::unknown_hat`. The CLI boundary must
    // honour the same hat map the runtime does, regardless of
    // whether the operator set `RALPH_HATS_SOURCE`.
    let hats_yaml_path = workspace_root.join(".ralph/hats.yml");
    if hats_yaml_path.exists() {
        merge_workspace_hats_into(&mut base, &hats_yaml_path)?;
    }

    // If `RALPH_HATS_SOURCE` is set, defer to the historic
    // preset-merge path so the operator's intent wins. The
    // `.ralph/hats.yml` merge above is a no-op then because
    // the env preset already supplies the full hats map.
    let Some(label) = env_label else {
        return Ok(Some(base));
    };

    let parsed = Some(HatsSource::parse(&label));
    // 2026-07-13-001 plan U4 + review #C4: reuse the SSOT-resolved
    // path that `load_workspace_config` already produced for `base`
    // (through the same `config_sources` slice), instead of
    // hardcoding `workspace_root.join("ralph.yml")`. This keeps
    // the `RALPH_HATS_SOURCE` preset-merge path consistent with
    // the base-config path so an agent that sets both
    // `RALPH_CONFIG=custom.yml` and `RALPH_HATS_SOURCE=builtin:...`
    // merges the preset on top of the operator's custom project
    // config, not a synthesised default.
    let config_path = base
        .config_path
        .clone()
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

/// 2026-07-07-001 plan U5: when the workspace has no `ralph.yml`
/// but does have `.ralph/hats.yml`, surface a minimal
/// `RalphConfig` carrying just the hats map so the CLI
/// `OriginRule` accepts user-defined hats. `RALPH_HATS_SOURCE`
/// still wins when set (the historic preset-merge path).
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 emit-path parity
// (hats-only discovery as a fallback for the user-scoped hats
// flow).
#[allow(dead_code)]
pub(crate) fn load_policy_config_from_hats_only(
    root: Option<&PathBuf>,
    env_label: Option<&str>,
) -> Result<Option<RalphConfig>> {
    if env_label.is_some() {
        return Ok(None);
    }
    let workspace_root = resolve_workspace_root(root);
    let hats_yaml_path = workspace_root.join(".ralph/hats.yml");
    if !hats_yaml_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&hats_yaml_path)
        .with_context(|| format!("Failed to load hats from {:?}", hats_yaml_path))?;
    let value: serde_yaml::Value =
        config_resolution::parse_yaml_value(&content, &hats_yaml_path.display().to_string())?;
    let hats_map = match value.get("hats") {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        Some(other) => {
            anyhow::bail!(
                "`.ralph/hats.yml` top-level `hats` must be a mapping, found {:?}",
                other
            );
        }
        None => {
            return Ok(None);
        }
    };
    let mut config = RalphConfig::default();
    let mut new_hats = std::collections::HashMap::new();
    for (k, v) in hats_map {
        let key_str = match k {
            serde_yaml::Value::String(s) => s,
            other => {
                anyhow::bail!(
                    "`.ralph/hats.yml` hat id must be a string, found {:?}",
                    other
                );
            }
        };
        let hat_config: ralph_core::config::HatConfig = serde_yaml::from_value(v)
            .with_context(|| format!("Failed to parse hat config for `{key_str}`"))?;
        new_hats.insert(key_str, hat_config);
    }
    config.hats = new_hats;
    Ok(Some(config))
}

/// 2026-07-07-001 plan U5: deep-merge the `hats:` mapping from a
/// `.ralph/hats.yml` file into an existing `RalphConfig`'s `hats`
/// map. Existing entries (from `ralph.yml`) win on key collision
/// because the workspace `ralph.yml` is the operator's explicit
/// declaration; `.ralph/hats.yml` only fills gaps.
pub(crate) fn merge_workspace_hats_into(
    config: &mut RalphConfig,
    hats_yaml_path: &std::path::Path,
) -> Result<()> {
    let content = std::fs::read_to_string(hats_yaml_path)
        .with_context(|| format!("Failed to load hats from {:?}", hats_yaml_path))?;
    let value: serde_yaml::Value =
        config_resolution::parse_yaml_value(&content, &hats_yaml_path.display().to_string())?;
    let hats_map = match value.get("hats") {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        Some(other) => {
            anyhow::bail!(
                "`.ralph/hats.yml` top-level `hats` must be a mapping, found {:?}",
                other
            );
        }
        None => return Ok(()),
    };
    for (k, v) in hats_map {
        let key_str = match k {
            serde_yaml::Value::String(s) => s,
            other => {
                anyhow::bail!(
                    "`.ralph/hats.yml` hat id must be a string, found {:?}",
                    other
                );
            }
        };
        let hat_config: ralph_core::config::HatConfig = serde_yaml::from_value(v)
            .with_context(|| format!("Failed to parse hat config for `{key_str}`"))?;
        config.hats.entry(key_str).or_insert(hat_config);
    }
    Ok(())
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
#[allow(deprecated, clippy::result_large_err)]
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
                ..Default::default()
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
                ..Default::default()
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

pub(crate) fn mismatch_to_validation_error(
    m: &ProgressTaskMismatch,
    topic: &str,
) -> ValidationError {
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
        ..Default::default()
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
#[allow(clippy::result_large_err)]
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
#[allow(clippy::result_large_err)]
pub(crate) fn check_wave_dimension_assignment_with_env(
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
        ..Default::default()
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
#[allow(clippy::result_large_err)]
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
    if hat_id == "ralph" && ralph_core::event_origin::RALPH_CONTROL_TOPICS.contains(&topic) {
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
        ..Default::default()
    })
}

// ─────────────────────────────────────────────────────────────────────────
// U6 (2026-06-21-002 plan §U6): unified policy check path.
//
// `run_policy_check_unified` runs the U4 `ValidationPipeline` over an
// event (the same pipeline the loop uses via `process_parse_result`)
// and produces a `PolicyCheckReport` with structured `reason_codes`.
// The CLI `--policy-check` always routes through this path; the
// legacy `validate_event_with_hat` path is preserved only for diff /
// no-policy fallback runs.
//
// Why a separate `PolicyCheckReport` instead of the existing
// `ValidationFailure`? `ValidationFailure` is a single-failure shape
// tailored to the legacy batch validator; the unified pipeline
// reports one `ValidationResult` per rule (pre + post commit), so the
// new report carries a *list* of `reason_codes` plus a parallel
// `suggestions` list (one human-readable hint per rejection).
// Agents parsing `--output json` get a single document they can diff
// against the legacy shape.
// ─────────────────────────────────────────────────────────────────────────

/// Structured report returned by [`run_policy_check_unified`].
///
/// `accepted == true` means every U4 rule (pre + post commit) accepted
/// the event; `reason_codes` is empty. On rejection, `reason_codes` lists
/// one string per failed rule, in pipeline order, and `suggestions`
/// carries the matching `correction_hint` values (or empty strings when
/// a rule rejected without a hint). The list shape matches the U4
/// `ValidationReport::pre_commit + post_commit` ordering so callers can
/// correlate 1:1.
/// reading. Returns `<missing>` for any non-JSON payload, missing
/// field, or non-string value. Trims the value (P1#6 fix) so the
/// CLI precheck matches the merge layer's
/// `parse_payload_dimension` behavior — otherwise a payload with
/// `"dimension": "testing "` would be rejected by the precheck
/// but accepted by the merge layer, producing a confusing
/// "agent got rejected but the merge would have accepted" loop.
pub fn extract_dimension_field(payload_str: &str) -> String {
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

/// `policy_check` so the CLI does not pull a `&EventBus` boundary
/// just to look at two payload fields.
pub fn extract_step_and_task_id_from_payload(
    payload_str: &str,
) -> (Option<String>, Option<String>) {
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
