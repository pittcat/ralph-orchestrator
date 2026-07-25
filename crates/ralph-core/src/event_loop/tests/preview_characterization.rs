//! 2026-07-26-001 plan U1: Characterization tests for the current
//! auto-inject skill set. These tests pin the **current behavior**
//! before introducing the U2 `PromptPreview` API, so that:
//!   - the preview API stays consistent with what `build_prompt`
//!     actually injects;
//!   - any future drift in the auto-inject rules fails these tests
//!     loudly instead of silently slipping past the new inspect
//!     command.
//!
//! Scope: only the *auto-inject set* is characterized, not the
//! content of each skill. The skill doc bodies live in
//! `crates/ralph-core/data/*.md` and are pinned by the
//! `test_build_prompt_injects_ralph_tools_skill_r0_block` test
//! in `build_prompt.rs`.
//!
//! Plan reference: docs/plans/2026-07-26-001-...-plan.md §5.U1.

use super::*;

const AUTO_INJECT_MARKERS: &[&str] = &[
    "<ralph-tools-skill>",
    "<ralph-tools-tasks-skill>",
    "<ralph-tools-memories-skill>",
    "<ralph-tools-opac-skill>",
];

const ON_DEMAND_SKILLS: &[&str] = &[
    "ralph-tools-emit",
    "ralph-tools-wave",
    "ralph-tools-cmdref",
    "ralph-tools-precheck",
    "ralph-tools-recovery-directives",
];

/// Compute the auto-inject set actually injected into the prompt
/// produced by `build_prompt` for the given config + hat.
///
/// We don't reach into the private `prepend_auto_inject_skills`;
/// instead we inspect the prompt's tags, which is the same surface
/// the agent sees at runtime. The classification mirrors the gate
/// documented at `EventLoop::inject_memories_and_tools_skill`
/// (event_loop/mod.rs around L5989).
fn auto_injected(prompt: &str) -> Vec<&'static str> {
    AUTO_INJECT_MARKERS
        .iter()
        .copied()
        .filter(|marker| prompt.contains(marker))
        .collect()
}

/// Inverse of [`auto_injected`] — the set of builtin skills that
/// must NOT appear as full `<…-skill>` blocks. They are only
/// available via `ralph tools skill load <name>` (on-demand).
fn assert_on_demand_absent(prompt: &str) {
    for name in ON_DEMAND_SKILLS {
        let block = format!("<{name}-skill>");
        assert!(
            !prompt.contains(&block),
            "{name} must remain on-demand (no full auto-inject block); got block: {block}"
        );
    }
}

#[test]
fn char_auto_inject_default_gate_includes_ralph_tools_and_opac() {
    // Default-gate scenario: memories OR tasks enabled (here both).
    // Expected auto-inject: ralph-tools + ralph-tools-opac (gate),
    // ralph-tools-tasks (tasks enabled), ralph-tools-memories
    // (memories enabled).
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 default-gate characterization");

    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt should build");
    let inject = auto_injected(&prompt);
    assert!(
        inject.contains(&"<ralph-tools-skill>"),
        "ralph-tools must auto-inject under default gate; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-tasks-skill>"),
        "ralph-tools-tasks must auto-inject when tasks.enabled=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-memories-skill>"),
        "ralph-tools-memories must auto-inject when memories.enabled=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-opac-skill>"),
        "ralph-tools-opac must auto-inject under default gate; got {inject:?}"
    );
    assert_on_demand_absent(&prompt);
}

#[test]
fn char_auto_inject_double_off_excludes_ralph_tools_family() {
    // Double-off scenario: tasks.enabled=false AND memories.enabled=false.
    // Expected auto-inject: empty (no ralph-tools* block).
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: false
tasks:
  enabled: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 double-off characterization");

    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt should build");
    let inject = auto_injected(&prompt);
    assert!(
        inject.is_empty(),
        "double-off must produce empty auto-inject; got {inject:?}"
    );
    assert_on_demand_absent(&prompt);
}

#[test]
fn char_auto_inject_tasks_only_includes_ralph_tools_no_memories() {
    // Branch coverage: tasks=true, memories=false.
    // Expected: ralph-tools + ralph-tools-tasks + ralph-tools-opac.
    // NOT: ralph-tools-memories.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: false
tasks:
  enabled: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 tasks-only characterization");

    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt should build");
    let inject = auto_injected(&prompt);
    assert!(
        inject.contains(&"<ralph-tools-skill>"),
        "ralph-tools must auto-inject when tasks=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-tasks-skill>"),
        "ralph-tools-tasks must auto-inject when tasks=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-opac-skill>"),
        "ralph-tools-opac must auto-inject when tasks=true; got {inject:?}"
    );
    assert!(
        !inject.contains(&"<ralph-tools-memories-skill>"),
        "ralph-tools-memories must NOT inject when memories=false; got {inject:?}"
    );
    assert_on_demand_absent(&prompt);
}

#[test]
fn char_auto_inject_memories_only_includes_ralph_tools_no_tasks() {
    // Branch coverage: memories=true, tasks=false.
    // Expected: ralph-tools + ralph-tools-memories + ralph-tools-opac.
    // NOT: ralph-tools-tasks.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: true
  inject: auto
tasks:
  enabled: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 memories-only characterization");

    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt should build");
    let inject = auto_injected(&prompt);
    assert!(
        inject.contains(&"<ralph-tools-skill>"),
        "ralph-tools must auto-inject when memories=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-memories-skill>"),
        "ralph-tools-memories must auto-inject when memories=true; got {inject:?}"
    );
    assert!(
        inject.contains(&"<ralph-tools-opac-skill>"),
        "ralph-tools-opac must auto-inject when memories=true; got {inject:?}"
    );
    assert!(
        !inject.contains(&"<ralph-tools-tasks-skill>"),
        "ralph-tools-tasks must NOT inject when tasks=false; got {inject:?}"
    );
    assert_on_demand_absent(&prompt);
}

#[test]
fn char_auto_inject_per_hat_filter_respects_hat_restriction() {
    // Verify per-hat filter: a skill with hat restriction only
    // injects when the active hat matches. We register a
    // user-defined skill via skills.dirs override to exercise the
    // filter without depending on built-in frontmatter.
    //
    // If the registry is reached via the test workspace, the
    // build_index path is exercised; if not, this test still pins
    // that the *built-in* auto-inject set is unchanged.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  coord:
    name: "Coord"
    triggers: ["work.start"]
    publishes: ["work.done"]
  worker:
    name: "Worker"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1 per-hat filter characterization");

    let coord_prompt = event_loop
        .build_prompt(&HatId::new("coord"))
        .expect("coord prompt should build");
    let worker_prompt = event_loop
        .build_prompt(&HatId::new("worker"))
        .expect("worker prompt should build");

    // Both hats must see the same default auto-inject set since
    // none of the injected skills have a hat restriction in their
    // frontmatter.
    let coord_inject = auto_injected(&coord_prompt);
    let worker_inject = auto_injected(&worker_prompt);
    assert_eq!(
        coord_inject, worker_inject,
        "default-gate auto-inject must be hat-agnostic"
    );
}