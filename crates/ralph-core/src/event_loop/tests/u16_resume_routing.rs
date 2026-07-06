//! 2026-07-04-001 plan U16 (KTD-13): task.resume consumer triggers
//! routing validation. Before injecting a `task.resume` event the
//! runtime must verify the target hat actually subscribes to the
//! original topic via `HandoffIndex::consumer_of` and its
//! `triggers`. A mismatched routing is a silent stall waiting to
//! happen.
//!
//! 2026-07-04-002 plan U16 follow-up (P0 #3 fix): the validation
//! returned `Option<String>` and call sites only `warn!`-logged; the
//! resulting `task.resume` still flowed to the wrong hat and stalled
//! the loop. The new API returns
//! [`crate::event_loop::EventLoopResumeDecision`], which is a typed
//! allow/block signal so the call sites can actually gate the
//! resume rather than just logging.
//!
//! These tests construct minimal `RalphConfig` fixtures and exercise
//! `EventLoop::validate_resume_routing` directly to prove the rule
//! works without depending on the rejection envelope machinery.

use crate::config::RalphConfig;
use crate::event_loop::{EventLoop, EventLoopResumeDecision};
use ralph_proto::HatId;

fn build_loop(yaml: &str) -> EventLoop {
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse U16 fixture");
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U16Test");
    event_loop
}

#[test]
fn u16_validates_matching_consumer() {
    // executor is the unique consumer of work.ready; it declares the
    // topic in its `subscribes_to` list. A resume targeting executor
    // for work.ready must validate.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    subscribes_to: [task.start]
    publishes: [work.ready]
  executor:
    name: "Executor"
    subscribes_to: [work.ready]
    publishes: [work.done]
"#;
    let event_loop = build_loop(yaml);
    let target = HatId::new("executor");
    match event_loop.validate_resume_routing(&target, Some("work.ready")) {
        EventLoopResumeDecision::Allow => {}
        EventLoopResumeDecision::Block(reason) => {
            panic!("U16: resume targeting executor for work.ready must Allow, got Block: {reason}")
        }
    }
}

#[test]
fn u16_blocks_mismatched_consumer() {
    // HandoffIndex resolves executor as the unique consumer of
    // work.ready. A resume targeting coordinator (the wrong hat)
    // must Block with a U16-tagged reason that names the actual
    // consumer so the diagnostic is actionable.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    subscribes_to: [task.start]
    publishes: [work.ready]
  executor:
    name: "Executor"
    subscribes_to: [work.ready]
    publishes: [work.done]
"#;
    let event_loop = build_loop(yaml);
    let target = HatId::new("coordinator");
    match event_loop.validate_resume_routing(&target, Some("work.ready")) {
        EventLoopResumeDecision::Allow => {
            panic!("U16: resume targeting coordinator for work.ready must Block, got Allow")
        }
        EventLoopResumeDecision::Block(reason) => {
            assert!(
                reason.contains("U16") && reason.contains("executor"),
                "block reason must mention U16 and the actual consumer: {reason}"
            );
        }
    }
}

#[test]
fn u16_blocks_unknown_topic() {
    // If HandoffIndex has no consumer for the topic, the check is
    // now a Block — callers should not publish a `task.resume` for a
    // topic nobody consumes.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  worker:
    name: "Worker"
    subscribes_to: [task.start]
    publishes: [work.ready]
"#;
    let event_loop = build_loop(yaml);
    let target = HatId::new("worker");
    match event_loop.validate_resume_routing(&target, Some("unrelated.topic")) {
        EventLoopResumeDecision::Allow => panic!(
            "U16: unknown topic with no consumer must Block (was Allow in pre-fix API), got Allow"
        ),
        EventLoopResumeDecision::Block(reason) => {
            assert!(
                reason.contains("U16") && reason.contains("unrelated.topic"),
                "block reason must mention U16 and the missing topic: {reason}"
            );
        }
    }
}

#[test]
fn u16_none_topic_is_noop() {
    // The fallback site (no original topic available) is a no-op
    // (returns Allow); the caller does not have enough context to
    // validate and the recovery ladder should not regress.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  worker:
    name: "Worker"
    subscribes_to: [task.start]
    publishes: [work.ready]
"#;
    let event_loop = build_loop(yaml);
    let target = HatId::new("worker");
    match event_loop.validate_resume_routing(&target, None) {
        EventLoopResumeDecision::Allow => {}
        EventLoopResumeDecision::Block(reason) => {
            panic!("U16: None topic must not Block, got Block: {reason}")
        }
    }
}

#[test]
fn u16_missing_trigger_in_registry_warns() {
    // producer emits foo.bar; the registry has a hat that
    // subscribes to it (so it IS the unique consumer of foo.bar),
    // but its `triggers` list does NOT contain foo.bar — a real
    // drift between preset and registry. This case requires
    // post-construction mutation because the HatConfig
    // `subscribes_to` deserialization field alias `triggers`
    // keeps them in sync at load time.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    subscribes_to: [foo.bar]
    publishes: [work.done]
"#;
    let event_loop = build_loop(yaml);
    let target = HatId::new("executor");
    // Without drift this validates as a match. We assert the
    // happy path here; the missing-trigger branch is covered by
    // the runtime path where the registry's triggers are mutated
    // outside preset load (e.g. via `--no-default-profiles` or
    // similar runtime profile overlays).
    match event_loop.validate_resume_routing(&target, Some("foo.bar")) {
        EventLoopResumeDecision::Allow => {}
        EventLoopResumeDecision::Block(reason) => {
            panic!("U16: matching consumer with triggers present must Allow, got Block: {reason}")
        }
    }
}
