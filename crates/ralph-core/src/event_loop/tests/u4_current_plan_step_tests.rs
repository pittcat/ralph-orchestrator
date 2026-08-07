use super::super::*;
use crate::config::{
    EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
};

fn flow_config(steps: Vec<(&str, Vec<&str>)>) -> RalphConfig {
    let step_configs: Vec<FlowStepConfig> = steps
        .into_iter()
        .map(|(id, allowed)| FlowStepConfig {
            id: id.to_string(),
            kind: None,
            allowed_emits: allowed.into_iter().map(String::from).collect(),
            terminal_when: None,
            on_partial: std::collections::BTreeMap::new(),
            runs: None,
            on: None,
            on_any_of: Vec::new(),
            transition_emits: Vec::new(),
        })
        .collect();
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: step_configs,
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };
    cfg
}

#[test]
fn initial_returns_first_step_id_from_top_level_mechanism() {
    // Preset SSOT is top-level `mechanism:`, not
    // `event_loop.mechanism`. initial_current_plan_step must
    // read via effective_mechanism_config.
    let mut cfg = RalphConfig::default();
    cfg.mechanism = Some(MechanismConfig {
        flow: Some(FlowDeclarationConfig {
            flow_type: "declared".to_string(),
            version: 1,
            terminal_emits: vec!["LOOP_COMPLETE".to_string()],
            steps: vec![FlowStepConfig {
                id: "unit_loop".to_string(),
                kind: None,
                allowed_emits: vec!["work.ready".to_string()],
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: None,
                on_any_of: Vec::new(),
                transition_emits: Vec::new(),
            }],
            ..FlowDeclarationConfig::default()
        }),
        phase_authority: None,
    });
    assert_eq!(initial_current_plan_step(&cfg), "unit_loop");
}

#[test]
fn initial_returns_first_step_id() {
    let cfg = flow_config(vec![("unit_loop", vec!["work.done"])]);
    assert_eq!(initial_current_plan_step(&cfg), "unit_loop");
}

#[test]
fn initial_returns_empty_when_no_flow() {
    let cfg = RalphConfig::default();
    assert_eq!(initial_current_plan_step(&cfg), "");
}

#[test]
fn advance_on_transition_event() {
    let cfg = flow_config(vec![
        ("unit_loop", vec!["work.done", "review.start"]),
        ("review_walk", vec!["review.complete"]),
    ]);
    let next = advance_plan_step(&cfg, "unit_loop", "review.start");
    assert_eq!(next, Some("review_walk".to_string()));
}

#[test]
fn advance_skips_non_transition_event() {
    let cfg = flow_config(vec![
        ("unit_loop", vec!["work.done", "review.start"]),
        ("review_walk", vec!["review.complete"]),
    ]);
    // work.done is in allowed_emits but not a transition
    // event in this flow — staying on unit_loop is correct.
    let next = advance_plan_step(&cfg, "unit_loop", "work.done");
    assert_eq!(next, None);
}

#[test]
fn advance_returns_none_at_last_step() {
    let cfg = flow_config(vec![("ship", vec!["LOOP_COMPLETE"])]);
    let next = advance_plan_step(&cfg, "ship", "LOOP_COMPLETE");
    assert_eq!(next, None);
}

#[test]
fn advance_returns_none_with_empty_current() {
    let cfg = flow_config(vec![("unit_loop", vec!["review.start"])]);
    let next = advance_plan_step(&cfg, "", "review.start");
    assert_eq!(next, None);
}

#[test]
fn advance_returns_none_when_current_unknown() {
    let cfg = flow_config(vec![("unit_loop", vec!["review.start"])]);
    let next = advance_plan_step(&cfg, "ghost", "review.start");
    assert_eq!(next, None);
}

#[test]
fn advance_no_flow_returns_none() {
    let cfg = RalphConfig::default();
    let next = advance_plan_step(&cfg, "unit_loop", "review.start");
    assert_eq!(next, None);
}

/// 2026-07-26-004 plan U6 (R7 / R8): the flow authority advances by
/// DECLARED transition (`on` / `on_any_of`), is idempotent on repeat,
/// and branches a failed review wave straight to `finalize` (not
/// positionally through `synth_await` / `fix_plan`) — the
/// primary-20260726 flow-drift root cause. Mirrors the
/// implementation-review flow shape.
#[test]
fn u6_declared_transition_authority_is_idempotent_and_branching() {
    let mk =
        |id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk(
                        "scope_freeze",
                        vec!["scope.ready", "scope.blocked"],
                        None,
                        vec![],
                    ),
                    mk(
                        "review_wave",
                        vec![
                            "review.unit.done",
                            "review.wave.complete",
                            "review.wave.failed",
                        ],
                        Some("scope.ready"),
                        vec![],
                    ),
                    mk(
                        "synth_await",
                        vec!["review.synthesized"],
                        Some("review.wave.complete"),
                        vec![],
                    ),
                    mk(
                        "fix_plan",
                        vec!["fix.plan.ready"],
                        Some("review.synthesized"),
                        vec![],
                    ),
                    mk(
                        "finalize",
                        vec!["LOOP_COMPLETE"],
                        None,
                        vec!["fix.plan.ready", "scope.blocked", "review.wave.failed"],
                    ),
                ],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };

    // Declared `on`: scope.ready transitions scope_freeze → review_wave.
    assert_eq!(
        advance_plan_step(&cfg, "scope_freeze", "scope.ready"),
        Some("review_wave".to_string())
    );
    // Idempotent: scope.ready is not allowed at review_wave, so a
    // replayed transition is a no-op (the step does not re-advance).
    assert_eq!(advance_plan_step(&cfg, "review_wave", "scope.ready"), None);
    // Non-transition unit terminal stays on review_wave.
    assert_eq!(
        advance_plan_step(&cfg, "review_wave", "review.unit.done"),
        None
    );
    // Declared `on`: review.wave.complete → synth_await.
    assert_eq!(
        advance_plan_step(&cfg, "review_wave", "review.wave.complete"),
        Some("synth_await".to_string())
    );
    // BRANCH (on_any_of): review.wave.failed jumps straight to
    // finalize, NOT positionally to synth_await.
    assert_eq!(
        advance_plan_step(&cfg, "review_wave", "review.wave.failed"),
        Some("finalize".to_string())
    );
    // BRANCH from the first step: scope.blocked → finalize.
    assert_eq!(
        advance_plan_step(&cfg, "scope_freeze", "scope.blocked"),
        Some("finalize".to_string())
    );
    // Recovery: rebuilding from the initial step + the accepted
    // transition lands on the same step the live loop reached.
    let initial = initial_current_plan_step(&cfg);
    assert_eq!(initial, "scope_freeze");
    assert_eq!(
        advance_plan_step(&cfg, &initial, "scope.ready"),
        Some("review_wave".to_string())
    );
}

/// 2026-07-26-004 plan U7 (R7 / R8): `recover_current_plan_step`
/// rebuilds the SAME current step a resident EventLoop reaches
/// incrementally, by folding the single `advance_plan_step`
/// authority over the accepted topic sequence. A restart / replay /
/// CLI policy-check that calls this never re-derives from the flow's
/// first step independently (the primary-20260726 flow drift).
#[test]
fn u7_recover_current_plan_step_matches_incremental_advance() {
    let mk =
        |id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk(
                        "scope_freeze",
                        vec!["scope.ready", "scope.blocked"],
                        None,
                        vec![],
                    ),
                    mk(
                        "review_wave",
                        vec!["review.unit.done", "review.wave.failed"],
                        Some("scope.ready"),
                        vec![],
                    ),
                    mk(
                        "finalize",
                        vec!["LOOP_COMPLETE"],
                        None,
                        vec!["scope.blocked", "review.wave.failed"],
                    ),
                ],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };

    // No events → first step.
    assert_eq!(recover_current_plan_step(&cfg, &[]), "scope_freeze");
    // After scope.ready → review_wave (matches incremental advance).
    assert_eq!(
        recover_current_plan_step(&cfg, &["scope.ready"]),
        "review_wave"
    );
    // review.unit.done is a non-transition → stays review_wave.
    assert_eq!(
        recover_current_plan_step(&cfg, &["scope.ready", "review.unit.done"]),
        "review_wave"
    );
    // Branch: review.wave.failed → finalize.
    assert_eq!(
        recover_current_plan_step(&cfg, &["scope.ready", "review.wave.failed"]),
        "finalize"
    );
    // Recovery is deterministic: replaying the same sequence twice
    // yields the same step the resident loop holds.
    let seq = ["scope.ready", "review.unit.done", "review.unit.done"];
    assert_eq!(recover_current_plan_step(&cfg, &seq), "review_wave");
    assert_eq!(recover_current_plan_step(&cfg, &seq), "review_wave");
}

// 2026-07-24-005 plan U2 (R2 / R3 / S1 / S6): supervisor
// exec_wave accepts `exec.unit.done` / `exec.unit.failed`
// without advancing the step, while `exec.wave.complete`
// still advances to `exec_integrate`. These three topics
// are pinned in the `NON_TRANSITION_TOPICS` whitelist of
// `advance_plan_step` so the supervisor wave does not
// collapse after the first unit completion.
//
// KTD3: the whitelist is the smaller change vs. the
// alternative of an `exec_unit_*` non-transition bucket.
/// 2026-07-29-001 plan U1 (R1): when a step declares an
/// explicit `transition_emits`, only those topics advance
/// the plan-mode current step. Other topics that remain
/// in `allowed_emits` (e.g. `forge.review.ready`) are
/// still accepted in the current step (FlowStepScope) but
/// no longer collapse the step boundary through the
/// positional-advance fallback. Topic names use a bespoke
/// namespace that avoids the runtime's NON_TRANSITION_TOPICS
/// whitelist, so the assertions actually prove the
/// transition_emits field narrows the authority (the
/// whitelist would otherwise mask the failure on
/// `work.ready`/`work.failed`-style topics).
#[test]
fn u1_transition_emits_only_named_topics_advance() {
    let mk = |id: &str, allowed: Vec<&str>, transition: Vec<&str>| -> FlowStepConfig {
        FlowStepConfig {
            id: id.to_string(),
            kind: None,
            allowed_emits: allowed.into_iter().map(String::from).collect(),
            terminal_when: None,
            on_partial: std::collections::BTreeMap::new(),
            runs: None,
            on: None,
            on_any_of: Vec::new(),
            transition_emits: transition.into_iter().map(String::from).collect(),
        }
    };
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk(
                        "unit_loop",
                        vec![
                            "forge.triage.ready",
                            "forge.triage.partial",
                            "forge.triage.done",
                        ],
                        vec!["forge.triage.done"],
                    ),
                    mk(
                        "review_walk",
                        vec!["forge.review.complete"],
                        vec!["forge.review.complete"],
                    ),
                ],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };
    // forge.triage.done is in transition_emits → advances.
    assert_eq!(
        advance_plan_step(&cfg, "unit_loop", "forge.triage.done"),
        Some("review_walk".to_string())
    );
    // forge.triage.ready is in allowed_emits but NOT in
    // transition_emits → must NOT advance.
    assert_eq!(
        advance_plan_step(&cfg, "unit_loop", "forge.triage.ready"),
        None
    );
    // forge.triage.partial is in allowed_emits but NOT in
    // transition_emits → must NOT advance.
    assert_eq!(
        advance_plan_step(&cfg, "unit_loop", "forge.triage.partial"),
        None
    );
}

/// 2026-07-29-001 plan U1 (R1 / R8): when `transition_emits`
/// is empty (the legacy default), every `allowed_emits`
/// topic remains transition-capable — the contract a
/// preset wrote before this field was introduced.
#[test]
fn u1_empty_transition_emits_keeps_legacy_allowed_emits_authority() {
    let cfg = flow_config(vec![
        ("unit_loop", vec!["work.done", "review.start"]),
        ("review_walk", vec!["review.complete"]),
    ]);
    // review.start advances (legacy contract).
    assert_eq!(
        advance_plan_step(&cfg, "unit_loop", "review.start"),
        Some("review_walk".to_string())
    );
}

/// 2026-07-29-001 plan U1 (R8): resident EventLoop
/// (`advance_plan_step`) and replay (`recover_current_plan_step`)
/// share the same authority. When `transition_emits` is
/// explicit, the replay-folding must agree with the live
/// incremental advance on every accepted topic sequence.
#[test]
fn u1_recover_current_plan_step_matches_incremental_with_transition_emits() {
    let mk = |id: &str, allowed: Vec<&str>, transition: Vec<&str>| -> FlowStepConfig {
        FlowStepConfig {
            id: id.to_string(),
            kind: None,
            allowed_emits: allowed.into_iter().map(String::from).collect(),
            terminal_when: None,
            on_partial: std::collections::BTreeMap::new(),
            runs: None,
            on: None,
            on_any_of: Vec::new(),
            transition_emits: transition.into_iter().map(String::from).collect(),
        }
    };
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk(
                        "unit_loop",
                        vec!["work.ready", "work.failed", "review.start"],
                        vec!["review.start"],
                    ),
                    mk(
                        "review_walk",
                        vec!["review.complete"],
                        vec!["review.complete"],
                    ),
                ],
                ..FlowDeclarationConfig::default()
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };
    // Resident path.
    let mut live = initial_current_plan_step(&cfg);
    assert_eq!(live, "unit_loop");
    for topic in ["work.ready", "work.failed", "review.start"] {
        if let Some(next) = advance_plan_step(&cfg, &live, topic) {
            live = next;
        }
    }
    assert_eq!(live, "review_walk");
    // Replay path — must agree.
    let replayed = recover_current_plan_step(&cfg, &["work.ready", "work.failed", "review.start"]);
    assert_eq!(replayed, live);
}

fn exec_wave_flow() -> RalphConfig {
    flow_config(vec![
        ("unit_loop", vec!["work.ready", "execution.plan.ready"]),
        (
            "exec_wave",
            vec![
                "exec.wave.complete",
                "exec.wave.failed",
                "exec.unit.done",
                "exec.unit.failed",
            ],
        ),
        ("exec_integrate", vec!["plan.complete"]),
    ])
}

#[test]
fn u2_advance_unit_done_on_exec_wave_returns_none() {
    // S1 + R3: a unit terminal on the exec_wave step
    // must NOT advance the plan to exec_integrate.
    let cfg = exec_wave_flow();
    let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.done");
    assert_eq!(next, None);
}

#[test]
fn u2_advance_unit_failed_on_exec_wave_returns_none() {
    let cfg = exec_wave_flow();
    let next = advance_plan_step(&cfg, "exec_wave", "exec.unit.failed");
    assert_eq!(next, None);
}

#[test]
fn u2_advance_wave_complete_on_exec_wave_advances() {
    // S6: the wave terminal must still advance to the
    // next step (exec_integrate) — the wave has truly
    // closed.
    let cfg = exec_wave_flow();
    let next = advance_plan_step(&cfg, "exec_wave", "exec.wave.complete");
    assert_eq!(next, Some("exec_integrate".to_string()));
}

#[test]
fn u2_advance_unit_done_on_unit_loop_returns_none() {
    // S2 boundary: the supervisor preset must NOT
    // double-mount `exec.unit.done` on `unit_loop`;
    // the helper still returns None because the topic
    // is not in `unit_loop.allowed_emits` (and is in
    // the non-transition list).
    let cfg = exec_wave_flow();
    let next = advance_plan_step(&cfg, "unit_loop", "exec.unit.done");
    assert_eq!(next, None);
}

#[test]
fn u2_advance_execution_plan_ready_advances_to_exec_wave() {
    // S3 / R4: `execution.plan.ready` accepted on
    // `unit_loop` advances to `exec_wave`. Confirms
    // the flow declaration wires task-planner →
    // exec-wave-dispatcher.
    let cfg = exec_wave_flow();
    let next = advance_plan_step(&cfg, "unit_loop", "execution.plan.ready");
    assert_eq!(next, Some("exec_wave".to_string()));
}

fn review_fix_wave_flow() -> RalphConfig {
    flow_config(vec![
        (
            "review_loop",
            vec![
                "review.wave.complete",
                "review.wave.failed",
                "review.unit.ready",
                "review.unit.done",
            ],
        ),
        (
            "fix_loop",
            vec![
                "fix.wave.complete",
                "fix.wave.failed",
                "fix.unit.ready",
                "fix.unit.done",
                "fix.unit.failed",
            ],
        ),
        ("plan_end", vec!["plan.complete"]),
    ])
}

#[test]
fn u2_review_unit_done_on_review_loop_returns_none() {
    let cfg = review_fix_wave_flow();
    assert_eq!(
        advance_plan_step(&cfg, "review_loop", "review.unit.done"),
        None
    );
}

#[test]
fn u2_fix_unit_done_on_fix_loop_returns_none() {
    let cfg = review_fix_wave_flow();
    assert_eq!(advance_plan_step(&cfg, "fix_loop", "fix.unit.done"), None);
    assert_eq!(advance_plan_step(&cfg, "fix_loop", "fix.unit.failed"), None);
}

#[test]
fn u2_review_wave_complete_advances_to_fix_loop() {
    let cfg = review_fix_wave_flow();
    assert_eq!(
        advance_plan_step(&cfg, "review_loop", "review.wave.complete"),
        Some("fix_loop".to_string())
    );
}

#[test]
fn u2_fix_wave_complete_advances_to_plan_end() {
    let cfg = review_fix_wave_flow();
    assert_eq!(
        advance_plan_step(&cfg, "fix_loop", "fix.wave.complete"),
        Some("plan_end".to_string())
    );
}
