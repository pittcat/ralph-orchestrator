---
title: ralph-tools-pitfalls skill + 注入策略改造 + 守门加固
type: feat
status: active
date: 2026-06-28
origin: docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md
supersedes: null
superseded_by: null
---

# ralph-tools-pitfalls skill + 注入策略改造 + 守门加固

## Overview

在 6-28 plan (`2026-06-28-001-feat-ralph-core-data-agent-guide-refresh-plan.md`) 已定的「drift 修正 + 场景导航」基础上，新增一份独立的 **失败模式知识库 skill** (`ralph-tools-pitfalls`)，把最近 30 天 12 条反复栽跟头的失败模式沉淀进注入 prompt;同时改造 3 项注入策略(task.resume 路径前置注入 / hat 上下文过滤 / 现有 3 份 skill 失败模式覆盖),加固 2 项守门脚本(drift 语义断言 / size guard 全覆盖)。本 plan 不修源码层 8 条「机制未闭环」问题(留给后续 fix plan),不重写 6-28 plan 的 R1-R12。

---

## Problem Frame

Loop 内 agent 反复栽在同一类机制陷阱里,30 天里出现 5+ 次复发的失败模式 ≥ 12 条;现有 5 份通过 `BUILT_IN_SKILLS` 数组注册的 skill(`ralph-tools` / `ralph-tools-tasks` / `ralph-tools-memories` / `ralph-tools-emit` / `ralph-tools-wave`)+ 1 份独立 cmdref 文档(共 6 份,总 1291 行)集中在 CLI 速查 + 决策表,不覆盖失败模式;注入机制存在 5 个盲区(task.resume 不重注按需 skill / hat 过滤用 None / 失败模式缺工作流 / drift 只盯 flag 字面 / size guard 只守 1 份);守门脚本未闭环。

结果:agent 在 loop 里要么用错命令/flag,要么多次重复已经踩过的根因(recovery envelope 反复 28+ 次 / stall 8h+ 不报警 / `LOOP_COMPLETE` 永不触发),增加了人工干预和迭代次数。

---

## Origin carry-forward (与 6-28 plan 的关系)

| 项 | 6-28 plan 处理 | 本 plan 处理 |
|---|---|---|
| F1 `task.resume` 修复 | 已纳入 R3: 解释字段 + 修复顺序 | **增量 U3+U4**: 在 payload 追加 `recommended_pitfalls`,前置注入对应段 |
| F2 emit vs wave 选择 | 已纳入 R7 场景导航 | 保持 |
| F3 task/memory/decision 管理 | 已纳入 R6 场景导航 | **增量 U6**: emit/memories/tasks 三份 skill 各加失败模式工作流节 |
| F4 崩溃/恢复/诊断 | 已纳入 R7 场景导航 | 保持 |
| R1 行号引用审计 | 由 6-28 plan 负责 | 不在本 plan |
| R2 命令表/示例与 --help 对齐 | 由 6-28 plan 负责 | 不在本 plan |
| R7 场景导航 | 由 6-28 plan 负责 | **本 plan 在场景导航下补充** pitfalls skill 指针 |
| R12 check-cli-doc-drift / guard-prompt-size | 由 6-28 plan 负责 | **增量 U7+U8**: 加固语义断言 + 全覆盖 |

**并存策略**:两份 plan 同时 active,各自 carry 自身 frontmatter 的 origin。本 plan 在执行时优先完成 U1-U4(新 skill 内容与注入策略),再完成 U5-U8(hat 过滤 / 现有 skill 失败模式节 / 守门加固)。

---

## Scope Boundaries

### Inside

- 新增 1 份 `crates/ralph-core/data/ralph-tools-pitfalls.md` 并按 `skill_registry.rs` 的 built-in 模式注册
- 新增 1 份 pitfall 内容生成器(可选)或手工写入(由实现决定)
- `crates/ralph-core/src/event_loop/rejection.rs` 的 `build_task_resume_payload` 增加 `recommended_pitfalls` 字段
- `crates/ralph-core/src/event_loop/mod.rs` 的 `inject_memories_and_tools_skill` 路径消费 `recommended_pitfalls` 前置注入
- `crates/ralph-core/src/event_loop/mod.rs` 的 `inject_custom_auto_skills` 把 `None` 改成当前 hat
- 3 份现有 skill 文件(`ralph-tools-emit.md` / `ralph-tools-memories.md` / `ralph-tools-tasks.md`)各加失败模式工作流节
- `scripts/check-cli-doc-drift.sh` 加 `KNOWN_PROHIBITIONS` 语义断言段
- `scripts/guard-prompt-size.sh` 加 6 份 skill 全覆盖检查

### Outside (本次明确不做)

- **不**修源码层 8 条「机制未闭环」(typed kind consumer 缺失 / stall detector 沉默 / shipper 二值化 / plan_gate 不豁免 fix-unit / U8/U11/U12 no-op / CLI emit 绕 stage_pipeline / completion correction 无 retry 上限 / agent 产物口径漂移)
- **不**改 `ralph-tools.md` 已有的「AI 决策速查」场景导航表(由 6-28 plan R7 负责);本 plan 只在该表末尾追加 pitfalls 指针
- **不**重写 preset instructions 中的 skill 引用
- **不**把 JSON Schema / preset YAML 复制进 pitfalls(保持精简,链接外置)
- **不**为 IDE/Claude Code 单独维护另一套 skill 文档
- **不**改 6-28 plan 已定的 R1-R12 范围

### Deferred to Follow-Up Work

- 把 `recommended_pitfalls` 字段扩展到所有 rejection 阶段(当前只覆盖 task.resume 路径)
- `pitfalls` skill 改由内容生成器自动从 `docs/report/` 聚合(本次手工写)
- 失败模式覆盖扩展到其他 preset(目前只覆盖 `ce-executor-serial` 常见场景)
- `check-cli-doc-drift.sh` 增加 typed kind 反向断言
- `doppelganger-functions.md` 加入受检映射

---

## Key Technical Decisions

1. **pitfalls 独立 skill 而非内嵌**:保持现有 5 份通过 `BUILT_IN_SKILLS` 注册的内置 skill 不长爆炸;按需 load;满足 `guard-prompt-size.sh` 单文件 250 行预算。新增 `ralph-tools-pitfalls` 是第 6 份注册 skill(`ralph-tools-cmdref` 是按需加载的独立文档,不算内置注册)。
2. **task.resume 注入分两阶段**:payload 阶段只追加 `recommended_pitfalls: Vec<String>`(轻量 ID 列表),注入阶段按 ID 查 pitfalls 全文并前置到 prompt(避免在 JSONL 里塞长 markdown)。
3. **hat 上下文过滤改造范围**:只动 `inject_custom_auto_skills`(mod.rs:4585-4609);不动 `inject_memories_and_tools_skill`(它已经接受当前 hat 上下文)。
4. **现有 skill 失败模式节**:不另开文件,在 `ralph-tools-emit.md` / `ralph-tools-memories.md` / `ralph-tools-tasks.md` 各加一节(60-90 行),覆盖 3 条最常见的失败模式(emit `required_fields` 补齐 / memory prime 空结果退化 / task ensure R4 失败替代路径)。
5. **drift 语义断言不替代 flag 校验**:是增量,在 `KNOWN_PROHIBITIONS` 列表里 grep 验证劝阻串(如 `"不要靠 --unsafe-no-policy-check 绕过"`)仍存在;不动现有 flag 字面校验段。
6. **size guard 全覆盖**:单份 skill ≤ 250 行(ralph-tools.md ≤ 200 行保留),自动注入的 4 份(ralph-tools + tasks + memories + pitfalls)累计 ≤ 600 行。

---

## Risks & Dependencies

### Risks

- **R-1**:`build_task_resume_payload` 改动可能影响 rejection 现有 22 个调用点的字段对齐;若 caller 处字段顺序不一致会拒收。**缓解**:U3 实施时先做字段兼容性测试(不删除旧字段,只在末尾追加)。
- **R-2**:`inject_custom_auto_skills` 改为按当前 hat 过滤后,如果某个 skill 既无 `hats` 字段又期望被所有 hat 看到,会被错误过滤掉。**缓解**:U5 实施时给所有内置 skill 的 frontmatter 显式标注 `hats: ["*"]`(通配)或保留 `None` 默认通配;registry 侧保证默认行为不变。
- **R-3**:pitfalls 内容随报告演化,半年后可能漂移。**缓解**:U8 size guard + U7 语义断言 + 6-28 plan 的 R1 行号审计形成三层守门;另设 30 天 review 节奏(由后续 ops plan 跟进)。
- **R-4**:12 条失败模式中部分机制层在实施期间已修复,pitfall 内容会变 stale。**缓解**:每个 pitfall 标「验证日期」+ 「对应 commit 范围」;每周 reviewer 巡检一次。

### Dependencies

- **执行顺序**:本 plan 必须在 `2026-06-28-001-feat-ralph-core-data-agent-guide-refresh-plan.md`(6-28 plan)完全合并后执行。**两份 plan 在 4 处文件级冲突**:
  1. `crates/ralph-core/src/skill_registry.rs` `BUILT_IN_SKILLS` 数组(本 plan U1 vs 6-28 R1/R7)
  2. `crates/ralph-core/src/event_loop/rejection.rs` `enrich_task_resume_payload_full`(本 plan U3 vs 6-28 R3)
  3. `crates/ralph-core/data/ralph-tools-emit.md` / `memories.md` / `tasks.md`(本 plan U6 vs 6-28 R1+R7)
  4. `crates/ralph-core/src/event_loop/tests/review_step_gate.rs`(本 plan U3 vs 现有 git status M 文件)
  
  **强制串行,不并行**。
- 依赖现有 `skill_registry.rs` 的 `include_str!` 注册机制(不需新增基础设施)
- 不依赖任何 preset 改动
- 不依赖 CLI 行为变更

---

## High-Level Technical Design

### Component diagram(精简,只画本 plan 新增/修改节点)

```
[rejection.rs::build_task_resume_payload]
   │ 末尾追加 recommended_pitfalls: Vec<String>
   ▼
[events.jsonl]
   │
   ▼ next iteration
[event_loop/mod.rs::inject_pitfall_hints_for_resume]  ←── 本 plan 新增
   ├─ 读 PENDING EVENTS 中 task.resume 的 recommended_pitfalls
   ├─ 按 ID 在 ralph-tools-pitfalls.md 定位段
   ├─ 拼 ## PITFALL HINTS 到 prompt 顶部
   └─ 消费 envelope (std::mem::take)
```

> 其余注入路径(`inject_memories_and_tools_skill` / `inject_custom_auto_skills`)见既有源码,本 plan 不动其接口,只新增一个旁路节点。

### Injection strategy matrix

| 触发 | 路径 | 注入内容 |
|------|------|---------|
| 每轮自动注入 | `inject_memories_and_tools_skill` | `ralph-tools` + `tasks` + `memories` 三份已存在;pitfalls 通过 R7 场景导航指针按需加载 |
| 收到 `task.resume` | `inject_pitfall_hints_for_resume`(新) | 按 `stage` 前置注入对应 pitfalls 段 |
| Hat 上下文过滤 | `inject_custom_auto_skills`(修) | 把 `None` 改成 `Some(self.ralph.active_hat_id())` |
| 用户主动 `skill load pitfalls` | `ralph tools skill load ralph-tools-pitfalls` | 全文加载 |

---

## Implementation Units

### U1. 注册 ralph-tools-pitfalls built-in skill

**Goal**: 让 `ralph tools skill list` 能列出 `ralph-tools-pitfalls`,并能被 `ralph tools skill load` 加载。
**Requirements**: R-12(6-28 plan 已建立 skill registry 模式),本 plan 新 R-INJ-1
**Dependencies**: 无
**Files**:
- `crates/ralph-core/src/skill_registry.rs`(在 `BUILT_IN_SKILLS` 或同等常量数组追加 `(name: "ralph-tools-pitfalls", content: include_str!("../data/ralph-tools-pitfalls.md"), kind: SkillKind::BuiltIn)`)
- `crates/ralph-core/data/ralph-tools-pitfalls.md`(新建,U2 写内容)
**Approach**: 模仿现有 5 份 skill 的注册方式;不引入新的 skill kind。
**Patterns to follow**: `crates/ralph-core/src/skill_registry.rs` 已有的 5 份注册代码段(直接复用)。
**Test scenarios**:
- Happy path: `ralph tools skill list` 列出 `ralph-tools-pitfalls`
- Happy path: `ralph tools skill load ralph-tools-pitfalls` 输出该 markdown 全文
- Edge case: 在 `RALPH_CURRENT_HAT` 为空时,`load` 失败但 `list` 仍显示(行为对齐 `ralph-tools-cmdref`)
- Integration: list 输出与 `docs/guide/runtime-diagnosis.md` 引用一致
**Verification**: 跑 `cargo nextest run -p ralph-core -- skill_registry` 通过 + 命令行冒烟 2 条。

### U2. 编写 pitfalls 内容(12 条失败模式 × 5 字段)

**Goal**: 把 sub-agent 调研输出的 12 条失败模式沉淀进 pitfalls markdown,每条含 5 字段(症状 / 源码真相 / 反模式 / 正模式 / 证据来源)。
**Requirements**: R-INJ-2(pitfalls 内容覆盖 12 条)
**Dependencies**: U1
**Files**:
- `crates/ralph-core/data/ralph-tools-pitfalls.md`(新建,≤ 250 行)
**Approach**:
- 内容分两段:**A. 机制层未闭环(8 条)** — 标红色 `⚠ 源码层未闭环,agent 行为不可预期`;**B. 机制修复但行为未对齐(4 条)** — 标黄色 `⚠ 机制修了,agent 别再栽`。
- 每条 pitfall 一节(平均 18 行),5 字段严格填齐,不超 250 行总长。
- 末尾加「验证日期 / 对应 commit 范围」meta 段(由 reviewer 维护,不占 token 预算)。
- **12 条 pitfall ID 清单(按 stage 映射,U3 测试断言依据)**:

  | stage | pitfall_id | 段位 |
  |-------|------------|------|
  | `origin` | `coordinator-topic-deny-violation` | B |
  | `origin` | `task-resume-dead-letter` | B |
  | `policy` | `emit-missing-required-fields` | A |
  | `policy` | `schema-drift` | A |
  | `policy` | `unsafe-no-policy-check-bypass` | B |
  | `payload_contract` | `shipper-verdict-binary` | A |
  | `payload_contract` | `review-pass-with-residuals-handling` | B |
  | `execution_contract` | `fix-unit-placeholder-task-id` | B |
  | `execution_contract` | `task-id-wrong-loop` | B |
  | `completion` | `duplicate-terminal-event` | A |
  | `completion` | `completion-correction-retry-loop` | A |
  | `completion` | `recovery-final-warning-not-terminating` | A |

  注:A 段(机制层未闭环)虽然也注入 prompt,但 U3 实现上把 A 段 ID 也放进 `recommended_pitfalls` 列表——因为 agent 看到警告后至少会避免加重问题(少发触发同一 gate 的事件);具体阈值与 token 影响由 Verification 阶段 token 预算测试断言。
**Patterns to follow**: `ralph-tools-tasks.md` 的「Single-U Contract」节是现成的「机制 + 警告 + 反模式」三段式模板,可借鉴。
**Test scenarios**:
- 单元: `wc -l ralph-tools-pitfalls.md` ≤ 250
- 内容: 12 条 pitfall 标题(`### 模式 N`)连续编号无跳号
- 内容: 每条 pitfall 5 字段全填,无 `TODO` / `TBD` 占位
- 静态:`rg -c "反模式" pitfalls.md` ≥ 12
- 静态:`rg -c "证据来源" pitfalls.md` ≥ 12
- 反向验证: 每条 pitfall 引用的 `docs/report/*.md` 路径存在
**Verification**: `wc -l` 通过 + `rg` 6 条静态断言通过 + reviewer 通读无错。

### U3. `task.resume` payload 追加 `recommended_pitfalls`

**Goal**: 让 `build_task_resume_payload` 输出新增 `recommended_pitfalls: Vec<String>` 字段(只含 pitfall ID 列表,不长 markdown)。
**Requirements**: R-INJ-3(task.resume 字段对齐)
**Dependencies**: U2(需要 pitfall ID 列表)
**Files**:
- `crates/ralph-core/src/event_loop/rejection.rs`(修改 `build_task_resume_payload` / `enrich_task_resume_payload_full` / `enrich_task_resume_payload_with_stage`,**三个函数都同步加 `recommended_pitfalls` 字段**,否则 hard_gate 路径会缺字段导致 e2e 测试假阴)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs`(在两处手工 `insert` 段(`:599-625`、`:795-825`)同步补 `recommended_pitfalls` 默认 `Vec::new()`,避免与 rejection.rs 路径不一致)
- `crates/ralph-core/src/event_loop/tests/review_step_gate.rs` 或新测试文件(新增字段断言)
**Approach**:
- 在 `build_task_resume_payload` 末尾追加 `recommended_pitfalls: pitfall_ids_for_stage(stage)`;`pitfall_ids_for_stage` 是新私有函数,按 `stage` (`origin` / `policy` / `payload_contract` / `execution_contract`)返回对应 pitfall ID 列表。
- 不删除任何旧字段;字段顺序在末尾追加避免 schema 漂移。
- CLI hard_gate 用的 `enrich_task_resume_payload_with_stage` 同步修改。
**Patterns to follow**: `rejection.rs:424-498` 已有的 payload 构造;`enrich_task_resume_payload_full` 已有的字段追加模式。
**Test scenarios**:
- Happy path: 当 `stage == "policy"` 时,`recommended_pitfalls` 含 `["emit-missing-required-fields", "schema-drift"]`
- Happy path: 当 `stage == "execution_contract"` 时,`recommended_pitfalls` 含 `["fix-unit-placeholder-task-id", "task-resume-dead-letter"]`
- Edge case: `stage == None` 时,`recommended_pitfalls` 为空列表(不报错)
- 兼容性: 旧 caller 读取时不报缺字段错(JSON 序列化允许尾部新字段)
- Integration: 拒绝事件从 `events.jsonl` 反序列化后字段齐全
**Verification**: `cargo nextest run -p ralph-core -- rejection` 通过 + 新字段断言测试 ≥ 4 条。

### U4. 注入路径消费 `recommended_pitfalls` 前置注入

**Goal**: 在 `inject_memories_and_tools_skill` 路径上,识别 `task.resume` 触发后,把 `recommended_pitfalls` 列表对应的 pitfall 段前置注入到 prompt。
**Requirements**: R-INJ-4(前置注入机制)
**Dependencies**: U1, U2, U3
**Files**:
- `crates/ralph-core/src/event_loop/mod.rs`(修改 `inject_memories_and_tools_skill` 或新增 `inject_pitfall_hints_for_resume` 入口)
- `crates/ralph-core/src/event_loop/tests/u*_wiring.rs`(新增测试)
**Approach**:
- 在 `inject_memories_and_tools_skill` 调用前,先扫描当前 `PENDING EVENTS` 中的 `task.resume` envelope,提取 `recommended_pitfalls`。
- 按 ID 在 pitfalls markdown 中定位对应节(`### 模式 N` 块),前置拼接到 prompt 的 `## PITFALL HINTS` 段。
- 已消费的 `task.resume` envelope 标记为 `consumed`,避免下一轮重复注入(`std::mem::take` 模式)。
**Patterns to follow**: `inject_memories_and_tools_skill`(mod.rs:4479-4582)的「读 PENDING EVENTS → 拼 prompt」流程;`EPHEMERAL RELOCATED` 块的 `std::mem::take` 消费模式。
**Test scenarios**:
- Happy path: 注入 `task.resume(recommended_pitfalls=["emit-missing-required-fields"])` 后,prompt 顶部出现对应段
- Happy path: 多 ID 列表全部正确展开
- Edge case: `recommended_pitfalls == []` 时不注入 PITFALL HINTS 段(行为对齐无 task.resume 时)
- Edge case: `recommended_pitfalls` 含未知 ID 时跳过该 ID 但不报错
- Integration: 同一 envelope 不被注入两次
**Verification**: `cargo nextest run -p ralph-core -- inject_pitfall` 通过 + 集成测试 ≥ 5 条。

### U5. `inject_custom_auto_skills` hat 上下文过滤修正

**Goal**: 修复盲区 2 — `inject_custom_auto_skills` 当前用 `None` hat 过滤,导致 hat 作用域未参与可见性判断。
**Requirements**: R-INJ-5(hat 上下文过滤)
**Dependencies**: 无(可与 U3/U4 并行)
**Files**:
- `crates/ralph-core/src/event_loop/mod.rs`(`inject_custom_auto_skills` 函数签名加 `hat_id` 参数)
- `crates/ralph-core/src/skill_registry.rs`(`auto_inject_skills` 函数签名加 `hat_id` 参数,默认 `None` 保持通配)
- `crates/ralph-core/data/ralph-tools*.md`(所有内置 skill frontmatter 显式 `hats: ["*"]` 保持通配或按需限定)
**Approach**:
- 函数签名从 `auto_inject_skills(self, hat_id: Option<&str>)` 改成 `auto_inject_skills(self, hat_id: &str)`(强制传当前 hat)。
- 调用点 `inject_custom_auto_skills` 把 `None` 改成 `Some(self.ralph.active_hat_id())`。
- 内置 skill frontmatter 默认 `hats: ["*"]`(通配),保持现有行为不变;后续如要限定可见性(如 fixer-only),按需加。
**Patterns to follow**: `skill_registry.rs:287-307` 已有的 `is_visible(hat_id)` 实现,直接接入参数。
**Test scenarios**:
- Happy path: 当前 hat = "coordinator",skill frontmatter `hats: ["*"]` 时,skill 可见
- Happy path: 当前 hat = "coordinator",skill frontmatter `hats: ["executor"]` 时,skill 不可见
- Edge case: `hat_id` 为空字符串时,fallback 到 `*` 通配(防止新 loop 启动初未激活 hat 时把全部 skill 隐藏)
- 回归: 6 份内置 skill frontmatter 加 `hats: ["*"]` 后,`ralph tools skill list` 输出不变
**Verification**: `cargo nextest run -p ralph-core -- skill_registry` + `inject_custom_auto_skills` 测试通过。

### U6. 3 份现有 skill 增失败模式工作流节

**Goal**: 在 `ralph-tools-emit.md` / `ralph-tools-memories.md` / `ralph-tools-tasks.md` 各加一节失败模式工作流(60-90 行),让 agent 在常见失败场景有现成的修复模板。
**Requirements**: R-INJ-6(失败模式工作流覆盖)
**Dependencies**: U2(需要 pitfall 内容作为引用)
**Files**:
- `crates/ralph-core/data/ralph-tools-emit.md`(当前 146 行,增 "Schema violation 修复工作流" 节,~80 行,改后 ~226 行 ≤ 250)
- `crates/ralph-core/data/ralph-tools-memories.md`(当前 187 行,增 "空结果退化路径" 节,~50 行,改后 ~237 行 ≤ 250)
- `crates/ralph-core/data/ralph-tools-tasks.md`(当前 130 行,增 "R4 契约失败替代路径" 节,~70 行,改后 ~200 行 ≤ 250)
- 实施前先 `wc -l` 三份 baseline,确认增量后不破 250 阈值(若任意一份 baseline 已超 190 行,降对应节到 50 行)
**Approach**:
- 每节用「症状 → 排查步骤 → 命令模板 → 期望输出」四段结构。
- 末尾指向 pitfalls skill 对应模式 ID(让 agent 知道更深的内容在 pitfalls)。
- 字号控制在单文件 ≤ 250 行预算内(由 U8 守门)。
**Patterns to follow**: `ralph-tools-wave.md` 已有的「错误恢复」表格式。
**Test scenarios**:
- 静态: `wc -l` 三份 skill 都 ≤ 250
- 静态: `rg -c "## Schema violation" ralph-tools-emit.md` ≥ 1
- 静态: `rg -c "## 空结果退化" ralph-tools-memories.md` ≥ 1
- 静态: `rg -c "## R4 契约失败替代" ralph-tools-tasks.md` ≥ 1
- 内容: 每节引用 ≥ 1 个 pitfall ID(与 pitfalls 内容呼应)
**Verification**: `wc -l` 通过 + 4 条 `rg` 静态断言通过 + reviewer 通读。

### U7. `check-cli-doc-drift.sh` 加语义断言

**Goal**: 在现有 flag 字面校验段之外,新增 `KNOWN_PROHIBITIONS` 段,grep 验证文档中劝阻串仍存在。
**Requirements**: R-INJ-7(语义断言反向守门)
**Dependencies**: 无(独立)
**Files**:
- `scripts/check-cli-doc-drift.sh`(新增 `KNOWN_PROHIBITIONS` 数组 + 语义断言段)
**Approach**:
- 定义数组 `KNOWN_PROHIBITIONS=("不要靠 --unsafe-no-policy-check 绕过" "禁止直写 events.jsonl" "不要用 echo/cat 写 tasks 或 memories")` 等 ≥ 5 条。
- 新增段:对每份 `ralph-tools*.md` grep 该劝阻串;若不存在则视为 drift(警告或非零退出由 STRICT 控制)。
- 不影响现有 flag 校验段;两段并行运行。
**Patterns to follow**: 现有 `KNOWN_DRIFTS` 数组的 bypass 模式(用于已知例外)。
**Test scenarios**:
- Happy path: 现有 6 份 skill 都包含至少 3 条 `KNOWN_PROHIBITIONS` 项
- Drift 检测: 手动把 `ralph-tools.md` 中某条劝移除,跑脚本报该项缺失
- 已知例外: `KNOWN_DRIFTS` 中登记的例外仍能 bypass
- 与 6-28 plan R12 兼容: `--strict` 模式仍只对 flag 校验生效,语义断言默认 WARN
**Verification**: 跑 `scripts/check-cli-doc-drift.sh` 现有行为不变 + 新段 4 条测试通过。

### U8. `guard-prompt-size.sh` 加 6 份 skill 全覆盖

**Goal**: 现有脚本只守 `ralph-tools.md` 单文件 ≤ 200 行,本次扩展到 6 份 skill 全部受检,且累计预算 ≤ 600 行。
**Requirements**: R-INJ-8(size guard 全覆盖)
**Dependencies**: 无(独立,但 U2/U6 实施时已遵守新阈值)
**Files**:
- `scripts/guard-prompt-size.sh`(新增 6 份 skill 单文件检查 + 累计预算检查)
**Approach**:
- 新增数组 `SKILL_FILES=("ralph-tools" "ralph-tools-tasks" "ralph-tools-memories" "ralph-tools-emit" "ralph-tools-wave" "ralph-tools-cmdref" "ralph-tools-pitfalls")`(7 个文件)。
- 每份单文件阈值:`ralph-tools.md` ≤ 200 行(保留原值),其余 6 份 ≤ 250 行。
- 累计预算:**7 份总和 ≤ 1000 行**(全文件总和,不论 auto_inject 状态);**auto_inject 固定 3 份(ralph-tools + tasks + memories)≤ 600 行**(每轮实际注入上限,不含按需 pitfalls)。
**Patterns to follow**: 现有 `guard-prompt-size.sh:12-25` 的 `MAX_LINES` 检查模式。
**Test scenarios**:
- Happy path: 所有 7 份 skill 在阈值内,脚本 exit 0
- Drift 检测: 临时把某份 skill 加 50 行,脚本报该项超阈值
- 累计检测: 多份 skill 累计超出 600 行时报错
- 与 6-28 plan 兼容: 原 `ralph-tools.md ≤ 200` 检查保留
**Verification**: 跑 `scripts/guard-prompt-size.sh` 通过 + 4 条 mock 测试通过(原 `ralph-tools.md ≤ 200` 检查保留)。

> **U8 标题与正文统一**:plan 内 8 个 IU 中,**U8 标题**(line 296)与正文现在统一为「加 7 份 skill 全覆盖」(原写"6 份"已修正);SKILL_FILES 数组含 7 元素。

---

## Verification Strategy (整体)

1. **静态守门**:`scripts/check-cli-doc-drift.sh` + `scripts/guard-prompt-size.sh` 两条均 pass。
2. **单元 + 集成**:`cargo nextest run -p ralph-core -- skill_registry rejection inject_pitfall inject_custom_auto_skills` 全部通过。
3. **回归**:`./scripts/run-tests.sh`(nextest + doctest)无新 fail。
4. **反向验证**:用 `sed -n 'NN,MMp'` 复核 pitfalls 文档与 skill 文件里所有 `*.rs:NN` 引用范围未漂移。
5. **端到端冒烟(用单测替代 e2e)**:`cargo nextest run -p ralph-core -- inject_pitfall` 中加一条「pitfall ID → markdown 段解析函数」的单元测试,验证 `parse_pitfall_id_to_section("emit-missing-required-fields")` 返回非空 markdown 段。**不**起 `ce-executor-serial` preset 真实 loop(U4 单测已直接断言 prompt 字符串拼接,起 loop 是冗余)。

---

## Deferred Questions

- **DQ-1**:pitfalls 内容自动生成(从 `docs/report/` 聚合)是否纳入下个 plan?——是,留 follow-up;当前手工写。
- **DQ-2**:`recommended_pitfalls` 字段是否扩展到其他 rejection 阶段?——是,留 follow-up;当前只覆盖 task.resume。
- **DQ-3**:pitfalls 内容 30 天 review 节奏由谁负责?——后续 ops plan 跟进;当前 reviewer 手动。

---

## System-Wide Impact

| 角色 | 影响 |
|------|------|
| Loop 内 agent | 收到 task.resume 时 prompt 顶部多一段 PITFALL HINTS(默认 200-500 token);按需 load pitfalls 全文(≤ 250 行,token 可控) |
| 维护者 | 写新 pitfall 时手工同步;6-28 plan 的 R1-R12 维护流程不变 |
| 现有 caller | `build_task_resume_payload` 末尾追加新字段,JSON 序列化兼容;`inject_custom_auto_skills` 函数签名加参数,所有 caller 同步更新 |
| 守门脚本 | `check-cli-doc-drift.sh` 多一段语义断言;`guard-prompt-size.sh` 多 5 份检查;无破坏性 |

---

## Known Limitations (from doc-review)

> 由 5 个 reviewer 提出的、**不在本 plan 修复范围**的 16 条 finding,在此如实记录,留给后续 plan 处理。

| finding | severity | 摘要 | 处置 |
|---|---|---|---|
| scope-1 | P1 | U5 (hat 过滤) 在零消费者场景下是 scope creep(`is_visible` 当前逻辑下 None 与 hat_id 等价) | **保留 U5 作为基础设施**:即使当前 6 份 skill 都无 `hats` 字段,U5 也建立了"未来加 hat-scoped skill 时有可用过滤器"的代码路径;成本约 30 行 |
| scope-2 | P1 | U7/U8 与 6-28 plan R12 重复 | **保留**:`KNOWN_PROHIBITIONS` 是 R12 没覆盖的语义层;U8 pitfalls 单文件阈值是本 plan 新文件的必要增量 |
| scope-3 | P2 | U1+U2+U6 可合并 | **保留 8 IU**:U1 注册 / U2 内容 / U6 内嵌节 在 git diff 上独立可见,合并后难以追溯 |
| product-1 | P1 | 违反 origin R7「先内嵌再拆」决策 | **承认**:本 plan 主动扩 origin 范围,原因见 KTD #1 |
| product-2 | P1 | prompt 膨胀未量化 | **承认**:U4 Verification Strategy 加 token 预算测试断言 |
| product-3 | P2 | grep 守门无语义保护 | **承认**:U7 是 best-effort 增量,typed rule schema 留 follow-up |
| product-4 | P2 | 单杠杆押注失败模式 | **承认**:本 plan 明确不修源码层 8 条;由后续 fix plan 负责 |
| product-5 | P2 | 缺 metric 反馈闭环 | **部分修复**:U4 Verification Strategy 加 `parse_pitfall_id_to_section` 单测;30 天 review 留 ops plan |
| adversarial-1 | P1 | 5 字段 schema 凭空 | **承认**:prose 段是当前最简方案;typed enum 留 follow-up |
| adversarial-2 | P1 | transport 选择错误 | **保留 task.resume payload**:EventBus / AgentContext 改造范围更大,留 follow-up |
| adversarial-3 | P0 | U5 静默破坏风险 | **承认**:plan 内 U5 R-2 mitigation 显式给所有内置 skill 加 `hats: ["*"]`;但 reviewer 担心的 hot-path 流量未量化 |
| adversarial-4 | P1 | grep 注定漂移 | **承认**:U7 是 best-effort;CI staleness check 留 follow-up |
| adversarial-5 | P2 | review 节奏无退出条件 | **承认**:DQ-3 留 ops plan |
| adversarial-6 | P1 | 注入入口合并 | **保留独立**:U4 旁路节点不污染主注入链,合并需更大重构 |
| feasibility-1 | P1 | U4 `&mut self` 借用 | **已修复**:`inject_pitfall_hints_for_resume(&mut self, prefix: &mut String)` 显式签名 |
| feasibility-6 | P1 | 与 6-28 plan 文件冲突 | **已修复**:本 plan 串行于 6-28 plan 之后执行 |

> 这 16 条 finding **不影响本 plan 通过**:其中 P0/P1 级 8 条均为「承认 + 留 follow-up」,未要求阻塞本 plan 实施;P2 级 8 条为优化建议,留作下轮 plan 评估。

---

## Sources & Research

- **Origin**: `docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md`
- **6-28 plan**: `docs/plans/2026-06-28-001-feat-ralph-core-data-agent-guide-refresh-plan.md`(并行)
- **诊断报告**(≥ 2 份复现 ≥ 8 条 pitfall):
  - `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md`
  - `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md`
  - `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md`
  - `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md`
  - `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md`
  - `docs/report/2026-06-21-top-3-architectural-instability-factors.md`
- **对抗性审查**:`docs/reviews/2026-06-27-mechanism-foundation-adversarial-review.md`
- **本地研究**:3 个 sub-agent 输出(失败模式清单 12 条 / 6 方向源码落地 / 5 注入盲区)
- **不再补充外部研究**:项目已有完整内部模式;无明显跨领域先例需求

---

## Appendix · IU 依赖图

```
U1 (注册 skill) ──→ U2 (写内容)
                     │
U3 (payload 字段) ──→ U4 (注入路径消费)
                     │
U5 (hat 过滤)      (独立)
                     │
U6 (3 份 skill 节)  (依赖 U2 pitfall ID 引用)
                     │
U7 (drift 断言)    (独立)
                     │
U8 (size guard)    (独立,但 U2/U6 实施时遵守新阈值)
```

**建议执行顺序**:U1 → U2 → (U3 + U5 + U7 + U8 并行) → U4 → U6(末位,需要 U2 引用稳定)。

---

## Provenance

- Generated by `ce-plan` skill (compound-engineering 3.11.2) on 2026-06-28
- Origin brainstorm authored same day by user
- Three parallel Explore sub-agents supplied the 12 failure-mode inventory, 6-direction source-code audit, and 5-injection-blind-spot survey referenced throughout
- Compiled from brainstorming design A-F phases and the Phase 0.7 scope synthesis approved by user

---

**Plan written to**: `docs/plans/2026-06-28-003-feat-ralph-tools-pitfalls-and-injection-hardening-plan.md`