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

// 2026-08-08-004 fix-plan (U5 / R6 / A2): SHA-256 recomputation helper
// for the scope handoff guard. The guard only validates the *format*
// of declared digests; this helper closes the bypass by recomputing
// SHA-256 over the artifact bytes and constant-time-comparing against
// the declared digest. Used from every topic branch in
// `check_scope_handoff_guard`.
use sha2::{Digest, Sha256};

/// U5 (2026-08-08-004 fix-plan, A2): recompute SHA-256 over
/// `artifact_path` (relative to `workspace_root`) and compare against
/// `declared_digest`. Returns `Err(ValidationError)` with
/// `reason_code = "scope_handoff_inconsistent"` on read error, format
/// mismatch, or digest mismatch. The declared digest is **not** used
/// as input to the SHA-256 computation (matches the
/// `ralph-tools-emit.md`「Scope handoff contract」canonicalization
/// rule: "不得把 scope_digest 字段本身算进去").
// ValidationError is the stable, structured policy-check error returned by
// this module. Keep it unboxed here so callers retain the existing error shape
// and avoid an API-wide Result type change for this internal helper.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_artifact_digest(
    workspace_root: &Path,
    artifact_path: &str,
    declared_digest: &str,
    digest_field: &str,
) -> std::result::Result<(), ValidationError> {
    let full_path = workspace_root.join(artifact_path);
    let bytes = match std::fs::read(&full_path) {
        Ok(b) => b,
        Err(e) => {
            return Err(ValidationError {
                payload_index: 0,
                field: digest_field.to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{digest_field} verification failed: could not read {artifact_path}: {e}"
                ),
                ..Default::default()
            });
        }
    };
    let computed = {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let mut hex_str = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut hex_str, "{byte:02x}");
        }
        hex_str
    };
    if !declared_digest.eq_ignore_ascii_case(&computed) {
        return Err(ValidationError {
            payload_index: 0,
            field: digest_field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{digest_field} does not match SHA-256 of {artifact_path} bytes; manifest may have been tampered with (declared={declared_digest}, computed={computed})"
            ),
            ..Default::default()
        });
    }
    Ok(())
}

/// Verify the merge boundary digest over its canonical JSON representation.
/// The self-referential `boundary_digest` field is excluded before encoding;
/// serde_json's default map representation provides stable key ordering and
/// the canonical bytes end with one newline.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_canonical_json_digest(
    workspace_root: &Path,
    artifact_path: &str,
    declared_digest: &str,
    digest_field: &str,
) -> std::result::Result<(), ValidationError> {
    let full_path = workspace_root.join(artifact_path);
    let bytes = std::fs::read(&full_path).map_err(|e| ValidationError {
        payload_index: 0,
        field: digest_field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{digest_field} verification failed: could not read {artifact_path}: {e}"),
        ..Default::default()
    })?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| ValidationError {
            payload_index: 0,
            field: digest_field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{digest_field} verification failed: {artifact_path} is not valid JSON: {e}"
            ),
            ..Default::default()
        })?;
    if let Some(object) = value.as_object_mut() {
        object.remove("boundary_digest");
    }
    let mut canonical = serde_json::to_vec(&value).map_err(|e| ValidationError {
        payload_index: 0,
        field: digest_field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{digest_field} canonicalization failed: {e}"),
        ..Default::default()
    })?;
    canonical.push(b'\n');
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    let computed = format!("{:x}", hasher.finalize());
    if !declared_digest.eq_ignore_ascii_case(&computed) {
        return Err(ValidationError {
            payload_index: 0,
            field: digest_field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{digest_field} does not match canonical SHA-256 of {artifact_path}; manifest may have been tampered with (declared={declared_digest}, computed={computed})"
            ),
            ..Default::default()
        });
    }
    Ok(())
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

/// U1 (plan 2026-08-08-004, D12/D13): CLI scope handoff consistency gate.
///
/// Fires for the four scope topics: `merge.integrated`, `merge.stabilized`,
/// `postmerge.changemap.ready`, and `redteam.plan.resolved`. Verifies the
/// payload carries the required scope-related fields, the artifact paths
/// are under the allowed roots (`.ralph/merge/`, `.ralph/post-merge/`,
/// `.ralph/red-team/`), and the file exists.
///
/// This gate runs in BOTH `--policy-check` and real emit paths, and even
/// when `--unsafe-no-policy-check` is set — the scope contract is mandatory
/// for these topics.
///
/// The digest re-computation and threshold checks are handled by the
/// `payload_consistency` rules in the EventLoop; this CLI gate enforces
/// the structural preconditions (field presence, path validity, file
/// readability) so agents get early backpressure before writing.
///
/// Paths are normalized, reject traversal components, and are canonicalized
/// before the resolved path is checked against the topic-specific root. This
/// also rejects symlinks that point outside the allowed artifact directory.
#[allow(clippy::result_large_err)]
pub fn check_scope_handoff_guard(
    topic: &str,
    payload_str: &str,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    const SCOPE_TOPICS: &[&str] = &[
        "merge.integrated",
        "merge.stabilized",
        "postmerge.changemap.ready",
        "redteam.plan.resolved",
    ];

    if !SCOPE_TOPICS.contains(&topic) {
        return Ok(());
    }

    let trimmed = payload_str.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return Err(ValidationError {
            payload_index: 0,
            field: "payload".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "scope handoff guard: topic '{topic}' requires a non-empty JSON object payload"
            ),
            ..Default::default()
        });
    }

    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return Err(ValidationError {
                payload_index: 0,
                field: "payload".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "scope handoff guard: topic '{topic}' requires valid JSON payload for scope field extraction"
                ),
                ..Default::default()
            });
        }
    };

    // Topic-specific structural checks.
    match topic {
        "merge.integrated" => {
            check_merge_integrated_scope_fields(&value, workspace_root)
        }
        "merge.stabilized" => {
            check_merge_stabilized_scope_fields(&value, workspace_root)
        }
        "postmerge.changemap.ready" => {
            check_postmerge_changemap_scope_fields(&value, workspace_root)
        }
        "redteam.plan.resolved" => {
            check_redteam_plan_resolved_scope_fields(&value, workspace_root)
        }
        _ => Ok(()),
    }
}

#[allow(clippy::result_large_err)]
fn check_merge_integrated_scope_fields(
    value: &serde_json::Value,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    // U8 (M1): structural guard — payload must be a JSON object; the
    // previous `value.as_object().unwrap()` panicked on arrays /
    // strings / null.
    let obj = expect_scope_payload_object(value, "merge.integrated")?;

    // U9 (M2): shared `merge_boundary_path` + `merge_boundary_digest`
    // structural validation; `require_status = Some("merge.integrated")`
    // adds the `merge_boundary_status` enum check on the integrated
    // topic.
    let (boundary_path, boundary_digest) = validate_merge_boundary_pair(
        obj,
        workspace_root,
        Some("merge.integrated"),
    )?;

    // Boundary digests use canonical JSON, excluding the self-referential
    // boundary_digest field. Other scope artifacts remain raw-byte digests.
    verify_canonical_json_digest(
        workspace_root,
        &boundary_path,
        &boundary_digest,
        "merge_boundary_digest",
    )?;

    Ok(())
}

#[allow(clippy::result_large_err)]
fn check_merge_stabilized_scope_fields(
    value: &serde_json::Value,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    // U8 (M1): structural guard — payload must be a JSON object; the
    // previous `value.as_object().unwrap()` panicked on arrays /
    // strings / null.
    let obj = expect_scope_payload_object(value, "merge.stabilized")?;

    // U9 (M2): `require_status = None` skips the `merge_boundary_status`
    // enum check on the stabilized topic; only path + digest are
    // enforced.
    let (boundary_path, boundary_digest) =
        validate_merge_boundary_pair(obj, workspace_root, None)?;

    // Boundary digests use the same canonical JSON representation as
    // merge.integrated.
    verify_canonical_json_digest(
        workspace_root,
        &boundary_path,
        &boundary_digest,
        "merge_boundary_digest",
    )?;

    Ok(())
}

/// U8 (M1): payload-must-be-object guard. Replaces
/// `value.as_object().unwrap()` with a clean `ValidationError` so
/// arrays / strings / null payloads fail-close instead of panicking
/// on the unwrap.
#[allow(clippy::result_large_err)]
fn expect_scope_payload_object<'a>(
    value: &'a serde_json::Value,
    topic: &str,
) -> std::result::Result<&'a serde_json::Map<String, serde_json::Value>, ValidationError> {
    match value.as_object() {
        Some(o) => Ok(o),
        None => Err(ValidationError {
            payload_index: 0,
            field: "payload".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "scope handoff guard: topic '{topic}' payload must be a JSON object"
            ),
            ..Default::default()
        }),
    }
}

/// Resolve an artifact path only when it is a relative path below the
/// topic-specific scope directory. Lexical prefix checks alone allow
/// `..` traversal and symlinks to escape the intended artifact root.
#[allow(clippy::result_large_err)]
fn validate_scoped_artifact_path(
    workspace_root: &Path,
    artifact_path: &str,
    allowed_prefix: &str,
    field: &str,
) -> std::result::Result<PathBuf, ValidationError> {
    let normalized = artifact_path.replace('\\', "/");
    let relative = Path::new(&normalized);
    let invalid_component = relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::Prefix(_)
                | std::path::Component::RootDir
                | std::path::Component::ParentDir
                | std::path::Component::CurDir
        )
    });
    if invalid_component || !normalized.starts_with(allowed_prefix) {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{field} must be a relative path under {allowed_prefix}; got: {artifact_path}"
            ),
            ..Default::default()
        });
    }

    let full_path = workspace_root.join(relative);
    let canonical_path = std::fs::canonicalize(&full_path).map_err(|e| ValidationError {
        payload_index: 0,
        field: field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{field} file does not exist or is unreadable: {artifact_path}: {e}"),
        ..Default::default()
    })?;
    let canonical_root = std::fs::canonicalize(workspace_root.join(allowed_prefix.trim_end_matches('/')))
        .map_err(|e| ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("scope artifact root is unavailable: {allowed_prefix}: {e}"),
            ..Default::default()
        })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{field} resolves outside {allowed_prefix}: {artifact_path}"),
            ..Default::default()
        });
    }

    let metadata = std::fs::metadata(&canonical_path).map_err(|e| ValidationError {
        payload_index: 0,
        field: field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{field} metadata could not be read: {artifact_path}: {e}"),
        ..Default::default()
    })?;
    if metadata.len() > 1024 * 1024 {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{field} exceeds 1 MiB limit: {artifact_path}"),
            ..Default::default()
        });
    }

    Ok(canonical_path)
}

#[allow(clippy::result_large_err)]
fn required_scope_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    topic: &str,
) -> std::result::Result<String, ValidationError> {
    match obj.get(field).and_then(serde_json::Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} requires non-empty string field {field}"),
            ..Default::default()
        }),
    }
}

#[allow(clippy::result_large_err)]
fn required_scope_u64(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    topic: &str,
) -> std::result::Result<u64, ValidationError> {
    obj.get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} requires non-negative integer field {field}"),
            ..Default::default()
        })
}

/// U9 (M2): shared `merge_boundary_path` + `merge_boundary_digest`
/// structural validation for `merge.integrated` and `merge.stabilized`.
///
/// `require_status = Some(topic)` adds the `merge_boundary_status` enum
/// check on the integrated topic; `None` skips it for the stabilized
/// topic. Returns the validated `(path, digest)` so the caller can
/// thread them into the SHA-256 recomputation (`verify_artifact_digest`).
#[allow(clippy::result_large_err)]
fn validate_merge_boundary_pair(
    obj: &serde_json::Map<String, serde_json::Value>,
    workspace_root: &Path,
    require_status: Option<&str>,
) -> std::result::Result<(String, String), ValidationError> {
    let path = match obj.get("merge_boundary_path").and_then(|v| v.as_str()) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => {
            return Err(ValidationError {
                payload_index: 0,
                field: "merge_boundary_path".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: "merge_boundary_path is required and must be a non-empty string".to_string(),
                ..Default::default()
            });
        }
    };

    validate_scoped_artifact_path(
        workspace_root,
        &path,
        ".ralph/merge/",
        "merge_boundary_path",
    )?;

    let digest = match obj.get("merge_boundary_digest").and_then(|v| v.as_str()) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => {
            return Err(ValidationError {
                payload_index: 0,
                field: "merge_boundary_digest".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: "merge_boundary_digest is required and must be a non-empty string".to_string(),
                ..Default::default()
            });
        }
    };

    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "merge_boundary_digest".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "merge_boundary_digest must be a 64-char hex string; got {} chars",
                digest.len()
            ),
            ..Default::default()
        });
    }

    if let Some(topic) = require_status {
        let status = match obj.get("merge_boundary_status").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => {
                return Err(ValidationError {
                    payload_index: 0,
                    field: "merge_boundary_status".to_string(),
                    reason_code: "scope_handoff_inconsistent".to_string(),
                    message: format!("{topic} requires merge_boundary_status in payload"),
                    ..Default::default()
                });
            }
        };
        if status != "complete" && status != "incomplete" {
            return Err(ValidationError {
                payload_index: 0,
                field: "merge_boundary_status".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "merge_boundary_status must be 'complete' or 'incomplete'; got: {status}"
                ),
                ..Default::default()
            });
        }
    }

    Ok((path, digest))
}

#[allow(clippy::result_large_err)]
fn check_postmerge_changemap_scope_fields(
    value: &serde_json::Value,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    // U8 (M1): structural guard.
    let obj = expect_scope_payload_object(value, "postmerge.changemap.ready")?;
    let manifest_path = required_scope_string(obj, "scope_manifest_path", "postmerge.changemap.ready")?;
    let manifest_digest = required_scope_string(obj, "scope_digest", "postmerge.changemap.ready")?;
    let _scope_status = required_scope_string(obj, "scope_status", "postmerge.changemap.ready")?;
    let _overall_confidence = required_scope_u64(obj, "overall_confidence", "postmerge.changemap.ready")?;
    let _critical_unknown_count = required_scope_u64(obj, "critical_unknown_count", "postmerge.changemap.ready")?;
    let scope_base_sha = required_scope_string(obj, "scope_base_sha", "postmerge.changemap.ready")?;
    let _scope_source = required_scope_string(obj, "scope_source", "postmerge.changemap.ready")?;

    if manifest_digest.len() != 64 || !manifest_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_digest".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("scope_digest must be a 64-char hex string; got {} chars", manifest_digest.len()),
            ..Default::default()
        });
    }
    if scope_base_sha.len() != 40 || !scope_base_sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_base_sha".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("scope_base_sha must be a 40-char hex SHA; got {} chars", scope_base_sha.len()),
            ..Default::default()
        });
    }

    validate_scoped_artifact_path(
        workspace_root,
        &manifest_path,
        ".ralph/post-merge/",
        "scope_manifest_path",
    )?;

    // U5 (R6 / A2): SHA-256 recomputation on the manifest file.
    verify_artifact_digest(workspace_root, &manifest_path, &manifest_digest, "scope_digest")?;

    Ok(())
}

#[allow(clippy::result_large_err)]
fn check_redteam_plan_resolved_scope_fields(
    value: &serde_json::Value,
    workspace_root: &Path,
) -> std::result::Result<(), ValidationError> {
    // U8 (M1): structural guard.
    let obj = expect_scope_payload_object(value, "redteam.plan.resolved")?;
    let manifest_path = required_scope_string(obj, "scope_manifest_path", "redteam.plan.resolved")?;
    let manifest_digest = required_scope_string(obj, "scope_digest", "redteam.plan.resolved")?;
    let _scope_status = required_scope_string(obj, "scope_status", "redteam.plan.resolved")?;
    let _overall_confidence = required_scope_u64(obj, "overall_confidence", "redteam.plan.resolved")?;
    let _critical_unknown_count = required_scope_u64(obj, "critical_unknown_count", "redteam.plan.resolved")?;
    let scope_base_sha = required_scope_string(obj, "scope_base_sha", "redteam.plan.resolved")?;
    let patch_path = required_scope_string(obj, "resolved_patch_path", "redteam.plan.resolved")?;
    let patch_digest = required_scope_string(obj, "patch_digest", "redteam.plan.resolved")?;

    if manifest_digest.len() != 64 || !manifest_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_digest".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("scope_digest must be a 64-char hex string; got {} chars", manifest_digest.len()),
            ..Default::default()
        });
    }
    if scope_base_sha.len() != 40 || !scope_base_sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_base_sha".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("scope_base_sha must be a 40-char Git SHA, not a placeholder; got: {scope_base_sha}"),
            ..Default::default()
        });
    }
    if patch_digest.len() != 64 || !patch_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError {
            payload_index: 0,
            field: "patch_digest".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("patch_digest must be a 64-char hex string; got {} chars", patch_digest.len()),
            ..Default::default()
        });
    }

    validate_scoped_artifact_path(
        workspace_root,
        &manifest_path,
        ".ralph/red-team/",
        "scope_manifest_path",
    )?;
    validate_scoped_artifact_path(
        workspace_root,
        &patch_path,
        ".ralph/red-team/",
        "resolved_patch_path",
    )?;

    // U5 (R6 / A2): SHA-256 recomputation on the manifest file
    // AND the resolved patch file (both have declared digests).
    verify_artifact_digest(workspace_root, &manifest_path, &manifest_digest, "scope_digest")?;
    verify_artifact_digest(workspace_root, &patch_path, &patch_digest, "patch_digest")?;

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────
// 2026-08-08-004 fix-plan Unit 4 (R5/A1): direct unit tests for
// `check_scope_handoff_guard` + the U5 (R6/A2) SHA-256 recomputation
// helper. Each topic branch has at least one accept-test (real
// artifact + recomputed digest) and one reject-test (missing
// artifact / wrong digest / non-JSON-object payload). The
// not-a-scope-topic pass-through test covers the gate's exit-ramp
// for non-scope topics.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    /// Write a fake manifest to `relative_path` under `root` and return
    /// the lowercase hex SHA-256 of the bytes. Mirrors the canonical
    /// format used in the production `verify_artifact_digest` helper so
    /// accept-tests can construct matching declared digests.
    fn write_artifact(
        root: &std::path::Path,
        relative_path: &str,
        body: &[u8],
    ) -> (String, PathBuf) {
        let abs = root.join(relative_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&abs, body).expect("write artifact");
        let mut hasher = Sha256::new();
        hasher.update(body);
        let digest = hasher.finalize();
        let mut hex_str = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut hex_str, "{byte:02x}");
        }
        (hex_str, abs)
    }

    fn build_boundary_payload(digest: &str, status: Option<&str>) -> String {
        let mut payload = format!(
            r#"{{"merge_boundary_path":".ralph/merge/merge-boundary.json","merge_boundary_digest":"{digest}""#
        );
        if let Some(s) = status {
            payload.push_str(&format!(r#","merge_boundary_status":"{s}""#));
        }
        payload.push('}');
        payload
    }

    fn write_boundary(root: &std::path::Path, body: &[u8]) -> (String, PathBuf) {
        let abs = root.join(".ralph/merge/merge-boundary.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("create parent");
        std::fs::write(&abs, body).expect("write boundary");
        let mut value: serde_json::Value = serde_json::from_slice(body).expect("valid JSON");
        value
            .as_object_mut()
            .expect("boundary object")
            .remove("boundary_digest");
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        (format!("{:x}", hasher.finalize()), abs)
    }

    fn build_postmerge_payload(manifest_path: &str, manifest_digest: &str) -> String {
        format!(
            r#"{{"scope_manifest_path":"{manifest_path}","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}}"#
        )
    }

    fn build_redteam_payload(
        manifest_path: &str,
        manifest_digest: &str,
        patch_path: &str,
        patch_digest: &str,
    ) -> String {
        format!(
            r#"{{"scope_manifest_path":"{manifest_path}","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","resolved_patch_path":"{patch_path}","patch_digest":"{patch_digest}"}}"#
        )
    }

    #[test]
    fn not_a_scope_topic_passes_through() {
        // R5: work.done is not in SCOPE_TOPICS, so the guard exits
        // before any structural check — even an empty payload returns
        // Ok(()).
        let root = tempfile::tempdir().expect("tempdir");
        let result = check_scope_handoff_guard("work.done", "", root.path());
        assert!(result.is_ok(), "non-scope topic must pass through");
    }

    #[test]
    fn merge_integrated_accepts_real_artifact() {
        let root = tempfile::tempdir().expect("tempdir");
        let (digest, _) = write_boundary(root.path(), br#"{"target_identity":"abc"}"#);
        let payload = build_boundary_payload(&digest, Some("complete"));
        let result = check_scope_handoff_guard("merge.integrated", &payload, root.path());
        assert!(result.is_ok(), "merge.integrated with real artifact must accept: {result:?}");
    }

    #[test]
    fn merge_integrated_rejects_missing_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        // Digest is a syntactically valid 64-hex but the file is not written.
        let payload = build_boundary_payload(
            "0000000000000000000000000000000000000000000000000000000000000000",
            Some("complete"),
        );
        let result = check_scope_handoff_guard("merge.integrated", &payload, root.path());
        let err = result.expect_err("missing manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("merge_boundary_path"));
    }

    #[test]
    fn merge_integrated_rejects_wrong_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_artifact(
            root.path(),
            ".ralph/merge/merge-boundary.json",
            br#"{"target_identity":"abc"}"#,
        );
        // Declare a different 64-hex digest than the file's actual SHA-256.
        let payload = build_boundary_payload(
            "1111111111111111111111111111111111111111111111111111111111111111",
            Some("complete"),
        );
        let result = check_scope_handoff_guard("merge.integrated", &payload, root.path());
        let err = result.expect_err("tampered digest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("merge_boundary_digest"));
    }

    #[test]
    fn merge_stabilized_accepts_real_artifact() {
        let root = tempfile::tempdir().expect("tempdir");
        let (digest, _) = write_boundary(root.path(), br#"{"target_identity":"xyz"}"#);
        // merge.stabilized does NOT require merge_boundary_status.
        let payload = build_boundary_payload(&digest, None);
        let result = check_scope_handoff_guard("merge.stabilized", &payload, root.path());
        assert!(result.is_ok(), "merge.stabilized with real artifact must accept: {result:?}");
    }

    #[test]
    fn merge_stabilized_rejects_missing_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let payload = build_boundary_payload(
            "0000000000000000000000000000000000000000000000000000000000000000",
            None,
        );
        let result = check_scope_handoff_guard("merge.stabilized", &payload, root.path());
        let err = result.expect_err("missing manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
    }

    #[test]
    fn merge_stabilized_rejects_wrong_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_artifact(
            root.path(),
            ".ralph/merge/merge-boundary.json",
            br#"{"target_identity":"xyz"}"#,
        );
        let payload = build_boundary_payload(
            "1111111111111111111111111111111111111111111111111111111111111111",
            None,
        );
        let result = check_scope_handoff_guard("merge.stabilized", &payload, root.path());
        let err = result.expect_err("tampered digest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("merge_boundary_digest"));
    }

    #[test]
    fn postmerge_changemap_accepts_real_artifact() {
        let root = tempfile::tempdir().expect("tempdir");
        let (digest, _) = write_artifact(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            br#"{"resolved":true}"#,
        );
        let payload = build_postmerge_payload(".ralph/post-merge/scope-manifest.json", &digest);
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        assert!(result.is_ok(), "postmerge.changemap.ready with real artifact must accept: {result:?}");
    }

    #[test]
    fn postmerge_changemap_rejects_missing_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let payload = build_postmerge_payload(
            ".ralph/post-merge/missing.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("missing manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
    }

    #[test]
    fn postmerge_changemap_rejects_wrong_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_artifact(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            br#"{"resolved":true}"#,
        );
        let payload = build_postmerge_payload(
            ".ralph/post-merge/scope-manifest.json",
            "1111111111111111111111111111111111111111111111111111111111111111",
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("tampered digest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("scope_digest"));
    }

    #[test]
    fn redteam_plan_resolved_accepts_real_artifact() {
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            br#"{"resolved":true}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a","b"]}"#,
        );
        let payload = build_redteam_payload(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        assert!(result.is_ok(), "redteam.plan.resolved with real artifacts must accept: {result:?}");
    }

    #[test]
    fn redteam_plan_resolved_rejects_missing_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload(
            ".ralph/red-team/scope-manifest.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("missing manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
    }

    #[test]
    fn redteam_plan_resolved_rejects_wrong_patch_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            br#"{"resolved":true}"#,
        );
        write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            "1111111111111111111111111111111111111111111111111111111111111111",
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("tampered patch_digest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("patch_digest"));
    }

    #[test]
    fn non_object_payload_rejects_without_panic() {
        // R9 / M1: arrays, strings, null must fail-close with
        // `scope_handoff_inconsistent`, not panic on the previous
        // `value.as_object().unwrap()`.
        let root = tempfile::tempdir().expect("tempdir");
        for payload in [
            "[1,2,3]",
            "\"string\"",
            "null",
            "42",
        ] {
            let result = check_scope_handoff_guard("merge.integrated", payload, root.path());
            assert!(
                result.is_err(),
                "non-object payload {payload} must reject, got {result:?}"
            );
            if let Err(e) = result {
                assert_eq!(e.reason_code, "scope_handoff_inconsistent");
            }
        }
    }

    #[test]
    fn scoped_artifact_paths_reject_parent_traversal() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join(".ralph/merge")).expect("create scope root");
        let outside = root.path().join("outside.json");
        let (digest, _) = write_artifact(root.path(), "outside.json", br#"{}"#);
        let payload = format!(
            r#"{{"merge_boundary_path":".ralph/merge/../../outside.json","merge_boundary_digest":"{digest}","merge_boundary_status":"complete"}}"#
        );
        let result = check_scope_handoff_guard("merge.integrated", &payload, root.path());
        assert!(result.is_err(), "path traversal must not resolve to {outside:?}");
    }

    #[test]
    fn empty_payload_rejects() {
        let root = tempfile::tempdir().expect("tempdir");
        let result = check_scope_handoff_guard("merge.integrated", "", root.path());
        let err = result.expect_err("empty payload must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
    }
}
