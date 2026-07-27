//! 2026-07-26-001 plan U2: unit tests for the `EventLoop::prompt_preview`
//! structured API.
//!
//! These tests verify:
//!   - `prompt_preview` returns `None` for unknown hats
//!   - the auto-inject set matches what `build_prompt` actually
//!     injects (cross-check vs the U1 characterization markers)
//!   - on-demand skills are exposed for `ralph tools skill load`
//!   - the JSON output is stable (serde round-trip)
//!   - block titles are extracted in order

use super::*;

/// U3: thin alias forwarding to `common::minimal_isolated_config`.
/// Kept as a `pub(crate) fn` so existing call sites in this file
/// (and any future sibling test) do not need to know about the
/// `common` module's import path. The YAML template lives in
/// exactly one place now.
fn minimal_isolated_config(memories: bool, tasks: bool) -> RalphConfig {
    common::minimal_isolated_config(memories, tasks)
}

#[test]
fn preview_unknown_hat_returns_none() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 unknown hat test");

    assert!(
        event_loop
            .prompt_preview(&HatId::new("does-not-exist"))
            .is_none(),
        "unknown hat must return None"
    );
}

#[test]
fn preview_default_gate_each_gated_builtin_appears_once() {
    // 2026-07-26-002 U1 follow-up: after the dedup fix, every
    // gated builtin must appear EXACTLY once in `build_prompt`,
    // not just "at least once". A future maintainer who widens
    // the `inject_custom_auto_skills` skip-list OR re-chains
    // registry_auto into `inject_memories_and_tools_skill` would
    // flip a count from 1 to 2 (or 0). Pin the count.
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 count==1 lock");

    let hat_id = HatId::new("builder");
    let prompt = event_loop
        .build_prompt(&hat_id)
        .expect("prompt should build");

    for name in [
        "ralph-tools",
        "ralph-tools-tasks",
        "ralph-tools-memories",
        "ralph-tools-opac",
    ] {
        let count = prompt.matches(&format!("<{name}-skill>")).count();
        assert_eq!(
            count, 1,
            "<{name}-skill> must appear exactly once in build_prompt; got {count}"
        );
    }
}

#[test]
fn preview_default_gate_auto_inject_matches_build_prompt_markers() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 default-gate preview");

    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview should exist");
    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt should build");

    let auto_names: Vec<&str> = preview
        .auto_inject
        .iter()
        .map(|e| e.name.as_str())
        .collect();

    // Build the expected set from the actual prompt markers so this
    // test stays aligned with what the agent sees, not a stale list.
    let markers = [
        "ralph-tools",
        "ralph-tools-tasks",
        "ralph-tools-memories",
        "ralph-tools-opac",
    ];
    let expected: Vec<&str> = markers
        .iter()
        .copied()
        .filter(|m| prompt.contains(&format!("<{m}-skill>")))
        .collect();

    assert_eq!(
        auto_names, expected,
        "preview.auto_inject must match markers present in live prompt"
    );
    assert!(preview.auto_inject.iter().any(|e| e.name == "ralph-tools"));
    assert!(
        preview
            .auto_inject
            .iter()
            .all(|e| e.source == PromptSkillSource::Gated)
    );
}

#[test]
fn preview_double_off_auto_inject_is_empty() {
    let config = minimal_isolated_config(false, false);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 double-off preview");

    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview should exist");
    assert!(
        preview.auto_inject.is_empty(),
        "double-off must produce empty auto_inject; got {:?}",
        preview.auto_inject
    );
}

/// 2026-07-26-002 plan U10 (R12): when `memories.enabled = false`,
/// `prompt_preview` must drop `ralph-tools-memories` from
/// `auto_inject` AND the live `build_prompt` must omit its
/// marker — they MUST agree (R12 cross-check), and the gated
/// set must drop it just like the registry removes it in the
/// live path (line 1304 of event_loop/mod.rs).
#[test]
fn preview_memories_off_drops_ralph_tools_memories_in_both_paths() {
    let config = minimal_isolated_config(false, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U10 memories-off cross-check");

    let hat_id = HatId::new("builder");

    // Live path: build_prompt must NOT include the memories skill
    // marker when memories are disabled.
    let prompt = event_loop
        .build_prompt(&hat_id)
        .expect("prompt should build");
    assert!(
        !prompt.contains("<ralph-tools-memories-skill>"),
        "live prompt must not include ralph-tools-memories when memories.enabled=false; got prompt with marker"
    );

    // Preview path: auto_inject must NOT list ralph-tools-memories
    // and on_demand must NOT either (same registry state).
    let preview = event_loop
        .prompt_preview(&hat_id)
        .expect("preview should exist");
    let auto_names: Vec<&str> = preview
        .auto_inject
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        !auto_names.contains(&"ralph-tools-memories"),
        "preview.auto_inject must omit ralph-tools-memories when memories.enabled=false; got {auto_names:?}"
    );
    let on_demand_names: Vec<&str> = preview.on_demand.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !on_demand_names.contains(&"ralph-tools-memories"),
        "preview.on_demand must also omit ralph-tools-memories; got {on_demand_names:?}"
    );

    // Cross-check: preview and live must agree on what IS injected.
    for name in auto_names {
        assert!(
            prompt.contains(&format!("<{name}-skill>")),
            "preview auto-inject lists {name} but live prompt is missing its marker"
        );
    }
}

#[test]
fn preview_gates_snapshot_reflects_config() {
    let config = minimal_isolated_config(true, false);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 gates snapshot");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");
    assert!(preview.gates.memories_enabled);
    assert!(!preview.gates.tasks_enabled);
}

#[test]
fn preview_on_demand_includes_emit_wave_cmdref_precheck() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 on-demand list");

    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");
    let on_demand_names: Vec<&str> = preview.on_demand.iter().map(|e| e.name.as_str()).collect();

    for expected in [
        "ralph-tools-emit",
        "ralph-tools-wave",
        "ralph-tools-cmdref",
        "ralph-tools-precheck",
        "ralph-tools-recovery-directives",
    ] {
        assert!(
            on_demand_names.contains(&expected),
            "{expected} must be on-demand; got {on_demand_names:?}"
        );
        assert!(
            on_demand_names
                .iter()
                .all(|n| !preview.auto_inject.iter().any(|a| a.name == *n))
        );
    }
}

#[test]
fn preview_on_demand_is_sorted_for_stable_json() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 on-demand sort order");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");

    let names: Vec<&str> = preview.on_demand.iter().map(|e| e.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "on-demand must be sorted by name");
}

#[test]
fn preview_json_roundtrip() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 JSON roundtrip");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");

    let json = serde_json::to_string(&preview).expect("serialize");
    let back: PromptPreview = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(preview, back);
}

#[test]
fn preview_block_titles_are_non_empty_and_unique() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 block titles");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");
    assert!(
        !preview.block_titles.is_empty(),
        "builder prompt must contain at least one ## section; got empty"
    );
    let mut deduped = preview.block_titles.clone();
    deduped.dedup();
    assert_eq!(
        preview.block_titles.len(),
        deduped.len(),
        "block_titles must not contain duplicates"
    );
}

#[test]
fn preview_source_field_is_gated_for_default_skills() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 source field");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");
    for entry in &preview.auto_inject {
        assert_eq!(
            entry.source,
            PromptSkillSource::Gated,
            "{name} should be Gated (default install)",
            name = entry.name
        );
    }
    for entry in &preview.on_demand {
        assert_eq!(
            entry.source,
            PromptSkillSource::OnDemand,
            "{name} should be OnDemand",
            name = entry.name
        );
    }
}

/// 2026-07-26-001 plan U2: equivalence test — `SkillInjector::plan_auto_inject`
/// must produce the same auto-inject set that `build_prompt` injects into
/// the live prompt. This is the SSOT cross-check.
#[test]
fn plan_auto_inject_matches_build_prompt() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 SSOT equivalence");

    let hat_id = HatId::new("builder");

    // Build the registry as the live path would
    let registry = SkillRegistry::from_config(
        &event_loop.config.skills,
        std::path::Path::new(&event_loop.config.core.workspace_root),
        Some(event_loop.config.cli.backend.as_str()),
    )
    .unwrap_or_else(|_| SkillRegistry::new(Some(event_loop.config.cli.backend.as_str())));

    let (gated, registry_auto, _on_demand) =
        SkillInjector::plan_auto_inject(&event_loop.config, &hat_id, &registry);

    let auto_inject_names: std::collections::HashSet<&str> = gated
        .iter()
        .chain(registry_auto.iter())
        .map(|e| e.name.as_str())
        .collect();

    let live_prompt = event_loop
        .build_prompt(&hat_id)
        .expect("prompt should build");

    // Parse the live prompt's skill markers
    let live_markers: std::collections::HashSet<&str> = [
        "ralph-tools",
        "ralph-tools-tasks",
        "ralph-tools-memories",
        "ralph-tools-opac",
    ]
    .iter()
    .copied()
    .filter(|m| live_prompt.contains(&format!("<{m}-skill>")))
    .collect();

    assert_eq!(
        auto_inject_names, live_markers,
        "SkillInjector::plan_auto_inject must match what build_prompt injects"
    );
}

/// 2026-07-26-002 plan U1 SSOT extension: with a custom
/// registry_auto skill, the live prompt must include it exactly
/// once and `plan_auto_inject` must agree. Cross-check for the
/// single-injection fix — `inject_custom_auto_skills` is the sole
/// owner of registry_auto in the live path.
#[test]
fn plan_auto_inject_matches_build_prompt_with_custom_skill() {
    const UNIQUE_DUP_MARKER: &str = "UNIQUE_DUP_MARKER_2026_07_26_002_SSOT";

    let tmp = tempfile::tempdir().expect("tempdir");
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    let body = format!("ssot body {UNIQUE_DUP_MARKER}");
    std::fs::write(
        skill_dir.join("custom-ssot.md"),
        format!("---\nname: custom-ssot\ndescription: U1 SSOT\n---\n\n{body}\n"),
    )
    .expect("write");

    let mut config = minimal_isolated_config(true, true);
    config.skills.dirs = vec![skill_dir.clone()];
    config.skills.overrides.insert(
        "custom-ssot".to_string(),
        crate::config::SkillOverride {
            enabled: Some(true),
            auto_inject: Some(true),
            ..Default::default()
        },
    );

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 SSOT cross-check");

    let hat_id = HatId::new("builder");
    let registry = SkillRegistry::from_config(
        &event_loop.config.skills,
        std::path::Path::new("."),
        Some(event_loop.config.cli.backend.as_str()),
    )
    .unwrap_or_else(|_| SkillRegistry::new(Some(event_loop.config.cli.backend.as_str())));

    let (gated, registry_auto, _on_demand) =
        SkillInjector::plan_auto_inject(&event_loop.config, &hat_id, &registry);

    let auto_inject_names: std::collections::HashSet<&str> = gated
        .iter()
        .chain(registry_auto.iter())
        .map(|e| e.name.as_str())
        .collect();

    let live_prompt = event_loop
        .build_prompt(&hat_id)
        .expect("prompt should build");

    assert!(
        auto_inject_names.contains("custom-ssot"),
        "plan_auto_inject must list custom-ssot"
    );
    assert!(
        live_prompt.contains("<custom-ssot-skill>"),
        "live prompt must include custom-ssot"
    );
    assert_eq!(
        live_prompt.matches("<custom-ssot-skill>").count(),
        1,
        "live prompt must include custom-ssot exactly once"
    );
    assert_eq!(
        live_prompt.matches(UNIQUE_DUP_MARKER).count(),
        1,
        "live prompt must include custom-ssot body marker exactly once"
    );
}

/// 2026-07-26-001 plan U2: disabled-skills is a hard-off signal —
/// `plan_auto_inject` must return empty sets when skills.enabled=false.
#[test]
fn plan_auto_inject_with_disabled_skills() {
    // U3: use the common reverse-case fixture; YAML template
    // lives in exactly one place. Future maintainers who tweak the
    // global gate must keep this fixture returning empty
    // (skills.enabled = false must short-circuit regardless of
    // memories/tasks flags).
    let config = common::fixture_with_disabled_skills();
    let registry = SkillRegistry::new(Some("claude"));
    let hat_id = HatId::new("builder");

    let (gated, registry_auto, on_demand) =
        SkillInjector::plan_auto_inject(&config, &hat_id, &registry);

    assert!(
        gated.is_empty() && registry_auto.is_empty() && on_demand.is_empty(),
        "disabled skills must produce empty auto_inject; got gated={gated:?}, registry_auto={registry_auto:?}, on_demand={on_demand:?}"
    );
}

/// 2026-07-26-002 plan U1: a custom registry auto_inject skill must
/// appear **exactly once** in `build_prompt` output — never twice.
/// Today the live path chain-injects `registry_auto` inside
/// `inject_memories_and_tools_skill` AND re-injects via
/// `inject_custom_auto_skills`, producing duplicate markers.
///
/// Markers we assert on:
/// - `<custom-dup-skill>` open tag (one per injection)
/// - `UNIQUE_DUP_MARKER` substring inside the body (one per injection)
///
/// When the bug is live both counts == 2; after fix both == 1.
#[test]
fn custom_auto_inject_skill_appears_once() {
    const UNIQUE_DUP_MARKER: &str = "UNIQUE_DUP_MARKER_2026_07_26_002";

    // temp workspace + skills.dirs so the registry sees a real
    // custom skill file; `auto_inject: true` in frontmatter forces
    // the registry auto-inject path so the bug can fire.
    let tmp = tempfile::tempdir().expect("tempdir");
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    let body = format!("custom-dup body {UNIQUE_DUP_MARKER} payload");
    let raw = format!("---\nname: custom-dup\ndescription: U1 dup regression\n---\n\n{body}\n");
    std::fs::write(skill_dir.join("custom-dup.md"), raw).expect("write skill");

    let mut config = minimal_isolated_config(true, true);
    config.core.workspace_root = tmp.path().to_path_buf();
    // EventLoop::with_context_and_diagnostics uses `Path::new(".")`
    // as the skill registry scan root regardless of workspace_root,
    // so the skills dir must be absolute to land inside the
    // tempdir we just created.
    config.skills.dirs = vec![skill_dir.clone()];
    config.skills.overrides.insert(
        "custom-dup".to_string(),
        crate::config::SkillOverride {
            enabled: Some(true),
            auto_inject: Some(true),
            ..Default::default()
        },
    );

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 dup injection regression");

    let hat_id = HatId::new("builder");
    let prompt = event_loop
        .build_prompt(&hat_id)
        .expect("prompt should build");

    let tag_count = prompt.matches("<custom-dup-skill>").count();
    let marker_count = prompt.matches(UNIQUE_DUP_MARKER).count();

    assert_eq!(
        tag_count, 1,
        "<custom-dup-skill> open tag must appear exactly once; got {tag_count}"
    );
    assert_eq!(
        marker_count, 1,
        "custom auto_inject marker must appear exactly once; got {marker_count}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Unit 1 of plan 2026-07-27-002: scenario injection fields.
// ─────────────────────────────────────────────────────────────────────

/// Default PromptPreview (no scenario args) must have `evidence_level == "static"`
/// and all optional fields as None.
#[test]
fn preview_default_evidence_level_is_static() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 evidence-level test");
    let preview = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");

    assert_eq!(preview.evidence_level, "static");
    assert!(preview.trigger_context_injected.is_none());
    assert!(preview.wave_context_injected.is_none());
    assert!(preview.orchestrator_context_injected.is_none());
    assert!(preview.correction_injected.is_none());
    assert!(preview.skill_gates.is_none());
}

/// JSON round-trip of PromptPreview must succeed with new optional fields.
/// The existing `preview_json_roundtrip` already covers the default case;
/// this test constructs a PromptPreview with all optional fields set to
/// verify serde skip/deserialize works correctly.
#[test]
fn preview_json_roundtrip_with_all_fields() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 json roundtrip all fields");
    let base = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");

    let preview = PromptPreview {
        evidence_level: "runtime".to_string(),
        trigger_context_injected: Some(crate::trigger_context::TriggerContextView {
            source_topic: "build.task".to_string(),
            source_hat: Some("worker".to_string()),
            current_hat: "builder".to_string(),
            summary: Vec::new(),
            matched_hints: Vec::new(),
        }),
        wave_context_injected: Some(crate::wave_context::WaveContext {
            wave_id: "wave-1".to_string(),
            wave_total: 3,
            received_count: 2,
            expected_dimensions: vec!["lint".to_string(), "test".to_string()],
            missing_dimensions: vec!["audit".to_string()],
            all_dimensions_received: false,
            aggregate_timeout: false,
        }),
        orchestrator_context_injected: Some(serde_json::json!({
            "task_count": 5,
            "phase": "review"
        })),
        correction_injected: Some(crate::correction::CorrectionContext {
            reason_code: "origin:ralph_control_only".to_string(),
            stage: "origin".to_string(),
            topic: "work.ready".to_string(),
            source_hat: Some("worker".to_string()),
            retry_key: "origin:worker:work.ready:ralph_control_only".to_string(),
            retry_count: 1,
            escalation_threshold: 3,
            needs_escalation: false,
            last_message: "test correction".to_string(),
            expected_payload_template: "{}".to_string(),
            allowed_topics: vec!["work.ready".to_string()],
            required_fields: vec!["task_key".to_string()],
        }),
        skill_gates: Some(SkillGateFlags {
            scratchpad_enabled: true,
        }),
        ..base
    };

    let json = serde_json::to_string(&preview).expect("serialize");
    let back: PromptPreview = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(preview, back);
}

// ─────────────────────────────────────────────────────────────────────
// Unit 2 of plan 2026-07-27-002: candidate_emit field.
// ─────────────────────────────────────────────────────────────────────

/// When `candidate_emit` is provided, it serializes in the JSON output.
#[test]
fn prompt_preview_candidate_emit_field_appears_when_provided() {
    let config = minimal_isolated_config(true, true);
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U2 candidate_emit test");
    let base = event_loop
        .prompt_preview(&HatId::new("builder"))
        .expect("preview");

    // Verify default: candidate_emit is None.
    assert!(base.candidate_emit.is_none());

    // Construct a preview with candidate_emit set.
    let preview = PromptPreview {
        candidate_emit: Some(crate::event_policy::CandidateEmitPreview {
            policy_decision: "accept".to_string(),
            reasons: Vec::new(),
            projection: None,
            next_hat_candidates: crate::event_policy::NextHatCandidates::Unverified,
        }),
        ..base.clone()
    };

    // JSON output must include the candidate_emit field.
    let json = serde_json::to_value(&preview).expect("serialize");
    assert!(
        json.get("candidate_emit").is_some(),
        "JSON must contain candidate_emit when set"
    );
    assert_eq!(
        json["candidate_emit"]["policy_decision"],
        serde_json::json!("accept")
    );

    // Serialize the default preview without candidate_emit and verify
    // the key is absent (skip_serializing_if).
    let json_default = serde_json::to_value(&base).expect("serialize");
    assert!(
        json_default.get("candidate_emit").is_none(),
        "JSON must omit candidate_emit when None"
    );
}
