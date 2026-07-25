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

fn minimal_isolated_config(memories: bool, tasks: bool) -> RalphConfig {
    let yaml = format!(
        r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: {memories}
  inject: auto
tasks:
  enabled: {tasks}
"#
    );
    serde_yaml::from_str(&yaml).unwrap()
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
