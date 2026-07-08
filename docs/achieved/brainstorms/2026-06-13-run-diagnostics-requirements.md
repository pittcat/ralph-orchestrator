# 运行链路诊断能力 (Run Diagnostics) — 需求文档

> 状态：草案（Phase 3 v2，待 Phase 4 选 handoff）
> 日期：2026-06-13
> 目标读者：preset 作者、loop 机制维护者、产品/工程 manager

## TL;DR

为 ralph-orchestrator 加一套**事后诊断能力**。机制很轻量：

1. **新加 1 个 builtin preset**：`presets/en/diagnose-run.yml`（4-hat isolated，自指式诊断）
2. **现有 preset 在 PROMPT.md 顶部加 `## DIAGNOSTICS MODE` 段**：用户写"跑完后请诊断 `<run-dir>`，对照 `<preset>`"
3. **该 preset 的 hat instructions 末尾追加一段"诊断收尾 SOP"**：调起诊断 preset
4. **报告落 `docs/diagnoses/YYYY-MM-DD-<preset>-<run>.md`**，固定 6 段、人话、manager-facing

**不新增 CLI subcommand、不新增 skill、不新增 cli 命令**——所有能力放在 preset YAML + PROMPT.md 里。

## 1. 背景与动机

### 1.1 现状痛点

跑完 `ralph run`，结果经常出现以下之一：

1. **机制 bug**：loop 状态推进错、event 没 dispatch 到 hat、required_events 没触发、plan-gate 卡死、queue 顺序错
2. **preset bug**：事件流设计有缺口、hat 触发条件矛盾、payload schema 不一致、terminal_events 没声明、step 顺序断裂
3. **agent 执行 / 产物问题**：events.jsonl 有事件但 payload 不合规、tasks.jsonl 和 progress.md 对不上、findings/fix-log/report 没闭环
4. **多因素叠加**：preset 设计诱导 agent 走错路径，机制又把错路径放大

目前用户拿到结果后**只能自己逐文件对账**——`.ralph/agent/tasks.jsonl`、`.ralph/diagnostics/*/events.jsonl`、`progress.md`、`findings.md`、`fix-log.md`、`report.md`、preset YAML 散落在不同地方，肉眼拼链路 + 归因效率极低，manager 视角更是看不见全貌。

### 1.2 现在用户的兜底（counterfactual）

- 啥都不查，靠记忆判断是否成功
- `cat events.jsonl` / `tail diagnostics` 手动对账
- 写一次性 shell 脚本事后即弃

这些都不沉淀、不汇报 manager、不驱动 preset/机制迭代。

## 2. 目标与非目标

### 2.1 目标

- **G1**：每个 preset 在自己的 `PROMPT.md` 顶部加 `## DIAGNOSTICS MODE` 段，写明"诊断目标路径 + 对照 preset 名"
- **G2**：跑完 preset 的最后一个 hat，**自动调起** `diagnose-run` preset
- **G3**：`diagnose-run` 跑完后，输出 report.md 到 `docs/diagnoses/YYYY-MM-DD-<preset>-<run>.md`
- **G4**：覆盖 5 个诊断维度（事件循环/state machine、preset 校验/origin/schema、tasks·progress·memories、diagnostics 观测、worktree·merge queue·lock）
- **G5**：报告固定 6 段、人话、manager 5 分钟能读完
- **G6**：诊断本身是新 4-hat isolated preset，不污染被诊断 run

### 2.2 非目标

- **N1**：**不做实时诊断**（loop 还在跑时不强制干预）
- **N2**：**不做自动修复**（只出 plan，不改 preset / 不改基座）
- **N3**：**不做 LLM-as-judge 主观打分**（所有判断基于文件/事件/payload 等可验证证据）
- **N4**：**不加 CLI subcommand、不加 skill**——能力完全在 preset YAML + PROMPT.md 里，user 不需要学新命令
- **N5**：**不替代 preset lint / preflight**（那是启动前的硬门，本能力是跑完后复盘）

## 3. 用户故事

### 3.1 主路径

> 作为跑 `ralph run -c presets/en/debug.yml -p "Tests fail intermittently in CI"` 的用户，我在跑之前在 `PROMPT.md` 顶部写：
>
> ```
> ## DIAGNOSTICS MODE
> 诊断目标：.ralph/run-2026-06-13-debug-01/
> 对照 preset：debug
> ```
>
> 跑完后自动在 `docs/diagnoses/2026-06-13-debug-run-2026-06-13-debug-01.md` 看到一份人话报告，告诉我哪里偏、归因到机制还是 preset、短期/中期/长期该怎么改。

### 3.2 报告读者分层

| 读者 | 期望章节 |
|---|---|
| Manager（人话、5 分钟读完） | §1 结论摘要 + §2 链路对比图 + §6 三段式计划 |
| Preset 作者 | §4 问题归因表 P1/P2 + §6 中期 preset 改动建议 |
| Loop 机制维护者 | §3 证据清单 + §4 P0 + §6 长期机制增强建议 |

## 4. 功能需求

### 4.1 输入契约（写在 PROMPT.md）

每个 preset 的 `PROMPT.md`（或仓库根 `PROMPT.md`）顶部加：

```markdown
## DIAGNOSTICS MODE
诊断目标：<run-dir>          # 默认 .ralph/，可省略
对照 preset：<preset-name>     # 必填，对照哪个 preset 的预期事件流
报告路径：docs/diagnoses/     # 默认即可，可省略
启用：是                     # 默认是，可填"否"关闭
```

agent 读到这段就知道要调起 `diagnose-run` preset。

### 4.2 触发机制（写在 preset YAML）

被诊断 preset 的 hat `instructions` 末尾增加一段「诊断收尾 SOP」：

```yaml
diagnostics:
  on_completion: "diagnose-run"   # 调起 diagnose-run preset
  on_terminal_failure: "diagnose-run"
```

hat 在自己走到 terminal event 后，**显式 emit** 一个内部事件（如 `diagnosis.request`），触发 `diagnose-run` 的 investigator hat 接管（diagnose-run 自己订阅 `diagnosis.request`，跨 preset 用 isolated hat registry 联通）。

或者更简单：hat 在 `terminal_events` 触发后，**自己 spawn** `ralph run -H builtin:diagnose-run -p "诊断 <run-dir> 对照 <preset-name>"`，这是 OS 层 fork，不走 event bus，更解耦。

### 4.3 输出契约（report.md）

报告固定 6 段，写到 `docs/diagnoses/YYYY-MM-DD-<preset>-<run>.md`：

1. **结论摘要**（≤ 200 字、人话）——这次 run 成功 / 部分成功 / 失败，原因 + 归因维度一句话
2. **执行链路对比图**——mermaid graph，左列"预期"（来自 preset YAML）、右列"实际"（来自 events.jsonl），箭头标偏离
3. **证据清单**——文件路径 + 行号 / 事件 topic + payload / task id，每异常 ≥ 1 条证据
4. **问题归因表**——P0/P1/P2 三档，每行：级别 / 现象 / 归因维度 / 证据指针
5. **修复建议**——按归因维度分组，给具体动作（"改 `presets/en/debug.yml` L42-L48 publishes 列表"）
6. **三段式计划**
   - **短期（≤ 1 周）**：用户手动改哪个文件、跑哪条命令验证
   - **中期（≤ 1 月）**：哪个 preset 需要重构 / 哪个机制需要补 guard
   - **长期（≥ 1 季度）**：loop 基座的架构性增强

### 4.4 5 个诊断维度硬清单

| 维度 | 必查项 |
|---|---|
| **A. 事件循环 / state machine / hat 调度** | 事件按时序入 bus、required_events 全触发、hat 触发条件匹配、`terminal_events` 声明、isolated 游标公平轮转、state machine 不早进 terminal |
| **B. preset 校验 / event origin / schema** | preset YAML 通过 `preset_lint`（4-hat 强制 isolated、execution_mode 字段）、`EventOriginGuard` 拒绝事件、`default_publishes` 合规、payload schema 与事件流一致 |
| **C. tasks·progress·memories 一致性** | `tasks.jsonl` 状态与 `progress.md` 阶段对齐、`findings.md` 覆盖每个 `hypothesis.rejected`、`fix-log.md` 闭环到 `fix.applied`、`memories.md` 记录关键决策 |
| **D. diagnostics 观测数据** | `recovery.jsonl` 有无 `event.isolation.boundary_violation`、`drift.jsonl` 三指标（field completeness / coord join rate / emit cadence）跌破阈值、`diagnosis-summary.json` 已写 |
| **E. worktree / merge queue / lock** | `.ralph/loop.lock` PID 存活、`.ralph/loops.json` 的 `worktree_path` 合规、merge queue 有无未消化条目、stale lock 误读 |

### 4.5 自指约束

`diagnose-run` preset 自己必须做到：

- **必须 isolated**（4 个 hat，超过 3-hat coordinator 上限，触发 U6 强制校验）
- **必须 self-contained**：诊断 loop 不写被诊断 run 的 `.ralph/events.jsonl`，只读快照文件
- **必须 fail-safe**：诊断自身崩了不影响被诊断 run 的状态

## 5. 非功能需求

| 维度 | 指标 |
|---|---|
| 报告生成延迟 | 单次诊断 ≤ 5 分钟 |
| 报告可读性 | manager 5 分钟能读完结论摘要 + 三段式计划 |
| 证据可验证 | 每个 P0/P1 问题 ≥ 1 条 file:line 或 event:topic 指针 |
| 报告可复现 | 同一 `--preset` + 同 PROMPT.md 输入，报告内容 deterministic（除时间戳） |
| 诊断隔离 | 诊断 crash 不影响被诊断 run 的 loop lock / events.jsonl |
| 用户学习成本 | **零**（用户只填 PROMPT.md 顶部一段，不需要学新 CLI / 新 skill） |

## 6. 关键决策与权衡

### 6.1 为什么"新 preset + PROMPT.md 协议"而不是"CLI subcommand"？

- 用户贴 PROMPT.md + 自动跑，是 Ralph 已有约定（`prompt_file: "PROMPT.md"`）
- 用户不需要学新 CLI 子命令
- 诊断能力复用 loop 基座，不另起炉灶
- "preset YAML 是契约 + PROMPT.md 是用户意图"的边界清晰

### 6.2 为什么诊断触发放在"hat 末尾"而不是"loop 终止后 hook"？

- Loop 终止 hook（`hooks/completion`）当前是"外部命令"，不是 hat 触发链
- 让诊断走 isolated 4-hat 才能产出结构化报告
- 末位 hat 调起"下一个 run"是 OS 层 fork，与被诊断 run 完全解耦

### 6.3 为什么报告放 `docs/diagnoses/` 而不是 `.ralph/diagnoses/`？

- `.ralph/` 是运行时状态目录，不放"汇报文档"
- `docs/diagnoses/` 是仓库可见、可 commit、可分享给 manager 的位置
- 与 `docs/solutions/` 同层（解决方案文档也是给 manager 看的）

### 6.4 为什么不做自动修复？

- 机制修复需要发版，诊断报告是输入不是输出
- preset 修复需要作者判断，agent 改 preset 是越权
- "诊断 → 计划 → 人决策 → 改"是 Ralph 核心反压循环，不能被自动化短路

## 7. 依赖与前提

### 7.1 复用现有能力

- **preset 解析**：`crates/ralph-cli/src/presets.rs::load_preset`
- **事件解析**：`crates/ralph-core/src/event_loop/` 下的 EventReader / EventParser
- **task store**：`crates/ralph-core/src/task_store.rs`
- **diagnostics 输出**：`crates/ralph-core/src/diagnostics/` 现有 `recovery.jsonl` / `drift.jsonl` / `diagnosis-summary.json`

### 7.2 需要新增/修改

- **新增**：`presets/en/diagnose-run.yml`（4-hat isolated 诊断 preset）
- **修改**：`presets/manifest.yml` 的 `embedded:` 列表加一行
- **修改**：`crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组加 `EmbeddedPreset`
- **修改**：每个被支持的 preset（如 `debug.yml`）末尾加 `diagnostics:` 段 + `PROMPT.md` 加 `## DIAGNOSTICS MODE` 模板
- **同步**：`scripts/ralph-zsh-plugin.zsh` 的 zsh 补全加 `diagnose-run`（仅 builtin:* 列表）
- **同步**：`presets/index.json` + `CLAUDE.md`/`AGENTS.md` Presets 列表
- **同步**：`presets/COLLECTION.md`

### 7.3 同步规则

按 `CLAUDE.md` 的"builtin preset 改动后"清单同步改 4-5 处（en/yml、manifest、presets.rs、index.json、zsh 补全、CLAUDE.md/AGENTS.md Presets 列表）。

## 8. 验收标准

### 8.1 给 manager 看

- [ ] 跑 `presets/en/debug.yml` 一次（含 PROMPT.md 顶部 `## DIAGNOSTICS MODE` 段），5 分钟内拿到 `docs/diagnoses/<date>-debug-<run>.md`
- [ ] report 第 1 段结论摘要 ≤ 200 字、人话、无术语堆砌
- [ ] report 第 6 段三段式计划，每段 ≥ 1 个具体动作
- [ ] 报告贴给非工程 manager，对方能复述"这次 run 因为 X 失败了，Y 团队下季度该做 Z"

### 8.2 给 preset 作者看

- [ ] 4 个 P1/P2 问题各指向具体 preset YAML 行号
- [ ] 中期计划里 ≥ 1 条"重构 preset X 的事件流"建议
- [ ] 同 preset 重跑诊断，报告 §4 表格字段顺序、§6 章节顺序 byte-level 一致（除时间戳）

### 8.3 给 loop 机制维护者看

- [ ] P0 问题 ≥ 1 条指向 `crates/ralph-core/src/...` 具体文件
- [ ] 长期计划 ≥ 1 条机制级增强建议
- [ ] `diagnose-run` preset 自己能通过 `preset_lint::check_multi_hat_isolation`

### 8.4 给 dogfood 链看

- [ ] 用报告"短期修复"去改 preset，重跑同 run，第二次 report 的 P0/P1 数量下降
- [ ] 诊断 preset 自己 crash 不影响被诊断 run 的 `events.jsonl` / `loop.lock`

## 9. 风险与回滚

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| `diagnose-run` 自己 4-hat 没对齐触发条件，循环跑不完 | 中 | 中 | `completion_promise: "DIAGNOSE_COMPLETE"` + `required_events` 强制；`max_iterations: 8` 上限 |
| 报告"人话"被 LLM 写成技术腔 | 高 | 中 | `reporter` hat instructions 强制"第 1 段禁术语清单" + "三段式计划每段首句以动词开头" |
| 诊断读快照时被 run 实时写覆盖 | 低 | 高 | 诊断启动时 `cp -r <run-dir>/.ralph <diag-snapshot-dir>/` 一次性快照 |
| preset 改动后 4-5 处同步漏 | 中 | 中 | 跑 `just verify-presets` / `ralph preset check` 作为预提交 hook |
| manager 看不懂 mermaid | 低 | 低 | report §2 同时输出 mermaid 源码 + 文字版 bullet 列表 |

## 10. 范围之外（明确不做）

- 实时在线诊断
- 自动改 preset / 自动发版机制修复
- LLM-as-judge 主观打分
- 跨多次 run 的趋势分析
- 接管 preset lint / preflight 的硬门职责
- **新增 CLI subcommand**（不学新命令）
- **新增独立 skill**（能力放在 preset YAML 里）

## 11. 词汇表（暂存，待 CONCEPTS.md 校验）

- **诊断 run (diagnose run)**：用 `diagnose-run` preset 启动的一次独立 loop，目标分析另一次 run 的产物
- **运行链路 (run lineage)**：从 `loop.start` 到 `DEBUG_COMPLETE` / `LOOP_COMPLETE` 的完整事件序列
- **对账 (reconciliation)**：把实际产物与 preset 预期逐项对照
- **归因维度 (attribution dimension)**：preset / 机制 / agent / 多因素，4 选 1+ 的根因分类
- **三段式计划 (three-horizon plan)**：短期（≤ 1 周手动改）/ 中期（≤ 1 月 preset 重构）/ 长期（≥ 1 季度机制增强）
- **DIAGNOSTICS MODE**：`PROMPT.md` 顶部的约定段，用户写"诊断目标 + 对照 preset + 启用开关"