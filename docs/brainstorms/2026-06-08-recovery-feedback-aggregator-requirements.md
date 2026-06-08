---
date: 2026-06-08
topic: recovery-feedback-aggregator
---

# Recovery 上报与反馈聚合：3 层证据 → 信号 → Spec，人类 Review 闭环

## Summary

把当前"recovery 撑住"的能力升级为"recovery 撑住 + 上报"的能力：runtime evidence（`recovery.jsonl` / `drift.jsonl`）→ 跨 iteration/跨 run 聚合 → 触发阈值后自动写 `.ralph/specs/fix-NNN.md` 草稿 + scratchpad 提示，让 AI 在下次 loop 中看到 signal 而非沉默失败。**AI 永远不直接改 preset**（builtin 编译进二进制，自定义 preset 也不应让 AI 在 loop 中改），spec 由人类 review、人类 commit、人类触发 rebuild，链路有自然冷却。

## Problem Frame

grand-lily run 暴露的核心矛盾：**runtime 知道 preset 有问题，但反馈回路断在"知道"和"修好"之间**。

`recovery.jsonl` 12 事件全是 evidence：
- iter 2 `TaskNotTerminal`（execution_contract 违反）
- iter 5/8/11 三次 `stall_no_events`（stall_recovery）
- iter 9 `MissingPayloadField plan_path`（execution_contract）
- 多次 `drift_monitor` 字段跌破阈值

`drift.jsonl` 显示 U5 drift 三个指标中有 2 个跌破 baseline。

但所有这些 evidence 都被 swallow 了 —— recovery 让 loop 继续跑、跑完了、exit 0。下次 ralph run 用**完全相同**的 preset，可能再次遇到完全相同的 violation，因为：
1. evidence 没被聚合到"这是同一类问题"；
2. 聚合后没被路由到"应该改 preset 的哪个 hat"；
3. 即使知道改哪里，AI 在 loop 中也**不能**改 preset（builtin 是 `include_str!` 编译进二进制）；
4. 写出来的 spec 也没机制让"下次 loop 的 agent 看到 + 让人类 review"。

结果是：ralph 显得"很 resilient"，但**长期同一种错误反复出现**，runtime 永远在 recover 同一种问题。

## Key Decisions

### 1. 3 层反馈模型：Evidence → Aggregation → Action

- **Layer 1 - Evidence Collection（已有，本次扩展）**：
  - `recovery.jsonl`：stall / execution_contract / payload_contract / drift_monitor / hook_retry / loop_stale 等 envelope
  - `drift.jsonl`：U5 drift 三个指标（field completeness / coord join rate / emit cadence）
  - **新增**：`feedback.jsonl` —— Layer 2 聚合后的 signal 写到独立文件，与 evidence 分离
- **Layer 2 - Aggregation（新）**：
  - 跨 iteration 聚合：同一 envelope_type + 同一 topic 出现 N 次 → 升级
  - 跨 run 聚合：`.ralph/feedback/history/` 持久化 signal 摘要，新 run 启动时加载
  - 阈值触发：升级 outcome 类（`pending → recovered / repeated / escalated`）
- **Layer 3 - Action（新）**：
  - 阈值触发后写 `.ralph/specs/fix-NNN-{signal_id}.md` 草稿（**AI 只写草稿，不修改 preset**）
  - 在下次 loop 的 scratchpad 注入 "## Recovery Signal" 段
  - emit `preset.design.signal` 事件（new event type，给未来 hook 留扩展点）
  - **人类 review 链路**（见 R6）：spec review → 人类 commit preset 修改 → 人类触发 rebuild → 下次 ralph run 用新二进制

不引入"AI 自我修复 loop" —— 显式拒绝。AI 在 Layer 3 只能写 spec 草稿 + 写 signal 到 scratchpad，**不能**写 preset，不能 git commit，不能 cargo build。

### 2. outcome 类映射规则

| outcome | 触发条件 | 后续行为 |
|---|---|---|
| `pending` | 首次发现 | 仅记录，不影响 loop |
| `recovered` | evidence 后该问题消失 ≥ 1 个完整 iteration | 写 recovery 摘要，不发 spec |
| `repeated` | 同 envelope_type + 同 topic 在同一 run 出现 ≥ 2 次 | 写 scratchpad 提示 + 记入 `feedback.jsonl` |
| `escalated` | `repeated` 跨 run 出现 ≥ 2 次（最近 5 次 run 内） | 写 spec 草稿 |
| `failed` | escalated 后 spec 草稿 review 超时（人类 7 天未处理） | 升级为 `not_retriable` 候选 |
| `not_retriable` | 显式标记或 failed 30 天未处理 | 不再聚合，下次 run 跳过 |

### 3. signal 路由用 Doc 1 的 `owner_hat` 字段

- 同一 signal 涉及多个 topic 时，按 topic 拆出"owner 集合"，scratchpad 提示"问题涉及 hat: {executor, review-coordinator}"。
- spec 草稿的 frontmatter 包含 `affected_topics` / `affected_owner_hats` 字段，便于人类 review 时一眼看到"修改会动到哪些 hat"。
- 若 `topic_owners[topic]` 不存在（向后兼容场景），spec 草稿标 `unrouted: true`，人类 review 时补 owner 字段。

### 4. AI 不直接改 preset（硬约束）

- preset 修改路径：**人类**在编辑器或通过 `ralph preset edit`（CLI 提供 atomic 编辑入口）修改 yml，**人类**触发 `cargo build`，**人类**提交 git commit。
- AI 在 loop 中最多做到：把 signal 写进 spec 文件 + scratchpad 段。这是 Layer 3 唯一允许的 AI 动作。
- builtin preset 的修改还有"重建二进制"冷却时间：spec 写完 → 人类 review → 人类 commit → 人类 `cargo build` → 几小时后下次 run 才生效。这给"自我强化循环"提供了自然冷却。

不实现 `ralph preset fix` 这类 AI 自动命令 —— 这是 2026-06-02 doc 显式排除的。

### 5. 反馈有节流，不刷屏

- 同一 `signal_id` 在同一 run 内最多触发 1 次 `escalated`（避免 loop 中 spec 草稿被反复覆盖）。
- 同一 signal_id 在最近 5 次 run 内只生成 1 个 spec 草稿（避免 spec 目录爆炸）。
- 跨 run 升级时检查 `affected_topics` 是否有变化：若完全相同则复用旧 spec 编号，仅更新 `last_seen_at`；若有变化则生成新 spec。

### 6. scratchpad 注入的格式

- 注入位置：loop 下次启动时（而非循环内），scratchpad 顶部追加 `## Recovery Signal(s)` 段
- 格式（伪 yaml）：
  ```yaml
  ## Recovery Signal(s)
  
  signal_id: sig-2026-06-08-executor-stall-repeated
  outcome: escalated
  affected_topics: [work.done, review.wave.ready]
  affected_owner_hats: [executor, review-coordinator]
  evidence_count_this_run: 4
  evidence_count_last_5_runs: 7
  spec_draft: .ralph/specs/2026-06-08-fix-executor-stall-repeated.md
  
  Brief: executor hat 4 次 stall 后仍未 emit work.done, 关联 review-coordinator 的 review.wave.ready 也未触发。推测 preset 中 executor.terminal_event 路径缺失或 publish 顺序错误。
  
  Action: (1) review spec 草稿 (2) 修复后改 presets/en/ce-executor.yml (3) 人类手动 commit + rebuild
  ```
- AI agent 看到此段后，可读 spec 草稿，可把 spec 内容转成"建议 prompt"塞给当前 hat，但**不能**直接修改 preset。

## Actors

- **Recovery subsystem（rust）**：写 evidence 到 recovery.jsonl / drift.jsonl（已有）；写 signal 到 feedback.jsonl（新增）。
- **Aggregation daemon（rust）**：在 loop 启动与每次 stall 升级时跑聚合计算，生成 outcome 转换。
- **Spec writer（rust + AI 协助）**：在 outcome = escalated 时调 AI 生成 spec 草稿（AI 只能写 .ralph/specs/，不能写 preset）。
- **Scratchpad injector（rust）**：loop 启动时把 escalated signal 注入 scratchpad 段。
- **AI agent（loop 内）**：读 scratchpad signal；写 spec 草稿（不允许）；不写 preset。
- **人类 preset 维护者**：唯一被授权修改 preset + 触发 rebuild 的角色。
- **CI / 钩子（可选）**：spec 写完后可挂 webhook / GitHub issue（默认关闭，opt-in）。

## Requirements

### R1. feedback.jsonl 与 signal schema

- R1.1 新增 `.ralph/feedback.jsonl`，每行一个 signal：
  ```json
  {
    "signal_id": "sig-2026-06-08-executor-stall-repeated",
    "first_seen_run_id": "loop-2026-06-08T18-02-16",
    "last_seen_run_id": "loop-2026-06-08T18-02-16",
    "first_seen_at": "2026-06-08T18:02:16Z",
    "last_seen_at": "2026-06-08T18:34:51Z",
    "outcome": "escalated",
    "outcome_history": [
      {"at": "2026-06-08T18:02:16Z", "from": "pending", "to": "repeated"},
      {"at": "2026-06-08T18:34:51Z", "from": "repeated", "to": "escalated"}
    ],
    "envelope_type": "stall_recovery",
    "affected_topics": ["work.done", "review.wave.ready"],
    "affected_owner_hats": ["executor", "review-coordinator"],
    "evidence_summary": {
      "iter_5": {"type": "stall_no_events", "hat": "executor"},
      "iter_8": {"type": "stall_no_events", "hat": "executor"},
      "iter_11": {"type": "stall_no_events", "hat": "executor"}
    },
    "spec_draft_path": ".ralph/specs/2026-06-08-fix-executor-stall-repeated.md",
    "human_review_status": "pending"
  }
  ```
- R1.2 signal_id 生成规则：`sig-{date}-{short_hash(topics + envelope)}`，保证同一类问题生成同一 ID。
- R1.3 feedback.jsonl append-only，outcome 转换通过追加 outcome_history 项表达，不修改既有行。

### R2. 跨 iteration 聚合

- R2.1 recovery.jsonl 每写一条新 evidence，触发聚合 daemon 检查：同 `envelope_type` + 同 `affected_topic` 在当前 run 内出现次数。
- R2.2 阈值：≥ 2 次 → outcome 从 `pending` 升 `repeated`；≥ 3 次 → 直接升 `escalated`。
- R2.3 升级时 outcome_history 追加一项。

### R3. 跨 run 聚合

- R3.1 loop 启动时聚合 daemon 加载 `.ralph/feedback/history/*.json`（每次 run 结束后把 feedback.jsonl snapshot 到 history）。
- R3.2 检查最近 5 次 run 中，同 signal_id 的 outcome 分布：
  - `repeated` ≥ 2 次 → 升级 `escalated`
  - `escalated` ≥ 1 次（之前已写 spec）但 7 天未处理 → 升级 `failed`
- R3.3 `failed` 30 天未处理 → 升级 `not_retriable`，写入 `not_retriable.jsonl`（永久跳过的黑名单）。

### R4. Spec 草稿生成

- R4.1 outcome 升 `escalated` 时，调用 spec_writer 生成 `.ralph/specs/{date}-fix-{signal_id_short}.md` 草稿。
- R4.2 草稿 frontmatter：
  ```yaml
  ---
  date: 2026-06-08
  topic: fix-executor-stall-repeated
  signal_id: sig-2026-06-08-executor-stall-repeated
  affected_topics: [work.done, review.wave.ready]
  affected_owner_hats: [executor, review-coordinator]
  evidence_artifact: .ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl
  status: draft
  ---
  ```
- R4.3 草稿正文由 AI 生成，包含：问题描述、evidence 摘要、推测的 preset 修改点（**不**直接给出改好的 yml 片段）、人类 review checklist。
- R4.4 AI 调用方是 `spec_writer` 内部子进程，AI 只能写 `.ralph/specs/`，不能写 `presets/`（路径白名单强制）。

### R5. scratchpad 注入

- R5.1 loop 启动时 scratchpad_injector 扫描 `feedback.jsonl`，过滤 `outcome in [repeated, escalated, failed]` 且 `human_review_status != resolved` 的 signal。
- R5.2 把这些 signal 序列化为 "## Recovery Signal(s)" 段，追加到 scratchpad 顶部（不覆盖既有内容）。
- R5.3 signal 段在 prompt 中对 agent 可见，但不强制 agent 行动（agent 可选择忽略，但 spec 草稿路径会被列出便于深读）。

### R6. 人类 review 闭环

- R6.1 spec 草稿 `status: draft` 表示待 review。
- R6.2 人类可执行三种动作（通过 CLI）：
  - `ralph feedback accept <signal_id>` → spec 状态升 `accepted`，人类需手动修改 preset（不在 CLI 范围内），修改完后 `ralph feedback resolved <signal_id>` 标记解决。
  - `ralph feedback defer <signal_id>` → spec 状态 `deferred`，aggregation 不再升级此 signal。
  - `ralph feedback dismiss <signal_id>` → spec 状态 `dismissed`，signal 永久跳过。
- R6.3 三种动作都不自动触发 preset 修改 —— **人类**在编辑器或 `ralph preset edit` 中改 yml，**人类** git commit，**人类** `cargo build`。
- R6.4 反馈 CLI 行为写入 `feedback.jsonl` 的 outcome_history，便于审计。

### R7. 内置 preset 修改的"重建冷却"

- R7.1 builtin preset 是 `include_str!` 编译进二进制（`crates/ralph-cli/src/presets.rs:35`），改 yml 后必须 `cargo build` 才生效。
- R7.2 aggregation daemon 不试图绕过这一约束（不实现"运行时重载 preset"）。spec 写完后人类 review → 人类 commit → 人类 rebuild → 下次 run 才生效。
- R7.3 文档明示此约束，让"为什么我改了 yml 但行为没变"对用户透明。

### R8. 与现有 telemetry / drift 集成

- R8.1 复用 `crates/ralph-core/src/diagnostics/` 的 U5 drift monitor、recovery 写入路径。
- R8.2 复用 `.ralph/diagnostics/{timestamp}/` 目录结构，反馈链路与诊断产物共生。
- R8.3 现有 `ralph diagnose --session latest` 命令扩展 `--include-feedback` 选项，输出 `feedback.jsonl` 摘要。

## Acceptance Examples

- AE1. **stall 重复升级到 escalated**
  - **Given** 当前 run 中 `executor` hat 3 次 stall。
  - **When** 第 3 次 stall 写入 recovery.jsonl。
  - **Then** feedback.jsonl 出现 signal `outcome: repeated`（iter 5 第 2 次升级），下次 stall 升级为 `escalated`，spec 草稿写 `.ralph/specs/{date}-fix-executor-stall-repeated.md`。
- AE2. **跨 run 升级**
  - **Given** 最近 5 次 run 中有 2 次出现 `sig-...-executor-stall-repeated` 且 outcome 都是 `repeated`。
  - **When** 第 6 次 run 启动。
  - **Then** aggregation daemon 把 signal 升 `escalated`，spec 草稿生成（若未生成过），scratchpad 注入。
- AE3. **scratchpad 注入对 agent 可见**
  - **Given** feedback.jsonl 有一条 `escalated` signal。
  - **When** 下次 ralph run 启动，agent hat 进入第 1 iteration。
  - **Then** agent prompt 的 scratchpad 段含 "## Recovery Signal(s)" 子段，含 signal_id / outcome / affected_topics / spec 草稿路径。
- AE4. **AI 不写 preset**
  - **Given** agent 在 prompt 看到 scratchpad signal 指向 spec 草稿。
  - **When** agent 试图 `ralph preset edit` 或 `git commit` 修改 `presets/en/ce-executor.yml`。
  - **Then** `spec_writer` 路径白名单强制拒写 presets/ 路径（不通过 editor API），git 钩子（若安装）拒 commit。
- AE5. **人类 resolve 流程**
  - **Given** 人类看到 spec 草稿，修改了 `presets/en/ce-executor.yml`（给 executor 加 `terminal_event`），commit + `cargo build`。
  - **When** 跑 `ralph feedback resolved sig-...-executor-stall-repeated`。
  - **Then** feedback.jsonl 的 `human_review_status` 变 `resolved`，下次 run 启动 aggregation 不再升级此 signal。
- AE6. **失败升级到 not_retriable**
  - **Given** signal 升 `escalated` 30 天未 resolve。
  - **When** aggregation daemon 第 31 天检查。
  - **Then** signal outcome 升 `not_retriable`，写入 `not_retriable.jsonl`，后续 run 永久跳过此 signal。

## Success Criteria

- [ ] `crates/ralph-core/src/feedback/` 新模块：`signal.rs` / `aggregator.rs` / `spec_writer.rs` / `scratchpad_injector.rs`。
- [ ] `feedback.jsonl` 与 `.ralph/feedback/history/` 目录 schema 与 R1.1 一致。
- [ ] outcome 升级逻辑（R2 / R3）单测覆盖 6 个 outcome 类。
- [ ] spec_writer AI 调用路径白名单强制（不写 presets/）。
- [ ] scratchpad 注入在下次 loop 启动时生效（不污染本次 loop 内的 agent context）。
- [ ] 反馈 CLI：`ralph feedback accept|defer|dismiss|resolved <signal_id>`。
- [ ] 文档：`.ralph/feedback/README.md` 说明"AI 不改 preset"硬约束 + "rebuild 冷却"现象。
- [ ] grand-lily 重放后：feedback.jsonl 至少 1 条 `escalated` signal + spec 草稿生成 + scratchpad 注入。
- [ ] `cargo test` 通过（`./scripts/run-tests.sh` 走完 nextest + doctest）。

## Scope Boundaries

### 包括（In Scope）

- 3 层反馈模型：evidence → aggregation → action
- `feedback.jsonl` 与 outcome 6 类
- 跨 iteration / 跨 run 聚合
- spec_writer 写 `.ralph/specs/`
- scratchpad 注入
- 人类 review CLI
- 与现有 telemetry / drift 集成

### 不包括（Out of Scope）

- **AI 直接修改 preset**：硬约束，由人类 review + 人类 commit + 人类 rebuild。
- **运行时重载 builtin preset**：bin 物理上不支持（`include_str!` 编译进），且会破坏"AI 自我强化循环"的冷却保护。
- **自动触发 cargo build / git commit / GitHub PR**：所有副作用操作由人类执行。
- **跨机器聚合**：本次只在单 repo 单 worktree 聚合，不做"全公司 preset 错误统计"。
- **AI 改进 prompt 模板**：AI 可在 spec 草稿中"建议改 prompt"，但 prompt 模板修改不在本次范围。
- **owner_hat 字段定义**（Doc 1）**与** payload schema（2026-06-02 doc）**与** terminal_event（Doc 2）**与** runtime topic_format（Doc 2）：本次依赖这些前置，本次**不**实现它们。

## Dependencies / Assumptions

- **Doc 1（preset 静态 lint）**：本 doc R3 / R6 依赖 `topic_owners` 字段已落地（用于 affected_owner_hats 字段填充）。若 Doc 1 未实现，本 doc 退化为 `affected_owner_hats: []`（仍可工作但路由盲）。
- **Doc 2（hat 生命周期契约）**：本 doc R2 / R3 依赖 `task.terminal_forced` 与 `repeated_stall` 事件已存在。recovery.jsonl 的 envelope_type 才能被聚合 daemon 识别。
- **2026-06-02 doc（payload 契约）**：本 doc R1 / R3 复用其 payload schema，spec_writer 生成的草稿引用 `event_policy.schemas` 中的 required_fields。
- **builtin preset `include_str!` 编译机制**（`presets.rs:35`）：本 doc R7 把这作为不可绕过的硬约束，不试图 runtime 补丁。
- **AI 子进程路径白名单**：依赖现有 spec_writer 的目录白名单机制（或新增），确保 AI 写 spec 时不污染 presets/。
- **人类 review 时延假设**：默认 7 天内 resolve；超过 30 天转 not_retriable。这两个阈值可通过 preset 配置覆盖，但默认走本次值。
- **假设 5 次 run 窗口内同 signal 出现 2 次 = 需要 escalate**：阈值可通过 preset 调，本次先写死。

## Sources / Research

- 现场证据 1：`.ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl` 12 事件（iter 2 / 5 / 8 / 9 / 11 各种 envelope）。
- 现场证据 2：`.ralph/diagnostics/2026-06-08T18-02-16/drift.jsonl` U5 drift 指标跌破 baseline。
- 现场证据 3：`.ralph/diagnostics/2026-06-08T18-02-16/trace.jsonl` hat 选择决策流。
- 现有 doc：`2026-06-08-preset-static-lint-requirements.md`（本系列 Doc 1，定义 `topic_owners` 字段）。
- 现有 doc：`2026-06-08-hat-lifecycle-contract-requirements.md`（本系列 Doc 2，定义 `task.terminal_forced` / `repeated_stall`）。
- 现有 doc：`2026-06-02-payload-contract-validation-requirements.md`（payload 字段 schema，spec 草稿引用）。
- 现有实现：`crates/ralph-core/src/diagnostics/`（runtime diagnostics，U5 drift 写入路径）。
- 现有实现：`crates/ralph-core/src/stall_tracker.rs`（在 Doc 2 规划中新增，本 doc 依赖其输出 `repeated_stall` 事件）。
- Ralph 原则 1-6（"Fresh Context Is Reliability"、"Backpressure Over Prescription"等）支持"反馈而非兜底"的方向。

## 实现计划指引

给后续 ce-plan 的参考信息。

### 修改文件列表

1. **`crates/ralph-core/src/feedback/mod.rs`**（新模块）
   - 4 个子模块：`signal` / `aggregator` / `spec_writer` / `scratchpad_injector`
2. **`crates/ralph-core/src/feedback/signal.rs`**
   - `pub struct Signal { signal_id, outcome, outcome_history, ... }`
   - `pub fn load_feedback_jsonl() -> Vec<Signal>`
   - `pub fn append_signal(signal: &Signal)`
3. **`crates/ralph-core/src/feedback/aggregator.rs`**
   - `pub fn aggregate_iteration(evidence: &Evidence) -> Vec<OutcomeTransition>`
   - `pub fn aggregate_cross_run(history_dir: &Path) -> Vec<OutcomeTransition>`
4. **`crates/ralph-core/src/feedback/spec_writer.rs`**
   - `pub fn write_spec_draft(signal: &Signal) -> Result<PathBuf>`
   - AI 调用路径白名单：只写 `.ralph/specs/`，写其他路径 panic
5. **`crates/ralph-core/src/feedback/scratchpad_injector.rs`**
   - `pub fn inject_signals(scratchpad: &mut String)`
6. **`crates/ralph-core/src/event_loop/mod.rs`**
   - 启动时调用 `aggregate_cross_run` + `inject_signals`
   - 每次 recovery 写入时调用 `aggregate_iteration`
7. **`crates/ralph-cli/src/feedback.rs`**（新文件）
   - `FeedbackCommands::Accept` / `Defer` / `Dismiss` / `Resolved` / `List`
8. **`crates/ralph-core/src/diagnostics/`**
   - 扩展 `ralph diagnose --include-feedback` 选项
9. **`.ralph/feedback/README.md`**（新文件）
   - 文档化"AI 不改 preset"硬约束 + "rebuild 冷却"现象 + 人类 review 流程
10. **`.ralph/specs/README.md`**（如不存在，新建）
    - 文档化 spec 草稿格式与 review checklist

### 测试策略

- **单元测试**：
  - `aggregator.rs`：6 个 outcome 类的所有转换路径
  - `spec_writer.rs`：路径白名单强制（写 presets/ 时 panic）
  - `signal.rs`：signal_id 生成的稳定性（同输入 → 同 ID）
- **集成测试**：
  - 模拟 grand-lily run 的 recovery.jsonl，验证 feedback.jsonl 写出 + spec 草稿生成
  - 模拟人类 `ralph feedback resolved` 后，aggregation 不再升级
  - 模拟 30 天时序（用 mock clock）验证 not_retriable 升级
- **冒烟测试**：
  - 跑一次 mini-ce-executor run（mock backend），人为注入 stall，验证 scratchpad 注入
  - 跑 `ralph feedback list` 列出所有 signal
  - 跑 `ralph feedback resolved` 后跑第二次 run，验证 signal 不再触发
- **审计测试**：
  - AI 子进程写 presets/ 路径时，spec_writer 立即 panic（不污染任何文件）
  - git pre-commit hook 拒 commit 修改 presets/ 的 PR（若 hook 已配置）

### 增量交付顺序

1. PR 1：feedback 模块骨架 + signal schema + 跨 iteration 聚合（不含 AI 调用）
2. PR 2：跨 run 聚合 + history 持久化
3. PR 3：spec_writer（AI 集成 + 路径白名单）
4. PR 4：scratchpad 注入 + 反馈 CLI
5. PR 5：文档 + 现有 preset 接入（feedback 默认启用）

## Outstanding Questions

- **OQ1（Resolve Before Planning）**：spec 草稿的 AI 生成是**同步阻塞** loop 启动，还是**异步后台**生成后下次 run 注入？
  - 方案 A：同步阻塞（loop 启动前等 spec 写完），优点是 spec 必现，缺点是增加 5-30s 启动时延
  - 方案 B：异步后台（loop 启动同时跑 spec_writer），优点是启动快，缺点是首次 run 可能没 spec 草稿可用
  - 倾向：方案 B + "scratchpad 提示 spec 草稿可能稍后生成"，给人类可观察的时序
- **OQ2（Resolve Before Planning）**：aggregation daemon 是 loop 内部常驻还是外部 cron？
  - 方案 A：loop 内部（每条 evidence 触发同步聚合），简单但耦合
  - 方案 B：loop 启动/关闭时跑批处理，解耦但有"loop 跑一半时 evidence 不聚合"窗口
  - 倾向：方案 A，但聚合计算限制在 ≤ 50ms / evidence（不阻塞 loop 主路径）
- **OQ3（Deferred to Planning）**：spec_writer 调 AI 时用什么 prompt？是直接用 scratchpad signal 喂给通用 LLM，还是用专门的 spec 写作 prompt 模板？模板若维护，归属哪个 doc？
- **OQ4（Deferred to Planning）**：跨机器 / 跨 worktree 聚合（不在本次范围）何时立项？可能是 v2 doc。
