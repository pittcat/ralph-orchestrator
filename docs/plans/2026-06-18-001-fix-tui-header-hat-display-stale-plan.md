---
title: fix: TUI 顶部 hat 显示在 subagent 切换后不同步
status: active
created: 2026-06-18
---

# fix: TUI 顶部 hat 显示在 subagent 切换后不同步

## Summary

修复 TUI 顶部 hat 显示 bug:当用户为同一 topic 配置多个 subagent(例如 `planner` 既可以走 `📋 Planner` 又可以走 `🧠 Strategist`),TUI 顶部在迭代中段会显示与实际执行不符的 hat 名。根因是 `TuiState::update` 在通过 `hat_map` 查表更新 `pending_hat` 时,**没有同步回填当前正在进行的 `IterationBuffer.hat_display`**,导致 header 渲染在 `current_iteration_hat_display` 与 `pending_hat` 之间产生视觉漂移。本计划限定为显示层修复,不改 backend adapter / hat 选择策略,严格避免引入回归。

## Problem Frame

### 现象

用户在 hat 集合里为同一 topic(例如 `build.task`)声明了多个候选 subagent(如 `Planner` 与 `Strategist`),event loop 可能在两次 iteration 中间为同一 topic 选中不同的 subagent。TUI 顶部 header 应反映**当前正在执行**的 hat,但实际:

- **iteration 进行中**:header 显示**上一轮**冻结的 `IterationBuffer.hat_display`(已陈旧)
- **iteration 完成后短暂间隔**:header 才切到 `pending_hat`(参见 `crates/ralph-tui/src/widgets/header.rs:92-102` 的 pending_hat fallback)
- **视觉表现**:顶部图标/名称在迭代真正开始前就滞后,用户误以为"hat 没切过来",但其实后端已切,只是 TUI 显示的快照没刷

### 根因(基于源码确认)

1. `crates/ralph-tui/src/state.rs:392-399`:当事件 `topic` 命中 `hat_map` 时,**只更新 `pending_hat`**,**不更新当前 `IterationBuffer.hat_display`**。当 `iteration_finished == false`(iter 进行中),header 在 `crates/ralph-tui/src/widgets/header.rs:97-102` 走 `current_iteration_hat_display()` 分支,返回的就是 iter 开始时冻结的旧值。
2. `crates/ralph-tui/src/state.rs:597-601`:`start_new_iteration_with_metadata` 把 `pending_hat` 拷给新 buffer,但**不监听**"同一 iteration 期间 pending_hat 被改写"的情况——这是当前架构的盲点。
3. `crates/ralph-tui/src/widgets/header.rs:92-102` 的 `iteration_finished` 判断依赖 `buffer.elapsed.is_some()`;iter 进行中走的是 frozen 分支,而非 pending 分支。

### 影响面

- **仅限显示**:不影响 hat 选择、不影响 backend 调用、不影响事件流。
- **触发条件**:hat_map 命中的 topic 在 iter 进行中再次被同一 `HatId` 的不同 `name` 切换;或同一 topic 命中不同的 subagent(架构支持但当前 hardcoded 路径走的是 `task.start`/`build.task` 等)。
- **用户感知**:底部 footer / 内容区的 hat 信息可能已经更新(它们走不同字段),但顶部 header 滞后。看起来"不一致"。

### 不在范围(避免回归)

- ❌ 修改 backend adapter / subagent 选择逻辑
- ❌ 修改 `build_tui_hat_map` 的 topic→hat 解析规则
- ❌ 改动 header 渲染优先级 / 宽度断点
- ❌ 改动 `pending_hat` 的写入路径(只新增"同步回填当前 buffer"的旁路)
- ❌ 改动其他 widget(content / footer / wave)的 hat 来源

## Requirements

### R1. iter 进行中,pending_hat 变更必须立即反映到顶部 header

当 `TuiState::update(event)` 通过 `hat_map` 命中 topic 且当前 `current_iteration().hat_display != pending_hat.display` 时,同步把 `pending_hat` 的 display 写回**当前** `IterationBuffer.hat_display`。此规则仅在 `current_iteration()` 存在且 `elapsed.is_none()`(iter 未完成)时触发——已完成的 iter 保留历史冻结值不被覆盖(否则会破坏 `current_view` 翻看历史时的正确显示)。

### R2. 顶部显示在 subagent 切换时无视觉漂移

- 在 iter 期间,把"正在执行的 iter buffer"(`self.iterations.last()`, 见 KTD2)的 `hat_display` 同步到最新 subagent 的 name(emoji + 文本)。
- backend 字段不在本计划范围。`current_iteration_backend()` 走的是 `buffer.backend`,来源是 `start_new_iteration_with_metadata` 的 `self.pending_backend`,写入路径独立;其同步漂移(若存在)是另一回事,作为独立 follow-up 处理,不并入本计划。

### R3. 回归保护

- **正在执行的 iter 之外**的 iter, 其 `hat_display` 不得被覆盖。`current_iteration_mut()` 拿的是 `self.current_view` 指向的 iter——用户在 REVIEW 模式翻看历史时, 它指向历史 iter(可能 `elapsed.is_none()`, 因为还没收到 `build.done`), 而**正在执行**的 iter 是 `self.iterations.last()`。两者必须明确区分, 否则会污染 `header_uses_per_iteration_hat_from_events_when_reviewing` 这类历史回放测试(场景: 用户翻到 iter 1 看历史, 后端在 iter 2 触发 hat_map 命中, 错误地把 iter 1.hat_display 改了)。
- `pending_hat` 字段本身的语义不变。
- `hat_map` 未命中(hardcoded 路径)时,行为完全保持当前。
- `task.start` 的 `*self = Self::new()` reset 路径不受影响(此路径本来就会重置 `pending_hat`,不需回填)。

### R4. 无回归

- 不改动 `crates/ralph-tui/src/widgets/header.rs` 的渲染函数(保持渲染逻辑冻结)。
- 不改动 `crates/ralph-tui/src/widgets/content.rs` / `footer.rs` 的 hat 来源。
- 不引入新的 pub API;不改动 `HatRegistry` / `HatId` 形态。
- 不修改 `presets/manifest.yml` / preset 文件。
- 不修改 backend adapter 或 loop_runner 的 hat 选择策略。

## Key Technical Decisions

### KTD1. 同步点选 `state.rs:update`,不回填到 `header.rs`

`header.rs:92-102` 已经走"iter 完成 → pending_hat,否则 → frozen"的二元判断。**修改渲染函数会扩大 blast radius**(可能影响宽度断点 / 压缩路径 / 现有 12 个 header 测试)。改 `state.rs:update` 的写入侧,把"pending_hat 变化"也镜像到"当前 buffer 的 hat_display"——一次写入、双处可见,渲染侧零改动,回归面最小。

**替代方案:在 header 渲染时直接读 pending_hat**——否决,因为这会打破"iter 进行中显示 frozen 值"在历史回放场景下的语义(参见 `header_uses_per_iteration_hat_from_events_when_reviewing` 测试)。

### KTD2. 同步触发条件:目标 buffer 是 `self.iterations.last()` AND hat_map 命中

- **目标 buffer 必须是正在执行的 iter**(`self.iterations.last()`):其他 iter(无论 `elapsed.is_some()` 与否)都不得被修改。`elapsed.is_some()` 不是充分条件——用户在 REVIEW 模式翻看历史时,历史 iter 的 `elapsed` 可能还是 `None`(因为还没收到 `build.done`),此时 `elapsed.is_none()` 守卫会放行,但那是历史 iter,写它会污染历史回放。
- **`hat_map` 未命中**(hardcoded 路径):当前 `update` 不改 `pending_hat`(走 `custom_hat.is_none()` 分支,显式 set 一个 hardcoded 值)。这些 hardcoded 值在 `build.task` 时是 `🔨Builder`,在 `build.done` 时是 `📋Planner`——已经是稳定映射,不需回填,且不在 bug 触发条件内。

### KTD3. 不同步 `backend`,只同步 `hat_display`

`buffer.backend` 来源在 `start_new_iteration_with_metadata` 的 `self.pending_backend`,而 `pending_backend` 的写入路径独立(不在 `update` 里)。本计划**不**触碰 backend,严格限定在 hat_display 同步,避免越界。`current_iteration_backend()` 的显示漂移是另一回事,作为独立 follow-up。

## Implementation Units

### U1. 在 `TuiState::update` 中镜像 pending_hat → current buffer.hat_display

- **Goal**: 当 `hat_map` 命中 topic 且当前 iter 未完成时,把 `pending_hat` 的 display 同步到 `current_iteration_mut().hat_display`,使 header 在 iter 期间与实际执行保持一致。
- **Requirements**: R1, R2, R3, R4
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-tui/src/state.rs`(修改 `TuiState::update`,在 `hat_map` 命中分支后新增同步逻辑)
  - `crates/ralph-tui/src/state.rs`(测试 `custom_hat_topics_update_pending_hat` 块新增测试用例)
- **Approach**:
  1. 在 `state.rs:392-399` 的 `if let Some((hat_id, hat_display)) = custom_hat.clone()` 分支后,新增一段:
     - 取 `self.iterations.last_mut()`(语义:**正在执行**的 iter,不是 `current_iteration_mut()`——后者按 `self.current_view` 取, 用户翻看历史时会拿到历史 iter, 见 R3 与 KTD2)。
     - 若 `Some(buffer)` 且 `buffer.hat_display.as_deref() != Some(hat_display.as_str())`,则 `buffer.hat_display = Some(hat_display.clone())`。
     - **不**用 `elapsed.is_none()` 守卫——见 KTD2, 用户翻看历史时历史 iter 的 `elapsed` 可能还是 `None`, 不能用此条件判断"是不是正在执行"。
  2. 同步条件必须显式写全,不留隐式兜底——避免未来 refactor 时被误改。
  3. 不动 `pending_hat = Some((hat_id, hat_display))` 这行(原有行为)。
  4. 不动 hardcoded 路径(`build.task` / `build.done` 等)——它们本来就不在 bug 触发条件内。
- **Test scenarios**:
  - **Happy path**: 给 state 设 `hat_map` 含 `review.security` → "🔒 Security Reviewer";`start_new_iteration()` 后 `update(Event::new("review.security", ...))`;断言 `state.iterations.last().unwrap().hat_display == Some("🔒 Security Reviewer")`。
  - **iter 已完成不变更**: `start_new_iteration_with_metadata(Some("📋 Planner".to_string()), ...)`,手动设 `iterations[0].elapsed = Some(Duration::from_secs(5))`,再 `update` 触发 `hat_map` 命中;断言 buffer 的 `hat_display` 仍是 `"📋 Planner"`,**不**被覆盖。
  - **未命中 hat_map 不回填**: `TuiState::new()`(无 hat_map),`start_new_iteration()`,`update(Event::new("build.task", ...))`(hardcoded 路径);断言 `state.iterations.last().unwrap().hat_display` 仍是 `None`(未被回填)。
  - **pending_hat 自身语义不变**: 跑完 happy path 后,断言 `state.get_pending_hat_display()` 仍是新 hat 的 display(确认回填是"镜像"而非"替换")。
  - **不修改 backend**: 跑完 happy path 后,断言 `state.current_iteration_backend()` 不受影响(若原本是 `None`,仍是 `None`)。
  - **历史回放不被污染(关键回归用例)**: 创建 iter 1 (`start_new_iteration_with_metadata("📋 Planner", ...)`),`update(review.security)` 写 iter 1.hat_display = "Security";再 `start_new_iteration()` 创建 iter 2(此时用户 `navigate_prev()` → `current_view = 0`, 用户在 REVIEW iter 1, iter 1.elapsed 仍是 `None`);后端在 iter 2 触发 `update(review.correctness)`(hat_map 命中);断言 **iter 1.hat_display 仍是 "Security" 不被改**(本测试是 R3 的核心保护——若实现误用 `current_iteration_mut()`, 此测试会失败, 因为它会拿到 iter 1 然后写过去)。
- **Verification**:
  - `cargo nextest run -p ralph-tui -- <substring>` 全绿(U1 的所有新测试 + 现有 12 个 header 测试 + 现有 `custom_hat_topics_update_pending_hat` / `unknown_topics_keep_pending_hat_unchanged` 不退化)。
  - `cargo clippy -p ralph-tui -- -D warnings` 干净。

### U2. Header 端到端回归验证

- **Goal**: 用现有 header 渲染测试 + 新增的"iter 期间 hat 切换"渲染测试,确认顶部显示与实际执行同步。
- **Requirements**: R1, R2, R4
- **Dependencies**: U1
- **Files**:
  - `crates/ralph-tui/src/widgets/header.rs`(测试块新增)
- **Approach**:
  - 新增一个测试 `header_reflects_hat_map_change_during_iteration`:
    1. `TuiState::with_hat_map(...)` 设 `review.security` → "🔒 Security Reviewer",`review.correctness` → "🎯 Correctness Reviewer"。
    2. `start_new_iteration()`(iter 1,无 metadata,buffer.hat_display = None)。
    3. `update(Event::new("review.security", ...))`——`pending_hat` 更新为 Security,**且**(经 U1)buffer.hat_display 同步更新。
    4. `update(Event::new("review.correctness", ...))`——`pending_hat` 更新为 Correctness,buffer 同步。
    5. `render_to_string(&state)`,断言 header 包含 "Correctness" 且**不**包含 "Security"。
  - 现有 `header_shows_hat` / `header_uses_per_iteration_hat_from_events_when_reviewing` / `header_review_uses_frozen_elapsed_and_backend_from_events` 测试**保持不修改**,作为回归基线(若它们失败,说明 U1 越界了)。
  - 另加一个端到端回归测试 `header_does_not_pollute_history_when_user_reviews_older_iter`:
    1. 设 hat_map 含 `review.security` 与 `review.correctness`。
    2. iter 1: `start_new_iteration()`, `update(review.security)` → iter 1.hat_display = Security。
    3. iter 2: `start_new_iteration()`, `update(review.correctness)` → iter 2.hat_display = Correctness。
    4. `navigate_prev()` → `current_view = 0`, `following_latest = false`(用户在 REVIEW iter 1, iter 1.elapsed 仍为 `None`)。
    5. 后端触发 `update(review.correctness)`(意图是再切一次,模拟 subagent 在 iter 2 内二次切换)。
    6. `render_to_string(&state)`,断言 header 包含 "Security"(iter 1 的 frozen 值,不被 iter 2 的 update 污染),不包含 "Correctness"。
- **Test scenarios**:
  - **新测试**: `header_reflects_hat_map_change_during_iteration` + `header_does_not_pollute_history_when_user_reviews_older_iter`——U1 修复的端到端验证 + 关键回归保护。
  - **回归**: 现有所有 header 测试不需改动,`cargo nextest run -p ralph-tui` 全绿即为通过。
- **Verification**:
  - 跑 `cargo nextest run -p ralph-tui`(并行),U1 + U2 + 全部现有 header / state 测试全绿。

### U3. 文档与机制层一致性复核

- **Goal**: 确认 `ralph-tools*.md` / `.cursor/rules/*.mdc` 中描述 hat 显示的段落与新行为一致;若不一致,同步修正。
- **Requirements**: R4
- **Dependencies**: U1, U2
- **Files**:
  - `docs/solutions/` 下如有任何描述 "TUI 顶部 hat 来源"的文档(根据 `.cursor/rules/` grep 决定)。
  - `crates/ralph-core/data/ralph-tools*.md`(若引用了 TUI 显示行为)。
- **Approach**:
  1. `rg -n "hat_display|pending_hat|hat.*显示|TUI.*header" docs/ .cursor/rules/ crates/ralph-core/data/ crates/ralph-tui/src/` 扫一遍。
  2. 若发现描述与新行为矛盾(例如文档说"iter 期间 header 永远显示 frozen 值"),在 PR 描述中标注"文档需后续同步"或在本计划范围内同步——取决于命中数量。
  3. 若无命中或仅命中与新行为一致的描述,记入 PR 描述的"无文档影响"声明。
- **Test scenarios**: Test expectation: none — 这是文档一致性检查,不是行为变更。
- **Verification**: grep 输出已审阅;无矛盾或在 PR 范围内已修正。

## Scope Boundaries

### In scope

- `TuiState::update` 中"hat_map 命中 → 同步回填当前 buffer.hat_display"的旁路写入。
- 新增 4 个 state 层单测 + 1 个 header 端到端单测。
- 文档一致性 grep + 必要的局部同步。

### Out of scope(明确不做)

- 同步 `current_iteration_backend()`(backend 字段的同步是独立需求,见 KTD3)。
- 改 `header.rs` 渲染函数(零改动是 R4 的硬约束)。
- 改 `build_tui_hat_map` 的解析规则。
- 改 backend adapter / subagent 选择策略。
- 改 preset 文件。
- 改 `*self = Self::new()` 在 `task.start` 中的 reset 路径。

### Deferred to follow-up work

- **backend 字段的同步漂移**: 若 `pending_backend` 在 iter 期间被改写,header 的 `@backend` 也会滞后。修复方式与本计划同构(回填到 `current_iteration_mut().backend`),但属于独立需求,作为 issue 跟踪而非并入本计划。
- **hat 切换的过渡动画**: 用户在 TUI 中可能希望看到 hat 切换的视觉提示(如闪烁、徽章)。属于 UX 增量,非 bug 修复。
- **多 hat 并发执行的 header 表现**: 当前架构假设 iter 期间单 hat,若未来支持 wave 内并发 hat,需重新审视 header 数据源。

## Risks & Mitigations

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 同步回填破坏历史回放(iter 完成后被改) | 低 | 高(用户体验回归) | 显式检查 `buffer.elapsed.is_none()`;U1 测试 2 专门覆盖 |
| 同步逻辑被未来 refactor 误删(无断言保护) | 中 | 中(回归) | 现有 `custom_hat_topics_update_pending_hat` 测试断言 `pending_hat` 行为;U1 测试断言 `buffer.hat_display` 行为——双重断言 |
| backend 不同步导致用户仍抱怨"显示错" | 中 | 低(本计划严格限定 hat_display) | 在 PR 描述中明确"backend 同步为 follow-up",避免范围蔓延 |
| 性能影响(`current_iteration_mut()` 查找) | 极低 | 极低 | 该函数 O(1)(Vec 索引),且只在 hat_map 命中时触发,远低于事件频率 |

## Test Strategy

### 单元测试(`crates/ralph-tui/src/state.rs`)

- 复用 `custom_hat_topics_update_pending_hat` 测试块,新增 4 个测试。
- 测试入口:`cargo nextest run -p ralph-tui`(默认并行,本 crate 无 serial 约束)。

### Header 渲染测试(`crates/ralph-tui/src/widgets/header.rs`)

- 复用 `render_to_string` / `render_to_string_with_width` helper,新增 1 个测试。
- 入口同上。

### 回归基线

- 跑完整 `cargo nextest run -p ralph-tui` 确保零退化。
- `cargo clippy -p ralph-tui -- -D warnings`。
- 完整 workspace 验证(开发基线,非强制):`./scripts/run-tests.sh`。

## Verification

完成定义(DoD):

1. ✅ U1 + U2 + U3 全部完成,所有新增测试通过。
2. ✅ 现有 `ralph-tui` 包内所有测试通过(零回归)。
3. ✅ `cargo clippy -p ralph-tui -- -D warnings` 干净。
4. ✅ `cargo fmt --check` 干净(若有格式问题,`cargo fmt` 一次性修正)。
5. ✅ 无 `header.rs` 渲染函数改动(可通过 `git diff` 复核)。
6. ✅ 无 preset / backend / adapter 改动(可通过 `git diff --stat` 复核,改动面应仅限 `ralph-tui` 包内 + 可选 docs)。
7. ✅ 文档一致性 grep 完成,矛盾已同步或在 PR 描述中标注。
