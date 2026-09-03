//! 2026-09-03-0959 plan U1: tri-state `scheduler_mode` gate that
//! isolates the legacy `Wave` authority from the new
//! runtime-owned DAG scheduler authority.
//!
//! Scope is intentionally tiny:
//! - [`SchedulerMode`] enum + serde defaults
//! - [`SchedulerModeError`] validation error type
//! - [`validate_scheduler_mode`] helper that rejects `DagShadow`
//!   and `Dag` modes unless the supervisor is enabled and the
//!   execution mode is `Isolated` (fail-closed per S1/S2).
//!
//! Lives in `crate::config` (not `crate::supervisor`) so both
//! `EventLoopConfig` (which holds `supervisor.scheduler_mode`)
//! and the supervisor runtime can depend on it without creating
//! a `config -> supervisor -> config` import cycle.
//!
//! Future Units will wire this enum through the dispatcher /
//! coordinator. **Do not** introduce DB tables, scheduler rows,
//! or change `parallel-forge` preset YAML in this Unit.

use serde::{Deserialize, Serialize};

use super::workflow_guards::HatExecutionMode;

/// Tri-state selector for the wave-scheduler authority.
///
/// `Wave` is the legacy `WaveTracker` path; `DagShadow` runs the
/// legacy path while the DAG scheduler observes in dry-run
/// (no DB writes, no `forge.wave.*` projections); `Dag` enables
/// the new runtime-owned work-conserving DAG scheduler.
///
/// Defaults to [`SchedulerMode::Wave`] so existing loops and
/// presets keep the legacy behaviour (R3 / D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerMode {
    /// Legacy `WaveTracker` path. Default.
    #[default]
    Wave,
    /// Dual-run: legacy path active, DAG scheduler observes in
    /// dry-run (no side effects). Used during the cutover
    /// window to validate the new authority on production
    /// traffic.
    DagShadow,
    /// Runtime-owned work-conserving DAG scheduler owns
    /// dispatch.
    Dag,
}

impl SchedulerMode {
    /// Helper used by the dispatcher / preflight to decide which
    /// authority is in charge. Conservative: only `Wave` returns
    /// `true` here; the DAG branches require explicit validation
    /// (see [`validate_scheduler_mode`]).
    pub fn uses_legacy_authority(self) -> bool {
        matches!(self, SchedulerMode::Wave)
    }

    /// String form used in error messages and JSON traces.
    pub fn as_str(self) -> &'static str {
        match self {
            SchedulerMode::Wave => "wave",
            SchedulerMode::DagShadow => "dag_shadow",
            SchedulerMode::Dag => "dag",
        }
    }
}

/// Validation error returned by [`validate_scheduler_mode`] when
/// the chosen mode does not fit the rest of the configuration.
///
/// The error type intentionally carries a stable, structured
/// payload (`mode`, `execution_mode`) so preflight / future CLI
/// commands can render a deterministic message and so tests can
/// assert on individual fields without string matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerModeError {
    /// `dag_shadow` or `dag` were selected while the supervisor
    /// is disabled. The DAG authority requires
    /// `event_loop.supervisor.enabled: true`.
    SupervisorDisabled { mode: SchedulerMode },

    /// `dag_shadow` or `dag` were selected while `execution_mode`
    /// is `Coordinator`. The DAG authority requires
    /// `event_loop.execution_mode: isolated`.
    NotIsolated {
        mode: SchedulerMode,
        execution_mode: HatExecutionMode,
    },
}

impl core::fmt::Display for SchedulerModeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SchedulerModeError::SupervisorDisabled { mode } => write!(
                f,
                "event_loop.supervisor.scheduler_mode = {} requires \
                 event_loop.supervisor.enabled = true (currently false)",
                mode.as_str()
            ),
            SchedulerModeError::NotIsolated {
                mode,
                execution_mode,
            } => write!(
                f,
                "event_loop.supervisor.scheduler_mode = {} requires \
                 event_loop.execution_mode = isolated (currently {})",
                mode.as_str(),
                execution_mode.as_str()
            ),
        }
    }
}

impl std::error::Error for SchedulerModeError {}

impl HatExecutionMode {
    /// Stable string form used by [`SchedulerModeError`] and by
    /// the preflight / CLI rendering. Kept here (rather than on
    /// `HatExecutionMode` itself in `workflow_guards`) so the
    /// supervisor-facing scheduler primitive does not grow the
    /// `workflow_guards` surface area.
    pub fn as_str(&self) -> &'static str {
        match self {
            HatExecutionMode::Coordinator => "coordinator",
            HatExecutionMode::Isolated => "isolated",
        }
    }
}

/// Validates the chosen [`SchedulerMode`] against the rest of the
/// event-loop configuration. Returns `Ok(())` for `Wave` (always
/// legal) and for `DagShadow` / `Dag` only when the supervisor is
/// enabled and `execution_mode == Isolated`.
///
/// Fail-closed: any unsupported combination returns
/// [`SchedulerModeError`] rather than silently downgrading to
/// `Wave` (E12 / E17).
pub fn validate_scheduler_mode(
    mode: SchedulerMode,
    supervisor_enabled: bool,
    execution_mode: HatExecutionMode,
) -> Result<(), SchedulerModeError> {
    if mode.uses_legacy_authority() {
        return Ok(());
    }
    if !supervisor_enabled {
        return Err(SchedulerModeError::SupervisorDisabled { mode });
    }
    if execution_mode != HatExecutionMode::Isolated {
        return Err(SchedulerModeError::NotIsolated {
            mode,
            execution_mode,
        });
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// 2026-09-03-0959 plan U1: characterisation + validation tests.
//
// Each test exercises one row of the validation matrix and pins
// the wire format for the error type so future CLI / preflight
// rendering cannot silently drift.
// ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod scheduler_mode_tests {
    use super::*;

    #[test]
    fn scheduler_mode_default_is_wave() {
        assert_eq!(SchedulerMode::default(), SchedulerMode::Wave);
        assert!(SchedulerMode::default().uses_legacy_authority());
    }

    #[test]
    fn scheduler_mode_roundtrips_through_yaml() {
        for (raw, expected) in [
            ("wave", SchedulerMode::Wave),
            ("dag_shadow", SchedulerMode::DagShadow),
            ("dag", SchedulerMode::Dag),
        ] {
            let parsed: SchedulerMode = serde_yaml::from_str(raw).expect("snake_case parse");
            assert_eq!(parsed, expected, "serde roundtrip mismatch for {raw}");
            let rendered = serde_yaml::to_string(&expected)
                .expect("render")
                .trim()
                .to_string();
            assert_eq!(rendered, raw, "snake_case render mismatch for {raw}");
        }
    }

    #[test]
    fn scheduler_mode_rejects_unknown_value() {
        let result: Result<SchedulerMode, _> = serde_yaml::from_str("fifo");
        assert!(
            result.is_err(),
            "unknown scheduler_mode must fail at the serde boundary (E12)"
        );
    }

    #[test]
    fn scheduler_mode_default_when_field_omitted() {
        // When the operator's ralph.yml omits the field, the
        // typed view defaults to `Wave` so the legacy path
        // keeps its zero-regression contract (R3 / D2).
        let cfg: serde_yaml::Value = serde_yaml::from_str("enabled: true").unwrap();
        let mode: SchedulerMode = cfg
            .get("scheduler_mode")
            .cloned()
            .map(serde_yaml::from_value)
            .transpose()
            .expect("serde parse of optional field")
            .unwrap_or_default();
        assert_eq!(mode, SchedulerMode::Wave);
    }

    #[test]
    fn validate_wave_always_ok() {
        // `Wave` is the default and works under any combination
        // (supervisor disabled, coordinator mode, both).
        for (enabled, mode) in [
            (false, HatExecutionMode::Coordinator),
            (false, HatExecutionMode::Isolated),
            (true, HatExecutionMode::Coordinator),
            (true, HatExecutionMode::Isolated),
        ]
        .iter()
        .cloned()
        {
            assert!(
                validate_scheduler_mode(SchedulerMode::Wave, enabled, mode.clone()).is_ok(),
                "Wave must always validate (enabled={enabled}, execution_mode={:?})",
                mode
            );
        }
    }

    #[test]
    fn validate_dag_shadow_requires_supervisor_enabled_and_isolated() {
        // (supervisor=false, isolated) → SupervisorDisabled
        let err =
            validate_scheduler_mode(SchedulerMode::DagShadow, false, HatExecutionMode::Isolated)
                .unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::SupervisorDisabled {
                mode: SchedulerMode::DagShadow
            }
        );
        // (supervisor=true, coordinator) → NotIsolated
        let err = validate_scheduler_mode(
            SchedulerMode::DagShadow,
            true,
            HatExecutionMode::Coordinator,
        )
        .unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::NotIsolated {
                mode: SchedulerMode::DagShadow,
                execution_mode: HatExecutionMode::Coordinator,
            }
        );
        // (supervisor=false, coordinator) → SupervisorDisabled wins
        // (precedence: supervisor gate is the master switch and must
        // be reported first so the operator does not chase the
        // downstream error after enabling the supervisor only to
        // discover they still need to switch execution_mode).
        let err = validate_scheduler_mode(
            SchedulerMode::DagShadow,
            false,
            HatExecutionMode::Coordinator,
        )
        .unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::SupervisorDisabled {
                mode: SchedulerMode::DagShadow
            }
        );
        // Happy path: (supervisor=true, isolated) → Ok
        assert!(
            validate_scheduler_mode(SchedulerMode::DagShadow, true, HatExecutionMode::Isolated)
                .is_ok()
        );
    }

    #[test]
    fn validate_dag_has_same_preconditions_as_dag_shadow() {
        // supervisor=false → SupervisorDisabled
        let err = validate_scheduler_mode(SchedulerMode::Dag, false, HatExecutionMode::Isolated)
            .unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::SupervisorDisabled {
                mode: SchedulerMode::Dag
            }
        );
        // coordinator mode → NotIsolated
        let err = validate_scheduler_mode(SchedulerMode::Dag, true, HatExecutionMode::Coordinator)
            .unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::NotIsolated {
                mode: SchedulerMode::Dag,
                execution_mode: HatExecutionMode::Coordinator,
            }
        );
        // Happy path
        assert!(
            validate_scheduler_mode(SchedulerMode::Dag, true, HatExecutionMode::Isolated).is_ok()
        );
    }

    #[test]
    fn scheduler_mode_error_message_includes_field_path_and_value() {
        // The error contract requires a stable field-path
        // reference (`event_loop.supervisor.scheduler_mode`) so
        // operators and tests can locate the misconfiguration
        // without inspecting YAML line numbers.
        let err = SchedulerModeError::SupervisorDisabled {
            mode: SchedulerMode::Dag,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("event_loop.supervisor.scheduler_mode"),
            "error must reference the field path; got: {rendered}"
        );
        assert!(
            rendered.contains("dag"),
            "error must include the offending value; got: {rendered}"
        );
        assert!(
            rendered.contains("event_loop.supervisor.enabled"),
            "error must reference the supervisor.enabled dependency; got: {rendered}"
        );

        let err = SchedulerModeError::NotIsolated {
            mode: SchedulerMode::DagShadow,
            execution_mode: HatExecutionMode::Coordinator,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("event_loop.supervisor.scheduler_mode"),
            "error must reference the field path; got: {rendered}"
        );
        assert!(
            rendered.contains("isolated"),
            "error must name the required value; got: {rendered}"
        );
        assert!(
            rendered.contains("coordinator"),
            "error must report the current value; got: {rendered}"
        );
    }
}
