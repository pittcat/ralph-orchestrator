---
name: ralph-tools-handoff
description: ce-executor step handoff 深参考 — `task.resume` 复杂 violation 的归属、progress 修复、wave 收摊路径（按需加载）
metadata:
  internal: true
---

# ralph-tools-handoff — Step handoff 深参考

> **先读自动注入 R0**：loop 内 agent 收到 `task.resume` 时，**第一手修复路径**在每轮自动注入的 `ralph-tools.md`「收到 `task.resume` 时」段；本文档供按需 `ralph tools skill load ralph-tools-handoff` 后**深查**复杂 violation（progress / handoff dispatch / plan.blocked / wave 收摊等）。
>
> **不注入**：本 skill 不在 auto-inject 白名单中（plan 004 KTD3）；按需 load 节省 token。

## 1. Step handoff topic 归属（`ce-executor-isolated` preset）

| Topic | 发布 hat | 消费者 hat | 拒收 reason_code 常见值 |
|-------|---------|----------|------------------------|
| `work.ready` | `plan-gate` | `executor`（唯一） | `payload_contract_violation` / `MissingPayloadField` |
| `plan.complete` | `executor` | `plan-gate` | `progress_task_mismatch` |
| `plan.blocked` | `plan-gate` | `shipper` | `payload_contract_violation`（见 §5 provenance 约束） |
| `queue.advance` | `executor` | `plan-gate` | `progress_task_mismatch` |
| `review.wave.ready` | `plan-gate` | `dimension-reviewer` | `MissingPayloadField`（`depth`） |
| `review.dimension.done` | `dimension-reviewer` | `review-synthesizer` | wave 收摊后被 `review_passed_while_wave_open` 拒（独立 bucket） |
| `review.complete` | `review-synthesizer` | `plan-gate` | `MissingPayloadField` |
| `LOOP_COMPLETE` | `executor` / `plan-gate` | ralph | 终态，须在 hat `publishes` 显式声明 |

`trigger_multi_consumer_topics`：上表中**唯一消费者**的 topic（`work.ready` / `queue.advance` / `plan.complete` / `plan.blocked` / `review.complete`）走 `HandoffTracker` 30s SLA（`event_loop.workflow_contract.handoff_dispatch_timeout_seconds`，上限 120s）；多消费者 topic（`review.wave.ready` / `review.dimension.done`）走 wave 收摊而非 handoff。

## 2. `progress_task_gate` / `progress_task_mismatch` 修复

`queue.advance` 或 `plan.complete` 在 step handoff 时必须满足 `progress.md` ↔ `tasks.jsonl` 对齐，否则 `progress_task_gate` 拒收并触发 `task.resume`（payload 含 `reason_code: progress_task_mismatch`）。

**修复顺序**（agent 视角）：

1. 读 `.ralph/agent/progress.md` 顶部 `## Completed Steps` 列表
2. `ralph tools task list --status closed` 拿到本 step 内已 `closed` 的 task
3. 对齐：所有「当前 step 已完成」的 task 必须 `closed`；所有 `closed` 的 task 必须在 `## Completed Steps` 里
4. 缺一则先补 `ralph tools task close <task-id>` 或在 `progress.md` 加记录
5. 重发原 topic（不要绕过 gate）

**校验命令**：

```bash
# 1. 列出当前 step 的 closed task
ralph tools task list --status closed --format json | jq -r '.[] | .id'

# 2. 对比 progress.md 已记录的 step
grep -A 5 '## Completed Steps' .ralph/agent/progress.md

# 3. 看 recovery.jsonl 历史 progress_task_mismatch
jq 'select(.reason_code == "progress_task_mismatch")' \
   .ralph/diagnostics/latest/recovery.jsonl
```

CLI 入口预检（`--policy-check` 接 `progress_task_gate`）见计划 `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md` U1（机制在本计划不落地）。

## 3. `handoff_dispatch_timeout` 修复

`work.ready` 等唯一消费者 handoff 在 `handoff_dispatch_timeout_seconds`（默认 30s，上限 120s）内未被激活，触发 `recovery.jsonl` `reason_code: handoff_dispatch_timeout` + `task.resume to plan-gate`（Hard 升级）。

**排查**：

- 消费者 hat（`executor`）是否在 budget 内被 backpressure 阻塞
- 后端是否已返回但事件未 flush（看 `.ralph/events.jsonl` 末尾）
- 是否多个 worktree 同时持有 executor 上下文导致隔离预算耗尽

**不要**自重发 `work.ready` — `task.resume` 已经由机制重派回源 hat；先解决消费者未激活的根因。

## 4. Wave 收摊：缺维度 / 超时

review wave `received_count < expected_dimensions` 时的两条路径：

- **等待中**（`now - last_dimension_at < 0.8 * aggregate_timeout_secs`）：继续等 worker 收尾，**不要**自补 `review.dimension.done`。
- **超时**（`now - last_dimension_at >= 0.8 * aggregate_timeout_secs`）：**机制层**自动 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`，路由 `review-synthesizer` → `shipper`（不要等 plan-gate 自消费）；详见 `docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md` U1+U2 与 `crates/ralph-core/src/flow_lifecycle/incomplete_wave_gate.rs`。

`review_passed_while_wave_open`（U1）改为 `ViolationType::SemanticGateViolation`，独立 recoverable bucket，**不**计入 `U2_REJECTION_RETRY_LIMIT`，不发 fatal `PayloadContractViolation`；`task.resume` hint 显式禁止 empty_diff，要求等待 `plan.blocked` 或补全维度。

## 5. `plan.blocked` provenance 约束

`plan.blocked` 在 isolated 模式下**只能**由 `plan-gate` hat 发布（preset 唯一合法 publisher）。其他 hat 自发 `plan.blocked` 会被 `EventOriginGuard` 拒收并触发 `task.resume`（`reason_code: out_of_scope_topic`）。

不要绕：若需表达「我无法推进」，发 `human.guidance`（等人类决策）而非 `plan.blocked`。

## 5.5 Hat→hat roadmap handoff（2026-06-18-002 plan）

> **2026-06-18 状态**：机制已落地（`hat_handoff` 模块 + CLI `ralph tools handoff prepare` + `gate` + `inject`），但**默认 disabled**（`event_loop.hat_handoff.enabled: false`）。U11 开启 `ce-executor-*` 留作 follow-up，需先跑通 `ralph-e2e --mock` 全量（计划 KTD-11 / U9 全绿门槛）。本文档供按需 load 后查阅。

### 5.5.1 概念

**宏观边** = 唯一消费者 topic ∧ 非自环 ∧ 非豁免。`ce-executor-isolated` 下的典型宏观边：

| 起点 hat | topic | 终点 hat |
|---|---|---|
| `plan-gate` | `work.ready` | `executor` |
| `executor` | `work.done` | `review-coordinator` |
| `review-coordinator` | `review.wave.ready` | dimension-reviewer wave |
| `review-synthesizer` | `review.complete` | `plan-gate` |

**微观边**（豁免，无需 roadmap）：

- `review.dimension.{ready,done,failed}`（同一 wave 内多 reviewer 之间）
- `queue.advance` 自环（plan-gate → plan-gate，KTD-9 显式豁免）

### 5.5.2 操作流程

发布宏观边前：

1. **Prepare**（拿确定性路径 + 五段式 skeleton）：

   两种等效方式选一种即可：

   **方式 A：从 `## ORCHESTRATOR CONTEXT` 读**（U3，enabled 时上下文自带）：

   ```bash
   # 解析 ORCHESTRATOR CONTEXT 块里的 hat_handoff_seq / hat_handoff_next_seq
   # 提取脚本示例（伪）：
   ITER=$(grep -E '^- current_step: ' prompt.txt | head -1 | awk '{print $2}' | tr -d 'step-')
   # 实际上:loop 注入的 ORCHESTRATOR CONTEXT 含 hat_handoff_seq 直接用
   ralph tools handoff prepare \
     --from executor \
     --to review-coordinator \
     --topic work.done \
     --iteration "$ITER" \
     --current-seq "$HAT_HANDOFF_SEQ" \
     --json
   ```

   **方式 B：从 env var 读**（U1,loop 子进程内 runner 注入）：

   ```bash
   ralph tools handoff prepare \
     --from executor \
     --to review-coordinator \
     --topic work.done \
     --iteration "$RALPH_LOOP_ITERATION" \
     --current-seq "$RALPH_HAT_HANDOFF_SEQ" \
     --json
   ```

   返回 `handoff_path: .ralph/agent/hat-handoff/{iter}-{seq+1}-{from}-{to}.md`。
   `-` / `.` / ` ` 等在文件名中会被 sanitize 为 `_`，保证 4 段拆分稳定。

2. **填写五段式 markdown**：

   ```markdown
   # Handoff: executor → review-coordinator
   ## context
   <从 executor 上下文复制>

   ## changed
   <本 step 改了哪些文件 / 行>

   ## verify
   <已完成的自检 / 测试结果>

   ## next
   **动作**: emit review.wave.ready after dimension list assembly
   **阻塞**: 无

   ## notes
   无
   ```

3. **Emit 时带 payload 字段**：

   ```bash
   ralph emit --hat executor --topic work.done \
     --payload "$(jq -nc --arg hp '.ralph/agent/hat-handoff/1-2-executor-review_coordinator.md' \
       '{task_id:"task-real-001", handoff_path:$hp}')"
   ```

4. **CLI 镜像预检**：`ralph emit --policy-check` 走 `gate::evaluate_event` 纯函数（reason_code SSOT 与 runtime 共享）。

### 5.5.3 拒收 reason_code 表

| reason_code | 含义 | 修复 |
|---|---|---|
| `hat_handoff_missing_path` | 宏观边 payload 无 `handoff_path` | 跑 `ralph tools handoff prepare` 拿路径 |
| `hat_handoff_path_escape` | 路径含 `..` 或绝对路径 | 使用 repo-相对路径 |
| `hat_handoff_filename_mismatch` | 文件名 iter / seq / from / to 与 caller 不一致 | 重新 prepare 或 `--force` 同 path 覆盖（KTD-14） |
| `hat_handoff_file_not_found` | 路径存在但文件缺失 | 跑 prepare 让它写 skeleton，再填 |
| `hat_handoff_file_read_fail` | 文件存在但不可读 | 检查 workspace 权限 |
| `hat_handoff_structure_invalid` | 五段式 / `## next` 不合法 | 填齐五段 + `**动作**:` / `**阻塞**:` 必填 |
| `hat_handoff_illegal_emit_topic` | `## next` 动作行引用的 topic 不在下游 hat publishes | 改为下游 hat 能发的 topic，或纯阅读类动作 |

### 5.5.4 同 path 重试（KTD-14）

拒收 → 修复内容 → `ralph tools handoff prepare --force ...`：覆盖**同一 seq 路径**。禁止写已 accept 的旧 seq(防 history 覆写)。

### 5.5.5 修复流程（agent 视角）

收到 `task.resume(reason_code=hat_handoff_*)` 后：

1. 读 payload `message` 字段拿到具体 reason_code
2. 按上表对应修复
3. 同 path `--force` 覆盖（KTD-14）或重新 prepare 取下一个 seq
4. 重发原 topic（不要绕过 gate）

KTD-5 保护：拒收时 `HandoffTracker::cancel_pending(event_id)` 会抹掉 policy-accept 时记录的 phantom pending，不会触发 30s escalation 假阳性。

### 5.5.6 plan-gate 双发场景（KTD-9）

`plan-gate` 在每 step 同时发 `queue.advance` + `work.ready`：

- `queue.advance`：**自环** → **无需** handoff（KTD-9 豁免）
- `work.ready`：**宏观边** → **必须** handoff

不要给 `queue.advance` 写 plan-gate→plan-gate 的废话 handoff。

### 5.5.7 注入块（KTD-6, KTD-16）

下游 hat `build_prompt` 会在 `## WAVE CONTEXT` 与 `## ORCHESTRATOR CONTEXT` 之间注入 `## HAT HANDOFF` 块。**fail-closed**：文件缺失 / 不可读 / path 与 pending 不一致 → **不注入** + 发 `event.hat_handoff.inject_failed` diagnostic。

超 `max_bytes`（默认 2048）截断时**完整保留 `## next`** 段。

### 5.5.8 调试命令

```bash
# 看当前 hat pending 是否含 handoff_path
ralph tools handoff prepare --from <self> --to <consumer> --topic <topic> --no-write

# CLI 预检（与 runtime 同 reason_code）
ralph emit --hat <self> --topic <topic> \
  --payload "$(cat payload.json)" --policy-check

# 看 gate 拒收历史
jq 'select(.topic == "diagnostic.hat_handoff.rejected")' \
  .ralph/events.jsonl | tail -5
```

### 5.5.9 U11 开启步骤（follow-up）

```yaml
# presets/en/ce-executor-isolated.yml
event_loop:
  hat_handoff:
    enabled: true
```

开启前必须：

1. `cargo nextest run -p ralph-core --test scenarios hat_handoff` 全绿
2. `cargo run -p ralph-e2e -- --mock` 全量通过
3. 在小流量 preset 上 dot release 1 个完整 plan，验证 `findings.md` 中无新增 `hat_handoff_*` 拒收

若开 enable 后出现批量拒收，立即回滚为 `enabled: false` 并按 5.5.3 reason_code 表排查 agent 操作问题（不要临时改 gate 逻辑）。

## 6. 校验命令速查

```bash
# 当前 hat 可发 topic（与 isolated 越权判定对齐）
ralph hats list --format json | jq -r '.[] | select(.id == "'"$RALPH_CURRENT_HAT"'") | .publishes[]'

# 看最近一轮 task.resume 来源
jq 'select(.type == "task.resume")' .ralph/events.jsonl | tail -1

# 看 recovery.jsonl 全部 envelope
jq '.' .ralph/diagnostics/latest/recovery.jsonl

# 出报告（CI / post-mortem）
ralph diagnose --session latest
```

## 7. 相关文档

- `docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md` — step handoff 机制完整设计
- `docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md` — wave 收摊 / R6 机制
- `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md` — CLI 预检对齐（姊妹 PR）
- `docs/guide/runtime-diagnosis.md` §10 / §12.1 — 诊断决策树
- `crates/ralph-core/data/ralph-tools.md` — 每轮自动注入的修复段（速查）
- `crates/ralph-core/data/ralph-tools-emit.md` — emit 详表（schema / null-payload / isolated）
