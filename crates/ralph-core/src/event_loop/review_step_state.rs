//! Per-step review terminal state for plan-gate hard enforcement (U1/U3).

use crate::event_policy::{PolicyFinding, ViolationType};
use crate::event_reader::Event as JsonlEvent;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Emitted when a review wave exceeds the synthesizer aggregate window (U4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateTimeoutAction {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
    pub wave_id: String,
    pub received: u32,
    pub expected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StepKey {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
}

/// U3 (2026-06-17-003 plan): minimal projection of an open wave
/// used by the rejection-hint formatter to print the
/// `## WAVE_OPEN HINT` block. Carries no time information — the
/// textual hint only needs the wave id + receive/total counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWaveSnapshot {
    pub wave_id: String,
    pub received: u32,
    pub expected: u32,
}

#[derive(Debug, Clone, Default)]
struct StepReviewState {
    open_wave_id: Option<String>,
    wave_expected: u32,
    wave_started_at: Option<Instant>,
    /// U2 (2026-06-17-003 plan): wall-clock of the most recent
    /// `review.dimension.done` for this step's open wave. Used by
    /// `open_waves_needing_intervention` to detect the
    /// "incomplete + no-progress" stall that triggers the
    /// mechanism-emitted `plan.blocked`. `None` until the first
    /// `review.dimension.done` arrives.
    last_dimension_at: Option<Instant>,
    aggregate_timeout_dispatched: bool,
    dimensions_received: HashSet<String>,
    synth_terminal: Option<String>,
    synth_pass: bool,
    failed_pending_fix: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ReviewStepTracker {
    steps: HashMap<StepKey, StepReviewState>,
    /// U2 of plan 2026-07-02-005: plan-level review terminal state.
    ///
    /// 140149 root cause: `review.complete` is a **plan-level** event
    /// (one review synthesizes findings across every unit of the
    /// plan), but the existing per-step gate requires `task_id` to
    /// match the per-step entry exactly. When the terminal review
    /// emits `plan.complete` with a different `task_id` (e.g.
    /// `finalize` task that aggregates everything), per-step
    /// matching fails and `plan_gate_review_not_terminal` rejects.
    ///
    /// When `observe_accepted` sees `review.complete` /
    /// `review.passed` with a pass-class verdict AND a null/empty
    /// `fix_plan_file`, we record `plan_review_terminal[plan_name] =
    /// pass`. The `plan.complete` gate consults this map FIRST and
    /// accepts non-`fix-*` `plan.complete` as long as the plan has
    /// hit a terminal pass.
    plan_review_terminal: HashMap<String, PlanReviewTerminal>,
}

/// U2 of plan 2026-07-02-005: plan-level terminal projection. One
/// entry per plan that has reached `review.complete` /
/// `review.passed` with a pass-class verdict AND no pending fix
/// plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReviewTerminal {
    /// Verdict string from the original `review.complete` /
    /// `review.passed` payload (e.g. `pass`,
    /// `pass_with_residuals`).
    pub verdict: String,
    /// Last observed `fix_plan_file` value. When non-null /
    /// non-empty, the terminal is **not** considered plan-level
    /// pass — fix-unit chain is still pending.
    pub fix_plan_file_seen: Option<String>,
}

/// U2 (2026-06-17-003 plan): describe an open wave that the
/// mechanism should emit `plan.blocked` for. Constructed by
/// [`ReviewStepTracker::open_waves_needing_intervention`].
///
/// `expected` is the wave's `wave_total`. `received` is the
/// count of **unique** dimensions already reported (the
/// tracker's set deduplicates duplicate `dimension.done`
/// events for the same dimension). `missing_dimensions` is
/// the set of dimension labels the wave still expects — when
/// the agent emits `dimension` strings (e.g. `sec`, `perf`),
/// those flow into the audit payload; otherwise the set is
/// empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteWaveInfo {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
    pub wave_id: String,
    pub expected: u32,
    pub received: u32,
    pub missing_dimensions: Vec<String>,
    pub started_at: Instant,
    pub last_dimension_at: Option<Instant>,
}

fn step_key_from_event(topic: &str, payload: Option<&str>) -> Option<StepKey> {
    let p = payload?;
    let obj = serde_json::from_str::<Value>(p).ok()?;
    let plan_name = obj.get("plan_name")?.as_str()?.to_string();
    match topic {
        "queue.advance" | "work.ready" => {
            // Step-advance handoffs from plan-gate carry reviewed-step
            // correlation fields; coordinator's initial work.ready does not.
            if let Some(task_id) = obj.get("reviewed_task_id").and_then(|v| v.as_str()) {
                let step = obj.get("completed_step")?.as_str()?.to_string();
                return Some(StepKey {
                    plan_name,
                    task_id: task_id.to_string(),
                    step,
                });
            }
            if topic == "queue.advance" {
                return None;
            }
            let task_id = obj.get("task_id")?.as_str()?.to_string();
            let step = obj.get("step")?.as_str()?.to_string();
            Some(StepKey {
                plan_name,
                task_id,
                step,
            })
        }
        _ => {
            let task_id = obj.get("task_id")?.as_str()?.to_string();
            let step = obj.get("step")?.as_str()?.to_string();
            Some(StepKey {
                plan_name,
                task_id,
                step,
            })
        }
    }
}

fn plan_gate_step_gate(topic: &str, state: &StepReviewState) -> Option<PolicyFinding> {
    if state.failed_pending_fix {
        return Some(plan_gate_finding(
            topic,
            "plan_gate_review_failed_pending_fix",
        ));
    }
    let terminal_ok = state
        .synth_terminal
        .as_deref()
        .is_some_and(|t| matches!(t, "review.passed" | "review.complete") && state.synth_pass);
    if !terminal_ok {
        Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"))
    } else {
        None
    }
}

fn wave_open(state: &StepReviewState) -> bool {
    state.open_wave_id.is_some()
        && (state.wave_expected == 0
            || (state.dimensions_received.len() as u32) < state.wave_expected)
}

fn plan_gate_finding(topic: &str, reason: &str) -> PolicyFinding {
    PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::BusinessEventAfterCompletion {
            topic: topic.to_string(),
        },
        message: format!(
            "{reason}: cannot emit '{topic}' until review-synthesizer terminal \
             (review.passed or review.complete with pass verdict) for this step"
        ),
    }
}

/// U2 of plan 2026-07-02-005: helper that decides whether a
/// `verdict` payload field counts as a pass-class verdict for the
/// purposes of `plan_review_terminal`. Mirrors the per-step
/// `synth_pass` rule (`verdict != "fail"`).
fn is_pass_class_verdict(verdict: &str) -> bool {
    !verdict.eq_ignore_ascii_case("fail")
}

impl ReviewStepTracker {
    /// 2026-06-28-002 U1: 解析 fix-plan 文件中 `### U{N}.` 形式的标题，
    /// 为每个 `fix-{NN}` step 预填 `synth_terminal` + `synth_pass`。
    /// 失败（文件不存在 / 解析失败）静默忽略 —— plan_gate 的豁免
    /// 已经在 `plan.complete` step=`fix-*` 分支生效，这里只是给
    /// tracker 提供更早的"已填好"状态，便于下游做诊断与后续
    /// `is_wave_closed` 查询。
    ///
    /// 注：解析器为自包含内联实现，不依赖 2026-07-01-001 U6 已回滚的
    /// 共享 `scan_unit_headings` helper。此函数与其调用点属于
    /// 2026-06-28-002 U1，不在 U6 回滚范围内。
    fn prefill_fix_steps_from_plan(&mut self, plan_path: &str) {
        let Ok(content) = std::fs::read_to_string(plan_path) else {
            return;
        };
        let mut found = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("### U") {
                continue;
            }
            // 期望形式：`### U{N}. <title>` 或 `### U{N} <title>`
            let after_marker = trimmed.trim_start_matches("### U");
            let digits: String = after_marker
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u32>()
                && n > 0
            {
                found.push(format!("fix-{n:02}"));
            }
        }
        if found.is_empty() {
            return;
        }
        // 拿到已存在的任意 step_key（plan_name + task_id）作为模板，
        // 为新 prefill 的 fix-{NN} 复制 plan_name + task_id。
        let (plan_name, task_id) = match self.steps.keys().next() {
            Some(k) => (k.plan_name.clone(), k.task_id.clone()),
            None => return,
        };
        for fix_step in found {
            let key = StepKey {
                plan_name: plan_name.clone(),
                task_id: task_id.clone(),
                step: fix_step,
            };
            let state = self.steps.entry(key).or_default();
            if state.synth_terminal.is_none() {
                state.synth_terminal = Some("review.complete".to_string());
                state.synth_pass = true;
                state.open_wave_id = None;
            }
        }
    }

    /// Semantic gates that run after schema validation (U1/U3).
    pub fn check_semantic_gates(&self, event: &JsonlEvent) -> Option<PolicyFinding> {
        let hat = event.hat.as_deref().unwrap_or("");
        let topic = event.topic.as_str();

        if hat == "review-coordinator" && topic == "review.passed" {
            if let Some(key) = step_key_from_event(topic, event.payload.as_deref()) {
                if let Some(state) = self.steps.get(&key)
                    && wave_open(state)
                {
                    // U1 (2026-06-17-003 plan): emit the
                    // dedicated `SemanticGateViolation` variant
                    // instead of forging `InvalidFieldValue {
                    // field: "skip_reason" }`. The payload itself
                    // is well-formed; the violation is in the
                    // **state** (wave open + coordinator
                    // fast-pathing). The runtime loop classifies
                    // this as recoverable and continues — see
                    // `is_recoverable_policy_finding` and the
                    // runner's `PayloadContractViolation` branch.
                    // The `gate` field carries the canonical name
                    // (`review_passed_while_wave_open`) for
                    // audit/diagnostics.
                    return Some(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::SemanticGateViolation {
                            gate: "review_passed_while_wave_open".to_string(),
                            context: format!(
                                "wave='{}' received={}/{} expected",
                                state.open_wave_id.as_deref().unwrap_or("?"),
                                state.dimensions_received.len(),
                                state.wave_expected,
                            ),
                        },
                        message: format!(
                            "review_passed_while_wave_open: review-coordinator must not emit \
                             review.passed while wave '{}' is incomplete ({}/{} dimensions)",
                            state.open_wave_id.as_deref().unwrap_or("?"),
                            state.dimensions_received.len(),
                            state.wave_expected
                        ),
                    });
                }
            }
        }

        if topic == "review.passed"
            && let Some(p) = event.payload.as_deref()
            && let Ok(Value::Object(obj)) = serde_json::from_str(p)
            && obj.get("skip_reason").and_then(|v| v.as_str()) == Some("aggregate_timeout")
            && hat != "review-synthesizer"
        {
            return Some(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::InvalidFieldValue {
                    field: "skip_reason".to_string(),
                    value: Value::String("aggregate_timeout".to_string()),
                },
                message: "aggregate_timeout skip_reason is reserved for review-synthesizer"
                    .to_string(),
            });
        }

        if topic == "queue.advance" {
            let key = step_key_from_event(topic, event.payload.as_deref())?;
            let Some(state) = self.steps.get(&key) else {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            };
            return plan_gate_step_gate(topic, state);
        }

        if topic == "work.ready" {
            let p = event.payload.as_deref()?;
            let obj = serde_json::from_str::<Value>(p).ok()?;
            // Coordinator bootstrap work.ready has no reviewed-step correlation;
            // only step-advance handoffs from plan-gate are gated.
            if obj
                .get("reviewed_task_id")
                .and_then(|v| v.as_str())
                .is_none()
            {
                return None;
            }
            let key = step_key_from_event(topic, event.payload.as_deref())?;
            let Some(state) = self.steps.get(&key) else {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            };
            return plan_gate_step_gate(topic, state);
        }

        if topic == "plan.complete" {
            let p = event.payload.as_deref()?;
            let obj = serde_json::from_str::<Value>(p).ok()?;
            let plan_name = obj.get("plan_name")?.as_str()?;
            let task_id = obj.get("task_id")?.as_str()?;
            // 2026-06-28-002 U1: fix-unit 流程走 `review.complete(fix_plan_file)`
            // 而不是 review.passed/review.complete 的逐 step terminal。
            // coordinator 在 `plan.complete` 时携带 `step="fix-{NN}"`，
            // 我们直接放行，避免 plan_gate 死锁。
            //
            // 2026-06-30 P0-1 (primary-153653): `step` 也可能为对象
            // 形态 `{"id":"fix-02","last_in_phase":true}`
            // (CoordinatorDecisionGateStage::rewrite_work_ready_topic
            // 在改 topic 时不重写 payload，参见
            // `event_loop::stages::coordinator_decision_gate_stage`)。
            // 这里同时支持 string 与 object.id 两种形态。
            let step_str = obj
                .get("step")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(o) => o.get("id").and_then(|i| i.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            if step_str.starts_with("fix-") {
                return None;
            }

            // U2 of plan 2026-07-02-005: 优先看 plan 级 terminal。
            // 140149 路径：`review.complete(pass_with_residuals,
            // fix_plan_file=null)` 已经把 plan 整体标为 pass，
            // 但 `plan.complete` 的 `task_id` 可能与任何单个 step
            // 的 `task_id` 都不匹配 — 这种情况下 per-step 匹配必然
            // 返回空集。`plan_review_terminal` 是 plan 级终态的
            // 单一事实源；存在即放行（fail verdict 永远不会被
            // `observe_accepted` 记录到这里，见实现）。
            //
            // 但是：`review.failed` 已标记 `failed_pending_fix`
            // 的 per-step 状态仍然高于 plan-level terminal。如果
            // 任意 per-step 在 fail 队列，agent 必须先走 fix 单元
            // 才能 `plan.complete`。这是 `failed_then_passed_blocks_
            // plan_complete` 的核心意图。
            if let Some(terminal) = self.plan_review_terminal.get(plan_name)
                && is_pass_class_verdict(&terminal.verdict)
            {
                let any_failed_pending = self
                    .steps
                    .values()
                    .any(|s| s.failed_pending_fix);
                if !any_failed_pending {
                    return None;
                }
            }

            let matching: Vec<_> = self
                .steps
                .iter()
                .filter(|(k, _)| k.plan_name == plan_name && k.task_id == task_id)
                .collect();
            if matching.is_empty() {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            }
            if matching.iter().any(|(_, s)| s.failed_pending_fix) {
                return Some(plan_gate_finding(
                    topic,
                    "plan_gate_review_failed_pending_fix",
                ));
            }
            let terminal_ok = matching.iter().all(|(_, s)| {
                s.synth_terminal.as_deref().is_some_and(|t| {
                    matches!(t, "review.passed" | "review.complete") && s.synth_pass
                })
            });
            if !terminal_ok {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            }
        }

        None
    }

    /// Update step state after an event passes all validation layers.
    pub fn observe_accepted(&mut self, event: &JsonlEvent) {
        let hat = event.hat.as_deref().unwrap_or("");
        let topic = event.topic.as_str();

        if matches!(topic, "plan.complete" | "queue.advance") {
            return;
        }

        let Some(key) = step_key_from_event(topic, event.payload.as_deref()) else {
            return;
        };
        let state = self.steps.entry(key).or_default();

        match topic {
            "review.wave.ready" => {
                state.open_wave_id = event.wave_id.clone();
                state.wave_expected = event.wave_total.unwrap_or(0);
                state.wave_started_at = Some(Instant::now());
                state.aggregate_timeout_dispatched = false;
                state.dimensions_received.clear();
            }
            "review.dimension.done" => {
                if let Some(open) = &state.open_wave_id
                    && event.wave_id.as_ref() != Some(open)
                {
                    return;
                }
                if let Some(p) = event.payload.as_deref()
                    && let Ok(Value::Object(obj)) = serde_json::from_str(p)
                    && let Some(dim) = obj.get("dimension").and_then(|v| v.as_str())
                {
                    state.dimensions_received.insert(dim.to_string());
                }
                // U2 (2026-06-17-003 plan): bump the
                // "last progress" timestamp so the staleness
                // gate in `open_waves_needing_intervention`
                // can distinguish "stalled" (no recent
                // dimension.done) from "slow but moving".
                state.last_dimension_at = Some(Instant::now());
                if state.wave_expected > 0
                    && state.dimensions_received.len() as u32 >= state.wave_expected
                {
                    state.open_wave_id = None;
                }
            }
            "review.passed" | "review.complete" => {
                let pass = event
                    .payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<Value>(p).ok())
                    .and_then(|obj| {
                        obj.get("verdict")
                            .and_then(|v| v.as_str())
                            .map(|v| v != "fail")
                    })
                    .unwrap_or(true);
                if hat == "review-coordinator" && wave_open(state) {
                    return;
                }
                state.synth_terminal = Some(topic.to_string());
                state.synth_pass = pass;
                state.open_wave_id = None;

                // U2 of plan 2026-07-02-005: record plan-level
                // terminal. We only mark the plan as terminal when
                // (a) the verdict is pass-class AND (b) `fix_plan_file`
                // is null/missing/empty. Otherwise the fix-unit
                // chain is still pending and the per-step matching
                // remains authoritative (existing `prefill_fix_steps_from_plan`
                // handles that branch).
                if let Some(p) = event.payload.as_deref()
                    && let Ok(Value::Object(obj)) = serde_json::from_str(p)
                    && let Some(plan_name) =
                        obj.get("plan_name").and_then(|v| v.as_str())
                    && pass
                {
                    let fix_plan_file = obj
                        .get("fix_plan_file")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty() && s != "null");
                    if fix_plan_file.is_none() {
                        let verdict = obj
                            .get("verdict")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pass")
                            .to_string();
                        self.plan_review_terminal.insert(
                            plan_name.to_string(),
                            PlanReviewTerminal {
                                verdict,
                                fix_plan_file_seen: None,
                            },
                        );
                    }
                }

                // 2026-06-28-002 U1: `review.complete` 携带非空
                // `fix_plan_file` 时，按 fix-plan 中的 `### U{N}.`
                // 数量预填每个 fix-{NN} step 的 synth_terminal。
                // 这条路径独立于当前 step_key —— fix-unit 的
                // review.complete 通常以 `step="fix-NN"` 落盘，
                // 但 fix-plan 才是真正的"全部 U 编号"事实源。
                if topic == "review.complete"
                    && let Some(p) = event.payload.as_deref()
                    && let Ok(Value::Object(obj)) = serde_json::from_str(p)
                    && let Some(plan_path) = obj.get("fix_plan_file").and_then(|v| v.as_str())
                    && plan_path != "null"
                    && !plan_path.is_empty()
                {
                    self.prefill_fix_steps_from_plan(plan_path);
                }
            }
            "review.failed" => {
                state.failed_pending_fix = true;
                state.synth_terminal = None;
                state.synth_pass = false;
            }
            "fix.applied" => {
                state.failed_pending_fix = false;
            }
            _ => {}
        }
    }

    /// B2 (002-adversarial-review): apply a
    /// `CommitDelta::ReviewStepUpdated` payload to the tracker.
    /// The previous `apply_delta` path was a no-op for this
    /// variant, so `replay_from_disk` never rebuilt the
    /// review-step state. The runtime kept working because the
    /// live `LoopState::review_step_tracker` was mutated by the
    /// legacy path; the snapshot stayed empty across cold
    /// start.
    ///
    /// The method takes the **scalar** fields the delta carries
    /// (synth_pass / synth_terminal) and applies them to the
    /// matching `StepReviewState`. Unknown plan / task / step
    /// triples are inserted as a fresh entry — replay
    /// reconstructs the state from the commit log alone.
    pub fn apply_review_step_delta(
        &mut self,
        plan_name: &str,
        task_id: &str,
        step: &str,
        synth_pass: bool,
        synth_terminal: Option<&str>,
    ) {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        let state = self.steps.entry(key).or_default();
        state.synth_pass = synth_pass;
        if let Some(term) = synth_terminal {
            state.synth_terminal = Some(term.to_string());
            // A terminal event implicitly closes the wave;
            // `wave_open` (the `!state.open_wave_id.is_some()
            // || dimensions_received.len() >= wave_expected`
            // check) keys off `open_wave_id`; the legacy
            // `observe_accepted` clears it on `review.passed` /
            // `review.complete` so we mirror that here.
            state.open_wave_id = None;
        }
    }

    /// True when any tracked step still has an incomplete review wave.
    pub fn has_open_review_wave(&self) -> bool {
        self.steps.values().any(wave_open)
    }

    /// R-F5 / 003-U5: query whether the review wave for a given step
    /// has fully closed (all dimensions received OR a verdict terminal
    /// has been emitted). Returns `true` only when the tracker has
    /// NO open wave for that step AND either no wave was ever opened
    /// or it was already completed (received >= expected or terminal
    /// event seen).
    ///
    /// Used by agents and the runner to gate `last_reviewed_sha`
    /// persistence: writing the SHA is only safe after the wave
    /// closes, so DEC-002 empty_diff fast-paths cannot use a premature
    /// SHA as fuel.
    pub fn is_wave_closed(&self, plan_name: &str, task_id: &str, step: &str) -> bool {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        match self.steps.get(&key) {
            None => true, // No tracker entry means no wave ever opened.
            Some(state) => !wave_open(state),
        }
    }

    /// U3 (2026-06-17-003 plan): return a small snapshot of the
    /// first open review wave tracked by the registry, or `None`
    /// if every wave is closed. The snapshot carries the fields
    /// the rejection-hint formatter needs to print the
    /// `## WAVE_OPEN HINT` block on a `work.done` rejection —
    /// `wave_id`, `received` (`dimensions_received.len()`),
    /// `expected` (`wave_expected`). Used only for the textual
    /// rejection hint; the mechanism layer (`open_waves_needing_intervention`
    /// + `maybe_emit_incomplete_wave_blocked`) remains the
    /// single source of truth for whether the wave is actually
    /// stalled and whether `plan.blocked` should be emitted.
    pub fn first_open_wave_snapshot(&self) -> Option<OpenWaveSnapshot> {
        for state in self.steps.values() {
            if !wave_open(state) {
                continue;
            }
            let Some(wave_id) = state.open_wave_id.clone() else {
                continue;
            };
            return Some(OpenWaveSnapshot {
                wave_id,
                received: state.dimensions_received.len() as u32,
                expected: state.wave_expected,
            });
        }
        None
    }

    /// U2 (2026-06-17-003 plan): close the wave tracked under
    /// `key`. Idempotent — returns `true` if a wave was actually
    /// open and was closed; `false` otherwise. Used by the
    /// mechanism's `plan.blocked` emit path so the gate does
    /// not re-fire on subsequent iterations.
    pub fn close_wave(&mut self, key: &StepKey) -> bool {
        if let Some(state) = self.steps.get_mut(key)
            && state.open_wave_id.is_some()
        {
            state.open_wave_id = None;
            state.aggregate_timeout_dispatched = true;
            return true;
        }
        false
    }

    /// U2 (2026-06-17-003 plan): enumerate the open review waves
    /// that exceed `staleness_secs` past their **last dimension
    /// progress** without converging. The caller compares
    /// `now.duration_since(last_dimension_at) > staleness_secs`
    /// to decide whether to emit the mechanism-level
    /// `plan.blocked`.
    ///
    /// `staleness_secs` is the configured aggregate timeout in
    /// seconds; the production gate uses `0.8 * aggregate_timeout_secs`
    /// but the function takes the absolute threshold so unit
    /// tests can compress time without depending on the
    /// configured `aggregate.timeout`.
    ///
    /// Returns **one entry per (plan_name, task_id, step) wave**.
    /// The caller is expected to dedup across iterations (via the
    /// `aggregate_timeout_dispatched` flag pattern or an external
    /// ledger) so this is a pure observation — emitting is the
    /// caller's job.
    pub fn open_waves_needing_intervention(&self, staleness_secs: u64) -> Vec<IncompleteWaveInfo> {
        let now = Instant::now();
        let staleness = std::time::Duration::from_secs(staleness_secs);
        let mut out = Vec::new();
        for (key, state) in &self.steps {
            if !wave_open(state) {
                continue;
            }
            // Only intervene when at least one dimension has
            // arrived — without a baseline, the wave is simply
            // "just started" and the staleness math has no
            // anchor. We skip pure "no workers yet" cases; the
            // aggregate-timeout path (U4 / `inject_review_aggregate_timeouts`)
            // still covers them.
            let Some(last_dim) = state.last_dimension_at else {
                continue;
            };
            if now.duration_since(last_dim) <= staleness {
                continue;
            }
            // Expected vs received counts are unique (set-based).
            let received = state.dimensions_received.len() as u32;
            let expected = state.wave_expected;
            // Missing dimensions: the caller does not know what
            // names the wave expects unless the tracker can
            // observe them. Today the tracker only learns
            // dimensions on `dimension.done`, so we expose the
            // **unfilled** ones the agent has not yet reported
            // for this wave. When the wave's `wave_total` is
            // known but the per-dimension labels are not (most
            // `review.wave.ready` events), `missing_dimensions`
            // is empty and the audit surfaces counts only.
            let missing_dimensions: Vec<String> = Vec::new();
            out.push(IncompleteWaveInfo {
                plan_name: key.plan_name.clone(),
                task_id: key.task_id.clone(),
                step: key.step.clone(),
                wave_id: state.open_wave_id.clone().unwrap_or_default(),
                expected,
                received,
                missing_dimensions,
                started_at: state.wave_started_at.unwrap_or(now),
                last_dimension_at: Some(last_dim),
            });
        }
        out
    }

    /// Steps whose review wave exceeded `timeout` without receiving all dimensions (U4).
    pub fn drain_expired_aggregate_timeouts(
        &mut self,
        timeout: Duration,
    ) -> Vec<AggregateTimeoutAction> {
        let now = Instant::now();
        let mut actions = Vec::new();
        for (key, state) in &mut self.steps {
            if !wave_open(state) || state.aggregate_timeout_dispatched {
                continue;
            }
            let Some(started) = state.wave_started_at else {
                continue;
            };
            if now.duration_since(started) <= timeout {
                continue;
            }
            state.aggregate_timeout_dispatched = true;
            actions.push(AggregateTimeoutAction {
                plan_name: key.plan_name.clone(),
                task_id: key.task_id.clone(),
                step: key.step.clone(),
                wave_id: state.open_wave_id.clone().unwrap_or_default(),
                received: state.dimensions_received.len() as u32,
                expected: state.wave_expected,
            });
        }
        actions
    }

    #[cfg(test)]
    fn backdate_open_wave_for_test(
        &mut self,
        plan_name: &str,
        task_id: &str,
        step: &str,
        ago: Duration,
    ) {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        if let Some(state) = self.steps.get_mut(&key) {
            state.wave_started_at =
                Some(Instant::now().checked_sub(ago).unwrap_or_else(Instant::now));
        }
    }

    /// U2 (2026-06-17-003 plan): test-only helper to back-date
    /// the `last_dimension_at` field so the staleness gate in
    /// `open_waves_needing_intervention` can be exercised
    /// without sleeping.
    #[cfg(test)]
    fn backdate_last_dimension_for_test(
        &mut self,
        plan_name: &str,
        task_id: &str,
        step: &str,
        ago: Duration,
    ) {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        if let Some(state) = self.steps.get_mut(&key) {
            state.last_dimension_at =
                Some(Instant::now().checked_sub(ago).unwrap_or_else(Instant::now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, PayloadType, ViolationAction,
    };
    use crate::event_policy::{PolicyDecision, PolicyRuntimeState, validate_event};
    use std::collections::HashMap;

    fn jsonl(topic: &str, hat: &str, payload: &str) -> JsonlEvent {
        JsonlEvent {
            topic: topic.to_string(),
            payload: Some(payload.to_string()),
            ts: String::new(),
            hat: Some(hat.to_string()),
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }
    }

    fn ce_executor_schemas() -> EventPolicyConfig {
        let mut config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert(
            "review.passed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".into(),
                    "task_id".into(),
                    "task_key".into(),
                    "step".into(),
                    "findings_count".into(),
                    "fix_round".into(),
                    "verdict".into(),
                    "skip_reason".into(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        config.schemas.insert(
            "review.failed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".into(),
                    "fix_round".into(),
                    "safe_auto_count".into(),
                    "gated_manual_count".into(),
                    "findings_summary".into(),
                    "task_id".into(),
                    "task_key".into(),
                    "step".into(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        config
    }

    #[test]
    fn plan_complete_rejected_without_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );

        tracker.observe_accepted(&passed);
        assert!(tracker.check_semantic_gates(&plan_complete).is_none());

        let tracker2 = ReviewStepTracker::default();
        let finding = tracker2
            .check_semantic_gates(&plan_complete)
            .expect("must reject");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn session_b_incomplete_passed_rejected_by_schema() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","skip_reason":"empty_diff"}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn session_b_string_failed_rejected_by_schema() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "review.failed",
            Some("Review failed due to critical issues in src/lib.rs"),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn coordinator_passed_while_wave_open_rejected() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(3);
        tracker.observe_accepted(&wave);

        let passed = jsonl(
            "review.passed",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let finding = tracker.check_semantic_gates(&passed).expect("must reject");
        assert!(finding.message.contains("review_passed_while_wave_open"));
        // U1 (2026-06-17-003 plan): the finding must be the
        // dedicated `SemanticGateViolation` variant — NOT a
        // forged `InvalidFieldValue { field: "skip_reason" }`.
        // The gate field carries the canonical name for audit.
        match &finding.violation_type {
            ViolationType::SemanticGateViolation { gate, context } => {
                assert_eq!(gate, "review_passed_while_wave_open");
                assert!(
                    context.contains("received=0/3"),
                    "context must surface dimensions counts, got: {context}"
                );
            }
            other => panic!(
                "expected SemanticGateViolation, got {other:?} (must NOT be the legacy \
                 InvalidFieldValue{{field: skip_reason}} forged variant)"
            ),
        }
        // And the event must be classified as recoverable in the
        // independent bucket — this is what keeps the loop from
        // terminating with `PayloadContractViolation`.
        use crate::event_policy::is_recoverable_policy_finding;
        let class = is_recoverable_policy_finding(&finding)
            .expect("SemanticGateViolation must be in the recoverable set");
        assert_eq!(
            class,
            crate::event_policy::ReasonClass::SemanticGateViolation
        );
    }

    /// U1 (2026-06-17-003 plan): ensure the real schema-level
    /// `skip_reason` allowed_values mismatch still routes to
    /// `AllowedValueMismatch` and stays in the **non-recoverable**
    /// fatal bucket. This is the regression guard that
    /// `finding_to_payload_contract_violation`'s
    /// `InvalidFieldValue` arm remains unchanged.
    #[test]
    fn real_skip_reason_allowed_value_mismatch_stays_fatal() {
        use crate::event_policy::{ViolationType, is_recoverable_policy_finding};
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::InvalidFieldValue {
                field: "skip_reason".to_string(),
                value: serde_json::Value::String("not_an_allowed_value".to_string()),
            },
            message: "Field 'skip_reason' has invalid value \"not_an_allowed_value\".".to_string(),
        };
        // Schema-derived `InvalidFieldValue` MUST remain
        // non-recoverable so the U6 `PayloadContractViolation`
        // fatal path still triggers. U1 only re-classifies the
        // semantic-gate variant — not the real allowed_values
        // mismatch.
        assert!(
            is_recoverable_policy_finding(&finding).is_none(),
            "real skip_reason AllowedValueMismatch must stay fatal"
        );
    }

    #[test]
    fn failed_then_passed_blocks_plan_complete() {
        let mut tracker = ReviewStepTracker::default();
        let failed = jsonl(
            "review.failed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","fix_round":0,"safe_auto_count":1,"gated_manual_count":0,"findings_summary":"x"}"#,
        );
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );

        tracker.observe_accepted(&failed);
        tracker.observe_accepted(&passed);
        let finding = tracker
            .check_semantic_gates(&plan_complete)
            .expect("must reject");
        assert!(
            finding
                .message
                .contains("plan_gate_review_failed_pending_fix")
        );
    }

    #[test]
    fn queue_advance_rejected_without_review_state() {
        let tracker = ReviewStepTracker::default();
        let advance = jsonl(
            "queue.advance",
            "plan-gate",
            r#"{"plan_name":"p","completed_step":"1","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
        );
        let finding = tracker.check_semantic_gates(&advance).expect("must reject");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn work_ready_step_advance_rejected_without_synth_terminal() {
        let tracker = ReviewStepTracker::default();
        let ready = jsonl(
            "work.ready",
            "plan-gate",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t2","task_key":"k2","step":"2","complexity":"small","reviewed_task_id":"t1","reviewed_task_key":"k1","completed_step":"1","next_step":"2"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&ready)
            .expect("must reject step-advance work.ready without synth terminal");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn work_ready_step_advance_allowed_after_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);

        let advance = jsonl(
            "queue.advance",
            "plan-gate",
            r#"{"plan_name":"p","completed_step":"1","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
        );
        let ready = jsonl(
            "work.ready",
            "plan-gate",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t2","task_key":"k2","step":"2","complexity":"small","reviewed_task_id":"t1","reviewed_task_key":"k1","completed_step":"1","next_step":"2"}"#,
        );

        assert!(
            tracker.check_semantic_gates(&advance).is_none(),
            "queue.advance must pass after synth terminal"
        );
        assert!(
            tracker.check_semantic_gates(&ready).is_none(),
            "work.ready handoff must pass after synth terminal (P1 / merry-wren fix)"
        );
    }

    #[test]
    fn coordinator_initial_work_ready_not_gated_by_review_state() {
        let tracker = ReviewStepTracker::default();
        let ready = jsonl(
            "work.ready",
            "coordinator",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t1","task_key":"k1","step":"1","complexity":"small"}"#,
        );
        assert!(
            tracker.check_semantic_gates(&ready).is_none(),
            "coordinator bootstrap work.ready must not require prior synth terminal"
        );
    }

    #[test]
    fn expired_open_wave_surfaces_aggregate_timeout_action() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(3);
        tracker.observe_accepted(&wave);

        let mut dim = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec","findings_count":0,"findings_file":"f.json"}"#,
        );
        dim.wave_id = Some("w-1".to_string());
        tracker.observe_accepted(&dim);

        tracker.backdate_open_wave_for_test("p", "t1", "1", Duration::from_secs(301));

        let actions = tracker.drain_expired_aggregate_timeouts(Duration::from_secs(300));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].received, 1);
        assert_eq!(actions[0].expected, 3);
        assert_eq!(actions[0].wave_id, "w-1");
        assert!(
            tracker
                .drain_expired_aggregate_timeouts(Duration::from_secs(300))
                .is_empty(),
            "second drain must be idempotent"
        );
    }

    /// Step-handoff (2026-06-17-002) U5: a null `review.passed`
    /// payload is hard-rejected by `validate_event_with_hat` with
    /// `RejectWithResume` and a WAC R10 finding. The state machine
    /// never sees this event, so `synth_terminal` stays unset and
    /// any subsequent `plan.complete` stays blocked. This test
    /// pins the end-to-end hard gate at the policy boundary.
    #[test]
    fn step_handoff_u5_review_passed_null_rejected_by_policy() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.passed", None, &config, &mut state);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    finding.message.contains("WAC R10") || finding.message.contains("null payload"),
                    "review.passed null must be flagged with WAC R10 finding, got: {}",
                    finding.message
                );
            }
            other => panic!("review.passed null must RejectWithResume, got {:?}", other),
        }
    }

    /// Step-handoff U5: `observe_accepted` is a no-op when the
    /// payload is missing or unparseable (step_key_from_event
    /// returns None). So even if a null `review.passed` ever
    /// leaks past the policy gate, the state machine cannot
    /// accidentally set `synth_terminal` from it.
    #[test]
    fn step_handoff_u5_review_passed_null_does_not_set_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();

        // (1) A null-payload review.passed routed into the state
        // machine must be a no-op (no step_key, no state mutation).
        let null_passed = JsonlEvent {
            topic: "review.passed".to_string(),
            payload: None,
            ts: String::new(),
            hat: Some("review-synthesizer".to_string()),
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        tracker.observe_accepted(&null_passed);

        // (2) After the no-op, plan.complete is still blocked
        // because synth_terminal was never set.
        let plan_complete_blocked = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&plan_complete_blocked)
            .expect("plan.complete must stay blocked when synth_terminal was never set");
        assert!(
            finding.message.contains("plan_gate_review_not_terminal"),
            "expected plan_gate_review_not_terminal, got: {}",
            finding.message
        );

        // (3) A subsequent well-formed review.passed unlocks the
        // gate cleanly — the no-op did not corrupt state.
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);
        assert!(
            tracker
                .check_semantic_gates(&plan_complete_blocked)
                .is_none(),
            "synth_terminal must be set after a well-formed review.passed, \
             so plan.complete must pass the gate"
        );
    }

    /// U2 (2026-06-17-003 plan): an open wave with at least one
    /// `dimension.done` arrival but no progress past the staleness
    /// window must surface in `open_waves_needing_intervention`.
    /// The expected/received counts are unique (set-based), so the
    /// caller can detect "wave started, some progress, then stalled".
    #[test]
    fn open_waves_needing_intervention_returns_stalled_wave() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-stall".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        // Two distinct dimensions arrive.
        let mut d1 = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec","findings_count":0,"findings_file":"f.json"}"#,
        );
        d1.wave_id = Some("w-stall".to_string());
        tracker.observe_accepted(&d1);

        let mut d2 = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"perf","findings_count":0,"findings_file":"f.json"}"#,
        );
        d2.wave_id = Some("w-stall".to_string());
        tracker.observe_accepted(&d2);

        // Before staleness elapses, no intervention needed.
        let actions = tracker.open_waves_needing_intervention(60);
        assert!(
            actions.is_empty(),
            "before staleness the wave must not surface, got {actions:?}"
        );

        // Compress: pretend the last dimension arrived 600s ago.
        tracker.backdate_last_dimension_for_test("p", "t1", "1", Duration::from_secs(600));

        // Now at 60s staleness, the wave is stalled (4/11 unique).
        let actions = tracker.open_waves_needing_intervention(60);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].wave_id, "w-stall");
        assert_eq!(actions[0].expected, 11);
        assert_eq!(
            actions[0].received, 2,
            "received count must be unique (set-based)"
        );
        assert!(actions[0].last_dimension_at.is_some());
    }

    /// U2 (2026-06-17-003 plan): a wave that has **not yet** seen
    /// any dimension.done (just-started, no workers yet) must NOT
    /// surface as needing intervention — the staleness math has no
    /// anchor. The aggregate-timeout path (`drain_expired_aggregate_timeouts`)
    /// still covers it.
    #[test]
    fn open_waves_needing_intervention_skips_waves_with_no_dimensions() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-fresh".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        // No dimension.done arrives. Even with a generous
        // staleness window, the wave must not surface because
        // there is no baseline to compare against.
        let actions = tracker.open_waves_needing_intervention(0);
        assert!(
            actions.is_empty(),
            "fresh wave without dimensions must not surface, got {actions:?}"
        );
    }

    /// U2 (2026-06-17-003 plan): a wave that **closed cleanly**
    /// (received == expected) must NOT surface as needing
    /// intervention — the aggregate path is the synthesizer's
    /// job now.
    #[test]
    fn open_waves_needing_intervention_skips_closed_waves() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-closed".to_string());
        wave.wave_total = Some(2);
        tracker.observe_accepted(&wave);

        // Two distinct dimensions → received == expected → wave
        // closes (open_wave_id becomes None).
        for dim in ["sec", "perf"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-closed".to_string());
            tracker.observe_accepted(&d);
        }

        let actions = tracker.open_waves_needing_intervention(0);
        assert!(
            actions.is_empty(),
            "closed wave must not surface, got {actions:?}"
        );
    }

    // 003-U5 / R-F5: last_reviewed_sha wave-closed gate tests
    //
    // `is_wave_closed` is the query that agents and the runner use to
    // decide whether writing `last_reviewed_sha` is safe. The gate MUST
    // return `false` when a wave is open (even if `review.wave.ready`
    // was emitted) and `true` only when the wave is fully closed
    // (all dimensions received OR a verdict terminal seen).
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u5_is_wave_closed_no_tracker_entry_returns_true() {
        // No wave ever opened for this step — writing SHA is safe.
        let tracker = ReviewStepTracker::default();
        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: no tracker entry means no open wave, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_after_wave_ready_returns_false() {
        // `review.wave.ready` just emitted, no dimensions yet.
        // Writing SHA here is the DEC-002 empty_diff fuel the plan
        // explicitly forbids.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: wave just opened, SHA write must be blocked"
        );
    }

    #[test]
    fn u5_is_wave_closed_partial_dimensions_returns_false() {
        // 4/11 dimensions received, wave still open.
        // This is the zippy-sparrow stall scenario: a premature SHA
        // write would let the next pass claim empty diff.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        for dim in ["sec", "rel", "perf", "a11y"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-1".to_string());
            tracker.observe_accepted(&d);
        }

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: 4/11 dimensions received, wave open, SHA write must be blocked"
        );
    }

    #[test]
    fn u5_is_wave_closed_all_dimensions_returns_true() {
        // All 11 dimensions received — wave fully closed.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(2);
        tracker.observe_accepted(&wave);

        for dim in ["sec", "rel"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-1".to_string());
            tracker.observe_accepted(&d);
        }

        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: all dimensions received, wave closed, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_after_verdict_returns_true() {
        // Wave opened then `review.passed` verdict seen — wave closed.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(3);
        tracker.observe_accepted(&wave);

        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);

        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: verdict terminal seen, wave closed, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_different_step_isolated() {
        // Wave open for step "1" must not affect step "2" gate.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(5);
        tracker.observe_accepted(&wave);

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: step 1 wave is open"
        );
        assert!(
            tracker.is_wave_closed("p", "t1", "2"),
            "R-F5: step 2 has no wave, SHA write is safe (different step)"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-02-005 U2: plan-level review terminal
    //
    // Background: `review.complete(pass_with_residuals,
    // fix_plan_file=null)` is a PLAN-level terminal. The per-step
    // matching gate cannot see it because the `task_id` of the
    // terminalizing review is `finalize` (or similar), not the
    // task_id of any single unit step. The plan-level terminal map
    // is the authoritative source for `plan.complete` gate when no
    // per-step entry matches.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u2_plan_complete_after_review_complete_pass_with_residuals_accepted_even_with_different_task_id() {
        // 140149-shape: review.complete carries verdict=pass_with_residuals
        // and an EMPTY fix_plan_file. The plan should be marked
        // terminal at the plan level.
        let mut tracker = ReviewStepTracker::default();
        let review = jsonl(
            "review.complete",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"finalize","task_key":"kF","step":"finalize","verdict":"pass_with_residuals","final_findings_count":3,"fix_plan_file":""}"#,
        );
        tracker.observe_accepted(&review);

        // plan.complete carries a DIFFERENT task_id (no matching
        // per-step entry). With the plan-level terminal, the gate
        // must accept.
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"step-02","verdict":"pass"}"#,
        );
        assert!(
            tracker.check_semantic_gates(&plan_complete).is_none(),
            "plan-level terminal must let plan.complete pass even when per-step entry is missing"
        );
    }

    #[test]
    fn u2_plan_complete_verdict_fail_does_not_set_plan_terminal() {
        let mut tracker = ReviewStepTracker::default();
        let review = jsonl(
            "review.complete",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"step-01","verdict":"fail"}"#,
        );
        tracker.observe_accepted(&review);

        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"step-01","verdict":"pass"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&plan_complete)
            .expect("verdict=fail must NOT set plan-level terminal");
        assert!(
            finding.message.contains("plan_gate_review_not_terminal"),
            "expected plan_gate_review_not_terminal, got: {}",
            finding.message
        );
    }

    #[test]
    fn u2_plan_complete_without_any_review_observation_still_rejected() {
        let tracker = ReviewStepTracker::default();
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"step-01","verdict":"pass"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&plan_complete)
            .expect("plan.complete without any review observation must be rejected");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn u2_plan_complete_fix_unit_routes_through_fix_branch_not_plan_terminal() {
        // fix-NN plan.complete goes through the dedicated
        // `step.starts_with("fix-")` exemption, NOT the
        // plan-level terminal. Plan-level terminal is irrelevant
        // here. Verify the fix branch produces a clean accept.
        let tracker = ReviewStepTracker::default();
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"fix-02","verdict":"pass"}"#,
        );
        assert!(tracker.check_semantic_gates(&plan_complete).is_none());
    }

    #[test]
    fn u2_plan_complete_review_complete_with_fix_plan_file_does_not_set_plan_terminal() {
        // review.complete with NON-empty fix_plan_file → fix-unit
        // chain pending. Plan-level terminal MUST NOT be set;
        // otherwise fix-02 plan.complete would skip review gate.
        let mut tracker = ReviewStepTracker::default();
        let review = jsonl(
            "review.complete",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"step-01","verdict":"pass_with_residuals","fix_plan_file":"docs/plans/fix.md"}"#,
        );
        tracker.observe_accepted(&review);

        // A subsequent plan.complete with a DIFFERENT task_id (not
        // matching step-01's task_id) and no plan-level terminal
        // should be rejected (no per-step entry matches).
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"finalize","task_key":"kF","step":"step-99","verdict":"pass"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&plan_complete)
            .expect("fix_plan_file non-empty must NOT set plan terminal");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    /// 2026-06-28-002 U1 回归守卫（2026-07-01 补）：`review.complete`
    /// 携带非空 `fix_plan_file` 时，必须按 fix-plan 里的 `### U{N}.`
    /// 标题为每个 `fix-{NN}` step 预填 review 终态（synth_terminal /
    /// synth_pass / open_wave_id=None），使下游 `is_wave_closed` 对
    /// 未直接出现在事件里的 fix 单元也返回 closed。此前该路径无
    /// 测试，导致 U6 回滚误删 `prefill_fix_steps_from_plan` 时无人
    /// 察觉；本测试把行为钉死。
    #[test]
    fn prefill_fix_steps_from_plan_seeds_all_fix_units_on_review_complete() {
        let dir = tempfile::TempDir::new().unwrap();
        let fix_plan = dir.path().join("fix-plan.md");
        std::fs::write(&fix_plan, "### U1. first\n### U2. second\n### U3. third\n").unwrap();
        let fix_plan_str = fix_plan.to_str().unwrap();

        let mut tracker = ReviewStepTracker::default();
        // 一条覆盖整份 fix-plan 的 review.complete，只显式携带 fix-02。
        let payload = format!(
            r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"fix-02","verdict":"pass","fix_plan_file":"{fix_plan_str}"}}"#
        );
        let complete = jsonl("review.complete", "review-synthesizer", &payload);
        tracker.observe_accepted(&complete);

        // fix-01 / fix-03 从未在事件里直接出现，但 prefill 必须给
        // 它们建条目并置为已 review、wave 已闭合。
        for fix in ["fix-01", "fix-02", "fix-03"] {
            let key = StepKey {
                plan_name: "p".to_string(),
                task_id: "t1".to_string(),
                step: fix.to_string(),
            };
            let state = tracker
                .steps
                .get(&key)
                .unwrap_or_else(|| panic!("{fix} must be prefilled from fix-plan"));
            assert_eq!(
                state.synth_terminal.as_deref(),
                Some("review.complete"),
                "{fix}: synth_terminal must be prefilled from fix-plan"
            );
            assert!(state.synth_pass, "{fix}: synth_pass must be true");
            assert!(
                tracker.is_wave_closed("p", "t1", fix),
                "{fix}: wave must be closed after prefill"
            );
        }

        // 负控制：fix-plan 里没有的 fix-04 不得被凭空创建。
        let absent = StepKey {
            plan_name: "p".to_string(),
            task_id: "t1".to_string(),
            step: "fix-04".to_string(),
        };
        assert!(
            tracker.steps.get(&absent).is_none(),
            "fix-04 not present in fix-plan must not be prefilled"
        );
    }
}
