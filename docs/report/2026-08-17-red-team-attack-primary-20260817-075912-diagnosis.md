---
title: red-team-attack Loop `primary-20260817-075912` 运行链路诊断报告
date: 2026-08-17
type: diagnosis
loop_id: primary-20260817-075912
preset: builtin:red-team-attack
run_dir: .
status: 只读约束被绕过：workspace 出现未跟踪源码文件
diagnostics_mode: MINIMAL
history_search: preset-only
---

# red-team-attack Loop `primary-20260817-075912` 运行链路诊断报告

> **诊断对象**：`.ralph/`，loop 从 2026-08-17 15:59:12 启动。
> **对照 preset**：`presets/en/red-team-attack.yml` 与 `presets/schemas/red-team-attack.yml`。
> **执行方式**：isolated 单链；本次诊断在主线程完成。
> **Diagnostics 模式**：MINIMAL。
> **history_search**：preset-only，扫描近 30 天与 red-team/preset/loop 相关报告。
> **报告仓库**：`ralph-orchestrator` 主仓。

## 0. 产物盘点（Phase 0）

**execution_capabilities**：`[single-chain]`。preset 设置 `event_loop.execution_mode: isolated`，未启用 supervisor；preset 和可信 events 中没有 `ralph wave emit`、`ralph wave verify` 或 `wave_id`。`.ralph/supervisor.db` 虽存在，但不是本次 preset 的 supervisor 能力证据，属于已有运行状态。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `.ralph/events-20260817-075912.jsonl` | 是 | 13 | 唯一可信 events 指针；链路推进到 3 个 `redteam.experiment.done` |
| S | `.ralph/events-history-20260817-075912.jsonl` | 是 | 1 | 配对 warmup 输入 |
| S | `.ralph/ledger.jsonl` | 是 | 24 | 本 workspace ledger，含其他运行残留，未单独作为本次事件源 |
| S | `.ralph/recovery.jsonl` | 是 | 38 | 本 workspace recovery；本次最新诊断 session 只新增 agent_doc_sync 信息项 |
| A | `.ralph/agent/tasks.jsonl` | 否 | tasks disabled | 符合 preset 配置 |
| A | `.ralph/agent/summary.md` / `handoff.md` | 是 | 存在 | summary/handoff 主要反映其他已结束运行，不覆盖本次 trusted events |
| B | `.ralph/diagnostics/2026-08-17T15-59-12/` | 是 | MINIMAL | 有 trace/runtime-trace/recovery/drift 等，无 orchestration.jsonl |
| C | `.ralph/red-team/**` | 是 | 多阶段产物 | target lock、plan resolution、patch、experiment/evidence 均存在 |
| C | `examples/compute_scope_digest.rs` | 是 | 24 行，未跟踪 | 不在 `.ralph/red-team/` 内；当前工作区唯一异常变更 |

## 1. 结论摘要

### 1.1 健康度

- **判定**：部分偏离，且存在 P0 级只读边界违规。
- **P0/P1 数量**：P0 1，P1 1（均达到置信度门槛）。
- **最高根因置信度**：P0-1 = **85/100**，受 MINIMAL 模式和缺少 agent-output 证据的硬顶限制。
- **历史复发**：是。同类 red-team 运行历史反复出现“prompt 约束与实际执行环境脱节”，但本次新增的具体未跟踪文件只由本次时间线确认。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分合规 | events 和 runtime trace 显示 activation 正常合并；但 workspace 写入违反 preset 的只读边界 | 75 |
| Q2 | 基座机制是否正常生效？ | ✅ 基本正常 | plan-resolver 的 `redteam.plan.resolved` 被接受，实验事件也被正常接受；没有证据表明 Ralph 事件/路由机制导致文件写入 | 80 |
| Q3 | 编排是否合理、正常运行？ | ❌ 安全边界不完整 | plan-resolver 能在真实主 workspace 执行，且没有硬性 write allowlist；随后实验只把污染当作事前状态 | 85 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **preset/机制边界缺口为主，agent 行为为触发因素** | preset 明确禁止写代码，但执行器只提供真实 workspace cwd，没有文件系统隔离；文件在 plan-resolver 阶段出现 | 85 |

### 1.3 根因一句话

`red-team-attack` 把“只能写 `.ralph/red-team/`、不能改代码”写成了 prompt 约束，却让 plan-resolver 在真实 workspace 中运行；agent 为完成 `scope_digest` 计算写出了 `examples/compute_scope_digest.rs`，runtime 没有拦截该写入。

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| **首轮终态** | 可信 events 在本次诊断截取点只推进到 `redteam.experiment.done`，没有可信的 `redteam.complete`；因此业务终态证据不足。 |
| **恢复状态** | 未发现失败终态后的成功恢复。 |
| **最终代码状态** | HEAD=`3d2a4b43bb20f2b9542c6bd2bd44e27c37ab2657`，tree SHA=`039de86b606defe79fa9ecf264400a8629772e60`；tracked tree 未变，但存在未跟踪文件 `examples/compute_scope_digest.rs`。 |
| **一致性告警** | “tracked tree unchanged”不等于“workspace 未被修改”；本次实验记录只证明 tracked 文件和 HEAD/tree 未变，不能证明 workspace 完全只读。 |

## 2. 执行链路对比

| 顺序 | Hat | 可信证据 | 结论 |
|---:|---|---|---|
| 1 | loop-bootstrap | `.ralph/events-history-20260817-075912.jsonl:1` | 接收 `redteam.start`，输入要求所有实验文件只能写 `.ralph/red-team/` |
| 2 | target-locker | `.ralph/red-team/01-target-lock.md` | 15:59:12 锁定 HEAD/tree，记录当时 workspace clean |
| 3 | plan-resolver | runtime trace `sequence=6..10`；`02-plan-resolution.md` | 在 16:01 后运行，16:04 产出 resolution；随后生成 patch/scope 产物 |
| 4 | 未跟踪文件出现 | `stat examples/compute_scope_digest.rs` | 创建于 16:15:55，早于 scope manifest 16:26:04，处于 plan-resolver 的工作窗口 |
| 5 | experiment-runner | `RTE-001/state_before.txt:4` | 17:03 的首次实验明确把该文件记录为既有 `UNTRACKED_BEFORE`；实验未创建它 |
| 6 | 后续实验 | `RTE-002/state_before/after.txt`、`RTE-003/state_before/after.txt` | 文件持续存在；实验只验证 tracked tree/HEAD/tree 未变化 |

## 3. 历史问题上下文

本次使用 `history_search=preset-only`，扫描近 30 天相关报告。历史中已有多个 red-team 运行报告指出实验计划、测试目标和真实执行环境之间存在脱节，例如：

- `docs/report/2026-08-17-red-team-attack-primary-20260816-151130-diagnosis.md`：实验命令和真实测试目标不一致，导致 control collapse。
- `docs/report/2026-08-10-red-team-attack-red-teamprompt-cool-falcon-diagnosis.md`：scope digest 交接与 artifact contract 不稳定。
- `docs/report/2026-08-15-red-team-attack-primary-20260814-144832-diagnosis.md`：历史上依赖 tracked-tree 检查来判断代码安全边界。

这些历史报告支持“preset/执行环境边界反复依赖 agent 自律”的复发判断，但没有证明本次源码文件由历史 run 创建。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 初估 | 计分项 | 缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | workspace 出现不在允许目录中的源码文件 | `examples/compute_scope_digest.rs`；`stat`；`.ralph/red-team/evidence/RTE-001/state_before.txt:4` | P0 | 85 | file/time 证据 +25；preset 行号 +15；Tier C 交叉验证 +10；历史同类 +10；MINIMAL 硬顶 | 缺 FULL agent tool-call |
| DEV-002 | 只读要求未被 runtime 文件系统边界强制执行 | `crates/ralph-adapters/src/cli_executor.rs:123-146`；`pty_executor.rs:340-357` | P1 | 80 | file:line +25；preset 行号 +15；双侧执行路径 +20 | 缺专门 BDD/硬拦截测试 |

### 4.1 OPAC 逐 hat 审计

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| target-locker | ✅ | ✅ | ✅ | ✅ | target lock、accepted event、runtime trace | 80 |
| plan-resolver | ✅ | ❌ | ✅ | ✅ | `02-plan-resolution.md`/`scope-manifest.json` 存在；外部源码文件不符合写入范围 | 70 |
| attack-surface-mapper | ✅ | ✅ | ✅ | ✅ | `.ralph/red-team/04-attack-surface.md`、05 plan | 75 |
| experiment-plan-validator | ✅ | ✅ | ✅ | ✅ | validation artifacts；将既有未跟踪文件误视为 out-of-scope | 70 |
| experiment-runner | ✅ | ✅ | ✅ | ⚠️ | `state_before`/`state_after` 记录 tracked tree 未变，但检查不足以覆盖 untracked workspace mutation | 75 |
| evidence-gate | ✅ | ✅ | ✅ | ⚠️ | 正常推进队列；未升级已有 workspace 污染 | 70 |

**Prompt visibility 对账（plan-resolver）**：`ralph ... inspect prompt --hat plan-resolver --format json` 显示 `ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac` 为 auto-inject，`ralph-tools-emit` 等为 on-demand；未发现“instructions 声称某 on-demand skill 已自动注入”的矛盾。问题不是 visibility 缺失，而是 filesystem 写入没有 runtime 强制边界。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 计分项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | 只读 red-team run 在真实 workspace 创建了 `examples/compute_scope_digest.rs`；preset 的“只写 `.ralph/red-team/`”没有成为硬边界 | **compound：preset/机制边界 60% + agent 触发 40%** | **85** | DEV-001 + DEV-002 | file:line +25；preset 行号 +15；Tier C +10；历史 +10；双执行路径 +20 | 高：同类环境脱节反复出现 | 第1轮 preset 行号；第2轮 runtime 执行器源码 +历史 |
| P1 | 实验 runner 只检查 tracked diff/HEAD/tree，未把 untracked 文件纳入“workspace unchanged”门禁 | **preset** | **80** | `red-team-attack.yml:750-758`；RTE-001 state_before/after | file:line +25；preset 行号 +15；双账本/双状态 +20 | 中：历史报告多次把 tracked tree clean 当作安全结论 | 第1轮 preset；第2轮 artifact 对账 |

## 6. 修复建议

### 6.1 短期（operator workaround）

- 在继续运行前保留并人工确认该文件的归属；不要直接删除用户文件。若确认它是本次 run 生成的临时文件，再由用户手动移入 `.ralph/red-team/` 的证据目录或删除。
- 只读 preset 运行使用 `--worktree` 或独立临时 clone，避免 agent 直接拥有主 workspace 写权限。
- 运行前后同时记录 `git status --porcelain=v1 --untracked-files=all`、`git diff --exit-code`、文件清单/快照；不要只检查 HEAD/tree。

### 6.2 中期（preset/schema/instructions）

- 明确把 `plan-resolver` 的 digest 计算限定为 shell/现有工具或 `.ralph/red-team/` 内临时文件，禁止在 `examples/`、`src/`、`tests/` 等项目目录创建辅助源码。
- 将实验门禁中的“tracked tree unchanged”改为完整 workspace cleanliness contract，至少包含 untracked 文件快照与新增/删除路径对账。
- 若检测到启动后新增的 workspace 路径，立即写 `.ralph/red-team/failures/<stage>.md` 并发 `redteam.failed`，不要继续把该文件当作“启动前状态”。

### 6.3 长期（机制/底座）

- 为 read-only preset 增加 runtime 级文件系统隔离：将 agent 子进程放进临时 worktree/clone，或提供 OS sandbox/受限 bind mount，只允许写 `.ralph/red-team/` 和 runtime 必要目录。
- 在执行器/loop runner 增加可配置 write policy，并在进程退出时对 workspace snapshot 做 fail-closed 校验；prompt 只作为补充提示。
- 增加真实 runtime integration test：agent 尝试写 `examples/probe.rs` 时必须被拒绝或 run 进入失败终态；写 `.ralph/red-team/evidence.txt` 应允许。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| 是否存在某个更早的人工命令在 16:15:55 创建该文件 | 45 | 缺完整 agent tool-call / OS audit 记录 | 已查 shell history、trusted events、artifact timestamps；未发现人工命令 |

