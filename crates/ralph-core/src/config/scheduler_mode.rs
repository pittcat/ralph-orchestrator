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

use super::loop_config::DagPoolsConfig;
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

    /// 2026-09-03-0959 plan Step 3 (D16): `dag_pools` was declared
    /// while `scheduler_mode = wave`. The legacy `WaveTracker`
    /// authority does not consume per-pool caps, so accepting the
    /// block would create a second, silently-ignored capacity
    /// authority. Fail-closed: the operator must either drop
    /// `event_loop.supervisor.dag_pools` or switch the scheduler
    /// mode to `dag_shadow` / `dag`.
    DagPoolsWithWaveMode,

    /// 2026-09-03-0959 plan Step 3 (D16): a `dag_pools` pool cap of
    /// `0` would wedge the pool permanently (no job could ever be
    /// admitted), so zero caps are rejected at validation time
    /// rather than deadlocking the scheduler at runtime.
    DagPoolsZeroCap { field: &'static str },

    /// 2026-09-07 DAG wiring step E0: a hat declared
    /// `hats[].runtime_driven: true` while `scheduler_mode = wave`.
    /// Runtime-driven hats are job templates for the runtime DAG
    /// driver; the legacy `WaveTracker` authority has no driver to
    /// spawn them, so accepting the field would leave the hat
    /// silently inert. Fail-closed: the operator must either drop
    /// `runtime_driven` or switch the scheduler mode to
    /// `dag_shadow` / `dag`.
    RuntimeDrivenWithWaveMode { hat: String },
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
            SchedulerModeError::DagPoolsWithWaveMode => write!(
                f,
                "event_loop.supervisor.dag_pools is set but \
                 event_loop.supervisor.scheduler_mode = wave; dag_pools only \
                 applies to scheduler_mode = dag_shadow | dag (remove the \
                 dag_pools block or switch scheduler_mode)"
            ),
            SchedulerModeError::DagPoolsZeroCap { field } => write!(
                f,
                "event_loop.supervisor.dag_pools.{field} must be >= 1 (got 0)"
            ),
            SchedulerModeError::RuntimeDrivenWithWaveMode { hat } => write!(
                f,
                "hats[].runtime_driven: hat \"{hat}\" declares runtime_driven = true but \
                 event_loop.supervisor.scheduler_mode = wave; runtime_driven hats only \
                 apply to scheduler_mode = dag_shadow | dag (remove the runtime_driven \
                 field or switch scheduler_mode)"
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

/// 2026-09-03-0959 plan Step 3 (D16): validates the optional
/// `event_loop.supervisor.dag_pools` block against the chosen
/// [`SchedulerMode`]. Rules:
///
/// - `None` (block absent) is always legal — every pool then falls
///   back to `max_concurrent_workers`.
/// - Any pool cap of `0` is rejected (`DagPoolsZeroCap`); a zero
///   cap would permanently starve that pool.
/// - Under `wave` the block is rejected (`DagPoolsWithWaveMode`)
///   because the legacy authority never consumes per-pool caps;
///   `dag_shadow` and `dag` are legal.
///
/// Zero-cap shape errors are reported before the mode-combination
/// error so the operator fixes the malformed value first.
pub fn validate_dag_pools(
    mode: SchedulerMode,
    dag_pools: Option<&DagPoolsConfig>,
) -> Result<(), SchedulerModeError> {
    let Some(pools) = dag_pools else {
        return Ok(());
    };
    for (field, value) in [
        ("executor", pools.executor),
        ("reviewer", pools.reviewer),
        ("verifier", pools.verifier),
        ("fixer", pools.fixer),
    ] {
        if matches!(value, Some(0)) {
            return Err(SchedulerModeError::DagPoolsZeroCap { field });
        }
    }
    if mode.uses_legacy_authority() {
        return Err(SchedulerModeError::DagPoolsWithWaveMode);
    }
    Ok(())
}

/// 2026-09-07 DAG wiring step E0: validates the per-hat
/// `hats[].runtime_driven` flag against the chosen [`SchedulerMode`].
///
/// A runtime-driven hat is a job template for the runtime DAG driver:
/// under `dag` its event-topology trigger matching is suppressed (the
/// driver spawns it directly); under `dag_shadow` the flag is accepted
/// but inert (shadow only observes — the legacy wave execution face
/// keeps activating the hat). Under `wave` there is no driver at all,
/// so the flag is rejected fail-closed rather than leaving the hat
/// silently inert.
///
/// `runtime_driven_hats` carries the IDs of hats that declared
/// `runtime_driven: true`; the first one (sorted, deterministic) is
/// reported. The empty case is always legal.
pub fn validate_runtime_driven_hats(
    mode: SchedulerMode,
    runtime_driven_hats: &[String],
) -> Result<(), SchedulerModeError> {
    if !mode.uses_legacy_authority() || runtime_driven_hats.is_empty() {
        return Ok(());
    }
    let mut sorted: Vec<&String> = runtime_driven_hats.iter().collect();
    sorted.sort();
    Err(SchedulerModeError::RuntimeDrivenWithWaveMode {
        hat: sorted[0].clone(),
    })
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

    // ─────────────────────────────────────────────────────────────
    // 2026-09-03-0959 plan Step 3 (D16): dag_pools validation tests.
    // ─────────────────────────────────────────────────────────────

    fn pools(executor: Option<u32>) -> DagPoolsConfig {
        DagPoolsConfig {
            executor,
            ..DagPoolsConfig::default()
        }
    }

    #[test]
    fn dag_pools_absent_is_always_legal() {
        for mode in [
            SchedulerMode::Wave,
            SchedulerMode::DagShadow,
            SchedulerMode::Dag,
        ] {
            assert!(
                validate_dag_pools(mode, None).is_ok(),
                "absent dag_pools must validate under any mode ({mode:?})"
            );
        }
    }

    #[test]
    fn dag_pools_rejected_under_wave_mode() {
        let err = validate_dag_pools(SchedulerMode::Wave, Some(&pools(Some(2)))).unwrap_err();
        assert_eq!(err, SchedulerModeError::DagPoolsWithWaveMode);
    }

    #[test]
    fn dag_pools_accepted_under_dag_modes() {
        let pools = DagPoolsConfig {
            executor: Some(4),
            reviewer: Some(2),
            verifier: Some(1),
            fixer: Some(3),
        };
        for mode in [SchedulerMode::DagShadow, SchedulerMode::Dag] {
            assert!(
                validate_dag_pools(mode, Some(&pools)).is_ok(),
                "dag_pools must be legal under {mode:?}"
            );
        }
    }

    #[test]
    fn dag_pools_zero_cap_rejected() {
        for (field, pools) in [
            ("executor", pools(Some(0))),
            (
                "fixer",
                DagPoolsConfig {
                    fixer: Some(0),
                    ..DagPoolsConfig::default()
                },
            ),
        ] {
            let err = validate_dag_pools(SchedulerMode::Dag, Some(&pools)).unwrap_err();
            assert_eq!(
                err,
                SchedulerModeError::DagPoolsZeroCap { field },
                "zero {field} cap must be rejected"
            );
        }
    }

    #[test]
    fn dag_pools_zero_cap_rejected_before_mode_combination() {
        // A zero cap is a shape error and must surface even under
        // `wave` so the operator fixes the malformed value first.
        let err = validate_dag_pools(SchedulerMode::Wave, Some(&pools(Some(0)))).unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::DagPoolsZeroCap { field: "executor" }
        );
    }

    #[test]
    fn dag_pools_error_message_includes_field_path() {
        let rendered = SchedulerModeError::DagPoolsWithWaveMode.to_string();
        assert!(
            rendered.contains("event_loop.supervisor.dag_pools"),
            "error must reference the field path; got: {rendered}"
        );
        assert!(
            rendered.contains("wave"),
            "error must report the offending mode; got: {rendered}"
        );

        let rendered = SchedulerModeError::DagPoolsZeroCap { field: "reviewer" }.to_string();
        assert!(
            rendered.contains("event_loop.supervisor.dag_pools.reviewer"),
            "error must reference the offending leaf field; got: {rendered}"
        );
    }

    // ─────────────────────────────────────────────────────────────
    // 2026-09-07 DAG wiring step E0: runtime_driven validation tests.
    // ─────────────────────────────────────────────────────────────

    #[test]
    fn runtime_driven_absent_is_always_legal() {
        for mode in [
            SchedulerMode::Wave,
            SchedulerMode::DagShadow,
            SchedulerMode::Dag,
        ] {
            assert!(
                validate_runtime_driven_hats(mode, &[]).is_ok(),
                "no runtime_driven hats must validate under any mode ({mode:?})"
            );
        }
    }

    #[test]
    fn runtime_driven_rejected_under_wave_mode() {
        let hats = vec!["executor".to_string()];
        let err = validate_runtime_driven_hats(SchedulerMode::Wave, &hats).unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::RuntimeDrivenWithWaveMode {
                hat: "executor".to_string()
            }
        );
    }

    #[test]
    fn runtime_driven_accepted_under_dag_modes() {
        let hats = vec!["executor".to_string(), "reviewer".to_string()];
        for mode in [SchedulerMode::DagShadow, SchedulerMode::Dag] {
            assert!(
                validate_runtime_driven_hats(mode, &hats).is_ok(),
                "runtime_driven hats must be legal under {mode:?}"
            );
        }
    }

    #[test]
    fn runtime_driven_reports_first_hat_sorted() {
        let hats = vec!["verifier".to_string(), "executor".to_string()];
        let err = validate_runtime_driven_hats(SchedulerMode::Wave, &hats).unwrap_err();
        assert_eq!(
            err,
            SchedulerModeError::RuntimeDrivenWithWaveMode {
                hat: "executor".to_string()
            },
            "offending hat report must be deterministic (sorted)"
        );
    }

    #[test]
    fn runtime_driven_error_message_includes_field_path() {
        let rendered = SchedulerModeError::RuntimeDrivenWithWaveMode {
            hat: "executor".to_string(),
        }
        .to_string();
        assert!(
            rendered.contains("hats[].runtime_driven"),
            "error must reference the field path; got: {rendered}"
        );
        assert!(
            rendered.contains("executor"),
            "error must name the offending hat; got: {rendered}"
        );
        assert!(
            rendered.contains("wave"),
            "error must report the offending mode; got: {rendered}"
        );
    }
}
