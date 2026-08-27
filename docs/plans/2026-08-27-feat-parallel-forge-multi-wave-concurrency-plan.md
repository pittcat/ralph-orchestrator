---
title: "parallel-forge 多 wave 高并发调度"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
status: draft
---

# parallel-forge 多 wave 高并发调度

## Goal

让 `builtin:parallel-forge` 在执行计划中存在多个互不依赖的 wave 时同时开工，充分利用 supervisor 的 worker slots；仍然保证依赖顺序、worktree 隔离、按 `integration_order` 的线性集成、失败恢复和最终验证的正确性。

本次已确认的执行模型是 `supervisor+wave`：supervisor 负责跨 wave 的 slot、队列、持久化和恢复，dispatcher 负责按执行计划选择并展开多个 ready wave。

## Current evidence and bottleneck

- `presets/en/parallel-forge.yml` 已启用 `event_loop.supervisor.enabled: true`，`max_concurrent_workers: 8`，executor 的 `concurrency: 8`。
- 但 `forge-dispatcher` 当前要求选择最小的未完成 `wave_index`，并在 `forge.wave.settled` 后才发出下一次 `forge.wave.prepare`；`worktree` 也只创建当前 wave 的 worktree。因此现状是“单 wave 内并发，wave 之间串行”。
- `SupervisorConfig.max_concurrent_workers` 的 runtime 语义已经是跨所有 active waves 的全局 worker 上限；supervisor store 的 `recover_active_waves` 也能返回多个 active wave，说明持久化层不需要重新建模为单 wave。
- isolated event loop 的 wave scope 当前把一个 activation 中的多个 distinct `wave_id` 视为 multi-business-emission 并拒绝；多 wave 调度必须保持每个 wave 独立的 activation/事件批次，不能简单把多个 wave 拼进一个 agent 输出批次。
- 当前 per-wave settlement 会刷新 `verified_base_commit`，而后续 wave 的 worktree 从该 commit 创建；因此只有 DAG 中不依赖前序代码结果的 wave 才能并发。依赖 wave 必须继续等待其前驱 settlement。

## Desired behavior

```text
execution-plan.yml
        |
        v
ready waves = DAG 中依赖已满足、资源审计通过的 wave
        |
        +--> wave A worktrees --> exec slots A --fan-in--> review/integration queue
        +--> wave B worktrees --> exec slots B --fan-in--> review/integration queue
        +--> wave C worktrees --> exec slots C --fan-in--> review/integration queue
                                      |
                                      v
                         integration_order 串行集成
                                      |
                                      v
                       settled base -> 解锁后继 waves
```

并发边界：

- 允许多个 ready wave 同时处于 `Dispatch` / `Collect`。
- 所有 active waves 共享 `max_concurrent_workers`；slot 不足时由 supervisor queue backpressure，不额外创建 worker。
- 每个 wave 使用独立 `wave_id`、worktree map、分支、资源命名空间和 artifact 目录。
- 同一 integration branch 的 merge、fast-forward、`forge.wave.settled` 和 verified base 更新必须按 `integration_order` 串行化。
- 任一 wave 失败只影响该 wave 的 correction/failure 路径；只有无法保证后继 wave base 或全局一致性时，才阻断整个计划。

## Implementation units

### U1 — 建立多 wave 调度状态与全局容量契约

**目标：**明确 runtime 如何同时登记、派发、fan-in 多个 wave，并保证 slot cap、幂等和恢复语义。

**修改范围：**

- `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs`
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs`
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/fan_in.rs`
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/coordination.rs`
- `crates/ralph-core/src/supervisor/coordinator.rs`
- `crates/ralph-core/src/supervisor/mod.rs`
- 必要时新增 `crates/ralph-cli/src/loop_runner/wave/dispatcher/multi_wave.rs`，避免把 dispatcher 单文件继续膨胀。

**设计：**

- 将 dispatcher 的“当前 wave”选择改为“ready wave 集合”选择：按 `execution_wave`/DAG 依赖筛选可启动 wave，再按 `integration_order` 和稳定 `wave_id` 排序。
- 为每个 wave 建立显式生命周期：`planned -> prepared -> dispatched -> collecting -> review/integration -> settled/failed`；重复触发只补缺失动作，不重新创建 worktree 或重复注册 slot。
- 使用 supervisor store 的全局 active slot 数计算本轮可派发容量；一轮可以登记多个 wave，但 `exec.unit.ready` 仍以每个 wave 独立 batch 发出，不能混合不同 `wave_id`。
- fan-in、coord event、salvage 和 recovery 必须按 `wave_id` 隔离；恢复时遍历全部 active waves，而不是只恢复一个当前 wave。
- 加入并发集成闸门：只有取得该 wave 的 integration turn、且其 predecessor 的 settlement 已确认，才能 merge 到 integration branch。

**完成条件：**两个无依赖 wave 可同时进入 supervisor store 并消耗不同 slots；重复 tick、进程重启、slot retry 和单 wave 失败都不会重复 dispatch、重复 merge 或错误关闭其它 wave 的 task。

### U2 — 支持多 wave worktree 与 base commit 生命周期

**目标：**并发 wave 有各自 worktree/branch，同时不让尚未可集成的 wave 覆盖 integration branch。

**修改范围：**

- `crates/ralph-cli/src/loop_runner/wave/worktree*`（按实际模块拆分位置落地）
- `crates/ralph-cli/src/loop_runner/wave/worktree_bind.rs`
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/salvage.rs`
- `presets/en/parallel-forge.yml`
- `presets/schemas/parallel-forge.yml`
- `presets/templates/parallel-forge/` 下的 execution/worktree/settlement 相关模板（若 payload/artifact 契约变化）。

**设计：**

- `worktree-map.yml` 改为可表达多个 active wave 的结构：`wave_id -> verified_base_commit -> unit -> branch/path/resource namespace`，保留旧字段的语义但不允许不同 wave 互相覆盖。
- 并发 wave 从同一个已验证 base 创建自己的 candidate branch；后继 wave 只有在依赖 wave `forge.wave.settled` 后才允许以新 verified base 创建。
- 设计 integration queue/lock 的持久化状态，记录 expected order、当前 turn、候选 commit 和 merge outcome；进程重启后可恢复，不依赖扫描 runtime ledger 猜测状态。
- cleanup 必须按全部 active/settled/failed wave 汇总资源，不能只清理最后一个 wave。

**完成条件：**两个 ready wave 可以同时修改各自 worktree；即使完成顺序相反，集成顺序仍稳定；后继 wave 不会从未验证的 candidate commit 启动。

### U3 — 放宽 isolated runtime 的合法多 wave 边界

**目标：**允许 supervisor+wave 的 dispatcher/worker 处理多个并发 wave，同时保留 isolated activation 防止一个 agent 伪造多个 wave 的保护。

**修改范围：**

- `crates/ralph-core/src/event_loop/wave_scope.rs`
- `crates/ralph-core/src/event_loop/parse_and_emit/step_dispatch.rs`
- `crates/ralph-core/src/event_loop/types.rs`
- `crates/ralph-core/src/event_loop/tests/wave_isolated_scope.rs`
- `crates/ralph-core/src/event_loop/tests/isolated_wave_budget.rs`

**设计：**

- 不取消“一个 isolated activation 只对应一个 wave”的 agent-facing 约束；改为 runtime 在读取批次后按 `wave_id` 分组，逐 wave 形成独立 dispatch group，再交给 supervisor。
- 多 wave 合法性只对 runtime 已确认的 supervisor dispatch provenance 生效；普通 hat 输出仍拒绝同一 activation 中的多个 distinct `wave_id`。
- 每组继续执行 origin、topic、policy、required fields、wave verify 和 idempotency 检查；任何一组失败只产生该 wave 的诊断/恢复 envelope。
- 明确 event ordering：同一 wave 内保持原顺序，不承诺不同 wave 的全局事件顺序；下游通过 `wave_id` 和 projection 关联。

**完成条件：**同一 read batch 含两个合法 supervisor wave 时两组都可被处理；同一普通 isolated hat activation 伪造两个 wave 仍被拒绝；单 wave 既有测试保持通过。

### U4 — 将 parallel-forge preset 改为 ready-set 多 wave 协议

**目标：**让 planner/guardian/dispatcher/worktree/integrator 的 agent-facing 契约表达“多个 wave 可同时执行”。

**修改范围：**

- `presets/en/parallel-forge.yml`
- `presets/schemas/parallel-forge.yml`
- `presets/en/parallel-forge-preset-author-notes.md`（若存在或需补充）
- `presets/scenarios/parallel-forge-*.yml`
- `presets/manifest.yml`、`presets/index.json`：仅在 builtin metadata 或 schema 入口实际变化时同步；preset 名称不变时不新增条目。
- `scripts/ralph-zsh-plugin.zsh`：确认 builtin 名称未变化；无需机械改动。

**设计：**

- `execution-plan.yml` 的 SSOT 继续是每个 Unit 的 `depends_on`、`execution_wave`、`integration_order`；planner 负责生成可并行的 wave，而不是让 dispatcher 临时猜依赖。
- guardian 增加跨 wave 资源审计：共享端口、数据库、容器、缓存和生成文件必须隔离或显式串行；只有满足 DAG 与资源证据的 wave 才能进入 ready set。
- dispatcher 一次 activation 只发一个 wave batch，但可以在多个 activation 中快速准备/派发多个 ready wave；每个 payload 必须携带完整 `wave_id`、`wave_index`、`verified_base_commit`、资源命名空间和 artifact 路径。
- worktree hat 不再把“future waves absent”作为全局不变量，而改为“未 ready/未获 integration turn 的 wave 不得创建”；已 ready 的多个 wave 可以并存。
- reviewer/verifier/integrator 的输入必须以 `wave_id` 为主键；integrator 明确按 `integration_order` 领取 turn，不能按完成时间抢 merge。
- reporter 汇总 `active_waves`/settled waves/failed waves，并记录最大并发 wave 数、最大 worker 使用量和被容量限制的等待量。

**完成条件：**preset lint、AAF、payload audit 能证明每一条跨 wave handoff 都有 projection 和 artifact 消费路径；真实 runtime scenario 能看到两个 wave 重叠执行而不是先后启动。

### U5 — Agent-facing skill、诊断和可观测性同步

**目标：**让 loop 内 agent 知道多 wave 的可执行规则，同时让 operator 能区分“wave 等待依赖”和“slot 容量不足”。

**修改范围：**

- `crates/ralph-core/data/ralph-tools-wave.md`
- `crates/ralph-core/data/ralph-tools-emit.md`
- `crates/ralph-core/data/ralph-tools-cmdref.md`（若命令/输出字段变化）
- `crates/ralph-core/data/ralph-tools.md`
- `crates/ralph-cli/src/commands/inspect.rs`
- `crates/ralph-cli/src/commands/diagnose.rs`
- `docs/guide/cli-reference.md` 或对应 operator 文档。

**规则：**文档只写 agent 当前 activation 能执行的动作：如何从 trigger/projection 获取 `wave_id`、如何读取 ready-set 和 capacity signal、何时停止并报告资源冲突；不泄漏 supervisor DB、events ledger 或内部函数名。

**完成条件：**所有引用的 CLI 参数与 `ralph <cmd> --help` 一致，`scripts/check-cli-doc-drift.sh` 通过；诊断输出能展示多个 active waves 和全局 queue depth，且不暴露内部 ledger 路径。

### U6 — 结构化 runtime/BDD/replay 验证

**目标：**用真实 runtime path 证明并发、依赖、失败、恢复和终态均正确。

**新增或扩展验证：**

- `crates/ralph-cli/src/loop_runner/tests/wave_supervisor/`：多 wave registration、global slot cap、不同完成顺序、integration turn、重复 dispatch、crash recovery、partial failure。
- `crates/ralph-cli/src/loop_runner/tests/wave.rs` 或 dispatcher tests：ready-set 选择、capacity backpressure、wave payload 不混组。
- `crates/ralph-core/src/event_loop/tests/wave_isolated_scope.rs`：合法 supervisor 多 wave 分组与非法 agent multi-wave rejection 的区分。
- `crates/ralph-core/tests/scenarios/parallel_forge_multi_wave_concurrent_runtime.yml`：至少 3 个 Unit，两个无依赖 wave 重叠，第三个依赖前两者 settlement；断言 dispatch 时间/事件先后关系、每 wave 各自 fan-in、integration order 和最终 `forge.report.done`/`LOOP_COMPLETE`。
- failure scenario：wave A 失败或 correction，wave B 成功；断言 B 不被错误重放，A 的失败/修复边界不污染 B，最终报告诚实反映两者状态。
- recovery scenario：进程在两个 wave 分别处于 Collect/Integration 时重启；断言 active waves 都恢复，未完成 Unit 只重派，已提交 merge 不重复。

**验证命令：**

```bash
ralph preset check -H builtin:parallel-forge --strict
cargo nextest run -p ralph-core -- <targeted wave/event-loop tests>
cargo nextest run -p ralph-cli --bin ralph -- <targeted supervisor/dispatcher tests>
cargo nextest run -p ralph-core --test scenarios -- <parallel-forge scenario filter>
./scripts/check-cli-doc-drift.sh
./scripts/run-tests.sh
```

最终交付前必须走仓库规定的 `./scripts/run-tests.sh`，不得用裸 `cargo test` 替代 `ralph-cli` 测试入口。

## Configuration recommendation

第一版不把“wave 数”与“worker 数”分裂成两个互相竞争的硬上限：继续以 `max_concurrent_workers` 作为全局硬 cap，并可增加可选的 `max_concurrent_waves` 作为 worktree/集成压力保护，默认值建议为 `3`，实际有效并发为：

```text
min(max_concurrent_waves, ready_wave_count,
    capacity-aware waves fitting max_concurrent_workers)
```

如果实现阶段发现多个 wave 的 slot 数差异很大，应优先采用“按所需 slot 数的 capacity-aware admission”，不要用简单的 wave 数量造成 slot 饥饿。该配置应在 `SupervisorConfig` 中加 serde/schema/帮助文本/默认值测试，并在 `parallel-forge` preset 显式设置，避免不同 preset 对默认语义产生歧义。

## Non-goals

- 不把有依赖的 wave 强行并发；`execution_wave`/DAG 仍是 planner 的调度 SSOT。
- 不允许多个 wave 同时直接 merge 同一 integration branch。
- 不把多个 wave 的事件拼成一个 agent activation 输出。
- 不在本计划中重写 supervisor 的 SQLite 存储或取消现有 retry/fan-in 语义。
- 不通过单纯提高 `max_concurrent_workers` 宣称完成多 wave 并发。

## Risks and rollback

- **集成冲突：**通过 per-wave candidate branch + integration turn 串行化；冲突进入该 wave 的 correction/blocked 路径，不回滚其它已 settled wave。
- **资源争用：**guardian 在 admission 前审计；executor 对端口、数据库、容器、缓存做 `plan_key + wave_id + slot_index` 命名空间隔离。
- **事件交叉污染：**所有 payload、projection、artifact 路径和恢复键都必须带 `wave_id`；runtime 按 wave 分组处理。
- **恢复复杂度：**先补多 active wave 的 store/bridge 测试，再切换 preset，避免只修改 prompt 后暴露运行时不支持。
- **回滚：**保留 `max_concurrent_waves: 1` 的配置开关作为安全降级路径；即使启用 supervisor+wave，容量 cap 为 1 时行为应等价于当前单 wave 串行流程。

## Review gates

1. 先完成 U1–U3 的 runtime contract 和测试，再改 preset instructions/schema。
2. preset author/reviewer 必须按 `supervisor+wave` capability audit 检查 wave/supervisor 命令、触发上下文、payload required fields、artifact-first handoff 和 integration ownership。
3. 不新增只匹配 YAML/prompt 文本的测试；以 runtime scenario、结构化 schema/lint、supervisor store 和真实 CLI 行为为证据。
4. 复核新增/修改源码文件均未超过 5000 行；必要时拆分 dispatcher/worktree/integration 模块。
5. builtin 名称未改变时不修改 manifest/index/zsh completion，但必须在实现 PR 中记录已检查；若新增配置或事件 topic，则同步所有 schema、BDD、文档和 preset author/reviewer references。
