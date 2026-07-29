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
        description: "Linear single-chain plan-driven CE executor (Ralph primary path). Executor owns unit/subagent work; downstream hats run 6 serial dimension reviews → synthesize → fix → align → report → complete.",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/presets/ce-executor-pipeline.yml"
        )),
        public: true,
    },
    EmbeddedPreset {
        name: "ce-executor-pipeline-loop",
        description: "Review-loop CE executor: pipeline execution with serial six-dimension review, convergence-gated fix/re-review rounds, max 6 review rounds.",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/presets/ce-executor-pipeline-loop.yml"
        )),
        public: true,
    },
    // 2026-07-03-001 plan U13: supervisor parallel preset.
    // 16 functional hats + progress-steward. Built on top of the
    // `ce-executor-pipeline` topology; swaps the in-process WaveTracker
    // for the rusqlite-backed SupervisorStore (U2/U3/U5/U8/U12) and
    // exposes the six supervisor coordination topics via
    // `event_loop.supervisor.enabled: true` + isolated mode
    // (R-SW-1 lint enforces the contract).
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
    EmbeddedPreset {
        name: "post-merge-converge",
        description: "After one or more development plans have landed in the current branch: baseline, change map, six-dimension system audit, test-gap plan, per-finding reproduce/fix/regression, clean-env validation, and independent final review",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/post-merge-converge.yml")),
        public: true,
    },
    EmbeddedPreset {
        name: "parallel-forge",
        description: "Parallel Forge: Spec-First planning, supervisor-driven parallel Unit TDD in worktrees, serial integration with linear commits, full regression, audit, and manager report",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/parallel-forge.yml")),
        public: true,
    },
    // 2026-07-24-003 plan / KTD1: implementation-review — six-hat
    // isolated wave preset. Scope-preparer freezes baseline + patch +
    // digests; review-dispatcher emits a single six-payload
    // `review.unit.ready` wave; the runtime default wave hot path
    // injects `review.wave.complete` / `review.wave.failed` (no
    // supervisor execution model, no worktree slots); review-synthesizer
    // + fix-planner + finalizer produce one `LOOP_COMPLETE` with
    // `result` + `artifact_path`.
    EmbeddedPreset {
        name: "implementation-review",
        description: "Post-implementation six-dimension review: freeze scope, fan out a single SharedReadonly wave across goal-alignment / correctness / testing / maintainability / project-standards / adversarial, synthesize findings, and emit a fix-plan.md or block artifact as the terminal LOOP_COMPLETE",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/presets/implementation-review.yml"
        )),
        public: true,
    },
    EmbeddedPreset {
        name: "red-team-attack",
        description: "Experiment-driven Red Team analysis: reverse-locate plan commits from Git history, reconstruct patches, execute real attack experiments with control groups, apply hard-threshold evidence gating, and produce a zero-regression repair plan awaiting human confirmation",
        content: include_str!(concat!(env!("OUT_DIR"), "/presets/red-team-attack.yml")),
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
pub const TIER_0_WAC_PRESETS: &[&str] = &["ce-executor-pipeline"];

/// `true` if `preset_name` is in the Tier-0 list. Used by the CI
/// gate and by the test suite that asserts the Tier-0 preset
/// passes WAC strict.
#[allow(dead_code)] // 003 plan tiered-gates 预留：见 docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md
pub fn is_tier_0_wac_preset(preset_name: &str) -> bool {
    TIER_0_WAC_PRESETS.contains(&preset_name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::event_origin::{OriginCheck, validate_event_origin};
    use ralph_core::payload_contract::validate_payload_contract;
    use ralph_core::{HatRegistry, RalphConfig};

    #[test]
    fn test_minimal_preset_files_exclude_deleted_backends() {
        // U7: presets/minimal/ must no longer contain a per-backend yml
        // for any of the 5 deleted backends (amp, kiro, roo, copilot, kiro-acp).
        let preset_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("presets/minimal");
        let deleted = ["amp", "kiro", "roo", "copilot", "kiro-acp"];
        for name in deleted {
            let path = preset_dir.join(format!("{name}.yml"));
            assert!(
                !path.exists(),
                "{name}.yml must be removed from presets/minimal (backend deleted)"
            );
        }
    }

    #[test]
    fn test_zsh_plugin_backend_array_excludes_deleted_backends() {
        // U7: scripts/ralph-zsh-plugin.zsh `_RALPH_BACKENDS=( ... )` array
        // must no longer carry entries for the deleted backends.
        let plugin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("scripts/ralph-zsh-plugin.zsh");
        let src = std::fs::read_to_string(&plugin).expect("read zsh plugin");
        let deleted = ["kiro", "amp", "copilot", "roo"];
        for backend in deleted {
            // Each removed backend must no longer appear as a quoted entry
            // like `"<backend>:...` inside `_RALPH_BACKENDS=(...)`.
            let marker = format!("\"{backend}:");
            let occurrences: Vec<_> = src.matches(&marker).collect();
            assert!(
                occurrences.is_empty(),
                "_RALPH_BACKENDS array still references deleted backend `{backend}` ({occurrences:?} matches in {})",
                plugin.display()
            );
        }
    }

    #[test]
    fn test_zsh_plugin_backend_array_includes_agent() {
        // U5 (R3a): scripts/ralph-zsh-plugin.zsh `_RALPH_BACKENDS` must
        // expose `agent` so the user-facing completion matches the
        // backend registry surfaced by `ralph --backend agent`.
        let plugin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("scripts/ralph-zsh-plugin.zsh");
        let src = std::fs::read_to_string(&plugin).expect("read zsh plugin");
        assert!(
            src.contains("\"agent:Cursor Agent"),
            "_RALPH_BACKENDS must include the Cursor `agent` backend (R3a)"
        );
    }

    #[test]
    fn test_tools_evaluate_scripts_exclude_kiro() {
        // U7: tools/PRESET_EVALUATOR_PROMPT.md and tools/evaluate-*.sh must
        // no longer tell evaluators to use the deleted kiro-cli backend.
        let tools_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools");
        for entry in [
            "PRESET_EVALUATOR_PROMPT.md",
            "evaluate-all-presets.sh",
            "evaluate-preset.sh",
        ] {
            let path = tools_dir.join(entry);
            if path.exists() {
                let src = std::fs::read_to_string(&path).expect("read tool script");
                assert!(
                    !src.contains("kiro-cli") && !src.contains("\"kiro\""),
                    "tools/{entry} still references deleted `kiro` backend"
                );
            }
        }
    }

    #[test]
    fn test_changelog_records_backend_removal() {
        // U8: top-level CHANGELOG.md must declare the 5-backend removal
        // in `[Unreleased] ### Removed` (and ideally docs/reference/changelog.md too).
        let ws_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let mut changelogs = vec![ws_root.join("CHANGELOG.md")];
        let docs_chlog = ws_root.join("docs/reference/changelog.md");
        if docs_chlog.exists() {
            changelogs.push(docs_chlog);
        }
        let required_marker = "Removed backends: amp, roo, kiro, kiro-acp, copilot";
        let remaining_marker = "claude, gemini, codex, opencode, pi, traecli, custom";
        for path in changelogs {
            let text = std::fs::read_to_string(&path).expect("read changelog");
            assert!(
                text.contains(required_marker),
                "{} must declare `{required_marker}` in Removed section",
                path.display()
            );
            assert!(
                text.contains(remaining_marker),
                "{} should also list remaining backends `{remaining_marker}`",
                path.display()
            );
        }
    }

    #[test]
    fn test_docs_index_no_longer_links_deleted_backend_guides() {
        // U8: docs/guide/index.md must no longer link to the deleted
        // per-backend guides (kiro-migration.md, roo-backend.md).
        let ws_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let index_md = ws_root.join("docs/guide/index.md");
        let text = std::fs::read_to_string(&index_md).expect("read docs index");
        for deleted in ["kiro-migration.md", "roo-backend.md"] {
            assert!(
                !text.contains(deleted),
                "{index_md:?} still references deleted guide `{deleted}`"
            );
        }
    }

    #[test]
    fn test_cursor_rules_exclude_deleted_backends() {
        // U8: `.cursor/rules/architecture-modules.mdc` and `feature-flags.mdc`
        // backend lists must no longer mention any of the 5 deleted backends.
        let ws_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let deleted = [
            "kiro",
            "amp",
            "roo",
            "copilot",
            "kiro-acp",
            "kiro-cli",
            "copilot_stream",
        ];
        let rules = [
            ws_root.join(".cursor/rules/architecture-modules.mdc"),
            ws_root.join(".cursor/rules/feature-flags.mdc"),
        ];
        for path in rules {
            if path.exists() {
                let text = std::fs::read_to_string(&path).expect("read rule");
                for backend in deleted {
                    // Only flag clear canonical mentions (not arbitrary
                    // substrings inside unrelated identifiers).
                    let patterns = [
                        format!("`{backend}`"),
                        format!(" `{backend}`"),
                        format!("'{backend}'"),
                        format!("{backend}-acp"),
                    ];
                    for pat in patterns {
                        assert!(
                            !text.contains(&pat),
                            "{} still references deleted backend pattern `{pat}`",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_kiro_and_roo_dedicated_docs_removed() {
        // U8: dedicated per-backend docs for deleted backends must be gone.
        let ws_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        for deleted in ["kiro-migration.md", "roo-backend.md"] {
            let path = ws_root.join("docs/guide").join(deleted);
            assert!(
                !path.exists(),
                "expected deleted guide to be removed: {}",
                path.display()
            );
        }
    }

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

    // Unit 1 (plan 2026-07-07-006): single-chain execution primary path.
    // ce-executor-pipeline becomes the recommended CE executor;
    // ce-executor-serial is removed from the public builtin registry.
    // These assertions lock that contract at the registry boundary.

    /// Registry must expose `ce-executor-pipeline` as a public builtin.
    #[test]
    fn test_preset_names_contains_pipeline() {
        let names = preset_names();
        assert!(
            names.contains(&"ce-executor-pipeline"),
            "ce-executor-pipeline must be a public builtin; got {names:?}"
        );
        assert!(
            names.contains(&"ce-executor-pipeline-loop"),
            "ce-executor-pipeline-loop must be a public builtin; got {names:?}"
        );
    }

    #[test]
    fn test_ce_executor_pipeline_loop_routes_are_single_consumer() {
        let preset = get_preset("ce-executor-pipeline-loop")
            .expect("ce-executor-pipeline-loop must be embedded");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("loop preset YAML should parse");

        let mut consumers: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for (hat_id, hat) in &config.hats {
            for topic in &hat.triggers {
                consumers
                    .entry(topic.clone())
                    .or_default()
                    .push(hat_id.clone());
            }
        }

        for topic in [
            "work.done",
            "stabilization.done",
            "stabilization.blocked",
            "fix.done",
            "review.round.ready",
            "review.synthesized",
            "review.accepted",
            "fix.requested",
            "review.complete",
            "review.loop.blocked",
            "align.done",
        ] {
            let actual = consumers.get(topic).cloned().unwrap_or_default();
            assert_eq!(
                actual.len(),
                1,
                "topic {topic} must have exactly one explicit consumer, got {actual:?}"
            );
        }

        assert_eq!(
            consumers.get("work.done").cloned().unwrap_or_default(),
            vec!["test-stabilizer".to_string()]
        );
        assert_eq!(
            consumers
                .get("stabilization.done")
                .cloned()
                .unwrap_or_default(),
            vec!["review-reentry".to_string()]
        );
        assert_eq!(
            consumers
                .get("stabilization.blocked")
                .cloned()
                .unwrap_or_default(),
            vec!["reporter".to_string()]
        );
        assert_eq!(
            consumers.get("fix.done").cloned().unwrap_or_default(),
            vec!["review-reentry".to_string()]
        );
        assert_eq!(
            consumers.get("fix.requested").cloned().unwrap_or_default(),
            vec!["fix-planner".to_string()]
        );
        assert_eq!(
            consumers
                .get("review.complete")
                .cloned()
                .unwrap_or_default(),
            vec!["fixer".to_string()]
        );
    }

    #[test]
    fn test_ce_executor_pipeline_loop_fix_reentry_contract() {
        let preset = get_preset("ce-executor-pipeline-loop")
            .expect("ce-executor-pipeline-loop must be embedded");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("loop preset YAML should parse");

        let schemas = &config
            .event_loop
            .event_policy
            .as_ref()
            .expect("loop preset must declare event policy")
            .schemas;
        let fix_done = schemas
            .get("fix.done")
            .expect("loop preset must schema fix.done");
        assert!(
            fix_done
                .required_fields
                .iter()
                .any(|field| field == "next_review_plan"),
            "fix.done must require next_review_plan so null/missing payloads are rejected"
        );
    }

    /// Registry must NOT expose `ce-executor-serial` once Unit 1 is complete.
    #[test]
    fn test_preset_names_excludes_serial() {
        let names = preset_names();
        assert!(
            !names.contains(&"ce-executor-serial"),
            "ce-executor-serial must not appear in preset_names(); got {names:?}"
        );
    }

    /// Reverse lock test (plan 2026-07-07-006 fix-plan U1): the
    /// `ce-executor-lite` template's `source` field must point to
    /// the Ralph primary path `ce-executor-pipeline`, never to
    /// the removed `ce-executor-serial`. If a future edit re-points
    /// the template at a non-pipeline preset (or back at the
    /// deleted serial), `preset list` / `preset show` will mislead
    /// first-time readers who copy the printed `Source:` line.
    #[test]
    fn test_ce_executor_lite_source_points_to_pipeline() {
        let manifest = crate::preset_templates::TemplateCatalog::get_manifest("ce-executor-lite")
            .expect("ce-executor-lite template must exist");
        assert_eq!(
            manifest.source.as_deref(),
            Some("builtin:ce-executor-pipeline"),
            "ce-executor-lite template source must point to ce-executor-pipeline \
             (the Ralph primary path); got {:?}",
            manifest.source
        );
        // Belt-and-braces: the deleted serial must never re-appear.
        assert_ne!(
            manifest.source.as_deref(),
            Some("builtin:ce-executor-serial"),
            "ce-executor-lite template source must not point to removed ce-executor-serial"
        );
        // The raw template body must not mention the deleted preset either,
        // so the template cannot re-introduce serial through a side door.
        let body = crate::preset_templates::TemplateCatalog::raw_template("ce-executor-lite")
            .expect("ce-executor-lite raw template must exist");
        assert!(
            !body.contains("ce-executor-serial"),
            "ce-executor-lite raw template body must not mention the removed \
             ce-executor-serial preset anywhere; plan 2026-07-07-006 forbids it"
        );
    }

    /// `get_preset("ce-executor-serial")` must return `None`, not a redirect
    /// or an alias to a different preset.
    #[test]
    fn test_get_preset_serial_returns_none() {
        assert!(
            get_preset("ce-executor-serial").is_none(),
            "ce-executor-serial lookup must return None, not a redirect"
        );
    }

    // Unit 2 (plan 2026-07-07-006): pipeline schema static self-check.
    // Lock the registry's claim that pipeline's work.done schema already
    // carries the unit-evidence fields the executor needs (SC1), and
    // that pipeline does not depend on runtime unit-loop topics (SC2).
    // Read-only assertions — no pipeline mutation, no schema edits.

    /// Unit-evidence fields the executor must place in `work.done`
    /// per the executor-mode contract. Sourced from the shared
    /// `ralph_core::test_support::unit_evidence::UNIT_EVIDENCE_FIELDS`
    /// SSOT (plan 2026-07-07-006 fix-plan U7 / SR-M1); both this
    /// crate's lock test and the scenarios BDD in `ralph-core`
    /// reference the same constant so the two lock tests cannot
    /// drift apart on a future edit.
    use ralph_core::test_support::unit_evidence::UNIT_EVIDENCE_FIELDS;

    fn parse_pipeline_work_done_required_fields() -> std::collections::BTreeSet<String> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("en")
            .join("ce-executor-pipeline.yml");
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read pipeline preset at {}: {}", path.display(), e));
        let v: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse pipeline yaml");
        let seq = v["event_loop"]["event_policy"]["schemas"]["work.done"]["required_fields"]
            .as_sequence()
            .expect("event_loop.event_policy.schemas.work.done.required_fields must be a list");
        seq.iter()
            .map(|s| {
                s.as_str()
                    .unwrap_or_else(|| panic!("required_fields entries must be strings"))
                    .to_string()
            })
            .collect()
    }

    /// SC1: pipeline `work.done.required_fields` must cover all
    /// unit-evidence fields the executor mode promises. If this test
    /// fails, the plan's "no schema extension" invariant is broken —
    /// stop and raise to the user (per plan Unit 2 Step 2.5).
    #[test]
    fn test_pipeline_work_done_required_fields_covers_unit_evidence() {
        let required = parse_pipeline_work_done_required_fields();
        let needed: std::collections::BTreeSet<String> =
            UNIT_EVIDENCE_FIELDS.iter().map(|s| s.to_string()).collect();
        let missing: Vec<&String> = needed.difference(&required).collect();
        assert!(
            missing.is_empty(),
            "pipeline work.done required_fields is missing unit evidence fields: {missing:?}. \
             Per plan 2026-07-07-006 Unit 2, this would require expanding the pipeline schema, \
             which violates the Pipeline Hard Rule. Stop the plan and raise to the user."
        );
    }

    /// SC2 static lock: pipeline must not reference any runtime
    /// unit-loop topic on a hat's `triggers` or `publishes`. If a future
    /// refactor accidentally adds `unit.ready` / `unit.done` /
    /// `unit.validated` / `test.passed` / `test.failed` to the
    /// pipeline topology, this test fails before the regression can
    /// reach runtime.
    #[test]
    fn test_pipeline_schema_has_no_runtime_unit_loop_topics() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("en")
            .join("ce-executor-pipeline.yml");
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read pipeline preset at {}: {}", path.display(), e));
        let v: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse pipeline yaml");
        let forbidden: std::collections::BTreeSet<&str> = [
            "unit.ready",
            "unit.done",
            "unit.validated",
            "test.passed",
            "test.failed",
        ]
        .iter()
        .copied()
        .collect();
        let hats = v["hats"].as_mapping().expect("hats must be a mapping");
        let mut all_topics: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (_name, hat) in hats {
            for key in ["triggers", "publishes"] {
                if let Some(seq) = hat[key].as_sequence() {
                    for t in seq {
                        if let Some(s) = t.as_str() {
                            all_topics.insert(s.to_string());
                        }
                    }
                }
            }
        }
        let hits: Vec<&String> = all_topics
            .iter()
            .filter(|t| forbidden.contains(t.as_str()))
            .collect();
        assert!(
            hits.is_empty(),
            "pipeline must not reference runtime unit-loop topics on any hat's \
             triggers/publishes. Found {hits:?}. If this regresses, the \
             single-chain execution invariant is broken."
        );
    }

    /// Cross-check: pipeline also must not have a `mechanism.flow`
    /// block, since single-chain execution is hat-only. This pins the
    /// plan's claim that pipeline is a "flat serial hat chain".
    #[test]
    fn test_pipeline_has_no_mechanism_flow_block() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("presets")
            .join("en")
            .join("ce-executor-pipeline.yml");
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read pipeline preset at {}: {}", path.display(), e));
        let v: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse pipeline yaml");
        assert!(
            v.get("mechanism").is_none(),
            "pipeline must NOT define a top-level `mechanism:` block; \
             single-chain execution is hat-only. Found: {:?}",
            v.get("mechanism")
        );
    }

    fn assert_public_preset_has_required_events(preset: &EmbeddedPreset) {
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
        // 2026-07-24-003 plan KTD14: implementation-review is a
        // branching finalizer-only completion preset. Mutually
        // exclusive success/blocked handoffs must NOT be listed in
        // required_events (that caused the historic review.passed +
        // review.complete infinite-loop class of bugs). Empty is the
        // correct contract: finalizer is the sole LOOP_COMPLETE
        // publisher and ownership/publishes gates block premature
        // completion from other hats.
        if preset.name == "implementation-review" {
            assert!(
                config.event_loop.required_events.is_empty(),
                "Preset '{}' must keep required_events empty (KTD14 \
                 branching finalizer-only completion); got {:?}",
                preset.name,
                config.event_loop.required_events
            );
            return;
        }
        assert!(
            !config.event_loop.required_events.is_empty(),
            "Preset '{}' should define required_events to block premature completion",
            preset.name
        );
    }

    #[test]
    fn test_list_presets_returns_all() {
        let presets = list_presets();
        // Hard-coded count of 6 was true pre-2026-07-24
        // (ce-executor-supervisor / ce-executor-pipeline / debug /
        // merge-batch / merge-loop / autoresearch). 2026-07-24
        // plan U5 added `implementation-review`; bump to 7.
        // 2026-07-27 added `parallel-forge`; bump to 9.
        // 2026-07-28 added `red-team-attack`; bump to 10.
        assert_eq!(
            presets.len(),
            10,
            "Expected 10 public presets (added red-team-attack 2026-07-28)"
        );
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
        // Structural parity (per plan 2026-07-09-005): parse the
        // embedded YAML into RalphConfig so a future content rewrite
        // cannot silently break the merge-loop contract — only
        // registry presence + description + parseable schema matter.
        // We deliberately do not assert on hat triggers/publishes
        // here; that topology is governed by preset_lint and the
        // BDD scenarios, not by a substring/text lock.
        let _config = RalphConfig::parse_yaml(preset.content)
            .expect("merge-loop embedded YAML must remain parseable as RalphConfig");
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
    /// The replacement is `ce-executor-pipeline` (plan 2026-07-07-006,
    /// Ralph primary CE executor).
    #[test]
    fn test_ce_executor_returns_unknown_after_u7_removal() {
        // F5 / AE7: registry lookup must fail explicitly.
        assert!(
            get_preset("ce-executor").is_none(),
            "U7: legacy 'ce-executor' must NOT be resolvable. \
             R13–R15 require removal of YAML, manifest, public index, \
             registry entry, and shell completion without aliasing to \
             'ce-executor-pipeline'."
        );

        // The replacement entry point must remain resolvable.
        let replacement = get_preset("ce-executor-pipeline")
            .expect("ce-executor-pipeline must remain the only complete CE executor entry point");
        assert_eq!(replacement.name, "ce-executor-pipeline");
        assert!(
            !replacement.content.is_empty(),
            "ce-executor-pipeline must still be embedded with non-empty content"
        );

        // Public listing must drop the legacy name and keep the replacement.
        let public_names = preset_names();
        assert!(
            !public_names.contains(&"ce-executor"),
            "U7: 'ce-executor' must NOT appear in public preset_names()"
        );
        assert!(
            public_names.contains(&"ce-executor-pipeline"),
            "U7: 'ce-executor-pipeline' must remain in public preset_names()"
        );

        // Sibling templates (lite / wave) must be unaffected.
        assert!(
            !public_names.contains(&"ce-executor-lite"),
            "ce-executor-lite is a template, not a builtin — it must NOT be in public_names()"
        );
        // Plan 2026-07-07-006: ce-executor-pipeline is the only public
        // single-chain CE executor entry point.
        assert!(
            public_names.contains(&"ce-executor-pipeline"),
            "ce-executor-pipeline must be a public builtin (plan 2026-07-07-006)"
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
        // 2026-07-24 plan U5: added `implementation-review`, the
        // post-implementation six-dimension wave-review preset.
        // 2026-07-28: added `red-team-attack`.
        assert_eq!(names.len(), 10);
        assert!(names.contains(&"autoresearch"));
        assert!(names.contains(&"ce-executor-pipeline"));
        assert!(names.contains(&"ce-executor-pipeline-loop"));
        assert!(names.contains(&"ce-executor-supervisor"));
        assert!(names.contains(&"debug"));
        assert!(names.contains(&"merge-batch"));
        assert!(names.contains(&"parallel-forge"));
        assert!(names.contains(&"post-merge-converge"));
        assert!(names.contains(&"implementation-review"));
        assert!(names.contains(&"red-team-attack"));
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

        let tester = config.hats.get("tester").expect("tester hat should exist");
        assert_eq!(tester.triggers, vec!["hypothesis.test".to_string()]);
        assert_eq!(
            tester.publishes,
            vec![
                "hypothesis.confirmed".to_string(),
                "hypothesis.rejected".to_string(),
            ]
        );

        let fixer = config.hats.get("fixer").expect("fixer hat should exist");
        assert_eq!(
            fixer.publishes,
            vec!["fix.applied".to_string(), "fix.blocked".to_string()]
        );
        assert_eq!(fixer.default_publishes.as_deref(), Some("fix.blocked"));

        let verifier = config
            .hats
            .get("verifier")
            .expect("verifier hat should exist");
        assert_eq!(
            verifier.publishes,
            vec!["fix.verified".to_string(), "fix.failed".to_string()]
        );
        assert_eq!(verifier.default_publishes.as_deref(), Some("fix.failed"));
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
                    assert!(
                        registry.is_empty(),
                        "Preset '{}': unknown hat 'strategist' should be rejected",
                        preset.name
                    );
                }
                OriginCheck::Rejected { .. } => {} // Expected
            }
        }
    }

    #[test]
    fn test_ce_executor_required_events_is_report_done() {
        // Verify ce-executor uses report.done as completion gate (not mutually exclusive
        // branch events review.passed + review.complete which caused infinite loops)
        let preset = get_preset("ce-executor-pipeline").expect("ce-executor preset should exist");
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
        // U2: executor must NOT default_publishes to `work.done` — that
        // would silently swallow a real failure as success. The pipeline
        // preset deliberately uses `work.failed` as the default publish
        // fallback so the no-event gate fails closed; the contract
        // below pins that asymmetric behavior.
        let preset = get_preset("ce-executor-pipeline").expect("ce-executor preset should exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("ce-executor YAML should parse");
        let executor = config
            .hats
            .get("executor")
            .expect("ce-executor should define executor hat");

        match executor.default_publishes.as_deref() {
            None => { /* explicit emit, fine */ }
            Some("work.failed") => { /* fail-closed default, fine */ }
            Some(other) => panic!(
                "executor default_publishes must be either unset or 'work.failed' \
                 (fail-closed); got {other:?}. Setting it to 'work.done' would \
                 silently swallow real failures as success."
            ),
        }
    }

    #[test]
    fn test_implementation_review_no_success_shaped_default_publishes_and_worker_timeout() {
        // Structured contract (not prompt text): success-shaped
        // default_publishes on fan-out / worker / planner / finalizer
        // recreates the silent-success class (synthetic ready/done/
        // LOOP_COMPLETE without real artifacts). Fail-closed defaults
        // on scope-preparer / review-synthesizer remain allowed.
        let preset =
            get_preset("implementation-review").expect("implementation-review preset should exist");
        let config = RalphConfig::parse_yaml(preset.content)
            .expect("implementation-review YAML should parse");

        for hat_id in [
            "review-dispatcher",
            "review-worker",
            "fix-planner",
            "finalizer",
        ] {
            let hat = config
                .hats
                .get(hat_id)
                .unwrap_or_else(|| panic!("implementation-review should define {hat_id}"));
            assert!(
                hat.default_publishes.is_none(),
                "{hat_id} must not set success-shaped default_publishes; got {:?}",
                hat.default_publishes
            );
        }

        assert_eq!(
            config
                .hats
                .get("scope-preparer")
                .expect("scope-preparer")
                .default_publishes
                .as_deref(),
            Some("scope.blocked"),
            "scope-preparer must keep fail-closed default_publishes"
        );
        assert_eq!(
            config
                .hats
                .get("review-synthesizer")
                .expect("review-synthesizer")
                .default_publishes
                .as_deref(),
            Some("review.blocked"),
            "review-synthesizer must keep fail-closed default_publishes"
        );

        let worker = config.hats.get("review-worker").expect("review-worker");
        assert_eq!(
            worker.timeout,
            Some(900),
            "review-worker per-worker timeout must be 15 minutes (900s)"
        );
        assert_eq!(
            worker.concurrency, 6,
            "review-worker concurrency must stay 6 for six-dimension fan-out"
        );
        // KTD-2 default-wave path opens supervisor.db and applies
        // max_concurrent_workers even when supervisor.enabled is
        // false. Cap < concurrency collapses effective_cap and
        // leaves trailing TUI eggs blank (observed: 6 eggs / 4 live).
        assert!(
            config.event_loop.supervisor.max_concurrent_workers >= worker.concurrency,
            "supervisor.max_concurrent_workers ({}) must be >= review-worker.concurrency ({}); \
             otherwise dispatcher effective_cap = min(hat, bridge) drops trailing slots",
            config.event_loop.supervisor.max_concurrent_workers,
            worker.concurrency
        );
        assert!(
            !config.event_loop.supervisor.enabled,
            "implementation-review stays on default wave (supervisor.enabled=false); \
             full supervisor product mode is ce-executor-supervisor, not this preset"
        );
    }

    /// 2026-07-26-004 plan U9 (R9 / R10 / S5 / S10): implementation-review
    /// adopts the generic mechanism contract — the finalizer is the SOLE
    /// `LOOP_COMPLETE` publisher (`review.wave.failed` is a runtime
    /// coordination topic delivered via trigger, NOT a finalizer publish),
    /// `review-worker` publishes only `review.unit.done`, and the declared
    /// flow branches `review.wave.failed` straight to `finalize` via
    /// `on_any_of` (the transition the U6 flow authority now honors).
    #[test]
    fn test_implementation_review_adopts_generic_mechanism_contract() {
        let preset = get_preset("implementation-review").expect("implementation-review preset");
        let config = RalphConfig::parse_yaml(preset.content).expect("YAML parses");

        // R9 / KTD7: finalizer publishes ONLY LOOP_COMPLETE.
        let finalizer = config.hats.get("finalizer").expect("finalizer");
        assert_eq!(
            finalizer.publishes,
            vec!["LOOP_COMPLETE".to_string()],
            "finalizer must be the sole LOOP_COMPLETE publisher"
        );
        assert!(
            !finalizer
                .publishes
                .iter()
                .any(|t| t == "review.wave.failed"),
            "finalizer must NOT publish the runtime coordination topic review.wave.failed"
        );

        // review-worker publishes only review.unit.done (producer contract).
        let worker = config.hats.get("review-worker").expect("review-worker");
        assert_eq!(worker.publishes, vec!["review.unit.done".to_string()]);

        // R10 / S10: the declared flow branches review.wave.failed → finalize.
        let flow = config
            .mechanism
            .as_ref()
            .and_then(|m| m.flow.as_ref())
            .expect("implementation-review declares mechanism.flow");
        let finalize = flow
            .steps
            .iter()
            .find(|s| s.id == "finalize")
            .expect("finalize step");
        assert!(
            finalize.on_any_of.iter().any(|t| t == "review.wave.failed"),
            "finalize must branch on review.wave.failed (U6 declared transition); got {:?}",
            finalize.on_any_of
        );
        assert!(
            finalize.on_any_of.iter().any(|t| t == "dispatch.blocked"),
            "finalize must branch on dispatch.blocked (dispatcher re-verify fail-close); got {:?}",
            finalize.on_any_of
        );
        let dispatcher = config
            .hats
            .get("review-dispatcher")
            .expect("review-dispatcher");
        assert!(
            dispatcher.publishes.iter().any(|t| t == "dispatch.blocked"),
            "review-dispatcher must publish dispatch.blocked for re-verify fail-close"
        );
        // The preset adopts the U6/U7 flow authority end-to-end: scope.ready
        // advances scope_freeze → review_wave (positional), and a failed wave
        // branches straight to finalize via the declared on_any_of above.
        use ralph_core::event_loop::recover_current_plan_step;
        assert_eq!(
            recover_current_plan_step(&config, &["scope.ready"]),
            "review_wave",
            "scope.ready must advance scope_freeze → review_wave"
        );
        assert_eq!(
            recover_current_plan_step(&config, &["scope.ready", "review.wave.failed"]),
            "finalize",
            "a failed review wave must branch to finalize"
        );
        assert_eq!(
            recover_current_plan_step(&config, &["scope.ready", "dispatch.blocked"]),
            "finalize",
            "dispatch.blocked must branch to finalize"
        );
    }

    /// 2026-07-28-001 plan U3 (R5/S5, R6/S6, R7/S7, R9/S9): the real
    /// embedded `parallel-forge` preset adopts the §3.1 14-step
    /// flow authority end-to-end. Each cross-hat handoff uses the
    /// next step's `on`; multi-source block branches use
    /// `on_any_of` on `report`; exec_wave unit topics and
    /// `work.failed` are non-transitions; `exec.wave.complete` and
    /// `exec.wave.failed` route to distinct branches
    /// Plan 2026-07-29-001 U7: the parallel-forge flow now uses
    /// a `development_loop` step (kind: loop) instead of the
    /// legacy single-shot `exec_wave` / `exec_finalize` /
    /// `exec_failure` triple. The planning handoff chain still
    /// reaches `development_loop` deterministically (R1);
    /// `forge.plan.blocked` still branches to `report` (R2);
    /// and the loop's `transition_emits` (`forge.exec.development.done`,
    /// `work.failed`) advance to `full_verify` / `report` exactly
    /// once the final wave settles.
    #[test]
    fn test_parallel_forge_adopts_declared_14step_flow_authority() {
        use ralph_core::event_loop::recover_current_plan_step;

        let preset = get_preset("parallel-forge").expect("parallel-forge preset");
        let config = RalphConfig::parse_yaml(preset.content).expect("YAML parses");

        // R1: planning handoff steps advance explicitly (not via
        // positional fallback) into the development loop.
        assert_eq!(
            recover_current_plan_step(
                &config,
                &[
                    "forge.plan.inspected",
                    "forge.plan.ready",
                    "forge.concurrency.approved",
                    "forge.worktrees.ready",
                ],
            ),
            "development_loop",
            "R1: full planning handoff chain must reach development_loop"
        );

        // R2: forge.plan.blocked branches to report (not development_loop).
        let flow = config
            .mechanism
            .as_ref()
            .and_then(|m| m.flow.as_ref())
            .expect("parallel-forge declares mechanism.flow");
        let report = flow
            .steps
            .iter()
            .find(|s| s.id == "report")
            .expect("report step");
        assert!(
            report.on_any_of.iter().any(|t| t == "forge.plan.blocked"),
            "report.on_any_of must include forge.plan.blocked (R2); got {:?}",
            report.on_any_of
        );
        assert_eq!(
            recover_current_plan_step(&config, &["forge.plan.inspected", "forge.plan.blocked"],),
            "report",
            "R2: forge.plan.blocked must advance to report"
        );

        // R4 / R5 (U7): the development_loop's `transition_emits`
        // is the single advance path. `forge.exec.development.done`
        // exits the loop into `full_verify`, and `work.failed`
        // exits the loop into `report`.
        let dev_loop = flow
            .steps
            .iter()
            .find(|s| s.id == "development_loop")
            .expect("development_loop step");
        let transition_emits: Vec<String> = dev_loop.transition_emits.clone();
        assert!(
            transition_emits.iter().any(|s| s == "forge.exec.development.done"),
            "development_loop.transition_emits must include forge.exec.development.done (R5); got {:?}",
            transition_emits
        );
        assert!(
            transition_emits.iter().any(|s| s == "work.failed"),
            "development_loop.transition_emits must include work.failed (R7); got {:?}",
            transition_emits
        );

        // R5: full success path: planning chain → development_loop
        // (transition) → full_verify → audit → report → plan_end.
        assert_eq!(
            recover_current_plan_step(
                &config,
                &[
                    "forge.plan.inspected",
                    "forge.plan.ready",
                    "forge.concurrency.approved",
                    "forge.worktrees.ready",
                    "forge.exec.development.done",
                    "forge.full.verified",
                    "forge.audit.done",
                    "forge.report.done",
                ],
            ),
            "plan_end",
            "R5: full success path must reach plan_end"
        );
        assert_eq!(
            recover_current_plan_step(
                &config,
                &[
                    "forge.plan.inspected",
                    "forge.plan.ready",
                    "forge.concurrency.approved",
                    "forge.worktrees.ready",
                    "work.failed",
                    "forge.report.done",
                ],
            ),
            "plan_end",
            "R6: exec_wave.failed → exec_failure → report.done must reach plan_end"
        );

        // R7: plan_end rejects LOOP_COMPLETE as a transition (it's
        // the terminal step; LOOP_COMPLETE is accepted by the gate
        // but does not advance the step).
        let plan_end = flow
            .steps
            .iter()
            .find(|s| s.id == "plan_end")
            .expect("plan_end step");
        assert_eq!(
            plan_end.kind.as_deref(),
            Some("terminal"),
            "plan_end must be kind: terminal"
        );

        // S3 (U7): development_loop's `transition_emits` is the
        // single advance path. Forge-side wave topics are
        // allowed (in-scope) but only `forge.exec.development.done`
        // and `work.failed` advance the step.
        let dev_loop = flow
            .steps
            .iter()
            .find(|s| s.id == "development_loop")
            .expect("development_loop step");
        assert!(
            dev_loop
                .allowed_emits
                .iter()
                .any(|t| t == "forge.wave.settled"),
            "development_loop must allow forge.wave.settled (per-wave terminal)"
        );
        assert!(
            dev_loop
                .transition_emits
                .iter()
                .any(|t| t == "forge.exec.development.done"),
            "development_loop.transition_emits must include forge.exec.development.done (loop exit)"
        );
        assert!(
            !plan_end
                .allowed_emits
                .iter()
                .any(|t| t == "forge.report.done"),
            "plan_end must NOT re-allow forge.report.done (transition is in report.on): got {:?}",
            plan_end.allowed_emits
        );
    }

    /// Plan 2026-07-29-002 U1 / R1: the embedded parallel-forge
    /// schema must declare `task_id` / `task_key` as required fields
    /// on `exec.unit.done`, and the same preset must wire the
    /// projection that closes that task atomically. This is a
    /// structural contract — the agent never calls `task close`.
    #[test]
    fn test_parallel_forge_exec_unit_done_requires_task_identity() {
        let preset = get_preset("parallel-forge").expect("parallel-forge preset");
        let config = RalphConfig::parse_yaml(preset.content).expect("parallel-forge YAML parses");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("parallel-forge declares event_policy");
        let entry = policy
            .schemas
            .get("exec.unit.done")
            .expect("parallel-forge schema must declare exec.unit.done");
        let required: std::collections::BTreeSet<&str> =
            entry.required_fields.iter().map(String::as_str).collect();
        for field in ["task_id", "task_key"] {
            assert!(
                required.contains(field),
                "exec.unit.done.required_fields missing `{field}`; got {required:?}"
            );
        }
    }

    /// Plan 2026-07-29-002 U2 / R2: the embedded parallel-forge
    /// preset must enable `completion_payload_match` on
    /// `forge.report.done` with `report_path` as the compared field.
    /// This is the runtime contract that prevents a mismatched
    /// `LOOP_COMPLETE` from overwriting the terminal report fact.
    #[test]
    fn test_parallel_forge_configures_report_done_path_match() {
        let preset = get_preset("parallel-forge").expect("parallel-forge preset");
        let config = RalphConfig::parse_yaml(preset.content).expect("parallel-forge YAML parses");
        let match_cfg = config
            .event_loop
            .completion_payload_match
            .as_ref()
            .expect("parallel-forge must configure completion_payload_match");
        assert_eq!(match_cfg.topic, "forge.report.done");
        assert_eq!(match_cfg.fields, vec!["report_path"]);
        assert!(match_cfg.validate().is_ok());
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
        let preset = get_preset("ce-executor-pipeline").expect("ce-executor preset should exist");
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
    fn test_ce_executor_pipeline_fix_done_routes_directly_to_alignment() {
        let preset = get_preset("ce-executor-pipeline").expect("linear preset should exist");
        let config = RalphConfig::parse_yaml(preset.content).expect("linear preset should parse");

        let stabilizer = config.hats.get("test-stabilizer").expect("test-stabilizer");
        assert_eq!(stabilizer.triggers, vec!["work.done".to_string()]);

        let alignment = config.hats.get("alignment").expect("alignment");
        assert_eq!(alignment.triggers, vec!["fix.done".to_string()]);
    }

    // plan 2026-07-22-004 U5 (S6 / AE2): the REAL embedded
    // `ce-executor-pipeline` preset declares a `payload_consistency`
    // gate on `fix.done` that rejects the self-contradictory
    // `review_verdict=blocked + fixes_applied=0 + planned_fix_units
    // non-empty + fix_status=applied` shape while accepting the legal
    // `fix_status=blocked + non-empty failure_reason` exit and the
    // empty-plan fast path (`planned_fix_units=[]`). This test loads
    // the genuine embedded (schema-merged) preset — NOT a hand-built
    // config — and drives `validate_event` over hitting vs legal
    // payloads to lock the gate's runtime behaviour.
    #[test]
    fn test_ce_executor_pipeline_fix_done_payload_consistency_gate() {
        use ralph_core::{PolicyDecision, PolicyRuntimeState, ViolationType, validate_event};

        let preset = get_preset("ce-executor-pipeline").expect("linear preset should exist");
        let config = RalphConfig::parse_yaml(preset.content).expect("linear preset should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("pipeline preset must declare event_policy");

        // The gate must actually be enabled on the real preset.
        assert!(
            policy.payload_consistency.enabled,
            "ce-executor-pipeline must enable payload_consistency"
        );

        // A fully schema-valid `fix.done` payload (all required fields
        // present) representing the LEGAL blocked exit: verdict blocked,
        // zero fixes applied, non-empty planned_fix_units, but
        // fix_status=blocked with a non-empty failure_reason. The
        // `fix_status=applied` guard in rule 1 keeps this legal.
        let legal = serde_json::json!({
            "plan_name": "2026-07-12-003-demo-plan",
            "plan_path": "docs/plans/2026-07-12-003-demo-plan.md",
            "plan_contract_version": "ce-unified-plan/v1",
            "normalized_plan_file": ".ralph/review/2026-07-12-003-demo-plan/normalized-plan.md",
            "plan_contract_digest": "sha256:deadbeef",
            "trace_file": ".ralph/review/2026-07-12-003-demo-plan/trace.jsonl",
            "executor_head_sha": "0123456789abcdef0123456789abcdef01234567",
            "resolved_baseline_sha": "fedcba9876543210fedcba9876543210fedcba98",
            "head_sha": "89abcdef0123456789abcdef0123456789abcdef",
            "fix_attempt_commit_sha": "89abcdef0123456789abcdef0123456789abcdef",
            "worktree_status": "clean",
            "fix_plan_file": ".ralph/review/2026-07-12-003-demo-plan/fix-plan.md",
            "review_verdict": "blocked",
            "fixes_applied": 0,
            "fixes_skipped": 0,
            "planned_fix_units": ["U1", "U2"],
            "attempted_fix_units": ["U1", "U2"],
            "completed_fix_units": [],
            "failed_fix_units": ["U1", "U2"],
            "blocked_fix_units": [],
            "skipped_fix_units": [],
            "fix_status": "blocked",
            "failure_reason": "U1 and U2 verification remained red after honest attempts",
            "decisions_file": ".ralph/agent/decisions.md",
            "baseline_verification_status": "green",
            "baseline_verification_file": ".ralph/review/2026-07-12-003-demo-plan/baseline-verification.md",
            "post_verification_status": "red",
            "post_verification_file": ".ralph/review/2026-07-12-003-demo-plan/final-verification.md",
            "verification_delta_file": ".ralph/review/2026-07-12-003-demo-plan/verification-delta.md",
            "baseline_existing_count": 0,
            "new_business_regressions_count": 0,
            "test_compatibility_updates_count": 0,
            "flaky_or_environmental_count": 0,
            "settlement_confidence": 92,
            "settlement_evidence_coverage": 80,
            "settlement_evidence_file": ".ralph/review/2026-07-12-003-demo-plan/fix-settlement-evidence.md"
        });

        // 1. Legal blocked exit (fix_status=blocked + failure_reason)
        //    must be ACCEPTED — the fix_status=applied guard prevents
        //    rule 1 from misfiring on an honest blocked settlement.
        let mut state = PolicyRuntimeState::default();
        let legal_str = legal.to_string();
        let decision = validate_event("fix.done", Some(&legal_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "legal fix_status=blocked + failure_reason exit must be accepted"
        );

        // 2. HITTING payload: flip fix_status to applied (and clear the
        //    failure_reason) while keeping blocked + zero fixes + non-empty
        //    planned_fix_units. Rule 1 must reject with the
        //    `payload_consistency:fix-done-blocked-zero-fixes-applied` gate.
        let mut hitting = legal.clone();
        hitting["fix_status"] = serde_json::json!("applied");
        hitting["failure_reason"] = serde_json::json!("");
        let hitting_str = hitting.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(&hitting_str), policy, &mut state);
        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("hitting fix.done must be rejected, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, context, .. } = &finding.violation_type
        else {
            panic!(
                "hitting fix.done must trip a SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert_eq!(
            gate,
            "payload_consistency:fix-done-blocked-zero-fixes-applied"
        );
        assert!(
            context.contains("fix_status=applied contradicts"),
            "gate context must carry the actionable rule message, got {context}"
        );

        // 3. Empty-plan fast path NON-MISFIRE: planned_fix_units=[] with
        //    fixes_applied=0 + review_verdict=blocked + fix_status=applied
        //    must NOT trip rule 1 (the `planned_fix_units non_empty` guard).
        let mut empty_plan = legal.clone();
        empty_plan["planned_fix_units"] = serde_json::json!([]);
        empty_plan["attempted_fix_units"] = serde_json::json!([]);
        empty_plan["failed_fix_units"] = serde_json::json!([]);
        empty_plan["fix_status"] = serde_json::json!("applied");
        empty_plan["failure_reason"] = serde_json::json!("");
        let empty_plan_str = empty_plan.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(&empty_plan_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "empty-plan fast path (planned_fix_units=[]) must not trip rule 1"
        );

        // 4. Legal partial exit NON-MISFIRE: fix_status=partial with a
        //    non-empty failure_reason and non-empty planned_fix_units must
        //    NOT trip rule 1 (the fix_status=applied guard).
        let mut partial = legal.clone();
        partial["fix_status"] = serde_json::json!("partial");
        partial["fixes_applied"] = serde_json::json!(1);
        partial["completed_fix_units"] = serde_json::json!(["U1"]);
        partial["failed_fix_units"] = serde_json::json!(["U2"]);
        partial["failure_reason"] = serde_json::json!("U2 verification remained red");
        let partial_str = partial.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(&partial_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "legal fix_status=partial exit must not trip rule 1"
        );

        // 5. Rule 2 HITTING: new_business_regressions_count>0 while
        //    post_verification_status=green must be rejected with the
        //    `payload_consistency:fix-done-green-with-regressions` gate.
        let mut green_with_regressions = legal.clone();
        green_with_regressions["fix_status"] = serde_json::json!("applied");
        green_with_regressions["review_verdict"] = serde_json::json!("pass");
        green_with_regressions["fixes_applied"] = serde_json::json!(2);
        green_with_regressions["completed_fix_units"] = serde_json::json!(["U1", "U2"]);
        green_with_regressions["failed_fix_units"] = serde_json::json!([]);
        green_with_regressions["new_business_regressions_count"] = serde_json::json!(1);
        green_with_regressions["post_verification_status"] = serde_json::json!("green");
        let green_str = green_with_regressions.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(&green_str), policy, &mut state);
        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("green-with-regressions fix.done must be rejected, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, .. } = &finding.violation_type else {
            panic!(
                "green-with-regressions fix.done must trip a SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert_eq!(gate, "payload_consistency:fix-done-green-with-regressions");

        // 6. Rule 2 NON-MISFIRE: new_business_regressions_count>0 with an
        //    honest post_verification_status=red must NOT trip rule 2.
        let mut honest_red = green_with_regressions.clone();
        honest_red["fix_status"] = serde_json::json!("partial");
        honest_red["failure_reason"] = serde_json::json!("introduced regression left red");
        honest_red["post_verification_status"] = serde_json::json!("red");
        let honest_red_str = honest_red.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(&honest_red_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "honest red post-verification with regressions must not trip rule 2"
        );
    }

    // plan 2026-07-24-002 U1: the REAL embedded `ce-executor-pipeline`
    // preset declares a `payload_consistency` gate on `work.done` that
    // rejects the self-contradictory `post_verification_status=green +
    // new_business_regressions_count>0` shape (mirroring the existing
    // fix-done-green-with-regressions rule) while accepting the honest
    // red path. This test loads the genuine embedded preset and drives
    // `validate_event` to lock the gate's runtime behaviour.
    #[test]
    fn test_ce_executor_pipeline_work_done_payload_consistency_gate() {
        use ralph_core::{PolicyDecision, PolicyRuntimeState, ViolationType, validate_event};

        let preset = get_preset("ce-executor-pipeline").expect("linear preset should exist");
        let config = RalphConfig::parse_yaml(preset.content).expect("linear preset should parse");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("pipeline preset must declare event_policy");

        assert!(
            policy.payload_consistency.enabled,
            "ce-executor-pipeline must enable payload_consistency"
        );

        // A fully schema-valid `work.done` payload representing an
        // honest partial execution with red verification.
        let legal = serde_json::json!({
            "plan_name": "2026-07-24-001-demo-plan",
            "plan_path": "docs/plans/2026-07-24-001-demo-plan.md",
            "plan_contract_version": "ce-unified-plan/v1",
            "normalized_plan_file": ".ralph/review/2026-07-24-001-demo-plan/normalized-plan.md",
            "plan_contract_digest": "sha256:deadbeef",
            "trace_file": ".ralph/review/2026-07-24-001-demo-plan/trace.jsonl",
            "executor_head_sha": "0123456789abcdef0123456789abcdef01234567",
            "resolved_baseline_sha": "fedcba9876543210fedcba9876543210fedcba98",
            "planned_units": ["U1", "U2", "U3"],
            "completed_units": ["U1", "U3"],
            "attempted_units": ["U1", "U2", "U3"],
            "failed_units": ["U2"],
            "blocked_units": [],
            "skipped_units": [],
            "execution_status": "partial",
            "decisions_file": ".ralph/agent/decisions.md",
            "baseline_verification_status": "green",
            "baseline_verification_file": ".ralph/review/2026-07-24-001-demo-plan/baseline-verification.md",
            "post_verification_status": "red",
            "post_verification_file": ".ralph/review/2026-07-24-001-demo-plan/final-verification.md",
            "verification_delta_file": ".ralph/review/2026-07-24-001-demo-plan/verification-delta.md",
            "baseline_existing_count": 0,
            "new_business_regressions_count": 1,
            "test_compatibility_updates_count": 0,
            "flaky_or_environmental_count": 0,
            "tests_run": 120,
            "tests_passed": 119,
            "changed_lines": 85,
            "commit_count": 2
        });

        // 1. Honest red + regressions must be ACCEPTED.
        let mut state = PolicyRuntimeState::default();
        let legal_str = legal.to_string();
        let decision = validate_event("work.done", Some(&legal_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "honest red work.done with regressions must be accepted"
        );

        // 2. HITTING: flip post_verification_status to green while
        //    keeping regressions>0. Must be rejected with the
        //    `payload_consistency:work-done-green-with-regressions` gate.
        let mut hitting = legal.clone();
        hitting["post_verification_status"] = serde_json::json!("green");
        let hitting_str = hitting.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", Some(&hitting_str), policy, &mut state);
        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("green-with-regressions work.done must be rejected, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, context, .. } = &finding.violation_type
        else {
            panic!(
                "green-with-regressions work.done must trip a SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert_eq!(gate, "payload_consistency:work-done-green-with-regressions");
        assert!(
            context.contains("post_verification_status=green contradicts"),
            "gate context must carry the actionable rule message, got {context}"
        );

        // 3. NON-MISFIRE: green with zero regressions must be accepted.
        let mut clean_green = legal.clone();
        clean_green["post_verification_status"] = serde_json::json!("green");
        clean_green["new_business_regressions_count"] = serde_json::json!(0);
        clean_green["tests_passed"] = serde_json::json!(120);
        let clean_str = clean_green.to_string();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", Some(&clean_str), policy, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "green work.done with zero regressions must not trip the gate"
        );
    }

    // 2026-07-16-002 plan U2: structured guard that the enhanced
    // `test-stabilizer` hat in each CE executor preset still owns
    // *only* the stabilization.* terminal topics and stays the sole
    // gate between `work.done` and the review chain. This test pins
    // the topic-level authority boundary that the BDD scenarios rely
    // on so a future instructions edit cannot silently widen the hat's
    // publish set.
    #[test]
    fn test_ce_executor_test_stabilizer_terminal_authority() {
        for name in ["ce-executor-pipeline", "ce-executor-pipeline-loop"] {
            let preset = get_preset(name).unwrap_or_else(|| panic!("preset {name} embedded"));
            let config = RalphConfig::parse_yaml(preset.content)
                .unwrap_or_else(|e| panic!("preset {name} parse: {e}"));
            let stabilizer = config
                .hats
                .get("test-stabilizer")
                .unwrap_or_else(|| panic!("{name} must declare test-stabilizer"));

            assert_eq!(
                stabilizer.triggers,
                vec!["work.done".to_string()],
                "{name}: test-stabilizer trigger must remain work.done"
            );
            let mut publishes: Vec<String> = stabilizer.publishes.clone();
            publishes.sort();
            assert_eq!(
                publishes,
                vec![
                    "stabilization.blocked".to_string(),
                    "stabilization.done".to_string()
                ],
                "{name}: test-stabilizer must publish only stabilization.done/blocked"
            );
            let mut terminals: Vec<String> = stabilizer.terminal_events.clone();
            terminals.sort();
            assert_eq!(
                terminals, publishes,
                "{name}: terminal_events must mirror the only allowed stabilization.* topics"
            );
        }
    }

    /// 2026-07-09-001 plan (U8): the embedded `ce-executor-pipeline-loop`
    /// preset must declare agent-facing `field_docs` / `examples`
    /// metadata for the review/fix convergence topics so that the
    /// U3 enrichment layer (policy-check errors) and the U6
    /// schema-aware prompt section can consume them. Plan
    /// 2026-07-09-005 replaced the previous SSOT byte-equality
    /// assertion with a structured schema assertion that survives
    /// legitimate prompt-wording or comment edits in the YAML.
    #[test]
    fn test_ce_executor_pipeline_loop_embedded_includes_u8_field_docs() {
        let preset = get_preset("ce-executor-pipeline-loop")
            .expect("ce-executor-pipeline-loop preset should exist");
        let value: serde_yaml::Value =
            serde_yaml::from_str(preset.content).expect("embedded preset must be valid YAML");
        let schemas = value
            .get("event_loop")
            .and_then(|v| v.get("event_policy"))
            .and_then(|v| v.get("schemas"))
            .expect("ce-executor-pipeline-loop must carry event_policy.schemas");
        for topic in [
            "review.synthesized",
            "review.accepted",
            "fix.requested",
            "review.complete",
            "review.loop.blocked",
        ] {
            let schema = schemas.get(topic).unwrap_or_else(|| {
                panic!("U8 pilot topic `{topic}` must appear in embedded schemas")
            });
            let field_docs = schema
                .get("field_docs")
                .unwrap_or_else(|| panic!("U8 pilot topic `{topic}` must declare field_docs"));
            assert!(
                field_docs.is_mapping(),
                "U8 pilot topic `{topic}` field_docs must be a mapping"
            );
            assert!(
                !field_docs.as_mapping().unwrap().is_empty(),
                "U8 pilot topic `{topic}` field_docs must not be empty"
            );
        }
    }

    /// 2026-07-07-002 plan Unit 9: generic data skill docs must document correction/bounded retry.
    #[test]
    fn test_data_skill_docs_correction_guidance_unit9() {
        // CARGO_MANIFEST_DIR points to `crates/ralph-cli`; the data
        // docs live under `crates/ralph-core/data/` which is two
        // levels up.  Going only one level up (`..` only) would
        // land at `crates/` and produce a bogus
        // `crates/crates/...` path that fails file-existence
        // checks (DEV-007: pre-fix this test panicked with
        // NotFound because the join landed outside the repo).
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let emit = std::fs::read_to_string(
            manifest_dir.join("crates/ralph-core/data/ralph-tools-emit.md"),
        )
        .expect("read ralph-tools-emit.md");
        assert!(
            emit.contains("协议违规后的 EmitResult"),
            "ralph-tools-emit must document protocol violation EmitResult/correction response"
        );
        assert!(
            emit.contains("protocol_violation_repeated"),
            "ralph-tools-emit must mention bounded fail-close reason"
        );

        let recovery = std::fs::read_to_string(
            manifest_dir.join("crates/ralph-core/data/ralph-tools-recovery-directives.md"),
        )
        .expect("read ralph-tools-recovery-directives.md");
        assert!(
            recovery.contains("Correction 优先级"),
            "ralph-tools-recovery-directives must document correction priority"
        );
        assert!(
            recovery.contains("forbidden_action"),
            "ralph-tools-recovery-directives must document forbidden_action semantics"
        );

        let main =
            std::fs::read_to_string(manifest_dir.join("crates/ralph-core/data/ralph-tools.md"))
                .expect("read ralph-tools.md");
        assert!(
            main.contains("ralph-tools-recovery-directives"),
            "ralph-tools.md must point agents to recovery-directives on task.resume"
        );

        let precheck = std::fs::read_to_string(
            manifest_dir.join("crates/ralph-core/data/ralph-tools-precheck.md"),
        )
        .expect("read ralph-tools-precheck.md");
        assert!(
            precheck.contains("protocol correction"),
            "ralph-tools-precheck must mention correction-then-policy-check discipline"
        );

        let tasks = std::fs::read_to_string(
            manifest_dir.join("crates/ralph-core/data/ralph-tools-tasks.md"),
        )
        .expect("read ralph-tools-tasks.md");
        assert!(
            tasks.contains("live identity") || tasks.contains("live record"),
            "ralph-tools-tasks must document live task identity"
        );
        assert!(
            tasks.contains("close-before"),
            "ralph-tools-tasks must document close-before-done"
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
            "builtin:ce-executor-pipeline",
            "builtin:ce-executor-pipeline-loop",
            "builtin:ce-executor-supervisor",
            "builtin:debug",
            "builtin:autoresearch",
            "builtin:merge-batch",
            "builtin:parallel-forge",
            "builtin:post-merge-converge",
            // 2026-07-24-003 plan: post-implementation six-dim
            // wave-review preset; mirrored in
            // scripts/ralph-zsh-plugin.zsh _RALPH_BUILTIN_HAT_VALUES.
            "builtin:implementation-review",
            "builtin:red-team-attack",
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
                } else {
                    current.push(c as char);
                    idx += 1;
                }
            } else {
                if c == b'"' {
                    in_quote = true;
                    had_quote = true;
                    idx += 1;
                } else if c == b')' {
                    return Some(out);
                } else {
                    idx += 1;
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
        let preset = get_preset("ce-executor-pipeline").expect("ce-executor preset should exist");
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
        ];

        // Per-preset finding-id exemptions for the non-strict authoring
        // contract test. Mirrors `EXEMPT_FINDINGS` below (the strict-
        // lint counterpart) but is its own const so the two tests can
        // diverge if needed.
        //
        // Currently empty: `ce-executor-pipeline` and
        // `ce-executor-pipeline-loop` used to trip three WAC findings
        // each on the chain head whose root cause was the static-lint
        // BFS bound (EGRESS_MAX_HOPS). The 2026-07-08 bump of
        // EGRESS_MAX_HOPS from 10 to 12 let both presets pass strict
        // with zero findings, so these exemptions are no longer
        // needed.
        const AUTHORING_EXEMPT_FINDINGS: &[(&str, &str)] = &[];

        for preset in PRESETS.iter().filter(|p| p.public) {
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_runtime_config(&config);
            let strictness = RuntimeContractStrictness::default(); // non-strict
            let report = RuntimeContractAggregator::aggregate(
                format!("builtin:{}", preset.name),
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
        let strict_presets = &["ce-executor-pipeline"];
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
                format!("builtin:{}", preset.name),
                &config,
                &registry,
                strictness,
                Some(preset.content),
            );
            // U10: dev presets may have known topology exceptions (e.g. a hat can
            // emit a terminal topic directly from `work.start`). This test
            // is focused on strict payload contract, so topology-only
            // findings are recorded rather than failing.
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
        // `ce-executor-supervisor` is NOT exempt: `required_events` holds only
        // the all-path convergence topic (`LOOP_COMPLETE`), and success-spine
        // `work.done` is gated via `path_required_events` on `plan.complete`.
        let topology_exempt: &[&str] = &["autoresearch", "debug"];

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
        // `ce-executor-pipeline` and `ce-executor-pipeline-loop` used to
        // be exempt for three WAC findings each
        // (`activation_egress_missing`, `handoff_pairing_broken`,
        // `re_emit_trap`) caused by the static-lint BFS bound
        // (EGRESS_MAX_HOPS). The 2026-07-08 bump of EGRESS_MAX_HOPS
        // from 10 to 12 let both presets pass strict with zero findings,
        // so these exemptions are no longer needed.
        //
        // `ce-executor-supervisor` / `config.empty_terminal_events` was
        // previously exempt for `exec-wave-dispatcher`; that hat now
        // declares `terminal_events: [exec.unit.ready]`.
        const EXEMPT_FINDINGS: &[(&str, &str, &str)] = &[];

        let mut failures = Vec::new();
        for preset in PRESETS {
            let config =
                RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
            let registry = HatRegistry::from_runtime_config(&config);
            let strictness = RuntimeContractStrictness::preset_check_strict();
            let report = RuntimeContractAggregator::aggregate(
                format!("builtin:{}", preset.name),
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

    /// Plan 2026-07-28-001 U2 / R14 / S9: the **newly-activated
    /// builtin projector action keys** (i.e. configured keys that
    /// fall outside the legacy `work.*` / `queue.advance` /
    /// `plan.complete` / `review.dimensions.complete` whitelist)
    /// must be **exactly `{forge.plan.ready}`** across every
    /// embedded builtin. Synthesizing a new builtin that activates
    /// another projected topic without an explicit plan amendment
    /// silently widens the action-key surface, so we pin the set
    /// structurally here (R14 / S9, plan §4.2).
    #[test]
    fn test_builtin_state_projection_action_keys_migration_inventory() {
        use ralph_core::config::StateProjectionAction;
        use std::collections::BTreeSet;

        // Legacy `PROBE`-only keys: inventory from the pre-U2 builtin
        // baseline. These keep the projector live but add no
        // newly-typed action. Anything outside this whitelist is part
        // of the new (approved) set and must end up in the new-key
        // pin.
        let legacy_keys: BTreeSet<String> = [
            "work.ready",
            "work.done",
            "queue.advance",
            "plan.complete",
            "review.dimensions.complete",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let expected_new_keys: BTreeSet<String> =
            ["forge.plan.ready", "forge.wave.settled"]
                .iter()
                .map(|s| s.to_string())
                .collect();

        let mut full_legacy: BTreeSet<String> = legacy_keys.clone();
        let mut full_new: BTreeSet<String> = BTreeSet::new();

        let mut bad_presets: Vec<String> = Vec::new();
        for preset in PRESETS {
            let config = RalphConfig::parse_yaml(preset.content)
                .unwrap_or_else(|e| panic!("{}: parse error: {e}", preset.name));
            if !config.event_loop.state_projection.enabled {
                continue;
            }
            let mut configured: BTreeSet<String> = BTreeSet::new();
            for key in config.event_loop.state_projection.actions.keys() {
                configured.insert(key.clone());
            }
            for (key, chain) in &config.event_loop.state_projection.actions_chain {
                if chain.is_empty() {
                    continue;
                }
                configured.insert(key.clone());
            }
            // The configure-time action kind (ensure_task / ensure
            // / mark_progress / chain) is irrelevant to this audit —
            // what matters is whether the topic key itself is part
            // of the legacy set or the post-2026-07-28 approved set.
            for key in &configured {
                if legacy_keys.contains(key) {
                    full_legacy.insert(key.clone());
                } else {
                    full_new.insert(key.clone());
                }
            }
            // The preset's per-key contribution still has to make
            // sense: every non-legacy configured key on a builtin
            // must be `expected_new_keys`. If a sibling preset adds a
            // new key of its own, surface it as a per-preset failure.
            let preset_extras: BTreeSet<String> = configured
                .iter()
                .filter(|k| !legacy_keys.contains(*k))
                .filter(|k| !expected_new_keys.contains(*k))
                .cloned()
                .collect();
            if !preset_extras.is_empty() {
                bad_presets.push(format!(
                    "{}: {} unexpected new action key(s)",
                    preset.name,
                    preset_extras.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            // Sanity: ensure `forge.plan.ready` is configured as a
            // batch action on the parallel-forge preset — this is the
            // one and only place its action is declared.
            if preset.name == "parallel-forge" {
                let Some(action) = config
                    .event_loop
                    .state_projection
                    .actions
                    .get("forge.plan.ready")
                else {
                    bad_presets.push(format!(
                        "{}: `forge.plan.ready` not declared in state_projection.actions",
                        preset.name
                    ));
                    continue;
                };
                if !matches!(action, StateProjectionAction::EnsureTaskBatch { .. }) {
                    bad_presets.push(format!(
                        "{}: forge.plan.ready is not an EnsureTaskBatch action",
                        preset.name
                    ));
                }
                // Plan 2026-07-29-005 U1: the static wave / order /
                // digest pointers must be declared so the projector
                // activates `validate_wave_schedule` for parallel-forge
                // (R2–R4). Without these three pointers the projector
                // silently takes the legacy DAG-only branch and the
                // static schedule contract is unenforced.
                if let StateProjectionAction::EnsureTaskBatch {
                    execution_wave,
                    integration_order,
                    execution_plan_digest,
                    ..
                } = action
                {
                    let pointers = [
                        ("execution_wave", execution_wave.as_deref()),
                        ("integration_order", integration_order.as_deref()),
                        ("execution_plan_digest", execution_plan_digest.as_deref()),
                    ];
                    for (field, value) in pointers {
                        if value.is_none() {
                            bad_presets.push(format!(
                                "{}: forge.plan.ready EnsureTaskBatch must declare `{}` pointer; \
                                 without it the projector skips validate_wave_schedule (plan 005 U1)",
                                preset.name, field
                            ));
                        }
                    }
                }
                // Plan 2026-07-29-001 U3: `forge.wave.settled`
                // is now the only state-authority path that closes
                // tasks for parallel-forge (slot → wave settlement
                // rather than slot → close_task). The legacy
                // `exec.unit.done → close_task` mapping was removed
                // because fan-out completion must not release
                // downstream Unit dependencies early (R7).
                match config
                    .event_loop
                    .state_projection
                    .actions
                    .get("forge.wave.settled")
                {
                    Some(StateProjectionAction::CloseTaskBatch { task_ids, .. })
                        if task_ids == "settled_task_ids" => {}
                    other => bad_presets.push(format!(
                        "{}: forge.wave.settled must be CloseTaskBatch{{task_ids:\"settled_task_ids\"}}, got {:?}",
                        preset.name, other
                    )),
                }
            }
        }

        assert_eq!(
            full_legacy, legacy_keys,
            "legacy action-key inventory drifted; expected {:?}, got {:?}",
            legacy_keys, full_legacy
        );
        assert_eq!(
            full_new, expected_new_keys,
            "newly-activated action keys drift; expected {:?}, got {:?}; offending presets: {:?}",
            expected_new_keys, full_new, bad_presets
        );
        assert!(
            bad_presets.is_empty(),
            "preset-level key errors: {}",
            bad_presets.join("; ")
        );
    }

    // Plan 2026-07-29-005 U3: the four per-wave hats
    // (reviewer / integrator / verifier / tester) must declare
    // `event_filter.events` that cover the business-entry topics
    // listed in their `triggers`. Without this coverage the runtime
    // event filter silently drops new wave-scoped topics and the
    // hat never activates.
    //
    // The matcher is structural: every `triggers` topic must
    // appear in `event_filter.events`. Topics still present in
    // `publishes` for backward-compat aliases do not have to be
    // mirrored in the filter.
    #[test]
    fn test_parallel_forge_event_filter_covers_triggers() {
        let preset = get_preset("parallel-forge").expect("parallel-forge preset must exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("parallel-forge YAML should parse");
        let registry = HatRegistry::from_config(&config);
        use ralph_proto::HatId;
        let target_hats = ["reviewer", "integrator", "verifier", "tester"];
        let mut problems: Vec<String> = Vec::new();
        for hat_id in target_hats {
            let hat_id_typed = HatId::new(hat_id);
            let Some(hat_cfg) = registry.get_config(&hat_id_typed) else {
                problems.push(format!("hat `{hat_id}` missing from parallel-forge registry"));
                continue;
            };
            let filter_events: std::collections::BTreeSet<&str> = match hat_cfg.event_filter.as_ref() {
                Some(f) => f.events.iter().map(String::as_str).collect(),
                None => std::collections::BTreeSet::new(),
            };
            for trigger in &hat_cfg.triggers {
                if !filter_events.contains(trigger.as_str()) {
                    problems.push(format!(
                        "hat `{hat_id}` trigger `{trigger}` not covered by event_filter.events {:?}",
                        filter_events
                    ));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "parallel-forge event_filter does not cover triggers: {}",
            problems.join("; ")
        );
    }

    // Plan 2026-07-29-005 U4 / G8: the forge-failure-handler hat
    // instructions must use a single consecutive step-number
    // sequence. Before U4 the "Final correction (3 rounds
    // exhausted)" sub-section reused the same `4.` / `5.` labels
    // as the main correction flow, making agent navigation
    // ambiguous (the same step number mapped to two distinct
    // actions). The structural assertion below scans for any
    // numbered list inside the hat instructions and rejects
    // duplicate step numbers across the document.
    #[test]
    fn test_parallel_forge_failure_handler_step_numbering_is_consecutive() {
        let preset = get_preset("parallel-forge").expect("parallel-forge preset must exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("parallel-forge YAML should parse");
        let registry = HatRegistry::from_config(&config);
        use ralph_proto::HatId;
        let hat_id_typed = HatId::new("forge-failure-handler");
        let hat_cfg = registry
            .get_config(&hat_id_typed)
            .expect("forge-failure-handler must exist");
        let body = &hat_cfg.instructions;

        // Collect every "^\s*N\.\s" markdown numbered step.
        // We do not assert the text of any step (avoid literal
        // locking); only the numbering invariant.
        let mut seen: Vec<u32> = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim_start();
            let digits: String = trimmed
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.is_empty() {
                continue;
            }
            // require "<digits>. " (a period + whitespace) so we
            // don't pick up arbitrary inline numbers.
            let rest = &trimmed[digits.len()..];
            if !rest.starts_with(". ") {
                continue;
            }
            if let Ok(n) = digits.parse::<u32>() {
                seen.push(n);
            }
        }
        // Find any number that appears more than once.
        let mut counts: std::collections::BTreeMap<u32, u32> =
            std::collections::BTreeMap::new();
        for n in &seen {
            *counts.entry(*n).or_insert(0) += 1;
        }
        let duplicates: Vec<u32> = counts
            .iter()
            .filter_map(|(n, c)| if *c > 1 { Some(*n) } else { None })
            .collect();
        assert!(
            duplicates.is_empty(),
            "forge-failure-handler instructions reuse step numbers {duplicates:?}; \
             plan 005 U4/G8 requires a single consecutive sequence"
        );
    }

    // Plan 2026-07-29-005 U2 (G3): the planner hat instructions
    // must be non-empty and the embedded `forge.plan.ready`
    // schema must declare `execution_plan_digest` and
    // `wave_total` as required fields. We do NOT lock the prose
    // (HARD RULE against instructions-text tests); the
    // behavioural gate is enforced by the schema SSOT below.
    #[test]
    fn test_parallel_forge_planner_instructions_nonempty_and_schema_wave_fields() {
        let preset = get_preset("parallel-forge").expect("parallel-forge preset must exist");
        let config =
            RalphConfig::parse_yaml(preset.content).expect("parallel-forge YAML should parse");
        let registry = HatRegistry::from_config(&config);
        use ralph_proto::HatId;
        let planner_id = HatId::new("planner");
        let planner_cfg = registry
            .get_config(&planner_id)
            .expect("planner hat must exist");
        assert!(
            !planner_cfg.instructions.trim().is_empty(),
            "planner hat instructions must be non-empty (plan 005 U2)"
        );

        // Behavioural gate: forge.plan.ready schema must require
        // execution_plan_digest and wave_total (G2 / G3); without
        // these, runtime validate_wave_schedule cannot activate.
        let event_policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("event_policy must be declared for parallel-forge");
        let forge_plan_ready = event_policy
            .schemas
            .get("forge.plan.ready")
            .expect("forge.plan.ready schema must exist");
        for required in ["execution_plan_digest", "wave_total"] {
            assert!(
                forge_plan_ready.required_fields.iter().any(|f| f == required),
                "forge.plan.ready schema must require `{required}` (plan 005 U2/G2/G3)"
            );
        }
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
                format!("builtin:{}", preset.name),
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

    fn collect_required_field_docs(
        preset: &RalphConfig,
        topic: &str,
    ) -> (Vec<String>, Vec<String>) {
        // Returns (required_fields, fields_missing_full_docs).
        let Some(entry) = preset
            .event_loop
            .event_policy
            .as_ref()
            .and_then(|policy| policy.schemas.get(topic))
        else {
            return (Vec::new(), Vec::new());
        };
        let required = entry.required_fields.clone();
        let mut missing = Vec::new();
        for field in &required {
            let has_full = entry
                .field_docs
                .get(field)
                .map(|fd| {
                    !fd.meaning.trim().is_empty()
                        && !fd.source.trim().is_empty()
                        && !fd.fill_rule.trim().is_empty()
                })
                .unwrap_or(false);
            if !has_full {
                missing.push(field.clone());
            }
        }
        (required, missing)
    }

    #[test]
    fn test_ce_reporter_inputs_are_required_and_documented() {
        for (name, topics) in [
            (
                "ce-executor-pipeline",
                &[
                    "plan.blocked",
                    "work.failed",
                    "stabilization.blocked",
                    "review.artifact.blocked",
                    "align.done",
                ][..],
            ),
            (
                "ce-executor-pipeline-loop",
                &[
                    "plan.blocked",
                    "work.failed",
                    "stabilization.blocked",
                    "review.artifact.blocked",
                    "review.loop.blocked",
                    "align.done",
                ][..],
            ),
        ] {
            let preset_content = get_preset(name)
                .unwrap_or_else(|| panic!("preset {name} embedded"))
                .content;
            let config = RalphConfig::parse_yaml(preset_content)
                .unwrap_or_else(|e| panic!("preset {name} parse: {e}"));
            let policy = config
                .event_loop
                .event_policy
                .as_ref()
                .expect("event policy");
            for topic in topics {
                let schema = policy
                    .schemas
                    .get(*topic)
                    .unwrap_or_else(|| panic!("{name}: missing schema for {topic}"));
                assert!(
                    schema
                        .required_fields
                        .iter()
                        .any(|field| field == "report_input_file"),
                    "{name}: {topic} must require report_input_file"
                );
                let docs = schema
                    .field_docs
                    .get("report_input_file")
                    .unwrap_or_else(|| panic!("{name}: {topic} must document report_input_file"));
                assert!(
                    !docs.meaning.trim().is_empty()
                        && !docs.source.trim().is_empty()
                        && !docs.fill_rule.trim().is_empty(),
                    "{name}: {topic}.report_input_file needs meaning/source/fill_rule"
                );
            }
        }
    }

    #[test]
    fn test_ce_builtin_stabilization_schemas_declared_inline() {
        // 原始 loop preset 必须 inline 声明 stabilization.done / blocked，
        // 以保证 path-based authoring view 也能独立通过 strict check。
        let loop_content = get_preset("ce-executor-pipeline-loop")
            .expect("ce-executor-pipeline-loop embedded")
            .content;
        let loop_config = RalphConfig::parse_yaml(loop_content).expect("loop preset parse");

        for topic in ["stabilization.done", "stabilization.blocked"] {
            let (required, missing) = collect_required_field_docs(&loop_config, topic);
            assert!(
                !required.is_empty(),
                "loop preset must declare inline schema for '{topic}'"
            );
            assert!(
                missing.is_empty(),
                "loop preset '{topic}' is missing full field_docs for: {missing:?}"
            );
        }

        // Linear preset: the inline authoring view must also declare
        // stabilization.* inline schemas (it inherits the same gate
        // because business_topics carries them in both presets).
        let linear_content = get_preset("ce-executor-pipeline")
            .expect("ce-executor-pipeline embedded")
            .content;
        let linear_config = RalphConfig::parse_yaml(linear_content).expect("linear preset parse");
        for topic in ["stabilization.done", "stabilization.blocked"] {
            let (required, missing) = collect_required_field_docs(&linear_config, topic);
            assert!(
                !required.is_empty(),
                "linear preset must declare inline schema for '{topic}'"
            );
            assert!(
                missing.is_empty(),
                "linear preset '{topic}' is missing full field_docs for: {missing:?}"
            );
        }
    }

    #[test]
    fn test_ce_builtin_loop_topic_handoff_owners_pinned() {
        // Review-only inventory: every business topic that flows into
        // reporter must be owned by a single emitter hat. Loop-only
        // review.*.done topics are owned by the corresponding dim hat;
        // convergence topics (review.synthesized / review.accepted /
        // fix.requested / review.loop.blocked) by their declared emitter.
        // If a future preset edit breaks this, the test fires so the
        // reporter-only single-consumer guarantee stays intact.
        let topics_owners = [
            ("review.goalalign.done", "dim:goal-alignment"),
            ("review.correctness.done", "dim:correctness"),
            ("review.testing.done", "dim:testing"),
            ("review.maintainability.done", "dim:maintainability"),
            ("review.standards.done", "dim:project-standards"),
            ("review.adversarial.done", "dim:adversarial"),
            ("stabilization.done", "test-stabilizer"),
            ("stabilization.blocked", "test-stabilizer"),
        ];

        for preset_name in ["ce-executor-pipeline", "ce-executor-pipeline-loop"] {
            let preset_content = get_preset(preset_name)
                .unwrap_or_else(|| panic!("preset {preset_name} embedded"))
                .content;
            let config = RalphConfig::parse_yaml(preset_content)
                .unwrap_or_else(|e| panic!("preset {preset_name} parse: {e}"));

            for (topic, owner) in topics_owners {
                let publishes = config
                    .hats
                    .get(owner)
                    .unwrap_or_else(|| panic!("{preset_name} hat '{owner}' declared"))
                    .publishes
                    .clone();
                assert!(
                    publishes.iter().any(|t| t == topic),
                    "{preset_name}: topic '{topic}' must be in hat '{owner}' publishes; got {publishes:?}"
                );
            }
        }
    }
}
