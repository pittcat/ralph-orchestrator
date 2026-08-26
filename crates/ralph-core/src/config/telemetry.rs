//! Telemetry and runtime-diagnosis configuration.
//!
//! Controls the runtime-diagnosis layer introduced by the 2026-06-04
//! "Runtime Diagnosis & Recovery Intelligence" plan. This is a purely
//! additive config section: omitting `telemetry:` from `ralph.yml`
//! leaves the orchestrator in a no-op state (no drift detection,
//! no recovery journal, no prompt alerts), and `RALPH_DIAGNOSTICS=1`
//! still works exactly as before.
//!
//! Activation contract (U0 activation matrix; see
//! `crate::diagnostics::DiagnosticsOptions`):
//!
//! | `runtime_diagnosis.enabled` | `write_artifacts` | `RALPH_DIAGNOSTICS=1` | Result |
//! |---|---|---|---|
//! | `false` (default) | `false` (default) | unset | no-op, no session created |
//! | `true` | `false` | unset | in-memory findings only, no on-disk session |
//! | `false` | `true` | unset | warn at validate time; minimal session still created by collector |
//! | `true` | `true` | unset | minimal diagnosis session, U3+ loggers enabled |
//! | any | any | `1` | full diagnostics session (subsumes minimal) |
//!
//! Example configuration:
//! ```yaml
//! telemetry:
//!   runtime_diagnosis:
//!     enabled: true
//!     write_artifacts: true
//!     prompt_injection_enabled: false
//!     max_prompt_findings: 5
//!     max_prompt_chars: 2000
//!     retry_window_iterations: 5
//!     max_repeated_recoveries: 3
//!     artifact_retention: 10
//!     malformed_jsonl_policy: warn
//!     drift:
//!       window_size: 50
//!       field_completeness_threshold: 0.9
//!       coord_join_rate_threshold: 0.6
//!       emit_cadence_sigma: 2.0
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::warning::ConfigWarning;
use crate::diagnostics::DiagnosticsOptions;

/// Top-level telemetry configuration.
///
/// Owns the [`RuntimeDiagnosisConfig`] block. Kept as a small wrapper so
/// the orchestrator can later attach non-diagnosis telemetry (e.g. OTel
/// export) without colliding with the runtime-diagnosis namespace.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Runtime-diagnosis configuration block.
    #[serde(default)]
    pub runtime_diagnosis: RuntimeDiagnosisConfig,

    /// Causal-evidence configuration block (U01a, plan
    /// `2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan`).
    /// Controls whether the orchestrator captures causal-evidence
    /// recordings (decision context, correlation IDs, ring-buffered
    /// traces) and how large the per-session window is.
    #[serde(default)]
    pub causal_evidence: CausalEvidenceConfig,
}

impl TelemetryConfig {
    /// Validate the telemetry config and return errors for hard problems.
    ///
    /// Soft warnings (e.g. `enabled=false && write_artifacts=true`) are
    /// emitted via [`ConfigWarning`] and returned alongside the result;
    /// the caller is expected to surface them through the normal
    /// `RalphConfig::validate` warnings channel. The function never emits
    /// a `tracing::warn!` for soft problems, because at the point this
    /// method runs the validate path may not have access to the global
    /// subscriber.
    pub fn validate(&self) -> Result<Vec<ConfigWarning>, ConfigError> {
        let warnings = self.runtime_diagnosis.validate()?;
        self.causal_evidence.validate()?;
        Ok(warnings)
    }

    /// Bridge helper for U01b's `DiagnosticsOptions.causal_evidence`
    /// field (owned by `crates/ralph-core/src/diagnostics/mod.rs`).
    ///
    /// U01a cannot directly populate `options.causal_evidence` because
    /// that field lives in a different module that U01a is forbidden
    /// from touching. This method exposes the *value* U01a would assign
    /// so the U01a side of the bridge can be unit-tested in isolation
    /// and the U01b side can call it from `DiagnosticsCollector`'s
    /// activation matrix. After integration, `to_diagnostics_options_inner`
    /// will populate `options.causal_evidence` from this value.
    #[must_use]
    pub fn causal_evidence_enabled_for_bridge(&self) -> bool {
        self.causal_evidence.enabled
    }

    /// Bridge telemetry config + `RALPH_DIAGNOSTICS` env into the
    /// [`DiagnosticsOptions`] consumed by U0's [`crate::diagnostics::DiagnosticsCollector`].
    ///
    /// Semantics:
    ///
    /// - `full_diagnostics` = `RALPH_DIAGNOSTICS` env var is exactly `"1"`.
    ///   This matches the historical behavior and is the only path that
    ///   writes the full historical loggers (orchestration, performance,
    ///   errors, hook-runs, agent-output, prompt-log).
    /// - `runtime_diagnosis_artifacts` = `enabled && write_artifacts` and
    ///   `RALPH_DIAGNOSTICS` is *not* `"1"`. The env flag subsumes the
    ///   minimal session, so we don't double-spend a session dir.
    /// - `session_dir` is `None` in U1. U3 (or later) may repurpose this
    ///   to let the CLI pre-allocate the session dir and hand it to the
    ///   collector; for now the collector creates its own timestamped
    ///   directory when activated.
    ///
    /// `workspace` is accepted for forward-compatibility with U3's
    /// `session_dir`-reuse path; the U1 implementation ignores it.
    #[must_use]
    pub fn to_diagnostics_options(&self, workspace: &Path) -> DiagnosticsOptions {
        let _ = workspace; // Reserved for U3 session-dir reuse.
        let full_diagnostics = std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);
        self.to_diagnostics_options_inner(workspace, full_diagnostics)
    }

    /// Pure variant of [`Self::to_diagnostics_options`] that takes the
    /// `full_diagnostics` flag directly, skipping the `RALPH_DIAGNOSTICS`
    /// env read. Useful for unit tests (the workspace is `forbid(unsafe_code)`
    /// so env mutation is not available) and for callers that want to
    /// inject the env-equivalent value from somewhere other than the
    /// process environment.
    #[must_use]
    pub fn to_diagnostics_options_with_full(
        &self,
        workspace: &Path,
        full_diagnostics: bool,
    ) -> DiagnosticsOptions {
        self.to_diagnostics_options_inner(workspace, full_diagnostics)
    }

    fn to_diagnostics_options_inner(
        &self,
        _workspace: &Path,
        full_diagnostics: bool,
    ) -> DiagnosticsOptions {
        // Full diagnostics subsume the minimal session: we don't need to
        // also flip runtime_diagnosis_artifacts because the collector
        // already creates the same directory and the historical loggers
        // cover the diagnosis surface. Avoid enabling both paths to keep
        // `is_enabled()` reporting stable across runs.
        let runtime_diagnosis_artifacts = !full_diagnostics
            && self.runtime_diagnosis.enabled
            && self.runtime_diagnosis.write_artifacts;

        // U01a → U01b bridge seam.
        //
        // The new `causal_evidence: bool` field is owned by U01b on
        // `DiagnosticsOptions` (in `crates/ralph-core/src/diagnostics/
        // mod.rs`, which U01a is forbidden from touching). U01a
        // computes the value via `causal_evidence_enabled_for_bridge`
        // so the integration commit (or the U01b-side matrix wiring
        // paired with this commit) can populate
        // `options.causal_evidence` without re-reading `TelemetryConfig`.
        //
        // Until the field exists in `DiagnosticsOptions` (i.e., until
        // U01b lands), the struct literal below cannot name
        // `causal_evidence` directly without breaking U01a's
        // standalone compile. The local assignment is therefore
        // intentionally shadowed by `let _ =` to suppress the
        // unused-variable warning while keeping the seam discoverable
        // for the integration step.
        let causal_evidence_enabled = self.causal_evidence_enabled_for_bridge();
        let _ = causal_evidence_enabled;

        DiagnosticsOptions {
            full_diagnostics,
            runtime_diagnosis_artifacts,
            trace_only: false,
            session_dir: None,
            workspace_root: None,
            // U01b: causal_evidence lives in `DiagnosticsOptions` so the
            // collector matrix can drive a minimal session off the
            // `telemetry.causal_evidence.enabled` flag (U01a). The U01a
            // commit fills this slot from `self.causal_evidence.enabled`
            // — keeping it on `..Default::default()` here means neither
            // parallel wave-1 commit has to enumerate the field at this
            // call site, and the integrator auto-resolves cleanly because
            // both edits target the same struct literal.
            ..Default::default()
        }
    }
}

/// Runtime-diagnosis configuration block.
///
/// Sits under `telemetry.runtime_diagnosis` in `ralph.yml`. All fields
/// are opt-in: the defaults reproduce the historical "no diagnosis
/// happened" behavior, so a config that omits the entire `telemetry:`
/// block is byte-equivalent to one that supplies every default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeDiagnosisConfig {
    /// Master switch for runtime-diagnosis logic in the orchestrator.
    ///
    /// When `false` (default), the loop runs exactly like before:
    /// no drift window, no recovery responder, no prompt alerts.
    /// When `true`, the orchestrator consults `drift`, `prompt_*`, and
    /// `retry_*` settings to decide what to record and inject.
    #[serde(default)]
    pub enabled: bool,

    /// Whether to spin up a minimal diagnostics session for
    /// runtime-diagnosis artifacts (`recovery.jsonl`, `drift.jsonl`,
    /// `diagnosis-summary.json` — see U3) when `enabled` is true and
    /// `RALPH_DIAGNOSTICS=1` is not set.
    ///
    /// `false` (default) keeps the orchestrator in a logless but
    /// in-memory state — useful for unit tests and for users who only
    /// want prompt alerts without paying for disk I/O.
    #[serde(default)]
    pub write_artifacts: bool,

    /// Whether to inject `## Runtime Diagnosis Alert` blocks into the
    /// next agent prompt when the recovery responder has findings.
    /// Off by default to keep the prompt stable for callers who haven't
    /// opted in to the diagnosis surface.
    #[serde(default)]
    pub prompt_injection_enabled: bool,

    /// Maximum number of findings the responder will fold into a single
    /// prompt alert. Bounds prompt growth even when the drift window
    /// reports many findings.
    #[serde(default = "default_max_prompt_findings")]
    pub max_prompt_findings: usize,

    /// Hard upper bound on the character length of any single prompt
    /// alert block. The responder truncates output to this many chars.
    #[serde(default = "default_max_prompt_chars")]
    pub max_prompt_chars: usize,

    /// Number of past iterations the responder looks back when deciding
    /// whether a recovery has been "repeated".
    #[serde(default = "default_retry_window_iterations")]
    pub retry_window_iterations: usize,

    /// Maximum number of repeated recovery attempts for the same
    /// `retry_key` before the responder escalates to a hard pause or
    /// human guidance. R8 from the plan.
    #[serde(default = "default_max_repeated_recoveries")]
    pub max_repeated_recoveries: usize,

    /// How many diagnosis sessions to keep on disk under
    /// `.ralph/diagnostics/`. Older sessions are pruned on a best-effort
    /// basis by the reporter. `0` is not allowed by [`Self::validate`].
    #[serde(default = "default_artifact_retention")]
    pub artifact_retention: usize,

    /// How to handle malformed lines in the JSONL artifacts the
    /// responder or the drift detector read.
    #[serde(default)]
    pub malformed_jsonl_policy: MalformedJsonlPolicy,

    /// Drift-detector configuration block.
    #[serde(default)]
    pub drift: DriftConfig,
}

impl Default for RuntimeDiagnosisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            write_artifacts: false,
            prompt_injection_enabled: false,
            max_prompt_findings: default_max_prompt_findings(),
            max_prompt_chars: default_max_prompt_chars(),
            retry_window_iterations: default_retry_window_iterations(),
            max_repeated_recoveries: default_max_repeated_recoveries(),
            artifact_retention: default_artifact_retention(),
            malformed_jsonl_policy: MalformedJsonlPolicy::default(),
            drift: DriftConfig::default(),
        }
    }
}

impl RuntimeDiagnosisConfig {
    fn validate(&self) -> Result<Vec<ConfigWarning>, ConfigError> {
        let mut warnings = Vec::new();

        // Soft warning: caller asked for on-disk artifacts but disabled
        // the diagnosis feature entirely. The collector would still
        // create a session dir, so this is a footgun, not a hard error.
        if !self.enabled && self.write_artifacts {
            warnings.push(ConfigWarning::InvalidValue {
                field: "telemetry.runtime_diagnosis.write_artifacts".to_string(),
                message: "write_artifacts=true has no effect when runtime_diagnosis.enabled is false; set enabled=true or remove write_artifacts".to_string(),
            });
        }

        // Hard errors: zero-valued sizing fields are operator mistakes
        // that would silently disable protection.
        if self.max_prompt_findings == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.max_prompt_findings".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        if self.max_prompt_chars == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.max_prompt_chars".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        if self.retry_window_iterations == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.retry_window_iterations".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        if self.max_repeated_recoveries == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.max_repeated_recoveries".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        if self.max_repeated_recoveries > u32::MAX as usize {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.max_repeated_recoveries".to_string(),
                message: format!("must not exceed u32::MAX ({})", u32::MAX),
            });
        }
        if self.artifact_retention == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.artifact_retention".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }

        self.drift.validate()?;
        Ok(warnings)
    }
}

/// Coord join rate evaluation mode.
///
/// The drift detector counts how often a `from_topic` event is followed
/// by a `to_topic` event within its rolling window. Two distinct
/// workflow shapes produce different expected rates:
///
/// - **parallel**: many `from` events, each expected to be followed by a
///   `to` event. Used by wave-mode presets (multiple review dimensions
///   emit `review.dimension.done` and each expects a `to_topic`
///   consumer).
/// - **serial**: the canonical pattern is "the LAST `from` event is
///   followed by exactly ONE `to` event" (e.g. `review.dimension.done`
///   for the 4th dimension triggers `review.dimensions.complete`). The
///   parallel threshold (60%) is unsuitable for serial workflows
///   because the rate is structurally low (1/N where N = number of
///   serial hops). The serial mode uses a "last-joins" semantic that
///   counts how often the last `from` event is followed by a `to`
///   event, which is the operationally meaningful signal.
///
/// 2026-06-23-004 plan U2 KTD-Drift: introduce `CoordJoinMode` so the
/// detector can be calibrated per workflow shape. Default is
/// `Parallel` for backwards compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CoordJoinMode {
    /// Many `from` → many `to` (wave-mode presets).
    #[default]
    Parallel,
    /// Last `from` → one `to` (serial presets like `ce-executor-serial`).
    Serial,
}

/// Drift-detector configuration.
///
/// Sits under `telemetry.runtime_diagnosis.drift`. The thresholds and
/// `window_size` feed the U5 drift signal source. U1 only persists and
/// validates the values; U5 (and U6) consume them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftConfig {
    /// Number of recent events the drift detector keeps in its rolling
    /// window. Larger windows smooth out noise at the cost of memory.
    #[serde(default = "default_drift_window_size")]
    pub window_size: usize,

    /// Required fraction (0.0..=1.0) of events that must include a given
    /// field for it to count as "complete". Below this the detector
    /// raises a `field_completeness` finding.
    #[serde(default = "default_field_completeness_threshold")]
    pub field_completeness_threshold: f64,

    /// Required fraction (0.0..=1.0) of `(from_topic, to_topic)` edges
    /// that must be observed in the window. Below this the detector
    /// raises a `coord_join_rate` finding (R5 from the plan).
    #[serde(default = "default_coord_join_rate_threshold")]
    pub coord_join_rate_threshold: f64,

    /// Coord join evaluation mode (2026-06-23-004 plan U2 KTD-Drift).
    /// Defaults to `parallel` for backwards compatibility; serial
    /// presets can opt in via `telemetry.runtime_diagnosis.drift.coord_join_mode: serial`.
    #[serde(default)]
    pub coord_join_mode: CoordJoinMode,

    /// Sensitivity (in standard deviations) for the emit-cadence
    /// detector (R6). `> 0`. Larger values reduce false positives but
    /// also delay detection of genuine cadence drift.
    #[serde(default = "default_emit_cadence_sigma")]
    pub emit_cadence_sigma: f64,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            window_size: default_drift_window_size(),
            field_completeness_threshold: default_field_completeness_threshold(),
            coord_join_rate_threshold: default_coord_join_rate_threshold(),
            coord_join_mode: CoordJoinMode::default(),
            emit_cadence_sigma: default_emit_cadence_sigma(),
        }
    }
}

impl DriftConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.window_size == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.drift.window_size".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        if !is_unit_interval(self.field_completeness_threshold) {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.drift.field_completeness_threshold".to_string(),
                message: format!(
                    "value {} is outside [0.0, 1.0]",
                    self.field_completeness_threshold
                ),
            });
        }
        if !is_unit_interval(self.coord_join_rate_threshold) {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.drift.coord_join_rate_threshold".to_string(),
                message: format!(
                    "value {} is outside [0.0, 1.0]",
                    self.coord_join_rate_threshold
                ),
            });
        }
        if self.emit_cadence_sigma <= 0.0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.runtime_diagnosis.drift.emit_cadence_sigma".to_string(),
                message: format!("value {} must be greater than 0.0", self.emit_cadence_sigma),
            });
        }
        Ok(())
    }
}

/// Causal-evidence configuration block.
///
/// Sits under `telemetry.causal_evidence` in `ralph.yml`. Controls
/// whether the orchestrator captures causal-evidence recordings
/// (decision context, correlation IDs, ring-buffered traces) and how
/// large the per-session window is. The defaults are opt-in (enabled)
/// so existing `ralph.yml` files capture causal evidence without an
/// explicit configuration migration; operators can disable recording
/// with `telemetry.causal_evidence.enabled: false`.
///
/// The actual `DiagnosticsOptions.causal_evidence` field is owned by
/// U01b (`crates/ralph-core/src/diagnostics/mod.rs`); this struct
/// defines the config-side input only and exposes
/// [`TelemetryConfig::causal_evidence_enabled_for_bridge`] so U01b can
/// pick up the value without `to_diagnostics_options_inner` having to
/// reach into forbidden modules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalEvidenceConfig {
    /// Master switch for causal-evidence recording.
    ///
    /// When `true` (default), the orchestrator captures decision
    /// context and correlation IDs into a per-session ring buffer
    /// (U6 wires the buffer). When `false`, causal-evidence recording
    /// is suppressed and no extra session directory is created.
    #[serde(default = "default_causal_evidence_enabled")]
    pub enabled: bool,

    /// Capacity of the per-session ring buffer that backs causal
    /// evidence (U6). Larger values retain more decision context at
    /// the cost of memory (8 KiB per slot per the plan's risk
    /// mitigation). `0` is rejected by [`Self::validate`].
    #[serde(default = "default_causal_evidence_window_capacity")]
    pub window_capacity: usize,
}

impl Default for CausalEvidenceConfig {
    fn default() -> Self {
        Self {
            enabled: default_causal_evidence_enabled(),
            window_capacity: default_causal_evidence_window_capacity(),
        }
    }
}

impl CausalEvidenceConfig {
    /// Validate the causal-evidence configuration. Mirrors the
    /// zero-rejection style of [`RuntimeDiagnosisConfig::validate`]:
    /// sizing fields below their minimum are a hard error rather than
    /// a silent no-op.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.window_capacity == 0 {
            return Err(ConfigError::TelemetryValidation {
                field: "telemetry.causal_evidence.window_capacity".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }
        Ok(())
    }
}

/// How the orchestrator treats a malformed line in a diagnostics JSONL
/// artifact (`recovery.jsonl`, `drift.jsonl`, etc.).
///
/// `Warn` (default) is the conservative middle ground: the offending
/// line is dropped, a `ConfigWarning` is recorded on the next validate
/// pass, and the responder keeps working. `Skip` drops the line
/// silently. `Error` fails the run, which is rarely what operators
/// want during a long-lived loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MalformedJsonlPolicy {
    /// Drop the line silently.
    Skip,
    /// Drop the line and emit a warning (default).
    #[default]
    Warn,
    /// Fail the run on the first malformed line.
    Error,
}

impl std::fmt::Display for MalformedJsonlPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skip => write!(f, "skip"),
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

// ── Default value helpers ──────────────────────────────────────────────
// `#[serde(default = "fn")]` requires a path; these are tiny and
// keep the field defaults discoverable from a single file.

fn default_max_prompt_findings() -> usize {
    5
}

fn default_max_prompt_chars() -> usize {
    2000
}

fn default_retry_window_iterations() -> usize {
    5
}

fn default_max_repeated_recoveries() -> usize {
    3
}

fn default_artifact_retention() -> usize {
    10
}

fn default_drift_window_size() -> usize {
    50
}

fn default_field_completeness_threshold() -> f64 {
    0.9
}

fn default_coord_join_rate_threshold() -> f64 {
    0.6
}

fn default_emit_cadence_sigma() -> f64 {
    2.0
}

fn default_causal_evidence_enabled() -> bool {
    true
}

fn default_causal_evidence_window_capacity() -> usize {
    200
}

fn is_unit_interval(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── YAML parse tests ────────────────────────────────────────────────

    /// When `telemetry:` is omitted entirely, `TelemetryConfig::default()`
    /// must produce the documented no-op state.
    #[test]
    fn test_default_telemetry_is_noop() {
        let cfg = TelemetryConfig::default();
        assert!(!cfg.runtime_diagnosis.enabled);
        assert!(!cfg.runtime_diagnosis.write_artifacts);
        assert!(!cfg.runtime_diagnosis.prompt_injection_enabled);
        assert_eq!(cfg.runtime_diagnosis.max_prompt_findings, 5);
        assert_eq!(cfg.runtime_diagnosis.max_prompt_chars, 2000);
        assert_eq!(cfg.runtime_diagnosis.retry_window_iterations, 5);
        assert_eq!(cfg.runtime_diagnosis.max_repeated_recoveries, 3);
        assert_eq!(cfg.runtime_diagnosis.artifact_retention, 10);
        assert_eq!(
            cfg.runtime_diagnosis.malformed_jsonl_policy,
            MalformedJsonlPolicy::Warn
        );

        // Drift defaults.
        let drift = &cfg.runtime_diagnosis.drift;
        assert_eq!(drift.window_size, 50);
        assert!((drift.field_completeness_threshold - 0.9).abs() < f64::EPSILON);
        assert!((drift.coord_join_rate_threshold - 0.6).abs() < f64::EPSILON);
        assert!((drift.emit_cadence_sigma - 2.0).abs() < f64::EPSILON);
    }

    /// Omitting `telemetry:` from a YAML config must keep the parsed
    /// `TelemetryConfig` byte-equivalent to `TelemetryConfig::default()`.
    /// This is the non-regression contract for existing `ralph.yml` files.
    #[test]
    fn test_yaml_without_telemetry_section_uses_defaults() {
        let yaml = r"
agent: claude
event_loop:
  completion_promise: DONE
";
        // `RalphConfig` is the natural entry point; if telemetry defaults
        // drift this test will fail first.
        let parsed: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.telemetry, TelemetryConfig::default());
    }

    /// `telemetry.runtime_diagnosis.enabled: true` must round-trip into
    /// the parsed `RuntimeDiagnosisConfig`.
    #[test]
    fn test_yaml_runtime_diagnosis_enabled_parses() {
        // When parsed as a `TelemetryConfig` directly, the YAML root is
        // `runtime_diagnosis` (the `telemetry:` wrapper only appears
        // inside `ralph.yml`).
        let yaml = r"
runtime_diagnosis:
  enabled: true
";
        let cfg: TelemetryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.runtime_diagnosis.enabled);
        // All other fields keep their defaults.
        assert!(!cfg.runtime_diagnosis.write_artifacts);
        assert_eq!(cfg.runtime_diagnosis.max_prompt_findings, 5);
    }

    /// Custom threshold + window values must parse without being
    /// silently coerced to defaults.
    #[test]
    fn test_yaml_drift_field_completeness_threshold_parses() {
        let yaml = r"
runtime_diagnosis:
  enabled: true
  drift:
    window_size: 75
    field_completeness_threshold: 0.85
    coord_join_rate_threshold: 0.45
    emit_cadence_sigma: 2.5
";
        let cfg: TelemetryConfig = serde_yaml::from_str(yaml).unwrap();
        let drift = &cfg.runtime_diagnosis.drift;
        assert_eq!(drift.window_size, 75);
        assert!((drift.field_completeness_threshold - 0.85).abs() < f64::EPSILON);
        assert!((drift.coord_join_rate_threshold - 0.45).abs() < f64::EPSILON);
        assert!((drift.emit_cadence_sigma - 2.5).abs() < f64::EPSILON);
    }

    /// `malformed_jsonl_policy` must accept the three documented values
    /// and reject unknown ones.
    #[test]
    fn test_yaml_malformed_jsonl_policy_parses_all_variants() {
        for (raw, expected) in [
            ("skip", MalformedJsonlPolicy::Skip),
            ("warn", MalformedJsonlPolicy::Warn),
            ("error", MalformedJsonlPolicy::Error),
        ] {
            let yaml = format!("runtime_diagnosis:\n  malformed_jsonl_policy: {raw}\n");
            let cfg: TelemetryConfig = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(cfg.runtime_diagnosis.malformed_jsonl_policy, expected);
        }

        // Unknown value must fail loudly.
        let bad = "runtime_diagnosis:\n  malformed_jsonl_policy: explode\n";
        let result: Result<TelemetryConfig, _> = serde_yaml::from_str(bad);
        assert!(
            result.is_err(),
            "unknown malformed_jsonl_policy variant must be rejected"
        );
    }

    /// All sizing fields can be supplied together and survive a
    /// serialize → deserialize round-trip (catches typos in `rename_all`
    /// or in `default = "fn"`).
    #[test]
    fn test_yaml_full_block_round_trip() {
        let yaml = r"
runtime_diagnosis:
  enabled: true
  write_artifacts: true
  prompt_injection_enabled: true
  max_prompt_findings: 8
  max_prompt_chars: 4096
  retry_window_iterations: 7
  max_repeated_recoveries: 2
  artifact_retention: 5
  malformed_jsonl_policy: error
  drift:
    window_size: 100
    field_completeness_threshold: 0.8
    coord_join_rate_threshold: 0.5
    emit_cadence_sigma: 3.0
";
        let cfg: TelemetryConfig = serde_yaml::from_str(yaml).unwrap();
        // Reserialize and parse again — the parsed value must match.
        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: TelemetryConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg, cfg2);
    }

    // ── to_diagnostics_options tests ───────────────────────────────────

    /// With `RALPH_DIAGNOSTICS=1`, `full_diagnostics` must be true
    /// regardless of the config block (this is the historical
    /// activation knob).
    #[test]
    fn test_to_diagnostics_options_env_overrides_config_for_full_diagnostics() {
        // The workspace sets `forbid(unsafe_code)`, which prevents
        // `std::env::set_var` / `remove_var` (they are unsafe since
        // Rust 1.81). Test the env-equivalent code path through
        // `to_diagnostics_options_with_full` instead, which is a thin
        // wrapper around the same inner function.
        let cfg = TelemetryConfig::default();
        let opts = cfg.to_diagnostics_options_with_full(Path::new("."), true);
        assert!(
            opts.full_diagnostics,
            "full_diagnostics=true must produce full diagnostics"
        );
    }

    /// `write_artifacts=true` with no env flag must turn on the minimal
    /// diagnosis session. The minimal session is the only thing the
    /// caller wanted when they asked for `write_artifacts`.
    #[test]
    fn test_to_diagnostics_options_write_artifacts_enables_minimal_session() {
        let cfg = TelemetryConfig {
            runtime_diagnosis: RuntimeDiagnosisConfig {
                enabled: true,
                write_artifacts: true,
                ..RuntimeDiagnosisConfig::default()
            },
            ..TelemetryConfig::default()
        };
        let opts = cfg.to_diagnostics_options_with_full(Path::new("."), false);
        assert!(!opts.full_diagnostics);
        assert!(
            opts.runtime_diagnosis_artifacts,
            "enabled+write_artifacts must enable minimal diagnosis session"
        );
    }

    /// Default config + no env = no-op `DiagnosticsOptions` (matches
    /// `DiagnosticsOptions::default()`).
    #[test]
    fn test_to_diagnostics_options_default_config_is_disabled() {
        let cfg = TelemetryConfig::default();
        let opts = cfg.to_diagnostics_options_with_full(Path::new("."), false);
        assert!(!opts.full_diagnostics);
        assert!(!opts.runtime_diagnosis_artifacts);
        assert!(opts.session_dir.is_none());
        // Matches the U0 default constructor.
        assert_eq!(opts, DiagnosticsOptions::default());
    }

    /// `write_artifacts=true` while `enabled=false` must NOT silently
    /// turn on the minimal session — the caller hasn't actually opted
    /// into runtime diagnosis. The validator will surface this as a
    /// warning, and the converter must be conservative: it follows the
    /// config without trying to "fix" it.
    #[test]
    fn test_to_diagnostics_options_disabled_with_write_artifacts_is_conservative() {
        let cfg = TelemetryConfig {
            runtime_diagnosis: RuntimeDiagnosisConfig {
                enabled: false,
                write_artifacts: true,
                ..RuntimeDiagnosisConfig::default()
            },
            ..TelemetryConfig::default()
        };
        let opts = cfg.to_diagnostics_options_with_full(Path::new("."), false);
        assert!(!opts.runtime_diagnosis_artifacts);
    }

    // ── validate() tests ───────────────────────────────────────────────

    /// Thresholds outside `[0.0, 1.0]` must return a hard error rather
    /// than silently passing through.
    #[test]
    fn test_validate_rejects_out_of_range_threshold() {
        let cfg = RuntimeDiagnosisConfig {
            drift: DriftConfig {
                field_completeness_threshold: 1.5,
                ..DriftConfig::default()
            },
            ..RuntimeDiagnosisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::TelemetryValidation { ref field, .. } if field.contains("field_completeness_threshold")),
            "expected TelemetryValidation for field_completeness_threshold, got {err:?}"
        );

        let cfg = RuntimeDiagnosisConfig {
            drift: DriftConfig {
                coord_join_rate_threshold: -0.1,
                ..DriftConfig::default()
            },
            ..RuntimeDiagnosisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::TelemetryValidation { ref field, .. } if field.contains("coord_join_rate_threshold")),
            "expected TelemetryValidation for coord_join_rate_threshold, got {err:?}"
        );
    }

    /// `emit_cadence_sigma` must be strictly positive.
    #[test]
    fn test_validate_rejects_non_positive_sigma() {
        for bad in [-1.0_f64, 0.0_f64] {
            let cfg = RuntimeDiagnosisConfig {
                drift: DriftConfig {
                    emit_cadence_sigma: bad,
                    ..DriftConfig::default()
                },
                ..RuntimeDiagnosisConfig::default()
            };
            let err = cfg.validate().unwrap_err();
            assert!(
                matches!(err, ConfigError::TelemetryValidation { ref field, .. } if field.contains("emit_cadence_sigma")),
                "expected TelemetryValidation for emit_cadence_sigma when {bad}, got {err:?}"
            );
        }
    }

    /// `window_size = 0` would silently disable the drift detector; it
    /// must be a hard error.
    #[test]
    fn test_validate_rejects_zero_window_size() {
        let cfg = RuntimeDiagnosisConfig {
            drift: DriftConfig {
                window_size: 0,
                ..DriftConfig::default()
            },
            ..RuntimeDiagnosisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::TelemetryValidation { ref field, .. } if field.contains("window_size")),
            "expected TelemetryValidation for window_size, got {err:?}"
        );
    }

    /// `enabled=false` and `write_artifacts=true` is a configuration
    /// smell but not a structural error. The validator must surface it
    /// as a `ConfigWarning` (soft) and return `Ok`.
    #[test]
    fn test_validate_warns_on_disabled_with_write_artifacts() {
        let cfg = RuntimeDiagnosisConfig {
            enabled: false,
            write_artifacts: true,
            ..RuntimeDiagnosisConfig::default()
        };
        let warnings = cfg.validate().expect("soft warning must not be an Err");
        assert!(
            warnings.iter().any(|w| matches!(w,
                ConfigWarning::InvalidValue { field, .. }
                if field == "telemetry.runtime_diagnosis.write_artifacts"
            )),
            "expected an InvalidValue warning for write_artifacts, got {warnings:?}"
        );
    }

    /// Default values must validate cleanly with zero warnings.
    #[test]
    fn test_validate_default_is_clean() {
        let cfg = RuntimeDiagnosisConfig::default();
        let warnings = cfg.validate().expect("defaults must validate");
        assert!(
            warnings.is_empty(),
            "defaults must not emit warnings, got {warnings:?}"
        );
    }

    /// The env-reading `to_diagnostics_options` must produce the same
    /// output as `to_diagnostics_options_with_full` when called with the
    /// same `full_diagnostics` value that the env reader would derive.
    /// We can't mutate the env under `forbid(unsafe_code)`, so we test
    /// the equivalence by reading whatever the current value is and
    /// comparing both paths.
    #[test]
    fn test_to_diagnostics_options_env_path_matches_injected_path() {
        let cfg = TelemetryConfig::default();
        let from_env = cfg.to_diagnostics_options(Path::new("."));
        let env_value = std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);
        let from_injected = cfg.to_diagnostics_options_with_full(Path::new("."), env_value);
        assert_eq!(from_env, from_injected);
    }

    // ── Causal-evidence (U01a) ─────────────────────────────────────────

    /// `CausalEvidenceConfig::default()` must produce the documented
    /// opt-in state: causal-evidence recording is on by default so the
    /// orchestrator can capture decision context without an explicit
    /// configuration migration, but operators can disable it via
    /// `telemetry.causal_evidence.enabled: false`.
    #[test]
    fn test_causal_evidence_default_is_enabled_with_capacity_200() {
        let cfg = CausalEvidenceConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.window_capacity, 200);
    }

    /// `telemetry:` block without a `causal_evidence:` sub-key must
    /// leave `TelemetryConfig.causal_evidence` at the documented defaults
    /// (`enabled=true, window_capacity=200`). This is the contract that
    /// existing `ralph.yml` files continue to opt into causal-evidence
    /// recording without any explicit configuration.
    #[test]
    fn test_telemetry_default_includes_causal_evidence_defaults() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.causal_evidence, CausalEvidenceConfig::default());
        assert!(cfg.causal_evidence.enabled);
        assert_eq!(cfg.causal_evidence.window_capacity, 200);
    }

    /// Explicit `enabled: false` must override the default-on contract.
    #[test]
    fn test_yaml_causal_evidence_enabled_false_parses() {
        let yaml = r"
causal_evidence:
  enabled: false
  window_capacity: 50
";
        let cfg: TelemetryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.causal_evidence.enabled);
        assert_eq!(cfg.causal_evidence.window_capacity, 50);
    }

    /// A `ralph.yml` that omits the entire `telemetry:` block must
    /// continue to parse byte-equivalent to `TelemetryConfig::default()`.
    /// This guards the existing contract for files that don't know about
    /// `telemetry.causal_evidence` yet.
    #[test]
    fn test_yaml_without_telemetry_section_uses_defaults_with_causal_evidence() {
        let yaml = r"
agent: claude
event_loop:
  completion_promise: DONE
";
        let parsed: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.telemetry, TelemetryConfig::default());
        // Causal-evidence defaults must also be present.
        assert!(parsed.telemetry.causal_evidence.enabled);
        assert_eq!(parsed.telemetry.causal_evidence.window_capacity, 200);
    }

    /// `window_capacity = 0` would silently disable the ring buffer
    /// (U6 will wire the buffer; U01a only validates the field). This
    /// must be a hard error rather than a silent no-op.
    #[test]
    fn test_validate_rejects_zero_window_capacity() {
        let cfg = CausalEvidenceConfig {
            enabled: true,
            window_capacity: 0,
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::TelemetryValidation { ref field, .. }
                if field == "telemetry.causal_evidence.window_capacity"),
            "expected TelemetryValidation for window_capacity=0, got {err:?}"
        );
    }

    /// The `TelemetryConfig::validate` aggregator must propagate the
    /// `CausalEvidenceConfig` validation error rather than swallowing it.
    #[test]
    fn test_telemetry_validate_propagates_causal_evidence_error() {
        let cfg = TelemetryConfig {
            runtime_diagnosis: RuntimeDiagnosisConfig::default(),
            causal_evidence: CausalEvidenceConfig {
                enabled: true,
                window_capacity: 0,
            },
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::TelemetryValidation { ref field, .. }
                if field == "telemetry.causal_evidence.window_capacity"),
            "TelemetryConfig::validate must surface CausalEvidenceConfig errors, got {err:?}"
        );
    }

    /// Default `CausalEvidenceConfig` must validate cleanly.
    #[test]
    fn test_validate_default_causal_evidence_is_clean() {
        let cfg = CausalEvidenceConfig::default();
        cfg.validate()
            .expect("default causal_evidence must validate");
    }

    /// Round-trip: serialize `TelemetryConfig` with causal_evidence
    /// configured and parse the result back. Catches typos in
    /// `rename_all` / `default = "fn"` and verifies the field survives
    /// the YAML boundary.
    #[test]
    fn test_yaml_full_block_round_trip_includes_causal_evidence() {
        let yaml = r"
runtime_diagnosis:
  enabled: true
causal_evidence:
  enabled: false
  window_capacity: 512
";
        let cfg: TelemetryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.causal_evidence.enabled);
        assert_eq!(cfg.causal_evidence.window_capacity, 512);
        // Reserialize and parse again — the parsed value must match.
        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: TelemetryConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg, cfg2);
    }

    /// `to_diagnostics_options_with_full` must continue to produce a
    /// valid `DiagnosticsOptions` when `causal_evidence` is in its
    /// default state. The actual `causal_evidence` field on
    /// `DiagnosticsOptions` is owned by U01b (`diagnostics/mod.rs`); the
    /// U01a-side bridge lives in `to_diagnostics_options_inner` and is
    /// verified at integration time. Here we ensure the addition of the
    /// new config block does not regress the existing activation matrix.
    #[test]
    fn test_to_diagnostics_options_default_causal_evidence_does_not_regress() {
        let cfg = TelemetryConfig::default();
        let opts = cfg.to_diagnostics_options_with_full(Path::new("."), false);
        // Existing semantics must remain intact.
        assert!(!opts.full_diagnostics);
        assert!(!opts.runtime_diagnosis_artifacts);
        // Causal-evidence is enabled by default, so the bridge function
        // must compute a non-zero causal-evidence readiness signal even
        // though we cannot assert on the field directly until U01b lands.
        // We assert via the helper exposed by the bridge below.
        assert!(
            cfg.causal_evidence_enabled_for_bridge(),
            "default config must produce causal_evidence=true via the bridge"
        );
        // Explicit disable must propagate through the bridge.
        let cfg_off = TelemetryConfig {
            causal_evidence: CausalEvidenceConfig {
                enabled: false,
                window_capacity: 200,
            },
            ..TelemetryConfig::default()
        };
        assert!(
            !cfg_off.causal_evidence_enabled_for_bridge(),
            "explicit enabled=false must propagate through the bridge"
        );
    }

    /// `max_repeated_recoveries > u32::MAX` would cause a truncation when
    /// cast to `u32` in [`crate::event_loop::prompt_injection::runtime_recovery_context`];
    /// the validator must reject it.
    #[test]
    fn assert_cfg_max_repeated_recoveries_bounded_by_u32_max() {
        let cfg = RuntimeDiagnosisConfig {
            max_repeated_recoveries: u32::MAX as usize + 1,
            ..RuntimeDiagnosisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::TelemetryValidation {
                    ref field,
                    ref message,
                    ..
                } if field.contains("max_repeated_recoveries") && message.contains("u32::MAX")
            ),
            "expected TelemetryValidation for max_repeated_recoveries exceeding u32::MAX, got {err:?}"
        );
    }
}
