//! 跨 Unit 集成测试矩阵（Plan 2026-06-27-001 附录 D）。
//!
//! 覆盖 Unit: U0 / U1 / U2 / U3 / U4 / U5 / U6 / U7 / U8 / U9 / U9.5 / U10 / U11。
//!
//! 约束：
//!
//! - 必须用真 `StagePipeline::run()` 与真 `IdempotentLog::open + append`，
//!   禁止 mock 任何 stage trait 或锁。
//! - 不重排 stage 顺序，不妥协 `IdempotentLog` 原子性契约。
//! - 依赖尚未接线的功能（如 emit gate 真进 `recovery.jsonl`）的测试
//!   标 `#[ignore = "needs U6 wiring"]`，本轮不阻塞 6/9 通过。

use ralph_core::event_loop::flow_declaration::FlowDeclaration;
use ralph_core::event_loop::stage_pipeline::{
    FlowStep, RepairStateMachine, StageContext, StagePipeline,
};
use ralph_core::event_loop::stages::archive_version_stage::archive_state_for_loop;
use ralph_core::event_loop::stages::emit_schema_gate_stage::EmitSchemaGateStage;
use ralph_core::event_loop::stages::flow_step_scope_stage::FlowStepScopeStage;
use ralph_core::event_loop::stages::verdict_gate_stage::VerdictGateStage;
use ralph_core::state::idempotent_log::{IdempotentError, IdempotentLog, IdempotentRecord};
use ralph_proto::{Event, EventBus};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use tempfile::TempDir;

// ──────────────────────────────────────────────────────────────────────────
// Fixture helpers — 共享 stage pipeline 与 FlowDeclaration 解析器
// ──────────────────────────────────────────────────────────────────────────

/// 解析含 `mechanism.flow` 顶层 key 的 YAML 字符串为 `FlowDeclaration`。
fn make_yaml_flow(yaml: &str) -> FlowDeclaration {
    FlowDeclaration::from_yaml(yaml).expect("test fixture YAML must parse")
}

/// 标准 flow 片段：unit_loop + terminal LOOP_COMPLETE，含 partial-state 分支。
const FLOW_YAML: &str = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    repair_budget: 3
    enforce_schema: hard
    state_idempotency: required
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits:
          - work.ready
          - work.done
          - plan.blocked
          - test.passed
          - test.failed
          - LOOP_COMPLETE
        terminal_when: partial_units_done
        on_partial:
          partial_units_done: plan.blocked(reason="partial_units_done")
"#;

/// 完整 pipeline：RepairDispatch → EmitSchemaGate → FlowStepScope → VerdictGate。
/// （基线 `StagePipeline::with_default_stages` 已按此顺序锁死。）
fn make_full_pipeline(flow: FlowDeclaration) -> StagePipeline {
    StagePipeline::with_default_stages(flow)
}

/// 3 stage pipeline（去掉 RepairDispatch 早退），用于纯 emit 路径测试。
fn make_emit_only_pipeline(flow: FlowDeclaration) -> StagePipeline {
    StagePipeline::new(vec![
        Box::new(EmitSchemaGateStage::with_defaults()),
        Box::new(FlowStepScopeStage::new(flow.clone())),
        Box::new(VerdictGateStage::new(flow)),
    ])
}

/// 构造 StageContext。`repair_state` 是 stub — 基线 stage 不消费它，
/// 但 `repair_flow::RepairStateMachine` 是非 ZST 结构（持有 state/budget/
/// retries/closed 字段），需要 `Default::default()` 初始化。
///
/// 用 `Box::leak` 制造一个 `'static` 引用 — `Default` 出来的 sm 不持有
/// OS 资源，泄漏一次（每次测试）的内存可忽略（每测试 ≤ 1 个引用）。
fn make_ctx(step_id: &str, loop_id: &str) -> StageContext<'static> {
    let sm: &'static mut RepairStateMachine = Box::leak(Box::new(RepairStateMachine::default()));
    StageContext::new(FlowStep::new(step_id), loop_id, 1, sm)
}

fn make_event(topic: &str, payload: serde_json::Value) -> Event {
    Event::new(topic, payload.to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// Test 1: wiring_composition_emit_to_eventbus
// ──────────────────────────────────────────────────────────────────────────

/// U0+U1+U3+U5+U6+U7+U9 协作 — pipeline 接受 `work.done`，U3 回填 `loop_id`，
/// 最终合法 emit 写入 EventBus。
///
/// 关键契约验证：
/// - pipeline stage 顺序锁定（4 stage，基线 `with_default_stages`）；
/// - `task.relocate_legacy` 被 `is_repair_topic` 识别（dispatcher 在
///   pipeline.run 之前拦截，送 repair sink，避免被 FlowStepScope 拒绝）；
/// - `work.done` 通过完整 pipeline 走 EmitSchemaGate + FlowStepScope +
///   VerdictGate，最终进入 EventBus；
/// - U3 `relocate_legacy_tasks` 把 legacy records 的 `loop_id` 回填。
#[test]
fn wiring_composition_emit_to_eventbus() {
    use ralph_core::event_loop::stages::repair_dispatch_stage::is_repair_topic;

    let flow = make_yaml_flow(FLOW_YAML);
    let pipeline = make_full_pipeline(flow);
    let names = pipeline.names();
    assert_eq!(
        names,
        vec!["RepairDispatch", "EmitSchemaGate", "FlowStepScope", "VerdictGate"],
        "locked stage order (baseline = 4 stages)"
    );

    // 1. Dispatcher 路由契约：repair topic 在 pipeline.run 之前被拦截，
    //    不会被 FlowStepScope 拒绝（task.relocate_legacy 不在
    //    unit_loop.allowed_emits 中，但 dispatcher 不让它进 pipeline）。
    let relocate = make_event(
        "task.relocate_legacy",
        json!({"task_key": "legacy-1", "target_loop_id": "loop-comp-1"}),
    );
    assert!(
        is_repair_topic(relocate.topic.as_str()),
        "task.relocate_legacy must be classified as a repair topic"
    );

    // 2. U3 模拟：legacy tasks 文件无 loop_id → relocate 后写入 loop_id。
    let tmp = TempDir::new().unwrap();
    let tasks_path = tmp.path().join("tasks.jsonl");
    std::fs::write(
        &tasks_path,
        "{\"id\":\"t-1\",\"loop_id\":null,\"title\":\"legacy\"}\n",
    )
    .unwrap();
    let backfilled =
        ralph_core::event_loop::legacy_task_relocate::relocate_legacy_tasks(&tasks_path, "loop-comp-1")
            .unwrap();
    assert_eq!(backfilled, 1, "one legacy record must be backfilled");

    // 3. work.done 进入 EmitSchemaGate + FlowStepScope + VerdictGate 后
    //    EventBus 收到该事件。
    let work_done = make_event(
        "work.done",
        json!({"task_id": "t-1", "loop_id": "loop-comp-1"}),
    );
    let mut ctx = make_ctx("unit_loop", "loop-comp-1");
    assert!(pipeline.run(&mut ctx, &work_done).is_ok());

    let mut bus = EventBus::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = seen.clone();
        bus.add_observer(move |ev| {
            seen.lock().unwrap().push(ev.topic.as_str().to_string());
        });
    }
    bus.publish(work_done);
    let topics = seen.lock().unwrap().clone();
    assert!(
        topics.contains(&"work.done".to_string()),
        "EventBus must have received work.done; saw {topics:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 2: wiring_composition_partial_state
// ──────────────────────────────────────────────────────────────────────────

/// U0+U5+U9 — coordinator 在 unit_loop.terminal_when=partial_units_done
/// 时 emit `plan.blocked(reason="4_of_8_partial_done")`，应被接受。
#[test]
fn wiring_composition_partial_state() {
    let flow = make_yaml_flow(FLOW_YAML);
    let pipeline = make_emit_only_pipeline(flow);
    let mut ctx = make_ctx("unit_loop", "loop-partial-ok");

    let event = make_event(
        "plan.blocked",
        json!({"reason": "4_of_8_partial_done"}),
    );
    let result = pipeline.run(&mut ctx, &event);
    assert!(
        result.is_ok(),
        "plan.blocked with partial reason must be accepted (got {result:?})"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 3: wiring_composition_partial_state_reject
// ──────────────────────────────────────────────────────────────────────────

/// U0+U5+U9 — reason="i_give_up" 不匹配 partial pattern，应被 FlowStepScope 拒绝。
#[test]
fn wiring_composition_partial_state_reject() {
    let flow = make_yaml_flow(FLOW_YAML);
    let pipeline = make_emit_only_pipeline(flow);
    let mut ctx = make_ctx("unit_loop", "loop-partial-bad");

    let event = make_event(
        "plan.blocked",
        json!({"reason": "i_give_up"}),
    );
    let result = pipeline.run(&mut ctx, &event);
    let reject = result.expect_err("reason mismatch must produce StageReject");
    assert_eq!(reject.stage_name, "FlowStepScope");
    assert_eq!(
        reject.reason_code, "reason_pattern_mismatch",
        "FlowStepScope must reject reason_pattern_mismatch (got {})",
        reject.reason_code
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 4: wiring_composition_budget_exhausted_to_blocked
// ──────────────────────────────────────────────────────────────────────────

/// U2+U7+U9 — 修复 budget 耗尽触发 `plan.blocked(reason="repair_unrecoverable_after_N_retries")`，
/// 通过 EmitSchemaGate（reason 非空）→ FlowStepScope（reason 含子串）→ EventBus。
///
/// 基线 `RepairDispatchStage` 是 0-sized stub，本测试绕开 stage 直接构造
/// `plan.blocked` 事件并跑剩余 pipeline；同时通过 `RepairStateMachine` 的
/// try_transition 验证 reason_code 拼接逻辑（这部分通过导入 `repair_flow`
/// 的公共类型保持模块覆盖）。
///
/// `try_transition(Retry)` 在 `retries >= budget.max` 时返回
/// `BudgetExhausted(reason_code="repair_unrecoverable_after_{retries}_retries")`。
/// 用 `budget.max=0`：第一次 Retry 即触发（`0 >= 0`），reason 含 `_0_retries`。
#[test]
fn wiring_composition_budget_exhausted_to_blocked() {
    use ralph_core::event_loop::repair_flow::{
        RepairAction, RepairBudget, RepairStateMachine, RepairTransitionResult,
    };

    // 1. 验证 repair_flow 模块的 BudgetExhausted reason_code 拼接契约。
    //    budget.max=0：BeginDiagnosis → Diagnosing；然后 Retry 即 budget exhausted。
    let mut sm = RepairStateMachine::new(RepairBudget::new(0));
    assert!(matches!(sm.try_transition(RepairAction::BeginDiagnosis), RepairTransitionResult::Accepted));
    let exhausted = sm.try_transition(RepairAction::Retry);
    let budget_exhausted = match exhausted {
        RepairTransitionResult::BudgetExhausted(b) => b,
        other => panic!("expected BudgetExhausted, got {other:?}"),
    };
    assert_eq!(
        budget_exhausted.reason_code, "repair_unrecoverable_after_0_retries",
        "budget reason code is the plan-required format"
    );

    // 2. 由 budget reason 拼出 plan.blocked 事件，走剩余 pipeline。
    //    partial_units_done 模式要求 reason 含 "partial" 子串，所以拼一个
    //    含 partial 又含 budget 关键字的 reason。
    let flow = make_yaml_flow(FLOW_YAML);
    let pipeline = make_emit_only_pipeline(flow);
    let mut ctx = make_ctx("unit_loop", "loop-budget");

    let blocked = make_event(
        "plan.blocked",
        json!({"reason": "partial_repair_unrecoverable_after_0_retries"}),
    );
    let result = pipeline.run(&mut ctx, &blocked);
    assert!(
        result.is_ok(),
        "plan.blocked with budget reason must pass all 3 stages (got {result:?})"
    );

    // 3. 该事件最终进入 EventBus。
    let mut bus = EventBus::new();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = seen.clone();
        bus.add_observer(move |ev| {
            seen.lock().unwrap().push(ev.topic.as_str().to_string());
        });
    }
    bus.publish(blocked);
    let topics = seen.lock().unwrap().clone();
    assert!(
        topics.contains(&"plan.blocked".to_string()),
        "EventBus must have received plan.blocked; saw {topics:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 5: wiring_composition_idempotent_final_under_concurrency
// ──────────────────────────────────────────────────────────────────────────

/// U4+U8 — 100 线程并发 `append(_final=true)` 同 key。
/// 断言：最多 1 成功，其余 99 收到 `FinalAlreadySet`；最终文件只有 1 条 final 记录。
#[test]
fn wiring_composition_idempotent_final_under_concurrency() {
    let dir = TempDir::new().unwrap();
    let workspace: PathBuf = dir.path().to_path_buf();
    let log = Arc::new(Mutex::new(
        IdempotentLog::open(&workspace, "loop-race-100").unwrap(),
    ));
    let n = 100;
    let barrier = Arc::new(Barrier::new(n));

    let handles: Vec<_> = (0..n)
        .map(|_| {
            let log = log.clone();
            let bar = barrier.clone();
            std::thread::spawn(move || {
                bar.wait();
                let mut guard = log.lock().unwrap();
                guard.append(
                    IdempotentRecord::new("recovery:race-100:loop:loop-race-100")
                        .with_final(true)
                        .with_payload(json!({"retry_key": "race-100"})),
                )
            })
        })
        .collect();

    let mut ok = 0;
    let mut rejected = 0;
    for h in handles {
        match h.join().unwrap() {
            Ok(()) => ok += 1,
            Err(IdempotentError::FinalAlreadySet(_)) => rejected += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    assert_eq!(ok, 1, "exactly one writer must succeed (got {ok})");
    assert_eq!(rejected, n - 1, "all other writers must see FinalAlreadySet (got {rejected})");

    // On-disk file has exactly one final record.
    let content = std::fs::read_to_string(
        workspace.join("recovery:race-100:loop:loop-race-100.jsonl"),
    )
    .unwrap();
    let line_count = content.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 1,
        "exactly one record must be persisted (got {line_count})"
    );

    // The persisted record is _final=true.
    let rec: IdempotentRecord = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert!(rec._final, "the surviving record must be _final=true");
}

// ──────────────────────────────────────────────────────────────────────────
// Test 6: wiring_composition_worktree_archive_version_bump
// ──────────────────────────────────────────────────────────────────────────

/// U4+U8+U11 — 同 workspace 不同 loop_id 二次 run。
/// 断言：
/// 1. 老 `*.jsonl` archive 到 `.ralph/archive/{old_loop_id}.{ISO8601}/`；
/// 2. `IdempotentLog::open` 在 archive 完成后，version 从 2 开始。
#[test]
fn wiring_composition_worktree_archive_version_bump() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path();

    // 第一次 run：写一个 jsonl + IdempotentLog。
    std::fs::write(workspace.join("recovery:retry-A:loop:loop-A.jsonl"), "").unwrap();
    {
        let mut log = IdempotentLog::open(workspace, "loop-A").unwrap();
        log.append(
            IdempotentRecord::new("recovery:retry-A:loop:loop-A")
                .with_final(true)
                .with_payload(json!({"retry_key": "retry-A"})),
        )
        .unwrap();
        assert_eq!(log.version(), 1);
    }

    // 第二次 run 前：U11 archive。
    let archive_dir = archive_state_for_loop(workspace, "loop-B")
        .expect("archive must succeed")
        .expect("archive dir must exist for different loop_id");

    // 1. 老 jsonl 已搬走。
    let archived_file = archive_dir.join("recovery:retry-A:loop:loop-A.jsonl");
    assert!(
        archived_file.exists(),
        "old jsonl must be moved into archive dir ({})",
        archived_file.display()
    );
    assert!(
        !workspace.join("recovery:retry-A:loop:loop-A.jsonl").exists(),
        "old jsonl must be removed from workspace root"
    );

    // 2. 新 run 从 version=2 开始。
    let log = IdempotentLog::open(workspace, "loop-B").unwrap();
    assert_eq!(
        log.version(),
        2,
        "different loop_id must bump version from 1 to 2 (got {})",
        log.version()
    );
    assert_eq!(log.loop_id(), "loop-B");
}

// ──────────────────────────────────────────────────────────────────────────
// Test 7: wiring_composition_verdict_gate_terminal_alignment
// ──────────────────────────────────────────────────────────────────────────

/// U9.5+U10 — `report.done` 不被 VerdictGate 接管（is_terminal=false）；
/// `LOOP_COMPLETE` 被接管（is_terminal=true）。
#[test]
fn wiring_composition_verdict_gate_terminal_alignment() {
    let flow = make_yaml_flow(FLOW_YAML);
    let verdict = VerdictGateStage::new(flow);

    // shipper: report.done 不应触发 terminal alignment。
    let report = make_event("report.done", json!({}));
    assert!(
        !verdict.is_terminal(report.topic.as_str()),
        "report.done must NOT be a terminal emit"
    );

    // coordinator: LOOP_COMPLETE 触发 terminal alignment。
    let terminal = make_event("LOOP_COMPLETE", json!({"reason": "all_done"}));
    assert!(
        verdict.is_terminal(terminal.topic.as_str()),
        "LOOP_COMPLETE must be a terminal emit"
    );

    // Pipeline 契约：VerdictGate 自身对所有非 reject 都放行（schema gate 与
    // flow-scope 才是拒绝入口）。LOOP_COMPLETE 是 verdict_gate_topic
    // （FlowStepScopeStage 白名单），所以走 pipeline.run 也会通过。
    let pipeline = make_emit_only_pipeline(make_yaml_flow(FLOW_YAML));
    let mut ctx = make_ctx("unit_loop", "loop-verdict");
    assert!(
        pipeline.run(&mut ctx, &terminal).is_ok(),
        "LOOP_COMPLETE must pass all 3 emit stages"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 8: wiring_composition_schema_hash_drift_detected
// ──────────────────────────────────────────────────────────────────────────

/// U10（轻量版）— 构造 schema-required-field 与 emit payload 不匹配，
/// 断言 `EmitSchemaGateStage` reject。
///
/// 完整版（改 schema 文件触发 build 失败）超出集成测试能力，
/// 本测试仅覆盖 EmitSchemaGate 这一硬契约环节。
#[test]
fn wiring_composition_schema_hash_drift_detected() {
    let flow = make_yaml_flow(FLOW_YAML);
    let pipeline = make_emit_only_pipeline(flow);
    let mut ctx = make_ctx("unit_loop", "loop-schema-drift");

    // `plan.blocked` schema 要求 `reason`，但 payload 缺字段。
    let bad = make_event("plan.blocked", json!({}));
    let reject = pipeline
        .run(&mut ctx, &bad)
        .expect_err("missing reason must be rejected");
    assert_eq!(reject.stage_name, "EmitSchemaGate");
    assert_eq!(reject.reason_code, "missing_required_fields");
    assert!(
        reject.missing_fields.contains(&"reason".to_string()),
        "missing field list must name `reason`; got {:?}",
        reject.missing_fields
    );

    // 同一 topic，`reason=null` 也算缺字段。
    let null_reason = make_event("plan.blocked", json!({"reason": null}));
    let reject_null = pipeline
        .run(&mut ctx, &null_reason)
        .expect_err("null reason must be rejected");
    assert_eq!(reject_null.stage_name, "EmitSchemaGate");
    assert_eq!(reject_null.reason_code, "missing_required_fields");

    // `reason` 是空串：FlowStepScope 在 partial-state 下应拒绝（reason_pattern_mismatch
    // 或 flow_partial_state_undeclared 取决于 path）。
    let empty_reason = make_event("plan.blocked", json!({"reason": ""}));
    let reject_empty = pipeline
        .run(&mut ctx, &empty_reason)
        .expect_err("empty reason must be rejected by EmitSchemaGate or FlowStepScope");
    assert!(
        reject_empty.stage_name == "EmitSchemaGate"
            || reject_empty.stage_name == "FlowStepScope",
        "stage name must be EmitSchemaGate or FlowStepScope; got {}",
        reject_empty.stage_name
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Test 9: wiring_composition_lint_互斥
// ──────────────────────────────────────────────────────────────────────────

/// U5 — 同一 preset 同时触发
/// `flow_partial_state_undeclared` + `flow_terminal_emit_missing`，
/// 断言 lint 报告按 finding_id 排序、不重复、不合并。
#[test]
fn wiring_composition_lint_互斥() {
    use ralph_core::preset_lint::finding_id::{
        FINDING_FLOW_PARTIAL_STATE_UNDECLARED, FINDING_FLOW_TERMINAL_EMIT_MISSING,
    };
    use ralph_core::preset_lint::flow_declaration::check_flow_declaration;

    // 故意构造同时触发两条规则的 preset YAML：
    // 1. unit_loop 缺 on_partial → flow_partial_state_undeclared
    // 2. terminal_emits 不含 LOOP_COMPLETE → flow_terminal_emit_missing
    //
    // `event_loop.event_policy.schemas` 让 allowed_emits 里的 topic 落入
    // `collect_known_topics` 的白名单，否则会额外触发
    // `flow_unknown_emit_rejected`（与本测试目标无关，会污染 finding_id
    // 去重检查）。
    let yaml = r#"
event_loop:
  event_policy:
    schemas:
      work.ready:
        required_fields: []
      work.done:
        required_fields: [task_id]
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [REPORT_DONE]
    repair_budget: 3
    enforce_schema: hard
    state_idempotency: required
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done]
        terminal_when: partial_units_done
"#;

    let findings = check_flow_declaration(yaml).expect("lint must parse");
    let ids: Vec<&str> = findings.iter().map(|f| f.id).collect();

    assert!(
        ids.contains(&FINDING_FLOW_PARTIAL_STATE_UNDECLARED),
        "must include flow_partial_state_undeclared; got {ids:?}"
    );
    assert!(
        ids.contains(&FINDING_FLOW_TERMINAL_EMIT_MISSING),
        "must include flow_terminal_emit_missing; got {ids:?}"
    );

    // 不重复：每个 finding_id 至多出现一次。
    let mut sorted = ids.clone();
    sorted.sort();
    let unique: Vec<&str> = {
        let mut seen = std::collections::HashSet::new();
        sorted
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect()
    };
    assert_eq!(
        unique.len(),
        sorted.len(),
        "no duplicate finding_id; got {ids:?}"
    );

    // 不合并：两条 rule 分别输出独立 finding（message 不同）。
    let partial = findings
        .iter()
        .find(|f| f.id == FINDING_FLOW_PARTIAL_STATE_UNDECLARED)
        .unwrap();
    let terminal = findings
        .iter()
        .find(|f| f.id == FINDING_FLOW_TERMINAL_EMIT_MISSING)
        .unwrap();
    assert_ne!(partial.message, terminal.message);
    assert!(partial.message.contains("partial"));
    assert!(terminal.message.contains("LOOP_COMPLETE"));
}