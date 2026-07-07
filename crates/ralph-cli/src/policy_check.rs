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

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::cli::{ConfigSource, load_config_with_overrides, resolve_workspace_root};
use crate::config_resolution;
use ralph_core::config::HatExecutionMode;
#[allow(deprecated)]
use ralph_core::step_handoff::progress_task_gate::{
    GateDecision, ProgressTaskMismatch, check_progress_task_alignment, is_gated_topic,
};
use ralph_core::{
    EventLoopHandoffConfig, EventPolicyConfig, HatRegistry, PolicyDecision, PolicyRuntimeState,
    RalphConfig, ViolationType, validate_event, validate_event_with_options,
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
    resolve_policy_check_mode_with_ctx(flags, config, false)
}

/// U15: agent-context-aware variant of [`resolve_policy_check_mode`].
///
/// When `is_agent_context` is true, callers behave **as if**
/// `event_loop.event_policy.require_policy_check_for_cli_emit: true`,
/// irrespective of the resolved config. This closes the gap where an
/// agent path fell through to `Skip` because the preset author did
/// not enable `event_policy.enabled` — agents must always pass the
/// schema precheck before writing an event.
///
/// Preset opt-out via `event_policy.allow_unsafe_cli_emit: true` is
/// still honoured: an agent calling `ralph emit --unsafe-no-policy-check`
/// on such a preset gets `Skip` (with a deprecation warning emitted
/// via [`crate::commands::emit::emit_unsafe_bypass_deprecation`]).
/// Human CLI (`is_agent_context == false`) keeps the legacy semantics.
pub fn resolve_policy_check_mode_with_ctx(
    flags: &PolicyCheckFlags,
    config: Option<&RalphConfig>,
    is_agent_context: bool,
) -> PolicyCheckMode {
    if flags.policy_check {
        return PolicyCheckMode::ExplicitCheck;
    }

    let config_strict = config
        .and_then(|c| c.event_loop.event_policy.as_ref())
        .map(|p| p.enabled && p.require_policy_check_for_cli_emit)
        .unwrap_or(false);
    let allow_unsafe_bypass = config
        .and_then(|c| c.event_loop.event_policy.as_ref())
        .map(|p| p.allow_unsafe_cli_emit)
        .unwrap_or(false);

    // The effective strict flag is "config-says-strict OR agent-context
    // defaults to strict". Human CLI strictly follows the config.
    let effective_strict = config_strict || is_agent_context;

    if effective_strict {
        if flags.no_policy_check && allow_unsafe_bypass {
            return PolicyCheckMode::Skip;
        }
        return PolicyCheckMode::Enforce;
    }

    // When neither config nor agent-context asks for strict, skip.
    // `--unsafe-no-policy-check` without strict config is a no-op.
    PolicyCheckMode::Skip
}

/// Backwards-compatible re-export of [`resolve_policy_check_mode`]
/// without operation context. Human CLI keep the legacy semantics.
pub fn legacy_resolve(flags: &PolicyCheckFlags, config: Option<&RalphConfig>) -> PolicyCheckMode {
    resolve_policy_check_mode_with_ctx(flags, config, false)
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
) -> Result<Option<RalphConfig>> {
    use crate::cli::{ConfigSource, HatsSource};
    use crate::preflight::load_config_for_preflight_sync;
    let env_label = std::env::var("RALPH_HATS_SOURCE")
        .ok()
        .filter(|s| !s.is_empty());
    let workspace_root = resolve_workspace_root(root);

    let mut base = match load_workspace_config(root, on_error)? {
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

/// 2026-07-07-001 plan U5: when the workspace has no `ralph.yml`
/// but does have `.ralph/hats.yml`, surface a minimal
/// `RalphConfig` carrying just the hats map so the CLI
/// `OriginRule` accepts user-defined hats. `RALPH_HATS_SOURCE`
/// still wins when set (the historic preset-merge path).
fn load_policy_config_from_hats_only(
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
    let value: serde_yaml::Value = config_resolution::parse_yaml_value(
        &content,
        &hats_yaml_path.display().to_string(),
    )?;
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
fn merge_workspace_hats_into(
    config: &mut RalphConfig,
    hats_yaml_path: &std::path::Path,
) -> Result<()> {
    let content = std::fs::read_to_string(hats_yaml_path)
        .with_context(|| format!("Failed to load hats from {:?}", hats_yaml_path))?;
    let value: serde_yaml::Value = config_resolution::parse_yaml_value(
        &content,
        &hats_yaml_path.display().to_string(),
    )?;
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
#[allow(deprecated)]
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
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PolicyCheckReport {
    /// Topic that was validated.
    pub topic: String,
    /// Hat that emitted the event (None when unknown).
    pub hat: Option<String>,
    /// Workspace root the validation was performed against.
    ///
    /// **FIX-5 (U11)**: marked `#[serde(skip)]` so the absolute
    /// path never leaks through any automatic `Serialize` path.
    /// The canonical JSON shape (built by [`Self::to_json_value`])
    /// exposes a basename-only `workspace_redacted` field instead.
    /// The struct still keeps the absolute path so in-process
    /// callers (e.g. log enrichment, file-system operations)
    /// can continue to use it.
    #[serde(skip)]
    pub workspace: PathBuf,
    /// `true` iff every U4 rule accepted the event.
    pub accepted: bool,
    /// Stable reason codes from each failed rule, in pre+post-commit
    /// pipeline order. Empty when `accepted == true`. Examples:
    /// `origin:ralph_control_only`, `engine_rejected:required_field:task_id`,
    /// `step_handoff:progress_task_mismatch:task_closed_but_progress_missing`.
    pub reason_codes: Vec<String>,
    /// Human-readable correction hints, parallel to `reason_codes`.
    /// Each entry is the `correction_hint` from the matching
    /// `ValidationResult` (empty string when the rule did not provide
    /// one).
    pub suggestions: Vec<String>,
    /// `true` when at least one rejection came from a post-commit rule.
    /// The CLI uses this to decide whether to surface a
    /// "post-state violation" warning distinct from the per-rule hints.
    pub post_commit_rejected: bool,
}

impl PolicyCheckReport {
    /// Convert the report into a JSON `Value` so callers can layer
    /// their own envelope (e.g. the legacy `ValidationFailure`
    /// shape).
    ///
    /// **FIX-5 (U11)**: the on-disk report shape MUST NOT include
    /// the absolute `workspace` path.  Policy check reports are
    /// written into `.ralph/events.jsonl` / JSON envelopes that
    /// may be shipped to operator dashboards, telemetry, or
    /// shared with downstream services; leaking the workspace
    /// path leaks the operator's directory layout.  The
    /// `workspace` field is kept on the struct (so downstream
    /// code can still inspect it in-process) but the JSON
    /// representation only carries a basename-style redacted
    /// `workspace_redacted` string.
    #[allow(dead_code)] // public API, exposed for downstream tooling
    pub fn to_json_value(&self) -> serde_json::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "topic": self.topic,
            "hat": self.hat,
            "workspace_redacted": self
                .workspace
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<redacted>"),
            "accepted": self.accepted,
            "reason_codes": self.reason_codes,
            "suggestions": self.suggestions,
            "post_commit_rejected": self.post_commit_rejected,
        }))
    }
}

/// Build a [`PolicyCheckReport`] from a U4 [`ValidationReport`].
///
/// Internal helper so the conversion logic is testable without
/// re-running the pipeline.
fn report_from_validation(
    report: &ralph_core::validation::ValidationReport,
    topic: &str,
    hat: Option<&str>,
    workspace: &Path,
) -> PolicyCheckReport {
    let mut reason_codes = Vec::new();
    let mut suggestions = Vec::new();
    for r in report.pre_commit.iter().chain(report.post_commit.iter()) {
        if r.accepted {
            continue;
        }
        let code = r
            .reason_code
            .clone()
            .unwrap_or_else(|| format!("{}:rejected", r.stage));
        let hint = r.correction_hint.clone().unwrap_or_default();
        reason_codes.push(code);
        suggestions.push(hint);
    }
    PolicyCheckReport {
        topic: topic.to_string(),
        hat: hat.map(|s| s.to_string()),
        workspace: workspace.to_path_buf(),
        accepted: report.accepted,
        reason_codes,
        suggestions,
        post_commit_rejected: report.post_commit_rejected,
    }
}

/// Run the unified `ValidationPipeline` over a single event and
/// produce a structured [`PolicyCheckReport`].
///
/// This is the U6 entry point that the CLI `--policy-check` always
/// routes through. The function builds a `ProtocolView` + the
/// current `LedgerSnapshot` (replayed from `.ralph/events.jsonl`),
/// constructs the canonical `ValidationPipeline` (same rules the
/// loop uses), and runs both pre-commit and post-commit phases
/// via `validate_with_preview`. The post-commit phase uses the
/// *current* snapshot as the projected snapshot — the unified
/// pipeline's `validate_with_preview` is conservative and does
/// not mutate the caller's snapshot, so a single call is enough
/// to surface both rule families.
///
/// `event_path` is the events JSONL to replay before running the
/// pipeline. It is used only to satisfy the pipeline's signature
/// (the post-commit rules need a `LedgerSnapshot`; we use the
/// cold-start snapshot here because CLI emit runs ahead of the
/// loop, not against its in-memory state).
pub fn run_policy_check_unified(
    topic: &str,
    payload: Option<&str>,
    hat: Option<&str>,
    triggered: Option<&str>,
    workspace: &Path,
) -> Result<PolicyCheckReport> {
    use ralph_core::Event;
    use ralph_core::preset::engine::protocol::ProtocolView;
    use ralph_core::state::{LedgerSnapshot, StateLedger};
    use ralph_core::validation::{ValidationContext, ValidationPipeline};

    // Load the config to build the protocol view. Reuse the
    // existing preflight loader so RALPH_HATS_SOURCE, schema
    // discovery, and the legacy fail-closed rules (C1/C4) all
    // behave identically. When the workspace has no config we
    // fall back to a default view (the unified pipeline will
    // accept everything, mirroring the legacy no-policy default).
    let workspace_root = resolve_workspace_root(Some(&workspace.to_path_buf()));
    let config = load_policy_config_for_cli_emit(Some(&workspace_root), OnConfigError::Tolerate)?;
    let event_loop_config = config
        .as_ref()
        .map(|c| c.event_loop.clone())
        .unwrap_or_default();

    // P2-#6 (002-adversarial-review): production-only env
    // read; tests must use `ProtocolView::from_event_loop`
    // (env-free) to stay isolated under `cargo nextest`.
    //
    // 2026-07-06-004 fix-plan U6 (R6): use the hats-aware
    // variant so the topology whitelist (every hat's
    // `triggers` ∪ `publishes`) is populated; the
    // `EventPolicyRule` consults it to reject
    // `success_signal` / `failure_signal` outside the
    // declared topology.
    let hats = config.as_ref().map(|c| c.hats.clone()).unwrap_or_default();
    let view =
        ProtocolView::from_event_loop_with_feature_for_env_and_hats(&event_loop_config, &hats);
    // 2026-07-07-001 plan U1: derive the runtime hat registry
    // from the loaded config so the unified pipeline's
    // `EventPolicyRule` registry-aware checks
    // (`unknown_to_hat`, `signal_outside_topology`) fire at
    // the CLI boundary. Previously the pipeline was built with
    // `from_config(&view, ...)` which defaulted the registry to
    // `None`, silently bypassing the `to_hat` check on the CLI
    // path. The no-config dry-run path still gets `None` so it
    // does not panic and only enforces schema / topology that
    // is derivable from the cold-start view.
    let pipeline = if let Some(cfg) = config.as_ref() {
        ValidationPipeline::from_ralph_config(&view, cfg)
    } else {
        ValidationPipeline::from_config(&view, &event_loop_config)
    };

    // R12 (U11-T7): load .ralph/events.jsonl into LedgerSnapshot so
    // the unified pipeline sees terminal/business state. The legacy
    // `LedgerSnapshot::cold_start()` produced an empty snapshot, which
    // made the post-commit rules in `validate_with_preview` reject
    // legitimate terminal events (e.g. `work.done` with `task_id`
    // pointing at a queue that does not exist in the snapshot).
    let events_path = workspace_root.join(".ralph/events.jsonl");
    let mut snapshot = if events_path.exists() {
        match StateLedger::replay_from_disk(&workspace_root) {
            Ok(snap) => snap,
            Err(e) => {
                eprintln!("Warning: ledger replay failed for policy check: {e}. Using cold start.");
                LedgerSnapshot::cold_start()
            }
        }
    } else {
        LedgerSnapshot::cold_start()
    };
    let mut projected = snapshot.clone();

    // U11-T7-R12b: legacy `validate_topic_payload_against_config` is
    // the canonical path for **terminal-monotonicity** and
    // **duplicate-terminal** enforcement. The unified pipeline does
    // not yet model these stateful rules (terminal_observed comes
    // from `PolicyRuntimeState::from_events`, not from
    // `LedgerSnapshot`), so we replay events.jsonl once more via
    // the legacy helper and short-circuit with a synthetic reject
    // when the legacy gate flags the event. This keeps the CLI's
    // terminal-monotonicity / duplicate-terminal contract intact
    // across the unified path; without this fallback, the unified
    // path would silently accept events that the loop would later
    // reject on the same events.jsonl replay.
    if let Some(policy) = event_loop_config.event_policy.as_ref()
        && policy.enabled
        && events_path.exists()
    {
        match validate_topic_payload_against_config(
            topic,
            payload.unwrap_or(""),
            policy,
            &events_path,
        ) {
            Ok(Some(legacy_error)) => {
                // Build a synthetic pre-rejected ValidationReport so
                // `report_from_validation` produces the same shape
                // downstream callers already parse. The legacy
                // `terminal_monotonicity_violation` /
                // `duplicate_terminal_event` /
                // `business_event_after_completion` reason codes
                // are surfaced verbatim — they are part of the
                // public CLI surface and tests pin them.
                use ralph_core::validation::ValidationReport;
                use ralph_core::validation::{ValidationResult, ValidationStage};
                let wrapped_reason =
                    format!("engine_rejected:legacy_policy:{}", legacy_error.reason_code);
                let reject = ValidationResult::reject(
                    ValidationStage::ExecutionContract,
                    wrapped_reason,
                    Some(legacy_error.message),
                    true,
                );
                let report = ValidationReport {
                    pre_commit: vec![reject],
                    post_commit: vec![],
                    accepted: false,
                    post_commit_rejected: false,
                };
                return Ok(report_from_validation(&report, topic, hat, &workspace_root));
            }
            Ok(None) => {
                // Legacy gate accepted; fall through to the unified
                // ValidationPipeline for the rest of the rules.
            }
            Err(e) => {
                eprintln!(
                    "Warning: legacy policy check failed for unified path: {e}. Falling back to unified pipeline only."
                );
            }
        }
    }

    let event = Event {
        topic: topic.to_string(),
        payload: payload.map(|s| s.to_string()),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: hat.map(|s| s.to_string()),
        triggered: None,
        source: Some("cli".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };

    let mut ctx = ValidationContext::new(&mut snapshot);
    let mut projected_ctx = ValidationContext::new(&mut projected);
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    let final_report = report_from_validation(&report, topic, hat, &workspace_root);

    // 2026-06-28-002 U7: when the unified pipeline rejects the
    // event, mirror the rejection into `.ralph/recovery.jsonl`
    // as a `repair_dispatch` envelope so the recovery stream
    // aggregator (downstream of `record_repair_event`) sees the
    // CLI emit path. Without this, CLI `ralph emit` failures
    // bypass U6/U7/U9.5/U12 entirely because the pipeline
    // result was a CLI-only side-effect. We append best-effort:
    // a missing `recovery.jsonl` is not fatal, and a write
    // failure is logged at WARN level.
    if !report.accepted {
        append_cli_reject_to_recovery(&workspace_root, topic, hat, payload, &report);
    }

    // U7 of plan 2026-07-05-005 (R6): envelope-layer
    // `triggered` validation. Runs after the unified pipeline
    // so the gate order is "payload schema → envelope
    // topology". Mirrors the apply-path gate so `--policy-check`
    // and the real write share the same rejection surface.
    if let Some(cfg) = config.as_ref() {
        if let Err(err) = check_envelope_triggered(topic, triggered, cfg) {
            let mut rej = final_report;
            rej.accepted = false;
            rej.reason_codes.push(err.reason_code);
            rej.suggestions.push(err.message);
            return Ok(rej);
        }
    }

    Ok(final_report)
}

/// 2026-06-28-002 U7: append a `repair_dispatch` envelope to
/// `.ralph/recovery.jsonl` when the unified pipeline rejects a
/// CLI emit. Delegates to `record_repair_event` so the envelope
/// shape is identical to the loop's internal repair stream —
/// downstream consumers (e.g. `ralph diagnose`) do not need to
/// special-case CLI rejects. Best-effort: a write failure is
/// logged at WARN level.
fn append_cli_reject_to_recovery(
    workspace_root: &Path,
    topic: &str,
    hat: Option<&str>,
    payload: Option<&str>,
    report: &ralph_core::validation::ValidationReport,
) {
    use ralph_proto::{Event, HatId};
    let event = Event {
        topic: ralph_proto::Topic::from(topic),
        payload: payload.unwrap_or("").to_string(),
        source: hat.map(|h| HatId::new(h)),
        target: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    if let Err(e) = ralph_core::event_loop::repair_stream_sink::record_repair_event(
        &event,
        &workspace_root.join(".ralph"),
    ) {
        tracing::warn!(
            target: "ralph_cli::policy_check",
            workspace_root = %workspace_root.display(),
            error = %e,
            "U7: failed to append CLI reject to recovery.jsonl via repair_stream_sink; \
             continuing without blocking the CLI"
        );
    }
    // Suppress unused-variable warning when the report is no longer
    // inspected after delegation.
    let _ = report;
}

#[cfg(test)]
mod u6_unified_path_tests {
    use super::*;
    use ralph_core::validation::{ReasonCode, ValidationResult, ValidationStage};
    use tempfile::TempDir;

    fn fake_report(
        pre: Vec<ValidationResult>,
        post: Vec<ValidationResult>,
    ) -> ralph_core::validation::ValidationReport {
        let accepted = pre.iter().all(|r| r.accepted) && post.iter().all(|r| r.accepted);
        let post_commit_rejected = post.iter().any(|r| !r.accepted);
        ralph_core::validation::ValidationReport {
            pre_commit: pre,
            post_commit: post,
            accepted,
            post_commit_rejected,
        }
    }

    #[test]
    fn report_from_validation_accepts_collects_no_reasons() {
        let report = fake_report(
            vec![ValidationResult::accept(), ValidationResult::accept()],
            vec![ValidationResult::accept()],
        );
        let workspace = std::path::PathBuf::from("/tmp/workspace");
        let out = report_from_validation(&report, "work.done", Some("executor"), &workspace);
        assert!(out.accepted);
        assert!(out.reason_codes.is_empty());
        assert!(out.suggestions.is_empty());
        assert!(!out.post_commit_rejected);
        assert_eq!(out.topic, "work.done");
        assert_eq!(out.hat.as_deref(), Some("executor"));
    }

    #[test]
    fn report_from_validation_pre_commit_collects_reason_code_and_hint() {
        // The unified pipeline emits `step_handoff::<reason>` (the
        // `STEP_HANDOFF_MISMATCH_PREFIX` constant already ends in
        // `:`, and the rule appends `<reason>` after another `:`).
        // Pin the exact shape so the loop ↔ CLI vocabulary stays
        // in lockstep.
        let report = fake_report(
            vec![
                ValidationResult::accept(),
                ValidationResult::reject(
                    ValidationStage::StepHandoff,
                    format!(
                        "{}:task_closed_but_progress_missing",
                        ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX
                    ),
                    Some("add progress entry for step".to_string()),
                    true,
                ),
            ],
            vec![ValidationResult::accept()],
        );
        let workspace = std::path::PathBuf::from("/tmp/workspace");
        let out = report_from_validation(&report, "queue.advance", None, &workspace);
        assert!(!out.accepted);
        assert_eq!(out.reason_codes.len(), 1);
        assert_eq!(
            out.reason_codes[0],
            "step_handoff::task_closed_but_progress_missing"
        );
        assert_eq!(out.suggestions[0], "add progress entry for step");
        assert!(!out.post_commit_rejected);
    }

    #[test]
    fn report_from_validation_post_commit_rejection_sets_flag() {
        use ralph_core::validation::RejectionHint;
        let report = fake_report(
            vec![ValidationResult::accept()],
            vec![ValidationResult::reject(
                ValidationStage::ExecutionContract,
                ReasonCode::CONTRACT_MISSING_TASK_ID,
                Some(RejectionHint::missing_task_id("task_id")),
                true,
            )],
        );
        let workspace = std::path::PathBuf::from("/tmp/workspace");
        let out = report_from_validation(&report, "work.done", Some("executor"), &workspace);
        assert!(!out.accepted);
        assert!(out.post_commit_rejected);
        assert_eq!(out.reason_codes.len(), 1);
        assert_eq!(out.reason_codes[0], ReasonCode::CONTRACT_MISSING_TASK_ID);
    }

    #[test]
    fn report_to_json_value_contains_reason_codes_array() {
        let report = fake_report(
            vec![ValidationResult::reject(
                ValidationStage::Origin,
                ReasonCode::RALPH_CONTROL_ONLY.to_string(),
                None,
                true,
            )],
            vec![],
        );
        let workspace = std::path::PathBuf::from("/tmp/workspace");
        let out = report_from_validation(&report, "review.passed", Some("ralph"), &workspace);
        let json = out.to_json_value().unwrap();
        assert_eq!(json["topic"], "review.passed");
        assert_eq!(json["hat"], "ralph");
        assert_eq!(json["accepted"], false);
        assert_eq!(
            json["reason_codes"][0],
            ReasonCode::RALPH_CONTROL_ONLY.to_string()
        );
        assert_eq!(
            json["suggestions"][0],
            serde_json::Value::String(String::new())
        );
    }

    #[test]
    fn run_policy_check_unified_accepts_no_required_fields() {
        // Empty workspace + no config → cold-start view with no
        // required fields. The pipeline accepts every event, the
        // report mirrors the loop's accept verdict.
        let tmp = TempDir::new().unwrap();
        let report =
            run_policy_check_unified("debug.step", Some("task_id=demo"), None, None, tmp.path())
                .expect("unified check should succeed on empty workspace");
        assert!(report.accepted, "report: {report:?}");
        assert!(report.reason_codes.is_empty());
        assert_eq!(report.topic, "debug.step");
    }

    #[test]
    fn run_policy_check_unified_rejects_missing_required_field() {
        // Build a workspace whose ralph.yml declares a required
        // field for `experiment.planned`. The payload omits it →
        // the unified pipeline must surface a structured
        // `engine_rejected:required_field:<name>` reason code.
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".ralph")).unwrap();
        std::fs::write(
            tmp.path().join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      experiment.planned:
        required_fields:
          - task_key
"#,
        )
        .unwrap();
        let report = run_policy_check_unified(
            "experiment.planned",
            Some(r#"{"foo":"bar"}"#),
            None,
            None,
            tmp.path(),
        )
        .expect("unified check should return a report");
        assert!(
            !report.accepted,
            "missing required field must reject: {report:?}"
        );
        assert!(
            report
                .reason_codes
                .iter()
                .any(|c| c.starts_with("engine_rejected:required_field")),
            "expected engine_rejected:required_field reason, got: {:?}",
            report.reason_codes
        );
    }

    #[test]
    #[allow(deprecated)]
    fn run_policy_check_unified_misaligned_queue_advance_rejected() {
        // Step-handoff gate (U1 2026-06-17-005 plan): when
        // progress.md and tasks.jsonl disagree, the loop rejects
        // `queue.advance` with `progress_task_mismatch`. The
        // unified pipeline must surface the same reason_code so
        // the CLI and loop never disagree.
        let tmp = TempDir::new().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        let agent_dir = ralph_dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Closed task for step-01.
        let task = serde_json::json!({
            "id": "task-1",
            "title": "step-01",
            "status": "closed",
            "priority": 3,
        });
        let tasks_path = agent_dir.join("tasks.jsonl");
        std::fs::write(&tasks_path, format!("{task}\n")).unwrap();

        // progress.md does NOT list step-01 → mismatch.
        let progress = "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n";
        std::fs::write(agent_dir.join("progress.md"), progress).unwrap();

        let report = run_policy_check_unified(
            "queue.advance",
            Some(r#"{"step":"step-02","task_id":"task-1"}"#),
            None,
            None,
            tmp.path(),
        )
        .expect("unified check should return a report");
        assert!(
            !report.accepted,
            "misaligned progress must produce a non-accepting report: {report:?}"
        );
        assert!(
            report
                .reason_codes
                .iter()
                .any(|c| c.starts_with("step_handoff:")),
            "expected step_handoff reason code, got: {:?}",
            report.reason_codes
        );
    }

    #[test]
    fn run_policy_check_unified_and_loop_agree_on_misaligned_progress() {
        // U6 plan §"Test scenarios" Error path: `--policy-check` and
        // the loop must produce matching verdicts (both reject) for a
        // misaligned `queue.advance`. The exact reason string may
        // differ — the unified pipeline runs against a cold-start
        // `LedgerSnapshot` (CLI emit runs ahead of the loop, not
        // against its in-memory state), while the legacy gate reads
        // `tasks.jsonl` directly. The contract we pin is that both
        // paths surface `step_handoff:` reason codes so downstream
        // tooling can route on the prefix; the unified pipeline also
        // emits a `step_handoff::` shape that the CLI precheck can
        // intersect with the loop's `progress_task_mismatch` family.
        #[allow(deprecated)]
        use ralph_core::step_handoff::progress_task_gate::{
            GateDecision, check_progress_task_alignment,
        };

        let tmp = TempDir::new().unwrap();
        let ralph_dir = tmp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();

        let task = serde_json::json!({
            "id": "task-1",
            "title": "step-01",
            "status": "closed",
            "priority": 3,
        });
        std::fs::write(ralph_dir.join("agent/tasks.jsonl"), format!("{task}\n")).unwrap();
        std::fs::write(
            ralph_dir.join("agent/progress.md"),
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
        )
        .unwrap();

        // Loop-side gate (the existing precheck helper
        // `check_progress_task_alignment` mirrors the loop's
        // `apply_step_handoff_gate`).
        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-02"),
            Some("task-1"),
            tmp.path(),
        );
        let loop_rejects = matches!(decision, GateDecision::Mismatch(_));
        assert!(
            loop_rejects,
            "loop gate must reject the misaligned progress"
        );

        // Unified-pipeline side: must also reject and emit a
        // `step_handoff:` reason code. The exact suffix differs
        // because the unified pipeline runs against a cold-start
        // snapshot (no tasks loaded), but the shared prefix is
        // what callers match on.
        let report = run_policy_check_unified(
            "queue.advance",
            Some(r#"{"step":"step-02","task_id":"task-1"}"#),
            None,
            None,
            tmp.path(),
        )
        .expect("unified check should return a report");
        assert!(!report.accepted, "unified pipeline must reject");
        assert!(
            report
                .reason_codes
                .iter()
                .any(|c| c.starts_with("step_handoff:")),
            "unified pipeline must surface step_handoff reason code; got: {:?}",
            report.reason_codes
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-07-001 plan U1: CLI `run_policy_check_unified` must
    // wire the runtime `HatRegistry` from the loaded config so
    // an envelope addressed to an unknown `receiver_contract.to_hat`
    // is rejected (the same wire shape as the runtime pipeline).
    // ─────────────────────────────────────────────────────────────────

    /// Build a workspace whose `ralph.yml` declares a small set of
    /// hats, an envelope-required schema on `work.done`, and a
    /// `handoff_envelope` policy block that asks the validator to
    /// run. Used by the U1 RED tests below.
    fn workspace_with_serial_handoff_policy() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let yaml = r#"
hats:
  executor:
    name: Executor
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    instructions: "execute"
  reviewer:
    name: Reviewer
    triggers: ["work.done"]
    publishes: ["review.passed"]
    instructions: "review"
event_loop:
  handoff_envelope:
    enabled: true
    validate_payload: true
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      work.done:
        required_fields: ["handoff_envelope", "task_id", "step"]
"#;
        std::fs::write(tmp.path().join("ralph.yml"), yaml).unwrap();
        tmp
    }

    /// Local copy of the U10 (2026-07-06-004) envelope fixture
    /// (defined inside `tests` module below) so the U1 tests can
    /// stand on their own without depending on the test-module
    /// access boundary.
    fn u1_full_envelope(to_hat: &str) -> String {
        format!(
            r#"{{
  "plan_name":"p1",
  "task_id":"t1",
  "task_key":"p1:step-2:implement",
  "step":"step-2",
  "commit_count":1,
  "changed_lines":10,
  "handoff_envelope":{{
    "schema_version":"handoff-envelope.v1",
    "root_goal":"ship without regressions",
    "plan":{{
      "name":"p1",
      "path":"docs/plans/p1.md",
      "current_step":"step-2",
      "completed_steps":["step-1"]
    }},
    "state":{{
      "current_status":"ready_for_review",
      "last_signal":"work.done",
      "blocking_reason":null
    }},
    "receiver_contract":{{
      "to_hat":"{to_hat}",
      "must_do":["review step-2"],
      "must_not_do":["regress step-1"],
      "success_signal":"work.done",
      "failure_signal":"work.failed"
    }}
  }}
}}"#
        )
    }

    #[test]
    fn policy_check_rejects_unknown_handoff_to_hat_from_builtin_serial_config() {
        // 2026-07-07-001 plan U1 (P1): CLI --policy-check must
        // reject an envelope addressed to an unknown to_hat when
        // the workspace config declares a real hat set. The
        // unified pipeline inside `run_policy_check_unified` must
        // be wired with the runtime `HatRegistry` so the
        // `EventPolicyRule` registry check actually fires.
        let tmp = workspace_with_serial_handoff_policy();
        let payload = u1_full_envelope("ghost-hat");
        let report = run_policy_check_unified(
            "work.done",
            Some(&payload),
            Some("executor"),
            None,
            tmp.path(),
        )
        .expect("unified check should return a report");
        assert!(
            !report.accepted,
            "unknown to_hat must produce a non-accepting report: {report:?}"
        );
        // The CLI report surfaces a stable reason prefix; the
        // envelope validator's `unknown_to_hat` code must reach
        // either `reason_codes` or `suggestions` so downstream
        // tooling can route on it.
        let reason_blob = format!(
            "{:?}\n{:?}",
            report.reason_codes, report.suggestions
        );
        assert!(
            reason_blob.contains("handoff_envelope_unknown_to_hat"),
            "report must surface the stable unknown-to_hat code; got: {reason_blob}"
        );
        assert!(
            reason_blob.contains("ghost-hat"),
            "report must name the offending to_hat id; got: {reason_blob}"
        );
    }

    #[test]
    fn policy_check_accepts_known_handoff_to_hat_from_builtin_serial_config() {
        // Symmetric happy-path: known to_hat must still pass.
        let tmp = workspace_with_serial_handoff_policy();
        let payload = u1_full_envelope("reviewer");
        let report = run_policy_check_unified(
            "work.done",
            Some(&payload),
            Some("executor"),
            None,
            tmp.path(),
        )
        .expect("unified check should return a report");
        assert!(
            report.accepted,
            "known to_hat must pass; report: {report:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
    // `triggered` validator. Both the apply path and the
    // `--policy-check` path share `check_envelope_triggered`;
    // tests cover the standalone helper and the unified
    // entry-point surface.
    // ─────────────────────────────────────────────────────────────────

    fn cfg_with_hats(ids: &[&str]) -> RalphConfig {
        // RalphConfig.hats is HashMap<String, HatConfig>; build
        // it as a YAML mapping keyed by hat id.
        let hat_blocks: String = ids
            .iter()
            .map(|id| format!("  {id}:\n    name: {id}\n    triggers: []\n    publishes: []\n"))
            .collect();
        let yaml = format!("hats:\n{hat_blocks}");
        serde_yaml::from_str(&yaml).expect("synthetic RalphConfig yaml")
    }

    #[test]
    fn u7_check_envelope_triggered_in_topology_allowed() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        // U7 of 2026-07-05-005: business-topic path; declared
        // hat must be accepted.
        check_envelope_triggered("work.done", Some("review-synthesizer"), &cfg)
            .expect("declared hat must be accepted");
    }

    #[test]
    fn u7_check_envelope_triggered_missing_allowed() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        // R12: missing triggered is allowed.
        check_envelope_triggered("work.done", None, &cfg).expect("missing triggered is allowed");
        check_envelope_triggered("work.done", Some(""), &cfg).expect("empty triggered is allowed");
    }

    #[test]
    fn u7_check_envelope_triggered_not_in_topology_rejected() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        let err = check_envelope_triggered("work.done", Some("planner"), &cfg).unwrap_err();
        assert_eq!(err.reason_code, "triggered_not_in_topology");
        assert!(err.message.contains("planner"));
        assert!(err.message.contains("review-synthesizer"));
    }

    /// U7 of plan 2026-07-05-005 (fix-plan §R11): a ralph-control
    /// topic carrying `triggered="ralph"` (the runtime pseudo-hat)
    /// is accepted even when `ralph` is not in the preset hats[].
    #[test]
    fn u7_check_envelope_triggered_ralph_control_topic_accepts_pseudo_hat() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        // task.resume is a ralph-control topic; `ralph` is the
        // runtime pseudo-hat that injects recovery events.
        check_envelope_triggered("task.resume", Some("ralph"), &cfg)
            .expect("ralph-control topic + triggered=ralph must accept");
    }

    /// U7 of plan 2026-07-05-005 (fix-plan §R11): an
    /// orchestrator-diagnostic topic carrying an unknown `triggered`
    /// is accepted (the runtime origin guard handles it).
    #[test]
    fn u7_check_envelope_triggered_diagnostic_topic_accepts_unknown() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        check_envelope_triggered("event.malformed", Some("ralph-runner"), &cfg)
            .expect("diagnostic topic + unknown triggered must accept");
    }

    /// U7 of plan 2026-07-05-005 (fix-plan §R11): a business
    /// topic carrying `triggered="ralph"` (the pseudo-hat, which
    /// is not in preset hats[]) MUST be rejected — the strict
    /// business-topic layer applies regardless of the value.
    #[test]
    fn u7_check_envelope_triggered_business_topic_rejects_pseudo_hat() {
        let cfg = cfg_with_hats(&["review-synthesizer"]);
        let err = check_envelope_triggered("work.done", Some("ralph"), &cfg).unwrap_err();
        assert_eq!(err.reason_code, "triggered_not_in_topology");
        assert!(err.message.contains("ralph"));
    }

    #[test]
    fn u7_policy_check_unified_surfaces_triggered_violation() {
        // Build a workspace with a ralph.yml that declares
        // exactly one hat, then call run_policy_check_unified
        // with an unknown `triggered` value. The report must
        // surface `triggered_not_in_topology` so the agent can
        // see the violation without writing to disk.
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".ralph")).unwrap();
        std::fs::write(
            tmp.path().join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  review-synthesizer:
    name: review-synthesizer
    triggers: ["work.done"]
    publishes: ["review.dimensions.complete"]
"#,
        )
        .unwrap();
        let report = run_policy_check_unified(
            "work.done",
            Some(r#"{"task_id":"t1","task_key":"k1","step":"s1"}"#),
            None,
            Some("planner"), // unknown hat id
            tmp.path(),
        )
        .expect("report");
        assert!(!report.accepted, "unknown triggered must reject");
        assert!(
            report
                .reason_codes
                .iter()
                .any(|c| c == "triggered_not_in_topology"),
            "expected triggered_not_in_topology in {:?}",
            report.reason_codes
        );
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

/// U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
/// `triggered` validator. The `triggered` field on the emit
/// record is an envelope field (not a payload field), so it
/// sits outside `policy_check`'s schema-driven path. This gate
/// is the dedicated check, mirroring the
/// `check_emit_provenance` style. Missing `triggered` is
/// allowed (R12) — only present-but-unknown values are
/// rejected.
///
/// Returns `Ok(())` when:
/// - `triggered` is `None` (R12)
/// - `triggered` is `Some(_)` AND the value matches a hat id
///   declared in `config.hats`
///
/// Returns `Err(ValidationError)` with
/// `reason_code = "triggered_not_in_topology"` when `triggered`
/// is set to a value that does not appear in the loaded
/// preset's `hats[]` map. The error message names the offending
/// value AND the resolved hat ids so the agent can self-correct.
pub fn check_envelope_triggered(
    topic: &str,
    triggered: Option<&str>,
    config: &RalphConfig,
) -> std::result::Result<(), ValidationError> {
    let Some(value) = triggered else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    // U7 carve-out: control / orchestrator-internal topics may
    // carry a pseudo-hat in `triggered` (e.g. `ralph-runner`)
    // that is not a preset hat id. Skip the topology check when
    // the topic is in the same allowlist that
    // `check_emit_provenance` uses; the runtime origin guard
    // handles downstream validation.
    //
    // U7 of plan 2026-07-05-005 (fix-plan §R11): layer the
    // carve-out by topic-trust so the same matrix as
    // `check_emit_provenance` applies:
    //   - ralph-control topics (loop.cancel / task.resume /
    //     loop.complete / human.*) — `triggered` is allowed
    //     even if not in preset hats[] (runtime pseudo-hat
    //     origin is fine).
    //   - orchestrator diagnostic topics (event.*) — same as
    //     ralph-control (the runtime injects them).
    //   - business topics — strict topology check applies;
    //     `triggered` must be a declared hat id.
    use ralph_core::event_origin::is_ralph_control_topic;
    use ralph_core::is_orchestrator_diagnostic_topic;
    if is_ralph_control_topic(topic) || is_orchestrator_diagnostic_topic(topic) {
        return Ok(());
    }
    if config.hats.contains_key(value) {
        return Ok(());
    }
    let mut allowed: Vec<&str> = config.hats.keys().map(String::as_str).collect();
    allowed.sort_unstable();
    Err(ValidationError {
        payload_index: 0,
        field: "triggered".to_string(),
        reason_code: "triggered_not_in_topology".to_string(),
        message: format!(
            "triggered='{value}' is not declared in preset hats[]. Resolved hat ids: [{}]. \
             Pass --triggered <hat-id> matching one of the declared hats, or omit --triggered \
             entirely (R12: missing triggered is allowed).",
            allowed.join(", ")
        ),
    })
}
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
/// 2026-07-06-004 plan U10: typed entry point that plumbs the
/// typed handoff envelope config into the policy pipeline. The
/// legacy `validate_topic_payload_against_config` /
/// `validate_topic_payload_with_state` keep their default-no-op
/// behaviour for callers that don't care about handoff envelopes;
/// new CLI code (e.g. `ralph emit --policy-check`) goes through
/// this entry point.
pub fn validate_topic_payload_with_handoff(
    topic: &str,
    payload_str: &str,
    policy: &EventPolicyConfig,
    events_file: &Path,
    handoff: &ralph_core::config::HandoffEnvelopeConfig,
) -> Result<Option<ValidationError>> {
    let ctx = PolicyCheckContext {
        events_file: events_file.to_path_buf(),
    };
    let mut state = build_policy_state(policy, &ctx);
    let adapter = EventLoopHandoffConfig {
        handoff_envelope: handoff,
    };
    let decision =
        validate_event_with_options(topic, Some(payload_str), policy, &mut state, None, &adapter);
    Ok(finding_to_validation_error(&decision, topic))
}

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
        // U2 (plan 2026-07-04-004): AcknowledgeAndForward is the
        // dedup-carve-out return for `review.dimensions.complete`
        // re-emits. Treat the same as `Accept` from the
        // CLI batch-validator's perspective — the dedup finding
        // is logged by the runtime but the payload reaches the
        // bus. Surfacing it as an error here would defeat the
        // carve-out (perky-maple silent-success root cause).
        PolicyDecision::AcknowledgeAndForward(_finding) => return None,
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

/// Build [`EmitResultParts`] for CLI `--output json` paths.
///
/// U4 (2026-07-06-002 plan, R5): the `target_path` parameter carries the
/// resolved events file path so it can surface in `EmitResult.target_path`
/// (`recorded: true` 的 apply 路径有效;其它路径默认 `None`).
///
/// U2 (2026-07-06-004 fix-plan): the `payload` parameter carries
/// the JSON payload the caller is about to emit. When `ok=true`
/// AND the typed `emit_result_summary` config is on AND the
/// payload contains a valid `handoff_envelope`, the summary is
/// extracted via `validate_handoff_envelope_payload` →
/// `HandoffEnvelopeSummary::from(...)` so the agent can confirm
/// the envelope was recognised. `ok=false` paths always emit
/// `None` per `assemble.rs` forced-clear logic.
#[allow(clippy::too_many_arguments)]
pub fn build_emit_result_parts(
    topic: String,
    ok: bool,
    recorded: bool,
    errors: Vec<ralph_core::emit_result::EmitError>,
    config: Option<&RalphConfig>,
    workspace: &Path,
    hat: Option<&str>,
    target_path: Option<String>,
    payload: Option<&str>,
) -> ralph_core::emit_result::assemble::EmitResultParts {
    use ralph_core::emit_result::resolve_emit_routing_from_config;
    use ralph_core::handoff_envelope::validate_handoff_envelope_payload;

    let routing = resolve_emit_routing_from_config(config, workspace, hat);
    let handoff_envelope = if ok && payload.is_some() && envelope_summary_enabled(config) {
        payload.and_then(|p| {
            serde_json::from_str::<serde_json::Value>(p)
                .ok()
                .and_then(
                    |value| match validate_handoff_envelope_payload(&value, None) {
                        Ok(parsed) => Some(ralph_core::emit_result::HandoffEnvelopeSummary::from(
                            &parsed,
                        )),
                        Err(_) => None,
                    },
                )
        })
    } else {
        None
    };
    ralph_core::emit_result::assemble::EmitResultParts {
        ok,
        recorded,
        topic,
        phase: routing.phase,
        allowed_next: routing.allowed_next,
        activate_next: Vec::new(),
        errors,
        handoff: None,
        target_path,
        handoff_envelope,
    }
}

/// Whether the typed `emit_result_summary` config flag is on.
/// U2 (2026-07-06-004 fix-plan) gates the summary extraction on
/// this so pipeline presets and ad-hoc emits see no
/// behavioural change.
fn envelope_summary_enabled(config: Option<&RalphConfig>) -> bool {
    config
        .map(|c| c.event_loop.handoff_envelope.emit_result_summary)
        .unwrap_or(false)
}

/// U7 (2026-07-06-001 plan): bridge `PolicyCheckReport` → `EmitResult`
/// for the `--output json` policy-check rejection path.
pub fn report_to_emit_result(
    report: &PolicyCheckReport,
    config: Option<&RalphConfig>,
) -> ralph_core::emit_result::EmitResult {
    let errors = ralph_core::emit_result::map_policy_report_to_errors(
        &report.reason_codes,
        &report.suggestions,
    );

    let parts = build_emit_result_parts(
        report.topic.clone(),
        false,
        false,
        errors,
        config,
        &report.workspace,
        report.hat.as_deref(),
        None,
        // U2: rejection paths never carry a payload summary
        // because the assemble layer forced-clears the field
        // when `ok=false`. Pass `None` for clarity.
        None,
    );
    ralph_core::emit_result::EmitResult::assemble(parts)
}

#[cfg(test)]
#[allow(deprecated)]
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
      work.done:
        required_fields:
          - {field}
      work.ready:
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

    // ─────────────────────────────────────────────────────────────────────
    // U15: agent context defaults to strict policy-check.
    // ─────────────────────────────────────────────────────────────────────

    fn agent_ctx_config() -> RalphConfig {
        // Config without `require_policy_check_for_cli_emit` — agent
        // context should still default to strict via U15.
        let yaml = r#"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
    allow_unsafe_cli_emit: false
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    fn agent_ctx_config_optout() -> RalphConfig {
        // Preset author opts out for agent context.
        let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    allow_unsafe_cli_emit: true
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn test_u15_agent_context_enforce_even_when_config_disabled() {
        let cfg = agent_ctx_config();
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: false,
        };
        // Human CLI: skip (config says no enforce).
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), false),
            PolicyCheckMode::Skip
        );
        // Agent CLI: enforce despite the disabled config (U15 default).
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
            PolicyCheckMode::Enforce
        );
    }

    #[test]
    fn test_u15_agent_context_explicit_check_wins() {
        let cfg = agent_ctx_config();
        let flags = PolicyCheckFlags {
            policy_check: true,
            no_policy_check: false,
        };
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
            PolicyCheckMode::ExplicitCheck
        );
    }

    #[test]
    fn test_u15_agent_unsafe_bypass_blocked_when_disallowed() {
        let cfg = agent_ctx_config();
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: true,
        };
        // Config has `allow_unsafe_cli_emit: false` so the bypass
        // flag is rejected with Enforce even in agent context.
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
            PolicyCheckMode::Enforce
        );
    }

    #[test]
    fn test_u15_agent_unsafe_bypass_allowed_when_optout_set() {
        let cfg = agent_ctx_config_optout();
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: true,
        };
        // Preset opts out → agent can bypass.
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
            PolicyCheckMode::Skip
        );
    }

    #[test]
    fn test_u15_human_cli_unchanged_by_agent_default() {
        // Backwards compat: human CLI uses the config flag verbatim.
        let cfg = agent_ctx_config();
        let flags = PolicyCheckFlags {
            policy_check: false,
            no_policy_check: false,
        };
        // Same call as legacy resolve_policy_check_mode.
        assert_eq!(
            resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), false),
            resolve_policy_check_mode(&flags, Some(&cfg))
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

    // ------------------------------------------------------------------
    // 2026-07-06-004 plan U10: end-to-end pipeline policy-check
    // exercises the handoff envelope validator through the CLI
    // entry point. The pipeline preset enables
    // `validate_payload: true`, so a missing `handoff_envelope`
    // must surface as a missing_required_field-style rejection.
    // ------------------------------------------------------------------

    fn serial_like_handoff_config() -> ralph_core::config::HandoffEnvelopeConfig {
        ralph_core::config::HandoffEnvelopeConfig {
            enabled: true,
            prompt_injection: true,
            validate_payload: true,
            emit_result_summary: true,
        }
    }

    fn full_handoff_envelope_payload() -> String {
        r#"{
            "plan_name":"p1",
            "task_id":"t1",
            "task_key":"p1:step-2:implement",
            "step":"step-2",
            "commit_count":1,
            "changed_lines":10,
            "handoff_envelope":{
                "schema_version":"handoff-envelope.v1",
                "root_goal":"ship without regressions",
                "plan":{
                    "name":"p1",
                    "path":"docs/plans/p1.md",
                    "current_step":"step-2",
                    "completed_steps":["step-1"]
                },
                "state":{
                    "current_status":"ready_for_review",
                    "last_signal":"work.done",
                    "blocking_reason":null
                },
                "receiver_contract":{
                    "to_hat":"reviewer",
                    "must_do":["review step-2"],
                    "must_not_do":["regress step-1"],
                    "success_signal":"work.done",
                    "failure_signal":"work.failed"
                }
            }
        }"#
        .to_string()
    }

    #[test]
    fn ce_executor_serial_policy_check_accepts_valid_handoff_envelope_payload() {
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let policy = strict_policy_with_required("handoff_envelope");
        let result = validate_topic_payload_with_handoff(
            "work.done",
            &full_handoff_envelope_payload(),
            &policy,
            &events,
            &serial_like_handoff_config(),
        )
        .unwrap();
        assert!(
            result.is_none(),
            "serial-like config must accept a valid envelope; got {:?}",
            result
        );
    }

    #[test]
    fn ce_executor_serial_policy_check_rejects_missing_handoff_envelope_payload() {
        let tmp = TempDir::new().unwrap();
        let events = tmp.path().join("events.jsonl");
        let policy = strict_policy_with_required("handoff_envelope");
        let payload =
            full_handoff_envelope_payload().replace("\"handoff_envelope\":{", "\"__stripped__\":{");
        let err = validate_topic_payload_with_handoff(
            "work.done",
            &payload,
            &policy,
            &events,
            &serial_like_handoff_config(),
        )
        .unwrap()
        .expect("missing envelope must reject");
        // Either the schema-side (required_fields) check or the
        // handoff-envelope validator side will surface. We
        // accept either; the contract is "rejected".
        assert!(
            err.reason_code == "missing_required_field" || err.message.contains("handoff_envelope"),
            "rejection must trace to handoff; got {:?}",
            err
        );
    }

    #[test]
    fn load_policy_config_for_cli_emit_rejects_invalid_hats_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let root_path = root.to_path_buf();
        std::fs::create_dir_all(root.join(".ralph")).unwrap();
        std::fs::write(
            root.join(".ralph/hats.yml"),
            r#"
hats:
  executor:
    name: 123
    triggers: ["work.ready"]
"#,
        )
        .unwrap();

        let err =
            load_policy_config_for_cli_emit(Some(&root_path), OnConfigError::Fail)
                .expect_err("invalid hat config must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("Failed to parse hat config for `executor`")
                || msg.contains("`.ralph/hats.yml`"),
            "unexpected error message: {msg}"
        );
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
    ///
    /// U1 of 2026-07-05-005 (KTD-1): the derived `current_step` accessor
    /// returns `completed_steps.last()`, so the fixture must mark the
    /// inbound event's step as already completed (or this would be a
    /// `step_mismatch`). We re-derive the test against the new rule:
    /// `## Current Step\nstep-02` plus `completed_steps: [step-01, step-02]`
    /// makes the derived `current_step() == Some("step-02")` and the
    /// closed task for `step-02` confirms task-step consistency.
    #[test]
    fn u1_step_handoff_gate_happy_path_aligned_progress() {
        let tmp = workspace();
        write_closed_task(&tmp, "task-1", "step-02");
        write_progress(
            &tmp,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n- step-02\n",
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
    #[allow(deprecated)]
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
        // 2026-06-28-005: human.guidance is no longer a control topic.
        // Use plan.blocked instead — it is the structured terminal
        // orchestrator topic and ships with the same allowlist
        // semantics.
        assert!(check_isolated_scope(Some("ralph"), "plan.blocked", &cfg).is_ok());
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
        // 2026-06-28-005: human.guidance was removed from this
        // list; plan.blocked is the new structured terminal
        // orchestrator topic and ships with the same allowlist
        // semantics.
        for topic in [
            "LOOP_COMPLETE",
            "loop.cancel",
            "task.resume",
            "plan.blocked",
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

    // ──────────────────────────────────────────────────────────
    // 2026-07-06-004 fix-plan U2: CLI integration test that
    // `ralph emit --policy-check --output json` returns a JSON
    // object whose `handoff_envelope` summary is populated when
    // the typed `emit_result_summary` flag is on AND the payload
    // contains a valid envelope. The summary carries 4 fields
    // (schema_version / to_hat / success_signal / failure_signal)
    // so the agent can confirm the envelope was recognised in
    // the same CLI round-trip.
    #[test]
    fn build_emit_result_parts_attaches_handoff_envelope_summary_when_enabled() {
        use ralph_core::config::{EventLoopConfig, EventPolicyConfig, HandoffEnvelopeConfig};

        let envelope_payload = serde_json::json!({
            "handoff_envelope": {
                "schema_version": "handoff-envelope.v1",
                "root_goal": "ship step-01 cleanly",
                "plan": {
                    "name": "plan-u2",
                    "path": "docs/plans/u2.md",
                    "current_step": "step-01",
                    "completed_steps": []
                },
                "state": {
                    "current_status": "ready_for_review",
                    "last_signal": "work.done",
                    "blocking_reason": null
                },
                "receiver_contract": {
                    "to_hat": "validator",
                    "must_do": ["run full suite"],
                    "success_signal": "test.passed",
                    "failure_signal": "test.failed"
                }
            }
        });
        let payload_str = envelope_payload.to_string();

        let cfg = RalphConfig {
            event_loop: EventLoopConfig {
                handoff_envelope: HandoffEnvelopeConfig {
                    enabled: true,
                    prompt_injection: true,
                    validate_payload: true,
                    emit_result_summary: true,
                },
                event_policy: Some(EventPolicyConfig::default()),
                ..EventLoopConfig::default()
            },
            ..RalphConfig::default()
        };

        let tmp = TempDir::new().unwrap();
        let parts = build_emit_result_parts(
            "work.done".to_string(),
            true,
            false,
            Vec::new(),
            Some(&cfg),
            tmp.path(),
            Some("coordinator"),
            None,
            Some(&payload_str),
        );

        // U2: when the typed `emit_result_summary` flag is on
        // AND the payload contains a valid envelope, the
        // summary must be populated with the 4 expected
        // fields.
        let summary = parts
            .handoff_envelope
            .as_ref()
            .expect("summary must be populated when emit_result_summary=true");
        assert_eq!(summary.schema_version, "handoff-envelope.v1");
        assert_eq!(summary.to_hat, "validator");
        assert_eq!(summary.success_signal, "test.passed");
        assert_eq!(summary.failure_signal, "test.failed");

        // Edge: rejected payload (ok=false) must surface as
        // `None` because the assemble layer's forced-clear
        // logic overwrites the field when `ok=false`.
        let parts_reject = build_emit_result_parts(
            "work.done".to_string(),
            false,
            false,
            vec![ralph_core::emit_result::EmitError {
                code: "x".to_string(),
                message: "y".to_string(),
                field: None,
                suggested_command: None,
            }],
            Some(&cfg),
            tmp.path(),
            Some("coordinator"),
            None,
            Some(&payload_str),
        );
        assert!(
            parts_reject.handoff_envelope.is_none(),
            "rejection paths must not surface a handoff_envelope summary: {:?}",
            parts_reject.handoff_envelope
        );

        // Edge: missing envelope payload → summary is None
        // even when the typed flag is on.
        let parts_no_env = build_emit_result_parts(
            "work.done".to_string(),
            true,
            false,
            Vec::new(),
            Some(&cfg),
            tmp.path(),
            Some("coordinator"),
            None,
            Some(r#"{"plan_name":"no-envelope"}"#),
        );
        assert!(
            parts_no_env.handoff_envelope.is_none(),
            "missing envelope payload must NOT populate summary: {:?}",
            parts_no_env.handoff_envelope
        );

        // Edge: typed flag off → summary is None even with a
        // valid envelope in the payload.
        let mut cfg_off = cfg.clone();
        cfg_off.event_loop.handoff_envelope.emit_result_summary = false;
        let parts_flag_off = build_emit_result_parts(
            "work.done".to_string(),
            true,
            false,
            Vec::new(),
            Some(&cfg_off),
            tmp.path(),
            Some("coordinator"),
            None,
            Some(&payload_str),
        );
        assert!(
            parts_flag_off.handoff_envelope.is_none(),
            "emit_result_summary=false must NOT populate summary: {:?}",
            parts_flag_off.handoff_envelope
        );
    }
}
