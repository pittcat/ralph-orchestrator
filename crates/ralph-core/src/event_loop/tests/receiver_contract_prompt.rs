//! Plan 2026-09-14-001 Unit 3: `## RECEIVER CONTRACT` prompt injection tests.
//!
//! The runtime derives a `## RECEIVER CONTRACT` block from the current
//! hat's `publishes` declaration plus the `event_policy.schemas`
//! `required_fields`, and prepends it on the isolated prompt chain
//! (serial hats and wave consumer hats share that chain; wave workers
//! use a separate prompt path and are out of scope).
//!
//! Anchoring note: assertions target the block's *body lines*
//! (`- <topic>: required fields: ...` / `(no schema declared)`), never
//! the `## RECEIVER CONTRACT` heading — the heading name also appears in
//! agent-facing skill docs that are themselves injected into the prompt,
//! so substring-checking the heading would produce false positives.

use ralph_proto::HatId;

use super::*;

/// Build an isolated config from three YAML fragments: the `hats:` block
/// body, extra `event_loop:` keys (e.g. `supervisor:` /
/// `receiver_contract:`), and an optional `event_policy:` block body.
fn contract_config(
    hats_yaml: &str,
    event_loop_extra: &str,
    event_policy_yaml: &str,
) -> RalphConfig {
    let yaml = format!(
        r#"
prompt_file: PROMPT.md
hats:
{hats_yaml}
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  starting_event: "task.start"
{event_loop_extra}
{event_policy_yaml}
"#
    );
    serde_yaml::from_str(&yaml).expect("config parses")
}

/// Build the isolated prompt for `hat` — the same chain the loop runner
/// uses for serial hats and wave consumer hats.
fn build_hat_prompt(config: RalphConfig, hat: &str) -> String {
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("unit test");
    event_loop
        .build_prompt(&HatId::new(hat))
        .expect("isolated build_prompt returns Some")
}

const EXECUTOR_HAT: &str = "  executor:\n    name: \"Executor\"\n    triggers: [\"work.ready\"]\n    publishes: [\"work.done\"]";

const WORK_DONE_POLICY: &str = "  event_policy:\n    enabled: true\n    mode: enforce\n    schemas:\n      work.done:\n        required_fields:\n          - summary\n          - commit";

/// A hat that publishes a schema-declared topic sees the block listing
/// the topic and every required field.
#[test]
fn receiver_contract_block_lists_publishes_requirements() {
    let prompt = build_hat_prompt(
        contract_config(EXECUTOR_HAT, "", WORK_DONE_POLICY),
        "executor",
    );
    assert!(
        prompt.contains("- work.done: required fields: summary, commit"),
        "prompt must list the published topic with its required fields; prompt={prompt}"
    );
}

/// Characterization: a hat with no `publishes` (or no event policy at
/// all) must see zero prompt change — no block, byte-identical prompt.
#[test]
fn receiver_contract_no_publishes_no_block() {
    let no_publishes_hat = "  executor:\n    name: \"Executor\"\n    triggers: [\"work.ready\"]";
    let prompt = build_hat_prompt(
        contract_config(no_publishes_hat, "", WORK_DONE_POLICY),
        "executor",
    );
    assert!(
        !prompt.contains(": required fields:") && !prompt.contains("(no schema declared)"),
        "a hat without publishes must not get a receiver contract block; prompt={prompt}"
    );

    // Byte-identical at the wiring level: the prepend helper returns
    // the prompt untouched for a hat without publishes.
    let mut event_loop = EventLoop::new(contract_config(no_publishes_hat, "", WORK_DONE_POLICY));
    event_loop.initialize("unit test");
    let base = String::from("BASE PROMPT");
    assert_eq!(
        event_loop.prepend_receiver_contract(base.clone(), &HatId::new("executor")),
        base,
        "no publishes → prompt must be returned byte-identical"
    );

    // No event policy at all → still no block.
    let prompt = build_hat_prompt(contract_config(EXECUTOR_HAT, "", ""), "executor");
    assert!(
        !prompt.contains(": required fields:") && !prompt.contains("(no schema declared)"),
        "without an event policy the block must not render; prompt={prompt}"
    );
}

/// A published topic with no matching schema entry is listed by name
/// with the explicit `(no schema declared)` marker.
#[test]
fn receiver_contract_topic_without_schema_listed_by_name() {
    let policy = "  event_policy:\n    enabled: true\n    mode: enforce\n    schemas:\n      other.topic:\n        required_fields:\n          - x";
    let prompt = build_hat_prompt(contract_config(EXECUTOR_HAT, "", policy), "executor");
    assert!(
        prompt.contains("- work.done: (no schema declared)"),
        "a published topic without a schema must be listed by name; prompt={prompt}"
    );
    assert!(
        !prompt.contains("other.topic"),
        "schemas of topics the hat does not publish must not leak; prompt={prompt}"
    );
}

/// Wave consumer hats (supervisor-enabled presets) reach the same
/// isolated prompt chain as serial hats, so they get the same block.
#[test]
fn receiver_contract_wave_consumer_reachable() {
    let consumer_hat = "  review-synthesizer:\n    name: \"Synthesizer\"\n    triggers: [\"review.wave.complete\"]\n    publishes: [\"review.passed\"]";
    let supervisor = "  supervisor:\n    enabled: true";
    let policy = "  event_policy:\n    enabled: true\n    mode: enforce\n    schemas:\n      review.passed:\n        required_fields:\n          - verdict";
    let prompt = build_hat_prompt(
        contract_config(consumer_hat, supervisor, policy),
        "review-synthesizer",
    );
    assert!(
        prompt.contains("- review.passed: required fields: verdict"),
        "wave consumer hats on the isolated chain must get the block; prompt={prompt}"
    );
}

/// `event_loop.receiver_contract.enabled: false` disables the block for
/// every hat, byte-identical prompt.
#[test]
fn receiver_contract_disabled_no_block() {
    let disabled = "  receiver_contract:\n    enabled: false";
    let prompt = build_hat_prompt(
        contract_config(EXECUTOR_HAT, disabled, WORK_DONE_POLICY),
        "executor",
    );
    assert!(
        !prompt.contains(": required fields:") && !prompt.contains("(no schema declared)"),
        "receiver_contract.enabled=false must suppress the block; prompt={prompt}"
    );

    // Byte-identical at the wiring level.
    let mut event_loop = EventLoop::new(contract_config(EXECUTOR_HAT, disabled, WORK_DONE_POLICY));
    event_loop.initialize("unit test");
    let base = String::from("BASE PROMPT");
    assert_eq!(
        event_loop.prepend_receiver_contract(base.clone(), &HatId::new("executor")),
        base,
        "disabled → prompt must be returned byte-identical"
    );
}

// ---------------------------------------------------------------------
// `EventLoop::build_receiver_contract_block` — pure renderer unit tests.
// ---------------------------------------------------------------------

fn hat_config(yaml: &str) -> crate::config::HatConfig {
    serde_yaml::from_str(yaml).expect("hat config parses")
}

fn event_policy(yaml: &str) -> crate::config::EventPolicyConfig {
    serde_yaml::from_str(yaml).expect("event policy parses")
}

#[test]
fn build_receiver_contract_block_no_publishes_returns_none() {
    let hat = hat_config("name: \"Executor\"\n");
    let policy = event_policy("enabled: true\nmode: enforce\n");
    assert_eq!(
        EventLoop::build_receiver_contract_block(&hat, Some(&policy)),
        None,
        "a hat without publishes must not render a block"
    );
}

#[test]
fn build_receiver_contract_block_no_event_policy_returns_none() {
    let hat = hat_config("name: \"Executor\"\npublishes: [\"work.done\"]");
    assert_eq!(
        EventLoop::build_receiver_contract_block(&hat, None),
        None,
        "without an event policy there are no schemas to derive from — no block"
    );
}

#[test]
fn build_receiver_contract_block_sorts_topics_stably() {
    let hat = hat_config("name: \"Executor\"\npublishes: [\"z.done\", \"a.done\"]");
    let policy = event_policy(
        "enabled: true\nmode: enforce\nschemas:\n  a.done:\n    required_fields: [x]\n  z.done:\n    required_fields: [y]",
    );
    let block = EventLoop::build_receiver_contract_block(&hat, Some(&policy)).expect("renders");
    assert!(block.starts_with("## RECEIVER CONTRACT\n"));
    let a_pos = block.find("- a.done: required fields: x").expect("a.done");
    let z_pos = block.find("- z.done: required fields: y").expect("z.done");
    assert!(a_pos < z_pos, "topics must render sorted; block={block}");
}

#[test]
fn build_receiver_contract_block_schema_without_required_fields() {
    let hat = hat_config("name: \"Executor\"\npublishes: [\"work.done\"]");
    let policy = event_policy(
        "enabled: true\nmode: enforce\nschemas:\n  work.done:\n    payload: json_object",
    );
    let block = EventLoop::build_receiver_contract_block(&hat, Some(&policy)).expect("renders");
    assert!(
        block.contains("- work.done: (no required fields declared)"),
        "a schema without required_fields must say so; block={block}"
    );
}

#[test]
fn build_receiver_contract_block_escapes_topic_and_field_names() {
    let hat = hat_config("name: \"Executor\"\npublishes: [\"bad`topic\"]");
    let policy = event_policy(
        "enabled: true\nmode: enforce\nschemas:\n  bad`topic:\n    required_fields: [\"fi`eld\"]",
    );
    let block = EventLoop::build_receiver_contract_block(&hat, Some(&policy)).expect("renders");
    assert!(
        block.contains("- bad``topic: required fields: fi``eld"),
        "backticks must be doubled; block={block}"
    );
}
