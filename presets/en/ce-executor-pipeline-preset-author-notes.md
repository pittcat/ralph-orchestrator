# ce-executor-pipeline preset author notes

## Change: executor / fixer anti-abdication settlement

目标：不改变 topic 拓扑，只收紧 `executor` 与 `fixer` 的单链执行契约。计划规模、Unit 数量、文件数量、预计上下文压力和预计耗时均不能代替真实执行证据。主 hat 只 dispatch、验收、提交和结账；每个 Unit 的 RED/GREEN/REFACTOR 由唯一 subagent 完成。

验证采用分层策略：每个 Unit 完成后运行 focused tests 与受影响的跨边界/集成测试；全部 Unit 结束后运行一次权威 full-suite。全量新增失败按因果相关失败簇最多委派 3 次 fresh repair subagent，主 hat 不直接编辑修复代码。

## Single-Chain-First

1. **本 preset 的 unit 拆分能否由 executor/fixer 内部 subagent 完成？** ✓。原始 Unit 与 fix Unit 都在各自主 hat 内逐个 dispatch。
2. **任何业务 topic 是否超过一个消费者？** ✓。未改变既有单消费者拓扑。
3. **fallback 是否可能路由到 success？** ✓。失败账单只进入既有 reporter/alignment 路径。
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✓。tasks 关闭；状态来自 trigger、subagent 结果、git 与验证报告。
5. **是否有 rescue hat 能改变业务链路？** ✓。未新增 rescue hat。

## Hat: executor

- **Q1 使命:** 逐个 dispatch 原始计划的所有独立 U-ID，验收、独立提交并发出完整执行账单。
- **Q2 输入:** 从 `plan.ready` 读取 `plan_path` 与 baseline SHA；从计划提取 U-ID/Dependencies；从 subagent 返回、git log 与验证报告取得尝试证据。
- **Q3 执行:** Observe → baseline verifier → 每 U-ID dispatch/验收/affected tests/commit → final full-suite → delegated repair（如需）→ settlement → policy-check → emit/confirm。
- **Q4 输出:** `work.done` 或结构化 `work.failed`。
- **Q5 交接:** reporter 从 `work.failed` 的 Unit 分类与 `reason` 生成 blocked 报告。

### Hat: executor — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `work.failed` | `planned_units` | string[] | 原始计划 Implementation Units | executor 可读 `plan_path` | 不涉及 | reporter 展示完整范围 | `field_docs.planned_units` |
| `work.failed` | `attempted_units` | string[] | decisions dispatch log | subagent 返回与 checkpoint | 不涉及 | 证明不是预测性失败 | `field_docs.attempted_units` |
| `work.failed` | `completed_units` / `failed_units` | string[] | git log、Unit 验收与验证结果 | executor 命令输出 | 不涉及 | reporter 区分完成/真实失败 | 对应 `field_docs` |
| `work.failed` | `blocked_units` | string[] | plan Dependencies + 实际 failed Unit | 原始计划与 Unit 结果 | 不涉及 | reporter 解释阻塞边 | `field_docs.blocked_units` |
| `work.failed` | `decisions_file` / `reason` | string | `.ralph/agent/decisions.md` 与观察到的失败 | executor 写入并可读 | 不涉及 | reporter 核验失败原因 | 对应 `field_docs` |

## Hat: fixer

- **Q1 使命:** 逐个 dispatch 所有 actionable fix Unit；无论 applied、partial 或 blocked，都发出一次诚实的 `fix.done` 尝试报告。
- **Q2 输入:** `review.complete` trigger、`fix_plan_file`、Unit Dependencies、subagent 返回、git 与 baseline/final/delta 报告。
- **Q3 执行:** Observe → baseline verifier → 每 fix Unit dispatch/验收/affected tests/commit → final full-suite → delegated repair（如需）→ settlement → policy-check → emit/confirm。
- **Q4 输出:** `fix.done`，以 `fix_status` 表达成功、部分完成或阻塞。
- **Q5 交接:** alignment 用 Unit 分类、SHA、worktree 与验证字段核对原计划和 fix plan。

### Hat: fixer — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `fix.done` | `planned_fix_units` / `attempted_fix_units` | string[] | fix plan headings + dispatch log | fixer 可读 fix plan/subagent 返回 | 不涉及 | alignment 检查覆盖率 | 对应 `field_docs` |
| `fix.done` | `completed_fix_units` / `failed_fix_units` | string[] | commit 与 Unit 验收 | git/subagent/验证输出 | 不涉及 | alignment 判断实际落地 | 对应 `field_docs` |
| `fix.done` | `blocked_fix_units` / `skipped_fix_units` | string[] | Dependencies 或 non-actionable 分类 | fix plan 可见字段 | 不涉及 | alignment 记录残留 | 对应 `field_docs` |
| `fix.done` | `fix_status` / `failure_reason` | enum/string | settlement audit | fixer instructions | 不涉及 | alignment 不把 partial 当成功 | 对应 `field_docs` |
| `fix.done` | `decisions_file` | string | fixer execution ledger | fixer 写入并可读 | 不涉及 | alignment/人工核验 | `field_docs.decisions_file` |

## Builtin Sync Checklist

1. runtime：未改 topic 或 completion 语义，无需 Rust 修改。
2. preset_lint：未改 finding。
3. BDD：结构化 contract 由 strict preset check 与 schema tests 覆盖。
4. config：未改配置字段。
5. CLI presets：未增删 builtin。
6. manifest/index：未增删 builtin。
7. docs/zsh：未增删 preset 名称。
