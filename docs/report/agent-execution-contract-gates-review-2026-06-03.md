# Agent 执行契约门控完成度与回归审查报告

审查对象：`docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md`

审查范围：计划提交 `35e0f02` 之后的实现提交 `f73156b..62e6fba`，主 diff 口径为 `35e0f02..HEAD`。

审查结论：**未达到百分百完成，当前不建议视为完成。** 核心框架已落地，`ce-executor` 也已启用 contract，但 `work.done` 验证器存在会放过假完成的实质性缺陷；测试覆盖没有证明现场失败路径不会复发；新增文档与代码、项目语言规则也不一致。

## 总体判定

| 维度 | 判定 | 说明 |
|---|---|---|
| 计划完成度 | 部分完成 | U1-U7/U9 有实现痕迹，U8 的 replay-light 语义测试不足；R7、R9 未满足。 |
| 回归风险 | 高 | contract validator 会放过不存在 task、空 diff/空 commit 的 `work.done`，直接削弱本计划核心目标。 |
| 文档同步 | 未完成 | 新增 guide 为英文，且描述了未实现的 git mode；`presets/COLLECTION.md` 同样混用英文。 |
| 测试可信度 | 不足 | 目标测试通过，但未覆盖 TaskNotFound、真实空 git evidence、contract rejection 与 missing-event gate 的交互。 |

## 阻断发现

### P1 - `work.done.task_id` 不存在时会被当作通过

| 文件 | 位置 | 问题 | 影响 | 建议 |
|---|---:|---|---|---|
| `crates/ralph-core/src/execution_contract.rs` | 211-224 | `let task = store.get(&task_id)?;` 在 task 不存在时返回 `None`，`validate_task()` 也返回 `None`，上层把它解释为“无违规”。 | 违反 R7：`work.done.task_id` 必须能在当前 task store 中找到。伪造或过期 task_id 的 `work.done` 只要 payload 字段齐全，就可能进入 git/test 检查并最终触发 review。 | 不存在 task 必须返回 `ExecutionContractViolationKind::TaskNotFound`；TaskStore 加载失败也应按 fail-closed 拒绝，而不是 warn 后放行。补 `TaskNotFound` 单元测试和 event loop 集成测试。 |

证据：代码已经定义 `TaskNotFound { task_id: String }`，但只在 enum 中存在，没有任何构造路径。

### P1 - `diff_or_commit` 的 commit 检查会让任意已有提交的仓库通过

| 文件 | 位置 | 问题 | 影响 | 建议 |
|---|---:|---|---|---|
| `crates/ralph-core/src/execution_contract.rs` | 307-323、`check_git_commit()` | `check_git_commit()` 只执行 `git log --oneline -1`，任何非空 git 仓库都会返回 true。于是 `!has_diff && !has_commit` 基本不会成立。 | 违反 R9：非 trivial work 不能空 diff/空 commit 通过。当前 executor 即使没有任何本轮变更，也可能通过 `work.done` contract。 | contract 需要和本轮起点绑定，例如使用 plan/context 的 `start_sha`、loop start SHA、event payload evidence，或注入可测试的 git evidence provider。至少应检测 `HEAD` 是否相对本轮起点有新 commit，并同时覆盖 staged、unstaged、untracked 语义。 |

额外问题：`require_git_change.mode` 字段当前未被使用，`diff_only` / `commit_only` 没有实现；文档却声称这些模式存在。

### P1 - contract rejection 会被主循环继续当作 missing-event hard gate

| 文件 | 位置 | 问题 | 影响 | 建议 |
|---|---:|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 4121-4123 | `had_events` 只看 `validated_events`，被 contract 拒绝的原始事件不计入。 |
| `crates/ralph-cli/src/loop_runner.rs` | 2712、2781-2796 | loop runner 用 `processed.had_events` 判断 `agent_wrote_events`；contract rejection 后会继续触发 missing-event gate、写第二份 guidance、递增 hard gate 计数。 |

影响：这和 U6 的设计描述“不终止 loop；让 injected guidance 驱动下一轮修复”不一致。连续发出无效 `work.done` 可能被计入 hard-gate exhaustion，表现成“没 emit”而不是“emit 了但 contract 不通过”，诊断会混乱，甚至提前停止。

建议：`ProcessedEvents` 增加区分字段，例如 `had_raw_events` / `had_rejected_events`，missing-event gate 只在真正没有任何事件时触发；contract rejection 走独立 backpressure 计数或仅记录。

## 完成度矩阵

| 单元 | 判定 | 依据 |
|---|---|---|
| U1 Emit Obligation Gate v2 | 基本完成但测试不足 | `should_gate_missing_events()` 和主循环接入存在；但测试只测 helper，不测真实 loop runner 分支、guidance 持久化和 3 次终止路径。 |
| U2 ce-executor 去默认兜底 | 完成 | `executor` hat 无 `default_publishes`；root 和 embedded preset 测试存在。注意 coordinator 仍有 `default_publishes: "work.failed"`，这是其他 hat，不违反 U2。 |
| U3 Execution Contract 配置模型 | 基本完成 | config 结构和解析测试存在；但 `mode` 为自由字符串且运行时没有按 mode 分支。 |
| U4 Work Done Contract Validator | 未完成 | payload 和 open task 路径有部分验证；TaskNotFound、TaskStore load fail、git evidence 是 fail-open 或错误语义。 |
| U5 Event Loop 接入 | 部分完成 | contract 插入在 publish 前，原 `work.done` 可被拒绝；但 rejection 与 `had_events`/missing-event gate 的交互有回归风险。 |
| U6 诊断与可观测性 | 部分完成 | warn 和 diagnostics 文件写入存在；但 contract rejection 会被 hard gate 混淆。 |
| U7 ce-executor Contract 启用 | 基本完成 | preset 启用 `event_loop.execution_contracts.rules.work.done`，root/zh/embedded 同步。 |
| U8 Replay-Light 集成测试 | 未完成 | 没有证明 “open task 拒绝且 review 收不到事件 / closed task 接受且 review 收到事件 / 旧 default fixture forbidden” 的完整 runtime 路径。 |
| U9 文档与学习沉淀 | 部分完成 | 文档文件存在，但新增 guide 和 COLLECTION 新段落大量英文；guide 描述了未实现模式。 |

## Requirements 对照

| Requirement | 判定 | 说明 |
|---|---|---|
| R1-R2 | 基本满足 | no-event gate 不再依赖输出提到 `ralph emit`。 |
| R3 | 部分满足 | no-event guidance 持久写 JSONL；contract rejection guidance 经 bus 的 `human.guidance` 路径进入后续 prompt，但会被 missing-event gate 重复污染。 |
| R4 | 基本满足但测试不足 | `check_termination()` 有 `HARD_GATE_MAX = 3`，缺 missing-event 真实路径测试。 |
| R5 | 部分满足 | `work.done` 在 publish 前验证，但验证器 fail-open。 |
| R6 | 基本满足 | ce-executor 配置要求 `plan_name`、`plan_path`、`task_id`、`task_key`、`step`。 |
| R7 | 未满足 | task 不存在被放行。 |
| R8 | 部分满足 | open task 会被拒绝；但 task 不存在、TaskStore 加载失败会放行。 |
| R9 | 未满足 | git commit 检查语义错误，空工作可通过。 |
| R10 | 部分满足 | optional 模式可解析；required payload field 有实现，但 ce-executor 当前仍 optional。 |
| R11 | 部分满足 | 原 `work.done` 不 publish；但 rejection 后会被 missing-event gate 混淆。 |
| R12 | 满足 | ce-executor executor 已移除 `default_publishes`。 |
| R13 | 基本满足 | 文档有边界说明，但英文和部分描述不准确。 |
| R14 | 满足 | `sync-embedded-files.sh check` 通过；root/embedded/zh 有同步测试。 |
| R15 | 未充分证明 | 只跑了部分目标测试；未跑完整 workspace gate。新增测试覆盖不足。 |

## 文档问题

| 文件 | 位置 | 问题 |
|---|---:|---|
| `docs/guide/execution-contracts.md` | 1-132 | 新增文档几乎全英文，违反 AGENTS.md 的“所有中文输出规则”。 |
| `docs/guide/execution-contracts.md` | 67-70 | 声称支持 `diff_only`、`commit_only`，但运行时代码没有按 `mode` 分支。 |
| `docs/guide/execution-contracts.md` | 112-115 | 声称能防止 “without real git changes”，实际 `git log -1` 让普通仓库空工作通过。 |
| `presets/COLLECTION.md` | 861-951 | 新增段落混用大量英文，违反项目中文输出规则。 |
| `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md` | 39-42 | 宣称 U8 replay-light 测试已完成，但实际测试没有覆盖计划列出的 replay-light 场景。 |

## 已执行验证

| 命令 | 结果 |
|---|---|
| `rtk proxy ./scripts/sync-embedded-files.sh check` | 通过，embedded assets in sync。 |
| `rtk cargo test -p ralph-core execution_contract -- --nocapture` | 通过，11 个匹配测试通过。 |
| `rtk cargo test -p ralph-cli test_missing_event_hard_gate -- --nocapture` | 通过，1 个匹配测试通过。 |
| `rtk cargo test -p ralph-cli test_ce_executor_executor_has_no_default_publishes -- --nocapture` | 通过，2 个匹配测试通过。 |

未执行完整 workspace test gate；本报告是审查报告，不是修复完成声明。

## 建议修复顺序

1. 修复 `validate_task()` fail-open：task store 读失败、task_id 不存在、task_id 非字符串、task_key 缺失/不匹配都应有明确 rejection。
2. 重做 git evidence：禁止使用“仓库存在任意 commit”作为本轮完成证据；引入 loop start SHA 或 injectable evidence provider，并补空 diff/空 commit 的失败测试。
3. 修正 contract rejection 与 missing-event gate 的交互：无效事件不应被误报为没 emit。
4. 补 U8 的 runtime/replay-light 测试：open task 拒绝且 review 不触发、closed task 接受且 review 触发、旧 executor success default fixture forbidden。
5. 同步文档：全部新增说明改中文，删除未实现的 `diff_only` / `commit_only` 声明，或先实现再保留。

## 最终结论

这轮实现完成了“结构搭起来”和“ce-executor 接上配置”，但没有完成“Ralph-owned 完成真实性验证”这个核心承诺。尤其是 TaskNotFound 和 git evidence 两处缺陷会让假 `work.done` 继续通过，因此不能认定计划百分百完成，也不能认定没有引入回归。
