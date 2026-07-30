//! 2026-07-30-002 plan U1 (R1/R2): parallel-forge fail-close must
//! derive the blocked topic from the preset's declared flow and
//! append a flow-authority snapshot, so the reporter's
//! `forge.report.done` clears `FlowStepScope` and the loop can
//! close via `LOOP_COMPLETE`.
//!
//! These tests pin the helper-level behaviour:
//!   - `derive_blocked_topic` (D1): one distinct match wins;
//!     0 or ≥2 distinct matches fall back to `plan.blocked`.
//!   - `resolve_escape_step` (D3): forward scan finds the first
//!     step whose `on` or `on_any_of` accepts the topic; once
//!     the current step is past the topic's natural entry, no
//!     step is returned (the helper is a one-shot escape, not a
//!     chain).

use crate::config::{FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig};
use crate::event_loop::{derive_blocked_topic, resolve_escape_step};

fn flow_with_steps(steps: Vec<FlowStepConfig>) -> MechanismConfig {
    MechanismConfig {
        flow: Some(FlowDeclarationConfig {
            flow_type: "declared".to_string(),
            version: 1,
            terminal_emits: vec![],
            steps,
            repair_budget: 0,
            enforce_schema: "hard".to_string(),
            state_idempotency: "required".to_string(),
        }),
        phase_authority: None,
    }
}

fn step(id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>) -> FlowStepConfig {
    FlowStepConfig {
        id: id.to_string(),
        kind: Some("linear".to_string()),
        allowed_emits: allowed.into_iter().map(String::from).collect(),
        terminal_when: None,
        on_partial: Default::default(),
        runs: None,
        on: on.map(String::from),
        on_any_of: on_any_of.into_iter().map(String::from).collect(),
        transition_emits: vec![],
    }
}

fn config_with_mechanism(mechanism: MechanismConfig) -> RalphConfig {
    let mut cfg = RalphConfig::default();
    cfg.mechanism = Some(mechanism);
    cfg
}

// -------- derive_blocked_topic --------

#[test]
fn derive_blocked_topic_single_forge_match_wins() {
    let mechanism = flow_with_steps(vec![
        step("planning", vec!["forge.plan.inspected"], Some("forge.start"), vec![]),
        step(
            "development_loop",
            vec!["forge.wave.settled", "forge.plan.blocked", "work.failed"],
            Some("forge.worktrees.ready"),
            vec![],
        ),
        step(
            "report",
            vec!["forge.report.done"],
            None,
            vec!["forge.audit.done", "forge.plan.blocked"],
        ),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(derive_blocked_topic(&cfg), "forge.plan.blocked");
}

#[test]
fn derive_blocked_topic_plain_plan_blocked_only_wins() {
    // ce-executor-supervisor shape: only `plan.blocked` shows up.
    let mechanism = flow_with_steps(vec![
        step("exec_wave", vec!["exec.wave.complete", "plan.blocked"], None, vec![]),
        step("finalize", vec!["plan.complete"], None, vec![]),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(derive_blocked_topic(&cfg), "plan.blocked");
}

#[test]
fn derive_blocked_topic_falls_back_when_no_mechanism() {
    // Hatless preset: no `mechanism:` block at all.
    let cfg = RalphConfig::default();
    assert_eq!(derive_blocked_topic(&cfg), "plan.blocked");
}

#[test]
fn derive_blocked_topic_falls_back_when_no_blocked_topics() {
    // autoresearch / debug shape: declared flow but no `*.plan.blocked` topic.
    let mechanism = flow_with_steps(vec![
        step("experiment", vec!["experiment.blocked"], None, vec![]),
        step("summarize", vec!["experiment.summary"], None, vec![]),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(derive_blocked_topic(&cfg), "plan.blocked");
}

#[test]
fn derive_blocked_topic_falls_back_when_multiple_distinct_matches() {
    // Defensive: if a future preset ever declares two distinct
    // `*.plan.blocked` topics, the conservative rule wins.
    let mechanism = flow_with_steps(vec![
        step(
            "alpha",
            vec!["alpha.plan.blocked"],
            None,
            vec![],
        ),
        step("beta", vec!["beta.plan.blocked"], None, vec![]),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(derive_blocked_topic(&cfg), "plan.blocked");
}

// -------- resolve_escape_step --------

#[test]
fn resolve_escape_step_finds_report_via_on_any_of() {
    let mechanism = flow_with_steps(vec![
        step("planning", vec!["forge.plan.inspected"], Some("forge.start"), vec![]),
        step(
            "development_loop",
            vec!["forge.wave.settled", "forge.plan.blocked", "work.failed"],
            Some("forge.worktrees.ready"),
            vec![],
        ),
        step(
            "report",
            vec!["forge.report.done"],
            None,
            vec!["forge.audit.done", "forge.plan.blocked", "work.failed"],
        ),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(
        resolve_escape_step(&cfg, "development_loop", "forge.plan.blocked"),
        Some("report".to_string())
    );
}

#[test]
fn resolve_escape_step_returns_none_when_current_is_already_report() {
    // Once we are in `report`, there is no forward step on the
    // blocked topic — the helper is one-shot, not a chain.
    let mechanism = flow_with_steps(vec![
        step("development_loop", vec!["forge.plan.blocked"], None, vec![]),
        step(
            "report",
            vec!["forge.report.done"],
            None,
            vec!["forge.plan.blocked"],
        ),
        step("plan_end", vec!["LOOP_COMPLETE"], Some("forge.report.done"), vec![]),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(
        resolve_escape_step(&cfg, "report", "forge.plan.blocked"),
        None
    );
}

#[test]
fn resolve_escape_step_returns_none_when_no_forward_match() {
    // Supervisor shape: `plan.blocked` is in `allowed_emits`
    // somewhere, but no step declares `on`/`on_any_of` for it.
    let mechanism = flow_with_steps(vec![
        step("exec_wave", vec!["exec.wave.complete", "plan.blocked"], None, vec![]),
        step("finalize", vec!["plan.complete"], None, vec![]),
    ]);
    let cfg = config_with_mechanism(mechanism);
    assert_eq!(
        resolve_escape_step(&cfg, "exec_wave", "plan.blocked"),
        None
    );
}