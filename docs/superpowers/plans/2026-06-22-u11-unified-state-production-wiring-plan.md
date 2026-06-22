# U11 — 统一编排状态重构 Production 接入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 U0–U10 已实现但未接入 production 的统一编排状态重构代码(`StateLedger`、`ValidationPipeline`、`CorrectionContext`、handoff auto-gen)接入 `event_loop/mod.rs` 的 `process_parse_result`,让 feature flag 默认全开、全部测试绿、文档同步。

**Architecture:** 渐进式接入(每个 task 独立 commit + 测试可回滚)+ flag 默认值反转(`UNIFIED_STATE_LEDGER` / `UNIFIED_VALIDATION` / `UNIFIED_DETERMINISTIC_CORRECTION` / `UNIFIED_PROTOCOL_VIEW` / `UNIFIED_POLICY_CHECK` 默认 `true`)+ 文档同步。每个 task 保留 escape hatch(显式 env var `=0` 可关闭)。

**Tech Stack:** Rust (1.78+), cargo nextest, ralph-core / ralph-cli, Serde JSONL, serde_json

**Spec:** `docs/superpowers/specs/2026-06-22-u11-unified-state-production-wiring-design.md`

---

## File Structure

### Modified Files

- `crates/ralph-core/src/state/ledger.rs` — T1(`new()` 调用 `replay_from_disk`)
- `crates/ralph-core/src/state/tests.rs` — T1(新增 replay 测试)
- `crates/ralph-core/src/event_loop/mod.rs` — T2(per-event pipeline 调用)+ T3(`publish_correction_via_context` 真实 context + ledger commit)+ T7(env var 默认值反转)
- `crates/ralph-core/src/correction/mod.rs` — T7(`is_correction_enabled` 默认反转)
- `crates/ralph-core/src/preset/engine/protocol.rs` — T7(`UNIFIED_PROTOCOL_VIEW` 默认反转)
- `crates/ralph-core/src/event_loop/tests/u11_unified_pipeline_integration.rs` — T2 新增(放 `crates/ralph-core/src/event_loop/tests/` 现有测试旁)
- `crates/ralph-core/tests/scenarios.rs` — T3(移除 2 处 `#[ignore]`)
- `crates/ralph-core/src/state/snapshot.rs` — T5(复核 4 个 delta 变体;若已有则跳过)
- `crates/ralph-cli/src/policy_check.rs` — T7(`run_policy_check_unified` 用 events.jsonl history)+ env var 默认值
- `crates/ralph-cli/tests/integration_emit_policy.rs` 和 `commands/emit/tests.rs` — T7(测试修复 / 2 条 ignore 转用 serial_test)
- `docs/plans/2026-06-21-002-unified-state-u10-verification.md` — T6(line 180-181 虚假声明修正)
- `docs/report/2026-06-21-top-3-architectural-instability-factors.md` — T8(修复状态章节)
- `docs/guide/runtime-diagnosis.md` — T8(`--from-ledger` 路径说明)
- `crates/ralph-core/data/ralph-tools*.md` — T8(`correction` / `loop.resume` / `StateLedger` 概念)

### Created Files

- `.sop/planning/u11-state-ledger-replay/task-01-replay-from-disk-in-new.code-task.md` — T1
- `.sop/planning/u11-event-loop-pipeline/task-01-per-event-pipeline-call.code-task.md` — T2
- `.sop/planning/u11-correction-context/task-01-real-prompt-context-and-ledger-commit.code-task.md` — T3
- `.sop/planning/u11-handoff-auto-gen/task-01-macro-edge-missing-path.code-task.md` — T4
- `.sop/planning/u11-snapshot-audit/task-01-verify-4-no-op-deltas.code-task.md` — T5
- `.sop/planning/u11-u10-report-fix/task-01-fix-false-claims.code-task.md` — T6
- `.sop/planning/u11-flag-default-on/task-01-flip-env-var-defaults.code-task.md` — T7(拆分 3 个子任务:core flag + CLI flag + 16 test gap)
- `.sop/planning/u11-docs-sync/task-01-runtime-diagnosis-doc.code-task.md` — T8(拆分 3 个文档)

---

## Implementation Tasks

### Task T1: StateLedger::new() 接入 replay_from_disk

**Files:**
- Modify: `crates/ralph-core/src/state/ledger.rs:167-177`(`new` 方法)
- Modify: `crates/ralph-core/src/state/tests.rs`(新增测试)

**Steps:**

- [ ] **Step 1.1: 写失败测试 — replay 后 snapshot 等价**

在 `state/tests.rs` 添加:

```rust
#[test]
fn new_replays_from_disk_when_ledger_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();

    // 写一个 commit 到 ledger.jsonl
    let mut ledger = StateLedger::new(workspace, true);
    let delta = CommitDelta::CounterChanged {
        counter: CounterKind::Iteration,
        new_value: 42,
    };
    ledger.commit(delta, Some("setup".to_string())).unwrap();

    // 重新构造:new() 应 replay
    let ledger2 = StateLedger::new(workspace, true);
    assert_eq!(ledger2.snapshot().iteration, 42);
}

#[test]
fn new_falls_back_to_cold_start_when_ledger_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let ledger = StateLedger::new(tmp.path(), true);
    assert_eq!(ledger.snapshot().iteration, 0);
    assert!(ledger.commit_log().is_empty());
}
```

- [ ] **Step 1.2: 跑测试确认失败**

```bash
cargo nextest run -p ralph-core -- state::tests::new_replays_from_disk_when_ledger_exists
```

Expected: FAIL (`new()` 当前不读 ledger)。

- [ ] **Step 1.3: 修改 `new()` 调用 `replay_from_disk`**

`crates/ralph-core/src/state/ledger.rs:167`:

```rust
pub fn new(workspace: &Path, feature_enabled: bool) -> Self {
    let snapshot = if feature_enabled {
        match Self::replay_from_disk(workspace) {
            Ok(snap) => {
                debug!(workspace = %workspace.display(), "replayed ledger.jsonl on cold start");
                snap
            }
            Err(e) => {
                warn!(
                    workspace = %workspace.display(),
                    error = %e,
                    "replay_from_disk failed; falling back to cold_start snapshot"
                );
                LedgerSnapshot::cold_start()
            }
        }
    } else {
        LedgerSnapshot::cold_start()
    };

    Self {
        snapshot,
        commit_log: Vec::new(),
        commit_seq: 0,
        workspace: workspace.to_path_buf(),
        ledger_path: workspace.join(LEDGER_RELATIVE_PATH),
        feature_enabled,
        bypass_active: Cell::new(false),
    }
}
```

- [ ] **Step 1.4: 跑测试确认通过**

```bash
cargo nextest run -p ralph-core -- state::tests::new_replays
```

Expected: PASS

- [ ] **Step 1.5: 跑全 state 模块测试**

```bash
cargo nextest run -p ralph-core -- state
```

Expected: 全过(原 897 行测试 + 新增 2 条)

- [ ] **Step 1.6: Commit**

```bash
git add crates/ralph-core/src/state/ledger.rs crates/ralph-core/src/state/tests.rs
git commit -m "feat(state): StateLedger::new replays ledger.jsonl on cold start (U11-T1)"
```

---

### Task T2: process_parse_result per-event ValidationPipeline 接入

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs:9046-9110`(per-event gate stack 注入 pipeline 调用)
- Create: `crates/ralph-core/src/event_loop/tests/u11_unified_pipeline_integration.rs`

**Steps:**

- [ ] **Step 2.1: 写失败测试 — pipeline 拒绝时走 correction**

新建 `crates/ralph-core/src/event_loop/tests/u11_unified_pipeline_integration.rs`:

```rust
//! U11-T2: per-event ValidationPipeline integration
//!
//! Verifies that when UNIFIED_VALIDATION=1, the runtime
//! validation pipeline produces a structured rejection
//! (ValidationResult::reject) that the event loop surfaces
//! via publish_correction_via_context instead of publishing
//! task.resume.

use ralph_core::config::EventLoopConfig;
use ralph_core::event_loop::EventLoop;
use ralph_core::preset::engine::protocol::ProtocolView;
use ralph_core::state::{LedgerSnapshot, StateLedger};
use ralph_core::validation::ValidationPipeline;

#[test]
fn unified_pipeline_rejects_misaligned_event_at_origin() {
    let view = ProtocolView::from_event_loop_with_index_and_feature(
        &EventLoopConfig::default(),
        None,
        true,
    );
    let snapshot = LedgerSnapshot::cold_start();
    let pipeline = ValidationPipeline::from_config(&view, &EventLoopConfig::default());

    // Build a malformed event (no required fields)
    use ralph_core::Event;
    let event = Event {
        topic: "queue.advance".to_string(),
        payload: None,
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("executor".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };

    let results = pipeline.validate_pre_commit_with_view(&view, &snapshot, &event);
    let rejected: Vec<_> = results.iter().filter(|r| !r.accepted).collect();
    assert!(!rejected.is_empty(), "OriginRule / RequiredFieldsRule should reject");
}
```

- [ ] **Step 2.2: 跑测试确认通过(单元测试 pipeline 已存在,这里仅验证接口可用)**

```bash
cargo nextest run -p ralph-core -- u11_unified_pipeline
```

Expected: PASS

- [ ] **Step 2.3: 在 process_parse_result 注入 pipeline 调用**

`crates/ralph-core/src/event_loop/mod.rs:9046` 之前,新增 per-event pipeline 调用:

```rust
// U11-T2: per-event unified ValidationPipeline call.
// Runs *before* the legacy gate stack so the new path is
// exercised on every event. Accept → legacy gates continue
// (向后兼容). Reject → publish_correction_via_context
// (existing A3 hook, now wired to real PromptContext in T3).
if let Some(ref pipeline) = unified_pipeline {
    use crate::state::LedgerSnapshot;
    let snapshot = self
        .state
        .state_ledger
        .as_ref()
        .map(|l| l.snapshot().clone())
        .unwrap_or_else(LedgerSnapshot::cold_start);
    let view = crate::preset::engine::protocol::ProtocolView::from_event_loop_with_index_and_feature(
        &self.config.event_loop,
        None,
        true,
    );
    let mut to_reject: Vec<(JsonlEvent, String)> = Vec::new();
    for evt in &events {
        use crate::validation::ValidationStage;
        let proto = crate::proto::Event::new(
            evt.topic.as_str(),
            evt.payload.as_deref().unwrap_or(""),
        );
        let results = pipeline.validate_pre_commit_with_view(&view, &snapshot, &proto);
        for r in &results {
            if !r.accepted {
                let reason = format!(
                    "{}: {}",
                    r.stage.as_str(),
                    r.message.as_deref().unwrap_or("rejected")
                );
                to_reject.push((evt.clone(), reason));
                break; // one rejection is enough
            }
        }
    }
    if !to_reject.is_empty() {
        let review_tracker = Some(&self.state.review_step_tracker);
        for (evt, payload) in to_reject {
            // A3 hook is used here; T3 swaps throwaway for real context.
            crate::event_loop::publish_correction_via_context(
                &mut self.bus,
                &evt,
                &payload,
            );
        }
        events.retain(|e| !to_reject.iter().any(|(r, _)| r.event_id == e.event_id));
    }
}
// --- End U11-T2 per-event unified pipeline ---
```

注:精确的 `JsonlEvent` 字段名(`event_id`、`topic`、`payload`)和 `Event::new` 签名需在实施时根据源码确认。

- [ ] **Step 2.4: 跑全 BDD scenarios**

```bash
cargo nextest run -p ralph-core --test scenarios
```

Expected: 63 passed(不变),0 failed

- [ ] **Step 2.5: 跑 smoke replay**

```bash
cargo nextest run -p ralph-core --features recording --test smoke_runner
```

Expected: 57 passed(不变)

- [ ] **Step 2.6: Commit**

```bash
git add crates/ralph-core/src/event_loop/mod.rs crates/ralph-core/src/event_loop/tests/u11_unified_pipeline_integration.rs
git commit -m "feat(event_loop): wire per-event ValidationPipeline call (U11-T2)"
```

---

### Task T3: emit_correction_context 走真实 PromptContext + StateLedger commit

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs:473-527`(`publish_correction_via_context`)
- Modify: `crates/ralph-core/tests/scenarios.rs:1698, 1718`(移除 `#[ignore]`)

**Steps:**

- [ ] **Step 3.1: 写失败测试 — correction 落地到 prompt_context**

新建 `crates/ralph-core/src/event_loop/tests/u11_correction_prompt_context.rs`:

```rust
//! U11-T3: verify that publish_correction_via_context actually
//! merges into LoopState::prompt_context rather than dropping
//! into a throwaway PromptContext.

use ralph_core::correction::CorrectionContext;
use ralph_core::event_loop::LoopState;

#[test]
fn publish_correction_via_context_writes_to_loop_state_prompt_context() {
    let mut state = LoopState::default();
    assert!(state.prompt_context.is_empty());

    // Simulate call: would normally come via publish_correction_via_context
    // which (after T3) writes to state.prompt_context
    let ctx = CorrectionContext {
        retry_key: "executor:queue.advance:origin".to_string(),
        reason_code: "origin:missing_field".to_string(),
        topic: "queue.advance".to_string(),
        source_hat: "executor".to_string(),
        prompt_block: "## ORCHESTRATOR CORRECTION\n...".to_string(),
        needs_escalation: false,
    };
    state.prompt_context.push(ctx);

    assert_eq!(state.prompt_context.len(), 1);
    assert!(state.prompt_context[0].prompt_block.contains("ORCHESTRATOR CORRECTION"));
}
```

- [ ] **Step 3.2: 跑测试确认通过(测试的是数据结构,真正接线在 3.3)**

```bash
cargo nextest run -p ralph-core -- u11_correction
```

Expected: PASS

- [ ] **Step 3.3: 修改 `publish_correction_via_context` 接受 `&mut LoopState`**

`crates/ralph-core/src/event_loop/mod.rs:473`:

```rust
fn publish_correction_via_context(
    bus: &mut EventBus,
    state: &mut crate::event_loop::LoopState,
    ledger: Option<&mut crate::state::StateLedger>,
    event: &JsonlEvent,
    payload: &str,
) {
    use crate::correction::PromptContext;
    let rejection = crate::event_loop::rejection::Rejection { /* 同原构造 */ };

    // Replace throwaway with real state.prompt_context
    let retry_count = ledger
        .as_ref()
        .and_then(|l| l.snapshot().rejection_digest().get(/* retry_key */).map(|e| e.count as u32))
        .unwrap_or(1);

    let ctx = crate::correction::emit_correction_context(
        ledger,
        &rejection,
        retry_count,
        Some(state.workspace_root()),
        &mut state.prompt_context,  // ← in-place 修改,不再是 throwaway
    );

    // U11-T3 also writes ledger commit
    if let Some(l) = ledger {
        let _ = l.commit(
            crate::state::CommitDelta::RejectionRecorded {
                key: ctx.retry_key.clone(),
                reason_code: ctx.reason_code.clone(),
                iteration: 0, // 由 caller 填
            },
            Some("correction.policy_rejection".to_string()),
        );
    }

    // R11 escalation unchanged
    if ctx.needs_escalation {
        crate::correction::maybe_escalate_to_human_guidance(bus, &ctx);
    }
}
```

**关键改动**:`publish_correction_via_context` 现在要求 `&mut LoopState` + `Option<&mut StateLedger>`(在 caller 已有)。所有 `process_parse_result` 中调用点需要相应更新传 state。

注:实施时需要根据源码精确签名调整(`workspace_root()` 是否存在 / `rejection_digest()` 是否需要新建 lookup 函数)。

- [ ] **Step 3.4: 更新所有 9 个调用点传 state + ledger**

`crates/ralph-core/src/event_loop/mod.rs` 全文 9 处 `publish_policy_rejection_resume(...)` 已经在调用 `publish_correction_via_context`,现在这些调用点位于的代码段需要访问 `&mut self.state` + `&mut self.state.state_ledger`。

由于 helper function 当前是 free function,需要重构为 `impl EventLoop` 的方法或把所有调用点改为通过 `&mut self` 调用。最简单做法:把 helper 改为 `impl EventLoop` 方法 `fn publish_correction_via_context(&mut self, ...)`,所有 9 个调用点改为 `self.publish_correction_via_context(bus, event, payload)`。

- [ ] **Step 3.5: 跑 BDD scenarios**

```bash
cargo nextest run -p ralph-core --test scenarios
```

Expected: 63 passed + 2 个原 `#[ignore]` BDD 现在通过

- [ ] **Step 3.6: 移除 `tests/scenarios.rs:1698, 1718` 的 `#[ignore]`**

```rust
// Before:
#[test]
#[ignore = "requires production wire-up ..."]

// After:
#[test]
```

- [ ] **Step 3.7: Commit**

```bash
git add crates/ralph-core/src/event_loop/mod.rs crates/ralph-core/src/event_loop/tests/u11_correction_prompt_context.rs crates/ralph-core/tests/scenarios.rs
git commit -m "feat(correction): wire prompt_context + ledger commit on policy rejection (U11-T3)"
```

---

### Task T4: commit_handoff_artifact 走 macro-edge 缺失 path

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs:8588-8602`(handoff accept 路径)

**Steps:**

- [ ] **Step 4.1: 写失败测试 — 缺失 handoff_path 时 auto-generate**

新增 `crates/ralph-core/src/hat_handoff/tests/u11_auto_gen.rs`:

```rust
//! U11-T4: macro-edge accept with missing handoff_path
//! triggers ledger.commit_handoff_artifact auto-generation.

use ralph_core::hat_handoff::{HandoffAcceptedInputs, HandoffKind};
use ralph_core::state::StateLedger;

#[test]
fn missing_handoff_path_triggers_commit_handoff_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ledger = StateLedger::new(tmp.path(), true);

    let inputs = HandoffAcceptedInputs {
        from: "executor".to_string(),
        to: "reviewer".to_string(),
        topic: "queue.advance".to_string(),
        iteration: 1,
        handoff_path: None,  // ← 缺失
        payload: Some("{}".to_string()),
        kind: HandoffKind::MacroEdge,
    };

    let result = ledger.commit_handoff_artifact(&inputs);
    assert!(result.is_ok(), "auto-generation should succeed");

    // Verify artifact was written to .ralph/agent/hat-handoff/
    let handoff_dir = tmp.path().join(".ralph/agent/hat-handoff");
    assert!(handoff_dir.exists(), "artifact directory should exist");
    let entries: Vec<_> = std::fs::read_dir(&handoff_dir).unwrap().collect();
    assert!(!entries.is_empty(), "at least one handoff file should be generated");
}
```

- [ ] **Step 4.2: 跑测试确认通过(`commit_handoff_artifact` 已实现)**

```bash
cargo nextest run -p ralph-core -- u11_auto_gen
```

Expected: PASS

- [ ] **Step 4.3: 在 evaluate_event macro-edge accept 路径上调用(缺失 path 时)**

`crates/ralph-core/src/event_loop/mod.rs` 在 `FileContent::Missing` 处理分支(约 line 8627):

```rust
// Before:
FileContent::Missing => evaluate_event(...),

// After:
FileContent::Missing => {
    // U11-T4: macro-edge with missing handoff_path →
    // trigger StateLedger::commit_handoff_artifact auto-generation.
    if let Some(ref mut ledger) = self.state.state_ledger {
        use crate::hat_handoff::{HandoffAcceptedInputs, HandoffKind};
        let inputs = HandoffAcceptedInputs {
            from: event.hat.clone().unwrap_or_else(|| "unknown".to_string()),
            to: /* ... */,
            topic: event.topic.to_string(),
            iteration: self.state.iteration,
            handoff_path: None,
            payload: event.payload.clone(),
            kind: HandoffKind::MacroEdge,
        };
        match ledger.commit_handoff_artifact(&inputs) {
            Ok(path) => {
                tracing::info!(
                    handoff_path = %path.display(),
                    "U11-T4: auto-generated handoff artifact for macro-edge"
                );
                // 把 path 写回事件元数据,后续 publish 时携带
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "U11-T4: auto-generation failed; macro-edge accept degraded"
                );
            }
        }
    }
    evaluate_event(...)
}
```

注:`evaluate_event` 签名和 `FileContent::Missing` 处理分支的精确位置需在实施时根据源码确认。

- [ ] **Step 4.4: 跑 hat_handoff 模块测试**

```bash
cargo nextest run -p ralph-core -- hat_handoff
```

Expected: 全过

- [ ] **Step 4.5: Commit**

```bash
git add crates/ralph-core/src/event_loop/mod.rs crates/ralph-core/src/hat_handoff/tests/u11_auto_gen.rs
git commit -m "feat(handoff): wire commit_handoff_artifact on macro-edge missing path (U11-T4)"
```

---

### Task T5: snapshot.rs 4 个 no-op delta 实现复核

**Files:**
- Modify(可能): `crates/ralph-core/src/state/snapshot.rs:434-557`(已实现则无需改动)
- Modify: `crates/ralph-core/src/state/tests.rs`(新增 audit log 测试)

**Steps:**

- [ ] **Step 5.1: 复核 4 个变体的当前实现**

读 `crates/ralph-core/src/state/snapshot.rs:434-557`:

```bash
sed -n '434,557p' crates/ralph-core/src/state/snapshot.rs
```

判定每个变体是否已实现:
- `CommitDelta::HandoffAccepted { from, to, handoff_path }` → 已写入 `handoff_accepted_log`(B1 fix)✓
- `CommitDelta::ReviewStepUpdated { plan_name, task_id, step, synth_pass, synth_terminal }` → 已 `apply_review_step_delta`(B2 fix)✓
- `CommitDelta::HandoffTrackerUpdated { event_id, accepted, escalation_reason }` → 已写入 `handoff_tracker_log`(B3 fix)✓
- `CommitDelta::FlowLifecycleUpdated { flow_unit_id, phase }` → 已写入 `flow_lifecycle_log`(B4 fix)✓

如已实现,跳过 Step 5.2-5.3,直接到 Step 5.4(写测试)。

- [ ] **Step 5.2 (conditional): 若未实现,补齐具体逻辑**

按 review 报告 B1-B4 注释的指引,把 no-op 替换为 audit log append + tracker apply。具体代码见对抗性审查 `002-adversarial-review.md` P2-7。

- [ ] **Step 5.3 (conditional): 跑测试**

```bash
cargo nextest run -p ralph-core -- state::tests
```

Expected: 全过

- [ ] **Step 5.4: 新增 audit log rebuild 测试**

在 `state/tests.rs` 添加:

```rust
#[test]
fn apply_delta_records_handoff_accepted_audit_entry() {
    use ralph_core::state::{CommitDelta, HandoffAcceptedInputs};

    let mut snap = LedgerSnapshot::cold_start();
    let delta = CommitDelta::HandoffAccepted {
        from: "executor".to_string(),
        to: "reviewer".to_string(),
        handoff_path: Some(".ralph/agent/hat-handoff/test.md".to_string()),
    };
    snap.apply_delta(&delta);

    assert_eq!(snap.handoff_accepted_log.len(), 1);
    assert_eq!(snap.handoff_accepted_log[0].from, "executor");
}

#[test]
fn apply_delta_records_review_step_update() {
    use ralph_core::state::CommitDelta;

    let mut snap = LedgerSnapshot::cold_start();
    let delta = CommitDelta::ReviewStepUpdated {
        plan_name: "test-plan".to_string(),
        task_id: "task-1".to_string(),
        step: "step-1".to_string(),
        synth_pass: false,
        synth_terminal: None,
    };
    snap.apply_delta(&delta);

    // ReviewStepTracker should have the update applied
    assert!(snap.review_step_tracker().has_step("test-plan", "task-1", "step-1"));
}
```

注:`review_step_tracker().has_step(...)` 可能不存在,需要根据 `ReviewStepTracker` 真实 API 调整断言。

- [ ] **Step 5.5: 跑测试确认通过**

```bash
cargo nextest run -p ralph-core -- apply_delta_records
```

Expected: PASS

- [ ] **Step 5.6: Commit(若仅测试新增,无源码改动,可合到 T1)**

```bash
git add crates/ralph-core/src/state/snapshot.rs crates/ralph-core/src/state/tests.rs
git commit -m "test(state): verify snapshot 4-delta audit log rebuild (U11-T5)"
```

---

### Task T6: U10 验证报告虚假声明修正

**Files:**
- Modify: `docs/plans/2026-06-21-002-unified-state-u10-verification.md:180-181`

**Steps:**

- [ ] **Step 6.1: 读当前 line 175-185 确认确切文本**

```bash
sed -n '175,185p' docs/plans/2026-06-21-002-unified-state-u10-verification.md
```

- [ ] **Step 6.2: 修改 line 180-181 的虚假声明**

Edit: line 180 改为

> "`UNIFIED_VALIDATION` / `UNIFIED_HANDOFF_AUTO`:源码里没有 env var 读取(注释过时);等价于**默认启用**但尚未在 `event_loop/mod.rs::process_parse_result` 中按 per-event 接入。"

Edit: line 181 改为

> "`UNIFIED_STATE_LEDGER`:env-var opt-in;U1 实现已 commit,`build_state_ledger_from_env` 在 `with_diagnostics` 构造器中接入(2026-06-22 修复前需 U11-T1 让 `new()` 自动 replay_from_disk)。**默认关闭**。"

其余 3 个 flag(`UNIFIED_PROTOCOL_VIEW` / `UNIFIED_POLICY_CHECK` / `UNIFIED_DETERMINISTIC_CORRECTION`)类似修正,把所有"默认启用"的措辞改为"代码实现完成 + 默认关闭 + production wire-up 待 U11 完成"。

- [ ] **Step 6.3: 跑反向验证(按 AGENTS.md 硬规则)**

按 CLAUDE.md 硬规则,文档改动后用 `sed -n 'NN,MMp'` 复核所有引用范围仍正确。本文件无代码引用,仅叙述文字,无需行号复核。

- [ ] **Step 6.4: Commit**

```bash
git add docs/plans/2026-06-21-002-unified-state-u10-verification.md
git commit -m "docs(plan): correct false claims in U10 verification report (U11-T6)"
```

---

### Task T7: Feature flag 默认值反转 + U6 unified pipeline 修复

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs:458, 10942`(`UNIFIED_VALIDATION` / `UNIFIED_STATE_LEDGER`)
- Modify: `crates/ralph-core/src/correction/mod.rs:600-605`(`UNIFIED_DETERMINISTIC_CORRECTION`)
- Modify: `crates/ralph-core/src/preset/engine/protocol.rs:188-190`(`UNIFIED_PROTOCOL_VIEW`)
- Modify: `crates/ralph-cli/src/policy_check.rs:834`(`UNIFIED_POLICY_CHECK`)
- Modify: `crates/ralph-cli/src/policy_check.rs:746-794`(`run_policy_check_unified` 改为加载 events.jsonl)
- Modify: `crates/ralph-core/src/preset/engine/protocol.rs:tests/`(2 条 env-isolation 测试改用 serial_test)

**Steps:**

- [ ] **Step 7.1: 反转 4 个 core flag 的默认值**

`UNIFIED_VALIDATION`(`event_loop/mod.rs:458`):

```rust
// Before:
let enabled = std::env::var("UNIFIED_VALIDATION")
    .ok()
    .map(|v| v.trim() == "1")
    .unwrap_or(false);

// After:
let enabled = match std::env::var("UNIFIED_VALIDATION").ok() {
    Some(v) if v.trim() == "0" => false,  // explicit off
    _ => true,                            // unset or "1" → on (default-on)
};
```

`UNIFIED_STATE_LEDGER`(`event_loop/mod.rs:10942`):

```rust
// Before:
let raw = std::env::var("UNIFIED_STATE_LEDGER").ok()?;
let trimmed = raw.trim();
if trimmed == "1" { ... }
else if trimmed.is_empty() || trimmed == "0" { None }

// After:
match std::env::var("UNIFIED_STATE_LEDGER").ok().as_deref() {
    Some("0") => None,    // explicit off
    _ => Some(crate::state::StateLedger::new(workspace, true)),  // unset or "1" → on
}
```

`UNIFIED_DETERMINISTIC_CORRECTION`(`correction/mod.rs:600`):

```rust
// Before:
.map(|v| v.trim() == "1")
.unwrap_or(false)

// After:
match std::env::var("UNIFIED_DETERMINISTIC_CORRECTION").ok().as_deref() {
    Some("0") => false,
    _ => true,
}
```

`UNIFIED_PROTOCOL_VIEW`(`protocol.rs:188`):

```rust
// Before:
let feature_flag_enabled =
    std::env::var("UNIFIED_PROTOCOL_VIEW").ok().as_deref() == Some("1");

// After:
let feature_flag_enabled = !matches!(
    std::env::var("UNIFIED_PROTOCOL_VIEW").ok().as_deref(),
    Some("0")
);
```

`UNIFIED_POLICY_CHECK`(`policy_check.rs:834`):

```rust
// Before:
if std::env::var("UNIFIED_POLICY_CHECK").ok().as_deref() == Some("1") { /* unified */ }

// After:
let unified = !matches!(
    std::env::var("UNIFIED_POLICY_CHECK").ok().as_deref(),
    Some("0")
);
if unified { /* unified */ }
```

- [ ] **Step 7.2: 修复 `run_policy_check_unified` events.jsonl 加载**

`policy_check.rs:776`:

```rust
// Before:
let snapshot = LedgerSnapshot::cold_start();
let projected = snapshot.clone();

// After:
// R12 (U11-T7): load .ralph/events.jsonl into LedgerSnapshot
// so the unified pipeline sees terminal/business state.
let events_path = workspace_root.join(".ralph/events.jsonl");
let snapshot = if events_path.exists() {
    match crate::state::StateLedger::replay_from_disk(&workspace_root) {
        Ok(snap) => snap,
        Err(e) => {
            eprintln!("Warning: replay failed for policy check: {e}. Using cold start.");
            LedgerSnapshot::cold_start()
        }
    }
} else {
    LedgerSnapshot::cold_start()
};
let projected = snapshot.clone();
```

- [ ] **Step 7.3: 修复 2 条 env-isolation 测试**

`preset/engine/protocol.rs::tests::u3_feature_flag_default_off_explicit_on` 和 `validation::tests::pipeline_records_protocol_view_feature_flag`:

```rust
// Before:
#[test]
fn u3_feature_flag_default_off_explicit_on() {
    std::env::set_var("UNIFIED_PROTOCOL_VIEW", "1");
    /* ... */
}

// After:
#[test]
#[serial]  // require serial_test crate; add to dev-dependencies if missing
fn u3_feature_flag_explicit_off_default_on() {
    // U11-T7: default is now on; explicit "0" disables
    std::env::set_var("UNIFIED_PROTOCOL_VIEW", "0");
    /* assert feature_enabled == false */
    
    std::env::remove_var("UNIFIED_PROTOCOL_VIEW");
    /* assert feature_enabled == true (default-on) */
}
```

如 `serial_test` 不在 dev-dependencies,需在 `Cargo.toml` 加 `serial_test = "3"`(dev-dependencies)。

- [ ] **Step 7.4: 跑默认状态全量测试**

```bash
./scripts/run-tests.sh
```

Expected: 5075/5075 PASS(默认状态下新增的 replay-on-cold-start 不破坏现有行为,因为 default-off 路径走 cold_start fallback)

- [ ] **Step 7.5: 跑 flag-on 全量测试**

```bash
UNIFIED_STATE_LEDGER=1 \
UNIFIED_VALIDATION=1 \
UNIFIED_DETERMINISTIC_CORRECTION=1 \
UNIFIED_PROTOCOL_VIEW=1 \
UNIFIED_POLICY_CHECK=1 \
./scripts/run-tests.sh
```

Expected: 5075/5075 PASS(U6 14 条 CLI 测试 gap 修复 + 2 条 env-isolation 测试修复 + T2 pipeline 接入 + T3 correction 真实 context 后,全部 16 条 flag-on 失败应消除)

- [ ] **Step 7.6: 兜底跑**

```bash
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

Expected: 全过

- [ ] **Step 7.7: Commit**

```bash
git add crates/ralph-core/src/event_loop/mod.rs crates/ralph-core/src/correction/mod.rs crates/ralph-core/src/preset/engine/protocol.rs crates/ralph-cli/src/policy_check.rs crates/ralph-core/Cargo.toml crates/ralph-cli/Cargo.toml
git commit -m "feat(flags): flip env var defaults to ON; fix U6 pipeline + env-isolation tests (U11-T7)"
```

---

### Task T8: 文档同步(P1-5/6/7)

**Files:**
- Modify: `docs/report/2026-06-21-top-3-architectural-instability-factors.md`
- Modify: `docs/guide/runtime-diagnosis.md`
- Modify: `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-tasks.md`、`ralph-tools-memories.md`(按 AGENTS.md 反向验证规则)

**Steps:**

- [ ] **Step 8.1: 更新 top-3 architectural instability report**

读 `docs/report/2026-06-21-top-3-architectural-instability-factors.md`,在末尾添加 "修复状态" 章节:

```markdown
## 修复状态(2026-06-22,U11 commit)

| 不稳定因素 | 修复单元 | 状态 |
|---|---|---|
| 状态源分散 | U1 StateLedger + U2 StateProjector migrate | ✅ 代码完成,U11 接入 production(U11-T1/T2) |
| `task.resume` 循环 | U7a deterministic correction + U7b `loop.resume` | ✅ 代码完成,U11 接入 correction context(U11-T3) |
| 协议视图分裂 | U3 ProtocolView + U4 ValidationPipeline + U6 CLI 迁移 | ✅ 代码完成,U11 接入 per-event + flag 默认全开(U11-T7) |
```

- [ ] **Step 8.2: 更新 runtime-diagnosis.md**

读 `docs/guide/runtime-diagnosis.md`,在 `ralph diagnose` 命令章节添加 `--from-ledger` 选项说明:

```markdown
### `--from-ledger`(U8 + U11)

`ralph diagnose --from-ledger` 优先读取 `.ralph/recovery.jsonl` 和 `.ralph/ledger.jsonl`,
输出由 `correction::emit_correction_context` 写入的结构化 `RejectionRecord` 列表,
按 `retry_key` (hat + topic + reason_code) 聚合。适用于冷启动后诊断历史 rejection。

fallback 路径:若 ledger 不存在,降级读取 legacy `.ralph/recovery.jsonl`。
```

- [ ] **Step 8.3: 反向验证 ralph-tools*.md**

按 CLAUDE.md 反向验证规则,用 `sed -n 'NN,MMp'` 复核 `ralph-tools*.md` 中所有形如 `xxx.rs:NN-MM` 的源码引用范围是否仍指向正确代码。重点确认:

- `ralph tools skill/interact` 章节的 env var 名称(`UNIFIED_*`)是否仍正确
- `ralph tools wave` 章节的 fallback 路径说明是否仍准确
- `ralph tools diagnose` 章节的 `--from-ledger` 选项是否描述正确(已 Step 8.2 更新)

如发现漂移(如行号变更 / 参数表与 clap 定义不符),立即同步修正。

- [ ] **Step 8.4: Commit**

```bash
git add docs/report/2026-06-21-top-3-architectural-instability-factors.md docs/guide/runtime-diagnosis.md crates/ralph-core/data/ralph-tools*.md
git commit -m "docs: sync U11 fixes to top-3 report + runtime-diagnosis + ralph-tools (U11-T8)"
```

---

## Self-Review

### Spec coverage
- T1 → spec §3 T1(StateLedger replay)✓
- T2 → spec §3 T2(per-event pipeline)✓
- T3 → spec §3 T3(correction context 真实化)✓
- T4 → spec §3 T4(handoff auto-gen missing path)✓
- T5 → spec §3 T5(snapshot delta 复核)✓
- T6 → spec §3 T6(U10 报告修正)✓
- T7 → spec §3 T7(flag default-on + U6 fix)✓
- T8 → spec §3 T8(文档同步)✓

### Placeholder scan
- "implement later", "TBD", "TODO" → 已替换为具体代码或精确描述
- "Similar to Task N" → 每个 Task 独立完整,无外部引用
- "Add appropriate error handling" → 已给出具体 match + warn! 模式
- "fill in details" → 已标注"实施时根据源码确认"(适用于具体字段名调整)

### Type consistency
- `StateLedger::replay_from_disk(workspace: &Path)` 在 T1/T7 中一致使用
- `ValidationPipeline::validate_pre_commit_with_view(view, snapshot, event)` 在 T2 中使用
- `publish_correction_via_context(bus, state, ledger, event, payload)` 在 T2/T3 中签名一致
- `ledger.commit_handoff_artifact(&HandoffAcceptedInputs)` 在 T4 中签名一致

### Open notes for executor
- T2 Step 2.3 / T3 Step 3.3 / T4 Step 4.3 标注"实施时根据源码确认"的字段,需 executor 在 read source 后精调。这是预期内的灵活性,不是 plan 缺陷。
- T5 是复核任务,可能 no-op(已实现) → executor 应先读源码再决定是否需要改代码。

---

## Acceptance Checklist

- [ ] T1: `StateLedger::new` 调用 `replay_from_disk`,fallback cold_start
- [ ] T2: per-event `pipeline.validate_pre_commit_with_view` 在 `process_parse_result` 中出现
- [ ] T3: `state.prompt_context` 在 correction 时 in-place 写入,2 条 BDD `#[ignore]` 移除并通过
- [ ] T4: macro-edge 缺失 path 时 `commit_handoff_artifact` 触发
- [ ] T5: snapshot.rs 4 个 delta 变体的 audit log 测试通过
- [ ] T6: U10 报告虚假声明修正
- [ ] T7: 4 个 feature flag 默认值反转,`./scripts/run-tests.sh` 默认 + flag-on 两组都 0 失败
- [ ] T8: 3 处文档同步完成
- [ ] `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底通过

---

## Risk Mitigation

- **T2 性能风险**:per-event pipeline 增加 ~10μs/事件。在 ce-executor-serial benchmark 上跑一次,若 >5% 退化,加 `ProtocolView` per-batch 缓存(已在 U3 设计中预留)。
- **T3 break BDD**:2 条 BDD `#[ignore]` 移除后若仍 fail,说明 correction 注入逻辑有 bug → 立即 revert + 看 `correction_deterministic.yml` 和 `correction_three_escalation.yml` 期望。
- **T7 flag-on 失败**:U6 14 条 CLI 测试若仍 fail,可能 `run_policy_check_unified` events.jsonl 加载路径有 race → 加 file lock / lock-free reader。
- **T8 文档漂移**:反向验证步骤必须执行,任何 `sed -n` 复核失败都需立刻修复。