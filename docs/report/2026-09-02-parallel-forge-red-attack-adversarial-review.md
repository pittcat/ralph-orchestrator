---
title: "parallel-forge 对抗性审查（Red Attack）：为何修了很久仍跑不起来"
date: 2026-09-02
type: adversarial-review
role: red-attack
subject_preset: builtin:parallel-forge
evidence_window: 2026-07-28 .. 2026-09-02
prior_review: docs/report/2026-09-01-parallel-forge-p0-gaps-adversarial-review.md
latest_runs:
  - docs/report/2026-09-02-parallel-forge-2026-09-01-2102-feat-trusted-worktree-continuation-plan-diagnosis.md
  - docs/report/2026-09-02-parallel-forge-2026-09-01-2102-feat-trusted-worktree-continuation-plan-rerun-20260902T115124-diagnosis.md
active_plans:
  - docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md
  - docs/plans/2026-09-01-001-feat-forge-signal-delivery-reliability-plan.md
code_baseline: pittcat-dev HEAD at review time
---

# parallel-forge Red Attack 对抗性审查

> 角色：对抗性审查，不是单次跑后诊断。样本：`docs/report/` 下 2026-07-28 至 2026-09-02 的 parallel-forge 诊断 + 2026-09-01 P0 审核 + 当前 `presets/en/parallel-forge.yml` / schema / runtime 源码对账。
>
> 目的：解释「这个 preset 相关 bug 修了很久，为什么到现在还跑不起来」，并把**现在仍然存在**的问题按层记录清楚。不改代码、不跑 loop。

## 0. 一句话判决

**parallel-forge 现在不是「差最后一个 bug」，而是一条过长、多层各自能单独把整条链掐死的编排，且修复策略长期在修下游症状。**

当前可观测的终态是：多数真实运行在 **wave fan-out 之前** 就被 fail-close / BLOCKED；即便偶然冲进 wave，**slot 完成真相也进不了主账本**。09-02 两次跑 `trusted-worktree-continuation` 都没碰到 executor。这不是 agent 写不好代码，是编排在「还没开始写代码」时就已经把自己判死。

## 1. 历史样本更新（相对 09-01 审核）

| 窗口 | 业务成功 | 主导死法 |
|---|---|---|
| 07-28 → 08-29（16 次，见 09-01 审核） | 4 | 信号丢失、payload 手误、终态契约、PTY 误杀 |
| 09-01 lucky-reed | 0 | worktree_setup 阶段 `flow_unknown_emit` → `forge.plan.blocked` |
| 09-02 重跑 `T115124` | 0 | `forge.worktrees.ready` 已落账，**dispatcher/executor 从未激活**，随后 empty channel + recovery 连击 + 人工 abort |

09-01 审核的最大 gap（「执行真相 vs 账本真相」）**仍然成立**，但被一层新的、更早的卡点盖住了：

**现在连 dispatcher 都经常到不了。** 修 wave 投递解决不了「根本没发出 `exec.unit.ready`」。

## 2. 当前修复账本（代码已动 vs 计划文书 vs 真实跑）

这一层本身就是 red-team 目标：计划状态与代码、与现场互相说谎。

| 工作 | 计划文件怎么写 | 代码现在怎样 | 现场 |
|---|---|---|---|
| 证据门禁 2026-08-27-1430 | 仍 `status: active`，§0 写「主仓尚未实现」 | preset 已有 6 条 LLM precheck + 一批 `payload_consistency`（worktrees / wave / review / settled / audit / work.failed） | 把门禁加在 **fan-out 正前方**，把失败点从 wave 中段前移到 worktree/gate |
| 信号投递 2026-09-01-001 | 仍 `READY`，声称未执行 | **部分已合入**：slot payload 持久化（`record_slot_event_payloads`）、启动补偿投递、超时 wave 注入 `exec.wave.failed`、hard gate 开始读 merge/backend 事实、`record_slot_pid` 已接线、worker 不再先删 channel | 09-02 两次跑都没进 wave，**这条修复没有被现场验证** |
| `apply_precheck_desugar` 同步 `allowed_emits` | 09-02 诊断当成根因 | 当前 `ralph_config.rs` **已经**把 `.proposed` / `.rejected` 加进当前 step 的 `allowed_emits` | 重跑仍 BLOCKED，说明「desugar 不同步」即使曾是真因，**也不是现在唯一的死法** |
| `restore_unmerged_completed_slot` | 09-01 标死代码 | 仍 `#[allow(dead_code)]`；真正补偿走的是另一条 `redeliver_persisted_slot_events` | 未在 09-02 路径上触发 |

红方结论：你们不是没修，而是 **计划不收口、修复与现场不同步、每次合入都在拓扑上再加一门**。现场永远在测「上一轮修过的点的下一毫米」，测不到 wave 主路径。

## 3. 当前阻断链（按真实运行顺序攻击）

下面按 **一次 `ralph run -H builtin:parallel-forge` 实际要活过的关** 列。任意一关失败，后面全是空。

```
inspector → planner → guardian
    → worktree emit ready
    → [LLM precheck gate ×1]          ← 09-01 lucky-reed 死在附近
    → forge-dispatcher 必须被激活     ← 09-02 重跑死在这里
    → ralph wave emit exec.unit.ready
    → supervisor JoinSet / PTY slots
    → slot 事件进主账本               ← 08-26 / 08-29 死在这里
    → reviewer → integrator → verifier → settle
    → （多波循环，每波再走一遍）
    → tester → auditor → [audit precheck] → finalizer FF merge
    → cleanup → reporter → LOOP_COMPLETE
```

16 个业务 hat + 最多 6 个合成 precheck hat。`max_iterations: 60`。单波约 9 个业务事件，还要求 agent 手工透传 `wave_id` / `plan_key` / 指纹。

### 3.1 现在就能一击致命的点（P0）

#### P0-A  fan-out 入口：`forge.worktrees.ready` 接受了，dispatcher 仍不出现

**证据**：09-02 重跑 current-events 有两条 `forge.worktrees.ready`，activation 表只有 inspector/planner/guardian/worktree/cleanup/reporter，**没有 `forge-dispatcher`**。tasks 5/5 failed，executor 0 次激活。

**预设侧**：dispatcher 的 `triggers` 明确包含 `forge.worktrees.ready`（`presets/en/parallel-forge.yml` 约 900–903 行）。按拓扑，accepted ready **必须**唤醒它。

**攻击面（未在 DT7 下定论，但是可被利用的缺口）**：

1. **precheck 把一次 handoff 拆成两次 isolated activation**。worktree 实际落到账本的是 `forge.worktrees.ready.proposed`，真正的 `forge.worktrees.ready` 由合成 hat `precheck-forge.worktrees.ready` 转发。任何 gate 空 channel、scope 拒绝、单事件预算误伤，dispatcher 都收不到 fan-out 信号。
2. **worktree 被激活两次**（重跑 merged 里 `worktree×2`）。第二次若仍发 `forge.worktrees.ready`，此时 flow 已进入 `development_loop`，该 step 的 `allowed_emits` **不含** `forge.worktrees.ready`（只有 `forge.wave.worktrees.ready` 等），`FlowStepScopeStage` 精确相等 → `flow_unknown_emit`（`flow_step_scope_stage.rs` `allows_topic`）。这与重跑 recovery 里的 `isolated_scope_violation` / `flow_unknown_emit` 同族。
3. **空 activation 被记成 hat `ralph`**（重跑 empty 行含 `ralph`）。若 dispatcher 被错误归因到默认 hat，hard gate 会把「调度 hat 没发出 wave」判成「有发布义务但没事件」。
4. **诊断因果门禁把这个 P0 变成「官方不存在」**：两次 09-02 报告 `causal_status` 为 `incomplete` / `not_evaluable`，§5 空表，§6 禁止给建议。于是现场死了，仓库里没有可执行的根因单。

这是 **当前「跑不起来」的最近阻断**。比 08-29 的 slot 丢事件更靠前。

#### P0-B  信号投递链仍无端到端保证（09-01 P0-1 未闭合）

即便哪天 dispatcher 活了，08-29 那类死法仍然合法：

- 健康路径 fan-in **仍读 dispatcher 内存**（计划自己写了约束 D2：不得改变健康路径事件来源）。
- `restore_unmerged_completed_slot` 仍是死代码。
- isolated hat-channel：空 channel → quarantine + Err；merge IO 失败 → 诊断后换新 iteration 文件名，旧文件不再被消费（`hat_channel.rs`）。
- hard gate 虽已开始读 merge/backend 事实（U4），**「3 次无事件 → fail-close」仍在**。空 dispatcher / 空 gate / 空 worktree 重试仍然能把 loop 打死。
- 无周期 reaper：running slot + 外部 kill 仍可靠超时（3600/7200s）或人工 abort。pid 接线只改善诊断，不收敛 wave。

#### P0-C  关键上下文靠 agent 手工透传，schema 与 instructions 对不齐

未变，且被 07-30 / 08-05 实证打死过：

- reviewer instructions 要求从 trigger 读 `wave_index`、`plan_key`；`exec.wave.complete` schema **只有** `wave_id` / `completed_slots` / `merge_root_event_id`（`presets/schemas/parallel-forge.yml`）。
- failure-handler 自己承认 `exec.wave.failed` **没有** `plan_key` / `wave_index` / `execution_plan_path`，要靠「投影」恢复。
- 投影 SSOT 写在顶层 `state_projection:`（yml 约 1797 行）。`RalphConfig` **没有这个字段**，serde 静默丢弃。真正的运行时投影是 `event_loop.state_projection`，本 preset **未启用**（default `enabled: false`）。
- 因此：「从全局 projection 读 `forge.wave.current_wave_id`」是 **对 agent 的假接口**。并发波次下单例 `forge.wave.current_wave_id` 就算接线也会误伤。
- `CloseTaskBatch` 被写进 integrator / dispatcher 叙事，但 live projector 未 opt-in。executor 又被禁止 `task close`。波次一旦跑完，**任务账本没有法定关闭者**——这是 08-05 拓扑死锁的同构缺口，现在还在。

#### P0-D  correction 轮次没有 runtime 计数器

failure-handler 仍从磁盘文件 `corrections/counter` 读轮次。runtime 唯一硬锚点是 `forge.final.correction.settled` 的 `allowed_values: correction_round=[3]`——只能卡单事件，不能跨事件计数。counter 丢了就超轮或永远发不出 settled。

90/75 置信度在 `work.failed` 上已部分进 schema/`required_fields` + precheck，比 09-01 好；**轮次本身仍是 prompt + 文件**。

#### P0-E  诊断系统与 preset 一起失败（元 P0）

几乎所有 parallel-forge 报告都是 MINIMAL / causal 不达标：

| 作用 | 后果 |
|---|---|
| DT7 `confidence > 85` 才许进 §5 | 09-02 两次报告 **零条官方根因** |
| MINIMAL 不写 orchestration / agent-output | OPAC Confirm 永久残废 |
| cap manifest / boundary_coverage 空 | `--causal` → `not_evaluable` |
| flow-authority stale-tail | recovery 被旧 `forge.plan.blocked` 污染（08-26 / 08-08 / 09-02 反复） |

红方利用方式：只要把故障做在「诊断契约覆盖不到」的缝上（gate 合成 hat、empty channel、双账本不一致），你们的修复流程会 **合法地拒绝产出修复建议**。这就是「修了很久仍说不清下一刀砍哪」的机制原因。

### 3.2 P1 — 放大故障率，不需要新的现场也能预测

- **`runs: forge.development_loop` 不是 runner 绑定。** runtime 只认 `wave.runtime.*` / `supervisor.*` 前缀。`kind: loop` 也不参与 `advance_plan_step`。这是死配置，作者以为有子流程运行时，实际只是一个带 `transition_emits` 的普通 step。
- **`business_topics` 漏掉几乎全部 wave 内事件**（`forge.wave.reviewed/integrated/verified/settled`、`forge.correction.*`、`exec.wave.*`、`forge.full.verification.failed`…），却仍躺着死 alias `forge.incremental.verified` / `forge.units.reviewed` / `forge.integration.done`。`terminal_monotonicity` 只检查该列表——wave 事件在 LOOP_COMPLETE 之后可以钻空子；cleanup 二次 emit 则会被打（09-02 lucky-reed orphan cleanup）。
- **hat `triggers` 仍是纯 topic 字符串**。`TriggerPredicate.payload_field_equals` 只服务激活后的 emit 义务，不做路由。`forge.audit.done` 的 REJECTED/BLOCKED 现靠 **precheck「只放行 ACCEPTED」+ auditor instructions** 软挡 finalizer；gate 被绕过或 exhausted 注入路径一旦漂移，finalizer 仍会对任意 `forge.audit.done` 做 `git merge --ff-only`。
- **重复 emit / 无幂等**：verifier ×3、`exec.unit.done` ×3、`forge.worktrees.ready` ×2。isolated 单事件预算吞掉的是 **后续合法推进事件**（07-28 dispatcher 从未 spawn 的那种）。
- **budget 算术**：`slot_retry_budget: 2` × 3600s 先撞 `aggregate_timeout_secs: 7200`；`max_iterations: 60` 对 16+6 hat × 多波不够。失败被包装成 timeout / no-progress，而不是「预算配错」。
- **work.failed 的 `retry_budget: 3`（多处 on_fail）** 与「work.failed 是终态死信」并存。07-29 settlement 专项已写明。agent 会以为失败能重试。

### 3.3 P2 — 卫生与清理，每次失败都把工作区弄得更脏

- cleanup 订阅 legacy `forge.units.reviewed`：误触发即空转。
- LOOP_COMPLETE 后 cleanup 再 emit → `terminal_monotonicity_violation`，但事件仍可能进 ledger（双账本，09-02 DEV-003）。
- branch / worktree 残留（08-05 11 个 refs；09-02 诊断时 worktree 已被 prune、map 还在）。
- reporter 被允许走 BLOCKED 闭环，看起来「preset 成功结束了」，operator 得到一份经理报告，**代码零交付**。

## 4. 分层攻击面（举一反三）

不要再按「某一个 hat 的 bug」思考。按层打：

### 4.1 Preset 层 — 拓扑过度、契约撒谎

- 16 hat supervisor 拓扑把「并行开发」拆成必须全部正确的串行状态机；**并行只存在于 dispatcher 之后**，而现场经常到不了 dispatcher。
- instructions 引用 projection / CloseTaskBatch / trigger 字段，**运行时没有对应消费端或 schema 字段**。这违反 HARD RULE 4（hat 只能依赖本 activation 可见的东西），且 lint 仍不查 instructions 视角。
- 证据门禁加在 fan-out 前：本意挡垃圾 payload，副作用是 **多一次 LLM isolated 进程**。gate 的空 channel 与 producer 的空 channel 对 hard gate 长得一样。
- 顶层 `state_projection:` 是给作者看的小说，不是给 runtime 的配置。

### 4.2 Runtime 层 — 单向投递 + 精确字符串 + fail-close

- Flow scope **精确匹配** topic 字面量。desugar、`.proposed`、legacy alias、`forge.wave.worktrees.ready` vs `forge.worktrees.ready` 任意一个漏同步，就 `flow_unknown_emit`。09-01 lucky-reed 是这一刀；重跑是第二刀（step 已前进，旧 topic 非法）。
- Isolated 单事件预算 + 3 次 hard gate：可恢复的「这轮没写出合法 JSONL」被升级成计划死亡。
- Supervisor wave：完成态事件在 fan-in 前不是主账本的一等公民。崩溃窗口 = 已完成 slot 蒸发。09-01 计划修了一部分，**健康路径仍走内存**。
- Hat 选择失败没有一等信号。dispatcher 没被选中时，loop 只会表现为 empty / no-progress / 人工 abort。

### 4.3 Agent 层 — 被要求做 runtime 该做的事

- 手工拼 `execution_wave`、数组 vs 逗号字符串、`failure_fingerprint` 固定格式、SHA 指纹算法（worktree instructions 里一整段 porcelain canonicalization）。任何一字节错误 = 死锁，不是「再试一次」。
- 07-30 planner off-by-one、08-05 integrator 字符串数组，都是 **把类型系统放进 prompt** 的必然产物。再加 precheck 只能拦住一部分空字段，拦不住语义 off-by-one。

### 4.4 可观测性层 — 让红队隐身

- 默认诊断弱；出事时 causal 评不上 85，修复建议被技能硬规则禁止。
- `hat_activation_outcome` 有 `empty`，但技能规定单凭 empty **不能** 定「agent 未 emit」。于是 7 个 empty 摆在桌上，仍然「unknown」。
- 双账本（events accepted vs stage reject）出现时，诊断选择降级而不是升 P0。

### 4.5 过程层 — 计划膨胀，收口为零

- 同时存在两份 `active` 计划，状态互相矛盾。
- 09-01 审核已指出 evidence-gates **明确把 stall 根因划出范围**；随后现场继续死在范围内外的缝上。
- 诊断报告越写越长，§5 越来越空。精力进了 `docs/report/`，没有进「下一刀必杀的单一机制」。

## 5. 「看起来像成功」的陷阱

红队最想要的不是 panic，而是 **BLOCKED 闭环**：

1. 在 worktree_setup 或 gate 处触发 `forge.plan.blocked`。
2. cleanup `retained_for_diagnosis` + reporter `status=BLOCKED` + `LOOP_COMPLETE`。
3. operator 看到 loop「完成了」，经理报告 2 万字节，HEAD 仍是计划基线。
4. 诊断因为 causal 不达标，不产出修复单。
5. 下一轮再加一门禁或再写一份 plan。

09-02 两次跑完整演示了这个循环。

## 6. 与 09-01 审核的差异清单（避免把已修的当未修）

| 09-01 条目 | 2026-09-02 对账 |
|---|---|
| desugar 不改 `allowed_emits` | **已改**（加 `.proposed/.rejected`）；不能再当唯一根因 |
| `record_slot_pid` 无生产调用 | **已接线**（dispatch.rs，失败 warn） |
| worker 退出即删 channel | **改为 persist-before-delete** |
| 超时 recovery 不注入 `exec.wave.failed` | **已有** `timed_out_pending_injection` + inject |
| slot 事件只在内存 | **有** store 持久化 + 启动 redelivery；健康 fan-in 仍读内存 |
| `forge.audit.done` 无 allowed_values | schema **已有** verdict 枚举；finalizer 仍无 payload 路由过滤 |
| `work.failed` 90/75 仅 prompt | **已进** required_fields + precheck |
| 顶层 state_projection 死配置 | **仍死** |
| business_topics 漏 wave 事件 | **仍漏** |
| correction counter 文件 | **仍在** |
| reviewer 读 trigger 里没有的字段 | **仍在** |
| hat-channel 空 → 硬归因 | U4 分类开始落地，**fail-close 三次阈值仍在**；09-02 仍被 empty 打死 |

## 7. 建议砍法（只记录，不执行）

按红队会优先守的顺序，反过来砍。**一次只砍能让下一次跑过 dispatcher 的那一刀。**

1. **先打通 fan-out 入口（P0-A）**  
   用最小复现（mock backend / BDD）证明：accepted `forge.worktrees.ready` 之后 **下一次 hat 必须是 `forge-dispatcher`**。若不是，修 hat 订阅 / precheck 转发 / flow 步进，而不是再加 gate。在这之前不要再跑真实 16-hat loop。

2. **关掉或短路 fan-out 前的 LLM precheck**，直到 dispatcher 稳定出现。结构检查（空字段 consistency）可以留；多一次 isolated LLM 现在是在给 hard gate 送弹药。

3. **把顶层 `state_projection:` 要么删掉要么迁到 `event_loop.state_projection.enabled: true` 并接 CloseTaskBatch。** 现在这是假 SSOT。

4. **补 reviewer / failure-handler 的真实 trigger 字段，或改 instructions 只读 artifact 路径。** 禁止「trigger 里有 wave_index」这种与 schema 相反的句子。

5. **闭合 09-01-001**：健康路径也要能从 store 重建；`restore_unmerged_completed_slot` 要么接线要么删除。计划文件改 `done` 或改范围，停止「READY 但代码已半落地」。

6. **诊断：MINIMAL + causal not_evaluable 时允许一条「编排未激活」P0**，不要用 DT7 把「dispatcher 零激活」写成 unknown。否则修复飞轮转不起来。

7. **P0-D correction 计数、P2 清理、legacy alias** 全部排在「至少一次真实 wave 跑完」之后。继续在这些点上开新 plan，就是红队想看到的资源分散。

## 8. 明确不建议的做法

- 再给 `exec.unit.done` 加 guard（两条 active plan 都禁止，且会让丢事件更难 salvage）。
- 再增加 hat / 再增加 precheck topic。
- 再写一份不改 `docs/plans/` 状态机的「全面重构」计划。
- 用另一轮真实 `ralph run` 当验证——在 P0-A 没有单测锁死之前，这只是再买一次 BLOCKED 报告。

## 9. 结论

parallel-forge 相关层的问题不是「还有几个漏网 bug」，而是：

1. **入口关**（worktree → precheck → dispatcher）已经能独立杀死 loop；09-02 两次都死在这里。  
2. **投递关**（slot → 内存 → fan-in）即使入口修好，08-29 类故障仍合法。  
3. **契约关**（schema / instructions / 死投影 / 手工透传）保证 agent 会再次制造 07-30 / 08-05 类死锁。  
4. **诊断关** 保证死了也开不出官方修复单。  
5. **计划关** 保证同时修两件正交的事，且文书与代码不一致。

红队不需要找新漏洞。重复触发上述任一关，这个 preset 就会继续「跑不起来」。
