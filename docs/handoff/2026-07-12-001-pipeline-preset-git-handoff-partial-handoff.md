# Handoff — 2026-07-12 上半 session: pipeline preset Git handoff 协议（部分完成）

## 任务摘要

- **来源 plan**：`docs/plans/2026-07-12-001-fix-pipeline-preset-git-handoff-precheck-plan.md`
- **本次 session 目标**：执行该 plan 的 U1（普通版 pipeline preset Git handoff 协议文本落地）以及其余 U2–U8 中能在单 session 内完成的部分。
- **session 编号**：2026-07-12 上半 session（上午时段）。
- **用户最终选择**：完整执行全部 U1–U8，不做 scope 削减。
- **实际完成态**：因单 session 工作量超过容量，仅完成 U1 的**普通版部分**（`presets/en/ce-executor-pipeline.yml`）落地，未跑任何 `cargo nextest run` / `preset_lint` 验证。loop preset（U1 剩余）、U3 结构性 schema 改动、U5、U6、U7、U8 均未触碰。
- **用户**已同意将剩余工作交接给下一个 agent（plan 002 阶段）。

## 已完成的工作（current state）

### 文件改动清单（本 session 仅修改 1 个文件）

- **修改**：`/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-pipeline.yml`
- **diff 概览**：`352 lines changed, 339 insertions(+), 13 deletions(-)`（来自 `git diff --stat HEAD`）
- **未修改**：`presets/en/ce-executor-pipeline-loop.yml`、`presets/schemas/*`、`crates/ralph-core/tests/scenarios/*`、`skills/ralph-preset-common/*`、`crates/ralph-core/data/ralph-tools*.md`、`crates/ralph-cli/src/presets.rs`

### `ce-executor-pipeline.yml` 详细改动

#### 1. executor（写入型 hat，约 line 834–901）

- 在 HARD RULES 块后新增 `### Final Git Handoff Precheck (applies to EVERY `work.done` emit)` 章节
- 明确两阶段门：
  - **Stage A — Pre-policy-check**：porcelain filter 检查（`git status --porcelain --untracked-files=all | awk '!/^\?\? \.ralph\// && !/^\?\? \.ralph$/'`，排除 `.ralph/` runtime 产物）；通过后计算 `FINAL_HEAD` / `commit_count` / `changed_lines`；确认 `.ralph/review/{plan_name}/review.diff.patch` 在位
  - **Stage B — Post-policy-check**：在真正 `ralph emit` 之前再次检查 HEAD 未变且仍 clean
- 明确归属判定：可归属本 hat 的可保留修改 → stage + commit under U-ID subject → 重新计算 SHA；不可安全归属 → 停止 emit，记入 `.ralph/agent/decisions.md`，发 `work.failed`
- **Skip-path note**：Step 1.5 flow-audit skip 也必须通过相同 Stage A / Stage B 检查，不能因为"无新 Unit"跳过交接门

#### 2. fixer（写入型 hat，约 line 2616 起）

- 同样在 HARD RULES 块后新增 `### Final Git Handoff Precheck (applies to EVERY `fix.done` emit)` 章节
- 强调 empty-plan fast path 也必须通过 clean/HEAD 检查（不创建 commit，但 HEAD 不变且 worktree clean）
- 必须产出 `head_sha` / `worktree_status` 字段以供 alignment 验收 fixer final HEAD
- fabricate clean 是 contract violation（不允许伪造 `worktree_status: clean`）
- 与 executor 对称的两阶段门

#### 3. 6 个 dim hats（只读 hat）

每个 dim hat（`dim:goal-alignment` / `dim:correctness` / `dim:testing` / `dim:maintainability` / `dim:project-standards` / `dim:adversarial`）均新增两块：

- **`### Entry Precheck (HARD RULE — read-only proof of clean handoff)`** 4 步：
  1. `expected_head_sha` 校验：`expected_head_sha = executor_head_sha`（从 trigger payload 取），`actual_start_head_sha = git rev-parse HEAD`，不匹配则记录 SHA mismatch 到 `.ralph/agent/decisions.md`，停止 audit，不 reset/restore/clean，stop with no emit
  2. Porcelain filter 检查：使用与 executor 相同的 filter 命令；任何非 `.ralph/` dirty 都是上游 carryover；**reviewer 不得 commit / restore / stash / reset**
  3. 保存起始证据到 `.ralph/review/{plan_name}/git-state-<dim>-start.txt`（含 `expected_head_sha` / `actual_start_head_sha` / porcelain 输出 / `review_round` / dimension / trigger topic）
  4. 入口失败时记入 `handoff_precheck_failed` 证据，**不**重新 emit 前一维的 done event
- **`### Step 5 — Exit Precheck and emit event`**：
  1. **Stage A** — pre-policy-check：HEAD 等于 `actual_start_head_sha` 且 porcelain filter 仍空（除 `.ralph/`）；不可执行 `git add` / `git commit` / `git restore` / `git stash` / `git reset`
  2. `ralph emit --policy-check --triggered dim:<dim> ...`
  3. **Stage B** — pre-real-emit：再次 HEAD + clean 检查；变化则 rebuild payload
  4. 保存 end 证据 `.ralph/review/{plan_name}/git-state-<dim>-end.txt`

#### 4. alignment（只读 hat）

- 新增 `### Entry Precheck (HARD RULE — read-only proof of clean handoff)`
  - **特殊说明**：alignment 入口 expected SHA 使用 `fix.done.head_sha`（如果有 fix commit）作为 `expected_head_sha`，普通版若 fix 未产生 commit 则透传 `executor_head_sha`
- 新增 `### Step 5 — Exit Precheck and emit event`
  - 与 dim hats 对称的两阶段门，证据文件命名 `git-state-alignment-{start,end}.txt`
  - payload 中 pass-through `review_verdict`、`fix_plan_file`、fixer 提供的最终 SHA 字段

## 未完成的工作（pending）

### U1（loop preset 部分）

- **`presets/en/ce-executor-pipeline-loop.yml`** 未编辑：executor / fixer / `review-reentry` / 6 dim / alignment 都还没有对应的协议块
- 注意 loop 版的 6 dim 入口 expected SHA 应使用 `round_base_sha` 而非 `executor_head_sha`（计划要求在 U5 统一处理），本 session 未做这个区分

### U2（executor 加固细节）

- 普通版已部分覆盖（Final Git Handoff Precheck 已落地）
- loop 版未覆盖

### U3（fixer 加固 + **结构性改动**）

- 普通版已部分覆盖（Final Git Handoff Precheck 已落地）
- **未完成（结构性）**：普通版 `fix.done` inline schema 需要新增最小字段 `head_sha` / `worktree_status` / `fix_attempt_commit_sha`
  - 当前普通版 `fix.done` schema（在 `presets/en/ce-executor-pipeline.yml` 的 `event_policy.schemas.fix.done` 段）缺这三个字段
  - 必须同步：
    1. inline schema 的 `required_fields` 列表
    2. BDD scenario payload（见 U6）
    3. alignment 步骤的引用（alignment 现在依赖 fixer final HEAD）

### U4（普通 pipeline reviewer 入口/出口证明）

- 普通版已覆盖（6 dim + alignment 全部加了 Entry/Exit Precheck）
- **下一步**：把证据文件路径结构（`.ralph/review/{plan_name}/git-state-<dim>-{start,end}.txt`）、Start/End 命名约定写入 README / operator skill（U7 时同步）

### U5（loop preset round-aware 检查）

未完成。需要：

1. `review-reentry` 自身新增 Entry / Exit precheck
2. loop preset 6 dim + alignment 入口 expected SHA 切换为 `round_base_sha`
   - 第 1 轮 `round_base_sha == executor_head_sha`
   - 第 2+ 轮 `round_base_sha == fix.done.head_sha`
3. start/end evidence 文件路径带 `round-<NN>` 维度（避免多轮覆盖）

### U6（BDD scenario 同步）

未完成。需要修改以下 6 个文件：

- `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_blocked.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_max_round_blocked.yml`
- `crates/ralph-core/tests/scenarios/ce_executor_plan_blocked.yml`

主要改动：普通版 `fix.done` mock payload 新增 `head_sha` / `worktree_status` / `fix_attempt_commit_sha` 字段；loop `fix.done` 字段保持但确认 parity；Round 2 review 时 `round_base_sha = fix.done.head_sha`。

**严格遵守**：所有 BDD 必须继续使用 `run_workflow_guard_scenario`（真 EventLoop runner，断言 events），**禁止**退化到 stub（`run_scenario` stub 只查 iterations 数，会静默吞掉拓扑失配——参见 CLAUDE.md HARD RULE 关于 2026-06-24 P0-2/P0-3 根因的描述）。

### U7（operator skills + agent skill guides 审计）

未完成。需要修改：

- `skills/ralph-preset-common/references/author-checklist.md`（新增通用审查项：写入型 hat clean + commit 后交接；只读型 hat 校验 trigger SHA + 保存 start evidence + 出口证明未改变）
- `skills/ralph-preset-common/references/patterns.md`
- `skills/ralph-preset-common/references/finding-rubric.md`（如果有 finding mapping 变化）
- `skills/ralph-preset-common/references/agent-native-model.md`（如果 handoff visibility 需要说明）

反向审计（review，不必然修改）：

- `skills/ralph-preset-common/references/commands.md`
- `crates/ralph-core/data/ralph-tools.md`
- `crates/ralph-core/data/ralph-tools-emit.md`
- `crates/ralph-core/data/ralph-tools-opac.md`

### U8（preset lint + BDD + 全 workspace nextest + doctest + CLI doc drift）

未完成。**严格遵守 CLAUDE.md 硬规则**：

- **禁止**裸跑 `cargo test -p ralph-cli` 或 `cargo test -p ralph-cli --bin ralph`（HARD RULE 1：根因是 `crates/ralph-cli/src/loop_runner/tests.rs:14-49` 的 4 个 process-global Mutex + 时间敏感测试）
- 必须用：
  - `./scripts/run-tests.sh`（全 workspace 推荐入口）
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-core -- preset_lint`
  - `cargo nextest run -p ralph-cli --bin ralph -- presets`
  - `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline[_loop][_fix_reentry]`
  - `cargo test --workspace --exclude ralph-e2e --doc`（仅 doctest）
  - `scripts/check-cli-doc-drift.sh`
- 如出现竞态/时序 flake，强制走 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底（**仅作为 flake 兜底，不是默认路径**）

## 已落地的设计决策（写明不重新发明）

### D1：写入型 hat 负责提交，只读 hat 只负责验收

executor / fixer 是唯一能判断本 activation 产物是否应保留、是否通过测试的角色。Reviewer 看到脏状态时不得替上游提交，否则会把未知修改合法化。已落地到 ce-executor-pipeline.yml 的 Entry Precheck 第 3 步。

### D2：clean 的 porcelain filter

```bash
git status --porcelain --untracked-files=all \
  | awk '!/^\?\? \.ralph\// && !/^\?\? \.ralph$/ { print }'
```

覆盖 staged / unstaged / tracked 删除/rename / untracked，排除 `.ralph/` runtime 产物。已落地到 ce-executor-pipeline.yml 的 Final Git Handoff Precheck Stage A 和所有 Entry Precheck 第 3 步。

### D3：起始证据最小快照

- `expected_head_sha`
- `actual_start_head_sha`
- 规范化入口 status 输出
- activation/round/dimension 标识
- 写入 `.ralph/review/.../git-state-<dim>-{start,end}.txt`

证据不进入 Git diff（在 `.ralph/` 下）。已落地到 ce-executor-pipeline.yml 的所有 Entry Precheck 第 4 步 + Step 5 Exit Precheck 第 4 步。

### D4：两个 preset 的 expected SHA 来源不同

- 普通 pipeline：所有 dim reviewer 用 `expected_head_sha = executor_head_sha`
- 普通 pipeline alignment：若 fixer 有 commit，用 `fix.done.head_sha`；无 fix commit 的 fast path 用 `executor_head_sha`
- loop 第 1 轮：`round_base_sha == executor_head_sha`（U5 时实现）
- loop 第 2+ 轮：`round_base_sha == fix.done.head_sha`（U5 时实现）

**当前状态**：普通版已落地 D4 的第一/二项；loop 版的第三/四项在 U5 阶段实现。

### D5：失败语义沿现有终态

- executor 失败：`work.failed`（沿用现有事件）
- fixer 失败：`fix.done{fix_status: partial|blocked, worktree_status: clean}`；连 clean 都无法保证时**不得伪造** `worktree_status: clean`
- reviewer 失败：记录 `handoff_precheck_failed` 证据，停止 audit，不重发前一维事件
- 不新增 recovery topic；不新增 handoff-cleaner / workspace-steward hat

已落地到 ce-executor-pipeline.yml 的 Final Git Handoff Precheck 失败路径 + Entry Precheck 失败路径。

## 验证状态

- **未跑任何验证**（`cargo nextest run` / `cargo test` / `preset_lint` / `scripts/check-cli-doc-drift.sh` 全部未执行）
- `presets/en/ce-executor-pipeline.yml` 已编辑保存；未跑 preset_lint 验证 YAML 仍可解析、schema parity 仍成立、topology 仍合法
- `presets/en/ce-executor-pipeline-loop.yml` 未编辑
- BDD / schema / skills / ralph-tools docs 全未触动

**风险**：因为 YAML 改动较大（339 行新增），未跑 preset_lint 时**不能确认**是否触发了任何 finding（如 event_policy drift、required_fields drift 等）。下一个 session 必须先跑 targeted preset_lint 再继续。

## 下一步具体行动（给 plan 002 的 agent）

1. **先读 plan 001 文件**：`docs/plans/2026-07-12-001-fix-pipeline-preset-git-handoff-precheck-plan.md`，完整理解 8 个 unit 的依赖关系与 SSOT 同步清单（参见 CLAUDE.md HARD RULE "preset/schema 改动后的下游同步清单"）。

2. **先跑 targeted 验证确认普通版 base 完好**：
   ```bash
   cargo nextest run -p ralph-cli --bin ralph -- preset_lint
   cargo nextest run -p ralph-core -- preset_lint
   cargo nextest run -p ralph-cli --bin ralph -- presets
   ```
   如果报错，先修复本 session 留下的 preset_lint 违规，再继续。

3. **U1 loop preset 部分**：把普通版已落地的协议块同步到 `presets/en/ce-executor-pipeline-loop.yml`：
   - executor / fixer 加 Final Git Handoff Precheck
   - 6 dim + alignment 加 Entry Precheck + Step 5 Exit Precheck
   - `review-reentry` 暂用 `executor_head_sha` 作 expected SHA，U5 时再切到 `round_base_sha`

4. **U3 结构性改动**：
   - 在 `presets/en/ce-executor-pipeline.yml` 的 `event_policy.schemas.fix.done.required_fields` 中追加：
     - `head_sha`
     - `worktree_status`
     - `fix_attempt_commit_sha`
   - 同步 alignment hat 的 references（说明使用 fixer final HEAD）

5. **U5 loop round-aware**：
   - `review-reentry` hat 加 Entry / Exit precheck（明确建立 `round_base_sha`，首轮 = `executor_head_sha`，fix 后 = `fix.done.head_sha`）
   - loop 6 dim + alignment 入口 expected SHA 切到 `round_base_sha`
   - start/end evidence 文件路径带 `round-<NN>` 维度

6. **U6 BDD 同步**：按上节"未完成的工作 — U6"列出的 6 个文件修改 mock payload；**必须用 `run_workflow_guard_scenario`**

7. **U7 operator skills**：按上节"未完成的工作 — U7"列出的文件修改 + 反向审计

8. **U8 全量验证**：按 CLAUDE.md 硬规则跑测试序列：
   - targeted preset_lint / scenarios（先确认核心路径通过）
   - `./scripts/run-tests.sh`（最终验证）
   - 如出现竞态/时序 flake，强制 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底

## 风险 / 警告

1. **不允许新增"大段 preset 文案 byte-equality 锁定"测试**（plan 001 Out of Scope 第 61 行 + CLAUDE.md Preset 测试规则 HARD RULE）。测试应只覆盖结构化语义（schema / required_fields / event topology / BDD），不锁定 instructions 文本。

2. **不修改 `crates/ralph-core/src/event_loop/mod.rs` 的 `audit_file_modifications`**（plan 001 Out of Scope 第 56 行）。runtime 全局 dirty 审计是已知残余风险，preset-only 方案不修 runtime。

3. **不让 reviewer 运行 `git add` / `git commit` / `git restore` / `git stash` / `git reset`**（plan 001 Out of Scope 第 59 行）。已落地到 Entry Precheck 与 Step 5 Exit Precheck；下一个 agent 不要在 U7 文档里说"reviewer 可在紧急情况下 commit"。

4. **`.ralph/` 是 runtime 输出区，不能 commit**。已落地；evidence 文件路径全在 `.ralph/review/...` 下，不污染 Git diff。

5. **普通版 fixture `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml` 第 287–291 行的 fix.done payload 现在缺新字段**（U3 落地后会要求 `head_sha` / `worktree_status` / `fix_attempt_commit_sha`）。跑 BDD 前必须先补齐这些字段，否则 schema reject。

6. **loop 版 6 dim expected SHA 切换要在 U5 统一处理**，不要在 U1 阶段就分别用 `executor_head_sha` 和 `round_base_sha`——会让协议块漂移。

7. **session 工作量估算**：本 session 大约用 60% 工作量完成普通版 U1（339 行新增 YAML + 反复 review）。loop 版 U1 + U3 结构性 + U5 + U6 + U7 + U8 估计需要同等甚至更多工作量。如果下一个 agent 也按完整 session 节奏跑，可能仍然不够——建议拆分 plan 002 为"先完成 loop U1 + U3 + U5 + U6 schema 部分"和"再跑 U7 + U8 验证"两个 stage。

## 上下文线索（给下一个 agent 的快速导航）

- 计划文件：`docs/plans/2026-07-12-001-fix-pipeline-preset-git-handoff-precheck-plan.md`
- 已改文件：`presets/en/ce-executor-pipeline.yml`（339 行新增 / 13 行删除）
- 已改文件 line 索引：
  - executor Final Git Handoff Precheck：约 line 834–901
  - fixer Final Git Handoff Precheck：约 line 2616
  - 6 dim Entry Precheck：约 line 1303 / 1561 / 1689 / 1796 / 1899 / 2009
  - 6 dim Step 5 Exit Precheck：约 line 1461 / 1605 / 1718 / 1824 / 1933 / 2040
  - alignment Entry Precheck：约 line 2845
  - alignment Step 5 Exit Precheck：约 line 2904
- 相关 memory（用户 auto-memory）：
  - `mem:ce-executor-pipeline-silent-fail-root-cause` — pipeline preset executor 0 emit 根因
  - `mem:default-publishes-success-side-misroute` — 切 default_publishes 到 success 侧会静默吞真失败
- 相关 claudeMd 硬规则：
  - "Preset 测试规则"：不允许 byte-equality 锁定 preset 文本
  - "preset yml 改动后必须同步 schema 并跑校验"：必须同步 schema、跑 lint
  - "preset/schema 改动后的下游同步清单"：runtime / preset_lint / BDD / config / CLI / manifest / 文档
  - "测试入口强制 nextest"：禁止裸 `cargo test -p ralph-cli`