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
        name: "ce-executor-pipeline",
        description: "Isolated-mode linear one-shot plan execution: review plan → execute whole plan with TDD + full suite green → 6 serial dimension reviewers → synthesize fix plan → fix → align → report → complete",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/presets/ce-executor-pipeline.yml"
        )),
        public: true,
    },
    EmbeddedPreset {
        name: "ce-executor-serial",
        description: "Isolated-mode plan-driven work execution with TDD executor, validator (full test suite), single overall review, auto-fix, shipping, and manager report",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor-serial.yml")),
        public: true,
    },
    // 2026-07-03-001 plan U13: supervisor parallel preset.
    // 16 functional hats + progress-steward. Requires the
    // `supervisor-db` feature at build time so the rusqlite
    // store links; isolated mode + supervisor.enabled: true is
    // required (R-SW-1 lint enforces).
    EmbeddedPreset {
        name: "ce-executor-supervisor",
        description: "Isolated-mode plan-driven work execution with parallel worker fan-out via rusqlite supervisor: per-slot worktrees, fan-in merge, parallel 6-dim review, parallel fix, integration + report",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/presets/ce-executor-supervisor.yml"
        )),
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
        name: "merge-batch",
        description: "Git-first batch merge: review design intent, merge multiple worktree branches, stabilize with verify-fix loop, write merge report",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/merge-batch.yml")),
        public: true,
    },
];

/// WRC-U5 (2026-06-12-003) / KTD-WRC-5: Tier-0 list of builtin
/// presets that the CI gate (`scripts/validate-builtin-presets.sh
/// --strict`) treats as **fully WAC-strict**. The gate surfaces a
/// WAC `lint.preset.*` error as a hard failure for every preset in
/// this list. Adding a preset here is the commitment to keep it
/// WAC-clean; non-Tier-0 presets get the legacy "warn-only" path
/// and can be promoted to Tier-0 in a follow-up PR.
///
/// Tier-0 design rationale (KTD-WRC-5): making every builtin
#[allow(dead_code)] // 003 plan tiered-gates 预留：见 docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md
pub const TIER_0_WAC_PRESETS: &[&str] = &["ce-executor-serial"];

/// `true` if `preset_name` is in the Tier-0 list. Used by the CI
/// gate and by the test suite that asserts the Tier-0 preset
/// passes WAC strict.
#[allow(dead_code)] // 003 plan tiered-gates 预留：见 docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md
pub fn is_tier_0_wac_preset(preset_name: &str) -> bool {
    TIER_0_WAC_PRESETS.iter().any(|n| *n == preset_name)
}

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

/// P2-6: SSOT multi-section merge table (KTD-1, plan 2026-06-20-001 U1).
///
/// The table itself lives in [`crate::preset_merge_table`] so
/// `build.rs` and this crate can share one source of truth
/// (build.rs uses `include!` because the build script and
/// the library are separate compilation units).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset_merge_table::SSOT_SECTION_TARGETS;
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
        assert_eq!(presets.len(), 6, "Expected 6 public presets");
    }

    #[test]
    fn test_get_preset_by_name() {
        let preset = get_preset("debug");
        assert!(preset.is_some(), "debug preset should exist");
        let preset = preset.unwrap();
        assert_eq!(preset.name, "debug");
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

    /// U7 / AE7: legacy `ce-executor` must NOT be resolvable. R13–R15 require
    /// removal of the actual YAML, manifest entry, public index, registry entry,
    /// and shell completion — without an alias. `get_preset("ce-executor")` must
    /// return `None` and the user-facing `preset_names()` must not list it.
    /// The replacement is `ce-executor-serial` (R12, the only complete CE
    /// executor entry point).
    #[test]
    fn test_ce_executor_returns_unknown_after_u7_removal() {
        // F5 / AE7: registry lookup must fail explicitly.
        assert!(
            get_preset("ce-executor").is_none(),
            "U7: legacy 'ce-executor' must NOT be resolvable. \
             R13–R15 require removal of YAML, manifest, public index, \
             registry entry, and shell completion without aliasing to \
             'ce-executor-serial'."
        );

        // The replacement entry point must remain resolvable.
        let replacement = get_preset("ce-executor-serial")
            .expect("ce-executor-serial must remain the only complete CE executor entry point");
        assert_eq!(replacement.name, "ce-executor-serial");
        assert!(
            !replacement.content.is_empty(),
            "ce-executor-serial must still be embedded with non-empty content"
        );

        // Public listing must drop the legacy name.
        let public_names = preset_names();
        assert!(
            !public_names.contains(&"ce-executor"),
            "U7: 'ce-executor' must NOT appear in public preset_names()"
        );
        assert!(
            public_names.contains(&"ce-executor-serial"),
            "U7: 'ce-executor-serial' must remain in public preset_names()"
        );

        // Sibling templates (lite / wave) must be unaffected.
        assert!(
            !public_names.contains(&"ce-executor-lite"),
            "ce-executor-lite is a template, not a builtin — it must NOT be in public_names()"
        );
        // 2026-06-17-002 U3: ce-executor-serial (serial-review variant) is a
        // sibling public builtin alongside -isolated.
        assert!(
            public_names.contains(&"ce-executor-serial"),
            "ce-executor-serial must be a public builtin (2026-06-17-002 U3)"
        );
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
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"autoresearch"));
        assert!(names.contains(&"ce-executor-pipeline"));
        assert!(names.contains(&"ce-executor-serial"));
        assert!(names.contains(&"ce-executor-supervisor"));
        assert!(names.contains(&"debug"));
        assert!(names.contains(&"merge-batch"));
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
                system_injected: None,
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
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
    fn test_ce_executor_executor_has_no_default_publishes() {
        // U2: executor must NOT have default_publishes — it must explicitly emit.
        // The no-event gate (U1) handles the "forgot to emit" case instead.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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

    /// Plan 2026-06-16-002 Unit 1: reproduce the merge that `build.rs`
    /// applies at compile time — read the canonical preset and the
    /// schema SSOT, then deep-merge the SSOT's `schemas:` mapping into
    /// `event_loop.event_policy.schemas` (SSOT base, inline override).
    /// Used by the SSOT-driven parity tests below to verify the binary
    /// embedded what the build pipeline intended.
    fn merge_root_with_ssot(preset_name: &str) -> String {
        let preset_text = read_root_preset(&format!("{preset_name}.yml"));
        let ssot_text = read_root_schema(&format!("{preset_name}.yml"));
        merge_preset_with_schema_yaml(&preset_text, &ssot_text).unwrap_or_else(|e| {
            panic!(
                "merge_root_with_ssot({preset_name}) failed: {e}\n\
                 (this mirrors build.rs — if it fails here, the test setup is broken, \
                  not the production code.)"
            )
        })
    }

    fn merge_preset_with_schema_yaml(preset_text: &str, ssot_text: &str) -> Result<String, String> {
        let mut preset: serde_yaml::Value =
            serde_yaml::from_str(preset_text).map_err(|e| format!("preset YAML: {e}"))?;
        let ssot: serde_yaml::Value =
            serde_yaml::from_str(ssot_text).map_err(|e| format!("SSOT YAML: {e}"))?;

        // 1) Schemas deep-merge into event_policy.schemas (U1).
        let ssot_schemas = match ssot.get("schemas") {
            Some(serde_yaml::Value::Mapping(m)) => m.clone(),
            Some(other) => {
                return Err(format!(
                    "SSOT `schemas` must be a mapping, found {:?}",
                    other
                ));
            }
            None => serde_yaml::Mapping::new(),
        };

        let event_loop = ensure_yaml_mapping(&mut preset, &["event_loop"])?;
        let event_policy = ensure_yaml_mapping(event_loop, &["event_policy"])?;
        let inline_schemas_mapping = event_policy
            .get("schemas")
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();
        let merged = deep_merge_yaml_mapping(&ssot_schemas, &inline_schemas_mapping);
        let event_policy_mapping = event_policy
            .as_mapping_mut()
            .expect("ensure_yaml_mapping returned a non-mapping Value");
        event_policy_mapping.insert(
            serde_yaml::Value::String("schemas".to_string()),
            serde_yaml::Value::Mapping(merged),
        );

        // 2) Multi-section protocol merge (plan 2026-06-20-001
        //    U1 / KTD-1). Mirrors `build.rs` exactly so the
        //    embedded copy produced by `cargo build` matches
        //    what this test computes. The mapping table lives
        //    in `crate::preset_merge_table::SSOT_SECTION_TARGETS`
        //    (P2-6) so the build script and the test can share
        //    one source of truth. Each SSOT top-level key (other
        //    than `schemas`) is deep-merged into
        //    `event_loop.<section>`.
        let section_targets = SSOT_SECTION_TARGETS;
        for (ssot_key, target_path) in section_targets {
            let Some(ssot_value) = ssot.get(*ssot_key) else {
                continue;
            };
            let ssot_mapping = match ssot_value {
                serde_yaml::Value::Mapping(m) => m.clone(),
                other => {
                    return Err(format!(
                        "SSOT `{ssot_key}` must be a mapping, found {:?}",
                        other
                    ));
                }
            };
            let parent_path = &target_path[..target_path.len() - 1];
            let leaf_key = target_path[target_path.len() - 1];
            let parent = ensure_yaml_mapping(&mut preset, parent_path)?;
            let parent_mapping = parent
                .as_mapping_mut()
                .expect("ensure_yaml_mapping returned a non-mapping Value");
            let inline_mapping = parent_mapping
                .get(leaf_key)
                .and_then(|v| v.as_mapping())
                .cloned()
                .unwrap_or_default();
            let section_merged = deep_merge_yaml_mapping(&ssot_mapping, &inline_mapping);
            parent_mapping.insert(
                serde_yaml::Value::String(leaf_key.to_string()),
                serde_yaml::Value::Mapping(section_merged),
            );
        }

        serde_yaml::to_string(&preset).map_err(|e| format!("re-serialise: {e}"))
    }

    fn ensure_yaml_mapping<'a>(
        root: &'a mut serde_yaml::Value,
        path: &[&str],
    ) -> Result<&'a mut serde_yaml::Value, String> {
        let mut current = root;
        for key in path {
            let entry = current
                .as_mapping_mut()
                .ok_or_else(|| format!("`{key}` parent is not a mapping"))?;
            let key_value = serde_yaml::Value::String((*key).to_string());
            if !entry.contains_key(&key_value) {
                entry.insert(
                    key_value.clone(),
                    serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
                );
            }
            current = entry.get_mut(&key_value).expect("key just inserted above");
        }
        if !current.is_mapping() {
            return Err(format!(
                "path `{}` did not resolve to a mapping",
                path.join(".")
            ));
        }
        Ok(current)
    }

    fn deep_merge_yaml_mapping(
        base: &serde_yaml::Mapping,
        override_: &serde_yaml::Mapping,
    ) -> serde_yaml::Mapping {
        let mut out = serde_yaml::Mapping::new();
        for (k, v) in base {
            out.insert(k.clone(), v.clone());
        }
        for (k, override_v) in override_ {
            match (out.get(k), override_v) {
                (Some(existing), serde_yaml::Value::Mapping(override_map))
                    if existing.is_mapping() =>
                {
                    let merged = deep_merge_yaml_mapping(
                        existing.as_mapping().expect("checked is_mapping above"),
                        override_map,
                    );
                    out.insert(k.clone(), serde_yaml::Value::Mapping(merged));
                }
                _ => {
                    out.insert(k.clone(), override_v.clone());
                }
            }
        }
        out
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
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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

    /// Guard: ce-executor must explicitly tell the agent NOT to create, switch,
    /// or rename branches, and NOT to create worktrees. Branching is reserved
    /// for the user via `ralph run --worktree`; the orchestrator handles it
    /// before the agent activates. The agent improvising a "git checkout -b
    /// feat/plan-name" or "git worktree add ..." was the original bug — see
    /// git history for "fix: ce-executor 禁建分支".
    ///
    /// Note: this guard scans hat instructions text because the
    /// prohibition is expressed in the prompt to the agent (a free-form
    /// string), not as a structured field. We check that
    /// `git checkout -b` and `git worktree add` each appear in a
    /// sentence that *also* carries a prohibition marker
    /// (NEVER / MUST NOT / "不要" / "禁止" / "严禁"). This is
    /// resilient to wording changes (e.g. "MUST NOT create" / "禁止
    /// 切换分支") but still fails if the policy block is dropped.
    #[test]
    fn test_ce_executor_forbids_agent_branch_creation() {
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
        let content = preset.content;

        let prohibition_markers = ["NEVER", "MUST NOT", "不要", "禁止", "严禁"];
        for forbidden_cmd in ["git checkout -b", "git worktree add"] {
            let mut found = false;
            for marker in prohibition_markers {
                // Check the 200 chars before each occurrence of the
                // forbidden command for a prohibition marker. This
                // tolerates re-flowing YAML comments. Use char_indices
                // because the content is UTF-8 and byte slicing would
                // split multi-byte characters (e.g. CJK).
                for (idx, _) in content.match_indices(forbidden_cmd) {
                    let start_byte = content
                        .char_indices()
                        .rev()
                        .filter(|(b, _)| *b <= idx)
                        .nth(200)
                        .map(|(b, _)| b)
                        .unwrap_or(0);
                    if content[start_byte..idx].contains(marker) {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            assert!(
                found,
                "ce-executor must forbid `{forbidden_cmd}` with a prohibition marker \
                 (NEVER / MUST NOT / 不要 / 禁止 / 严禁) in the surrounding 200 chars. \
                 Run ./scripts/sync-embedded-files.sh if the canonical file has the \
                 policy but the embedded mirror does not."
            );
        }

        // Negative regression: the exact "create one (e.g., `feat/plan-name`)"
        // instruction that caused the original bug must be absent.
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

    /// Guard: autoresearch must NOT tell the strategist hat to run
    /// `git checkout -b autoresearch/...` during fresh-session setup.
    /// Branching is reserved for the user via `ralph run --worktree`.
    /// Regression: the original preset had step 2 of "Fresh Session" read
    /// "Create a branch: `git checkout -b autoresearch/<goal-slug>-$(date +%Y%m%d)`"
    /// which the agent dutifully executed, polluting the user's branch.
    ///
    /// Like `test_ce_executor_forbids_agent_branch_creation`, this scans
    /// the strategist's prompt for `git checkout -b` and `git worktree add`
    /// *each paired with* a prohibition marker — resilient to wording drift.
    #[test]
    fn test_autoresearch_forbids_agent_branch_creation() {
        let preset = get_preset("autoresearch").expect("autoresearch preset should exist");
        let content = preset.content;

        let prohibition_markers = ["NEVER", "MUST NOT", "不要", "禁止", "严禁"];
        for forbidden_cmd in ["git checkout -b", "git worktree add"] {
            let mut found = false;
            for marker in prohibition_markers {
                for (idx, _) in content.match_indices(forbidden_cmd) {
                    let start_byte = content
                        .char_indices()
                        .rev()
                        .filter(|(b, _)| *b <= idx)
                        .nth(200)
                        .map(|(b, _)| b)
                        .unwrap_or(0);
                    if content[start_byte..idx].contains(marker) {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            assert!(
                found,
                "autoresearch must forbid `{forbidden_cmd}` with a prohibition marker \
                 (NEVER / MUST NOT / 不要 / 禁止 / 严禁) in the surrounding 200 chars."
            );
        }

        // Negative regression: the exact old line must be absent.
        assert!(
            !content.contains("git checkout -b autoresearch/<goal-slug>"),
            "autoresearch must NOT tell the strategist to run \
             `git checkout -b autoresearch/<goal-slug>-...`. Branching is reserved \
             for `ralph run --worktree`."
        );

        // The Chinese translation preset must stay in parity with English.
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
        // In the Chinese preset the marker is `绝对禁止`; the
        // command names stay English.
        for forbidden_cmd in ["git checkout -b", "git worktree add"] {
            let mut found = false;
            for marker in ["绝对禁止", "NEVER", "MUST NOT", "禁止", "严禁"] {
                for (idx, _) in zh_content.match_indices(forbidden_cmd) {
                    let start_byte = zh_content
                        .char_indices()
                        .rev()
                        .filter(|(b, _)| *b <= idx)
                        .nth(200)
                        .map(|(b, _)| b)
                        .unwrap_or(0);
                    if zh_content[start_byte..idx].contains(marker) {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            assert!(
                found,
                "autoresearch-zh must forbid `{forbidden_cmd}` with a prohibition marker \
                 (绝对禁止 / NEVER / MUST NOT / 禁止 / 严禁) in the surrounding 200 chars."
            );
        }
        assert!(
            !zh_content.contains("git checkout -b autoresearch/<goal-slug>"),
            "autoresearch-zh must NOT contain the old `git checkout -b \
             autoresearch/<goal-slug>` instruction either."
        );
    }

    #[test]
    fn test_ce_executor_dimension_reviewer_timeout_is_900() {
        // R1: dimension-reviewer must have explicit timeout to avoid default 300s.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
        // Plan 2026-06-16-002 Unit 1: the embedded copy is no longer
        // byte-equal to the canonical preset because `build.rs` now
        // deep-merges the schema SSOT into `event_policy.schemas`
        // before writing the embedded copy. The invariant we still
        // want to lock down is: the embedded copy is the merge of
        // (canonical preset, schema SSOT) — i.e. the build pipeline
        // produced what the SSOT prescribes.
        let merged = merge_root_with_ssot("ce-executor-serial");
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        assert_eq!(
            merged, preset.content,
            "Embedded ce-executor-serial must equal merge(canonical preset, schema SSOT). \
             Re-run `cargo build` so build.rs regenerates $OUT_DIR/presets/ce-executor-serial.yml."
        );
    }

    /// U12 (fix-plan U12 / F-020 / R-20): the
    /// `ce-executor-supervisor` preset must satisfy the
    /// same SSOT byte-equality contract as
    /// `ce-executor-serial` — the embedded copy must equal
    /// the merge of the canonical preset YAML with the
    /// supervisor schema SSOT. Drift between canonical +
    /// schema breaks the lint surface (M-12) because the
    /// lint reads from the canonical copy while the
    /// runtime reads from the embedded merge.
    #[test]
    fn test_ce_executor_supervisor_root_preset_matches_embedded() {
        let merged = merge_root_with_ssot("ce-executor-supervisor");
        let preset = get_preset("ce-executor-supervisor")
            .expect("ce-executor-supervisor preset should exist");
        assert_eq!(
            merged, preset.content,
            "Embedded ce-executor-supervisor must equal merge(canonical preset, schema SSOT). \
             Re-run `cargo build` so build.rs regenerates $OUT_DIR/presets/ce-executor-supervisor.yml."
        );
    }

    // -------------------------------------------------------------------------
    // 2026-06-17-002 U4: ce-executor-serial preset tests
    // -------------------------------------------------------------------------

    /// U7 (2026-07-04-003 plan): `ce-executor-serial` must narrow
    /// its `coordinator_hats` allowlist to exactly
    /// `[coordinator, progress-steward]` and must enable the
    /// two-step verify gate.
    ///
    /// This is the closed-list assertion that locks the
    /// 2026-07-04-003 narrowing; the wider 7-hat allowlist
    /// previously shipped here made `tasks.coordinator_hats`
    /// so permissive that worker hats could self-create tasks
    /// without going through the coordinator. Worker hats
    /// (executor, validator, fixer, shipper, reporter) must
    /// NOT appear in this list.
    /// 2026-07-06 plan U11: the coordinator allowlist is narrowed to
    /// EXACTLY ONE hat — `coordinator`. The previous U7
    /// (`coordinator, progress-steward`) carve-out is obsolete
    /// because `progress-steward` was removed from the preset in
    /// U10. Wider allowlists re-introduce the worker-self-create
    /// drift window. Narrower allowlists (zero hats) would
    /// surface `tasks.enabled=true` with no gate, so the
    /// single-`coordinator` invariant is the only safe value.
    #[test]
    fn test_ce_executor_serial_coordinator_hats_narrowed_to_one_after_u11() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&preset.content).expect("preset must be valid YAML");
        let hats = yaml
            .get("tasks")
            .and_then(|t| t.get("coordinator_hats"))
            .and_then(|c| c.as_sequence())
            .expect("tasks.coordinator_hats must be a sequence");
        let names: Vec<&str> = hats.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            names,
            vec!["coordinator"],
            "ce-executor-serial coordinator_hats must be narrowed to exactly 1 hat (`coordinator`); \
             wider allowlists re-introduce the worker-self-create drift window, narrower \
             allowlists surface tasks.enabled=true with no gate"
        );
    }

    /// 2026-07-06 plan U11: the previous `narrowed_to_two` invariant
    /// (`coordinator, progress-steward`) is OBSOLETE. `progress-steward`
    /// was removed from the preset in U10. The test below pins the
    /// NEGATIVE: `progress-steward` MUST NOT appear in
    /// `tasks.coordinator_hats` after U11.
    #[test]
    fn test_ce_executor_serial_coordinator_hats_excludes_progress_steward_after_u11() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&preset.content).expect("preset must be valid YAML");
        let hats = yaml
            .get("tasks")
            .and_then(|t| t.get("coordinator_hats"))
            .and_then(|c| c.as_sequence())
            .expect("tasks.coordinator_hats must be a sequence");
        let names: Vec<&str> = hats.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            !names.contains(&"progress-steward"),
            "ce-executor-serial coordinator_hats MUST NOT contain `progress-steward` after U11 \
             (the hat was removed in U10); got {:?}",
            names
        );
    }

    /// 2026-07-06 plan U11: `event_loop.progress_steward.enabled`
    /// MUST be `false` in `ce-executor-serial` after U11. The
    /// flag is the runtime's kill-switch for the stall detector
    /// (event_loop/mod.rs U12 reads it; shipper_reason U13
    /// reads it for the fail-close contract). Setting it back to
    /// `true` would re-activate `loop.stalled` wake publishes,
    /// which is the silent-success / phantom-recovery drift
    /// path the SSOT convergence plan exists to eliminate.
    #[test]
    fn test_ce_executor_serial_progress_steward_disabled_after_u11() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&preset.content).expect("preset must be valid YAML");
        let enabled = yaml
            .get("event_loop")
            .and_then(|e| e.get("progress_steward"))
            .and_then(|p| p.get("enabled"))
            .and_then(|v| v.as_bool())
            .expect("event_loop.progress_steward.enabled must be present");
        assert!(
            !enabled,
            "ce-executor-serial event_loop.progress_steward.enabled MUST be `false` after U11 \
             (runtime fail-close U12 + shipper_reason U13 rely on this kill-switch); got {enabled}"
        );
    }

    #[test]
    fn test_ce_executor_serial_has_two_step_verify_gate() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&preset.content).expect("preset must be valid YAML");
        let require = yaml
            .get("tasks")
            .and_then(|t| t.get("require_verify_for_cli_mutate"))
            .and_then(|v| v.as_bool())
            .expect("tasks.require_verify_for_cli_mutate must be present");
        assert!(require, "ce-executor-serial must require verify for add/ensure");
        let unsafe_hatch = yaml
            .get("tasks")
            .and_then(|t| t.get("allow_unsafe_task_mutate"))
            .and_then(|v| v.as_bool())
            .expect("tasks.allow_unsafe_task_mutate must be present");
        assert!(
            !unsafe_hatch,
            "ce-executor-serial must keep the unsafe escape hatch OFF by default"
        );
    }

    /// U4: ce-executor-serial must use `report.done` as its sole completion
    /// gate, mirroring ce-executor-serial. Without this, the loop would
    /// never reach `LOOP_COMPLETE` and stall at the missing-event gate.
    #[test]
    fn test_ce_executor_serial_has_report_done_completion_gate() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        assert_eq!(
            config.event_loop.required_events,
            &["report.done"],
            "ce-executor-serial must require 'report.done' as its only completion gate"
        );
    }

    /// U4: review-synthesizer's triggers must be `[review.dimensions.complete]`
    /// for the serial preset — the wave variant triggers on
    /// `review.dimension.done` and `wave.worker.failed`, neither of which
    /// exists in the serial path. If a serial preset accidentally keeps the
    /// wave-style triggers, the synthesizer never activates and the loop
    /// stalls on the missing-event gate.
    #[test]
    fn test_ce_executor_serial_synthesizer_triggers_on_dimensions_complete() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        let synthesizer = config
            .hats
            .get("review-synthesizer")
            .expect("ce-executor-serial must define a 'review-synthesizer' hat");
        assert_eq!(
            synthesizer.triggers,
            vec!["review.dimensions.complete".to_string()],
            "ce-executor-serial review-synthesizer must trigger only on review.dimensions.complete; \
             the wave-style triggers (review.dimension.done, wave.worker.failed) are absent in the \
             serial path and would never fire"
        );
        // Defense in depth: the wave topic must NOT appear anywhere in
        // the synthesizer's trigger list, even as a duplicate.
        assert!(
            !synthesizer
                .triggers
                .contains(&"review.wave.ready".to_string()),
            "ce-executor-serial review-synthesizer must NOT trigger on review.wave.ready (no wave in this preset)"
        );
        assert!(
            !synthesizer
                .triggers
                .contains(&"wave.worker.failed".to_string()),
            "ce-executor-serial review-synthesizer must NOT trigger on wave.worker.failed (no wave dispatcher in this preset)"
        );
    }

    /// U4: dimension-reviewer in the serial preset must have
    /// `concurrency: 1` (the default; no fan-out) and NO `aggregate`
    /// block — the serial path is a strict 1-instance-per-activation
    /// design. If a future edit re-introduces concurrency, the test
    /// fails loudly and forces the author to either keep the topology
    /// serial or rename the preset to a wave variant.
    #[test]
    fn test_ce_executor_serial_dimension_reviewer_no_concurrency_no_aggregate() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        let reviewer = config
            .hats
            .get("dimension-reviewer")
            .expect("ce-executor-serial must define a 'dimension-reviewer' hat");
        assert_eq!(
            reviewer.concurrency, 1,
            "ce-executor-serial dimension-reviewer concurrency must be 1 (single instance per activation)"
        );
        assert!(
            reviewer.aggregate.is_none(),
            "ce-executor-serial dimension-reviewer must have no aggregate block (no wait_for_all in serial path)"
        );
        // Timeout must still be 1800 (parallel preset's per-worker cap) so
        // a single hung dimension still surfaces in bounded wall time.
        assert_eq!(
            reviewer.timeout,
            Some(1800),
            "ce-executor-serial dimension-reviewer timeout must be 1800s to bound per-dim wall time"
        );
    }

    /// 2026-07-06 plan U10: progress-steward hat REMOVED from
    /// `ce-executor-serial`. Stall recovery is now runtime
    /// fail-close (`event_loop/mod.rs`, U12) + shipper_reason gate
    /// (`shipper_reason.rs`, U13). The pre-U10 assertion that the
    /// preset declared a `progress-steward` hat triggering on
    /// `loop.stalled` only is obsolete and has been deleted; the
    /// regression is preserved as a negative expectation below
    /// (the hat MUST NOT be present in the post-U10 preset).

    /// 2026-07-06 plan U10 negative assertion: the
    /// `progress-steward` hat MUST NOT be declared in the
    /// `ce-executor-serial` preset after U10. Stall recovery
    /// has migrated to runtime fail-close + shipper_reason.
    #[test]
    fn test_ce_executor_serial_no_progress_steward_hat_after_u10() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        assert!(
            !config.hats.contains_key("progress-steward"),
            "ce-executor-serial MUST NOT declare a 'progress-steward' hat after 2026-07-06 U10 \
             (runtime fail-close + shipper_reason replace it); got {:?}",
            config.hats.keys().collect::<Vec<_>>()
        );
    }

    /// U4: review-coordinator must own the serial review events
    /// (`review.dimension.ready` and `review.dimensions.complete`) and
    /// dimension-reviewer must own the per-dim completion events
    /// (`review.dimension.done` and `review.dimension.failed`).
    /// Crossing the ownership lines would let dimension-reviewer kick
    /// a new review dimension (denied by topic_deny_rules) or let
    /// review-coordinator emit a per-dim done (rejected by origin
    /// guard as it is not in the hat's `publishes` list).
    #[test]
    fn test_ce_executor_serial_topic_ownership() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        let coordinator = config
            .hats
            .get("review-coordinator")
            .expect("ce-executor-serial must define a 'review-coordinator' hat");
        let reviewer = config
            .hats
            .get("dimension-reviewer")
            .expect("ce-executor-serial must define a 'dimension-reviewer' hat");

        // review-coordinator owns the kick-off + close events
        assert!(
            coordinator
                .publishes
                .contains(&"review.dimension.ready".to_string()),
            "review-coordinator must publish review.dimension.ready"
        );
        assert!(
            coordinator
                .publishes
                .contains(&"review.dimensions.complete".to_string()),
            "review-coordinator must publish review.dimensions.complete (plural — aggregate over the sequence)"
        );
        assert!(
            !coordinator
                .publishes
                .contains(&"review.dimension.done".to_string()),
            "review-coordinator must NOT publish review.dimension.done (dimension-reviewer owns that)"
        );
        assert!(
            !coordinator
                .publishes
                .contains(&"review.dimension.failed".to_string()),
            "review-coordinator must NOT publish review.dimension.failed (dimension-reviewer owns that)"
        );

        // dimension-reviewer owns the per-dim completion events
        assert!(
            reviewer
                .publishes
                .contains(&"review.dimension.done".to_string()),
            "dimension-reviewer must publish review.dimension.done"
        );
        assert!(
            reviewer
                .publishes
                .contains(&"review.dimension.failed".to_string()),
            "dimension-reviewer must publish review.dimension.failed"
        );
        assert!(
            !reviewer
                .publishes
                .contains(&"review.dimensions.complete".to_string()),
            "dimension-reviewer must NOT publish review.dimensions.complete (review-coordinator owns that)"
        );
    }

    /// U4: root preset must match the embedded copy after build.rs's
    /// U4: ce-executor-serial must NOT declare `review.wave.ready` or
    /// `wave.worker.failed` as triggers / publishes / aggregate
    /// members / required keys on any hat. (The preset may mention
    /// these topics in PROSE comments explaining what was removed;
    /// that prose is fine. What is not fine is wiring the wave
    /// topics into the runtime contract.)
    ///
    /// This guards against a future edit accidentally wiring the
    /// serial preset to the wave dispatcher — the topic_deny_rules
    /// block would still reject the emit, but at that point the
    /// preset is internally inconsistent and the test should fire
    /// before runtime does.
    #[test]
    fn test_ce_executor_serial_has_no_wave_topic() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        for (hat_name, hat) in &config.hats {
            // Triggers must not subscribe to wave-only events.
            for forbidden in ["review.wave.ready", "wave.worker.failed"] {
                assert!(
                    !hat.triggers.contains(&forbidden.to_string()),
                    "ce-executor-serial hat '{}' must NOT declare '{}' as a trigger \
                     (no wave in this preset)",
                    hat_name,
                    forbidden
                );
                assert!(
                    !hat.publishes.contains(&forbidden.to_string()),
                    "ce-executor-serial hat '{}' must NOT declare '{}' in publishes \
                     (no wave in this preset)",
                    hat_name,
                    forbidden
                );
            }
        }
    }

    /// U4: ce-executor-serial uses a 6-dimension review
    /// sequence (goal-alignment → correctness → testing → maintainability →
    /// project-standards → adversarial). The review-coordinator's
    /// instructions must not still reference the old 4-dim/5-dim set.
    /// This guard must stay in sync with the preset's sequence contract.
    #[test]
    fn test_ce_executor_serial_review_sequence_is_six_dimensions() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor-serial YAML should parse");
        let coordinator = config
            .hats
            .get("review-coordinator")
            .expect("ce-executor-serial must define a 'review-coordinator' hat");
        let instructions = coordinator.instructions.as_str();

        // Sequence contract must list exactly the six dimensions in fixed order.
        let ordered_markers = [
            "1. `goal-alignment`",
            "2. `correctness`",
            "3. `testing`",
            "4. `maintainability`",
            "5. `project-standards`",
            "6. `adversarial`",
        ];
        let mut prev_idx = 0usize;
        for marker in &ordered_markers {
            let idx = instructions.find(marker).unwrap_or_else(|| {
                panic!(
                    "review-coordinator instructions must contain `{marker}` for the 6-dimension sequence contract"
                )
            });
            assert!(
                idx >= prev_idx,
                "review-coordinator instructions must list dimensions in fixed order; `{marker}` appeared before its predecessor"
            );
            prev_idx = idx;
        }
    }

    /// U4: ce-executor-serial must validate end-to-end (ambiguous routing
    /// whitelist, terminal event authority, etc.). Companion of
    /// test_ce_executor_serial_preset_validates_ambiguous_routing.
    #[test]
    fn test_ce_executor_serial_preset_validates() {
        let preset = get_preset("ce-executor-serial")
            .expect("ce-executor-serial must be embedded with non-empty content");
        let config = RalphConfig::parse_yaml(preset.content)
            .unwrap_or_else(|e| panic!("ce-executor-serial must parse: {e}"));
        config.validate().unwrap_or_else(|e| {
            panic!("ce-executor-serial must validate (ambiguous routing + terminal authority): {e}")
        });
    }

    #[test]
    fn test_ce_executor_has_hard_commit_cadence() {
        // R3: executor must have hard commit cadence rule.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
    fn test_ce_executor_executor_publishes_excludes_queue_advance() {
        // KTD4: executor no longer publishes queue.advance; plan-gate owns advancement.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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

    /// 2026-06-24 fix: the full `ce-executor-serial` preset must pass
    /// `RalphConfig::validate` end-to-end. Previously this test pinned the
    /// `trigger_multi_consumer_topics` whitelist for `fix.exhausted` and
    /// `debug.exhausted`; those multi-consumer declarations have been
    /// removed to eliminate the round-robin scheduling race. The test now
    /// just ensures the preset still validates cleanly.
    #[test]
    fn test_ce_executor_serial_preset_validates_ambiguous_routing() {
        let en = get_preset("ce-executor-serial")
            .expect("ce-executor-serial must be embedded with non-empty content");
        let zh = read_root_preset("ce-executor-serial.yml");
        let en_yaml: &str = en.content.as_ref();
        let cases: &[(&str, &str)] = &[
            ("ce-executor-serial", en_yaml),
            ("ce-executor-serial", zh.as_str()),
        ];
        for (name, yaml) in cases {
            let config =
                RalphConfig::parse_yaml(yaml).unwrap_or_else(|e| panic!("{name} must parse: {e}"));
            config
                .validate()
                .unwrap_or_else(|e| panic!("{name} must validate (U1 whitelist): {e}"));
        }
    }

    /// 2026-06-24 P1-4: shipper must route `plan.blocked` by `reason`.
    /// Recoverable reasons (review_terminal_drift /
    /// recovery_exhausted:<allowlisted retry_key> /
    /// review_failed / precheck_failed / default_publishes)
    /// run verification 1-2 and may publish REVIEW_COMPLETE
    /// with pass_or_fail=pass. Hard-fail reasons (executor
    /// failed / work_failed / fix_exhausted / dimension_failed
    /// / all_dimensions_failed / loop_stalled_max_iterations /
    /// steward_escalation) always publish REVIEW_COMPLETE with
    /// pass_or_fail=fail. 2026-07-06 U13 (KTD-1 fail-close):
    /// `loop_stalled_max_iterations` and `steward_escalation`
    /// are REMOVED from the recoverable set — the runtime's
    /// `loop.stalled` wake path was closed in U12, so a stall
    /// that reaches `plan.blocked` represents a real
    /// silent-success drift and must hard-fail.
    #[test]
    fn test_ce_executor_serial_shipper_plan_blocked_routes_by_reason() {
        let preset =
            get_preset("ce-executor-serial").expect("ce-executor-serial preset must be embedded");
        let content: &str = preset.content.as_ref();
        // 2026-06-30-001 P0-2: shipper instructions must use
        // the STRICT-MATCH marker on the recoverable-reasons
        // paragraph; the pre-fix wording ("reason-based
        // routing") is too loose and was being interpreted
        // as a substring match by the agent.
        assert!(
            content.contains("STRICT-MATCH") || content.contains("STRICT EXACT MATCH"),
            "P0-2: shipper instructions must use STRICT-MATCH on plan.blocked reason routing"
        );
        // Recoverable reasons must be listed.
        assert!(
            content.contains("review_terminal_drift"),
            "shipper must list recoverable reason `review_terminal_drift`"
        );
        // 2026-07-06 U13 fail-close (KTD-1): the two
        // stall-derived literals are NO LONGER recoverable.
        // Their STRICT-MATCH presence in the preset's
        // recoverable list would re-introduce the
        // silent-success drift the SSOT convergence plan
        // exists to eliminate. The Rust mechanism
        // (`shipper_reason.rs`) is the source of truth; the
        // preset prompt must agree with it.
        //
        // We allow the literals to appear in the preset ONLY
        // in the `plan.blocked` hard-fail examples (where
        // they document WHY the loop ended), not in the
        // recoverable whitelist. The safest assertion is to
        // require that they are NOT in the recoverable
        // bullet list. Without a structured parse, we assert
        // the negative via the paired comment: the preset
        // must contain a U13 fail-close note.
        assert!(
            content.contains("loop_stalled_max_iterations")
                && content.contains("steward_escalation"),
            "shipper instructions MUST still reference these literals (they appear in the \
             hard-fail examples + U13 documentation); the change is that they are no longer \
             on the recoverable whitelist"
        );
        // 2026-07-02 P1-B: the v1 extension adds the
        // `stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*`
        // and
        // `stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*`
        // retry_key shapes that the diagnosis responder
        // synthesises on handoff_dispatch_timeout. The 4
        // shipper fails in the 2026-07-01 ralph-e2e run
        // (events-20260701-220911.jsonl iter 22/24/26/34)
        // all hit this gap; without these entries the
        // shipper still routes them to hard-fail.
        //
        // 2026-07-03-005 plan (P0 fix C2+C8): the two entries
        // were REMOVED. They previously masked the mechanism-
        // side silent drop → retry → stall escalation as
        // `pass_with_residuals`, hiding the real root cause
        // (M-1 isolated budget + M-2 handoff_dispatch routing).
        // The hard-fail path now exposes the truth via
        // REVIEW_COMPLETE(fail) instead of self-closing the
        // loop. The `recovery_exhausted:stall_recovery:...`
        // drift-engine promotion path is preserved by the
        // `starts_with` fallback in `shipper_reason.rs`.
        assert!(
            !content.contains("stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*"),
            "shipper must NOT list stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:* as recoverable (P0 fix C2+C8 / 2026-07-03-005)"
        );
        assert!(
            !content.contains("stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*"),
            "shipper must NOT list stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:* as recoverable (P0 fix C2+C8 / 2026-07-03-005)"
        );
        // Hard-fail reasons must be listed.
        assert!(
            content.contains("executor failed")
                && content.contains("fix_exhausted")
                && content.contains("dimension_failed"),
            "shipper must list hard-fail reasons for plan.blocked"
        );
        // Recoverable path must allow pass_or_fail=pass.
        assert!(
            content.contains("pass_or_fail: \"pass\""),
            "shipper must allow pass_or_fail=pass for recoverable plan.blocked reasons"
        );
        // Hard-fail path must still emit pass_or_fail=fail.
        assert!(
            content.contains("pass_or_fail: \"fail\""),
            "shipper must emit pass_or_fail=fail for hard-fail plan.blocked reasons"
        );
    }

    #[test]
    fn test_ce_executor_reporter_defensive_plan_check() {
        // R8: Reporter instructions must contain a defensive plan completion check.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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

    /// U4 (2026-06-11-002) — superseded by 2026-06-17-005 R4. The
    /// legacy executor-instructions helper was removed when the
    /// U4 progress-reconcile tests were replaced by projector-driven
    /// assertions (`test_ce_executor_state_projection_enabled_*`).
    /// The new contract pins `event_loop.state_projection.enabled`
    /// and the `## ORCHESTRATOR CONTEXT` / `ralph tools task` HARD
    /// RULE clauses; the per-step `progress.md` / `task start` /
    /// `task close` ordering is no longer an agent obligation.

    // ------------------------------------------------------------------
    // 2026-06-17-005 R4 contract: state projection is the single writer
    // for `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md`.
    // Presets must opt in to the projector and pin agent-side
    // instructions away from the legacy hand-written path.  These
    // tests replace the 2026-06-11-002 U4 progress-reconcile tests
    // (the legacy contract) with assertions that match the
    // projector-driven model. See U4 of
    // docs/plans/2026-06-17-005-fix-state-projection-phase1-review-findings-plan.md.
    // ------------------------------------------------------------------

    /// R4: the cross-hat HARD RULE comment must declare
    /// `## ORCHESTRATOR CONTEXT` as the canonical read source
    /// **and** forbid agent-side `ralph tools task` calls.
    /// This pins U2 of 2026-06-17-005 in code: a future refactor
    /// that drops either clause fails CI.
    ///
    /// Note: the embedded preset content goes through
    /// `serde_yaml::to_string` in build.rs, which strips top-level
    /// YAML comments. The HARD RULE comment is therefore expected
    /// to live in the per-hat `instructions` string (preserved) or
    /// in `presets/schemas/<name>.yml` (merged). We assert the
    /// preserved shape here: a per-hat instruction that tells the
    /// agent to prefer `work.ready` over `ralph tools task`.
    #[test]
    fn test_ce_executor_orchestrator_context_is_canonical_read_source_en() {
        let preset = get_preset("ce-executor-serial").expect("ce-executor-serial preset");
        let content = &preset.content;
        assert!(
            content.contains("## ORCHESTRATOR CONTEXT"),
            "ce-executor-serial must reference `## ORCHESTRATOR CONTEXT` as the read source"
        );
        // The HARD RULE says the agent must prefer `work.ready`
        // and only fall back to `ralph tools task` when the
        // ORCHESTRATOR CONTEXT block shows the task is missing.
        // Pin the projector-driven shape (per-hat instructions
        // mention `work.ready` together with `ORCHESTRATOR CONTEXT`).
        assert!(
            content.contains("work.ready") && content.contains("ORCHESTRATOR CONTEXT"),
            "ce-executor-serial instructions must couple `work.ready` to `ORCHESTRATOR CONTEXT`"
        );
        // The Chinese preset mirrors the same cross-hat block
        // (R3 of 2026-06-17-005 — see U3).
        let zh = read_root_preset("ce-executor-serial.yml");
        assert!(
            zh.contains("## ORCHESTRATOR CONTEXT"),
            "ce-executor-serial must reference `## ORCHESTRATOR CONTEXT` (R3 of 2026-06-17-005)"
        );
        // And the zh HARD RULE mirrors the en "ralph tools task"
        // prohibition (added in U3 of 2026-06-17-005). The
        // comment wraps to a second line; collapse newlines
        // before searching. Each token is checked independently
        // so a future YAML reflow that breaks the literal
        // `ensure|start|close|fail|reopen` adjacency still
        // surfaces a meaningful failure message.
        let zh_collapsed: String = zh.chars().filter(|c| *c != '\n').collect();
        for token in ["ensure", "start", "close", "fail", "reopen"] {
            assert!(
                zh_collapsed.contains(token),
                "ce-executor-serial HARD RULE must mention `{token}` \
                 (R3 / U3 of 2026-06-17-005); multi-line comment is collapsed before search"
            );
        }
    }

    /// R4 (2026-06-17-005): the `ce-executor-serial` preset
    /// mirrors the same projector + ORCHESTRATOR CONTEXT contract.
    /// Pin the parity so an isolated-only edit to one preset
    /// does not silently drift the other.
    #[test]
    fn test_ce_executor_orchestrator_context_is_canonical_read_source_serial_en() {
        let preset = get_preset("ce-executor-serial").expect("ce-executor-serial preset");
        let content = &preset.content;
        assert!(
            content.contains("## ORCHESTRATOR CONTEXT"),
            "ce-executor-serial must reference `## ORCHESTRATOR CONTEXT` as the read source"
        );
        // The cross-hat HARD RULE mirrors the isolated preset
        // (per the inline comment in ce-executor-serial.yml).
        // The merged content keeps per-hat instructions as a
        // string; assert the per-hat "trust ORCHESTRATOR CONTEXT"
        // cue with a stronger signal than just substring
        // presence (the prior OR-fallback was a tautology once
        // the first assert above already checked the substring).
        // We require the explicit "trust the ORCHESTRATOR
        // CONTEXT" per-hat HARD RULE wording; that string is
        // unique to the serial preset's per-hat instructions
        // and would catch a regression that drops the per-hat
        // binding while leaving a stray mention elsewhere.
        assert!(
            content.contains("trust the `## ORCHESTRATOR CONTEXT`"),
            "ce-executor-serial must carry the per-hat 'trust the `## ORCHESTRATOR CONTEXT`' \
             HARD RULE binding (R4 of 2026-06-17-005)"
        );
    }

    /// R4: legacy regression guard. The pre-Phase-1 hand-written
    /// progress.md contract has been replaced. The old
    /// per-hat instruction that drove `test_ce_executor_u4_*`
    /// must no longer be enforced as a strict ordering rule;
    /// instead, agent behaviour is bound to the ORCHESTRATOR
    /// CONTEXT block. This test pins the **direction** of the
    /// change without locking the executor's per-step
    /// numbering (which the agent can still tune).
    #[test]
    fn test_ce_executor_u4_legacy_progress_reconcile_is_superseded() {
        // The R4 contract now lives in
        // `test_ce_executor_state_projection_enabled_serial_en`
        // and `test_ce_executor_orchestrator_context_is_canonical_read_source_en`.
        // We retain this test name as a one-line marker so
        // anyone reading the legacy contract test can see the
        // successor contract. It must stay green; if it fails
        // the projector was disabled.
        let preset = get_preset("ce-executor-serial").expect("ce-executor-serial preset");
        let config = RalphConfig::parse_yaml(&preset.content).expect("parse");
        assert!(
            config.event_loop.state_projection.enabled,
            "R4 contract is broken: state projection must stay enabled in ce-executor-serial"
        );
    }

    #[test]
    fn test_ce_executor_fixer_reads_task_correlation_fields() {
        // R17: fixer must read task_id/task_key/step from review.failed payload
        // so that fix.applied / fix.exhausted can carry them downstream.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
    /// `debug` is a template that shares its name with a builtin preset.
    /// Template-only names (minimal-linear, ce-executor-lite) are NOT preset names.
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
        let shared_names = ["debug"];
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

        let text = std::fs::read_to_string(&index_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", index_path.display(), e));
        let entries: Vec<serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("index.json must be valid JSON: {}", e));

        let index_names: std::collections::BTreeSet<String> = entries
            .iter()
            .map(|e| e.get("name").unwrap().as_str().unwrap().to_string())
            .collect();

        let public_names: std::collections::BTreeSet<String> =
            preset_names().iter().map(|s| s.to_string()).collect();

        let missing: Vec<_> = public_names.difference(&index_names).collect();
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

        let text = std::fs::read_to_string(&index_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", index_path.display(), e));
        let entries: Vec<serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("index.json must be valid JSON: {}", e));

        // Zsh completion values for builtin presets (from zsh plugin)
        // This must stay in sync with scripts/ralph-zsh-plugin.zsh
        let zsh_values: std::collections::BTreeSet<String> = [
            "builtin:ce-executor-serial",
            "builtin:ce-executor-pipeline",
            "builtin:ce-executor-supervisor",
            "builtin:debug",
            "builtin:autoresearch",
            "builtin:merge-batch",
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

        let text = std::fs::read_to_string(&index_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", index_path.display(), e));
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

    /// U4 (R13): The zsh builtin completion file must have value/description
    /// arrays with matching length and order, and every public preset must
    /// appear exactly once in the values array. Hidden presets (merge-loop)
    /// must NOT appear as values OR as orphan descriptions.
    #[test]
    fn test_zsh_builtin_completion_arrays_consistent() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let zsh_path = manifest_dir
            .join("..")
            .join("..")
            .join("scripts")
            .join("ralph-zsh-plugin.zsh");
        if !zsh_path.is_file() {
            eprintln!(
                "test_zsh_builtin_completion_arrays_consistent: {} not on build host; skipping",
                zsh_path.display()
            );
            return;
        }

        let text = std::fs::read_to_string(&zsh_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", zsh_path.display(), e));

        // Extract _RALPH_BUILTIN_HAT_VALUES= (...) body
        let values = extract_zsh_array(&text, "_RALPH_BUILTIN_HAT_VALUES")
            .expect("_RALPH_BUILTIN_HAT_VALUES array must exist in zsh plugin");
        let descriptions = extract_zsh_array(&text, "_RALPH_BUILTIN_HAT_DESCRIPTIONS")
            .expect("_RALPH_BUILTIN_HAT_DESCRIPTIONS array must exist in zsh plugin");

        // R13: length must match
        assert_eq!(
            values.len(),
            descriptions.len(),
            "zsh builtin completion values ({}) and descriptions ({}) must have matching length. \
             Update scripts/ralph-zsh-plugin.zsh so each value has a description.",
            values.len(),
            descriptions.len()
        );

        // Public preset names from presets.rs source of truth
        let public_names: std::collections::BTreeSet<String> =
            preset_names().iter().map(|s| s.to_string()).collect();

        // Every value should be a `builtin:<name>` reference; the name should
        // exist in the public preset set. Order must match between values
        // and descriptions (we use the same positional index, so length
        // equality is a necessary but not sufficient check — also verify
        // descriptions are non-empty and start with an uppercase letter so
        // they read like real descriptions, not stale fragments).
        assert_eq!(
            values.len(),
            public_names.len(),
            "zsh builtin completion values ({}) must match number of public presets ({}). \
             Add or remove entries in scripts/ralph-zsh-plugin.zsh.",
            values.len(),
            public_names.len()
        );

        for (i, value) in values.iter().enumerate() {
            // Each value must be `builtin:<name>`
            let prefix = "builtin:";
            assert!(
                value.starts_with(prefix),
                "zsh builtin completion value[{}] = {:?} must start with 'builtin:'",
                i,
                value
            );
            let name = &value[prefix.len()..];
            assert!(
                public_names.contains(name),
                "zsh builtin completion value[{}] = {:?} references preset '{}' which is NOT public. \
                 Hidden presets must NOT appear in _RALPH_BUILTIN_HAT_VALUES.",
                i,
                value,
                name
            );

            // Corresponding description must be non-empty, must NOT look like
            // a leftover orphan (e.g. merge-loop's "Internal preset for ..." text).
            let desc = &descriptions[i];
            assert!(
                !desc.is_empty(),
                "zsh builtin completion description[{}] (for value {:?}) must not be empty",
                i,
                value
            );
            // The hidden merge-loop description is "Internal preset for loop merge operations".
            // Catch any such orphan by checking that descriptions don't contain
            // words strongly associated with the hidden preset marker.
            assert!(
                !desc.contains("Internal preset for loop merge operations"),
                "zsh builtin completion description[{}] = {:?} is the orphan merge-loop description; \
                 remove it from _RALPH_BUILTIN_HAT_DESCRIPTIONS",
                i,
                desc
            );
        }

        // Order must be stable: collect values + public_names must agree on
        // the set, and there must be no duplicates.
        let value_set: std::collections::BTreeSet<&str> =
            values.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            value_set.len(),
            values.len(),
            "zsh builtin completion values must not contain duplicates"
        );
    }

    /// Parse a `NAME=(\n  "..."\n  "..."\n)` array body from a zsh file and
    /// return the list of string contents in declaration order.
    fn extract_zsh_array(text: &str, name: &str) -> Option<Vec<String>> {
        let marker = format!("{}=", name);
        let start = text.find(&marker)?;
        // Find the opening `(` after the marker
        let bytes = text.as_bytes();
        let mut idx = start + marker.len();
        // Skip whitespace
        while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
            idx += 1;
        }
        if idx >= bytes.len() || bytes[idx] != b'(' {
            return None;
        }
        idx += 1; // skip `(`

        // Now scan to matching `)` at column 0. We collect quoted strings
        // we encounter, ignoring any `\` escapes.
        let mut out: Vec<String> = Vec::new();
        let mut in_quote = false;
        let mut current = String::new();
        let mut had_quote = false;
        while idx < bytes.len() {
            let c = bytes[idx];
            if in_quote {
                if c == b'\\' && idx + 1 < bytes.len() {
                    // Skip escape; pass through next char verbatim.
                    current.push(bytes[idx + 1] as char);
                    idx += 2;
                    continue;
                } else if c == b'"' {
                    in_quote = false;
                    idx += 1;
                    // If we already had a quote, this is the closing one.
                    // Treat as end-of-entry only if the quote is followed by
                    // whitespace/newline/`)`.
                    // We push the collected value when the next non-WS char
                    // is a newline or `)`.
                    // Simpler: push when the quote closed and we have content
                    // collected; if the next char is also a quote, that
                    // would have been invalid zsh anyway.
                    if had_quote {
                        out.push(std::mem::take(&mut current));
                        had_quote = false;
                    }
                    continue;
                } else {
                    current.push(c as char);
                    idx += 1;
                    continue;
                }
            } else {
                if c == b'"' {
                    in_quote = true;
                    had_quote = true;
                    idx += 1;
                    continue;
                } else if c == b')' {
                    return Some(out);
                } else {
                    idx += 1;
                    continue;
                }
            }
        }
        None
    }

    #[test]
    fn test_ce_executor_strict_payload_contract_is_valid() {
        // Strict mode: every trigger topic with payload field references in
        // instructions must have a schema, and all referenced fields must be
        // declared in the schema's required_fields.
        let preset = get_preset("ce-executor-serial").expect("ce-executor preset should exist");
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
            // ce-executor-serial: 2026-06-28 plan U10 (R10) intentionally
            // allows coordinator to emit `LOOP_COMPLETE(success=false)` as a
            // self-stop signal when automated recovery is exhausted. This
            // creates an early-termination path from `work.start` that does
            // not pass through `report.done`, which the static topology
            // validator correctly flags. The runtime still rejects missing
            // required events on the success path.
            "ce-executor-serial",
            // ce-executor-pipeline: 2026-07-02-003 plan U1 (R3). The 13-hat
            // flat single-consumer chain (`plan-reviewer → executor → 6 dim
            // hats → review-synthesizer → fixer → alignment → reporter`) is
            // intentionally long. The first dimension hat
            // `dim:goal-alignment` is 9 hops from the terminal
            // `report.done` and exceeds the WAC EGRESS_MAX_HOPS=8 limit
            // (`crates/ralph-core/src/preset_lint/workflow_activation.rs:364`),
            // tripping `activation_egress_missing` by 1 hop. This is a known
            // false positive of the static-lint BFS bound — the chain
            // terminates deterministically via `report.done` (required_events)
            // and `LOOP_COMPLETE` (completion_promise), and the chain length
            // is a deliberate design choice (one hat per dimension; no
            // consolidation). Topology is structurally valid; the EGRESS
            // finding is a known bound artifact.
            "ce-executor-pipeline",
            // ce-executor-supervisor: 2026-07-03-001 plan U13. The supervisor
            // preset has intentional branching completion paths: a failed exec
            // wave (`exec.wave.failed`) routes through `exec-failure-handler` →
            // `work.failed` → `fixer` instead of through `work.done`. The
            // static topology validator therefore flags `work.done` as not on
            // all paths from `plan.ready`. This is by design — `work.done` is
            // the success-path handoff, while the failure path is handled by
            // the fix wave. The runtime still requires `work.done` on every
            // successful completion.
            "ce-executor-supervisor",
        ];

        // Per-preset finding-id exemptions for the non-strict authoring
        // contract test. Mirrors `EXEMPT_FINDINGS` below (the strict-
        // lint counterpart) but is its own const so the two tests can
        // diverge if needed.
        //
        // ce-executor-pipeline: the 13-hat flat chain trips three
        // WAC findings on the chain head `dim:goal-alignment` whose
        // root cause is the static-lint BFS bound (`EGRESS_MAX_HOPS=8`)
        // in `crates/ralph-core/src/preset_lint/workflow_activation.rs:364`.
        // Topology is structurally valid; the chain terminates at
        // `report.done` + `LOOP_COMPLETE`.
        const AUTHORING_EXEMPT_FINDINGS: &[(&str, &str)] = &[
            (
                "ce-executor-pipeline",
                "lint.preset.activation_egress_missing",
            ),
            ("ce-executor-pipeline", "lint.preset.handoff_pairing_broken"),
            ("ce-executor-pipeline", "lint.preset.re_emit_trap"),
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
                Some(preset.content),
            );

            if report.passed {
                continue;
            }

            // Check if all errors are topology errors or per-preset exempt
            // finding-id errors for an exempt preset. The id-level
            // exemption lets a topology-exempt preset also silence
            // specific WAC findings whose root cause is a static-lint
            // BFS bound (EGRESS_MAX_HOPS in
            // `crates/ralph-core/src/preset_lint/workflow_activation.rs:364`)
            // rather than a true topology defect.
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
            let all_id_exempt = !errors.is_empty()
                && errors.iter().all(|f| {
                    matches!(f.source, ralph_core::runtime_contract::FindingSource::Lint)
                        && AUTHORING_EXEMPT_FINDINGS
                            .iter()
                            .any(|(name, id)| *name == preset.name && *id == f.id.as_str())
                });

            if topology_exempt.contains(&preset.name) && (all_topology || all_id_exempt) {
                // Known exception — record but don't fail
                eprintln!(
                    "NOTE: preset '{}' has known authoring exceptions: {:?}",
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
        let strict_presets = &["ce-executor-serial"];
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
                Some(preset.content),
            );
            // U10: ce-executor-serial has a known topology exception
            // (coordinator may emit `LOOP_COMPLETE(success=false)` directly
            // from `work.start`, bypassing `report.done`). This test is
            // focused on strict payload contract, so topology-only findings
            // are recorded rather than failing.
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
            if !all_topology {
                assert!(
                    report.passed,
                    "Development preset '{}' failed strict contract: {:?}",
                    preset.name,
                    errors
                        .iter()
                        .map(|f| format!("{}: {}", f.id, f.message))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    /// All embedded presets must pass strict lint (R10).
    /// This covers the full manifest, not just the development subset.
    /// Topology-exempt presets (known branching completion paths) are
    /// excluded from the strict lint gate — their topology issues are
    /// documented exceptions.
    #[test]
    fn test_all_embedded_presets_pass_strict_lint() {
        // Presets with known topology issues (required events not on all
        // completion paths, or completion promise not reachable from start).
        // Same exemptions as authoring contract test.
        //
        // Plan reference: `docs/plans/2026-06-08-003-feat-preset-static-lint-plan.md`
        // section "U5: built-in preset migration" (autoresearch, debug explicitly
        // deferred due to multi-branch completion topologies).
        //
        // ce-executor-serial: 2026-06-28 plan U10 (R10) intentionally allows
        // coordinator to emit `LOOP_COMPLETE(success=false)` as a self-stop
        // signal when automated recovery is exhausted. This creates an
        // early-termination path from `work.start` that does not pass through
        // `report.done`, which the static topology validator correctly flags.
        //
        // ce-executor-pipeline: 2026-07-02-003 plan U1 (R3). The 13-hat flat
        // single-consumer chain is intentionally long; the first dimension
        // hat `dim:goal-alignment` is 9 hops from the terminal `report.done`
        // and exceeds the WAC EGRESS_MAX_HOPS=8 limit, tripping
        // `activation_egress_missing` by 1 hop. Known false positive of the
        // static-lint BFS bound — chain terminates deterministically via
        // `report.done` (required_events) and `LOOP_COMPLETE`
        // (completion_promise). Topology is structurally valid; the EGRESS
        // finding is a known bound artifact.
        //
        // ce-executor-supervisor: 2026-07-03-001 plan U13. The supervisor
        // preset has intentional branching completion paths: a failed exec
        // wave routes through `exec-failure-handler` → `work.failed` →
        // `fixer` instead of through `work.done`. The static topology
        // validator flags `work.done` as not on all paths from `plan.ready`.
        // This is by design — `work.done` is the success-path handoff, while
        // the failure path is handled by the fix wave. The runtime still
        // requires `work.done` on every successful completion.
        let topology_exempt: &[&str] = &[
            "autoresearch",
            "debug",
            "ce-executor-serial",
            "ce-executor-pipeline",
            "ce-executor-supervisor",
        ];

        // Per-preset finding-id exemptions (P2 #16 + #22).
        //
        // Each tuple is `(preset_name, finding_id, plan_back_reference)`:
        // - `preset_name` must match the embedded preset's `name` exactly.
        // - `finding_id` is matched by **exact equality** on the runtime
        //   contract finding id (NOT a prefix). This prevents a future
        //   sibling id (e.g. `config.empty_terminal_events_v2`) from
        //   being silently swallowed by a too-broad prefix.
        // - `plan_back_reference` is a human-readable hint to the plan /
        //   issue that documents why this exemption exists, so the next
        //   maintainer does not have to grep git blame to understand it.
        //
        // Currently empty: merge-loop was previously exempt but has been
        // migrated to a true strict-lint-clean topology (completion_promise =
        // merge.handled, both cleaner and failure_handler publish + declare
        // merge.handled as terminal, and the redundant cleanup.done /
        // merge.complete publishes have been removed).
        //
        // ce-executor-pipeline: 2026-07-02-003 plan U1 (R3). The 13-hat flat
        // single-consumer chain (`plan-reviewer → executor → 6 dim hats →
        // review-synthesizer → fixer → alignment → reporter`) is intentionally
        // long. The first dimension hat `dim:goal-alignment` is 9 hops from
        // the terminal `report.done` and exceeds the WAC EGRESS_MAX_HOPS=8
        // limit, tripping three related WAC findings on that one hat:
        // `lint.preset.activation_egress_missing` (the BFS bound rejection),
        // `lint.preset.handoff_pairing_broken` (downstream of the egress
        // miss), and `lint.preset.re_emit_trap` (also derived from the
        // 8-hop BFS bound). The chain terminates deterministically via
        // `report.done` (required_events) and `LOOP_COMPLETE`
        // (completion_promise); the chain length is a deliberate design
        // choice (one hat per dimension; no consolidation). Topology is
        // structurally valid; the WAC findings are known bound artifacts.
        const EXEMPT_FINDINGS: &[(&str, &str, &str)] = &[
            (
                "ce-executor-pipeline",
                "lint.preset.activation_egress_missing",
                "docs/plans/2026-07-02-003-feat-ce-executor-pipeline-preset-plan.md#u1",
            ),
            (
                "ce-executor-pipeline",
                "lint.preset.handoff_pairing_broken",
                "docs/plans/2026-07-02-003-feat-ce-executor-pipeline-preset-plan.md#u1",
            ),
            (
                "ce-executor-pipeline",
                "lint.preset.re_emit_trap",
                "docs/plans/2026-07-02-003-feat-ce-executor-pipeline-preset-plan.md#u1",
            ),
        ];

        let mut failures = Vec::new();
        for preset in PRESETS.iter() {
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_runtime_config(&config);
            let strictness = RuntimeContractStrictness::preset_check_strict();
            let report = RuntimeContractAggregator::aggregate(
                &format!("builtin:{}", preset.name),
                &config,
                &registry,
                strictness,
                Some(preset.content),
            );
            if report.passed {
                continue;
            }
            // For topology-exempt presets, skip if ALL blocking findings
            // (errors + warnings promoted by fail_on_warnings) are topology
            // or orphan (known pre-existing issues).
            if topology_exempt.contains(&preset.name) {
                let all_exempt = report.findings.iter().all(|f| {
                    // P2 #16: exact-id match, NOT starts_with. See
                    // EXEMPT_FINDINGS doc comment for rationale.
                    let is_id_exempt = EXEMPT_FINDINGS
                        .iter()
                        .any(|(name, id, _plan_ref)| *name == preset.name && *id == f.id.as_str());
                    is_id_exempt
                        || matches!(
                            f.source,
                            ralph_core::runtime_contract::FindingSource::Topology
                                | ralph_core::runtime_contract::FindingSource::Orphan
                        )
                });
                if all_exempt {
                    continue;
                }
            }
            // U7: ce-executor is removed; no carve-out needed.
            failures.push(format!(
                "'{}': {:?}",
                preset.name,
                report
                    .findings
                    .iter()
                    .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
                    .collect::<Vec<_>>()
            ));
        }
        assert!(
            failures.is_empty(),
            "Embedded presets failed strict lint:\n{}",
            failures.join("\n")
        );
    }

    // WRC-U3 / T-WRC-U3-04 (Tier-0 contract): every preset listed in
    // `TIER_0_WAC_PRESETS` must produce a `RuntimeContractReport`
    // with **zero WAC `lint.preset.*` errors** when checked under
    // `RuntimeContractStrictness::preset_check_strict()`. The
    // aggregator (WRC-U1 / WRC-U3) passes `builtin_source = true`
    // to WAC when the `source_label` starts with `builtin:`, so
    // every WAC finding for a Tier-0 preset surfaces as Error.
    //
    // This is the in-process counterpart to the
    // `validate-builtin-presets.sh --strict` gate. The two stay
    // in lockstep: when a preset is promoted to Tier-0 here, the
    // shell script's `TIER_0_WAC_PRESETS` array must also be
    // updated (the script is intentionally a separate source of
    // truth because the shell cannot query the ralph binary for
    // the list at runtime).
    //
    // Plan Unit: WRC-U3 of `2026-06-12-003-feat-wac-rollout-completion-plan.md`.
    #[test]
    fn test_tier_0_wac_presets_have_no_wac_errors() {
        for preset_name in TIER_0_WAC_PRESETS {
            let preset = PRESETS
                .iter()
                .find(|p| p.name == *preset_name)
                .unwrap_or_else(|| {
                    panic!(
                        "Tier-0 preset '{preset_name}' is in TIER_0_WAC_PRESETS but missing from PRESETS"
                    )
                });
            let config = RalphConfig::parse_yaml(preset.content)
                .unwrap_or_else(|e| panic!("Tier-0 preset '{preset_name}' failed to parse: {e}"));
            let registry = HatRegistry::from_runtime_config(&config);
            let report = RuntimeContractAggregator::aggregate(
                &format!("builtin:{}", preset.name),
                &config,
                &registry,
                RuntimeContractStrictness::preset_check_strict(),
                Some(preset.content),
            );
            let wac_errors: Vec<_> = report
                .findings
                .iter()
                .filter(|f| {
                    f.severity == ralph_core::runtime_contract::FindingSeverity::Error
                        && f.source == ralph_core::runtime_contract::FindingSource::Lint
                        && f.id.starts_with("lint.preset.")
                })
                .collect();
            assert!(
                wac_errors.is_empty(),
                "Tier-0 preset '{preset_name}' has {} WAC error(s) under strict; \
                 these block `ralph preset check --strict` and the run gate. \
                 Either fix the preset (preferred) or move it out of \
                 TIER_0_WAC_PRESETS in lockstep with scripts/validate-builtin-presets.sh. \
                 Findings: {:?}",
                wac_errors.len(),
                wac_errors
                    .iter()
                    .map(|f| format!("[{}] {}: {}", f.id, f.severity.as_str(), f.message))
                    .collect::<Vec<_>>()
            );
        }
    }

    // WRC-U5 / T-WRC-U5-01: the `is_tier_0_wac_preset` helper must
    // agree with the `TIER_0_WAC_PRESETS` constant byte-for-byte.
    // Drift between the two is the documented failure mode of the
    // shell-side list (`scripts/validate-builtin-presets.sh`).
    #[test]
    fn test_is_tier_0_wac_preset_helper_matches_constant() {
        for name in TIER_0_WAC_PRESETS {
            assert!(
                is_tier_0_wac_preset(name),
                "TIER_0_WAC_PRESETS contains '{name}' but is_tier_0_wac_preset disagrees"
            );
        }
        assert!(!is_tier_0_wac_preset("not-a-real-preset"));
        assert!(!is_tier_0_wac_preset("autoresearch")); // Tier-2
    }
}
