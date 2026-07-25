# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Multi-hat orchestration

### Hat

An isolated agent role in a preset loop. Each activation gets fresh context, own `publishes`/`triggers`, and optional tool restrictions. Hats coordinate through the event bus and runtime APIs (`ralph emit`, `ralph tools task`), not shared process state.

### dimension-reviewer

A read-only review hat in `ce-executor-pipeline` (and historically `ce-executor-serial`). Reviews one quality dimension per activation, emits `review.dimension.done` or `review.dimension.failed`, and may write findings only under the plan scratchpad. Must not modify tracked plan or source files; doing so triggers scope violation audit.

### disallowed_tools

Per-hat list of tool names the role must not use. In Ralph today this drives prompt TOOL RESTRICTIONS and post-iteration file-modification audit when `Edit` or `Write` is listed; CLI backends may support additional hard denial (e.g. Claude `--disallowedTools`) when the runner merges hat config at spawn time.

### scope_violation

A hat modified tracked files despite its read-only or tool-restriction contract. For `dimension-reviewer`, U5 promotes the first violation to a hard loop termination (`ScopeViolationHardRejected`) rather than a recoverable counting failure.

### artifact-first handoff

一种跨 hat 或 hat 与 sub-agent 的交接原则：完整结果、可恢复状态、证据和关键决策依据优先写入当前 workspace/worktree 的 `.ralph/` 业务 artifact，消息与事件只传递短状态、摘要、路径、必要身份和路由字段。Ralph 的内部 ledger 不属于可供 hat 自定义读写的业务 artifact。

### follow-on loop

跨两次独立 `ralph run` 的串联：第一环终态成功且交接校验通过后，再启动第二环（另一 preset/config）。与同一 preset 内的 hat 链、以及 `ce-executor-pipeline-loop` 的 review/fix 环不同。近亲行为是 worktree 成功后的 `auto_merge → merge-loop` spawn。

### chain handoff

follow-on 两环之间的强制交接产物：由第一环成功路径写出、启动第二环前校验；缺失或不合格则失败关闭、不启动第二环。正文在业务 artifact；第二环只消费约定路径与必填字段。

### execution.plan.ready

`ce-executor-supervisor` 中 task-planner 在成功写入 `.ralph/review/<plan-key>/execution-plan.yml` 之后发出的业务 handoff topic。事件只携带路径、hash 与路由身份字段；DAG 正文留在 artifact。消费方是 `exec-wave-dispatcher`（首次 ready-wave fan-out）。失败路径发 `plan.blocked`，二者在同一 activation 互斥（isolated 单业务事件预算）。

### wave

同一 hat 定义下、对 N 份同构 payload 的 orchestrator 级 fan-out/fan-in。共享 `wave_id`，用 `wave_index` / `wave_total` 区分 slot。需要账本级收齐、超时与诊断时用 wave；hat 内部分工优先 subagent。`events.jsonl` / `supervisor.db` 是 runtime ledger，不是 hat 业务 artifact。

### OPAC

Observe → Precheck → Apply → Confirm。isolated 模式下 state-changing 操作的纪律框架；Precheck/ACL 可由 CLI 硬闸，Confirm 对 wave/task 逐步硬化为 ticket 或公开查询证据。

### payload consistency（载荷一致性闸）

挂在 `event_policy.payload_consistency` 的同 payload 验收 checkpoint（默认关闭、preset opt-in）：在 `ralph emit` / `--policy-check` 与真实 Apply 使用同源路径上，按 preset 声明的谓词检查**当前交卷 JSON 是否自洽**（例如声称成功却与计数/状态互斥）。命中时以 `gate=payload_consistency:<rule_id>` 拒收，并附带 `referenced_fields`（从 predicate AST 派生的稳定字段路径数组）；agent 据此定位需修复字段，不从 `message` 解析。`rule.message` 是不可信诊断数据（≤1024 UTF-8 bytes，禁 ANSI/控制字符/零宽字符），不进 agent 指令通道。agent 恢复说明沿用其他 `SemanticGateViolation` 拒收的同一条恢复通道；不读事件历史（跨事件互斥是 follow-up）。与 OPAC（操作纪律）和 `execution_contracts`（完成证据义务）分工，不互相替代。

### wave protocol suite（六件套）

Wave/supervisor 协议层能力集合：反压、分布式取消、状态持久化、幂等键、内容哈希去重、补偿。语义 SSOT 在 `SupervisorStore`；默认 wave 路径应吸收该套件，而非仅 `supervisor.enabled` preset。

### Pi skill 上下文预算

`ralph run -b pi`（headless）默认用 `--no-skills --skill .agents/skills`：不加载用户全局 skill 索引，只挂项目 `.agents/skills`；全局 Pi extensions 仍可加载。交互式 `pi_interactive` 路径不强制套用。缺 `.agents/skills` 不得硬失败。

### Ralph 分阶段 Pi activation

单次 hat activation 内，Pi extension 识别 Ralph 长 prompt，按 `ORIENTATION → EXECUTE → VERIFY → REPORT`（对齐 `build_custom_hat`）自动多轮披露与续跑；缺段跳过，解析失败则不接管。阶段完成须确定性信号；事实源与门禁仍在 Ralph。适用于所有 Ralph→Pi activation，不限某个 hat。

## Operator skills（loop 外）

### ralph-e2e-bootstrap

Loop 外 Skill：对**外仓** E2E 沙箱，验证**当前仓改动 plan**。输入 sandbox + 改动 plan（+ 建议 preset）；自动发现沙箱业务 workload 作为 `ralph run --plan`；改动意图写入 `PROMPT`；检查/构建当前仓最新 `ralph`；静态门后交启动命令。不静默改写沙箱 plan；不动 preset（需则 handoff `ralph-preset-author`）；不代跑 live / 不做诊断（`ralph-run-diagnosis`）。实现面仅 skill + Python，**不改 Rust**。

### E2E 沙箱目录

Operator 指定的、用本仓编译出的 `ralph` 对真实 plan 手跑 preset 的可写沙箱（典型为独立 sibling 仓）。**不是**本仓 `crates/ralph-e2e` 测试 harness。


