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

### Effective Activation Contract

Orchestrator 在最终 resolved config、当前 flow step、hat、trigger event 与已接纳状态之上编译并持久化的版本化执行契约。它是 Prompt、agent CLI、事件接纳、投影与恢复共同使用的权限和动作真相；任何 activation identity、revision 或 config fingerprint 不匹配都必须拒绝继续执行。

> **See also**: [`docs/explanation/execution-contract-design.md`](./docs/explanation/execution-contract-design.md)（设计说明）| [`docs/explanation/execution-contract-usage.md`](./docs/explanation/execution-contract-usage.md)（使用指南）

### Accepted Transition

业务事件被正式接纳时的唯一状态变化边界：先完成契约、schema、flow、artifact 与投影准备校验，再原子写入可恢复的 transition receipt，随后幂等 materialize task/progress/authority 并发布事件。Reject 不产生业务副作用；commit 后崩溃由 receipt replay 补齐。

### Recovery Intent

可恢复拒收产生的结构化修复责任，至少绑定 activation lineage、contract revision、稳定 reason、责任 hat、允许的修复 primitive、retry key 与剩余预算。它用于下一次 activation 的确定性恢复，不从自由文本诊断消息推断动作。

### follow-on loop

跨两次独立 `ralph run` 的串联：第一环终态成功且交接校验通过后，再启动第二环（另一 preset/config）。与同一 preset 内的 hat 链、以及 `ce-executor-pipeline-loop` 的 review/fix 环不同。近亲行为是 worktree 成功后的 `auto_merge → merge-loop` spawn。

### chain handoff

follow-on 两环之间的强制交接产物：由第一环成功路径写出、启动第二环前校验；缺失或不合格则失败关闭、不启动第二环。正文在业务 artifact；第二环只消费约定路径与必填字段。

### execution.plan.ready

`parallel-forge` 中 task-planner 在成功写入 `.ralph/review/<plan-key>/execution-plan.yml` 之后发出的业务 handoff topic。事件只携带路径、hash 与路由身份字段；DAG 正文留在 artifact。消费方是 `exec-wave-dispatcher`（首次 ready-wave fan-out）。失败路径发 `plan.blocked`，二者在同一 activation 互斥（isolated 单业务事件预算）。

### wave

同一 hat 定义下、对 N 份同构 payload 的 orchestrator 级 fan-out/fan-in。共享 `wave_id`，用 `wave_index` / `wave_total` 区分 slot。需要账本级收齐、超时与诊断时用 wave；hat 内部分工优先 subagent。`events.jsonl` / `supervisor.db` 是 runtime ledger，不是 hat 业务 artifact。

### wave channel registry

Dispatcher 在 spawn worker 之前为本 wave 一次性提交的、按 `(loop id, wave id, slot index, canonical path)` 四元组绑定的私有事件通道授权记录；该记录按 wave 隔离、可幂等清理，runtime 据此回答「本次 slot 是否有权把事件写到指定路径」。每 wave 一份、不是全局 append-only —— 旧式跨 wave / 跨 loop 全局 marker 已被替代。任何 worker 子进程若不在该 registry 中命中绑定，`ralph emit` 收到稳定错误码 `wave_channel_registry_reject` 并拒绝回退到 main。

### authoritative terminal evidence

Supervisor 在 fan-in 阶段判断「某 review / fix 维度是否完成」时**唯一**采纳的依据：slot 终态证据，且终态证据必须通过 topic / dimension / slot assignment / payload fingerprint 四项校验后才算有效。Main ledger 中的孤儿事件即便 topic 看起来匹配，也不能用来证明完成 —— 它们只进入「main 与 store 不一致」的诊断分类，不参与完成性计算。

### projection observation

Fan-in 阶段对 main ledger 的只读扫描结果分类：把 main 中匹配本 wave 终态事件形状的条目，按「与权威完成证据一致 / 缺失 / 孤儿 / fingerprint 冲突」分组。**只是观察**，不作为完成性证据；目的是让 operator 从 main JSONL 看到「store 与投影之间的差异」，例如「store 标记 6 个 slot 失败、main 中只有 5 条 done」时区分出「5 个 orphan」而非「5 个 missing」。缺 / 冲突 / 孤儿都写进结构化诊断，不影响 `missing_dimensions` 的权威结果。

### worker_timeout

Supervisor/wave slot 的稳定失败 reason：该槽 **已进入租约/执行**（或等价 Timeout 出口）后，在 deadline 内未形成可接受终态（无 terminal，或 Timeout 分类路径判定为超时失败）。是调度租约语义，不是「已证明 agent 卡死」。有 `*.unit.done` / `*.unit.failed` terminal 的 Timeout 应 Completed，不得记本 reason。

### slot_never_started

Supervisor/wave slot 的稳定失败 reason：波次失败/取消闭合时，该槽 status 仍为 `Pending`（从未 `Dispatched` / `Running`）。用于区分「排队未开跑」与 `worker_timeout`（已开跑后到期）。不得与「已开跑但未 report」混用同一字符串。

### blocking_slots

`*.wave.failed` 载荷中的失败槽下标列表。契约上只含 `Failed` / `Cancelled` 槽；`Completed` 不得进入。宽泛全量列表（如 `[0,1,2,3,4]`）是诊断反模式，应配合 per-slot reason / diagnostics 归因。

### salvage merge

Supervisor 在注入 `*.wave.failed` 之前，把本波次已 `Completed` 槽的业务事件写入主 ledger 的动作。仍 fail-closed（不是 silent partial complete）：失败协调事件照发，`blocking_slots` 仍只含 Failed/Cancelled；成功槽结果对诊断与后续 focused run 可见。

### slot_retry_budget

`event_loop.supervisor.slot_retry_budget`：同一 public `wave_id` 内、对可重试 slot 失败的自动 redispatch 次数上限（默认 1 = 初始执行后再试 1 次，硬上限 2）。可重试 reason 固定为 4 个 frozen code：`worker_timeout` / `empty_worker_result` / `missing_worker_terminal` / `slot_never_started`（见 `ralph-core::supervisor::worker_outcome::RETRYABLE_REASONS`）。重试在同一 task 内执行，中间 attempt 不写 store（slot 永远不进入 `Failed` 中间态），且中间 attempt 的 progress / RPC / TUI side-effect 被截断（只最终 attempt 的 outcome 暴露给 reporter），让 TUI `wave.completed` 计数与 store 真实 record 保持一致。预算耗尽后槽永久 Failed，进入 salvage / `*.wave.failed` 路径；与 review dimension mismatch 的 `task.resume` 预算不是同一机制。`>2` 启动期 fail-closed。

### ralph wave redrive

Operator CLI：在 supervisor store 上将 Failed 波次的指定失败槽重置并重新进入 Collect/dispatch，不要求（也不允许）靠手工 `ralph emit exec.unit.done` 绕过 FlowStepScope。已写入 `LOOP_COMPLETE` 的 loop 上应拒绝并提示另开 focused run。

### StartToClose（wave worker）

Wave worker 硬顶：自 PTY spawn 起的最长存活时间，对应 hat 级 `timeout`。到期必须 kill，不论是否刚有心跳。与主 loop `cli.idle_timeout_secs` 不是同一字段。

### idle heartbeat（wave worker）

Wave worker 静默窗口（HeartbeatTimeout）：自上次合格进度信号起，超过 `hats.<id>.idle_heartbeat_secs` 无强/弱合格信号则 kill。`0` 或省略 = 关闭，仅 StartToClose 墙钟（legacy）。强信号优先（tool stream / events 文件增长）；弱信号（text/thinking）可续租但受 `idle_weak_signal_cap` 连续次数上限。不要求模型主动调 heartbeat API。与 `cli.idle_timeout_secs`（主 loop PtyExecutor）作用域不同。

### startup grace（wave worker）

Wave worker 冷启动容忍窗口（`hats.<id>.startup_grace_secs`，u32 秒）：仅当 idle 模式启用（`idle_heartbeat_secs > 0`）时生效。在首个合格进度信号到达之前，用 startup_grace 窗口取代 idle 心跳窗口以保护慢热的 backend（如 Claude headless 冷启动）；首个信号到达后即恢复 idle 语义。`0` 或省略 = 关闭，worker 行为与既有 idle 语义一致。超时归因为 `startup_kill`（仍归 `worker_timeout` family，可重试可 redrive）。与 `idle_heartbeat_secs` 是「首信号前 / 首信号后」的关系，作用域不重叠。

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

Loop 外 Skill：对**外仓** E2E 沙箱，验证**当前仓改动 plan**。强制入口
`scripts/bootstrap_pipeline.run_pipeline` / CLI。输入 sandbox + 改动
plan + preset；自动发现沙箱业务 workload 作为 `ralph run --plan`；改动
意图写入 `PROMPT` 且经 `--prompt-file` 进入 agent 可见主 prompt（与
`--plan` 同发）；检查当前仓最新 `ralph`（不新鲜则停）；静态门后交启动
命令。不静默改写沙箱 plan；`presets/` 触达走 `preset_gap` combo-box；
不代跑 live / 不做诊断（`ralph-run-diagnosis`）。实现面仅 skill +
Python，**不改 Rust**（共享 probe 可同时带 `--prompt-file` 与 `--plan`）。

### E2E 沙箱目录

Operator 指定的、用本仓编译出的 `ralph` 对真实 plan 手跑 preset 的可写沙箱（典型为独立 sibling 仓）。**不是**本仓 `crates/ralph-e2e` 测试 harness。

