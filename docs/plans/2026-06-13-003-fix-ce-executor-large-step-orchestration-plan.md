---
status: active
date: 2026-06-13
type: fix
origin: docs/report/2026-06-13-ce-executor-isolated-wave-not-firing-u2-stuck-diagnosis.md
preset: ce-executor-isolated
---

# fix: ce-executor-isolated 大单步编排 — large 强制 sub-task 拆分 + executor skip detection

## Summary

修复 `ce-executor-isolated` preset 的 **P0 编排 gap**：`complexity: large` 的单 Implementation Unit 被整包下发给 executor，agent 单 iteration 无法完成也不 emit `work.done`，导致 review wave 链路从未启动。

本计划 **只改 preset 编排 + 契约测试**，不手动补 U2 业务代码、不改 EventBus / stall recovery 基座（P1 机制补强另开计划）。

预期工作量：**1 个 PR、~2–4 小时**，以 YAML 指令 + `presets.rs` 文本契约测试为主。

---

## Problem Frame

### 现象

loop `2026-06-10-003-...-crisp-wren` 在 U2 卡死：

- coordinator emit `work.ready`(step-02, `complexity: large`) 后，events **无** `work.done`
- `review.wave.ready` **0 次** — wave 未启动是下游症状，非 wave 回归
- executor 反复追加 `HUMAN GUIDANCE` 到 scratchpad，task 始终 `open`
- U2 有部分未跟踪文件变更，但 **无 commit、无终态事件**

### 根因（编排）

| 层级 | 结论 |
|------|------|
| **P0 编排** | `Task Split Heuristics`（L310–337）默认「1 U = 1 task」，**缺少** `large` + 高行数时的强制 sub-task 拆分；coordinator 对 11606 行拆分仍建单 task |
| **P0 编排** | executor 无 **skip detection**：多轮激活仍 open、无 commit 时，不强制 emit `work.failed`，链路无限空转 |
| P1 机制 | hard gate 只注入 `human.guidance`，agent 可继续不写终态 — **本计划不覆盖** |

### 与历史 wave 修复的关系

2026-06-08 wave 短路、2026-06-09 batch emit、2026-06-12 plan-gate dispatch gap 均已修复。**本次故障层在 executor 终态之前**，上述修复不触发。

---

## Requirements

| ID | 需求 | 来源 |
|----|------|------|
| R1 | `complexity: large` 且单 U 预估变更 > 800 行（或 plan 明示 > 10 文件 / 跨 crate 大 refactor）时，coordinator **必须**拆成多个 sub-task（每 task < 500 行量级），仅对第一个 sub-task emit `work.ready` | 诊断 §6.2.1 |
| R2 | sub-task 使用稳定 key：`ce-executor:{plan_name}:step-NN:uN-a-{slug}` … `uN-e-{slug}`；每个 sub-task 独立 `task_id`，executor 每 iteration 只完成 **一个** sub-task 并 emit 一次 `work.done` | R1 衍生 |
| R3 | step 内 sub-task 顺序推进：executor 完成 sub-task A → `work.done` → review → plan-gate；plan-gate **不** advance 到下一 plan step，直到当前 step 全部 sub-task 完成（沿用现有 plan-gate 语义，sub-task 属于同一 `step-NN`） | 编排一致性 |
| R4 | executor 在同一 `task_id` 上 **≥3 次连续激活**且 task 仍 `open`、自上次 `work.ready` 以来 **无新 commit** 时，**必须** emit `work.failed`，`reason` 含 `executor stuck` | 诊断 §6.2.2 |
| R5 | executor **禁止**将 `human.guidance` / scratchpad 追加当作 iteration 主产出；guidance 仅作参考，不能替代 implement + commit + 终态 emit | 诊断 §4.1 causal step 2 |
| R6 | 中英文 preset 内容 parity；embedded preset 与 canonical YAML 一致 | CLAUDE.md preset 四件套 |
| R7 | `presets.rs` 新增文本契约测试，防止回归 | 现有 wave batch / preflight 测试模式 |

### 非目标

- 不改 `crates/ralph-core/src/event_loop/` stall escalation 路由（P1，另计划）
- 不加「git 有变更自动 emit work.done」基座机制（P1）
- 不修复 crisp-wren worktree 内 U2 未提交代码（用户明确不做手动补代码）
- 不做 8-step 全 plan E2E live API dogfood（可选手动验收，不阻塞 merge）

---

## Key Technical Decisions

### KTD-1：拆分阈值用行数 + complexity，不用 token 计数

**决定**：coordinator 读 plan IU 的 `Files` / `Approach` / `Verification` 段估算 `estimated_changed_lines`；`complexity: large` **且** `estimated_changed_lines > 800` → 强制拆 sub-task，每个 sub-task 目标 < 500 行。

**理由**：preset 指令层无法读 token meter；行数是 plan 文档已有信号（crisp-wren U2 明确 11606 行）。与诊断报告 C1 假设一致。

**备选否决**：纯文件数阈值 — U2 是单文件 mega-test，文件数不够敏感。

### KTD-2：sub-task 拆分模板写死在 coordinator instructions，不新增 CLI

**决定**：在 `### Task Split Heuristics` 后新增 `### Sub-task Decomposition (large complexity — HARD RULE)`，给出 5 步模板（scaffold → extract A → extract B → … → delete original）。

**理由**：Ralph tenet — 编排层 thin，agent 执行拆分逻辑；与现有 `ralph tools task ensure` 流程一致。

### KTD-3：skip detection 由 executor 自计 activation，不写新 runtime 字段

**决定**：executor 在 `decisions.md` 或 iteration 开头读 `progress.md` / git log，维护 mental counter；第 3 次无 commit 则 `work.failed`。

**理由**：零基座改动；与现有 `work.failed` → plan-gate trigger 链兼容。

**风险**：agent 可能不计数 — mitigated by R7 契约测试 + R4 指令 HARD RULE 措辞。

### KTD-4：plan-gate step advance 时 coordinator 仍负责拆 sub-task

**决定**：在 coordinator 的 `### Event Publishing` / step-advance 路径补充：每次为 **新 step** 创建 runtime task 前，先跑 Sub-task Decomposition 规则（不仅限于 `work.start` 首次）。

**理由**：crisp-wren 的 U2 task 是 step-02 `work.ready` 时创建，非 loop 冷启动。

---

## High-Level Technical Design

### 编排修复后的 U2 期望链路（对比）

```text
[Before — crisp-wren 实际]
coordinator → work.ready(step-02, 1 task, 11606 lines)
executor × N → scratchpad guidance 循环 → 无 work.done → wave 永不启动

[After — 本计划]
coordinator → work.ready(step-02-u2a, sub-task 1/5, ~200 lines)
executor → commit → work.done
review-coordinator → review.wave.ready → dimension-reviewer × N → ...
plan-gate → queue.advance (同 step 内) OR 下一 sub-task work.ready
... 重复至 step-02 全部 sub-task 完成 → plan-gate advance step-03
```

### Sub-task 与 review 边界

每个 sub-task 完成后走 **完整 review wave**（与现有单 U 行为一致）。接受略增 review 次数，换取 executor 可完成性。若后续需「step 级单次 review」，列为 follow-up（Deferred）。

---

## Implementation Units

### U1. Coordinator — large 强制 sub-task 拆分指令

**Goal**：R1、R2、KTD-1、KTD-2、KTD-4

**Dependencies**：无

**Files**：
- `presets/en/ce-executor-isolated.yml`（coordinator `instructions`，约 L287–365 区）
- `presets/zh/ce-executor-isolated-zh.yml`（mirror）

**Approach**：

1. 在 `### Complexity Assessment`（L287–290）补充：`large` 必须估算 `estimated_changed_lines`，写入 `context.md`。
2. 在 `### Task Split Heuristics`（L310–337）**之后**插入新段 `### Sub-task Decomposition (large complexity — HARD RULE)`：
   - 触发条件：`complexity: large` AND (`estimated_changed_lines > 800` OR plan IU 描述含「split / 拆分 / N subfiles」且总行数 > 800)
   - 动作：拆成 3–7 个 sub-task，每个 Goal 含明确文件列表与行数上限（< 500 行变更）
   - key 格式：`ce-executor:{plan_name}:step-NN:u{N}{letter}-{slug}`（例：`step-02:u2a-scaffold`）
   - 仅 `ralph tools task ensure` **第一个** sub-task；其余 sub-task 在本 step 内由 plan-gate/coordinator 在上一 sub-task review 通过后创建（或一次性 ensure 全部但只 handoff 第一个 — **推荐一次性 ensure 全部**，executor 仍 ONE task per iteration）
   - `work.ready` payload 的 `step` 字段用 `step-02-u2a` 形式，`preflight_checks` 列出 sub-task 专属前置
3. 在 `### Runtime Task Creation`（L301–308）加一句：large 强制拆分规则优先于「one task per U」默认。
4. 反模式：禁止对 >800 行 large U 创建单个 runtime task（与 L330–336 anti-pattern 对称）。

**Patterns to follow**：现有 `Task Split Heuristics` 结构与 `Preflight Contract` HARD RULE 语气。

**Test scenarios**（U3 实现，此处为验收标准）：
- coordinator instructions 含 `Sub-task Decomposition` + `800` + `500`
- 含禁止整包 large U 的 negative marker
- zh 变体 parity

**Verification**：
- `grep -n "Sub-task Decomposition" presets/en/ce-executor-isolated.yml` 有命中
- 人工读 crisp-wren U2 IU：按新规则应产出 ≥3 个 sub-task 描述

---

### U2. Executor — skip detection + 反 guidance 空转

**Goal**：R4、R5、KTD-3

**Dependencies**：U1（语义一致，可并行）

**Files**：
- `presets/en/ce-executor-isolated.yml`（executor `instructions`，约 L425–545 区）
- `presets/zh/ce-executor-isolated-zh.yml`

**Approach**：

1. 在 `### Events You MUST and MUST NOT Emit`（L527 前）插入 `### Skip Detection (HARD RULE)`：
   ```
   每次激活开始时：
   1. 记录 triggering task_id 与 git rev-parse HEAD
   2. 若与上一轮相同 task_id、task 仍 open、HEAD 未变 → 计数 +1
   3. 计数 ≥ 3 → 必须 ralph emit work.failed，payload 含 task_id, task_key, reason: "executor stuck: N consecutive no-op activations without commit"
   4. 禁止通过追加 scratchpad / HUMAN GUIDANCE 作为本轮唯一产出
   ```
2. 在 `### Failure Handling`（L532）交叉引用 skip detection。
3. 在 `### Constraints`（L537）强调：**Terminal emit 优先于一切文件编辑**；iteration 最后一步必须是 `ralph emit work.done` 或 `work.failed`。

**Patterns to follow**：`Commit Cadence (HARD RULE)`、`Preflight Contract (HARD RULE)` 段落结构。

**Test scenarios**：
- 含 `Skip Detection (HARD RULE)` + `consecutive` + `work.failed` + `executor stuck`
- 含禁止「仅写 scratchpad/guidance 而不 emit 终态」的 negative marker

**Verification**：
- executor 指令可读性：另一开发者 5 分钟内能说出「3 次无 commit 怎么办」

---

### U3. Preset 契约测试（presets.rs）

**Goal**：R7

**Dependencies**：U1、U2

**Files**：
- `crates/ralph-cli/src/presets.rs`

**Approach**：

1. 新增 helper `coordinator_instructions_from()` — 镜像 `executor_instructions_from()`（L1698–1707）。
2. 新增测试：

| 测试名 | 断言 |
|--------|------|
| `test_ce_executor_coordinator_large_step_subtask_decomposition` | coordinator 含 `Sub-task Decomposition`；含 `800` 与 `500`；含 `estimated_changed_lines` |
| `test_ce_executor_coordinator_forbids_whole_large_unit_task` | coordinator 含整包 large U 禁止语义（如 `MUST NOT create a single runtime task` 或等价） |
| `test_ce_executor_executor_skip_detection_hard_rule` | executor 含 `Skip Detection (HARD RULE)` + `work.failed` + `executor stuck` |
| `test_ce_executor_executor_forbids_guidance_only_iterations` | executor 禁止仅 scratchpad/guidance 产出（negative assert） |
| `test_ce_executor_root_preset_matches_embedded` | 已有 — 确保仍 pass |

3. 可选：extend `test_ce_executor_has_preflight_contract` 无关，保持独立。

**Execution note**：先写 failing test（删一段预期 marker）确认红灯，再恢复 preset 内容转绿。

**Test scenarios**：
- **Happy path**：preset 含全部 marker → 5 个 test pass
- **Regression**：故意删除 `800` → `test_ce_executor_coordinator_large_step_subtask_decomposition` fail
- **Mirror drift**：改 en 不改 zh → 若未来加 zh 专项测试则 fail（当前以 embedded match 为准）

**Verification**：
```bash
cargo test -p ralph-cli test_ce_executor_coordinator_large_step
cargo test -p ralph-cli test_ce_executor_executor_skip_detection
cargo test -p ralph-cli test_ce_executor_root_preset_matches_embedded
```

---

### U4. Preset 四件套同步 + lint 门禁

**Goal**：R6

**Dependencies**：U1–U3

**Files**：
- `presets/manifest.yml`（仅当 preset 名变更时 — **本次不变**）
- `crates/ralph-cli/src/presets.rs`（`PRESETS` 数组 — content 来自 build.rs，**不手改 content**）
- `presets/index.json`（不变）
- `scripts/ralph-zsh-plugin.zsh`（不变）
- `CLAUDE.md` / `AGENTS.md`（不变 — 无新 preset 名）

**Approach**：

1. 仅改 `presets/en/*.yml` + `presets/zh/*-zh.yml`；`cargo build -p ralph-cli` 触发 build.rs embed。
2. 跑 preset lint：
   ```bash
   cargo run -p ralph-cli -- preset check -H builtin:ce-executor-isolated
   cargo run -p ralph-cli -- preflight -H builtin:ce-executor-isolated -p "smoke" --dry-run 2>/dev/null || true
   ```
3. WAC Tier-0：`ce-executor-isolated` 须保持 WAC-clean（`TIER_0_WAC_PRESETS`）。

**Test scenarios**：
- preset check exit 0
- `cargo test -p ralph-cli presets::tests` 全绿

**Verification**：CI 等价于 `./scripts/run-tests.sh` 或 `cargo test -p ralph-cli presets::`

---

### U5. Dogfood 验收剧本（crisp-wren 场景复现）

**Goal**：端到端验证 R1–R5 在真实编排下 unblock wave，**不提交 U2 业务代码**

**Dependencies**：U1–U4 merged 或 branch 上 built binary

**Files**（只读验收，不改）：
- `.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-crisp-wren/`
- `docs/report/2026-06-13-ce-executor-isolated-wave-not-firing-u2-stuck-diagnosis.md`

**Approach — 验收步骤**：

#### 5.1 环境准备

```bash
# 1. 停 orphan loop
ralph loops stop 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-crisp-wren

# 2. 清理 U2 半拉子未跟踪文件（避免干扰计数）
cd .worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-crisp-wren
git clean -fd crates/ralph-cli/src/loop_runner/tests/ 2>/dev/null || true

# 3. 重置 U2 task（可选：删 open task 让 coordinator 重建 sub-tasks）
# 手工或 ralph tools task 操作 — 记录初始 tasks.jsonl 行数
```

#### 5.2 启动 loop（修复后 preset）

```bash
RALPH_DIAGNOSTICS=1 cargo run -p ralph-cli -- run \
  -H builtin:ce-executor-isolated \
  -P "Resume plan from U2 with sub-task decomposition" \
  --worktree \
  --max-iterations 30
```

#### 5.3 断言清单（30 分钟内应观察到）

| # | 检查项 | 命令 / 位置 | 期望 |
|---|--------|-------------|------|
| V1 | U2 拆成多个 task | `.ralph/agent/tasks.jsonl` | ≥3 条 key 含 `step-02:u2` 或 `u2a/u2b/...` |
| V2 | 首个 sub-task handoff | `events*.jsonl` | `work.ready` 的 `step` 含 `u2a` 或 sub-task slug，非整包 `split-loop-runner-tests` 单 task |
| V3 | executor 终态 | `events*.jsonl` | 第一个 sub-task 完成后 **30s 内** 出现 `work.done`（hat=executor） |
| V4 | wave 启动 | `events*.jsonl` | `work.done` 后 **60s 内** 出现 `review.wave.ready` |
| V5 | 无 guidance 空转 | `.ralph/agent/scratchpad.md` | 无新增 ≥3 条同模式 `HUMAN GUIDANCE` 而 task 仍 open |
| V6 | skip detection 兜底 | `recovery.jsonl` | 若 mock 空转 3 轮，应出现 `work.failed` 而非无限 loop |
| V7 | loop 非 orphan | `ralph loops list` | status 非 orphan，或正常完成退出 |

#### 5.4 失败分流

| 失败症状 | 可能原因 | 下一步 |
|----------|----------|--------|
| 仍单 task 11606 行 | U1 指令不够 HARD / coordinator 未读 plan | 加强 coordinator 反模式 + 加 `preflight_checks` 示例 |
| 有 work.done 无 wave | review-coordinator 层（非本计划） | 查 2026-06-08 修复是否回归 |
| 3 轮后仍无 work.failed | U2 指令被忽略 | 升 P1：基座强制终态（另计划） |

**Verification 完成标准**：V1–V4 全部 pass 即宣告编排 fix 有效；V5–V7 为加固项。

---

## Verification Matrix（CI + 本地）

| 阶段 | 命令 | 期望 | 阻塞 merge |
|------|------|------|-----------|
| 单元/契约 | `cargo test -p ralph-cli test_ce_executor_coordinator_large_step` | PASS | ✅ |
| 单元/契约 | `cargo test -p ralph-cli test_ce_executor_executor_skip_detection` | PASS | ✅ |
| 单元/契约 | `cargo test -p ralph-cli presets::tests` | PASS | ✅ |
| 全库 | `./scripts/run-tests.sh` 或 `cargo test --workspace --exclude ralph-e2e` | PASS | ✅ |
| Lint | `cargo clippy --workspace -- -D warnings` | PASS | ✅ |
| Preset | `cargo run -p ralph-cli -- preset check -H builtin:ce-executor-isolated` | exit 0 | ✅ |
| Dogfood | U5 剧本 V1–V4 | 人工 | ⚠️ 强烈建议，不阻塞若 CI 绿 |

---

## Risks & Dependencies

| 风险 | 缓解 |
|------|------|
| Agent 无视 sub-task 规则 | HARD RULE + presets.rs 契约测试 + dogfood V1 |
| Sub-task 过多导致 review 次数膨胀 | 接受；follow-up 可做 step-level review |
| zh preset 漂移 | U4 embedded match test |
| skip detection 依赖 agent 自计数 | dogfood V6；长期 P1 基座计数 |

---

## Deferred to Follow-Up Work

- P1：stall hard-escalation 路由到 coordinator / 自动 `work.failed`（`event_loop/mod.rs`）
- P1：git 产出与终态事件绑定（runner 层）
- P2：TUI「Awaiting executor terminal event」状态条
- P2：`docs/solutions/` 沉淀 crisp-wren learning（`/ce-compound`）
- BDD：`ce_executor_step_bridge.yml` integration scenario（solutions 建议，非本 PR 必需）

---

## Open Questions

| 问题 | 状态 | 决策 |
|------|------|------|
| sub-task 是一次 ensure 全部还是逐个创建？ | **已决** | 一次 ensure 全部，executor 仍 one task per iteration（KTD-2） |
| 每个 sub-task 都跑 full wave review？ | **已决** | 是，先求正确性（HTD） |
| 800/500 阈值是否可配置？ | Deferred | 先写死在 instructions，后续可提取到 preset 顶层 config |

---

## Sources & Research

- `docs/report/2026-06-13-ce-executor-isolated-wave-not-firing-u2-stuck-diagnosis.md` — 主诊断
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` — dispatch gap 已修复，本计划不重复
- `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` — 契约测试模式参考
- `presets/en/ce-executor-isolated.yml` L287–365, L425–545
- `crates/ralph-cli/src/presets.rs` L950–1028, L1698–1780
