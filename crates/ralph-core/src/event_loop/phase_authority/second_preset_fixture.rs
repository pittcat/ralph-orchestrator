//! 2026-07-02-006 plan U25: minimal "second preset" fixture.
//!
//! Demonstrates that a future preset (call it `merge-loop`
//! or `autoresearch`) can opt into the engine with just a
//! 2-phase YAML. The fixture exercises the parser; the
//! preset itself never lands — U25 is about proving the
//! extension story, not adding a second consumer.

use crate::event_loop::phase_authority::config::{
    PhaseAuthorityConfig, PhaseDeclConfig, PhaseTransitionConfig, TransitionOnConfig,
};
use crate::event_loop::phase_authority::declaration::PhaseAuthorityDeclaration;

/// Build a 2-phase declaration that mirrors a hypothetical
/// "merge-loop" preset's minimum needs.
pub fn minimal_second_preset_declaration() -> PhaseAuthorityDeclaration {
    let cfg = PhaseAuthorityConfig {
        enabled: true,
        initial_phase: Some("work".to_string()),
        phases: vec![
            PhaseDeclConfig {
                id: "work".to_string(),
                label: None,
                allowed_emits: Default::default(),
            },
            PhaseDeclConfig {
                id: "ship".to_string(),
                label: None,
                allowed_emits: Default::default(),
            },
        ],
        transitions: vec![PhaseTransitionConfig {
            from: "work".to_string(),
            to: "ship".to_string(),
            on: TransitionOnConfig(serde_yaml::from_str("event: work.start").unwrap()),
        }],
        violation_policy: Default::default(),
        progress_projection: Default::default(),
    };
    PhaseAuthorityDeclaration::try_from_config(&cfg).expect("2-phase declaration must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_minimal_second_preset_parses() {
        let decl = minimal_second_preset_declaration();
        assert_eq!(decl.phases.len(), 2);
        assert_eq!(decl.phases[0].id, "work");
        assert_eq!(decl.phases[1].id, "ship");
        assert_eq!(decl.initial_phase.as_deref(), Some("work"));
        assert_eq!(decl.transitions.len(), 1);
    }

    #[test]
    fn fixture_passes_the_declared_whitelist_query() {
        let decl = minimal_second_preset_declaration();
        let d = crate::event_loop::phase_authority::whitelist::allows(
            "executor",
            "work.start",
            "work",
            &decl,
        );
        // The fixture doesn't declare any allowed_emits for
        // `work`, so the whitelist denies everything —
        // including `work.start`. The point of U25 is that
        // the declaration **parses**, not that it allows a
        // particular emit; future presets fill the allowed
        // lists at authoring time.
        assert!(!d.allowed);
    }
}
