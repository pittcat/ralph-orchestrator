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
        name: "autoresearch",
        description: "Autonomous experiment loop: try ideas, measure, keep what works, discard what doesn't",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/autoresearch.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "ce-executor",
        description: "Plan-driven work execution with adversarial wave code review, auto-fix loop, shipping workflow, and manager report",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "ce-executor-wave",
        description: "Wave-based parallel plan-driven execution with adversarial review, auto-fix, and shipping",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor-wave.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "code-assist",
        description: "Default implementation workflow with TDD and adversarial validation",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/code-assist.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "debug",
        description: "Bug investigation, root-cause analysis, and adversarial fix verification",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/debug.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "merge-loop",
        description: "Merges completed parallel loop from worktree back to main branch",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/merge-loop.yml")),
        public: false,
    },
    EmbeddedPreset {
        name: "pdd-to-code-assist",
        description: "Advanced end-to-end idea-to-code workflow; powerful, slower, and best treated as a fun example",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/pdd-to-code-assist.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "research",
        description: "Read-only codebase and architecture exploration with evidence-first synthesis",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/research.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "review",
        description: "Adversarial code review without making modifications",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/review.yml")),
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
    use ralph_core::event_origin::{OriginCheck, validate_event_origin};
    use ralph_core::payload_contract::validate_payload_contract;
    use ralph_core::{HatRegistry, RalphConfig};

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
        assert_eq!(presets.len(), 8, "Expected 8 public presets");
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
        assert_eq!(names.len(), 8);
        assert!(names.contains(&"autoresearch"));
        assert!(names.contains(&"ce-executor"));
        assert!(names.contains(&"ce-executor-wave"));
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

        assert_eq!(config.core.specs_dir, ".agents/scratchpad/");
        assert!(
            preset
                .content
                .contains(".agents/scratchpad/implementation/{spec_name}/idea-honing.md")
        );
        assert!(!preset.content.contains("requirements-interview.md"));

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
        assert!(builder.instructions.contains("`task_id`"));
        assert!(builder.instructions.contains("`task_key`"));
        assert!(
            builder
                .instructions
                .contains("ralph tools task show <task_id> --format json")
        );
        assert!(
            builder
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(
            builder
                .instructions
                .contains("ONE runtime task / code task pair per iteration")
        );

        let critic = config.hats.get("critic").expect("critic hat should exist");
        assert_eq!(critic.triggers, vec!["review.ready".to_string()]);
        assert_eq!(
            critic.publishes,
            vec!["review.passed".to_string(), "review.rejected".to_string()]
        );
        assert_eq!(critic.default_publishes.as_deref(), Some("review.rejected"));
        assert!(critic.instructions.contains("`task_id`, `task_key`"));
        assert!(
            critic
                .instructions
                .contains("ralph tools task show <task_id> --format json")
        );
        assert!(critic.instructions.contains("ralph tools memory add"));

        let finalizer = config
            .hats
            .get("finalizer")
            .expect("finalizer hat should exist");
        assert_eq!(finalizer.triggers, vec!["review.passed".to_string()]);
        assert_eq!(
            finalizer.publishes,
            vec![
                "queue.advance".to_string(),
                "implementation.ready".to_string(),
                "finalization.failed".to_string(),
            ]
        );
        assert_eq!(
            finalizer.default_publishes.as_deref(),
            Some("finalization.failed")
        );
        assert!(
            finalizer
                .instructions
                .contains("ralph tools task close <task_id>")
        );
        assert!(
            finalizer
                .instructions
                .contains("ralph tools task reopen <task_id>")
        );
        assert!(
            finalizer
                .instructions
                .contains("implementation runtime tasks are closed")
        );
        assert!(
            finalizer
                .instructions
                .contains("Task Writer owns wave creation")
        );

        let task_writer = config
            .hats
            .get("task_writer")
            .expect("task_writer hat should exist");
        assert_eq!(
            task_writer.triggers,
            vec!["plan.ready".to_string(), "queue.advance".to_string()]
        );
        assert!(
            task_writer
                .instructions
                .contains("Mirror ONLY Step 1's code task files into runtime tasks")
        );
        assert!(
            task_writer
                .instructions
                .contains("mirror ONLY that next step's code task files into runtime tasks")
        );
        assert!(
            task_writer
                .instructions
                .contains("`pdd:{spec_name}:step-01:{task_slug}`")
        );
        assert!(
            task_writer
                .instructions
                .contains("runtime tasks are the live queue")
        );

        let validator = config
            .hats
            .get("validator")
            .expect("validator hat should exist");
        assert_eq!(
            validator.default_publishes.as_deref(),
            Some("validation.failed")
        );
        assert!(validator.instructions.contains(
            "validation runtime task with a stable key like `pdd:{spec_name}:validation`"
        ));
        assert!(
            validator
                .instructions
                .contains("implementation runtime tasks are closed")
        );

        let committer = config
            .hats
            .get("committer")
            .expect("committer hat should exist");
        assert_eq!(committer.default_publishes, None);
        assert_eq!(committer.publishes, vec!["LOOP_COMPLETE".to_string()]);
        assert!(
            committer
                .instructions
                .contains("commit runtime task with a stable key like `pdd:{spec_name}:commit`")
        );
        assert!(!committer.instructions.contains("assisted-by note"));
    }

    #[test]
    fn test_code_assist_uses_upstream_artifact_layout_and_builder_workflow() {
        let preset = get_preset("code-assist").expect("code-assist should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(config.core.specs_dir, ".agents/scratchpad/");
        assert_eq!(
            config.event_loop.required_events,
            vec!["review.passed".to_string()]
        );

        let planner = config
            .hats
            .get("planner")
            .expect("planner hat should exist");
        assert!(
            planner
                .instructions
                .contains(".agents/scratchpad/implementation/{task_name}/")
        );
        assert!(
            planner
                .instructions
                .contains("one file, one function, or one user-facing behavior")
        );
        assert!(
            planner
                .instructions
                .contains("Runtime tasks are the canonical execution queue")
        );
        assert!(
            planner
                .instructions
                .contains("Use `ralph tools task ensure` with a stable key")
        );
        assert!(
            planner
                .instructions
                .contains("`code-assist:{task_name}:step-01:{slug}`")
        );
        assert!(
            planner
                .instructions
                .contains("`code-assist:{task_name}:step-02:{slug}`")
        );
        assert_eq!(
            planner.triggers,
            vec!["build.start".to_string(), "queue.advance".to_string()]
        );
        assert!(planner.instructions.contains("`task_id`"));
        assert!(planner.instructions.contains("`task_key`"));
        assert!(planner.instructions.contains("context.md"));
        assert!(planner.instructions.contains("plan.md"));
        assert!(planner.instructions.contains("progress.md"));
        assert!(!planner.instructions.contains("rough-idea.md"));
        assert!(
            planner
                .instructions
                .contains("`plan.md` MUST be a numbered step plan")
        );
        assert!(planner.instructions.contains("`## Current Step`"));
        assert!(planner.instructions.contains("`## Active Wave`"));
        assert!(
            planner
                .instructions
                .contains("Only one step's wave may exist as open/ready work at a time.")
        );
        assert!(
            planner
                .instructions
                .contains("You MUST NOT create future-step waves early")
        );

        let builder = config
            .hats
            .get("builder")
            .expect("builder hat should exist");
        assert!(
            builder
                .instructions
                .contains("Read `CODEASSIST.md` if it exists in the repo root")
        );
        assert!(
            builder
                .instructions
                .contains("the runtime task from the event payload via `ralph tools task show <task_id> --format json`")
        );
        assert!(
            builder.instructions.contains(
                "Read the task title, description, requirements, and acceptance criteria"
            )
        );
        assert!(
            builder
                .instructions
                .contains("Start the task with `ralph tools task start <task_id>`")
        );
        assert!(builder.instructions.contains(
            "Keep documentation in the shared docs directory and code in the repo itself"
        ));
        assert!(builder.instructions.contains("VALIDATE THE INCREMENT"));
        assert!(
            builder
                .instructions
                .contains("You MUST keep implementation code out of the shared docs directory")
        );
        assert!(
            builder
                .instructions
                .contains("`progress.md` is a verification/log summary. It is NOT the queue.")
        );
        assert!(
            builder
                .instructions
                .contains("You MUST implement the runtime task from the current payload")
        );
        assert!(builder.instructions.contains(
            "finish with a minimally runnable skeleton that satisfies the task description"
        ));
        assert!(
            builder
                .instructions
                .contains("Implement exactly one runtime task per iteration.")
        );

        let finalizer = config
            .hats
            .get("finalizer")
            .expect("finalizer hat should exist");
        assert_eq!(
            finalizer.publishes,
            vec![
                "queue.advance".to_string(),
                "finalization.failed".to_string(),
                "LOOP_COMPLETE".to_string(),
            ]
        );
        assert!(
            finalizer
                .instructions
                .contains("runtime tasks as the canonical completion gate")
        );
        assert!(
            finalizer
                .instructions
                .contains("ralph tools task close <task_id>")
        );
        assert!(
            finalizer
                .instructions
                .contains("ralph tools task reopen <task_id>")
        );
        assert!(
            finalizer
                .instructions
                .contains(".agents/scratchpad/implementation/{task_name}/")
        );
        assert!(
            finalizer
                .instructions
                .contains("Do not go hunting for planner docs under `.eval-sandbox/code-assist/`.")
        );
        assert!(finalizer.instructions.contains(
            "`queue.advance` if the current step still has open work OR later planned steps remain"
        ));
        assert!(
            !finalizer
                .instructions
                .contains("`task.complete` if more runtime work remains")
        );
        assert!(
            finalizer
                .instructions
                .contains("You MUST NOT ensure the next step's runtime tasks yourself because Planner owns wave creation")
        );
        assert!(
            finalizer
                .instructions
                .contains("all planned steps are complete and no runtime tasks remain open")
        );
    }

    #[test]
    fn test_review_uses_staged_adversarial_completion_contract() {
        let preset = get_preset("review").expect("review preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(
            config.event_loop.required_events,
            vec![
                "review.section".to_string(),
                "analysis.complete".to_string()
            ]
        );

        let reviewer = config
            .hats
            .get("reviewer")
            .expect("reviewer hat should exist");
        assert_eq!(
            reviewer.triggers,
            vec!["review.start".to_string(), "review.followup".to_string()]
        );
        assert_eq!(reviewer.publishes, vec!["review.section".to_string()]);
        assert_eq!(
            reviewer.default_publishes.as_deref(),
            Some("review.section")
        );
        assert!(reviewer.instructions.contains("On `review.start`:"));
        assert!(reviewer.instructions.contains("On `review.followup`:"));
        assert!(
            reviewer
                .instructions
                .contains("Emit exactly one `review.section`")
        );
        assert!(reviewer.instructions.contains("`review:step-01:primary`"));
        assert!(reviewer.instructions.contains("`review:step-02:{slug}`"));
        assert!(reviewer.instructions.contains("`task_id` and `task_key`"));
        assert!(
            reviewer
                .instructions
                .contains("Writing `findings.md` alone does not complete the turn")
        );
        assert!(
            reviewer
                .instructions
                .contains("Do not try to produce the final report on this first pass")
        );
        assert!(
            reviewer
                .instructions
                .contains("❌ Emit `REVIEW_COMPLETE` on the initial `review.start` pass")
        );

        let analyzer = config
            .hats
            .get("analyzer")
            .expect("analyzer hat should exist");
        assert_eq!(analyzer.triggers, vec!["review.section".to_string()]);
        assert_eq!(analyzer.publishes, vec!["analysis.complete".to_string()]);
        assert_eq!(
            analyzer.default_publishes.as_deref(),
            Some("analysis.complete")
        );
        assert!(
            analyzer
                .instructions
                .contains("Emit exactly one `analysis.complete`")
        );
        assert!(
            analyzer
                .instructions
                .contains("ralph tools task start <analysis_task_id>")
        );
        assert!(
            analyzer
                .instructions
                .contains("ralph tools task close <analysis_task_id>")
        );
        assert!(
            analyzer
                .instructions
                .contains("adversarial or failure-path case")
        );
        assert!(
            analyzer
                .instructions
                .contains("Writing `findings.md` alone does not complete the turn")
        );
        assert!(
            analyzer
                .instructions
                .contains("Do not append a long prose recap after the emit command.")
        );

        let closer = config.hats.get("closer").expect("closer hat should exist");
        assert_eq!(closer.triggers, vec!["analysis.complete".to_string()]);
        assert_eq!(
            closer.publishes,
            vec!["review.followup".to_string(), "REVIEW_COMPLETE".to_string()]
        );
        assert_eq!(closer.default_publishes, None);
        assert!(closer.instructions.contains(
            "If task lookup is noisy or slow, skip the closure work and finish the review"
        ));
        assert!(
            closer
                .instructions
                .contains("\"$RALPH_BIN\" tools task close <primary_task_id>")
        );
        assert!(
            closer
                .instructions
                .contains("emit exactly one `review.followup` event")
        );
        assert!(
            closer
                .instructions
                .contains("emit exactly one `REVIEW_COMPLETE`")
        );
        assert!(
            closer
                .instructions
                .contains("Do not create tasks yourself.")
        );
        assert!(closer.instructions.contains(
            "real `ralph emit \"review.followup\" ...` or `ralph emit \"REVIEW_COMPLETE\" ...`"
        ));
    }

    #[test]
    fn test_research_uses_runtime_tasks_and_memory_discipline() {
        let preset = get_preset("research").expect("research preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");

        assert_eq!(config.core.specs_dir, ".agents/scratchpad/");
        assert_eq!(
            config.event_loop.required_events,
            vec!["research.finding".to_string()]
        );

        let researcher = config
            .hats
            .get("researcher")
            .expect("researcher hat should exist");
        assert_eq!(researcher.publishes, vec!["research.finding".to_string()]);
        assert_eq!(
            researcher.default_publishes.as_deref(),
            Some("research.finding")
        );
        assert!(
            researcher
                .instructions
                .contains("Runtime tasks are the canonical queue")
        );
        assert!(
            researcher
                .instructions
                .contains(".eval-sandbox/research/plan.md")
        );
        assert!(
            researcher
                .instructions
                .contains("`research:step-01:primary`")
        );
        assert!(
            researcher
                .instructions
                .contains("`research:step-02:{slug}`")
        );
        assert!(
            researcher
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(researcher.instructions.contains("ralph tools memory add"));
        assert!(researcher.instructions.contains("`task_id` and `task_key`"));
        assert!(
            researcher
                .instructions
                .contains("only the CURRENT research wave may exist as open work")
        );
        assert!(
            researcher
                .instructions
                .contains("Do NOT investigate that next wave inline in the same turn.")
        );
        assert!(
            researcher.instructions.contains(
                "As soon as you have 3-6 concrete evidence points with file:line support"
            )
        );
        assert!(researcher.instructions.contains(
            "If `.eval-sandbox/research/summary.md` already contains the current wave's answer"
        ));
        assert!(researcher.instructions.contains(
            "The turn is incomplete until a real `ralph emit \"research.finding\" ...` command succeeds."
        ));
        assert!(researcher.instructions.contains(
            "❌ End the turn after writing `summary.md` without emitting `research.finding`"
        ));
        assert!(
            researcher
                .instructions
                .contains("❌ Keep browsing once you already have enough evidence for this wave")
        );

        let synthesizer = config
            .hats
            .get("synthesizer")
            .expect("synthesizer hat should exist");
        assert!(
            synthesizer
                .instructions
                .contains("Runtime tasks are the completion gate")
        );
        assert!(
            synthesizer
                .instructions
                .contains("ralph tools task show <task_id> --format json")
        );
        assert!(
            synthesizer.instructions.contains(
                "If the payload omitted `task_id`, resolve the active task from open tasks"
            )
        );
        assert!(
            synthesizer
                .instructions
                .contains("ralph tools task close <task_id>")
        );
        assert!(
            synthesizer
                .instructions
                .contains("`research:step-02:{slug}`")
        );
        assert!(
            synthesizer.instructions.contains(
                "Every synthesizer turn MUST finish with exactly one `ralph emit` command"
            )
        );
        assert!(synthesizer.instructions.contains(
            "A gap merely mentioned in the summary does NOT satisfy the follow-up requirement."
        ));
        assert!(synthesizer.instructions.contains(
            "all planned research waves are complete, and no research follow-up tasks remain open"
        ));
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
        assert!(
            investigator
                .instructions
                .contains("On `debug.start` or `hypothesis.rejected`:")
        );
        assert!(investigator
            .instructions
            .contains("If the bug is already fixed, cannot be reproduced, or an existing debug note already captures the answer"));
        assert!(
            investigator
                .instructions
                .contains("Emit exactly one `hypothesis.test`")
        );
        assert!(
            investigator
                .instructions
                .contains("Use a real `ralph emit` command. Example:")
        );
        assert!(
            investigator
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(investigator.instructions.contains("`task_id`, `task_key`"));
        assert!(
            investigator
                .instructions
                .contains("On `hypothesis.confirmed`:")
        );
        assert!(investigator.instructions.contains("emit `fix.propose`"));
        assert!(investigator.instructions.contains("On `fix.verified`:"));
        assert!(
            investigator
                .instructions
                .contains("Emit exactly one `DEBUG_COMPLETE`")
        );
        assert!(
            investigator
                .instructions
                .contains("Use a real `ralph emit` command, not prose.")
        );
        assert!(
            investigator
                .instructions
                .contains("Do not end the turn with only prose")
        );
        assert!(investigator.instructions.contains(
            "❌ End the turn with only narration, document updates, or \"already complete\""
        ));
        assert!(
            investigator
                .instructions
                .contains("❌ Emit undeclared topics like `debug.start`")
        );
        assert!(
            investigator
                .instructions
                .contains("❌ Skip the event chain by doing fix or verification work inline")
        );

        let tester = config.hats.get("tester").expect("tester hat should exist");
        assert_eq!(tester.triggers, vec!["hypothesis.test".to_string()]);
        assert_eq!(
            tester.publishes,
            vec![
                "hypothesis.confirmed".to_string(),
                "hypothesis.rejected".to_string(),
            ]
        );
        assert!(
            tester
                .instructions
                .contains("If the hypothesis says the bug is already fixed")
        );
        assert!(
            tester
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(
            tester
                .instructions
                .contains("nearby adversarial or neighboring failure-path case")
        );
        assert!(tester.instructions.contains(
            "Use a real `ralph emit` command. The turn is incomplete until that command succeeds."
        ));

        let fixer = config.hats.get("fixer").expect("fixer hat should exist");
        assert_eq!(
            fixer.publishes,
            vec!["fix.applied".to_string(), "fix.blocked".to_string()]
        );
        assert_eq!(fixer.default_publishes.as_deref(), Some("fix.blocked"));
        assert!(!fixer.instructions.contains("Commit"));
        assert!(
            fixer
                .instructions
                .contains("❌ Make commits in this preset")
        );
        assert!(
            fixer
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(fixer.instructions.contains("ralph tools memory add"));
        assert!(fixer.instructions.contains(
            "Use a real `ralph emit` command. Writing code, notes, or tests alone does not complete the turn."
        ));
        assert!(fixer.instructions.contains(
            "If the proposed fix is already present in the code, do NOT rewrite the code or tests."
        ));
        assert!(
            fixer
                .instructions
                .contains("Write the required root-cause note in `.eval-sandbox/debug/counter.md`")
        );

        let verifier = config
            .hats
            .get("verifier")
            .expect("verifier hat should exist");
        assert_eq!(
            verifier.publishes,
            vec!["fix.verified".to_string(), "fix.failed".to_string()]
        );
        assert_eq!(verifier.default_publishes.as_deref(), Some("fix.failed"));
        assert!(
            verifier
                .instructions
                .contains("Re-run at least one nearby adversarial or failure-path case.")
        );
        assert!(
            verifier
                .instructions
                .contains("ralph tools task start <task_id>")
        );
        assert!(verifier.instructions.contains("`task_id`/`task_key`"));
        assert!(
            verifier
                .instructions
                .contains("The turn is incomplete until the `ralph emit` command succeeds.")
        );
    }

    #[test]
    fn test_preset_origin_guard_rejects_unknown_hats() {
        // Verify that public presets reject events from unknown hats via origin guard
        for preset in PRESETS.iter().filter(|p| p.public) {
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_config(&config);
            let cancellation = &config.event_loop.cancellation_promise;
            let completion = &config.event_loop.completion_promise;

            // Events from unknown hat "strategist" should be rejected in all presets
            let unknown_event = ralph_core::Event {
                topic: "test.topic".to_string(),
                payload: None,
                ts: "2025-01-01T00:00:00Z".to_string(),
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
            };

            match validate_event_origin(&unknown_event, &registry, cancellation, completion) {
                OriginCheck::Accepted => {
                    // Only acceptable when registry is empty (solo mode)
                    if !registry.is_empty() {
                        panic!(
                            "Preset '{}': unknown hat 'strategist' should be rejected",
                            preset.name
                        );
                    }
                }
                OriginCheck::Rejected { .. } => {} // Expected
            }
        }
    }

    #[test]
    fn test_ce_executor_required_events_is_report_done() {
        // Verify ce-executor uses report.done as completion gate (not mutually exclusive
        // branch events review.passed + review.complete which caused infinite loops)
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        assert_eq!(
            config.event_loop.required_events,
            &["report.done"],
            "ce-executor should require 'report.done' as its only completion gate event; \
             the old 'review.passed' + 'review.complete' gate causes infinite loops \
             because they are mutually exclusive branch events"
        );
    }

    #[test]
    fn test_ce_executor_required_events_is_report_done_for_root_preset() {
        // Mirror-drift guard: the embedded preset is loaded via `include_str!`
        // from `$OUT_DIR/presets/ce-executor.yml` (a copy made by build.rs from
        // `presets/en/ce-executor.yml`). If a future change edits the canonical
        // file but leaves a stale `$OUT_DIR` copy lying around, the `get_preset`
        // test above would still pass and the original infinite-loop bug would
        // silently return. Read the canonical file at test runtime so cargo test
        // fails whenever the two diverge on the completion gate.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let root_preset_path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("en")
            .join("ce-executor.yml");
        let root_content = std::fs::read_to_string(&root_preset_path).unwrap_or_else(|e| {
            panic!(
                "failed to read root ce-executor preset at {}: {}",
                root_preset_path.display(),
                e
            )
        });
        let config =
            RalphConfig::parse_yaml(&root_content).expect("root ce-executor YAML should parse");
        assert_eq!(
            config.event_loop.required_events,
            &["report.done"],
            "root ce-executor should require 'report.done' as its only completion gate; \
             mirror drift would let the old 'review.passed' + 'review.complete' gate \
             return without any embedded test noticing"
        );
    }

    #[test]
    fn test_ce_executor_executor_has_no_default_publishes() {
        // U2: executor must NOT have default_publishes — it must explicitly emit.
        // The no-event gate (U1) handles the "forgot to emit" case instead.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let executor = config
            .hats
            .get("executor")
            .expect("ce-executor should define executor hat");

        assert!(
            executor.default_publishes.is_none(),
            "executor must NOT have default_publishes; explicit emit is required"
        );
    }

    #[test]
    fn test_ce_executor_executor_has_no_default_publishes_for_root_preset() {
        // U2: root preset must match embedded preset
        let root_content = read_root_preset("ce-executor.yml");
        let config =
            RalphConfig::parse_yaml(&root_content).expect("root ce-executor YAML should parse");
        let executor = config
            .hats
            .get("executor")
            .expect("root ce-executor should define executor hat");

        assert!(
            executor.default_publishes.is_none(),
            "root ce-executor executor must have no default_publishes"
        );
    }

    #[test]
    fn test_ce_executor_publish_chain_origin_compatible() {
        // Verify ce-executor's normal publish chain survives origin guard
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let registry = HatRegistry::from_config(&config);
        let cancellation = &config.event_loop.cancellation_promise;
        let completion = &config.event_loop.completion_promise;

        // ce-executor's expected publish chain (using actual hat_keys from YAML):
        // coordinator(work.ready) -> executor(work.done) -> review-coordinator(review.wave.ready)
        //   -> dimension-reviewer(review.dimension.done) -> review-synthesizer(review.passed)
        //   -> plan-gate(queue.advance OR plan.complete) -> shipper(REVIEW_COMPLETE)
        //   -> reporter(report.done, LOOP_COMPLETE)
        //
        // `report.done` is the required_events completion gate, so it must appear in
        // the chain — otherwise the gate event would never fire and the original
        // infinite-loop bug returns even with `required_events: ["report.done"]`.
        let chain_publishes: Vec<(&str, &str)> = vec![
            ("coordinator", "work.ready"),
            ("executor", "work.done"),
            ("review-coordinator", "review.wave.ready"),
            ("dimension-reviewer", "review.dimension.done"),
            ("review-synthesizer", "review.passed"),
            ("plan-gate", "queue.advance"),
            ("plan-gate", "plan.complete"),
            ("plan-gate", "plan.blocked"),
            ("fixer", "fix.exhausted"),
            ("debug-resolver", "fix.plan.ready"),
            ("debug-resolver", "debug.exhausted"),
            ("debug-resolver", "plan.blocked"),
            ("shipper", "REVIEW_COMPLETE"),
            ("reporter", "report.done"),
            ("reporter", "LOOP_COMPLETE"),
        ];

        for (hat_name, expected_topic) in &chain_publishes {
            let event = ralph_core::Event {
                topic: expected_topic.to_string(),
                payload: None,
                ts: "2025-01-01T00:00:00Z".to_string(),
                hat: Some(hat_name.to_string()),
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
            };

            let result = validate_event_origin(&event, &registry, cancellation, completion);
            assert_eq!(
                result,
                OriginCheck::Accepted,
                "ce-executor: hat '{}' should be able to publish '{}', got: {:?}",
                hat_name,
                expected_topic,
                result
            );
        }
    }

    /// Helper: read a non-embedded root preset YAML by relative path.
    ///
    /// Picks the right canonical subdirectory based on filename suffix:
    /// `*`-zh.yml → `presets/zh/`, anything else → `presets/en/`.
    fn read_root_preset(filename: &str) -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let subdir = if filename.ends_with("-zh.yml") {
            "zh"
        } else {
            "en"
        };
        let path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join(subdir)
            .join(filename);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read root preset at {}: {}", path.display(), e))
    }

    fn read_root_schema(filename: &str) -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("schemas")
            .join(filename);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read root schema at {}: {}", path.display(), e))
    }

    #[test]
    fn test_ce_executor_zh_required_events_is_report_done() {
        // Happy-path: the Chinese ce-executor-zh preset must use "report.done"
        // as its sole completion gate, matching the English root preset.
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        assert_eq!(
            config.event_loop.required_events,
            &["report.done"],
            "ce-executor-zh should require 'report.done' as its only completion gate"
        );
    }

    #[test]
    fn test_ce_executor_zh_reporter_publishes_report_done_and_loop_complete() {
        // Regression: the Chinese preset's reporter hat must declare both
        // "report.done" (completion gate) and "LOOP_COMPLETE" (terminal promise).
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        let reporter = config
            .hats
            .get("reporter")
            .expect("ce-executor-zh must define a 'reporter' hat");
        assert!(
            reporter.publishes.iter().any(|p| p == "report.done"),
            "ce-executor-zh 'reporter' hat must declare 'report.done' in publishes. \
             current publishes: {:?}",
            reporter.publishes
        );
        assert!(
            reporter.publishes.iter().any(|p| p == "LOOP_COMPLETE"),
            "ce-executor-zh 'reporter' hat must declare 'LOOP_COMPLETE' in publishes. \
             current publishes: {:?}",
            reporter.publishes
        );
    }

    #[test]
    fn test_ce_executor_en_and_zh_completion_gate_consistent() {
        // Regression: English root preset, embedded mirror, and Chinese root preset
        // must all agree on the completion gate. If any diverges, the test fails.
        let en_root = read_root_preset("ce-executor.yml");
        let zh_root = read_root_preset("ce-executor-zh.yml");

        let en_config =
            RalphConfig::parse_yaml(&en_root).expect("English ce-executor YAML should parse");
        let zh_config =
            RalphConfig::parse_yaml(&zh_root).expect("Chinese ce-executor-zh YAML should parse");

        // Embedded mirror must match English root (sync-embedded-files.sh contract)
        let embedded_preset =
            get_preset("ce-executor").expect("ce-executor embedded preset should exist");
        let embedded_config = RalphConfig::parse_yaml(embedded_preset.content)
            .expect("embedded ce-executor YAML should parse");

        assert_eq!(
            embedded_config.event_loop.required_events, en_config.event_loop.required_events,
            "Embedded mirror must match English root preset required_events"
        );
        assert_eq!(
            zh_config.event_loop.required_events, en_config.event_loop.required_events,
            "Chinese ce-executor-zh must match English ce-executor required_events"
        );
    }

    #[test]
    fn test_ce_executor_reporter_publishes_report_done() {
        // Static-config guard for the completion-gate event. The chain test above
        // proves the event is origin-compatible at runtime, but `required_events`
        // and `hat.publishes` are independent YAML fields. If a future refactor
        // narrows `reporter.publishes` to just `["LOOP_COMPLETE"]` (or anything
        // that drops `report.done`), the gate event would never fire and the
        // infinite-loop bug would return even with `required_events: ["report.done"]`.
        // Reading the static config catches that case at unit-test time.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let reporter = config
            .hats
            .get("reporter")
            .expect("ce-executor must define a 'reporter' hat");
        assert!(
            reporter.publishes.iter().any(|p| p == "report.done"),
            "ce-executor 'reporter' hat must declare 'report.done' in `publishes` to \
             satisfy the required_events completion gate. current publishes: {:?}",
            reporter.publishes
        );
    }

    #[test]
    fn test_ce_executor_forbids_agent_branch_creation() {
        // Guard: ce-executor must explicitly tell the agent NOT to create, switch,
        // or rename branches, and NOT to create worktrees. Branching is reserved
        // for the user via `ralph run --worktree`; the orchestrator handles it
        // before the agent activates. The agent improvising a "git checkout -b
        // feat/plan-name" or "git worktree add ..." was the original bug — see
        // git history for "fix: ce-executor 禁建分支".
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;

        // Top-level guardrail must carry the prohibition
        assert!(
            content.contains("NEVER create, switch, or rename branches")
                && content.contains("`git checkout -b`")
                && content.contains("`git worktree add`"),
            "ce-executor guardrails must explicitly forbid branch creation by the \
             agent. Run ./scripts/sync-embedded-files.sh if the canonical file has \
             the policy but the embedded mirror does not."
        );

        // Per-hat 'Environment Setup' / 'Environment Check' sections must each
        // carry the Branch / Worktree Policy block. Coordinator must not delegate
        // branch creation to executor; executor must not run git checkout -b.
        for hat in ["coordinator", "executor"] {
            assert!(
                content.contains(&format!("{}:\n", hat))
                    || content.contains(&format!("  {}:\n", hat)),
                "ce-executor must define a '{}' hat section",
                hat
            );
        }

        // The "If not on a feature branch, create one (e.g., `feat/plan-name`)"
        // line is the exact regression that caused the bug. It must be absent.
        assert!(
            !content.contains("create one (e.g., `feat/plan-name`)"),
            "ce-executor must NOT instruct the executor to auto-create a feature \
             branch. Branching is reserved for `ralph run --worktree`."
        );
        assert!(
            !content.contains("Do not create branches (Executor handles that)"),
            "ce-executor must NOT defer branch creation to the executor. The \
             executor also does not create branches in this preset."
        );
    }

    #[test]
    fn test_autoresearch_forbids_agent_branch_creation() {
        // Guard: autoresearch must NOT tell the strategist hat to run
        // `git checkout -b autoresearch/...` during fresh-session setup.
        // Branching is reserved for the user via `ralph run --worktree`.
        // Regression: the original preset had step 2 of "Fresh Session" read
        // "Create a branch: `git checkout -b autoresearch/<goal-slug>-$(date +%Y%m%d)`"
        // which the agent dutifully executed, polluting the user's branch.
        let preset = get_preset("autoresearch").expect("autoresearch preset should exist");
        let content = preset.content;

        // Top-level guardrail carries the prohibition
        assert!(
            content.contains("NEVER create, switch, or rename branches")
                && content.contains("`git checkout -b`")
                && content.contains("`git worktree add`"),
            "autoresearch guardrails must explicitly forbid branch creation by the \
             agent. Run ./scripts/sync-embedded-files.sh if the canonical file has \
             the policy but the embedded mirror does not."
        );

        // Strategist's Fresh Session section must contain a Branch / Worktree
        // Policy block instead of the old "Create a branch" instruction.
        assert!(
            content.contains("Branch / Worktree Policy (HARD RULE)"),
            "autoresearch strategist must carry a Branch / Worktree Policy block in \
             the Fresh Session section."
        );

        // The exact regression line must be absent.
        assert!(
            !content.contains("git checkout -b autoresearch/<goal-slug>"),
            "autoresearch must NOT tell the strategist to run \
             `git checkout -b autoresearch/<goal-slug>-...`. Branching is reserved \
             for `ralph run --worktree`."
        );

        // The Chinese translation preset must stay in parity with English
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let zh_path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("zh")
            .join("autoresearch-zh.yml");
        let zh_content = std::fs::read_to_string(&zh_path).unwrap_or_else(|e| {
            panic!(
                "failed to read autoresearch-zh preset at {}: {}",
                zh_path.display(),
                e
            )
        });
        assert!(
            zh_content.contains("绝对禁止")
                && zh_content.contains("git checkout -b")
                && zh_content.contains("git worktree add"),
            "autoresearch-zh must translate the Branch / Worktree Policy so docs \
             stay in sync with the English preset."
        );
        assert!(
            !zh_content.contains("git checkout -b autoresearch/<goal-slug>"),
            "autoresearch-zh must NOT contain the old `git checkout -b \
             autoresearch/<goal-slug>` instruction either."
        );
    }

    #[test]
    fn test_ce_executor_dimension_reviewer_timeout_is_900() {
        // R1: dimension-reviewer must have explicit timeout to avoid default 300s.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let reviewer = config
            .hats
            .get("dimension-reviewer")
            .expect("ce-executor must define a 'dimension-reviewer' hat");
        assert_eq!(
            reviewer.timeout,
            Some(1800),
            "dimension-reviewer timeout must be explicitly set to 1800 seconds"
        );
    }

    #[test]
    fn test_ce_executor_root_preset_matches_embedded() {
        // Single-source-of-truth guard: the canonical preset and its embedded
        // copy (made by `build.rs` from `presets/manifest.yml`) must stay in sync.
        let root_content = read_root_preset("ce-executor.yml");
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        assert_eq!(
            root_content, preset.content,
            "Canonical presets/en/ce-executor.yml must match the embedded copy in $OUT_DIR/presets/. \
             The build script copies the canonical file on every change; if this fails, \
             `cargo clean -p ralph-cli && cargo build` will refresh it."
        );
    }

    #[test]
    fn test_ce_executor_zh_dimension_reviewer_timeout_is_900() {
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        let reviewer = config
            .hats
            .get("dimension-reviewer")
            .expect("ce-executor-zh must define a 'dimension-reviewer' hat");
        assert_eq!(
            reviewer.timeout,
            Some(1800),
            "ce-executor-zh dimension-reviewer timeout must be explicitly set to 1800 seconds"
        );
    }

    #[test]
    fn test_ce_executor_has_hard_commit_cadence() {
        // R3: executor must have hard commit cadence rule.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        assert!(
            content.contains("Commit Cadence (HARD RULE)"),
            "ce-executor must contain 'Commit Cadence (HARD RULE)'"
        );
        assert!(
            content.contains("Do NOT batch multiple U-IDs"),
            "ce-executor must forbid batching multiple U-IDs"
        );
    }

    #[test]
    fn test_ce_executor_has_preflight_contract() {
        // R4: executor must validate hard prerequisites before implementation.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        assert!(
            content.contains("Preflight Contract (HARD RULE)"),
            "ce-executor must contain 'Preflight Contract (HARD RULE)'"
        );
        assert!(
            content.contains("preflight check failed"),
            "ce-executor must reference preflight check failure"
        );
    }

    #[test]
    fn test_ce_executor_zh_has_hard_commit_cadence() {
        let content = read_root_preset("ce-executor-zh.yml");
        assert!(
            content.contains("提交节奏（硬性规则）"),
            "ce-executor-zh must contain hard commit cadence rule"
        );
        assert!(
            content.contains("禁止将多个 U-ID 合并为一个 commit"),
            "ce-executor-zh must forbid batching multiple U-IDs"
        );
    }

    #[test]
    fn test_ce_executor_zh_has_preflight_contract() {
        let content = read_root_preset("ce-executor-zh.yml");
        assert!(
            content.contains("前置检查契约（硬性规则）"),
            "ce-executor-zh must contain preflight contract"
        );
        assert!(
            content.contains("preflight check failed"),
            "ce-executor-zh must reference preflight check failure"
        );
    }

    #[test]
    fn test_ce_executor_zh_review_coordinator_obligation_parity() {
        // 2026-06-08 fix parity: ZH preset must mirror the EN preset's
        // `obligations:` block with `conditional_must_emit` on
        // review-coordinator. Without this the ZH preset cannot
        // catch the U3/U4 failure mode (review-coordinator emits
        // review.passed for a 400-line diff, skipping the wave).
        let en = read_root_preset("ce-executor.yml");
        let zh = read_root_preset("ce-executor-zh.yml");
        let en_config = RalphConfig::parse_yaml(&en).expect("EN preset should parse");
        let zh_config = RalphConfig::parse_yaml(&zh).expect("ZH preset should parse");

        let en_rc = en_config
            .hats
            .get("review-coordinator")
            .expect("EN must define review-coordinator");
        let zh_rc = zh_config
            .hats
            .get("review-coordinator")
            .expect("ZH must define review-coordinator");

        // The structural fix requires `obligations:` on review-coordinator.
        assert!(
            !zh_rc.obligations.is_empty(),
            "ZH review-coordinator must declare at least one obligation (U3/U4 bug fix)"
        );

        // Each EN obligation on work.done / fix.applied must have a
        // matching ZH obligation on the same trigger topic.
        for en_obligation in &en_rc.obligations {
            let zh_obligation = zh_rc
                .obligations
                .iter()
                .find(|o| o.on_trigger == en_obligation.on_trigger)
                .unwrap_or_else(|| {
                    panic!(
                        "ZH review-coordinator must have an obligation for on_trigger={}",
                        en_obligation.on_trigger
                    )
                });
            // Each EN conditional on a given trigger must have a
            // matching ZH conditional (same predicate intent).
            assert_eq!(
                en_obligation.conditional_must_emit.len(),
                zh_obligation.conditional_must_emit.len(),
                "ZH obligation on '{}' must have the same number of conditional_must_emit entries as EN ({})",
                en_obligation.on_trigger,
                en_obligation.conditional_must_emit.len()
            );
            // The strict set for each EN conditional must be review.wave.ready
            // (the same set ZH emits when an EN-style tightening applies).
            for (en_cond, zh_cond) in en_obligation
                .conditional_must_emit
                .iter()
                .zip(zh_obligation.conditional_must_emit.iter())
            {
                assert_eq!(
                    en_cond.must_emit_any_of, zh_cond.must_emit_any_of,
                    "ZH obligation on '{}' conditional must_emit_any_of must match EN",
                    en_obligation.on_trigger
                );
            }
        }

        // ZH instructions must contain the HARD RULE段 and skip_reason audit mention.
        let zh_instructions = zh_rc.instructions.as_str();
        assert!(
            zh_instructions.contains("HARD RULE"),
            "ZH review-coordinator instructions must include HARD RULE段"
        );
        assert!(
            zh_instructions.contains("skip_reason"),
            "ZH review-coordinator instructions must mention skip_reason audit field"
        );
        // ZH should NOT have a soft default_publishes兜底 (the strict
        // obligation path is in charge; keeping both would be confusing
        // and could mask enforcement).
        assert!(
            zh_rc.default_publishes.is_none(),
            "ZH review-coordinator must NOT have default_publishes — the strict obligation path replaces it"
        );
    }

    #[test]
    fn test_ce_executor_plan_gate_exists_and_routes_correctly() {
        // R1-R4: plan-gate must exist, must subscribe to review.passed + review.complete,
        // must publish queue.advance / plan.complete / plan.blocked, and must NOT
        // listen to fix.applied.
        // 2026-06-04 plan U5: plan-gate must ALSO trigger on work.failed so
        // failure events from coordinator/executor route to plan.blocked
        // instead of falling back to Ralph.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let plan_gate = config
            .hats
            .get("plan-gate")
            .expect("ce-executor must define a 'plan-gate' hat");
        assert!(
            plan_gate.triggers.contains(&"review.passed".to_string()),
            "plan-gate must trigger on review.passed"
        );
        assert!(
            plan_gate.triggers.contains(&"review.complete".to_string()),
            "plan-gate must trigger on review.complete"
        );
        assert!(
            plan_gate.triggers.contains(&"work.failed".to_string()),
            "plan-gate must trigger on work.failed so failures route to plan.blocked (U5)"
        );
        assert!(
            plan_gate.publishes.contains(&"queue.advance".to_string()),
            "plan-gate must publish queue.advance"
        );
        assert!(
            plan_gate.publishes.contains(&"plan.complete".to_string()),
            "plan-gate must publish plan.complete"
        );
        assert!(
            plan_gate.publishes.contains(&"plan.blocked".to_string()),
            "plan-gate must publish plan.blocked"
        );
        assert_eq!(
            plan_gate.default_publishes.as_deref(),
            Some("plan.blocked"),
            "plan-gate default_publishes should be plan.blocked"
        );
        assert!(
            !plan_gate.triggers.contains(&"fix.applied".to_string()),
            "plan-gate must NOT listen to fix.applied"
        );
    }

    #[test]
    fn test_ce_executor_shipper_triggers_finalization_only() {
        // R6: Shipper must only trigger on finalization inputs, not directly on review.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let shipper = config
            .hats
            .get("shipper")
            .expect("ce-executor must define a 'shipper' hat");
        assert!(
            !shipper.triggers.contains(&"review.passed".to_string()),
            "shipper must NOT trigger on review.passed"
        );
        assert!(
            !shipper.triggers.contains(&"review.complete".to_string()),
            "shipper must NOT trigger on review.complete"
        );
        assert!(
            shipper.triggers.contains(&"plan.complete".to_string()),
            "shipper must trigger on plan.complete"
        );
        assert!(
            shipper.triggers.contains(&"plan.blocked".to_string()),
            "shipper must trigger on plan.blocked"
        );
        assert!(
            shipper.triggers.contains(&"debug.exhausted".to_string()),
            "shipper must trigger on debug.exhausted"
        );
        assert!(
            !shipper.triggers.contains(&"fix.exhausted".to_string()),
            "shipper should NOT trigger on fix.exhausted in normal topology; debug-resolver handles that path"
        );
    }

    #[test]
    fn test_ce_executor_executor_publishes_excludes_queue_advance() {
        // KTD4: executor no longer publishes queue.advance; plan-gate owns advancement.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let executor = config
            .hats
            .get("executor")
            .expect("ce-executor must define an 'executor' hat");
        assert!(
            !executor.publishes.contains(&"queue.advance".to_string()),
            "executor must NOT publish queue.advance; plan-gate owns queue advancement"
        );
    }

    #[test]
    fn test_ce_executor_work_done_field_consistency() {
        // R6/R7/R13 (2026-06-04 plan U4): work.done required fields must be the
        // same set in execution contract and event policy schema. Drift
        // between these layers lets an agent emit a payload that passes one
        // gate and fails the other, which is exactly the false-positive
        // trap the contract is supposed to prevent.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let contracts = config
            .event_loop
            .execution_contracts
            .as_ref()
            .expect("ce-executor should have execution_contracts");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor should have event_policy");

        let contract_rule = contracts
            .rules
            .get("work.done")
            .expect("ce-executor execution contract should define work.done rule");
        let contract_fields: std::collections::BTreeSet<&str> = contract_rule
            .require_payload_fields
            .iter()
            .map(String::as_str)
            .collect();

        let schema = policy
            .schemas
            .get("work.done")
            .expect("ce-executor event_policy should define work.done schema");
        let schema_fields: std::collections::BTreeSet<&str> =
            schema.required_fields.iter().map(String::as_str).collect();

        // Every field the contract requires must also be in the schema
        // (the schema is the second gate and must not silently accept
        // fields the contract rejects).
        let missing_in_schema: Vec<&&str> = contract_fields
            .iter()
            .filter(|f| !schema_fields.contains(*f))
            .collect();
        assert!(
            missing_in_schema.is_empty(),
            "work.done: contract requires {:?} but event_policy schema is missing {:?}. \
             A payload that fails the contract must not silently pass the schema gate. \
             See docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md (U4).",
            contract_fields,
            missing_in_schema
        );

        // The required set must be exactly the plan's documented minimum
        // (plan_name, plan_path, task_id, task_key, step + 2026-06-08 fix:
        // commit_count + changed_lines for the review-coordinator gate).
        // If a future change drops one of these fields, contract validation
        // will weaken silently.
        let required_minimum: std::collections::BTreeSet<&str> = [
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "commit_count",
            "changed_lines",
        ]
        .iter()
        .copied()
        .collect();
        assert_eq!(
            contract_fields, required_minimum,
            "ce-executor work.done contract must require exactly \
             {{plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines}}"
        );
        assert_eq!(
            schema_fields, required_minimum,
            "ce-executor work.done event_policy schema must require exactly \
             {{plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines}}"
        );

        // Executor instructions must mention every required field. Use
        // concise token search (not exact paragraph match) so prose edits
        // do not break the test.
        let executor = config
            .hats
            .get("executor")
            .expect("ce-executor should have executor hat");
        let instructions = executor.instructions.as_str();
        for field in &required_minimum {
            assert!(
                instructions.contains(field),
                "executor instructions must mention required work.done field '{}'",
                field
            );
        }

        // Review-coordinator read-state must mention every required field
        // that review-coordinator needs from work.done.
        let reviewer = config
            .hats
            .get("review-coordinator")
            .expect("ce-executor should have review-coordinator hat");
        let reviewer_instructions = reviewer.instructions.as_str();
        for field in &required_minimum {
            assert!(
                reviewer_instructions.contains(field),
                "review-coordinator instructions must mention required work.done field '{}'",
                field
            );
        }
    }

    #[test]
    fn test_ce_executor_zh_work_done_field_consistency() {
        // R6/R7/R13 ZH parity: the Chinese preset must keep the same work.done
        // field set as the English preset.
        let en = read_root_preset("ce-executor.yml");
        let zh = read_root_preset("ce-executor-zh.yml");
        let en_config = RalphConfig::parse_yaml(&en).expect("English preset should parse");
        let zh_config = RalphConfig::parse_yaml(&zh).expect("Chinese preset should parse");

        let en_contracts = en_config
            .event_loop
            .execution_contracts
            .as_ref()
            .expect("EN ce-executor should have execution_contracts");
        let en_rule = en_contracts
            .rules
            .get("work.done")
            .expect("EN work.done contract should exist");
        let en_required: std::collections::BTreeSet<&str> = en_rule
            .require_payload_fields
            .iter()
            .map(String::as_str)
            .collect();

        let zh_contracts = zh_config
            .event_loop
            .execution_contracts
            .as_ref()
            .expect("ZH ce-executor should have execution_contracts");
        let zh_rule = zh_contracts
            .rules
            .get("work.done")
            .expect("ZH work.done contract should exist");
        let zh_required: std::collections::BTreeSet<&str> = zh_rule
            .require_payload_fields
            .iter()
            .map(String::as_str)
            .collect();

        assert_eq!(
            en_required, zh_required,
            "ZH ce-executor work.done contract must require the same field set as EN"
        );

        let zh_policy = zh_config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ZH ce-executor should have event_policy");
        let zh_schema = zh_policy
            .schemas
            .get("work.done")
            .expect("ZH work.done schema should exist");
        let zh_schema_fields: std::collections::BTreeSet<&str> = zh_schema
            .required_fields
            .iter()
            .map(String::as_str)
            .collect();

        for field in &en_required {
            assert!(
                zh_schema_fields.contains(field),
                "ZH ce-executor work.done event_policy schema must require '{}' (EN contract does)",
                field
            );
        }
    }

    #[test]
    fn test_ce_executor_failure_topics_accept_reason_only_payloads() {
        // Early failure paths can happen before a plan is parsed or a task is
        // selected, so they may not have plan_name/task_id/task_key/step yet.
        // The schema must allow those failures to reach plan-gate instead of
        // rejecting them at the event policy layer.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor should have event_policy");

        for topic in ["work.failed", "plan.blocked"] {
            let schema = policy
                .schemas
                .get(topic)
                .unwrap_or_else(|| panic!("ce-executor should define {} schema", topic));
            assert_eq!(
                schema.required_fields,
                vec!["reason".to_string()],
                "{} must require only reason; additional task/plan fields are optional context",
                topic
            );
        }

        let coordinator = config
            .hats
            .get("coordinator")
            .expect("ce-executor should have coordinator hat");
        assert!(
            coordinator.instructions.contains(r#"payload: `{"reason":"#),
            "coordinator documents reason-only work.failed early failure payloads"
        );
    }

    #[test]
    fn test_ce_executor_zh_failure_topics_match_en_reason_only_schema() {
        let en = read_root_preset("ce-executor.yml");
        let zh = read_root_preset("ce-executor-zh.yml");
        let en_config = RalphConfig::parse_yaml(&en).expect("English preset should parse");
        let zh_config = RalphConfig::parse_yaml(&zh).expect("Chinese preset should parse");

        let en_policy = en_config
            .event_loop
            .event_policy
            .as_ref()
            .expect("EN ce-executor should have event_policy");
        let zh_policy = zh_config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ZH ce-executor should have event_policy");

        for topic in ["work.failed", "plan.blocked"] {
            let en_schema = en_policy
                .schemas
                .get(topic)
                .unwrap_or_else(|| panic!("EN ce-executor should define {} schema", topic));
            let zh_schema = zh_policy
                .schemas
                .get(topic)
                .unwrap_or_else(|| panic!("ZH ce-executor should define {} schema", topic));
            assert_eq!(
                zh_schema.required_fields, en_schema.required_fields,
                "ZH {} schema must match EN",
                topic
            );
            assert_eq!(
                zh_schema.required_fields,
                vec!["reason".to_string()],
                "ZH {} schema must allow reason-only early failures",
                topic
            );
        }
    }

    #[test]
    fn test_ce_executor_zh_plan_gate_matches_en() {
        let en = read_root_preset("ce-executor.yml");
        let zh = read_root_preset("ce-executor-zh.yml");

        let en_config = RalphConfig::parse_yaml(&en).expect("English preset should parse");
        let zh_config = RalphConfig::parse_yaml(&zh).expect("Chinese preset should parse");

        let en_gate = en_config
            .hats
            .get("plan-gate")
            .expect("English preset must have plan-gate");
        let zh_gate = zh_config
            .hats
            .get("plan-gate")
            .expect("Chinese preset must have plan-gate");

        assert_eq!(
            en_gate.triggers, zh_gate.triggers,
            "plan-gate triggers must match between EN and ZH"
        );
        assert_eq!(
            en_gate.publishes, zh_gate.publishes,
            "plan-gate publishes must match between EN and ZH"
        );
        assert_eq!(
            en_gate.default_publishes, zh_gate.default_publishes,
            "plan-gate default_publishes must match between EN and ZH"
        );
    }

    #[test]
    fn test_ce_executor_reporter_defensive_plan_check() {
        // R8: Reporter instructions must contain a defensive plan completion check.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        assert!(
            content.contains("Defensive plan completion check")
                || content.contains("defensive plan completion check"),
            "reporter instructions must contain a defensive plan completion check"
        );
        assert!(
            content.contains("plan.md") && content.contains("progress.md"),
            "reporter must reference plan.md and progress.md for the defensive check"
        );
    }

    #[test]
    fn test_ce_executor_verdict_gate_targets_review_complete() {
        // R10: verdict_gate must check REVIEW_COMPLETE (not review.complete) because
        // REVIEW_COMPLETE carries pass_or_fail; review.complete carries verdict.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let gate = config
            .event_loop
            .verdict_gate
            .as_ref()
            .expect("ce-executor must have a verdict_gate");
        assert_eq!(
            gate.topic, "REVIEW_COMPLETE",
            "verdict_gate topic must be REVIEW_COMPLETE (uppercase) to match shipper output"
        );
        assert_eq!(
            gate.fail_field, "pass_or_fail",
            "verdict_gate fail_field must be pass_or_fail"
        );
        assert_eq!(
            gate.fail_value, "fail",
            "verdict_gate fail_value must be fail"
        );
    }

    #[test]
    fn test_ce_executor_dimension_reviewer_passes_through_task_correlation() {
        // R13: dimension-reviewer must read and publish task_id/task_key/step so
        // plan-gate can correlate wave results with the original task.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let dim_section = content
            .split("dimension-reviewer:")
            .nth(1)
            .expect("ce-executor must have dimension-reviewer section");
        assert!(
            dim_section.contains("task_id")
                && dim_section.contains("task_key")
                && dim_section.contains("step"),
            "dimension-reviewer instructions must reference task_id, task_key, and step"
        );
    }

    #[test]
    fn test_ce_executor_shipper_commit_only_on_plan_complete() {
        // R14: shipper must NOT commit or mark plan completed on plan.blocked/debug.exhausted.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let shipper_section = content
            .split("shipper:")
            .nth(1)
            .expect("ce-executor must have shipper section");
        assert!(
            shipper_section.contains("plan.complete ONLY")
                || shipper_section
                    .contains("Only execute this section when triggered by `plan.complete`"),
            "shipper must gate commit and plan-status update to plan.complete only"
        );
        assert!(
            shipper_section.contains("plan.blocked") && shipper_section.contains("debug.exhausted"),
            "shipper must reference plan.blocked and debug.exhausted in its guarded sections"
        );
    }

    #[test]
    fn test_ce_executor_executor_reads_reviewed_task_id_on_queue_advance() {
        // R15: executor must read reviewed_task_id/reviewed_task_key on queue.advance,
        // NOT task_id/task_key, to avoid confusing the reviewed step's tasks with the
        // next step's tasks.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let exec_section = content
            .split("  executor:\n")
            .nth(1)
            .expect("ce-executor must have executor section");
        assert!(
            exec_section.contains("reviewed_task_id") && exec_section.contains("reviewed_task_key"),
            "executor queue.advance instructions must reference reviewed_task_id and reviewed_task_key"
        );
        assert!(
            !exec_section.contains("payload may omit `task_id`"),
            "executor must NOT say payload may omit task_id on queue.advance"
        );
    }

    #[test]
    fn test_ce_executor_shipper_simplify_check_gated_to_plan_complete() {
        // R16: shipper's simplify check must be gated to plan.complete only.
        // On plan.blocked or debug.exhausted, the state is not shippable — simplify is inappropriate.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let shipper_section = content
            .split("shipper:")
            .nth(1)
            .expect("ce-executor must have shipper section");
        assert!(
            shipper_section.contains("Simplify Check (plan.complete ONLY)")
                || shipper_section.contains("Only execute on `plan.complete`"),
            "shipper must gate simplify check to plan.complete only"
        );
    }

    #[test]
    fn test_ce_executor_zh_verdict_gate_targets_review_complete() {
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        let gate = config
            .event_loop
            .verdict_gate
            .as_ref()
            .expect("ce-executor-zh must have a verdict_gate");
        assert_eq!(
            gate.topic, "REVIEW_COMPLETE",
            "ce-executor-zh verdict_gate topic must be REVIEW_COMPLETE"
        );
    }

    #[test]
    fn test_ce_executor_zh_dimension_reviewer_passes_through_task_correlation() {
        let content = read_root_preset("ce-executor-zh.yml");
        let dim_section = content
            .split("dimension-reviewer:")
            .nth(1)
            .expect("ce-executor-zh must have dimension-reviewer section");
        assert!(
            dim_section.contains("task_id")
                && dim_section.contains("task_key")
                && dim_section.contains("step"),
            "ce-executor-zh dimension-reviewer must reference task_id, task_key, step"
        );
    }

    #[test]
    fn test_ce_executor_zh_shipper_commit_only_on_plan_complete() {
        let content = read_root_preset("ce-executor-zh.yml");
        let shipper_section = content
            .split("shipper:")
            .nth(1)
            .expect("ce-executor-zh must have shipper section");
        assert!(
            shipper_section.contains("仅限 plan.complete")
                || shipper_section.contains("plan.complete 时"),
            "ce-executor-zh shipper must gate commit to plan.complete only"
        );
    }

    #[test]
    fn test_ce_executor_fixer_reads_task_correlation_fields() {
        // R17: fixer must read task_id/task_key/step from review.failed payload
        // so that fix.applied / fix.exhausted can carry them downstream.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let fixer = config
            .hats
            .get("fixer")
            .expect("ce-executor must define fixer");
        assert!(
            fixer.default_publishes.is_none(),
            "fixer must NOT use default_publishes; fix.exhausted requires contextual payload"
        );
        let content = preset.content;
        let fixer_section = content
            .split("fixer:")
            .nth(1)
            .expect("ce-executor must have fixer section");
        assert!(
            fixer_section.contains("task_id")
                && fixer_section.contains("task_key")
                && fixer_section.contains("step"),
            "fixer Read State must reference task_id, task_key, step from review.failed payload"
        );
        assert!(
            fixer_section.contains("MUST explicitly publish")
                || fixer_section.contains("必须显式发布"),
            "fixer instructions must require explicit fix.applied/fix.exhausted publishing"
        );
    }

    #[test]
    fn test_ce_executor_coordinator_work_ready_includes_task_correlation() {
        // R18: coordinator must publish task_id/task_key/step in work.ready payload
        // so that executor (including trivial path) can forward them to work.done.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let coord_section = content
            .split("\n  coordinator:\n")
            .nth(1)
            .expect("ce-executor must have coordinator section");
        assert!(
            coord_section.contains("task_id")
                && coord_section.contains("task_key")
                && coord_section.contains("step"),
            "coordinator Event Publishing must include task_id, task_key, step in work.ready payload"
        );
    }

    #[test]
    fn test_ce_executor_trivial_path_includes_task_correlation() {
        // R19: executor trivial path must publish task_id/task_key/step in work.done
        // so review-coordinator can correlate the review with the right task.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let exec_section = content
            .split("  executor:\n")
            .nth(1)
            .expect("ce-executor must have executor section");
        let trivial_start = exec_section
            .find("Trivial")
            .expect("executor must have Trivial section");
        let trivial_section = &exec_section[trivial_start..];
        assert!(
            trivial_section.contains("task_id")
                && trivial_section.contains("task_key")
                && trivial_section.contains("step"),
            "executor trivial path must include task_id, task_key, step in work.done payload"
        );
    }

    #[test]
    fn test_ce_executor_zh_fixer_reads_task_correlation_fields() {
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        let fixer = config
            .hats
            .get("fixer")
            .expect("ce-executor-zh must define fixer");
        assert!(
            fixer.default_publishes.is_none(),
            "ce-executor-zh fixer must NOT use default_publishes"
        );
        let fixer_section = content
            .split("fixer:")
            .nth(1)
            .expect("ce-executor-zh must have fixer section");
        assert!(
            fixer_section.contains("task_id")
                && fixer_section.contains("task_key")
                && fixer_section.contains("step"),
            "ce-executor-zh fixer Read State must reference task_id, task_key, step from review.failed payload"
        );
    }

    #[test]
    fn test_ce_executor_zh_coordinator_work_ready_includes_task_correlation() {
        let content = read_root_preset("ce-executor-zh.yml");
        let coord_section = content
            .split("\n  coordinator:\n")
            .nth(1)
            .expect("ce-executor-zh must have coordinator section");
        assert!(
            coord_section.contains("task_id")
                && coord_section.contains("task_key")
                && coord_section.contains("step"),
            "ce-executor-zh coordinator Event Publishing must include task_id, task_key, step in work.ready payload"
        );
    }

    #[test]
    fn test_ce_executor_zh_trivial_path_includes_task_correlation() {
        let content = read_root_preset("ce-executor-zh.yml");
        let exec_section = content
            .split("\n  executor:\n")
            .nth(1)
            .expect("ce-executor-zh must have executor section");
        let trivial_start = exec_section
            .find("Trivial")
            .expect("ce-executor-zh executor must have Trivial section");
        let trivial_section = &exec_section[trivial_start..];
        assert!(
            trivial_section.contains("task_id")
                && trivial_section.contains("task_key")
                && trivial_section.contains("step"),
            "ce-executor-zh executor trivial path must include task_id, task_key, step in work.done payload"
        );
    }

    #[test]
    fn test_ce_executor_fixer_exhausted_early_exit_keeps_task_correlation() {
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;
        let fixer_section = content
            .split("fixer:")
            .nth(1)
            .expect("ce-executor must have fixer section");
        let exhausted_start = fixer_section
            .find("fix_round + 1 > 3")
            .expect("fixer must describe exhausted early exit");
        let exhausted_section = &fixer_section[exhausted_start..];
        assert!(
            exhausted_section.contains("task_id")
                && exhausted_section.contains("task_key")
                && exhausted_section.contains("step"),
            "fixer early fix.exhausted path must carry task_id, task_key, step"
        );
    }

    #[test]
    fn test_ce_executor_zh_fixer_exhausted_early_exit_keeps_task_correlation() {
        let content = read_root_preset("ce-executor-zh.yml");
        let fixer_section = content
            .split("fixer:")
            .nth(1)
            .expect("ce-executor-zh must have fixer section");
        let exhausted_start = fixer_section
            .find("fix_round + 1 > 3")
            .expect("ce-executor-zh fixer must describe exhausted early exit");
        let exhausted_section = &fixer_section[exhausted_start..];
        assert!(
            exhausted_section.contains("task_id")
                && exhausted_section.contains("task_key")
                && exhausted_section.contains("step"),
            "ce-executor-zh fixer early fix.exhausted path must carry task_id, task_key, step"
        );
    }

    /// Cross-check the Rust `PRESETS` array against `presets/manifest.yml`.
    ///
    /// `build.rs` reads the manifest to decide which yml files to copy into
    /// `$OUT_DIR`; this test makes sure the Rust side lists the same set of
    /// names. If a contributor adds a preset to one place but not the other,
    /// the inconsistency surfaces here (or, for the manifest-only case, as a
    /// build.rs panic).
    ///
    /// The test only runs on the build host where `presets/manifest.yml` is
    /// reachable at `CARGO_MANIFEST_DIR/../../presets/manifest.yml`. When the
    /// crate is built from a crates.io tarball the manifest does not exist and
    /// the test is skipped via the early return.
    #[test]
    fn presets_array_matches_manifest() {
        let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets")
            .join("manifest.yml");
        if !manifest_path.is_file() {
            eprintln!(
                "presets_array_matches_manifest: {} not on build host; skipping",
                manifest_path.display()
            );
            return;
        }
        let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
            panic!("failed to read {}: {}", manifest_path.display(), e);
        });
        let value: serde_yaml::Value =
            serde_yaml::from_str(&text).expect("manifest.yml must be valid YAML");
        let embedded = value
            .get("embedded")
            .and_then(|v| v.as_sequence())
            .expect("manifest.yml must have an `embedded:` sequence");
        let manifest_names: Vec<String> = embedded
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .expect("each entry under `embedded:` must be a string")
            })
            .collect();

        let mut rust_names: Vec<String> = PRESETS.iter().map(|p| p.name.to_string()).collect();
        rust_names.sort();
        let mut expected = manifest_names.clone();
        expected.sort();

        assert_eq!(
            rust_names, expected,
            "PRESETS array in src/presets.rs disagrees with presets/manifest.yml.\n\
             Either add the missing entry to the Rust array, or remove the extra one from \
             the manifest. See presets/manifest.yml for authoring rules."
        );
    }

    // ── U6: Builtin Authoring Maintenance Guard ───────────────────────────────

    /// U6: Template-only names must NOT appear in `preset_names()`.
    ///
    /// Templates that share names with presets (code-assist, debug, research, review)
    /// ARE preset names — that's intentional because the templates are based on those presets.
    /// But template-only names (minimal-linear, ce-executor-lite) are NOT preset names.
    #[test]
    fn test_template_only_names_not_in_preset_names() {
        // Template-only names that should NOT appear in preset_names()
        let template_only_names = ["minimal-linear", "ce-executor-lite"];
        let public_preset_names: std::collections::BTreeSet<String> =
            preset_names().iter().map(|s| s.to_string()).collect();

        for name in template_only_names {
            assert!(
                !public_preset_names.contains(name),
                "Template-only name '{}' must NOT appear in preset_names(); \
                 templates are authoring scaffolding, not builtin presets",
                name
            );
        }

        // Templates that share names with presets SHOULD appear in preset_names()
        let shared_names = ["code-assist", "debug", "research", "review"];
        for name in shared_names {
            assert!(
                public_preset_names.contains(name),
                "Shared template/preset name '{}' SHOULD appear in preset_names() \
                 because templates are based on that builtin preset",
                name
            );
        }
    }

    /// U6: All public preset names must appear in `presets/index.json`.
    #[test]
    fn test_public_preset_names_in_index_json() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets");
        let index_path = manifest_dir.join("index.json");
        if !index_path.is_file() {
            eprintln!(
                "test_public_preset_names_in_index_json: {} not on build host; skipping",
                index_path.display()
            );
            return;
        }

        let text = std::fs::read_to_string(&index_path).unwrap_or_else(|e| {
            panic!("failed to read {}: {}", index_path.display(), e)
        });
        let entries: Vec<serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("index.json must be valid JSON: {}", e));

        let index_names: std::collections::BTreeSet<String> = entries
            .iter()
            .map(|e| e.get("name").unwrap().as_str().unwrap().to_string())
            .collect();

        let public_names: std::collections::BTreeSet<String> =
            preset_names().iter().map(|s| s.to_string()).collect();

        let missing: Vec<_> = public_names
            .difference(&index_names)
            .collect();
        assert!(
            missing.is_empty(),
            "Public preset names missing from presets/index.json: {:?}. \
             All public presets must be listed in index.json.",
            missing
        );
    }

    /// U6: All `presets/index.json` entries must appear in zsh builtin completion values.
    #[test]
    fn test_index_json_entries_have_zsh_completion() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets");
        let index_path = manifest_dir.join("index.json");
        if !index_path.is_file() {
            eprintln!(
                "test_index_json_entries_have_zsh_completion: {} not on build host; skipping",
                index_path.display()
            );
            return;
        }

        let text = std::fs::read_to_string(&index_path).unwrap_or_else(|e| {
            panic!("failed to read {}: {}", index_path.display(), e)
        });
        let entries: Vec<serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("index.json must be valid JSON: {}", e));

        // Zsh completion values for builtin presets (from zsh plugin)
        // This must stay in sync with scripts/ralph-zsh-plugin.zsh
        let zsh_values: std::collections::BTreeSet<String> = [
            "builtin:ce-executor",
            "builtin:ce-executor-wave",
            "builtin:code-assist",
            "builtin:debug",
            "builtin:research",
            "builtin:review",
            "builtin:pdd-to-code-assist",
            "builtin:autoresearch",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        for entry in entries {
            let name = entry.get("name").unwrap().as_str().unwrap();
            let expected_zsh_value = format!("builtin:{}", name);
            assert!(
                zsh_values.contains(&expected_zsh_value),
                "Preset '{}' is in index.json but missing from zsh builtin completion values. \
                 Add '{}' to _RALPH_BUILTIN_HAT_VALUES in scripts/ralph-zsh-plugin.zsh",
                name,
                expected_zsh_value
            );
        }
    }

    /// U6: Hidden presets (merge-loop) must NOT appear in index.json.
    #[test]
    fn test_hidden_presets_not_in_index_json() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets");
        let index_path = manifest_dir.join("index.json");
        if !index_path.is_file() {
            eprintln!(
                "test_hidden_presets_not_in_index_json: {} not on build host; skipping",
                index_path.display()
            );
            return;
        }

        let text = std::fs::read_to_string(&index_path).unwrap_or_else(|e| {
            panic!("failed to read {}: {}", index_path.display(), e)
        });
        let entries: Vec<serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("index.json must be valid JSON: {}", e));

        let index_names: std::collections::BTreeSet<_> = entries
            .iter()
            .map(|e| e.get("name").unwrap().as_str().unwrap())
            .collect();

        // merge-loop is hidden and should NOT appear in index.json
        assert!(
            !index_names.contains("merge-loop"),
            "Hidden preset 'merge-loop' must NOT appear in presets/index.json"
        );
    }

    #[test]
    fn test_ce_executor_findings_include_task_id_isolation() {
        // Bug #2 regression: dimension-reviewer must write findings files that
        // include task_id so stale files from prior steps/presets do not串扰.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let content = preset.content;

        // dimension-reviewer instructions must reference task-id-scoped paths
        assert!(
            content.contains("findings-{dimension}-{task_id}.json"),
            "ce-executor dimension-reviewer must instruct findings-{{dimension}}-{{task_id}}.json"
        );

        // The old bare findings-{dimension}.json pattern must be gone (except as
        // a substring of the new, longer pattern).
        let old_pattern = "findings-{dimension}.json";
        let new_pattern = "findings-{dimension}-{task_id}.json";
        // Every occurrence of the old pattern must be part of the new pattern.
        for (idx, _) in content.match_indices(old_pattern) {
            let end = idx + old_pattern.len();
            assert!(
                content[..end].ends_with(new_pattern),
                "ce-executor still contains bare findings-{{dimension}}.json at offset {} — all findings paths must be task-id-scoped",
                idx
            );
        }
    }

    #[test]
    fn test_ce_executor_debug_resolver_exists_and_routes_correctly() {
        // U6: debug-resolver must exist, subscribe to fix.exhausted, and publish
        // fix.plan.ready / debug.exhausted / plan.blocked explicitly.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let resolver = config
            .hats
            .get("debug-resolver")
            .expect("ce-executor must define a 'debug-resolver' hat");

        assert_eq!(
            resolver.triggers,
            vec!["fix.exhausted".to_string()],
            "debug-resolver must trigger on fix.exhausted"
        );
        assert!(
            resolver.publishes.contains(&"fix.plan.ready".to_string()),
            "debug-resolver must publish fix.plan.ready"
        );
        assert!(
            resolver.publishes.contains(&"debug.exhausted".to_string()),
            "debug-resolver must publish debug.exhausted"
        );
        assert!(
            resolver.publishes.contains(&"plan.blocked".to_string()),
            "debug-resolver must publish plan.blocked"
        );
        assert!(
            resolver.default_publishes.is_none(),
            "debug-resolver must NOT use default_publishes; debug.exhausted requires contextual payload"
        );

        let inst = resolver.instructions.as_str();
        assert!(
            inst.contains("Investigate before fixing") || inst.contains("先调查再修复"),
            "debug-resolver instructions must state 'Investigate before fixing' or its Chinese equivalent"
        );
        assert!(
            inst.contains("causal chain gate"),
            "debug-resolver instructions must reference causal chain gate"
        );
        assert!(
            inst.contains("prediction"),
            "debug-resolver instructions must reference prediction"
        );
        assert!(
            inst.contains("assumption audit"),
            "debug-resolver instructions must reference assumption audit"
        );
        assert!(
            inst.contains("smart escalation"),
            "debug-resolver instructions must reference smart escalation"
        );
        assert!(
            inst.contains("NEVER create, switch, or rename branches")
                || inst.contains("MUST NOT create, switch, or rename branches")
                || (inst.contains("绝对禁止") && inst.contains("git checkout -b")),
            "debug-resolver instructions must forbid branch creation"
        );
        assert!(
            !resolver.publishes.contains(&"work.done".to_string()),
            "debug-resolver must not publish work.done"
        );
        assert!(
            inst.contains("MUST explicitly publish") || inst.contains("必须显式发布"),
            "debug-resolver instructions must require an explicit terminal handoff event"
        );
    }

    #[test]
    fn test_ce_executor_debug_resolver_forbids_branch_creation() {
        // U6: debug-resolver must not create branches, push, or create PRs.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let resolver = config
            .hats
            .get("debug-resolver")
            .expect("ce-executor must define a 'debug-resolver' hat");
        let inst = resolver.instructions.as_str();

        assert!(
            inst.contains("NEVER create, switch, or rename branches")
                || inst.contains("MUST NOT create, switch, or rename branches")
                || (inst.contains("绝对禁止") && inst.contains("git checkout -b")),
            "debug-resolver must explicitly forbid branch creation"
        );
        assert!(
            inst.contains("push to origin")
                || inst.contains("push 到 origin")
                || inst.contains("MUST NOT push to origin")
                || inst.contains("MUST NOT create pull requests")
                || inst.contains("创建 pull request")
                || inst.contains("create pull requests"),
            "debug-resolver must explicitly forbid push / PR creation"
        );
        assert!(
            !resolver.publishes.contains(&"work.done".to_string()),
            "debug-resolver must not publish work.done"
        );
    }

    #[test]
    fn test_ce_executor_executor_accepts_fix_plan_ready() {
        // U6: executor must accept fix.plan.ready and enter fix-plan execution mode.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let executor = config
            .hats
            .get("executor")
            .expect("ce-executor must define an 'executor' hat");

        assert!(
            executor.triggers.contains(&"fix.plan.ready".to_string()),
            "executor must trigger on fix.plan.ready"
        );
        assert!(
            executor.instructions.contains("FIX PLAN EXECUTION MODE"),
            "executor instructions must define FIX PLAN EXECUTION MODE"
        );
        assert!(
            executor.instructions.contains("root_cause_summary")
                && executor.instructions.contains("causal_chain")
                && executor.instructions.contains("recommended_tests")
                && executor.instructions.contains("fix_plan"),
            "executor fix-plan mode instructions must reference all fix.plan.ready payload fields"
        );
        assert!(
            !executor.publishes.contains(&"queue.advance".to_string()),
            "executor must NOT publish queue.advance"
        );
        assert_eq!(
            executor.default_publishes, None,
            "executor must have no default_publishes"
        );
    }

    #[test]
    fn test_ce_executor_shipper_handles_debug_exhausted_not_fix_exhausted() {
        // U6: shipper must trigger on debug.exhausted, not on fix.exhausted in normal topology.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");

        let shipper = config
            .hats
            .get("shipper")
            .expect("ce-executor must define a 'shipper' hat");

        assert!(
            shipper.triggers.contains(&"debug.exhausted".to_string()),
            "shipper must trigger on debug.exhausted"
        );
        assert!(
            !shipper.triggers.contains(&"fix.exhausted".to_string()),
            "shipper must NOT trigger on fix.exhausted in normal topology; debug-resolver handles that path"
        );
        assert!(
            shipper.instructions.contains("`debug.exhausted`")
                && shipper.instructions.contains("pass_or_fail: \"fail\""),
            "shipper instructions must describe debug.exhausted failure publishing with pass_or_fail fail"
        );
    }

    #[test]
    fn test_ce_executor_debug_topics_have_schemas() {
        // U6: fix.plan.ready and debug.exhausted must have event_policy schemas with the
        // documented required fields (task correlation + debug plan fields).
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor should have event_policy");

        let fix_plan = policy
            .schemas
            .get("fix.plan.ready")
            .expect("ce-executor must define fix.plan.ready schema");
        for field in [
            "plan_name",
            "task_id",
            "task_key",
            "step",
            "root_cause_summary",
            "causal_chain",
            "recommended_tests",
            "fix_plan",
        ] {
            assert!(
                fix_plan.required_fields.contains(&field.to_string()),
                "fix.plan.ready schema must require '{}'",
                field
            );
        }

        let debug_exhausted = policy
            .schemas
            .get("debug.exhausted")
            .expect("ce-executor must define debug.exhausted schema");
        for field in [
            "plan_name",
            "reason",
            "task_id",
            "task_key",
            "step",
            "debug_summary",
        ] {
            assert!(
                debug_exhausted.required_fields.contains(&field.to_string()),
                "debug.exhausted schema must require '{}'",
                field
            );
        }
    }

    #[test]
    fn test_ce_executor_zh_debug_topology_matches_en() {
        // U6: Chinese preset must stay isomorphic to English for the new debug topology.
        let en = read_root_preset("ce-executor.yml");
        let zh = read_root_preset("ce-executor-zh.yml");
        let en_config = RalphConfig::parse_yaml(&en).expect("English preset should parse");
        let zh_config = RalphConfig::parse_yaml(&zh).expect("Chinese preset should parse");

        let en_resolver = en_config
            .hats
            .get("debug-resolver")
            .expect("EN preset must have debug-resolver");
        let zh_resolver = zh_config
            .hats
            .get("debug-resolver")
            .expect("ZH preset must have debug-resolver");

        assert_eq!(
            zh_resolver.triggers, en_resolver.triggers,
            "ZH debug-resolver triggers must match EN"
        );
        assert_eq!(
            zh_resolver.publishes, en_resolver.publishes,
            "ZH debug-resolver publishes must match EN"
        );
        assert_eq!(
            zh_resolver.default_publishes, en_resolver.default_publishes,
            "ZH debug-resolver default_publishes must match EN"
        );
        assert!(
            en_resolver.default_publishes.is_none(),
            "EN debug-resolver must not configure default_publishes"
        );

        let en_policy = en_config
            .event_loop
            .event_policy
            .as_ref()
            .expect("EN ce-executor should have event_policy");
        let zh_policy = zh_config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ZH ce-executor should have event_policy");

        for topic in ["fix.plan.ready", "debug.exhausted"] {
            let en_schema = en_policy
                .schemas
                .get(topic)
                .unwrap_or_else(|| panic!("EN ce-executor should define {} schema", topic));
            let zh_schema = zh_policy
                .schemas
                .get(topic)
                .unwrap_or_else(|| panic!("ZH ce-executor should define {} schema", topic));
            assert_eq!(
                zh_schema.required_fields, en_schema.required_fields,
                "ZH {} schema required_fields must match EN",
                topic
            );
        }

        let en_executor = en_config
            .hats
            .get("executor")
            .expect("EN preset must have executor");
        let zh_executor = zh_config
            .hats
            .get("executor")
            .expect("ZH preset must have executor");
        assert!(
            zh_executor.triggers.contains(&"fix.plan.ready".to_string()),
            "ZH executor must trigger on fix.plan.ready"
        );
        assert_eq!(
            zh_executor.triggers, en_executor.triggers,
            "ZH executor triggers must match EN"
        );

        let en_shipper = en_config
            .hats
            .get("shipper")
            .expect("EN preset must have shipper");
        let zh_shipper = zh_config
            .hats
            .get("shipper")
            .expect("ZH preset must have shipper");
        assert!(
            zh_shipper.triggers.contains(&"debug.exhausted".to_string()),
            "ZH shipper must trigger on debug.exhausted"
        );
        assert_eq!(
            zh_shipper.triggers, en_shipper.triggers,
            "ZH shipper triggers must match EN"
        );
    }

    #[test]
    fn test_ce_executor_strict_payload_contract_is_valid() {
        // Strict mode: every trigger topic with payload field references in
        // instructions must have a schema, and all referenced fields must be
        // declared in the schema's required_fields.
        let preset = get_preset("ce-executor").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let registry = HatRegistry::from_config(&config);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(
            result.is_valid(),
            "ce-executor strict payload contract validation failed: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_ce_executor_strict_payload_contract_is_valid_for_root_preset() {
        let content = read_root_preset("ce-executor.yml");
        let config = RalphConfig::parse_yaml(&content).expect("root ce-executor YAML should parse");
        let registry = HatRegistry::from_config(&config);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(
            result.is_valid(),
            "root ce-executor strict payload contract validation failed: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_ce_executor_zh_strict_payload_contract_is_valid() {
        let content = read_root_preset("ce-executor-zh.yml");
        let config = RalphConfig::parse_yaml(&content).expect("ce-executor-zh YAML should parse");
        let registry = HatRegistry::from_config(&config);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(
            result.is_valid(),
            "ce-executor-zh strict payload contract validation failed: {:?}",
            result.errors
        );
    }

    // ------------------------------------------------------------------
    // ce-executor-wave preset tests
    // ------------------------------------------------------------------

    #[test]
    fn test_ce_executor_wave_required_events_is_report_done() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        assert_eq!(
            config.event_loop.required_events,
            &["report.done"],
            "ce-executor-wave should require 'report.done' as its only completion gate event"
        );
    }

    #[test]
    fn test_ce_executor_wave_executor_has_no_default_publishes() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let executor = config
            .hats
            .get("parallel-executor")
            .expect("ce-executor-wave should define parallel-executor hat");
        assert!(
            executor.default_publishes.is_none(),
            "parallel-executor must NOT have default_publishes; explicit emit is required"
        );
    }

    #[test]
    fn test_ce_executor_wave_publish_chain_origin_compatible() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let registry = HatRegistry::from_config(&config);
        let cancellation = &config.event_loop.cancellation_promise;
        let completion = &config.event_loop.completion_promise;

        let chain_publishes: Vec<(&str, &str)> = vec![
            ("coordinator", "work.batch.ready"),
            ("execution-dispatcher", "work.unit.ready"),
            ("execution-dispatcher", "work.serial.ready"),
            ("parallel-executor", "work.unit.done"),
            ("parallel-executor", "work.unit.failed"),
            ("execution-synthesizer", "work.done"),
            ("execution-synthesizer", "work.failed"),
            ("serial-executor", "work.done"),
            ("serial-executor", "work.failed"),
            ("review-coordinator", "review.wave.ready"),
            ("review-coordinator", "review.passed"),
            ("dimension-reviewer", "review.dimension.done"),
            ("review-synthesizer", "review.passed"),
            ("review-synthesizer", "review.failed"),
            ("review-synthesizer", "review.complete"),
            ("plan-gate", "queue.advance"),
            ("plan-gate", "plan.complete"),
            ("plan-gate", "plan.blocked"),
            ("fixer", "fix.exhausted"),
            ("debug-resolver", "fix.plan.ready"),
            ("debug-resolver", "debug.exhausted"),
            ("debug-resolver", "plan.blocked"),
            ("shipper", "REVIEW_COMPLETE"),
            ("reporter", "report.done"),
            ("reporter", "LOOP_COMPLETE"),
        ];

        for (hat_name, expected_topic) in &chain_publishes {
            let event = ralph_core::Event {
                topic: expected_topic.to_string(),
                payload: None,
                ts: "2025-01-01T00:00:00Z".to_string(),
                hat: Some(hat_name.to_string()),
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
            };

            let result = validate_event_origin(&event, &registry, cancellation, completion);
            assert_eq!(
                result,
                OriginCheck::Accepted,
                "ce-executor-wave: hat '{}' should be able to publish '{}', got: {:?}",
                hat_name,
                expected_topic,
                result
            );
        }
    }

    #[test]
    fn test_ce_executor_wave_root_preset_matches_embedded() {
        let root_content = read_root_preset("ce-executor-wave.yml");
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        assert_eq!(
            root_content, preset.content,
            "Canonical presets/en/ce-executor-wave.yml must match the embedded copy in $OUT_DIR/presets/. \
             Run `cargo build` to refresh the $OUT_DIR mirror, or edit the canonical file and rebuild."
        );
    }

    #[test]
    fn test_ce_executor_wave_parallel_executor_has_concurrency_3_no_aggregate() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let executor = config
            .hats
            .get("parallel-executor")
            .expect("ce-executor-wave should define parallel-executor hat");
        assert_eq!(
            executor.concurrency, 3,
            "parallel-executor must have concurrency = 3"
        );
        assert!(
            executor.aggregate.is_none(),
            "parallel-executor must NOT have aggregate"
        );
    }

    #[test]
    fn test_ce_executor_wave_synthesizer_has_aggregate_no_concurrency() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let synthesizer = config
            .hats
            .get("execution-synthesizer")
            .expect("ce-executor-wave should define execution-synthesizer hat");
        assert!(
            matches!(
                synthesizer.aggregate.as_ref().map(|a| a.mode.clone()),
                Some(ralph_core::AggregateMode::WaitForAll)
            ),
            "execution-synthesizer aggregate.mode must be wait_for_all"
        );
        assert_eq!(
            synthesizer.concurrency, 1,
            "execution-synthesizer must use default concurrency (1), not an explicit override"
        );
    }

    #[test]
    fn test_ce_executor_wave_work_done_field_consistency() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");

        let contracts = config
            .event_loop
            .execution_contracts
            .as_ref()
            .expect("ce-executor-wave should have execution_contracts");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor-wave should have event_policy");

        let contract_rule = contracts
            .rules
            .get("work.done")
            .expect("ce-executor-wave execution contract should define work.done rule");
        let contract_fields: std::collections::BTreeSet<&str> = contract_rule
            .require_payload_fields
            .iter()
            .map(String::as_str)
            .collect();

        let schema = policy
            .schemas
            .get("work.done")
            .expect("ce-executor-wave event_policy should define work.done schema");
        let schema_fields: std::collections::BTreeSet<&str> =
            schema.required_fields.iter().map(String::as_str).collect();

        let required_minimum: std::collections::BTreeSet<&str> = [
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "commit_count",
            "changed_lines",
        ]
        .iter()
        .copied()
        .collect();
        assert_eq!(
            contract_fields, required_minimum,
            "ce-executor-wave work.done contract must require exactly \
             {{plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines}}"
        );
        assert_eq!(
            schema_fields, required_minimum,
            "ce-executor-wave work.done event_policy schema must require exactly \
             {{plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines}}"
        );

        // parallel-executor and serial-executor instructions must mention every required field
        for hat_name in ["parallel-executor", "serial-executor"] {
            let hat = config
                .hats
                .get(hat_name)
                .unwrap_or_else(|| panic!("ce-executor-wave should have {} hat", hat_name));
            let instructions = hat.instructions.as_str();
            for field in &required_minimum {
                assert!(
                    instructions.contains(field),
                    "{} instructions must mention required work.done field '{}'",
                    hat_name,
                    field
                );
            }
        }

        let reviewer = config
            .hats
            .get("review-coordinator")
            .expect("ce-executor-wave should have review-coordinator hat");
        let reviewer_instructions = reviewer.instructions.as_str();
        for field in &required_minimum {
            assert!(
                reviewer_instructions.contains(field),
                "review-coordinator instructions must mention required work.done field '{}'",
                field
            );
        }
    }

    #[test]
    fn test_ce_executor_wave_plan_gate_exists_and_routes_correctly() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");

        let plan_gate = config
            .hats
            .get("plan-gate")
            .expect("ce-executor-wave must define a 'plan-gate' hat");
        assert!(
            plan_gate.triggers.contains(&"review.passed".to_string()),
            "plan-gate must trigger on review.passed"
        );
        assert!(
            plan_gate.triggers.contains(&"review.complete".to_string()),
            "plan-gate must trigger on review.complete"
        );
        assert!(
            plan_gate.triggers.contains(&"work.failed".to_string()),
            "plan-gate must trigger on work.failed"
        );
        assert!(
            plan_gate.publishes.contains(&"queue.advance".to_string()),
            "plan-gate must publish queue.advance"
        );
        assert!(
            plan_gate.publishes.contains(&"plan.complete".to_string()),
            "plan-gate must publish plan.complete"
        );
        assert!(
            plan_gate.publishes.contains(&"plan.blocked".to_string()),
            "plan-gate must publish plan.blocked"
        );
        assert_eq!(
            plan_gate.default_publishes.as_deref(),
            Some("plan.blocked"),
            "plan-gate default_publishes should be plan.blocked"
        );
        assert!(
            !plan_gate.triggers.contains(&"fix.applied".to_string()),
            "plan-gate must NOT listen to fix.applied"
        );
    }

    #[test]
    fn test_ce_executor_wave_shipper_triggers_finalization_only() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");

        let shipper = config
            .hats
            .get("shipper")
            .expect("ce-executor-wave must define a 'shipper' hat");
        assert!(
            shipper.triggers.contains(&"plan.complete".to_string()),
            "shipper must trigger on plan.complete"
        );
        assert!(
            shipper.triggers.contains(&"plan.blocked".to_string()),
            "shipper must trigger on plan.blocked"
        );
        assert!(
            shipper.triggers.contains(&"debug.exhausted".to_string()),
            "shipper must trigger on debug.exhausted"
        );
        assert!(
            !shipper.triggers.contains(&"review.passed".to_string()),
            "shipper must NOT trigger on review.passed"
        );
        assert!(
            !shipper.triggers.contains(&"review.complete".to_string()),
            "shipper must NOT trigger on review.complete"
        );
    }

    #[test]
    fn test_ce_executor_wave_work_failed_requires_plan_context() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor-wave should have event_policy");

        let schema = policy
            .schemas
            .get("work.failed")
            .expect("ce-executor-wave should define work.failed schema");
        let expected_fields = [
            "plan_name",
            "plan_path",
            "step",
            "task_id",
            "task_key",
            "reason",
        ];
        assert_eq!(
            schema.required_fields.len(),
            expected_fields.len(),
            "work.failed schema field count mismatch"
        );
        for field in &expected_fields {
            assert!(
                schema.required_fields.contains(&field.to_string()),
                "work.failed schema must require field '{}'",
                field
            );
        }

        let plan_blocked = policy
            .schemas
            .get("plan.blocked")
            .expect("ce-executor-wave should define plan.blocked schema");
        assert_eq!(
            plan_blocked.required_fields,
            vec!["reason".to_string()],
            "plan.blocked may remain reason-only because it is the final manager-facing blocked topic"
        );
    }

    #[test]
    fn test_ce_executor_wave_forbids_agent_branch_creation() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let content = preset.content;

        assert!(
            content.contains("NEVER create, switch, or rename branches")
                && content.contains("`git checkout -b`")
                && content.contains("`git worktree add`"),
            "ce-executor-wave guardrails must explicitly forbid branch creation by the agent"
        );

        assert!(
            !content.contains("create one (e.g., `feat/plan-name`)"),
            "ce-executor-wave must NOT instruct the agent to auto-create a feature branch"
        );
    }

    #[test]
    fn test_ce_executor_wave_new_topics_have_schema_publisher_consumer() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor-wave should have event_policy");

        let new_topics = [
            "work.batch.ready",
            "work.unit.ready",
            "work.unit.done",
            "work.unit.failed",
            "work.serial.ready",
        ];

        for topic in &new_topics {
            // Some hat must publish this topic
            let has_publisher = config.hats.values().any(|hat| {
                hat.publishes.contains(&topic.to_string())
                    || hat.default_publishes.as_deref() == Some(topic)
            });
            assert!(
                has_publisher,
                "ce-executor-wave: topic '{}' must have at least one publisher hat",
                topic
            );

            // Some hat must trigger on this topic
            let has_consumer = config
                .hats
                .values()
                .any(|hat| hat.triggers.contains(&topic.to_string()));
            assert!(
                has_consumer,
                "ce-executor-wave: topic '{}' must have at least one consumer (trigger) hat",
                topic
            );

            // Schema must be defined
            assert!(
                policy.schemas.contains_key(*topic),
                "ce-executor-wave: topic '{}' must have a schema defined in event_policy.schemas",
                topic
            );
        }

        // P2-14: Precise publisher/consumer assertions for each topic
        let coordinator = config
            .hats
            .get("coordinator")
            .expect("coordinator must exist");
        assert!(
            coordinator
                .publishes
                .contains(&"work.batch.ready".to_string()),
            "coordinator must publish work.batch.ready"
        );

        let dispatcher = config
            .hats
            .get("execution-dispatcher")
            .expect("execution-dispatcher must exist");
        assert!(
            dispatcher
                .publishes
                .contains(&"work.unit.ready".to_string()),
            "execution-dispatcher must publish work.unit.ready"
        );
        assert!(
            dispatcher
                .publishes
                .contains(&"work.serial.ready".to_string()),
            "execution-dispatcher must publish work.serial.ready"
        );
        assert!(
            dispatcher
                .triggers
                .contains(&"work.batch.ready".to_string()),
            "execution-dispatcher must trigger on work.batch.ready"
        );

        let parallel_executor = config
            .hats
            .get("parallel-executor")
            .expect("parallel-executor must exist");
        assert!(
            parallel_executor
                .publishes
                .contains(&"work.unit.done".to_string()),
            "parallel-executor must publish work.unit.done"
        );
        assert!(
            parallel_executor
                .publishes
                .contains(&"work.unit.failed".to_string()),
            "parallel-executor must publish work.unit.failed"
        );
        assert!(
            parallel_executor
                .triggers
                .contains(&"work.unit.ready".to_string()),
            "parallel-executor must trigger on work.unit.ready"
        );

        let synthesizer = config
            .hats
            .get("execution-synthesizer")
            .expect("execution-synthesizer must exist");
        assert!(
            synthesizer.triggers.contains(&"work.unit.done".to_string()),
            "execution-synthesizer must trigger on work.unit.done"
        );
        assert!(
            synthesizer
                .triggers
                .contains(&"work.unit.failed".to_string()),
            "execution-synthesizer must trigger on work.unit.failed"
        );

        let serial_executor = config
            .hats
            .get("serial-executor")
            .expect("serial-executor must exist");
        assert!(
            serial_executor
                .triggers
                .contains(&"work.serial.ready".to_string()),
            "serial-executor must trigger on work.serial.ready"
        );
    }

    #[test]
    fn test_ce_executor_wave_parallel_executor_forbids_git_and_nested_waves() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let content = preset.content;
        let parallel_section = content
            .split("parallel-executor:")
            .nth(1)
            .expect("parallel-executor section must exist");
        let next_hat = parallel_section
            .find("execution-synthesizer:")
            .expect("execution-synthesizer must follow parallel-executor");
        let parallel_instr = &parallel_section[..next_hat];

        // Git operations prohibited
        assert!(
            parallel_instr.contains("MUST NOT run `git add` or `git commit`"),
            "parallel-executor must prohibit git add/commit"
        );
        // Branch creation prohibited
        assert!(
            parallel_instr.contains("MUST NOT create, switch, or rename branches or worktrees"),
            "parallel-executor must prohibit branch/worktree creation"
        );
        // Task lifecycle prohibited
        assert!(
            parallel_instr.contains("MUST NOT run `ralph tools task start/close/fail/reopen`"),
            "parallel-executor must prohibit task lifecycle operations"
        );
        // Nested waves prohibited
        assert!(
            parallel_instr.contains("MUST NOT run `ralph wave emit`"),
            "parallel-executor must prohibit nested wave emission"
        );
        // owned_files boundary
        assert!(
            parallel_instr.contains("owned_files"),
            "parallel-executor must reference owned_files boundary"
        );
        assert!(
            parallel_instr.contains("outside owned_files"),
            "parallel-executor must explicitly guard against modifying files outside owned_files"
        );

        // Timeout must be 1800 (not default 300)
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let executor = config
            .hats
            .get("parallel-executor")
            .expect("parallel-executor must exist");
        assert_eq!(
            executor.timeout,
            Some(1800),
            "parallel-executor timeout must be 1800s (30 min), not default 300s"
        );
    }

    #[test]
    fn test_ce_executor_wave_dispatcher_safety_rules() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let content = preset.content;
        let dispatcher_section = content
            .split("execution-dispatcher:")
            .nth(1)
            .expect("execution-dispatcher section must exist");
        let next_hat = dispatcher_section
            .find("parallel-executor:")
            .expect("parallel-executor must follow execution-dispatcher");
        let dispatcher_instr = &dispatcher_section[..next_hat];

        // Current-step-only analysis
        assert!(
            dispatcher_instr.contains("current step"),
            "dispatcher must scope analysis to current step only"
        );
        // Disjoint owned_files
        assert!(
            dispatcher_instr.contains("No two tasks share any file path"),
            "dispatcher must require disjoint file ownership"
        );
        // Conservative fallback principle
        assert!(
            dispatcher_instr.contains("When in doubt, go serial"),
            "dispatcher must encode conservative fallback principle"
        );
        assert!(
            dispatcher_instr.contains("Safety over throughput"),
            "dispatcher must prioritize safety over throughput"
        );
        // MUST NOT implement / modify tasks
        assert!(
            dispatcher_instr.contains("MUST NOT implement code"),
            "dispatcher must be forbidden from implementing code"
        );
        assert!(
            dispatcher_instr.contains("MUST NOT modify runtime tasks"),
            "dispatcher must be forbidden from modifying runtime tasks"
        );
        // Payloads-stdin required
        assert!(
            dispatcher_instr.contains("--payloads-stdin"),
            "dispatcher must use --payloads-stdin for wave dispatch"
        );
    }

    #[test]
    fn test_ce_executor_wave_work_unit_done_schema_consistency() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor-wave should have event_policy");
        let schema = policy
            .schemas
            .get("work.unit.done")
            .expect("work.unit.done schema must exist");

        let expected_fields = [
            "plan_name",
            "plan_path",
            "step",
            "task_id",
            "task_key",
            "owned_files",
            "changed_files",
            "tests",
        ];
        assert_eq!(
            schema.required_fields.len(),
            expected_fields.len(),
            "work.unit.done schema field count mismatch"
        );
        for field in &expected_fields {
            assert!(
                schema.required_fields.contains(&field.to_string()),
                "work.unit.done schema must require field '{}'",
                field
            );
        }

        // Instructions must mention all required payload fields
        let content = preset.content;
        let parallel_section = content
            .split("parallel-executor:")
            .nth(1)
            .expect("parallel-executor section must exist");
        let next_hat = parallel_section
            .find("execution-synthesizer:")
            .expect("execution-synthesizer must follow parallel-executor");
        let parallel_instr = &parallel_section[..next_hat];
        assert!(
            parallel_instr.contains("changed_files"),
            "parallel-executor instructions must reference changed_files"
        );
        assert!(
            parallel_instr.contains("tests"),
            "parallel-executor instructions must reference tests"
        );
    }

    #[test]
    fn test_ce_executor_wave_work_unit_failed_schema_consistency() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("ce-executor-wave should have event_policy");
        let schema = policy
            .schemas
            .get("work.unit.failed")
            .expect("work.unit.failed schema must exist");

        let expected_fields = [
            "plan_name",
            "plan_path",
            "step",
            "task_id",
            "task_key",
            "reason",
        ];
        assert_eq!(
            schema.required_fields.len(),
            expected_fields.len(),
            "work.unit.failed schema field count mismatch"
        );
        for field in &expected_fields {
            assert!(
                schema.required_fields.contains(&field.to_string()),
                "work.unit.failed schema must require field '{}'",
                field
            );
        }

        // Instructions must reference the failure reason
        let content = preset.content;
        let parallel_section = content
            .split("parallel-executor:")
            .nth(1)
            .expect("parallel-executor section must exist");
        let next_hat = parallel_section
            .find("execution-synthesizer:")
            .expect("execution-synthesizer must follow parallel-executor");
        let parallel_instr = &parallel_section[..next_hat];
        assert!(
            parallel_instr.contains("reason"),
            "parallel-executor instructions must reference reason in failure payload"
        );
    }

    #[test]
    fn test_ce_executor_wave_synthesizer_fail_closed() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let content = preset.content;
        let synth_section = content
            .split("execution-synthesizer:")
            .nth(1)
            .expect("execution-synthesizer section must exist");
        let next_hat = synth_section
            .find("serial-executor:")
            .expect("serial-executor must follow execution-synthesizer");
        let synth_instr = &synth_section[..next_hat];

        // Partial failure handling
        assert!(
            synth_instr.contains("Any `work.unit.failed`"),
            "synthesizer must treat any worker failure as batch failure"
        );
        assert!(
            synth_instr.contains("batch failed"),
            "synthesizer must reference batch failed state"
        );
        // Boundary violation handling
        assert!(
            synth_instr.contains("outside the worker's `owned_files`"),
            "synthesizer must check owned_files boundary violations"
        );
        // Timeout / missing worker
        assert!(
            synth_instr.contains("missing worker result within timeout"),
            "synthesizer must handle missing worker results (timeout)"
        );
        // Re-validation requirement
        assert!(
            synth_instr.contains("re-validation"),
            "synthesizer must require re-validation"
        );
        assert!(
            synth_instr.contains("Do NOT trust worker self-reported success alone"),
            "synthesizer must distrust worker self-reported success"
        );
        // Failure cleanup must not destroy unrelated user changes.
        assert!(
            synth_instr.contains("Do not run global rollback commands"),
            "synthesizer must forbid global rollback commands on batch failure"
        );
        assert!(
            synth_instr.contains("Never use `git checkout -- .`")
                && synth_instr.contains("`git restore .`")
                && synth_instr.contains("whole workspace"),
            "synthesizer must explicitly forbid whole-workspace checkout/restore rollback"
        );
        assert!(
            synth_instr.contains("changed_files") && synth_instr.contains("owned_files"),
            "synthesizer must scope failure cleanup to changed_files and owned_files"
        );
        assert!(
            synth_instr.contains("execution-batch.md"),
            "synthesizer must record dirty state in execution-batch.md"
        );
    }

    #[test]
    fn test_ce_executor_wave_reference_schema_matches_inline_schema() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let inline_yaml: serde_yaml::Value =
            serde_yaml::from_str(preset.content).expect("ce-executor-wave YAML should parse");
        let inline_schemas = inline_yaml
            .get("event_loop")
            .and_then(|value| value.get("event_policy"))
            .and_then(|value| value.get("schemas"))
            .expect("ce-executor-wave inline schemas should exist")
            .clone();

        let reference_content = read_root_schema("ce-executor-wave.yml");
        let reference_schemas: serde_yaml::Value =
            serde_yaml::from_str(&reference_content).expect("reference schema YAML should parse");

        assert_eq!(
            inline_schemas, reference_schemas,
            "presets/schemas/ce-executor-wave.yml must match inline event_policy.schemas"
        );
    }

    #[test]
    fn test_ce_executor_wave_synthesizer_aggregate_timeout() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let synthesizer = config
            .hats
            .get("execution-synthesizer")
            .expect("execution-synthesizer must exist");
        let aggregate = synthesizer
            .aggregate
            .as_ref()
            .expect("execution-synthesizer must have aggregate");
        assert_eq!(
            aggregate.timeout, 300,
            "execution-synthesizer aggregate timeout must be 300s"
        );
    }

    #[test]
    fn test_ce_executor_wave_dimension_reviewer_timeout() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let reviewer = config
            .hats
            .get("dimension-reviewer")
            .expect("dimension-reviewer must exist");
        assert_eq!(
            reviewer.timeout,
            Some(1800),
            "dimension-reviewer timeout must be 1800s"
        );
    }

    #[test]
    fn test_ce_executor_wave_review_synthesizer_default_publishes() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let reviewer = config
            .hats
            .get("review-synthesizer")
            .expect("review-synthesizer must exist");
        assert_eq!(
            reviewer.default_publishes,
            Some("review.complete".to_string()),
            "review-synthesizer default_publishes must be review.complete"
        );
        assert!(
            reviewer.publishes.contains(&"review.passed".to_string()),
            "review-synthesizer must publish review.passed"
        );
        assert!(
            reviewer.publishes.contains(&"review.failed".to_string()),
            "review-synthesizer must publish review.failed"
        );
        assert!(
            reviewer.publishes.contains(&"review.complete".to_string()),
            "review-synthesizer must publish review.complete"
        );
    }

    #[test]
    fn test_ce_executor_wave_serial_executor_triggers() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let serial = config
            .hats
            .get("serial-executor")
            .expect("serial-executor must exist");
        assert!(
            serial.triggers.contains(&"work.serial.ready".to_string()),
            "serial-executor must trigger on work.serial.ready"
        );
        assert!(
            serial.triggers.contains(&"fix.plan.ready".to_string()),
            "serial-executor must trigger on fix.plan.ready"
        );
        assert!(
            serial.publishes.contains(&"work.done".to_string()),
            "serial-executor must publish work.done"
        );
        assert!(
            serial.publishes.contains(&"work.failed".to_string()),
            "serial-executor must publish work.failed"
        );
    }

    #[test]
    fn test_ce_executor_wave_dispatcher_hat_config() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let dispatcher = config
            .hats
            .get("execution-dispatcher")
            .expect("execution-dispatcher must exist");
        assert_eq!(
            dispatcher.triggers,
            vec!["work.batch.ready".to_string()],
            "execution-dispatcher must trigger only on work.batch.ready"
        );
        assert!(
            dispatcher
                .publishes
                .contains(&"work.unit.ready".to_string()),
            "execution-dispatcher must publish work.unit.ready"
        );
        assert!(
            dispatcher
                .publishes
                .contains(&"work.serial.ready".to_string()),
            "execution-dispatcher must publish work.serial.ready"
        );
        assert!(
            dispatcher.publishes.contains(&"work.failed".to_string()),
            "execution-dispatcher must publish work.failed"
        );
    }

    #[test]
    fn test_ce_executor_wave_verdict_gate() {
        let preset = get_preset("ce-executor-wave").expect("ce-executor-wave preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-wave YAML should parse");
        let gate = config
            .event_loop
            .verdict_gate
            .as_ref()
            .expect("ce-executor-wave must have verdict_gate");
        assert_eq!(
            gate.topic, "REVIEW_COMPLETE",
            "verdict_gate topic must be REVIEW_COMPLETE"
        );
        assert_eq!(
            gate.fail_field, "pass_or_fail",
            "verdict_gate fail_field must be pass_or_fail"
        );
        assert_eq!(
            gate.fail_value, "fail",
            "verdict_gate fail_value must be 'fail'"
        );
    }

    #[test]
    fn test_ce_executor_wave_shared_tail_matches_ce_executor() {
        // P1-3: The back-half pipeline (fixer → debug-resolver → plan-gate → shipper → reporter)
        // is intentionally shared between ce-executor and ce-executor-wave. This test
        // gates against accidental drift in triggers/publishes/default_publishes.
        let wave_preset = get_preset("ce-executor-wave").expect("ce-executor-wave should exist");
        let wave_config = RalphConfig::parse_yaml(wave_preset.content)
            .expect("ce-executor-wave YAML should parse");

        let base_preset = get_preset("ce-executor").expect("ce-executor should exist");
        let base_config =
            RalphConfig::parse_yaml(base_preset.content).expect("ce-executor YAML should parse");

        let shared_hats = [
            "fixer",
            "debug-resolver",
            "plan-gate",
            "shipper",
            "reporter",
        ];

        for hat_name in &shared_hats {
            let wave_hat = wave_config
                .hats
                .get(*hat_name)
                .unwrap_or_else(|| panic!("ce-executor-wave must have {} hat", hat_name));
            let base_hat = base_config
                .hats
                .get(*hat_name)
                .unwrap_or_else(|| panic!("ce-executor must have {} hat", hat_name));

            assert_eq!(
                wave_hat.triggers, base_hat.triggers,
                "{}: triggers must match between ce-executor-wave and ce-executor",
                hat_name
            );
            assert_eq!(
                wave_hat.publishes, base_hat.publishes,
                "{}: publishes must match between ce-executor-wave and ce-executor",
                hat_name
            );
            assert_eq!(
                wave_hat.default_publishes, base_hat.default_publishes,
                "{}: default_publishes must match between ce-executor-wave and ce-executor",
                hat_name
            );
        }

        // Verify both presets share the same core safety constraints in instructions
        let wave_content = wave_preset.content;
        let base_content = base_preset.content;
        for phrase in [
            "NEVER create, switch, or rename branches",
            "NEVER push to origin",
            "MUST NOT modify code",
        ] {
            assert!(
                wave_content.contains(phrase),
                "ce-executor-wave must contain safety phrase: '{}'",
                phrase
            );
            assert!(
                base_content.contains(phrase),
                "ce-executor must contain safety phrase: '{}'",
                phrase
            );
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // U6: Builtin Preset Contract Regression Matrix
    // ──────────────────────────────────────────────────────────────────────

    use ralph_core::runtime_contract::{
        FindingSeverity, RuntimeContractAggregator, RuntimeContractStrictness,
    };

    /// All public builtin presets must parse, pass config validation,
    /// and pass the authoring contract check (topology, payload non-strict,
    /// orphan). Presets with known topology exceptions are listed in
    /// `TOPOLOGY_EXEMPT_PRESETS`.
    #[test]
    fn test_all_public_presets_pass_authoring_contract() {
        // Presets with known topology issues (required events not on all
        // completion paths). These are documented exceptions, not hidden
        // failures. Add to this list only with a comment explaining why.
        let topology_exempt: &[&str] = &[
            // autoresearch: experiment loop has branching completion paths
            // where required events (experiment.scored, experiment.evaluated)
            // are not on every path — this is by design for the experiment
            // loop's try/measure/keep/discard flow.
            "autoresearch",
            // debug: debug loop has branching paths where required events
            // (hypothesis.confirmed, fix.applied, fix.verified) are not on
            // every completion path — this is by design for the debug loop's
            // hypothesis/fix/verify flow.
            "debug",
        ];

        for preset in PRESETS.iter().filter(|p| p.public) {
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_runtime_config(&config);
            let strictness = RuntimeContractStrictness::default(); // non-strict
            let report = RuntimeContractAggregator::aggregate(
                &format!("builtin:{}", preset.name),
                &config,
                &registry,
                strictness,
            );

            if report.passed {
                continue;
            }

            // Check if all errors are topology errors for an exempt preset
            let errors: Vec<_> = report
                .findings
                .iter()
                .filter(|f| matches!(f.severity, FindingSeverity::Error))
                .collect();
            let all_topology = errors.iter().all(|f| {
                matches!(
                    f.source,
                    ralph_core::runtime_contract::FindingSource::Topology
                )
            });

            if topology_exempt.contains(&preset.name) && all_topology {
                // Known topology exception — record but don't fail
                eprintln!(
                    "NOTE: preset '{}' has known topology exceptions (exempt from authoring contract): {:?}",
                    preset.name,
                    errors.iter().map(|f| &f.id).collect::<Vec<_>>()
                );
                continue;
            }

            panic!(
                "Public preset '{}' failed authoring contract: {:?}",
                preset.name,
                errors
                    .iter()
                    .map(|f| format!("{}: {}", f.id, f.message))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Development presets must pass strict payload contract check.
    /// These presets are used for active development and must have
    /// fully declared schemas.
    #[test]
    fn test_development_presets_pass_strict_contract() {
        let strict_presets = &[
            "ce-executor",
            "ce-executor-wave",
            "code-assist",
            "pdd-to-code-assist",
        ];
        for preset_name in strict_presets {
            let preset = PRESETS
                .iter()
                .find(|p| p.name == *preset_name)
                .unwrap_or_else(|| panic!("preset '{}' not found", preset_name));
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_runtime_config(&config);
            let strictness = RuntimeContractStrictness::preset_check_strict();
            let report = RuntimeContractAggregator::aggregate(
                &format!("builtin:{}", preset.name),
                &config,
                &registry,
                strictness,
            );
            assert!(
                report.passed,
                "Development preset '{}' failed strict contract: {:?}",
                preset.name,
                report
                    .findings
                    .iter()
                    .filter(|f| matches!(f.severity, FindingSeverity::Error))
                    .map(|f| format!("{}: {}", f.id, f.message))
                    .collect::<Vec<_>>()
            );
        }
    }
}
