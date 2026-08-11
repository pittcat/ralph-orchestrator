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
use ralph_core::config::scope_topics::SCOPE_TOPICS;
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

/// U7 (2026-08-10-002 plan, R11/S1): the canonical self-excluding
/// digest verifier, the typed threshold / bool / bounded string
/// readers, and the two `verify_*_digest` wrappers live in
/// `super::scope` (see `crates/ralph-cli/src/policy_check/scope.rs`).
/// The re-exports below keep the existing public surface for the
/// `mod tests` block at the bottom of this file and for any external
/// caller that still imports through `super::gates::verify_*`. The
/// `#[allow(unused_imports)]` is needed because the production
/// callers in `gates.rs` only use `verify_canonical_json_digest` /
/// `verify_scope_manifest_digest`; the parameterised helper
/// `verify_canonical_json_digest_excluding` is reached only by the
/// inline tests at the bottom of this file.
#[allow(unused_imports)]
pub(crate) use super::scope::{
    bounded_scope_string, typed_required_bool, typed_threshold_u64, verify_canonical_json_digest,
    verify_canonical_json_digest_excluding, verify_scope_manifest_digest,
};

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
pub(crate) fn validate_scoped_artifact_path(
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
    // U2 (R4/A3 input-validation leg): route the call through the
    // bounded reader with a 256-char max. The unbounded variant is
    // preserved in tests; production callers all go through this
    // bound to defeat the 4 KiB UTF-8 attack documented in finding
    // A3.
    bounded_scope_string(obj, field, topic, 256)
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

/// U2 (2026-08-10-002 plan, R5): payload-vs-manifest equality for a
/// single typed field. Returns `Ok(())` when the field is absent on both
/// sides, when both sides carry the same JSON value, or when the field
/// is absent on the manifest but the payload carries the canonical
/// shape (caller may want to require the manifest side as well).
///
/// Returns a structured `ValidationError` naming the mismatched field
/// when the two sides disagree. The manifest is read by the topic
/// guards before calling this helper (see `read_scope_manifest_object`).
#[allow(clippy::result_large_err)]
fn assert_payload_manifest_field_equal(
    topic: &str,
    field: &str,
    payload: &serde_json::Value,
    manifest: &serde_json::Value,
) -> std::result::Result<(), ValidationError> {
    let payload_has = !payload.is_null();
    let manifest_has = !manifest.is_null();
    if !payload_has && !manifest_has {
        return Ok(());
    }
    if payload_has != manifest_has {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} field `{field}` mismatch with scope manifest: \
                 payload={} manifest={}",
                if payload_has { "present" } else { "absent" },
                if manifest_has { "present" } else { "absent" }
            ),
            ..Default::default()
        });
    }
    if payload != manifest {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} field `{field}` disagrees with scope manifest: payload={payload} manifest={manifest}"
            ),
            ..Default::default()
        });
    }
    Ok(())
}

/// U2 (2026-08-10-002 plan, R5): load the scope manifest JSON and
/// return the top-level object. The manifest shape is the
/// `multi-plan-scope/v1` canonical contract from the upstream plan
/// (decision fields live at the top level, never under a nested
/// `resolution` key).
#[allow(clippy::result_large_err)]
fn read_scope_manifest_object(
    workspace_root: &Path,
    manifest_path: &str,
    topic: &str,
) -> std::result::Result<serde_json::Map<String, serde_json::Value>, ValidationError> {
    let full_path = workspace_root.join(manifest_path);
    let bytes = std::fs::read(&full_path).map_err(|e| ValidationError {
        payload_index: 0,
        field: "scope_manifest_path".to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!(
            "{topic} cannot read scope manifest at {manifest_path}: {e}"
        ),
        ..Default::default()
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        ValidationError {
            payload_index: 0,
            field: "scope_manifest_path".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} scope manifest at {manifest_path} is not valid JSON: {e}"
            ),
            ..Default::default()
        }
    })?;
    match value {
        serde_json::Value::Object(map) => Ok(map),
        other => Err(ValidationError {
            payload_index: 0,
            field: "scope_manifest_path".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} scope manifest at {manifest_path} must be a JSON object; got {other}"
            ),
            ..Default::default()
        }),
    }
}

/// U2 (2026-08-10-002 plan, R5/D8): typed equality between the
/// `postmerge.changemap.ready` payload and the multi-plan-scope/v1
/// canonical manifest. The guard rejects with `scope_handoff_inconsistent`
/// naming the first mismatched field.
#[allow(clippy::result_large_err)]
fn check_postmerge_payload_manifest_consistency(
    obj: &serde_json::Map<String, serde_json::Value>,
    manifest: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), ValidationError> {
    let topic = "postmerge.changemap.ready";
    for field in [
        "scope_status",
        "overall_confidence",
        "critical_unknown_count",
        "scope_base_sha",
        "scope_source",
        "proceed",
    ] {
        let payload_val = required_decision_field(obj, topic, field)?;
        let manifest_val = required_manifest_decision_field(manifest, topic, field)?;
        assert_payload_manifest_field_equal(topic, field, &payload_val, &manifest_val)?;
    }
    Ok(())
}

/// U2 (2026-08-10-002 plan, R5/D8): typed equality between the
/// `redteam.plan.resolved` payload and the multi-plan-scope/v1
/// canonical manifest.
#[allow(clippy::result_large_err)]
fn check_redteam_payload_manifest_consistency(
    obj: &serde_json::Map<String, serde_json::Value>,
    manifest: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), ValidationError> {
    let topic = "redteam.plan.resolved";
    for field in [
        "scope_status",
        "overall_confidence",
        "critical_unknown_count",
        "scope_base_sha",
        "resolved_count",
        "coverage",
        "boundary_conflict",
    ] {
        let payload_val = required_decision_field(obj, topic, field)?;
        let manifest_val = required_manifest_decision_field(manifest, topic, field)?;
        assert_payload_manifest_field_equal(topic, field, &payload_val, &manifest_val)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn required_decision_field(
    payload: &serde_json::Map<String, serde_json::Value>,
    topic: &str,
    field: &str,
) -> std::result::Result<serde_json::Value, ValidationError> {
    payload.get(field).cloned().ok_or_else(|| ValidationError {
        payload_index: 0,
        field: field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{topic} requires decision field {field}"),
        ..Default::default()
    })
}

#[allow(clippy::result_large_err)]
fn required_manifest_decision_field(
    manifest: &serde_json::Map<String, serde_json::Value>,
    topic: &str,
    field: &str,
) -> std::result::Result<serde_json::Value, ValidationError> {
    manifest
        .get(field)
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or_else(|| ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} scope manifest is missing decision field {field}"),
            ..Default::default()
        })
}

/// and ambiguous require `proceed: false`.
#[allow(clippy::result_large_err)]
fn check_postmerge_resolved_thresholds(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), ValidationError> {
    let topic = "postmerge.changemap.ready";
    let scope_status = obj
        .get("scope_status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if scope_status == "resolved" {
        // U1 (R1/R2/C1): typed non-negative integer reads reject
        // negative `critical_unknown_count: -1` and string-encoded
        // `"0"` instead of silently coercing to 0.
        let overall_confidence =
            typed_threshold_u64(obj, "overall_confidence", topic, true, 0)?;
        if overall_confidence < 90 {
            return Err(ValidationError {
                payload_index: 0,
                field: "overall_confidence".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires overall_confidence >= 90 when scope_status=resolved; got {overall_confidence}"
                ),
                ..Default::default()
            });
        }
        let critical_unknown_count =
            typed_threshold_u64(obj, "critical_unknown_count", topic, true, 0)?;
        if critical_unknown_count != 0 {
            return Err(ValidationError {
                payload_index: 0,
                field: "critical_unknown_count".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires critical_unknown_count == 0 when scope_status=resolved; got {critical_unknown_count}"
                ),
                ..Default::default()
            });
        }
        // U2 (R3/A1): explicit-bool reader — string `"false"` is
        // rejected (was silently accepted by `as_bool() == Some(false)`).
        if let Some(false) = typed_required_bool(obj, "proceed", topic)? {
            return Err(ValidationError {
                payload_index: 0,
                field: "proceed".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires proceed=true when scope_status=resolved; got proceed=false"
                ),
                ..Default::default()
            });
        }
    } else if scope_status == "blocked" || scope_status == "ambiguous" {
        if let Some(true) = typed_required_bool(obj, "proceed", topic)? {
            return Err(ValidationError {
                payload_index: 0,
                field: "proceed".to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires proceed=false when scope_status={scope_status}; got proceed=true"
                ),
                ..Default::default()
            });
        }
    }
    Ok(())
}

/// U2 (2026-08-10-002 plan, R1/R2/D4): resolved-status threshold checks
/// for `redteam.plan.resolved`. Resolved requires `overall_confidence
/// >= 90`, `critical_unknown_count == 0`, `resolved_count >= 1`,
/// `coverage >= 90`, and `boundary_conflict != true`.
#[allow(clippy::result_large_err)]
fn check_redteam_resolved_thresholds(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), ValidationError> {
    let topic = "redteam.plan.resolved";
    let scope_status = obj
        .get("scope_status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if scope_status != "resolved" {
        return Ok(());
    }
    // U1 (R1/R2/C1/A1): typed non-negative integer reads reject
    // negative integers and string-encoded values that the previous
    // `as_u64().unwrap_or(0)` pattern silently coerced to 0.
    let overall_confidence = typed_threshold_u64(obj, "overall_confidence", topic, true, 0)?;
    if overall_confidence < 90 {
        return Err(ValidationError {
            payload_index: 0,
            field: "overall_confidence".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} requires overall_confidence >= 90 when scope_status=resolved; got {overall_confidence}"
            ),
            ..Default::default()
        });
    }
    let critical_unknown_count =
        typed_threshold_u64(obj, "critical_unknown_count", topic, true, 0)?;
    if critical_unknown_count != 0 {
        return Err(ValidationError {
            payload_index: 0,
            field: "critical_unknown_count".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} requires critical_unknown_count == 0 when scope_status=resolved; got {critical_unknown_count}"
            ),
            ..Default::default()
        });
    }
    let resolved_count = typed_threshold_u64(obj, "resolved_count", topic, true, 0)?;
    if resolved_count == 0 {
        return Err(ValidationError {
            payload_index: 0,
            field: "resolved_count".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} requires resolved_count >= 1 when scope_status=resolved; got 0"
            ),
            ..Default::default()
        });
    }
    let coverage = typed_threshold_u64(obj, "coverage", topic, true, 0)?;
    if coverage < 90 {
        return Err(ValidationError {
            payload_index: 0,
            field: "coverage".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} requires coverage >= 90 when scope_status=resolved; got {coverage}"
            ),
            ..Default::default()
        });
    }
    // U2 (R3/A1): explicit-bool reader — `boundary_conflict: "true"`
    // (string) is now rejected instead of silently accepted.
    if let Some(true) = typed_required_bool(obj, "boundary_conflict", topic)? {
        return Err(ValidationError {
            payload_index: 0,
            field: "boundary_conflict".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} does not allow boundary_conflict=true when scope_status=resolved"
            ),
            ..Default::default()
        });
    }
    Ok(())
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
    let scope_status = required_scope_string(obj, "scope_status", "postmerge.changemap.ready")?;
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
    if !matches!(scope_status.as_str(), "resolved" | "ambiguous" | "blocked") {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_status".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "postmerge.changemap.ready scope_status must be one of resolved/ambiguous/blocked; got {scope_status}"
            ),
            ..Default::default()
        });
    }

    validate_scoped_artifact_path(
        workspace_root,
        &manifest_path,
        ".ralph/post-merge/",
        "scope_manifest_path",
    )?;

    // U1 (2026-08-10-002 plan, R4): scope manifests are verified with the
    // canonical self-excluding JSON digest algorithm; producer and guard
    // share `verify_scope_manifest_digest` so the same canonical bytes
    // back the declared `scope_digest`.
    verify_scope_manifest_digest(
        workspace_root,
        &manifest_path,
        &manifest_digest,
        "scope_digest",
    )?;

    // U2 (R5): typed equality between payload fields and the canonical
    // manifest fields (D8 multi-plan-scope/v1). The guard rejects with
    // `scope_handoff_inconsistent` naming the first mismatched field.
    let manifest_obj = read_scope_manifest_object(workspace_root, &manifest_path, "postmerge.changemap.ready")?;
    check_postmerge_payload_manifest_consistency(obj, &manifest_obj)?;

    // U2 (R1/R2/D4): typed threshold checks (overall_confidence >=
    // 90, critical_unknown_count == 0, proceed consistency). These run
    // after digest + equality so manifest mismatch is reported first.
    check_postmerge_resolved_thresholds(obj)?;

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
    let scope_status = required_scope_string(obj, "scope_status", "redteam.plan.resolved")?;
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
    if !matches!(scope_status.as_str(), "resolved" | "ambiguous" | "blocked") {
        return Err(ValidationError {
            payload_index: 0,
            field: "scope_status".to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "redteam.plan.resolved scope_status must be one of resolved/ambiguous/blocked; got {scope_status}"
            ),
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

    // U1 (2026-08-10-002 plan, R4): scope manifest uses the canonical
    // self-excluding JSON digest algorithm so producer/guide and guard
    // stay in lockstep. Patch artifacts remain raw-byte SHA-256 per
    // U1 §13 (patch digest semantics unchanged from upstream plan).
    verify_scope_manifest_digest(
        workspace_root,
        &manifest_path,
        &manifest_digest,
        "scope_digest",
    )?;
    verify_artifact_digest(workspace_root, &patch_path, &patch_digest, "patch_digest")?;

    // U2 (R5): typed equality between payload fields and the canonical
    // manifest fields (D8 multi-plan-scope/v1).
    let manifest_obj = read_scope_manifest_object(workspace_root, &manifest_path, "redteam.plan.resolved")?;
    check_redteam_payload_manifest_consistency(obj, &manifest_obj)?;

    // U2 (R1/R2/D4): typed threshold checks (overall_confidence >=
    // 90, critical_unknown_count == 0, resolved_count >= 1,
    // coverage >= 90, boundary_conflict == false). Runs after digest
    // + equality so manifest mismatch is reported first.
    check_redteam_resolved_thresholds(obj)?;

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

    /// U1 (2026-08-10-002 plan, R4): write a scope manifest with its
    /// declared `scope_digest` field and return the canonical
    /// self-excluding digest (SHA-256 over the canonical JSON bytes
    /// after stripping `scope_digest`). Mirrors the producer side of
    /// the canonicalization contract.
    fn write_scope_manifest(
        root: &std::path::Path,
        relative_path: &str,
        body: &str,
    ) -> (String, PathBuf) {
        // Parse the body as JSON so we can compute the canonical digest
        // over the same bytes the guard will canonicalize. The digest is
        // computed AFTER stripping `scope_digest` so the field's own
        // value does not feed back into the digest (self-exclusion).
        let mut value: serde_json::Value =
            serde_json::from_str(body).expect("valid manifest JSON");
        // Insert a placeholder so the map shape matches the final
        // serialized form (otherwise key ordering or omission could
        // shift the canonical bytes); then strip the placeholder so
        // the digest input is the canonical bytes without `scope_digest`.
        if let Some(object) = value.as_object_mut() {
            // Keep the unit-test manifests aligned with the canonical
            // top-level decision contract. Production manifests must carry
            // these fields; the helper fills omitted values only for the
            // older minimal fixtures used by threshold/error-path tests.
            let defaults = [
                ("scope_status", serde_json::json!("resolved")),
                ("overall_confidence", serde_json::json!(100)),
                ("critical_unknown_count", serde_json::json!(0)),
                (
                    "scope_base_sha",
                    serde_json::json!("abc1234def5678901234567890abcdef12345678"),
                ),
                ("scope_source", serde_json::json!("test")),
                ("proceed", serde_json::json!(true)),
                ("resolved_count", serde_json::json!(1)),
                ("coverage", serde_json::json!(100)),
                ("boundary_conflict", serde_json::json!(false)),
            ];
            for (field, default) in defaults {
                object.entry(field.to_string()).or_insert(default);
            }
            object.insert(
                "scope_digest".to_string(),
                serde_json::Value::String("placeholder".to_string()),
            );
            object.remove("scope_digest");
        }
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        let computed = format!("{:x}", hasher.finalize());

        let abs = root.join(relative_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        // Now write the manifest with the real `scope_digest` value.
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "scope_digest".to_string(),
                serde_json::Value::String(computed.clone()),
            );
        }
        let serialized =
            serde_json::to_string(&value).expect("serialize manifest");
        std::fs::write(&abs, serialized.as_bytes()).expect("write manifest");
        (computed, abs)
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
        // U2: the postmerge payload now carries `proceed: true` so the
        // resolved-status guard accepts a `scope_status=resolved`
        // payload. Tests that want to exercise the inconsistency path
        // override this builder with a custom JSON literal.
        format!(
            r#"{{"scope_manifest_path":"{manifest_path}","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":true}}"#
        )
    }

    fn build_redteam_payload(
        manifest_path: &str,
        manifest_digest: &str,
        patch_path: &str,
        patch_digest: &str,
    ) -> String {
        // U2: the redteam payload now carries resolved-threshold
        // fields (`resolved_count: 1`, `coverage: 100`,
        // `boundary_conflict: false`) so a `scope_status=resolved`
        // payload passes the typed guard.
        format!(
            r#"{{"scope_manifest_path":"{manifest_path}","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","resolved_patch_path":"{patch_path}","patch_digest":"{patch_digest}","resolved_count":1,"coverage":100,"boundary_conflict":false}}"#
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
        // U1 (R4) + U2 (R5): manifest fields match the payload
        // fields exactly (scope_source="post-merge-converge") so the
        // typed equality check accepts.
        let (digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
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
        // U1 (R4) + U2 (R5): manifest carries typed decision fields
        // so the guard's equality check would also fire if the
        // declared digest had matched. We deliberately mismatch the
        // declared digest to exercise the digest path first.
        write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"direct-target"}"#,
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
        // U1 (R4) + U2 (R5): manifest carries the typed decision
        // fields (`scope_status`, `overall_confidence`,
        // `critical_unknown_count`, `scope_base_sha`) and matches the
        // payload fields 1:1.
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        // Patch digest stays as raw SHA-256 (U1 §13 preserves patch
        // semantics).
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
        // Manifest file intentionally absent; the guard must reject the
        // digest field on the missing file rather than letting the
        // patch digest path run.
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
        // U1: manifest uses canonical self-excluding digest so the
        // guard passes the manifest digest branch and exercises the
        // patch digest branch with the wrong declared value.
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
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

    // ──────────────────────────────────────────────────────────────────
    // U1 (2026-08-10-002 plan, R4): canonical self-excluding digest tests
    // for scope manifests. The producer computes the digest over the
    // canonical JSON bytes after stripping `scope_digest`; the guard
    // recomputes the same bytes. The contract pins producer and guard
    // to identical input so `scope_digest` self-mutation does not
    // change the digest input.
    // ──────────────────────────────────────────────────────────────────

    /// Helper for the U1 canonical-digest unit tests: build a scope
    /// manifest with the declared `scope_digest` value, compute the
    /// canonical self-excluding SHA-256, and return both. Lets a test
    /// rewrite the manifest's `scope_digest` field after the fact and
    /// still observe that the canonical bytes are stable.
    fn canonical_scope_digest(body: &str, declared_digest: &str) -> String {
        let mut value: serde_json::Value =
            serde_json::from_str(body).expect("valid manifest JSON");
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "scope_digest".to_string(),
                serde_json::Value::String(declared_digest.to_string()),
            );
        }
        if let Some(object) = value.as_object_mut() {
            object.remove("scope_digest");
        }
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn scope_manifest_canonical_digest_excludes_self() {
        // R4: producer and guard share one canonicalizer. The declared
        // `scope_digest` value is not part of the digest input.
        let body = r#"{"scope_status":"resolved","overall_confidence":90}"#;
        let declared = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let digest = canonical_scope_digest(body, declared);
        // Same body with any other `scope_digest` value yields the same
        // canonical digest.
        let digest_with_other_self = canonical_scope_digest(body, "0".repeat(64).as_str());
        assert_eq!(
            digest, digest_with_other_self,
            "scope_digest self-field must not affect canonical digest input"
        );
        // Sanity: mutating a real field changes the digest.
        let other_body = r#"{"scope_status":"resolved","overall_confidence":91}"#;
        let other_digest = canonical_scope_digest(other_body, declared);
        assert_ne!(
            digest, other_digest,
            "content mutation must change the canonical digest"
        );
    }

    #[test]
    fn scope_manifest_canonical_digest_handles_field_order() {
        // serde_json serializes object maps with stable key ordering,
        // so the producer-written order does not matter for the
        // canonical digest.
        let body_a = r#"{"alpha":1,"beta":2,"scope_status":"resolved"}"#;
        let body_b = r#"{"beta":2,"alpha":1,"scope_status":"resolved"}"#;
        let digest_a = canonical_scope_digest(body_a, "x");
        let digest_b = canonical_scope_digest(body_b, "x");
        assert_eq!(
            digest_a, digest_b,
            "field order in the source body must not change the canonical digest"
        );
    }

    #[test]
    fn verify_scope_manifest_digest_accepts_canonical_match() {
        // Direct helper round-trip: write a manifest with the canonical
        // digest embedded, then verify it accepts.
        let root = tempfile::tempdir().expect("tempdir");
        let body = r#"{"scope_status":"resolved","overall_confidence":90}"#;
        let (digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            body,
        );
        let result = verify_scope_manifest_digest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            &digest,
            "scope_digest",
        );
        assert!(result.is_ok(), "canonical digest match must accept: {result:?}");
    }

    #[test]
    fn verify_scope_manifest_digest_rejects_tampered_content() {
        // Mutate a business field after computing the digest and confirm
        // the verifier rejects.
        let root = tempfile::tempdir().expect("tempdir");
        let body = r#"{"scope_status":"resolved","overall_confidence":90}"#;
        let (digest, abs_path) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            body,
        );
        // Tamper with the business field but leave `scope_digest` intact.
        let tampered = r#"{"scope_status":"resolved","overall_confidence":100}"#;
        let mut value: serde_json::Value =
            serde_json::from_str(tampered).expect("valid JSON");
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "scope_digest".to_string(),
                serde_json::Value::String(digest.clone()),
            );
        }
        std::fs::write(
            &abs_path,
            serde_json::to_string(&value).expect("serialize").as_bytes(),
        )
        .expect("rewrite manifest");
        let result = verify_scope_manifest_digest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            &digest,
            "scope_digest",
        );
        let err = result.expect_err("tampered manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("scope_digest"));
    }

    #[test]
    fn verify_scope_manifest_digest_rejects_malformed_json() {
        // Manifest that is not parseable JSON must reject without
        // panicking on the unwrap-style serialization path.
        let root = tempfile::tempdir().expect("tempdir");
        let abs = root.path().join(".ralph/post-merge/scope-manifest.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("create parent");
        std::fs::write(&abs, b"this is not JSON").expect("write manifest");
        let result = verify_scope_manifest_digest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "scope_digest",
        );
        let err = result.expect_err("malformed JSON must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("not valid JSON"));
    }

    #[test]
    fn verify_scope_manifest_digest_rejects_missing_file() {
        // U5 (R5/A2): the verifier now threads a canonical PathBuf
        // from `validate_scoped_artifact_path`, so the missing-file
        // path is rejected at the canonicalize step (the file
        // doesn't exist on disk yet) rather than at the read step.
        // The error reason_code stays the same; the message now
        // names the validator failure instead of the read failure.
        let root = tempfile::tempdir().expect("tempdir");
        let result = verify_scope_manifest_digest(
            root.path(),
            ".ralph/post-merge/missing.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "scope_digest",
        );
        let err = result.expect_err("missing manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(
            err.message.contains("does not exist") || err.message.contains("could not read"),
            "error must surface either canonicalize or read failure: {}",
            err.message
        );
    }

    #[test]
    fn verify_scope_manifest_digest_accepts_non_object() {
        // Scope manifest that parses to a JSON array (not object) is
        // structurally invalid: there is no `scope_digest` to strip, and
        // the producer contract requires an object. We accept the
        // canonicalization as long as the digest match succeeds — the
        // canonical form is still well-defined and the verifier does
        // not need to enforce object shape here (object-shape is owned
        // by the typed guard in U2). This test pins the current
        // verifier contract: non-object manifests with matching digest
        // are accepted at the digest layer.
        let root = tempfile::tempdir().expect("tempdir");
        let abs = root.path().join(".ralph/post-merge/scope-manifest.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("create parent");
        std::fs::write(&abs, b"[1,2,3]").expect("write manifest");
        // For a non-object, the canonical bytes are the array bytes
        // with a trailing newline.
        let mut hasher = Sha256::new();
        hasher.update(b"[1,2,3]\n");
        let digest = format!("{:x}", hasher.finalize());
        let result = verify_scope_manifest_digest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            &digest,
            "scope_digest",
        );
        assert!(
            result.is_ok(),
            "non-object manifest with matching canonical digest must accept: {result:?}"
        );
    }

    #[test]
    fn patch_artifact_digest_still_uses_raw_bytes() {
        // U1 §13: patch digest semantics are NOT changed. Verify by
        // writing a patch with arbitrary bytes and confirming the raw
        // SHA-256 is the verified value (the canonical path strips
        // nothing and re-serializes the same bytes).
        let root = tempfile::tempdir().expect("tempdir");
        let abs = root.path().join(".ralph/red-team/resolved-patch.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("create parent");
        let body = br#"{"patches":["a","b"]}"#;
        std::fs::write(&abs, body).expect("write patch");
        let mut hasher = Sha256::new();
        hasher.update(body);
        let raw_digest = format!("{:x}", hasher.finalize());
        let result = verify_artifact_digest(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            &raw_digest,
            "patch_digest",
        );
        assert!(
            result.is_ok(),
            "raw-byte patch digest must still accept: {result:?}"
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // U2 (2026-08-10-002 plan, R5/D4): typed equality + threshold
    // checks. Each test exercises the typed guard's reject path on a
    // real manifest under the scope topic; the digest path is kept
    // green by writing the canonical manifest body.
    // ──────────────────────────────────────────────────────────────────

    fn build_redteam_payload_full(
        manifest_path: &str,
        manifest_digest: &str,
        patch_path: &str,
        patch_digest: &str,
        extra: &[(&str, serde_json::Value)],
    ) -> String {
        // Use a BTreeMap so the JSON object key order is sorted (the
        // guard parses the payload string as a JSON object; an array
        // would short-circuit at the expect_scope_payload_object guard
        // rather than exercising the typed decision checks).
        let mut fields: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::new();
        fields.insert(
            "scope_manifest_path".to_string(),
            serde_json::Value::String(manifest_path.to_string()),
        );
        fields.insert(
            "scope_digest".to_string(),
            serde_json::Value::String(manifest_digest.to_string()),
        );
        fields.insert(
            "scope_status".to_string(),
            serde_json::Value::String("resolved".to_string()),
        );
        fields.insert(
            "overall_confidence".to_string(),
            serde_json::Value::Number(serde_json::Number::from(100u64)),
        );
        fields.insert(
            "critical_unknown_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(0u64)),
        );
        fields.insert(
            "scope_base_sha".to_string(),
            serde_json::Value::String("abc1234def5678901234567890abcdef12345678".to_string()),
        );
        fields.insert(
            "resolved_patch_path".to_string(),
            serde_json::Value::String(patch_path.to_string()),
        );
        fields.insert(
            "patch_digest".to_string(),
            serde_json::Value::String(patch_digest.to_string()),
        );
        fields.insert(
            "resolved_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        );
        fields.insert(
            "coverage".to_string(),
            serde_json::Value::Number(serde_json::Number::from(100u64)),
        );
        fields.insert("boundary_conflict".to_string(), serde_json::Value::Bool(false));
        for (k, v) in extra {
            fields.insert(k.to_string(), v.clone());
        }
        serde_json::to_string(&fields).expect("serialize payload")
    }

    #[test]
    fn redteam_resolved_threshold_rejects_low_confidence() {
        // U2 (R2): resolved + overall_confidence=89 must reject via the
        // typed threshold guard (not a payload_consistency rule).
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("overall_confidence", serde_json::Value::Number(serde_json::Number::from(89u64)))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("low confidence resolved must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("overall_confidence"));
    }

    #[test]
    fn redteam_resolved_threshold_rejects_critical_unknown() {
        // U2 (R2): resolved + critical_unknown_count > 0 must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("critical_unknown_count", serde_json::Value::Number(serde_json::Number::from(1u64)))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("critical unknown resolved must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("critical_unknown_count"));
    }

    #[test]
    fn redteam_resolved_threshold_rejects_zero_resolved_count() {
        // U2 (R2): resolved + resolved_count=0 must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("resolved_count", serde_json::Value::Number(serde_json::Number::from(0u64)))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("zero resolved_count must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("resolved_count"));
    }

    #[test]
    fn redteam_resolved_threshold_rejects_low_coverage() {
        // U2 (R2): resolved + coverage<90 must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("coverage", serde_json::Value::Number(serde_json::Number::from(89u64)))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("low coverage resolved must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("coverage"));
    }

    #[test]
    fn redteam_resolved_threshold_rejects_boundary_conflict() {
        // U2 (R2): resolved + boundary_conflict=true must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("boundary_conflict", serde_json::Value::Bool(true))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("boundary_conflict=true must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("boundary_conflict"));
    }

    #[test]
    fn redteam_payload_manifest_mismatch_rejects() {
        // U2 (R5): payload disagrees with manifest on scope_status must
        // reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        // Payload declares scope_status=blocked while manifest says resolved.
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[("scope_status", serde_json::Value::String("blocked".to_string()))],
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("payload/manifest mismatch must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("scope_status"));
    }

    #[test]
    fn redteam_decision_field_manifest_mismatch_rejects() {
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","resolved_count":1,"coverage":100,"boundary_conflict":false}"#,
        );
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload_full(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
            &[(
                "coverage",
                serde_json::Value::Number(serde_json::Number::from(99u64)),
            )],
        );
        let err = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path())
            .expect_err("decision field mismatch must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("coverage"));
    }

    #[test]
    fn redteam_manifest_not_json_object_rejects() {
        // U2 (R5/D8): a scope manifest that is not a JSON object
        // fails the typed guard (no top-level fields to compare). This
        // is the fail-close branch D8 forbids fallback for.
        let root = tempfile::tempdir().expect("tempdir");
        let abs = root.path().join(".ralph/red-team/scope-manifest.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("create parent");
        let manifest_bytes: Vec<u8> = {
            let mut v = b"[1,2,3]".to_vec();
            v.push(b'\n');
            v
        };
        std::fs::write(&abs, &manifest_bytes).expect("write manifest");
        // Compute a digest that matches the canonical bytes (raw bytes
        // plus trailing newline) so the digest path passes; the typed
        // equality check then rejects the array shape.
        let mut hasher = Sha256::new();
        hasher.update(&manifest_bytes);
        let digest = format!("{:x}", hasher.finalize());
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/red-team/scope-manifest.json","scope_digest":"{digest}","scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","resolved_patch_path":".ralph/red-team/resolved-patch.json","patch_digest":"{patch_digest}","resolved_count":1,"coverage":100,"boundary_conflict":false}}"#
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("non-object manifest must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("JSON object"));
    }

    #[test]
    fn postmerge_resolved_threshold_rejects_low_confidence() {
        // U2 (R2): postmerge resolved + overall_confidence=89 must
        // reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/post-merge/scope-manifest.json","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":89,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":true}}"#
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("low confidence resolved must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("overall_confidence"));
    }

    #[test]
    fn postmerge_resolved_threshold_rejects_critical_unknown() {
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/post-merge/scope-manifest.json","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":90,"critical_unknown_count":1,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":true}}"#
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("critical unknown resolved must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("critical_unknown_count"));
    }

    #[test]
    fn postmerge_resolved_rejects_proceed_false() {
        // U2 (D4): resolved + proceed=false must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/post-merge/scope-manifest.json","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":false}}"#
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("resolved+proceed=false must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("proceed"));
    }

    #[test]
    fn postmerge_blocked_rejects_proceed_true() {
        // U2 (D4): blocked/ambiguous + proceed=true must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"blocked","overall_confidence":50,"critical_unknown_count":2,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/post-merge/scope-manifest.json","scope_digest":"{manifest_digest}","scope_status":"blocked","overall_confidence":50,"critical_unknown_count":2,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":true}}"#
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("blocked+proceed=true must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("proceed"));
    }

    #[test]
    fn postmerge_payload_manifest_mismatch_rejects() {
        // U2 (R5): postmerge payload disagrees with manifest on
        // overall_confidence must reject.
        let root = tempfile::tempdir().expect("tempdir");
        let (manifest_digest, _) = write_scope_manifest(
            root.path(),
            ".ralph/post-merge/scope-manifest.json",
            r#"{"scope_status":"resolved","overall_confidence":90,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge"}"#,
        );
        let payload = format!(
            r#"{{"scope_manifest_path":".ralph/post-merge/scope-manifest.json","scope_digest":"{manifest_digest}","scope_status":"resolved","overall_confidence":100,"critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678","scope_source":"post-merge-converge","proceed":true}}"#
        );
        let result = check_scope_handoff_guard("postmerge.changemap.ready", &payload, root.path());
        let err = result.expect_err("payload/manifest mismatch must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("overall_confidence"));
    }

    #[test]
    fn redteam_manifest_missing_field_payload_present_rejects() {
        // U2 (R5): payload carries a field the manifest does not. The
        // typed equality guard treats present/absent as a mismatch.
        let root = tempfile::tempdir().expect("tempdir");
        // Manifest does NOT carry overall_confidence.
        let (_, manifest_path) = write_scope_manifest(
            root.path(),
            ".ralph/red-team/scope-manifest.json",
            r#"{"scope_status":"resolved","critical_unknown_count":0,"scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#,
        );
        // The shared fixture helper fills the current contract defaults for
        // legacy tests. Remove this field again to exercise the fail-closed
        // missing-manifest-field branch, then refresh the self-excluding
        // digest exactly as the production verifier does.
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
                .expect("manifest JSON");
        let object = manifest.as_object_mut().expect("manifest object");
        object.remove("overall_confidence");
        object.remove("scope_digest");
        let mut canonical = serde_json::to_vec(&manifest).expect("canonical manifest");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        let manifest_digest = format!("{:x}", hasher.finalize());
        manifest
            .as_object_mut()
            .expect("manifest object")
            .insert(
                "scope_digest".to_string(),
                serde_json::Value::String(manifest_digest.clone()),
            );
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("write manifest JSON"),
        )
        .expect("write manifest");
        let (patch_digest, _) = write_artifact(
            root.path(),
            ".ralph/red-team/resolved-patch.json",
            br#"{"patches":["a"]}"#,
        );
        let payload = build_redteam_payload(
            ".ralph/red-team/scope-manifest.json",
            &manifest_digest,
            ".ralph/red-team/resolved-patch.json",
            &patch_digest,
        );
        let result = check_scope_handoff_guard("redteam.plan.resolved", &payload, root.path());
        let err = result.expect_err("manifest-missing-field mismatch must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("overall_confidence"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // 2026-08-10-002 fix-plan Unit 1 (R1/R2/C1/TC1) + Unit 2 (R3/A1)
    // + Unit 2 (R4/A3 input-validation leg) regression tests.
    //
    // The tests below are pure helper-level — they bypass the on-disk
    // artifact path required by `check_scope_handoff_guard` and assert
    // the typed threshold + typed bool + bounded string readers
    // directly. This isolates the regression to the reader surface
    // (the previous `as_u64().unwrap_or(0)` coerce site) without
    // having to stage a real manifest + patch + canonical digest.
    // ──────────────────────────────────────────────────────────────────────

    /// TC1 (plan 2026-08-10-002): the typed threshold reader MUST
    /// reject `critical_unknown_count: -1` with a `scope_handoff_inconsistent`
    /// error whose message names the field and the offending value.
    /// The previous `as_u64().unwrap_or(0)` pattern silently coerced
    /// this to `0` and passed the gate.
    #[test]
    fn typed_threshold_rejects_negative_critical_unknown_count() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "critical_unknown_count".to_string(),
            serde_json::Value::Number((-1).into()),
        );
        let err = typed_threshold_u64(
            &obj,
            "critical_unknown_count",
            "redteam.plan.resolved",
            true,
            0,
        )
        .expect_err("negative integer must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert_eq!(err.field, "critical_unknown_count");
        assert!(
            err.message.contains("critical_unknown_count"),
            "message must name field: {}",
            err.message
        );
        assert!(
            err.message.contains("-1"),
            "message must surface actual value, not 0: {}",
            err.message
        );
    }

    /// TC1 boundary: `critical_unknown_count: 0` + `scope_status: resolved`
    /// must still pass (the happy path the previous code accidentally
    /// passed; the fix must not regress it).
    #[test]
    fn typed_threshold_accepts_zero_critical_unknown_count() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "critical_unknown_count".to_string(),
            serde_json::Value::Number(0.into()),
        );
        let result = typed_threshold_u64(
            &obj,
            "critical_unknown_count",
            "redteam.plan.resolved",
            true,
            0,
        );
        assert_eq!(result, Ok(0));
    }

    /// TC1 type guard: `critical_unknown_count: "0"` (string) must be
    /// rejected. The previous `as_u64()` returned `None` for strings
    /// and `unwrap_or(0)` then coerced to `0`; the typed reader
    /// surfaces the type mismatch instead of silently passing.
    #[test]
    fn typed_threshold_rejects_string_zero() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "critical_unknown_count".to_string(),
            serde_json::Value::String("0".to_string()),
        );
        let err = typed_threshold_u64(
            &obj,
            "critical_unknown_count",
            "redteam.plan.resolved",
            true,
            0,
        )
        .expect_err("string-encoded integer must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(
            err.message.contains("string"),
            "message must surface the actual type: {}",
            err.message
        );
    }

    /// U2 (R3/A1): `proceed: "true"` (string) on `postmerge.changemap.ready`
    /// with `scope_status: blocked` must reject. The previous
    /// `as_bool() == Some(true)` returned `Some(false)` for strings
    /// (silently wrong), so the gate stayed open.
    #[test]
    fn typed_required_bool_rejects_string_proceed_true() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "proceed".to_string(),
            serde_json::Value::String("true".to_string()),
        );
        let err = typed_required_bool(&obj, "proceed", "postmerge.changemap.ready")
            .expect_err("string-encoded bool must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert_eq!(err.field, "proceed");
        assert!(
            err.message.contains("bool"),
            "message must explain expected type: {}",
            err.message
        );
    }

    /// U2 (R3/A1): `proceed: true` (actual bool) on the same payload
    /// must still surface the type reader as `Ok(Some(true))` so the
    /// caller can keep its `if let Some(true) = ...` arms.
    #[test]
    fn typed_required_bool_accepts_real_bool_proceed_true() {
        let mut obj = serde_json::Map::new();
        obj.insert("proceed".to_string(), serde_json::Value::Bool(true));
        let result = typed_required_bool(&obj, "proceed", "postmerge.changemap.ready");
        assert_eq!(result, Ok(Some(true)));
    }

    /// U2 (R4/A3): `required_scope_string` (now backed by
    /// `bounded_scope_string` with a 256-char cap) must reject a
    /// 257-character path. The unbounded variant silently accepted
    /// 4 KiB+ UTF-8 strings as long as lexical + canonicalize
    /// passed.
    #[test]
    fn bounded_scope_string_rejects_oversize_path() {
        let mut obj = serde_json::Map::new();
        let oversize = "a".repeat(257);
        obj.insert(
            "scope_manifest_path".to_string(),
            serde_json::Value::String(oversize.clone()),
        );
        let err = bounded_scope_string(&obj, "scope_manifest_path", "redteam.plan.resolved", 256)
            .expect_err("oversize string must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert_eq!(err.field, "scope_manifest_path");
        assert!(
            err.message.contains("256"),
            "message must surface the limit: {}",
            err.message
        );
    }

    /// U2 (R4/A3): embedded control characters (`\n`, `\t`, etc.) in
    /// a scope handoff string must reject. The unbounded variant
    /// silently passed `manifest_path: "foo\nbar"` through.
    #[test]
    fn bounded_scope_string_rejects_control_chars() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "scope_manifest_path".to_string(),
            serde_json::Value::String("foo\nbar".to_string()),
        );
        let err = bounded_scope_string(&obj, "scope_manifest_path", "redteam.plan.resolved", 256)
            .expect_err("control char must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("control"));
    }

    /// U2 (R3/A1) + scope_status=resolved + proceed=true: the typed
    /// bool reader must surface `Ok(Some(true))` so the threshold
    /// guard can keep its `if let Some(true)` match arm without a
    /// panic. This pins the helper's positive path on a real bool.
    #[test]
    fn typed_required_bool_missing_field_returns_none() {
        let obj = serde_json::Map::new();
        let result = typed_required_bool(&obj, "proceed", "redteam.plan.resolved");
        assert_eq!(result, Ok(None));
    }

    // ──────────────────────────────────────────────────────────────────────
    // 2026-08-10-002 fix-plan Unit 5 (R5/A2) + Unit 6 (R9/M1/A4)
    // regression tests.
    //
    // U5 closes the validator-to-verifier TOCTOU window by threading
    // the canonical PathBuf into the parameterised helper. U6 collapses
    // the two near-identical verifier bodies (`boundary_digest` /
    // `scope_digest`) into one helper that takes the excluded field
    // name as a parameter. The tests below exercise both helpers
    // against the same temp manifest to prove the parameterised
    // verifier stays behaviour-identical to the old two helpers.
    // ──────────────────────────────────────────────────────────────────────

    /// U6 (R9): the parameterised helper accepts
    /// `excluded_field = "scope_digest"` and matches the declared
    /// digest for a manifest where the self-referential field was
    /// stripped before encoding (canonical self-excluding).
    #[test]
    fn verify_canonical_json_digest_excluding_accepts_scope_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        // Build a manifest with a self-referential scope_digest
        // placeholder, compute the canonical digest over the
        // stripped bytes, then rewrite the file with the real
        // digest so the verifier's canonicalization step produces
        // the same bytes.
        let body = r#"{"scope_status":"resolved","scope_base_sha":"abc1234def5678901234567890abcdef12345678"}"#;
        let mut value: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        if let Some(object) = value.as_object_mut() {
            object.insert("scope_digest".to_string(), serde_json::json!("placeholder"));
            object.remove("scope_digest");
        }
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(&canonical);
        let declared = format!("{:x}", hasher.finalize());
        if let Some(object) = value.as_object_mut() {
            object.insert("scope_digest".to_string(), serde_json::json!(declared.clone()));
        }
        let serialized = serde_json::to_string(&value).expect("serialize");
        let abs = root.path().join(".ralph/post-merge/manifest.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir");
        std::fs::write(&abs, serialized.as_bytes()).expect("write manifest");
        let canonical_path = std::fs::canonicalize(&abs).expect("canonicalize");
        let result = verify_canonical_json_digest_excluding(
            &canonical_path,
            &declared,
            "scope_digest",
            "scope_digest",
        );
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    /// U6 (R9): the parameterised helper accepts
    /// `excluded_field = "boundary_digest"` and matches the declared
    /// digest for a merge-boundary-style manifest.
    #[test]
    fn verify_canonical_json_digest_excluding_accepts_boundary_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        let body = r#"{"merge_boundary_status":"complete","merge_integration_sha":"deadbeef1234567890abcdef1234567890abcdef12"}"#;
        let mut value: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        if let Some(object) = value.as_object_mut() {
            object.insert("boundary_digest".to_string(), serde_json::json!("placeholder"));
            object.remove("boundary_digest");
        }
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(&canonical);
        let declared = format!("{:x}", hasher.finalize());
        if let Some(object) = value.as_object_mut() {
            object.insert("boundary_digest".to_string(), serde_json::json!(declared.clone()));
        }
        let serialized = serde_json::to_string(&value).expect("serialize");
        let abs = root.path().join(".ralph/merge/boundary.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir");
        std::fs::write(&abs, serialized.as_bytes()).expect("write boundary");
        let canonical_path = std::fs::canonicalize(&abs).expect("canonicalize");
        let result = verify_canonical_json_digest_excluding(
            &canonical_path,
            &declared,
            "boundary_digest",
            "boundary_digest",
        );
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    /// U6 (R9): tampering with a non-excluded field changes the
    /// canonical bytes and the helper must reject with the same
    /// digest-mismatch error shape regardless of which excluded
    /// field is in play.
    #[test]
    fn verify_canonical_json_digest_excluding_rejects_tampered_field() {
        let root = tempfile::tempdir().expect("tempdir");
        let body = r#"{"scope_status":"resolved","scope_base_sha":"abc1234def5678901234567890abcdef12345678","resolved_patch_path":".ralph/red-team/x.json"}"#;
        let mut value: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        if let Some(object) = value.as_object_mut() {
            object.insert("scope_digest".to_string(), serde_json::json!("placeholder"));
            object.remove("scope_digest");
        }
        let mut canonical = serde_json::to_vec(&value).expect("canonical JSON");
        canonical.push(b'\n');
        let mut hasher = Sha256::new();
        hasher.update(&canonical);
        let declared = format!("{:x}", hasher.finalize());
        // Tamper with a non-excluded field after writing the digest
        // so the recomputed digest differs.
        if let Some(object) = value.as_object_mut() {
            object.insert("scope_digest".to_string(), serde_json::json!(declared.clone()));
            object.insert(
                "resolved_patch_path".to_string(),
                serde_json::json!("tampered.json"),
            );
        }
        let serialized = serde_json::to_string(&value).expect("serialize");
        let abs = root.path().join(".ralph/post-merge/manifest.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir");
        std::fs::write(&abs, serialized.as_bytes()).expect("write manifest");
        let canonical_path = std::fs::canonicalize(&abs).expect("canonicalize");
        let err = verify_canonical_json_digest_excluding(
            &canonical_path,
            &declared,
            "scope_digest",
            "scope_digest",
        )
        .expect_err("tampered non-excluded field must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("does not match canonical"));
    }

    /// U5 (R5/A2): the verifier no longer accepts a path the
    /// validator would reject. The wrapper calls
    /// `validate_scoped_artifact_path` internally, so a path
    /// outside the allowlist prefix must surface as a
    /// `scope_handoff_inconsistent` error mentioning the prefix.
    #[test]
    fn verify_canonical_json_digest_wrapper_rejects_path_outside_allowlist() {
        let root = tempfile::tempdir().expect("tempdir");
        // Write a boundary manifest under the wrong prefix.
        let abs = root.path().join("not-ralph/boundary.json");
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir");
        std::fs::write(&abs, br#"{"merge_boundary_status":"complete"}"#).expect("write");
        let result = verify_canonical_json_digest(
            root.path(),
            "not-ralph/boundary.json",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "merge_boundary_digest",
        );
        let err = result.expect_err("path outside allowlist must reject");
        assert_eq!(err.reason_code, "scope_handoff_inconsistent");
        assert!(err.message.contains("not-ralph") || err.message.contains("merge"));
    }
}
