# Red Team Attack Preset — Author Notes

## Intent

- **目标：** 从一个或多个已完成开发计划的 Git 历史反向定位实现提交，重建 Patch，设计并执行真实攻击实验，通过硬阈值证据门禁生成零回归修复计划。全程只读代码树，不修改生产代码、正式测试、tracked 配置或 Git 历史。
- **启动：** `ralph run -c ralph.red-team-attack.yml -H builtin:red-team-attack`；操作者在 `.ralph/red-team.prompt.md` 提供开发计划路径和可选 target/scope 参数。
- **成功交付：** `.ralph/red-team/PLAN.md`、`REPORT.md`、`QUESTIONS.md`，且 independent-reviewer 给出 `PLAN_READY`。
- **失败交付：** 任一阶段不能完成其契约时，先写失败 artifact，再发 `redteam.failed`；target-locker 是 required-event 例外：它只发一次 `redteam.target.locked(lock_status: failed)`，由 plan-resolver 转换为 `redteam.failed`。reporter 生成失败报告并发 `redteam.complete`，其中 `success: false`、`plan_path: ""`。

## Topology — single chain

plan-resolver 在任何 commit resolution 前必须先检查 `redteam.target.locked.lock_status`；若为
`failed`，读取 target-locker failure artifact，写入本阶段 failure artifact，并发 `redteam.failed`，不得继续解析。

成功脊只有一条：

```text
redteam.start
  → target.locked
  → plan.resolved
  → attack.mapped
  → experiment.done
  → evidence.gated
  → plan.ready
  → reviewed
  → complete
```

所有 producer hat 的失败都走同一个终止汇，不再建立业务 retry 回路：

```text
任一 producer → redteam.failed → reporter → redteam.complete(success=false)
```

`redteam.failed` 是本 preset 唯一的业务失败 topic。它不是 runtime 的
`plan.blocked`，也不会触发 runtime recovery、自动重跑或跨 preset 的特殊路由。
Evidence Gate 可以把补充实验建议写入 `07-retry-board.md`，但该文件只是失败证据和
人工 follow-up 记录，不是自动 retry 信号。

## Hard questions

1. **是否需要 supervisor / wave？** 不需要。8 个 hat 是顺序单链；hat 内部可以自行组织命令或 subagent，但不得把内部并行暴露为 runtime 拓扑。
2. **是否有多个消费者？** 每个成功 topic 只有一个下游消费者；`redteam.failed` 只有 reporter 消费。多个 producer 共享它是故障汇，不是多分支业务流程。
3. **失败是否可能进入成功脊？** 不会。producer 失败只能发 `redteam.failed`；reporter 只发 `redteam.complete(success=false)`。
4. **是否把 runtime recovery 当业务事实？** 不会。`task.resume`、`loop.stalled`、`plan.blocked` 是 runtime 控制面；本 preset 不订阅 `plan.blocked`。
5. **是否能自动 retry？** 不能。需要新实验时，reporter 在 `QUESTIONS.md` 写出 operator follow-up，下一次由操作者重新启动 loop。

## Failure contract

所有 producer 发 `redteam.failed` 前必须：

1. 在 `.ralph/red-team/failures/<stage>.md` 写完整原因、已完成检查、证据路径和人工 follow-up。
2. 使用当前激活 hat 的 stage ID 填 `failed_stage`。
3. 使用稳定、非空的 `reason` token，不把未经证实的结果写成安全 finding。
4. 填 `failure_artifact_path`，执行 `test -f` 验证文件可读。
5. 先运行 `ralph emit redteam.failed --policy-check`，确认 `ok` / `recorded` 后再执行真实 emit。
6. 一个 activation 只发一个业务事件；failure emit 被策略拒绝时，停止后修正 payload，不发第二个业务 topic。

Schema SSOT 位于 `presets/schemas/red-team-attack.yml`。失败字段为：

| 字段 | 含义 |
|---|---|
| `failed_stage` | 实际发出 failure event 的 producer hat：`plan-resolver`、`attack-surface-mapper`、`experiment-runner`、`evidence-gate`、`impact-boundary`、`independent-reviewer`。target-locker 的锁失败由 plan-resolver 转换。 |
| `reason` | 稳定的失败原因 token；完整解释放在 failure artifact。 |
| `failure_artifact_path` | `.ralph/red-team/` 下实际存在的失败 artifact 路径。 |

## Artifact ownership

- `target-locker`：`01-target-lock.md`
- `plan-resolver`：`scope-manifest.json`、`02-plan-resolution.md`、`commits/PLAN-*.md`、`03-patch-reconstruction.md`、`patches/**`
- `attack-surface-mapper`：`04-attack-surface.md`、`05-experiment-plan.md`
- `experiment-runner`：`experiments/RTE-*.md`、`evidence/RTE-*/**`、`repros/RTE-*/**`
- `evidence-gate`：`07-evidence-board.md`；失败时也可写 `07-retry-board.md`
- `impact-boundary`：`08-impact-boundary.md`、`findings/RTF-*.md`、`PLAN.md`
- `independent-reviewer`：`10-independent-review.md`
- `reporter`：`REPORT.md`、`QUESTIONS.md`
- 失败 producer：`failures/<stage>.md`

所有完整结果先落盘，事件只传路径、短计数、短状态和必要身份字段。Agent 不得读取或写入
`.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger。

## Hat contracts

### target-locker

读取 prompt 和 Git 状态，锁定 HEAD/tree，写 `01-target-lock.md`。干净且没有
MERGE/REBASE/CHERRY-PICK 状态时发 `redteam.target.locked(lock_status: locked)`；任何锁定失败都写
`failures/target-locker.md` 并发 `redteam.target.locked(lock_status: failed)`，由 plan-resolver 转换为失败汇。

### plan-resolver

读取 `redteam.target.locked` 和 lock artifact，反向定位 commit、重建 patch、写 scope
manifest。只有 scope_status 已 resolved 且所有阈值通过时发 `redteam.plan.resolved`；否则写
`failures/plan-resolver.md` 并发 `redteam.failed`，不得发 unresolved 分支。

### attack-surface-mapper

只由 `redteam.plan.resolved` 激活。必须验证 scope_status、critical_unknown_count、
overall_confidence、coverage、boundary_conflict；任一失败就写 failure artifact 并发
`redteam.failed`。通过后写攻击面和实验计划，发 `redteam.attack.mapped`，其中
`predecessor_event` 必须是 `redteam.plan.resolved`。

### experiment-runner

执行一个实验的 control group 与 attack group，保存原始证据，确认 tracked tree 未变，发
`redteam.experiment.done`。目标或 tree 改变、清理失败、代码树变脏或证据不可接受时，先写
failure artifact，再发 `redteam.failed`，不得用伪造的 `experiment.done` 继续下游。

### evidence-gate

只读取原始证据，检查二元执行门禁和 Confidence、Evidence Coverage、Verifiability、Impact
Certainty 四项指标。全部通过才发 `redteam.evidence.gated`；任一失败写
`07-retry-board.md` 和 failure artifact，发 `redteam.failed`。retry board 只记录 operator
可选的后续实验，不会激活 attack-surface-mapper。

### impact-boundary

由 `redteam.evidence.gated` 激活，继续执行调用者、消费者、配置、生命周期、兼容性和回归
边界检查。影响明确时写 findings、`08-impact-boundary.md` 和 `PLAN.md`，最后发
`redteam.plan.ready`；影响不确定时写 failure artifact 并发 `redteam.failed`。

### independent-reviewer

只由 `redteam.plan.ready` 激活，审查所有 artifact、真实实验、阈值、修复边界和零回归证据。
通过发 `redteam.reviewed`；任一检查失败写 failure artifact 并发 `redteam.failed`，不发
伪造的 `PLAN_REJECTED` 中间分支。

### reporter

消费 `redteam.reviewed` 或 `redteam.failed`。成功输入必须验证 `PLAN.md`；失败输入必须在
报告中区分“证据不足/流程失败”和“已确认安全 finding”，引用 `failure_artifact_path`，并把
`failed_stage` 与事件上下文中的 producer hat 对账；不一致时仍只能失败收尾。
始终先写并验证 `REPORT.md`、`QUESTIONS.md`，再发唯一终态 `redteam.complete`。

## Operator / author verification

修改 preset 或 schema 后必须同步检查：

1. `ralph preset check -H builtin:red-team-attack --strict --format json`
2. `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
3. `cargo nextest run -p ralph-core -- preset_lint`
4. `cargo nextest run -p ralph-cli --bin ralph -- presets`
5. `cargo nextest run -p ralph-core --test scenarios -- test_redteam_failed_reaches_reporter`
6. `cargo nextest run -p ralph-cli --test integration_emit_policy -- test_builtin_redteam_failed_sink_is_publishable_by_producers_only`
7. 对其他 builtin preset 至少执行 strict preset check，确认没有因 `redteam.failed` 或 runtime recovery 变更产生 findings。

该文件是 operator / preset author 说明，不替代 agent-facing `crates/ralph-core/data/*.md`；
agent-facing 能力变化必须同步更新对应 skill guide，并检查 CLI 文档 drift。
