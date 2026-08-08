---
title: "引入 Loro Collab Findings 板（opt-in 功能拓展，零回归）"
date: 2026-07-23
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
plan_format: spec-bdd-atdd-outside-in-tdd-serial-v1
origin:
  - 会话定稿：Loro=协作内容层；CollabStore+CLI+opt-in 六维 findings；不改 wave/supervisor/TaskStore
regression_policy: default-off-zero-behavior-change
---

# 引入 Loro Collab Findings 板（opt-in 功能拓展，零回归）

> **给 Coding Agent**：本文件是可执行开发计划。禁止在本计划阶段编写生产代码。  
> **执行纪律**：严格 `Unit 1 → Unit 2 → …` 串行；每 Unit 必须 Red→Green→Refactor→回归后才进入下一 Unit。  
> **零回归硬约束**：`event_loop.collab.enabled` 默认 `false`；未开启时不得创建 Collab 文件、不得改变既有 emit/wave/task/review 行为；不得改 `review.*.done` 的 `required_fields` 集合（v1 用导出投影满足现有门禁）。

---

## 1. 功能目标

### 业务目标

- 为多 Agent / 人机协作提供 **可合并的 findings 内容层**（基于 Loro），与 wave 调度、task 门禁解耦。
- Agent 通过 `ralph tools collab …` 读写 findings，禁止手搓 CRDT 文件。
- 在 **opt-in** 下，六维 review 把 findings 写入 Collab，并 **导出** 兼容现有 `findings_file` 的 markdown 投影，使 event policy / synthesizer 门禁不破。
- 默认关闭时，产品行为与今天 **完全一致**（功能拓展，零回归）。

### 本次范围

- `ralph-core`：Collab 持久化、findings 文档形状、upsert/list/export 投影 API、`event_loop.collab` 配置。
- `ralph-cli`：`ralph tools collab findings upsert|list|export`。
- 注入 skill：`crates/ralph-core/data/ralph-tools-collab.md` + `ralph-tools.md` 入口（通用规则，无计划号）。
- **一个** opt-in 消费者：六维 review findings（dim hat 写入 + synthesizer 可读汇总）；通过配置开关启用。
- 测试：nextest；CLI 集成测必须 scrub hat env（HARD RULE 5）。

### 非目标

- 不修改 wave dispatcher / supervisor fan-in / `WaveTracker` / `TaskStore` / 终态事件权威。
- 不删除、不改名现有 `findings_file` / `findings_count` required fields。
- 不做 frontier 增量 prompt 注入（follow-up）。
- 不迁 memories、不做实时协同 UI、不用 Loro 替代 Git/worktree。
- 不默认开启任何 builtin preset 的 collab（v1 仅提供能力 + 文档化如何开启；若接线 preset，必须仍默认 false）。

### 已知约束和假设

- **现状**：六维已是「瘦事件 + 每维 `findings_file`」；本功能不是修「同文件互盖」，而是补统一可合并内容板。
- **Loro**：纯库；存储、peer、同步由 Ralph 负责。并发懒建同一 key 必须用 `ensure_mergeable_*`。
- **Peer**：禁止多进程共享同一 PeerID 并发写。
- **依赖**：`loro` 加入 `ralph-core`（crates.io）；行为由配置开关门控，避免 feature 矩阵。
- **测试入口**：仅 `cargo nextest run` 系列（及文档 drift 脚本）；禁止裸 `cargo test -p ralph-cli`。
- **Preset 测试规则**：禁止锁定 hat instructions 全文；只测结构化语义与外部行为。
- **假设**：实现前用编译探测确认所用 `loro` 版本具备 `ensure_mergeable_*`、`export(ExportMode::…)`、`import`。

### 零回归策略（贯穿全部 Unit）

1. 默认配置路径：不创建 `.ralph/collab/**`，不改变 prompt/env（除显式开启）。
2. 每个 Unit 结束后跑「受影响回归包」（见各 Unit）；最终门禁跑全量 `./scripts/run-tests.sh`。
3. 触碰旧模块前先补 Characterization（若该路径无断言）。
4. 禁止用 skip/删断言/`.only`/无解释改 golden 换绿。

---

## 2. BDD 行为规格

```gherkin
Feature: Opt-in Loro Collab Findings Board
  作为多 Agent 编排中的协作内容层
  我希望 findings 能并发写入并合并，且默认不影响现有 review 门禁
  以便在不引入回归的前提下扩展协作能力

  # —— 默认关闭 / 零回归 ——

  Scenario: S0-default-off-no-collab-side-effects
    Given 一份未声明 event_loop.collab 的既有 RalphConfig
    When 解析配置并启动不依赖 collab 的既有路径（task/emit/wave 子集）
    Then collab.enabled 默认为 false
    And 工作区不会被强制创建 .ralph/collab/
    And 既有 review.*.done 的 required_fields 仍要求 findings_file 与 findings_count

  Scenario: S0b-enabled-false-explicit
    Given event_loop.collab.enabled 显式为 false
    When 调用不带 --path 覆盖的 collab CLI（若已实现）
    Then 命令以可观察错误退出（说明未启用或缺少路径），不得写盘成功冒充无 findings

  # —— CollabStore 持久化与合并 ——

  Scenario: S1-two-peers-merge-findings
    Given 同一 collab 根目录与空或可恢复的 store
    When peer A upsert finding_id=f1（dimension=correctness, severity=P1, summary=...）
    And peer A 导出 update 并由 peer B import
    And peer B upsert finding_id=f2（dimension=testing, ...）
    And 任一方 list
    Then list 同时包含 f1 与 f2
    And 各自字段可读且 dimension 不串

  Scenario: S2-upsert-same-id-is-stable
    Given store 中已有 finding_id=f1
    When 同一 peer 再次 upsert 同 id 但更新 summary
    Then list 中 f1 仅一条
    And summary 反映最后一次成功提交的内容（策略在实现中固定并测死）

  Scenario: S3-invalid-upsert-rejected
    Given 合法 store
    When upsert 缺少必需字段（空 finding_id / 空 dimension / 空 summary）
    Then 操作失败且不写入部分脏记录
    And 退出码或 Result 可区分校验失败与 IO 失败

  Scenario: S4-export-dimension-markdown-projection
    Given store 中有 dimension=correctness 的至少一条 finding
    When export --dimension correctness --out <path>
    Then <path> 成为非空 markdown 文件
    And findings_count 计数规则与导出条目数一致（含约定的干净占位规则若启用）
    And 该文件可作为既有 findings_file 字段值通过 schema 形态检查（路径存在、可数）

  Scenario: S5-corrupt-store-fail-visible
    Given collab 持久化文件被截断或损坏
    When 调用 list 或 export
    Then 失败可观察（非零退出或结构化错误）
    And 不得返回“空列表成功”冒充零 findings

  # —— CLI ——

  Scenario: S6-cli-upsert-list-export-roundtrip
    Given 临时 workspace 与显式 --path 指向可写 collab 根
    When 依次执行 ralph tools collab findings upsert / list / export
    Then list JSON/text 含写入项
    And export 文件存在
    And 在污染 env（RALPH_CURRENT_HAT 等）下复跑仍绿（scrub 后显式参数）

  Scenario: S7-cli-help-lists-subcommands
    When 运行 ralph tools collab findings --help
    Then 帮助列出 upsert、list、export（或最终敲定的等价子命令名）
    And 与注入 skill 命令表一致

  # —— 配置 ——

  Scenario: S8-config-enabled-with-path
    Given YAML 含 event_loop.collab.enabled: true 与可选 path
    When 解析 RalphConfig
    Then enabled 为 true 且 path 解析为约定默认或显式值

  # —— opt-in 六维消费者（行为级，不锁 instructions 全文）——

  Scenario: S9-opt-in-dim-writes-via-collab-then-emit-still-valid
    Given collab.enabled=true 且测试用最小 fixture（非全量 live agent）
    When 模拟 dim 路径：collab upsert → export findings_file → 构造 review.correctness.done payload
    Then payload 含既有 required_fields（含 findings_file、findings_count）
    And 对应该 topic 的 policy-check / schema 校验通过

  Scenario: S10-opt-in-synthesizer-can-list-all-dimensions
    Given 同一 plan_name 下两个 dimension 的 findings 已在 collab
    When synthesizer 侧调用 list --plan <plan_name>（或等价 API）
    Then 返回跨维完整集合
    And 可根据 dimension 过滤

  Scenario: S11-default-off-review-path-unchanged
    Given collab 未启用
    When 执行既有「仅 findings_file、无 collab」的 review 相关回归（既有 nextest / scenario 子集）
    Then 全部保持通过
    And 无新增对 collab 路径的强制依赖
```

---

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
| -------- | ---- | ------ | -------- |
| S0 | 默认 enabled=false；无强制建盘；schema required_fields 未改 | 单元（config）+ Characterization（schema 快照/断言） | 否 |
| S0b | disabled 时无路径 CLI 失败可观察 | CLI 集成 | 否 |
| S1 | 两 peer merge 后 list 含双方 | 单元（core collab）+ 并发/幂等补充 | 否 |
| S2 | 同 id 不重复行 | 单元 | 否 |
| S3 | 非法输入拒写 | 单元 | 否 |
| S4 | export 文件与 count 一致 | 单元 + 轻量集成 | 否 |
| S5 | 损坏 store 失败可见 | 单元（fault injection） | 否 |
| S6 | CLI roundtrip；hat env 污染仍绿 | CLI 集成 | 否 |
| S7 | --help 与 skill 一致 | 集成冒烟 + drift 脚本 | 否 |
| S8 | enabled+path 解析 | 单元（config） | 否 |
| S9 | upsert→export→payload 过 schema | 集成（cli+policy） | 否（禁止 live agent E2E） |
| S10 | list 跨维 | 单元/CLI 集成 | 否 |
| S11 | 默认关闭回归包全绿 | 回归（nextest 子集→全量） | 否（全量脚本非 live E2E） |

**额外风险驱动测试（按需，不机械全上）**

| 风险 | 测试类型 | 落在 |
| --- | --- | --- |
| 无测试旧 config 解析 | Characterization | U1 |
| 双 peer / 同 id 重复写 | Concurrency / Idempotency | U3 |
| 损坏快照 | Fault Injection | U2/U3 |
| export markdown 形态 | 轻量 Differential（与手写最小 fixture diff 关键字段） | U4 |
| CLI 在 hat env 下 | 污染复跑 | U5 |
| 接线旧 review 路径 | Regression S11 | U7/U8 |

---

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
| -- | -------- | ---- | ---- | ------- | --- |
| R0 默认关闭零回归 | S0, S0b, S11 | nextest 断言 + 全量门禁 | config 默认值；schema required_fields 未删 | 既有 review/wave/task 子集 | 无 live E2E |
| R1 Collab 可持久化并恢复 | S1, S5 | core 测 | open/commit/import/export | — | 无 |
| R2 双 peer findings 合并 | S1 | core 测 | merge | — | 无 |
| R3 upsert 校验与同 id 稳定 | S2, S3 | core 测 | validate/upsert | — | 无 |
| R4 export 兼容 findings_file | S4, S9 | core + policy | export | `event_policy` / emit policy-check | 无 |
| R5 CLI 可用且抗 hat env | S6, S7 | CLI 集成 | — | `common::ralph_bin` + help | 无 |
| R6 配置可开可关 | S8, S0 | config 测 | parse | — | 无 |
| R7 opt-in dim→emit 仍合法 | S9 | 集成 | — | schema/policy | 无 |
| R8 synthesizer 可读全表 | S10 | CLI/core | list filter | — | 无 |
| R9 skill 与 CLI 一致 | S7 | drift 脚本 | — | check-cli-doc-drift | 无 |

---

## 5. 严格串行开发单元

> 执行口令：完成标准未勾选完，禁止开始下一 Unit。

---

### Unit 1 — 配置开关默认关闭（零回归锚点）

* **Unit 目标：** 引入 `event_loop.collab` 配置形状；默认 `enabled=false`；缺失字段时旧 YAML 仍可解析。
* **对应 Scenario：** S0, S8（仅 parse 部分）
* **外部可观察结果：** `RalphConfig` 解析后 `event_loop.collab.enabled == false`（缺省）；显式 true/path 可读。
* **输入与输出：** 输入 YAML 片段；输出解析后的配置结构（字段名在实现时落在 `crates/ralph-core/src/config/loop_config.rs` 的 `EventLoopConfig` 扩展——**先写测试再加字段**）。
* **可依赖的已完成能力：** 现有 `EventLoopConfig` / `RalphConfig::parse_yaml`。
* **明确禁止依赖的未来能力：** Loro、CLI、findings API、preset 接线。
* **验收测试：** config 单测：缺省 / enabled true / 显式 false。
* **需要拆分的单元测试：** serde 默认值；未知其它字段不受影响的既有 fixture 解析（Characterization：选 1–2 个现有 preset YAML `parse_yaml` 仍 Ok）。
* **Red 预期失败原因：** `EventLoopConfig` 尚无 `collab` 字段或默认不为 false。
* **最小实现范围：** 仅配置结构体 + Default + serde；**不**创建目录、不链接 loro 行为。
* **集成验证：** `cargo nextest run -p ralph-core -- <config_collab 或 loop_config 子集>`
* **回归范围：** 既有 config/preset parse 相关 nextest；`cargo nextest run -p ralph-cli --bin ralph -- presets` 中与 parse 相关的子集（若本 Unit 只加字段且 `#[serde(default)]`，应仍绿）。
* **完成标准：** S0/S8 parse 断言绿；旧 preset 解析 Characterization 绿；无 `.ralph/collab` 副作用。
* **风险与注意事项：** 字段必须 `#[serde(default)]`；勿加入 `PRESET_OPT_IN` 误剥除列表 unless 产品需要——若动 `config_resolution` / `preflight`，本 Unit 内写清并测。

---

### Unit 2 — CollabStore 打开/提交/导入导出/损坏失败

* **Unit 目标：** 在给定根目录上，基于 `loro` 实现可恢复的文档存储（snapshot + updates 或等价崩溃安全策略）；import/export updates；损坏时失败可见。
* **对应 Scenario：** S1 的底层（先用简单 root map 探针亦可，但完成时必须能支撑后续 findings）、S5
* **外部可观察结果：** 目录中出现约定持久化文件；进程重启后 import 历史 updates 可恢复；损坏文件 list/open 失败。
* **输入与输出：** `path` → `CollabStore`；`export_updates() -> bytes`；`import(bytes)`；`commit()`。
* **可依赖：** Unit 1（可选读默认 path 辅助函数，但本 Unit 测试用临时目录显式 path）。
* **禁止依赖：** findings 领域模型、CLI、hat。
* **验收测试：** tempfile 下 commit→reopen；A export → B import；截断文件 → open/list 失败。
* **单元测试：** peer id 分配；空 store；重复 import 幂等或安全。
* **Red 预期失败原因：** 无 `collab` 模块 / 无 loro 依赖。
* **最小实现范围：** `crates/ralph-core/src/collab/mod.rs`（及 store 文件）；`Cargo.toml` 加 `loro`；**不**实现 findings upsert 语义以外的业务字段（可用内部 ping 键自测，但正式 findings API 留 U3）。
* **集成验证：** `cargo nextest run -p ralph-core -- collab`
* **回归范围：** Unit 1 测试；workspace 编译 `cargo check -p ralph-core`（或 nextest 编译期）。
* **完成标准：** S5 绿；双 doc import 往返绿；默认业务路径仍不自动建盘。
* **风险：** 选对 `ensure_mergeable_*` 仅在 U3 使用；本 Unit 固定持久化布局并文档化，避免 U3 改盘格式。并发写同一 store 文件需文件锁或单写者约定——写清并测。

---

### Unit 3 — Findings upsert / list / 双 peer 合并

* **Unit 目标：** 在 CollabStore 上提供 findings 领域 API：按 `finding_id` upsert；list 过滤；两 peer 合并不丢。
* **对应 Scenario：** S1, S2, S3, S10（API 层）
* **外部可观察结果：** `list()` 返回结构化 findings；非法输入 Err；同 id 单行。
* **输入与输出：** upsert({id, dimension, severity, summary, plan_name, …})；list(filter)；禁止依赖 CLI。
* **可依赖：** Unit 2。
* **禁止依赖：** export markdown、CLI、preset。
* **验收测试：** S1/S2/S3 直接对 API；Concurrency：两 store 实例交错 export/import。
* **单元测试：** 校验矩阵；filter by plan/dimension。
* **Red 预期失败原因：** 无 findings API 或误用非 mergeable 容器导致丢写。
* **最小实现范围：** `collab/findings.rs`；文档形状冻结在测试里（结构化断言，非全文 golden）。
* **集成验证：** `cargo nextest run -p ralph-core -- collab`
* **回归范围：** Unit 1–2；确认未改 event schema。
* **完成标准：** S1–S3、S10 API 绿；Mutation 不做强制，但同 id / 双 peer 断言不可弱化。
* **风险：** summary 用 mergeable Text；元数据 LWW 需在测试中写明策略。

---

### Unit 4 — Export 维度 markdown 投影（兼容 findings_file）

* **Unit 目标：** `export_dimension(dimension, out_path)` 生成可供既有 `findings_file` 使用的 markdown；count 规则测死。
* **对应 Scenario：** S4, S9（export 半段）
* **外部可观察结果：** 文件落盘；count == 导出条目规则。
* **输入与输出：** dimension + out path → 文件；返回 count。
* **可依赖：** Unit 3。
* **禁止依赖：** CLI 包装、改 schema required_fields、改 preset YAML 拓扑。
* **验收测试：** 有/无 findings；干净维的占位规则若采用必须与现网 `findings_count >= 1` 约定对齐（见 schema fill_rule）——**若对齐 G0 占位，在本 Unit 实现并测**，不得留给后续。
* **单元测试：** 多维并存时 export 单维不泄漏其它维。
* **Red 预期失败原因：** 无 export 或 count 错。
* **最小实现范围：** 投影格式保持简单可解析列表即可；不追求与历史手写 md 字节级一致（禁止无解释大 golden）。
* **集成验证：** 构造最小 `EventSchema`/`required_fields` 检查：路径存在 + count 字段可填（契约级，不跑全 loop）。
* **回归范围：** Unit 1–3；`event_policy` 中与 `findings_file` 相关既有测不得改断言含义。
* **完成标准：** S4 绿；为 S9 准备好稳定 export 契约。
* **风险：** 勿修改 `presets/schemas/*.yml` required_fields；只适配现有约束。

---

### Unit 5 — `ralph tools collab findings` CLI

* **Unit 目标：** Agent/人可通过 CLI 完成 upsert/list/export；help 可用；hat env 污染下测试仍绿。
* **对应 Scenario：** S6, S7, S0b
* **外部可观察结果：** 子进程退出码与 stdout；export 文件；`--help` 文本含子命令。
* **输入与输出：** clap 参数；复用 Unit 2–4 API。
* **可依赖：** Unit 1–4。
* **禁止依赖：** preset instructions 修改、runtime 自动注入。
* **验收测试：** `crates/ralph-cli/tests/` 新集成测，**必须** `common::ralph_bin()` / scrub；另跑污染 env 复验。
* **单元测试：** 参数互斥/校验可放 cli 侧薄测或依赖 clap。
* **Red 预期失败原因：** `tools.rs` 无 `Collab` 子命令。
* **最小实现范围：** `crates/ralph-cli/src/collab_cli.rs`（名可调整）+ `tools.rs` 挂载；显式 `--path`；当 enabled=false 且无 `--path` 时失败信息稳定（S0b）。
* **集成验证：** `cargo nextest run -p ralph-cli -- collab`
* **回归范围：** 其它 `ralph tools` 集成测；`ralph tools --help` 仍列出 memory/task/skill。
* **完成标准：** S6/S7/S0b 绿；污染复跑绿。
* **风险：** 勿在 CLI 里读 `.ralph/events.jsonl`；path 解析与 workspace root 规则写清。

---

### Unit 6 — 注入 skill 与 CLI 文档漂移门禁

* **Unit 目标：** `ralph-tools-collab.md` 描述触发条件、命令、失败停止；`ralph-tools.md` 入口；drift 脚本通过。
* **对应 Scenario：** S7（文档侧）
* **外部可观察结果：** skill 文件存在；`scripts/check-cli-doc-drift.sh` 退出 0；无计划号/内部模块名泄漏。
* **输入与输出：** markdown skill；与 clap 帮助一致。
* **可依赖：** Unit 5（命令名已稳定）。
* **禁止依赖：** 改 hat 拓扑、改 schema。
* **验收测试：** drift 脚本；必要时现有 skill load 测。
* **单元测试：** 一般不需要。
* **Red 预期失败原因：** 无 skill 或命令表与 clap 不符。
* **最小实现范围：** 仅 `crates/ralph-core/data/*.md` 允许的注入文件；按需 load 机制对齐现有 skill 模式。
* **集成验证：** `scripts/check-cli-doc-drift.sh`；`ralph tools skill load` 若适用。
* **回归范围：** 既有 ralph-tools 相关测。
* **完成标准：** 可读性/去计划化 HARD RULE 自检通过；S7 文档侧绿。
* **风险：** 不要把 Loro/OpLog 实现细节写进 skill。

---

### Unit 7 — opt-in 六维消费者最小接线（行为，不锁文案）

* **Unit 目标：** 当 `collab.enabled=true` 时，提供 **可测的** dim→Collab→export→emit 合法载荷路径；synthesizer 可通过 list 读全表。默认 false 时 **零调用**。
* **对应 Scenario：** S9, S10, S11（本 Unit 先保证 S9/S10；S11 在 U8 加码）
* **外部可观察结果：** 测试 fixture 下 policy-check 通过；list 跨维；enabled=false 时无 collab 写盘。
* **输入与输出：** 测试驱动的最小 wiring（优先 **库级/CLI 级编排函数** 或 prompt 注入开关，避免大改 preset）。若必须改 preset `instructions`，只加对 skill 的引用句，**禁止**新增全文锁定测。
* **可依赖：** Unit 1–6。
* **禁止依赖：** frontier 增量 prompt；改 `required_fields`；改 wave/supervisor。
* **验收测试：** 集成测覆盖 S9/S10；S11 子集：enabled 缺省时既有 review 测不依赖 collab。
* **单元测试：** “enabled 门面”：false 时 wiring API no-op。
* **Red 预期失败原因：** 无门面或 enabled 时未走 export。
* **最小实现范围：**  
  - 优先：`ralph-core` 提供 `write_dimension_findings_and_export(...)` 供 CLI/agent 脚本调用；  
  - preset 仅文档/instructions **引用** skill（可选）；  
  - **不**强制改 builtin preset 默认配置为 enabled。
* **集成验证：** `cargo nextest run -p ralph-core -- collab` + `cargo nextest run -p ralph-cli -- collab` + policy 相关子集。
* **回归范围：** `cargo nextest run -p ralph-core -- event_policy` 中 findings_file 相关；wave_context 相关既有测。
* **完成标准：** S9/S10 绿；默认路径无新失败；无 schema required_fields diff。
* **风险：** Outside-In 到此为止；不要把 synthesizer 主逻辑改成强依赖 collab。

---

### Unit 8 — 回归加固与最终门禁（禁止夹带新功能）

* **Unit 目标：** 证明功能拓展不引入回归；收口文档与完成定义。
* **对应 Scenario：** S0, S11 全量；全部计划 Scenario 复核
* **外部可观察结果：** `./scripts/run-tests.sh` 绿；clippy/fmt 按仓库惯例；无新增 ignore/skip。
* **输入与输出：** 仅测试与必要的缺陷修复（**禁止**本 Unit 新功能）。
* **可依赖：** Unit 1–7。
* **禁止依赖：** 任何新 API。
* **验收测试：** 全量脚本；若出现 flake，仅允许 `RALPH_BASELINE_SERIAL=1` 作为诊断，根因仍须修。
* **单元测试：** 无新功能；可补 Characterization 挡回归。
* **Red 预期失败原因：** 若全量红，本 Unit 只修回归，不扩 scope。
* **最小实现范围：** bugfix only。
* **集成验证：** `./scripts/run-tests.sh`；`scripts/check-cli-doc-drift.sh`；`cargo clippy` / `cargo fmt --check`（仓库若要求）。
* **回归范围：** workspace（exclude e2e 按脚本）；显式再跑 `ralph-cli` presets / `ralph-core` scenarios 子集若脚本已覆盖可不再重复。
* **完成标准：** 第 6 节门禁全部勾选；剩余风险仅允许 follow-up 列表中的项。
* **风险：** 严禁用跳过测试换绿。

---

## 6. 最终质量门禁

Executor 在宣告完成前必须满足：

* [ ] 计划内 Scenario S0–S11 均有自动化证据（S0b/S5/S7 含在内）
* [ ] 所有新建/相关单元测试通过：`cargo nextest run -p ralph-core -- collab`
* [ ] 所有必要集成测试通过：`cargo nextest run -p ralph-cli -- collab`（含 hat env 污染复跑）
* [ ] 无 live-agent E2E 要求；禁止用真实 LLM 调用充当门禁
* [ ] `scripts/check-cli-doc-drift.sh` 通过
* [ ] `./scripts/run-tests.sh` 通过（或记录等价全量门禁；flake 不得用 skip 掩盖）
* [ ] `cargo clippy` / `cargo fmt` 按项目惯例通过
* [ ] **零回归证据：** 未改 `review.*.done` required_fields；默认 `collab.enabled=false`；未启用时无强制 `.ralph/collab/`；wave/supervisor/TaskStore 无行为变更 diff
* [ ] 无新增 `#[ignore]` / 跳过 / 削弱断言
* [ ] **未验证内容 / 剩余风险（允许遗留）：** frontier 增量 prompt；builtin preset 默认开启；`findings_file` 字段退役；memories 迁入；真实多维 live loop 人工 spot-check（可选，非门禁）

---

## Executor 速查

```text
串行：U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8
每 Unit：写验收测 → Red（正确原因）→ 最小单测 TDD → 集成 → 回归 → 完成标准 → 下一 Unit
测试入口：cargo nextest run … / ./scripts/run-tests.sh
CLI 测：common::ralph_bin + scrub；污染复跑
禁止：改调度内核；改 required_fields；默认开启；skip 换绿；本计划阶段写生产代码
```
