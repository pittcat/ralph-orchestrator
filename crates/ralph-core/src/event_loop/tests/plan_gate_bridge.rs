//! 2026-06-23-005 U5 (R13+R17): plan-gate ↔ executor `work.ready`
//! bridging — link integrity test.
//!
//! Verifies the static wiring of `work.ready` in
//! `presets/en/ce-executor-serial.yml`:
//! - `coordinator` hat (`work.start` trigger) is the only hat allowed
//!   to publish `work.ready` on initial loop entry.
//! - `plan-gate` hat (`queue.advance` trigger) is the only hat allowed
//!   to publish `work.ready` on step advance.
//! - `executor` hat subscribes to `work.ready` (consumer).
//! - No third hat publishes `work.ready` (R17 anti-double-publish).
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U5 / R13 / R17 / AE-6.

use std::path::PathBuf;

/// Locate `presets/en/ce-executor-serial.yml` from the workspace root
/// (crate is rooted at `crates/ralph-core`, so walk up two levels).
fn serial_preset_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("presets/en/ce-executor-serial.yml")
}

/// Read the serial preset as a flat string for substring assertions.
/// We intentionally avoid pulling in a YAML crate to keep the test
/// dependency-free; the YAML structure is stable (single document,
/// consistent indentation).
fn read_serial_preset() -> String {
    std::fs::read_to_string(serial_preset_path()).expect("ce-executor-serial.yml must exist")
}

#[test]
fn plan_gate_publishes_work_ready_for_step_advance() {
    let yaml = read_serial_preset();
    // The plan-gate `publishes:` line must include `work.ready`. This
    // is the U5 P0-3 fix — before the fix, only `queue.advance /
    // plan.complete / plan.blocked` were in the list, leaving executor
    // waiting for a `work.ready` that never came after a step advance.
    //
    // 2026-06-24 plan U1: plan-gate now also publishes
    // `review.complete` as a mirror after consuming
    // `review.passed`/`review.failed` from review-synthesizer.
    // The list is therefore 5 items.
    assert!(
        yaml.contains(
            "publishes: [\"queue.advance\", \"work.ready\", \"plan.complete\", \"plan.blocked\", \"review.complete\"]"
        ),
        "plan-gate publishes must include work.ready for step advance (U5 P0-3 fix) AND review.complete mirror (2026-06-24 U1)"
    );
}

#[test]
fn executor_subscribes_to_work_ready() {
    let yaml = read_serial_preset();
    // The executor hat must declare `work.ready` in its triggers so it
    // activates after plan-gate emits the step's `work.ready`.
    assert!(
        yaml.contains("triggers: [\"work.ready\""),
        "executor (or its equivalent hat) must trigger on work.ready (U5 consumer wiring)"
    );
}

#[test]
fn no_double_publish_work_ready_outside_coordinator_and_plan_gate() {
    let yaml = read_serial_preset();
    // R17: `work.ready` must be emitted by exactly TWO hats:
    //   1. `coordinator` (initial loop entry, triggered by `work.start`)
    //   2. `plan-gate` (step advance, triggered by `queue.advance`)
    // Any OTHER hat publishing `work.ready` would constitute a
    // double-emit risk and break the typed kind SSOT (U1 typed kind
    // dispatch table expects exactly one emitter per kind).
    //
    // We locate each hat's `publishes:` by walking back from the line
    // until we find `  <hat-name>:` to attribute the publisher.
    let mut publishers: Vec<String> = Vec::new();
    let lines: Vec<&str> = yaml.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("publishes:") {
            continue;
        }
        if !line.contains("work.ready") {
            continue;
        }
        // Walk backwards to find the hat header (`  <hat>:` line).
        let mut hat: Option<String> = None;
        for back in (0..idx).rev() {
            let prev = lines[back];
            if prev.starts_with("hats:") {
                break; // crossed into the next / outer section
            }
            // A hat header is `  <name>:` (2-space indent, no further
            // colon-then-space, e.g. `  coordinator:`).
            let trimmed = prev.trim_start();
            if prev.starts_with("  ")
                && !prev.starts_with("    ")
                && trimmed.ends_with(':')
                && !trimmed.contains(' ')
            {
                hat = Some(trimmed.trim_end_matches(':').to_string());
                break;
            }
        }
        if let Some(hat_name) = hat {
            publishers.push(hat_name);
        }
    }
    // Expected set: {coordinator, plan-gate} (in any order).
    assert_eq!(
        publishers.len(),
        2,
        "exactly TWO hats must publish work.ready (coordinator + plan-gate); found {:?}",
        publishers
    );
    assert!(
        publishers.iter().any(|h| h == "coordinator"),
        "coordinator must be a work.ready publisher; got {:?}",
        publishers
    );
    assert!(
        publishers.iter().any(|h| h == "plan-gate"),
        "plan-gate must be a work.ready publisher; got {:?}",
        publishers
    );
}

#[test]
fn plan_gate_triggers_include_queue_advance_for_step_advance_loop() {
    let yaml = read_serial_preset();
    // R13: the plan-gate → executor bridge starts when plan-gate
    // receives `queue.advance`. If plan-gate's `triggers:` line omits
    // `queue.advance`, the loop never advances.
    assert!(
        yaml.contains("triggers: [\"review.passed\", \"review.complete\", \"work.failed\", \"loop.cancel\", \"queue.advance\", \"fix.exhausted\", \"debug.exhausted\"]"),
        "plan-gate triggers must include queue.advance (U5 R13 bridge start)"
    );
}
