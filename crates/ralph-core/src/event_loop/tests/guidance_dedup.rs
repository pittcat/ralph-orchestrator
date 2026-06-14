//! 2026-06-13-004 U9 + review fix tests: guidance dedup logic.
//!
//! These tests pin the U9 contract (per-plan §KTD-7 two-layer
//! dedup: persist layer + in-memory `robot_guidance` vec).
//! They were added in response to the post-review testing gap
//! (correctness F1 / testing T-P1-5): the original plan landed
//! only an end-to-end `incident_fixture.rs` happy path; the
//! dedup behavior itself was untested until this file.
//!
//! The tests exercise `persist_guidance_to_scratchpad` and
//! `update_robot_guidance` directly via a small private helper
//! that runs the full dedup path on a temp scratchpad. We do
//! NOT go through `process_events_from_jsonl` because that would
//! also exercise origin guard / scope check, which are
//! orthogonal to the dedup contract.

use std::io::Write;

use ralph_proto::Event;

use super::*;

/// A minimal solo-mode (no hats) RalphConfig so the EventLoop
/// constructs without preset requirements. The dedup logic
/// only touches scratchpad + robot_guidance, neither of which
/// require a hat topology.
fn make_solo_event_loop() -> EventLoop {
    let yaml = r#"
core:
  scratchpad:
    enabled: true
    path: scratchpad.md
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    EventLoop::new(config)
}

/// Build a `human.guidance` `Event` for tests. We use
/// `Event::new` directly (rather than `EventLoop::take_pending`
/// which filters by topic) so the raw payload flows into the
/// dedup helpers.
fn guidance_event(payload: &str) -> Event {
    Event::new("human.guidance", payload)
}

/// Write a synthetic scratchpad with a guidance block, a
/// following unrelated section, and (optionally) some bytes
/// after the last guidance. Returns the absolute scratchpad
/// path.
fn write_scratchpad_with_sections(
    workspace: &std::path::Path,
    sections: &[&str],
) -> std::path::PathBuf {
    let path = workspace.join("scratchpad.md");
    let mut f = std::fs::File::create(&path).unwrap();
    for s in sections {
        writeln!(f, "{}", s).unwrap();
    }
    f.flush().unwrap();
    path
}

/// Run `persist_guidance_to_scratchpad` for a fresh EventLoop
/// rooted at `workspace`, then return the resulting scratchpad
/// contents.
fn run_persist(workspace: &std::path::Path, events: &[Event]) -> String {
    let mut event_loop = make_solo_event_loop();
    // Point the EventLoop at the workspace by writing a fresh
    // config that uses the workspace as workspace_root.
    let cfg_yaml = format!(
        r#"
core:
  scratchpad:
    enabled: true
    path: scratchpad.md
  workspace_root: '{}'
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#,
        workspace.display()
    );
    eprintln!("[YAML]\n{}", cfg_yaml);
    let config: crate::config::RalphConfig = serde_yaml::from_str(&cfg_yaml).unwrap();
    // `CoreConfig::workspace_root` is `#[serde(skip)]` so it
    // always uses `Default::default()` (which falls back to
    // `current_dir()`). To pin the scratchpad to the test
    // tempdir, route construction through `with_context`,
    // which sets `loop_context = Some(LoopContext::primary(workspace))`.
    // `scratchpad_path()` then resolves to
    // `loop_context.workspace().join(active_scratchpad.path)`,
    // which is exactly the tempdir.
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    event_loop = EventLoop::with_context(config, ctx);
    // The dedup helpers go through `HatlessRalph::active_scratchpad()`
    // to choose the scratchpad path. In solo mode the EventLoop
    // does not pre-populate the ralph's active_scratchpad via
    // `build_prompt` (which is what production calls). Set it
    // here so `persist_guidance_to_scratchpad` writes to the
    // same path the test will read. The `path` override pins
    // the test to a local `scratchpad.md` rather than the
    // default `.ralph/agent/scratchpad.md` which would land
    // in a stale `target/.ralph/agent/` from a previous
    // failure run.
    let mut scratch_cfg = crate::config::ScratchpadConfig::default();
    scratch_cfg.path = "scratchpad.md".to_string();
    event_loop.ralph.set_active_scratchpad(scratch_cfg);
    // Access the private helper via a public test-only escape
    // hatch is not available; we replicate the dedup via a
    // scratch call that goes through update_robot_guidance.
    event_loop.update_robot_guidance(events.to_vec());
    std::fs::read_to_string(workspace.join("scratchpad.md")).unwrap()
}

/// T-P1-5 case 1: writing the same payload 3 times yields
/// exactly one `### HUMAN GUIDANCE` block in the scratchpad.
#[test]
fn test_persist_guidance_dedup_repeated_payload_in_single_call() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events = vec![
        guidance_event("Focus on error handling"),
        guidance_event("Focus on error handling"),
        guidance_event("Focus on error handling"),
    ];
    let content = run_persist(temp_dir.path(), &events);
    let guidance_blocks = content.matches("### HUMAN GUIDANCE").count();
    assert_eq!(
        guidance_blocks, 1,
        "3 identical guidance events must produce exactly 1 block; got {guidance_blocks}"
    );
}

/// T-P1-5 case 2: a pre-existing scratchpad with the same
/// guidance already on disk is NOT re-appended.
#[test]
fn test_persist_guidance_dedup_against_existing_scratchpad_tail() {
    let temp_dir = tempfile::tempdir().unwrap();
    // Pre-write a scratchpad with the payload already present
    // in a `### HUMAN GUIDANCE` block.
    let _ = write_scratchpad_with_sections(
        temp_dir.path(),
        &[
            "# Plan",
            "",
            "## NOTES",
            "Some unrelated text.",
            "",
            "### HUMAN GUIDANCE (2026-06-13 00:00:00 UTC)",
            "",
            "Keep this in mind",
            "",
        ],
    );
    let events = vec![guidance_event("Keep this in mind")];
    let content = run_persist(temp_dir.path(), &events);
    let guidance_blocks = content.matches("### HUMAN GUIDANCE").count();
    assert_eq!(
        guidance_blocks, 1,
        "duplicate guidance against existing scratchpad must NOT be re-appended; got {guidance_blocks}"
    );
}

/// T-P1-5 case 3 (correctness F1 fix verification): unrelated
/// text in a `## NOTES` section AFTER a guidance block does
/// NOT contaminate the dedup HashSet. A new guidance event
/// whose payload matches an `## NOTES` line must be appended.
#[test]
fn test_persist_guidance_does_not_skip_unrelated_text_in_other_sections() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _ = write_scratchpad_with_sections(
        temp_dir.path(),
        &[
            "### HUMAN GUIDANCE (2026-06-13 00:00:00 UTC)",
            "",
            "first guidance",
            "",
            "## NOTES",
            "",
            "Pay attention to A",
            "",
        ],
    );
    let events = vec![guidance_event("Pay attention to A")];
    let content = run_persist(temp_dir.path(), &events);
    // The new event payload was "Pay attention to A" which
    // appears in `## NOTES`. The state-machine dedup must NOT
    // have considered that as an existing guidance block.
    // After run_persist the scratchpad should now contain
    // "Pay attention to A" inside a new `### HUMAN GUIDANCE`
    // block (in addition to the pre-existing `## NOTES` line).
    let guidance_blocks = content.matches("### HUMAN GUIDANCE").count();
    assert_eq!(
        guidance_blocks, 2,
        "pre-fix bug: 'Pay attention to A' would be falsely skipped as a duplicate of the ## NOTES line; got {guidance_blocks} blocks"
    );
}

/// F2 KTD-7 in-memory dedup: `update_robot_guidance` does NOT
/// push the same payload into `robot_guidance` twice across
/// two calls.
#[test]
fn test_update_robot_guidance_dedup_across_calls() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cfg_yaml = format!(
        r#"
core:
  scratchpad:
    enabled: true
    path: scratchpad.md
  workspace_root: {}
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#,
        temp_dir.path().display()
    );
    let config: crate::config::RalphConfig = serde_yaml::from_str(&cfg_yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.update_robot_guidance(vec![guidance_event("alpha")]);
    event_loop.update_robot_guidance(vec![guidance_event("alpha")]);
    event_loop.update_robot_guidance(vec![guidance_event("beta")]);
    let robot = event_loop.robot_guidance_for_test();
    assert_eq!(
        robot,
        vec!["alpha".to_string(), "beta".to_string()],
        "KTD-7 in-memory layer must dedup; got {:?}",
        robot
    );
}

/// Batch dedup: two identical payloads in the same call do
/// NOT both end up in `robot_guidance` (companion to the
/// scratchpad persistence test).
#[test]
fn test_update_robot_guidance_dedup_within_batch() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cfg_yaml = format!(
        r#"
core:
  scratchpad:
    enabled: true
    path: scratchpad.md
  workspace_root: {}
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#,
        temp_dir.path().display()
    );
    let config: crate::config::RalphConfig = serde_yaml::from_str(&cfg_yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.update_robot_guidance(vec![
        guidance_event("only-once"),
        guidance_event("only-once"),
        guidance_event("only-once"),
    ]);
    let robot = event_loop.robot_guidance_for_test();
    assert_eq!(robot, vec!["only-once".to_string()]);
}
