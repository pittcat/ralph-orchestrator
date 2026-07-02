//! 2026-07-02-006 plan U15: tests for `build_stage_pipeline_from_config`.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- build_stage_pipeline_phase_branch`.

use crate::config::{EventLoopConfig, MechanismConfig, RalphConfig};
use crate::event_loop::build_stage_pipeline_from_config;
use crate::event_loop::phase_authority::config::{PhaseAuthorityConfig, PhaseDeclConfig};

fn base_config() -> RalphConfig {
    RalphConfig {
        event_loop: EventLoopConfig::default(),
        ..RalphConfig::default()
    }
}

#[test]
fn phase_authority_enabled_adds_phase_authority_stage() {
    let mut cfg = base_config();
    cfg.event_loop.mechanism = Some(MechanismConfig {
        flow: None,
        phase_authority: Some(PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("unit_loop".to_string()),
            phases: vec![PhaseDeclConfig {
                id: "unit_loop".to_string(),
                label: None,
                allowed_emits: Default::default(),
            }],
            transitions: vec![],
            violation_policy: Default::default(),
            progress_projection: Default::default(),
        }),
    });

    let (pipeline, _totals) = build_stage_pipeline_from_config(&cfg);
    let names = pipeline.names();
    assert!(
        names.contains(&"PhaseAuthority"),
        "phase-authority-enabled preset must build a pipeline that includes PhaseAuthority; got {:?}",
        names
    );
}

#[test]
fn phase_authority_disabled_omits_phase_authority_stage() {
    let cfg = base_config();
    let (pipeline, _totals) = build_stage_pipeline_from_config(&cfg);
    let names = pipeline.names();
    assert!(
        !names.contains(&"PhaseAuthority"),
        "default pipeline must not include PhaseAuthority when the engine is disabled; got {:?}",
        names
    );
}

#[test]
fn phase_authority_enabled_keeps_other_stages() {
    let mut cfg = base_config();
    cfg.event_loop.mechanism = Some(MechanismConfig {
        flow: None,
        phase_authority: Some(PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("unit_loop".to_string()),
            phases: vec![],
            transitions: vec![],
            violation_policy: Default::default(),
            progress_projection: Default::default(),
        }),
    });
    let (pipeline, _totals) = build_stage_pipeline_from_config(&cfg);
    let names = pipeline.names();
    for required in ["RepairDispatch", "EmitSchemaGate", "VerdictGate"] {
        assert!(
            names.contains(&required),
            "phase-authority pipeline missing required stage {}; got {:?}",
            required,
            names
        );
    }
}