//! Embedded presets for ralph init command.
//!
//! This module embeds all preset YAML files at compile time, making the
//! binary self-contained. Users can initialize projects with presets
//! without needing access to the source repository.
//!
//! Canonical presets live in the shared `presets/` directory at the repo root.
//! The sync script (`scripts/sync-embedded-files.sh`) mirrors them into
//! `crates/ralph-cli/presets/` for `include_str!` to work with crates.io publishing.

/// An embedded preset with its name, description, and full content.
#[derive(Debug, Clone)]
pub struct EmbeddedPreset {
    /// The preset name (e.g., "feature")
    pub name: &'static str,
    /// Short description extracted from the preset's header comment
    pub description: &'static str,
    /// Full YAML content of the preset
    pub content: &'static str,
    /// Whether this preset should be shown in normal user-facing listings.
    pub public: bool,
}

/// All embedded presets, compiled into the binary.
const PRESETS: &[EmbeddedPreset] = &[
    EmbeddedPreset {
        name: "code-assist",
        description: "Default implementation workflow with TDD and adversarial validation",
        content: include_str!("../presets/code-assist.yml"),
        public: true,
    },
    EmbeddedPreset {
        name: "debug",
        description: "Bug investigation, root-cause analysis, and adversarial fix verification",
        content: include_str!("../presets/debug.yml"),
        public: true,
    },
    EmbeddedPreset {
        name: "hatless-baseline",
        description: "Baseline hatless mode for comparison",
        content: include_str!("../presets/hatless-baseline.yml"),
        public: false,
    },
    EmbeddedPreset {
        name: "merge-loop",
        description: "Merges completed parallel loop from worktree back to main branch",
        content: include_str!("../presets/merge-loop.yml"),
        public: false,
    },
    EmbeddedPreset {
        name: "pdd-to-code-assist",
        description: "Advanced end-to-end idea-to-code workflow; powerful, slower, and best treated as a fun example",
        content: include_str!("../presets/pdd-to-code-assist.yml"),
        public: true,
    },
    EmbeddedPreset {
        name: "research",
        description: "Read-only codebase and architecture exploration with evidence-first synthesis",
        content: include_str!("../presets/research.yml"),
        public: true,
    },
    EmbeddedPreset {
        name: "review",
        description: "Adversarial code review without making modifications",
        content: include_str!("../presets/review.yml"),
        public: true,
    },
];

/// Returns all embedded presets.
pub fn list_presets() -> Vec<&'static EmbeddedPreset> {
    PRESETS.iter().filter(|preset| preset.public).collect()
}

/// Looks up a preset by name.
///
/// Returns `None` if the preset doesn't exist.
pub fn get_preset(name: &str) -> Option<&'static EmbeddedPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Returns a formatted list of preset names for error messages.
pub fn preset_names() -> Vec<&'static str> {
    PRESETS
        .iter()
        .filter(|preset| preset.public)
        .map(|preset| preset.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::RalphConfig;

    fn assert_public_preset_has_completion_path(preset: &EmbeddedPreset) {
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
        let promise = config.event_loop.completion_promise.trim();
        assert!(
            !promise.is_empty(),
            "Preset '{}' must define a non-empty completion promise",
            preset.name
        );

        let has_completion_path = config.hats.values().any(|hat| {
            hat.publishes.iter().any(|topic| topic == promise)
                || hat.default_publishes.as_deref() == Some(promise)
        });

        assert!(
            has_completion_path,
            "Preset '{}' must expose its completion promise '{}' via publishes/default_publishes",
            preset.name, promise
        );
    }

    fn assert_public_preset_has_required_events(preset: &EmbeddedPreset) {
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
        assert!(
            !config.event_loop.required_events.is_empty(),
            "Preset '{}' should define required_events to block premature completion",
            preset.name
        );
    }

    #[test]
    fn test_list_presets_returns_all() {
        let presets = list_presets();
        assert_eq!(presets.len(), 5, "Expected 5 public presets");
    }

    #[test]
    fn test_get_preset_by_name() {
        let preset = get_preset("code-assist");
        assert!(preset.is_some(), "code-assist preset should exist");
        let preset = preset.unwrap();
        assert_eq!(preset.name, "code-assist");
        assert!(!preset.description.is_empty());
        assert!(!preset.content.is_empty());
    }

    #[test]
    fn test_merge_loop_preset_is_embedded() {
        let preset = get_preset("merge-loop").expect("merge-loop preset should exist");
        assert_eq!(
            preset.description,
            "Merges completed parallel loop from worktree back to main branch"
        );
        // Verify key merge-related content
        assert!(preset.content.contains("RALPH_MERGE_LOOP_ID"));
        assert!(preset.content.contains("merge.start"));
        assert!(preset.content.contains("MERGE_COMPLETE"));
        assert!(preset.content.contains("conflict.detected"));
        assert!(preset.content.contains("conflict.resolved"));
        assert!(preset.content.contains("git merge"));
        assert!(preset.content.contains("git worktree remove"));
    }

    #[test]
    fn test_get_preset_invalid_name() {
        let preset = get_preset("nonexistent-preset");
        assert!(preset.is_none(), "Nonexistent preset should return None");
    }

    #[test]
    fn test_all_presets_have_description() {
        for preset in PRESETS {
            assert!(
                !preset.description.is_empty(),
                "Preset '{}' should have a description",
                preset.name
            );
        }
    }

    #[test]
    fn test_all_presets_have_content() {
        for preset in PRESETS {
            assert!(
                !preset.content.is_empty(),
                "Preset '{}' should have content",
                preset.name
            );
        }
    }

    #[test]
    fn test_preset_content_is_valid_yaml() {
        for preset in PRESETS {
            let result: Result<serde_yaml::Value, _> = serde_yaml::from_str(preset.content);
            assert!(
                result.is_ok(),
                "Preset '{}' should be valid YAML: {:?}",
                preset.name,
                result.err()
            );
        }
    }

    #[test]
    fn test_preset_names_returns_all_names() {
        let names = preset_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"debug"));
        assert!(names.contains(&"code-assist"));
        assert!(names.contains(&"research"));
        assert!(names.contains(&"review"));
        assert!(names.contains(&"pdd-to-code-assist"));
    }

    #[test]
    fn test_public_presets_have_completion_path() {
        for preset in PRESETS.iter().filter(|preset| preset.public) {
            assert_public_preset_has_completion_path(preset);
        }
    }

    #[test]
    fn test_public_presets_have_required_events() {
        for preset in PRESETS.iter().filter(|preset| preset.public) {
            assert_public_preset_has_required_events(preset);
        }
    }

    #[test]
    fn test_pdd_to_code_assist_uses_reviewed_increment_loop() {
        let preset = get_preset("pdd-to-code-assist").expect("pdd-to-code-assist should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(
            config.event_loop.required_events,
            vec![
                "design.approved".to_string(),
                "plan.ready".to_string(),
                "tasks.ready".to_string(),
                "implementation.ready".to_string(),
                "validation.passed".to_string(),
            ]
        );

        let builder = config
            .hats
            .get("builder")
            .expect("builder hat should exist");
        assert!(builder.triggers.contains(&"tasks.ready".to_string()));
        assert!(builder.triggers.contains(&"review.rejected".to_string()));
        assert!(
            builder
                .triggers
                .contains(&"finalization.failed".to_string())
        );
        assert!(builder.triggers.contains(&"validation.failed".to_string()));
        assert_eq!(
            builder.publishes,
            vec!["review.ready".to_string(), "build.blocked".to_string()]
        );
        assert_eq!(builder.default_publishes.as_deref(), Some("build.blocked"));

        let critic = config.hats.get("critic").expect("critic hat should exist");
        assert_eq!(critic.triggers, vec!["review.ready".to_string()]);
        assert_eq!(
            critic.publishes,
            vec!["review.passed".to_string(), "review.rejected".to_string()]
        );
        assert_eq!(critic.default_publishes.as_deref(), Some("review.rejected"));

        let finalizer = config
            .hats
            .get("finalizer")
            .expect("finalizer hat should exist");
        assert_eq!(finalizer.triggers, vec!["review.passed".to_string()]);
        assert_eq!(
            finalizer.publishes,
            vec![
                "tasks.ready".to_string(),
                "implementation.ready".to_string(),
                "finalization.failed".to_string(),
            ]
        );
        assert_eq!(
            finalizer.default_publishes.as_deref(),
            Some("finalization.failed")
        );

        let validator = config
            .hats
            .get("validator")
            .expect("validator hat should exist");
        assert_eq!(
            validator.default_publishes.as_deref(),
            Some("validation.failed")
        );

        let committer = config
            .hats
            .get("committer")
            .expect("committer hat should exist");
        assert_eq!(committer.default_publishes, None);
        assert_eq!(committer.publishes, vec!["LOOP_COMPLETE".to_string()]);
    }

    #[test]
    fn test_review_uses_staged_adversarial_completion_contract() {
        let preset = get_preset("review").expect("review preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(
            config.event_loop.required_events,
            vec!["review.section".to_string(), "analysis.complete".to_string()]
        );

        let reviewer = config
            .hats
            .get("reviewer")
            .expect("reviewer hat should exist");
        assert_eq!(
            reviewer.triggers,
            vec!["review.start".to_string(), "analysis.complete".to_string()]
        );
        assert_eq!(
            reviewer.publishes,
            vec!["review.section".to_string(), "REVIEW_COMPLETE".to_string()]
        );
        assert!(reviewer.instructions.contains("On `review.start`:"));
        assert!(reviewer.instructions.contains("Emit exactly one `review.section`"));
        assert!(reviewer.instructions.contains("On `analysis.complete`:"));
        assert!(reviewer.instructions.contains("Emit exactly one `REVIEW_COMPLETE`"));
        assert!(reviewer
            .instructions
            .contains("❌ Emit `REVIEW_COMPLETE` on the initial `review.start` pass"));

        let analyzer = config
            .hats
            .get("analyzer")
            .expect("analyzer hat should exist");
        assert_eq!(analyzer.triggers, vec!["review.section".to_string()]);
        assert_eq!(analyzer.publishes, vec!["analysis.complete".to_string()]);
        assert_eq!(analyzer.default_publishes, None);
        assert!(analyzer
            .instructions
            .contains("Emit exactly one `analysis.complete`"));
        assert!(analyzer
            .instructions
            .contains("adversarial or failure-path case"));
    }

    #[test]
    fn test_debug_uses_staged_adversarial_fix_contract() {
        let preset = get_preset("debug").expect("debug preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(
            config.event_loop.required_events,
            vec![
                "hypothesis.test".to_string(),
                "hypothesis.confirmed".to_string(),
                "fix.applied".to_string(),
                "fix.verified".to_string(),
            ]
        );

        let investigator = config
            .hats
            .get("investigator")
            .expect("investigator hat should exist");
        assert_eq!(
            investigator.triggers,
            vec![
                "debug.start".to_string(),
                "hypothesis.rejected".to_string(),
                "hypothesis.confirmed".to_string(),
                "fix.verified".to_string(),
            ]
        );
        assert_eq!(
            investigator.publishes,
            vec![
                "hypothesis.test".to_string(),
                "fix.propose".to_string(),
                "DEBUG_COMPLETE".to_string(),
            ]
        );
        assert!(investigator
            .instructions
            .contains("On `debug.start` or `hypothesis.rejected`:"));
        assert!(investigator
            .instructions
            .contains("If the bug is already fixed, cannot be reproduced, or an existing debug note already captures the answer"));
        assert!(investigator
            .instructions
            .contains("Emit exactly one `hypothesis.test`"));
        assert!(investigator
            .instructions
            .contains("On `hypothesis.confirmed`:"));
        assert!(investigator
            .instructions
            .contains("Emit exactly one `fix.propose`"));
        assert!(investigator.instructions.contains("On `fix.verified`:"));
        assert!(investigator
            .instructions
            .contains("Emit exactly one `DEBUG_COMPLETE`"));
        assert!(investigator
            .instructions
            .contains("Do not end the turn with only prose"));
        assert!(investigator
            .instructions
            .contains("❌ End the turn with only narration, document updates, or \"already complete\""));
        assert!(investigator
            .instructions
            .contains("❌ Emit undeclared topics like `debug.start`"));
        assert!(investigator
            .instructions
            .contains("❌ Skip the event chain by doing fix or verification work inline"));

        let tester = config.hats.get("tester").expect("tester hat should exist");
        assert_eq!(tester.triggers, vec!["hypothesis.test".to_string()]);
        assert_eq!(
            tester.publishes,
            vec![
                "hypothesis.confirmed".to_string(),
                "hypothesis.rejected".to_string(),
            ]
        );
        assert!(tester
            .instructions
            .contains("If the hypothesis says the bug is already fixed"));
        assert!(tester
            .instructions
            .contains("nearby adversarial or neighboring failure-path case"));

        let fixer = config.hats.get("fixer").expect("fixer hat should exist");
        assert_eq!(
            fixer.publishes,
            vec!["fix.applied".to_string(), "fix.blocked".to_string()]
        );
        assert_eq!(fixer.default_publishes.as_deref(), Some("fix.blocked"));
        assert!(!fixer.instructions.contains("Commit"));
        assert!(fixer.instructions.contains("❌ Make commits in this preset"));

        let verifier = config
            .hats
            .get("verifier")
            .expect("verifier hat should exist");
        assert_eq!(
            verifier.publishes,
            vec!["fix.verified".to_string(), "fix.failed".to_string()]
        );
        assert_eq!(verifier.default_publishes.as_deref(), Some("fix.failed"));
        assert!(verifier
            .instructions
            .contains("Re-run at least one nearby adversarial or failure-path case."));
    }
}
