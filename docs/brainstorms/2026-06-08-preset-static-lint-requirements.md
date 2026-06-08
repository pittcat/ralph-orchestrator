---
date: 2026-06-08
topic: preset-static-lint
---

# Preset 静态校验体系：owner_hat、跨 Hat 拓扑与 Topic 格式

## Summary

为 Ralph 编排器建立 **preset 静态 lint** 子系统：编排启动时对 preset yml 做强校验（Hard Gate），覆盖 owner_hat 字段、跨 hat 拓扑、topic 格式三类错误。lint 是"看门狗"角色 —— 不在 loop 中、不修改 preset、只产出结构化错误报告，由人类 review 后手动修复。

## Problem Frame

2026-06-08 在 `ce-executor` preset 的 grand-lily worktree run 中，编排器在 14 个事件内暴露了 4 类 preset 级错误，全部发生在 loop **运行中**，且都本可以在 **编排启动时** 静态拦截：

1. **跨 hat 越权 publish**：`shipper` publish 了 `review.complete`（属 `review-synthesizer` 的输出），`ralph`（兜底）publish 了 `work.done`（属 `executor` 的输出），`review-coordinator` 重复 publish `work.done`。这违反 hat 职责单一原则，让 `event_origin` 调试时无法定位"到底是谁干的"。

2. **topic 大小写不一致**：`REVIEW_COMPLETE`（大写，下划线）出现在 events 文件并被 `verdict_gate` 接受，但同 preset 的 `event_policy.schemas` 全部用 `review.*`（小写，点分）。同一 preset 内部两种命名约定并存。

3. **payload schema 漂移**：`work.failed` 的 `reason`、`report.done` 的 `report_path`、`review.passed` 的 `fix_round` 三个字段在不同迭代中缺失，runtime 校验通过部分字段后才报 violation，定位耗时。

4. **coordinator_hats 缺失**：`executor` 创建的 runtime task 收不到 `coordinator` 的关闭信号（preset 未配 `coordinator_hats`），导致 task 永远 open，最终被 recovery 兜底撑住（掩盖了 preset 错误）。

根因是 `ralph run` 启动前只跑 `preset_validator` 的拓扑连通性检查（谁能 publish 谁，谁 subscribe 谁），但**不**检查：
- 哪些 hat 是某 topic 的"责任方"（owner），以便违规时定位；
- 同一 hat 是否 publish 了不该 publish 的 topic（越权）；
- topic 字符串是否符合格式规范；
- task 系统所需的 `coordinator_hats` 是否齐备。

运行时再发现 → 只能靠 recovery 撑住，**preset 错误没被反馈回路捕获**，下次 run 还犯。

## Key Decisions

### 1. 引入 `owner_hat` 新字段，区分"权限"与"责任"

`publishes` 是允许名单（"哪个厨师能往这个窗口出菜"），`owner_hat` 是责任名单（"哪个厨师拥有这个菜谱，菜出问题时找他"）。两者都加在 **topic 级别** 而非 hat 级别：

- 在 preset 顶层加 `topic_owners: { work.done: executor, review.complete: review-synthesizer, ... }`。
- 同一 topic 可被多个 hat publish（现状），但只能有一个 owner（用于"菜出问题找谁修"）。
- lint 检查"每个被 publish 的 topic 是否声明了 owner"；运行期 Doc 3 的 feedback 路由用 owner 定位"上报到谁"。

不把 owner 塞进 `publishes` 数组里（语义混淆），也**不**强制每 hat 唯一 owner（多个 topic 共享一个 owner hat 是正常拓扑）。

### 2. lint 是 Hard Gate，编排启动时拦截

- `ralph run` 启动前**不可跳过**地执行 preset 静态 lint。
- 失败 → 直接退出（exit code 非零），不进入 loop，不产生任何事件文件。
- 与 `2026-06-02-payload-contract-validation-requirements.md` 的"payload 契约校验"是两层 gate：本次负责"preset 自身结构与命名"，那份 doc 负责"payload 字段 schema"。

不提供 `--skip-lint` 开关。**AI 不直接改 preset**（见 Doc 3），lint 的输出是给人类 review 用的。

### 3. topic 格式统一为 lowercase.dot.case

- 合法 pattern：`^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$`。
- `REVIEW_COMPLETE`、`workDone`、`Review.Complete` 全部 reject，错误信息附"建议替代写法"。
- 现有 `event_policy.schemas` 已是小写点分，lint 等于"补上 enforce 步骤"。
- `LOOP_COMPLETE` 这类历史遗留大写 token 走显式白名单（在 `event_loop.completion_promise` 解析时白名单化，不进 lint 错误）。

### 4. `coordinator_hats` 缺失是 lint 必报项

- 凡是 `tasks.enabled: true` 的 preset，必须声明 `coordinator_hats` 至少一个。
- lint 检查"所有 `task_*` 类 hat 是否被 `coordinator_hats` 覆盖"（粗规则：所有 `publishes` 含 `task.*` 的 hat 必须出现于 `coordinator_hats`）。
- 当前 `ce-executor.yml` 配了 `coordinator_hats: [executor, coordinator]`，lint 通过。

### 5. lint 输出人类可读 + 机器可消费

- 终端：按文件路径 + 行号 + 一行 hint 输出（参考 `2026-06-02-payload-contract-validation-requirements.md` 的 R5.4 格式）。
- 文件：`.ralph/diagnostics/preset-lint-{timestamp}.json`，含 `error_type / file:line / topic / hat_id / fix_hint`。
- 退出码：`0` 通过 / `2` lint 失败 / 其他同现有约定。

## Actors

- **Preset 作者（人类）**：lint 错误的目标消费者。fix 是人类的工作。
- **编排器（rust binary）**：执行 lint，作为 Hard Gate 拦截器。
- **AI agent（loop 内）**：lint 失败时**不**得到控制权 —— 编排器在 loop 启动前就拒了。AI agent 不在 lint 反馈回路中（这是与 Doc 3 的关键区别，Doc 3 才让 AI 看到 feedback）。

## Requirements

### R1. owner_hat 字段定义

- R1.1 preset 顶层新增可选字段 `topic_owners: { [topic: string]: hat_id }`。
- R1.2 `hat_id` 必须引用 preset 中已声明的 hat（type: string, 唯一于 preset 内）。
- R1.3 同一 `topic` 在 `topic_owners` 中只能出现一次（一对一映射）。
- R1.4 缺少 owner 不自动报错（**向后兼容**）：lint 默认 `warn` 级别，需要 `--strict` 才升级为 `error`。这是为了已存在 preset 不被一次 lint 全部阻断。

### R2. 跨 hat 拓扑越权检查

- R2.1 lint 枚举所有 hat 的 `publishes` 数组，对每个 `(hat, topic)` 边查询 `topic_owners`。
- R2.2 若 `topic_owners[topic]` 存在但 `(hat, topic)` 不在允许的 `publishes` 中 → `error: cross_hat_unauthorized_publish`。
- R2.3 若 `topic_owners[topic]` 不存在（向后兼容）→ 仅在 `--strict` 模式下报错。
- R2.4 错误信息必须包含：违规 hat 名、违规 topic、owner_hat（如有）、owner 拒绝原因（"owner=X 才有权"）、preset 中 `publishes` 的行号。

### R3. topic 格式校验

- R3.1 所有出现在 `publishes`、`triggers`、`event_policy.schemas`、`topic_owners`、`required_events`、`completion_promise`、verdict_gate.topic 字段的 topic 字符串必须匹配 R3.0 的 regex。
- R3.2 不匹配 → `error: invalid_topic_format`，错误信息附"建议替代"（自动转换尝试，如 `REVIEW_COMPLETE` → `review.complete`，仅在 lint 报告里显示，不修改文件）。
- R3.3 白名单：preset 可显式声明 `topic_format_whitelist: ["LOOP_COMPLETE", ...]`，白名单内 token 跳过格式检查（用于历史遗留 token）。

### R4. coordinator_hats 完备性检查

- R4.1 若 `tasks.enabled: true` 且 `coordinator_hats` 未声明或为空数组 → `error: missing_coordinator_hats`。
- R4.2 启发式规则（lint 阶段粗检，运行时由 task 系统精检）：所有 hat 的 `publishes` 中含 `task.*` topic 的 hat 必须在 `coordinator_hats` 中出现。
- R4.3 错误信息附"建议：在 `tasks.coordinator_hats` 加入 [...]"。

### R5. lint 执行与输出

- R5.1 `ralph run` 启动时**强制**调用 preset lint，不可跳过。
- R5.2 lint 通过 → 进入正常 loop 启动流程。
- R5.3 lint 失败 → 输出错误报告（终端 + json 文件），退出码非零。
- R5.4 终端格式示例：
  ```
  [PRESET LINT FAILED] preset=ce-executor errors=3
  
  presets/en/ce-executor.yml:482
    error: cross_hat_unauthorized_publish
    hat=shipper publishes=review.complete
    owner_hat=review-synthesizer (the only hat authorized to publish review.complete)
    fix_hint: remove `review.complete` from shipper.publishes, or set topic_owners.review.complete=null
  
  presets/en/ce-executor.yml:316
    error: invalid_topic_format
    topic=REVIEW_COMPLETE
    suggested=review.complete
    fix_hint: rename topic in publish/trigger/schema to `review.complete` (and add to topic_format_whitelist if preserving token is required)
  
  See .ralph/diagnostics/preset-lint-20260608T180216.json for full report.
  ```
- R5.5 json 报告结构：
  ```json
  {
    "preset": "ce-executor",
    "preset_path": "presets/en/ce-executor.yml",
    "errors": [
      {
        "error_type": "cross_hat_unauthorized_publish",
        "file": "presets/en/ce-executor.yml",
        "line": 482,
        "hat_id": "shipper",
        "topic": "review.complete",
        "owner_hat": "review-synthesizer",
        "fix_hint": "..."
      }
    ]
  }
  ```
- R5.6 `ralph hats validate` 单独提供 lint 入口（不启动 loop）。

### R6. 现有 preset 渐进迁移

- R6.1 内置 8 个 preset 首次 lint 时按"warn 默认、error 仅 `--strict`"运行，避免一次性大爆炸。
- R6.2 CI 跑 `ralph hats validate --strict` 强制所有 preset 升级到 strict 通过，作为本次需求的"通过门槛"。
- R6.3 仓库维护者负责逐 preset 补 `topic_owners`、修 topic 命名、补 `coordinator_hats`；**不**自动迁移（避免 AI 改 preset）。

## Acceptance Examples

- AE1. **现有 ce-executor 跑通 strict lint**
  - **Given** 仓库维护者按 R6.1 起步，按 R6.3 完成 `topic_owners` / topic 命名 / `coordinator_hats` 修补。
  - **When** 跑 `ralph hats validate --strict` 对 8 个内置 preset。
  - **Then** 全部 exit code 0，0 errors。
- AE2. **故意越权 preset 启动时被拒**
  - **Given** 临时把 `shipper.publishes` 加上 `work.done`（owner = `executor`）。
  - **When** 跑 `ralph run -H builtin:ce-executor -p "..."`。
  - **Then** lint 报 `cross_hat_unauthorized_publish`，exit code 2，loop 不启动，无 events 文件产生。
- AE3. **大写 topic 在 verdict_gate 仍工作**
  - **Given** preset 显式声明 `topic_format_whitelist: ["LOOP_COMPLETE"]`。
  - **When** lint 跑。
  - **Then** `LOOP_COMPLETE` 不报 `invalid_topic_format`，其他大写 topic 仍报错。
- AE4. **coordinator_hats 缺失**
  - **Given** preset 写 `tasks.enabled: true` 但不写 `coordinator_hats`。
  - **When** lint 跑。
  - **Then** 报 `missing_coordinator_hats`，列出"应加入的候选 hat 列表"（按 R4.2 启发式）。

## Success Criteria

- [ ] `crates/ralph-core/src/preset_lint.rs`（或同名模块）实现 R1–R5 全部规则，单测覆盖每个 error_type。
- [ ] `ralph run` 启动流程强制走 lint；`ralph hats validate [--strict]` 是公开入口。
- [ ] 8 个内置 preset 在 strict 模式下全部通过 0 errors。
- [ ] `ce-executor.yml` 补齐 `topic_owners`（10 个 hat 至少映射 12 个关键 topic）；其余 7 个 preset 至少补齐 `coordinator_hats`。
- [ ] 终端错误格式与 R5.4 一致，json 报告结构与 R5.5 一致。
- [ ] lint 不修改 preset 文件（只读）。
- [ ] `cargo test` 通过（`./scripts/run-tests.sh` 走完 nextest + doctest）。

## Scope Boundaries

### 包括（In Scope）

- `owner_hat` / `topic_owners` 字段的 schema 扩展
- 跨 hat 越权 publish 静态检查
- topic 格式 lint + 白名单机制
- `coordinator_hats` 缺失检查
- lint 报告的终端 + json 输出
- 8 个内置 preset 的 strict 通过

### 不包括（Out of Scope）

- **payload 字段 schema 校验**：由 `2026-06-02-payload-contract-validation-requirements.md` 覆盖。
- **运行时 owner_hat 强制**：lint 是编排期；运行时把"违规 publish"扔给 `event_origin` 已经是 `2026-05-31-event-origin-guard-requirements.md` 的范围。本次不重复。
- **AI 自动修 preset**：lint 错误由人类 review 后手动修（与 Doc 3 反馈回路一致）。
- **自定义（非 builtin）preset 的迁移工具**：本次只 lint 内置 8 个。
- **wave worker 拓扑**：wave 内的 `dimension-reviewer` 多 worker 实例 vs 单一 owner 的关系是 wave 体系内部问题，留给 wave 单独 doc。

## Dependencies / Assumptions

- `presets.rs:35` 的 `include_str!` 编译机制意味着 lint 校验的是运行时 binary 内的副本（与磁盘 `presets/en/ce-executor.yml` 可能有 drift）。lint 必须从 **编译进 binary 的那份** 读取，而非从磁盘，否则会误报。Mirror-drift guard 测试（`presets.rs:1087` 附近）保证两者一致。
- `2026-06-02-payload-contract-validation-requirements.md` 的 R1（R1.1 `schema_file`）已知是 builtin preset 失效场景（preset 文件路径在运行时不可解析），本次 lint 不依赖该机制。
- 假设人类 preset 维护者接受"strict 通过是上线路槛"的额外工序；这是 Ralph "Hard Gate Over Prescription" 原则的延伸。
- 假设 `topic_owners` 是新字段，旧 preset 不写也算合法（仅 `--strict` 才报错），保证向后兼容。

## Sources / Research

- 现场证据 1：`.worktrees/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf/.ralph/events-20260608-100217.jsonl`（14 events，`work.done` 来自 4 个不同 hat 含兜底 `ralph`，`REVIEW_COMPLETE` 大写 topic）。
- 现场证据 2：`.ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl`（12 events，跨 hat publish 多次被 runtime 发现但 preset 未报）。
- 现场证据 3：`presets/en/ce-executor.yml:482-485`（shipper 声明 `publishes: [..., review.complete, ...]`，属 `review-synthesizer` 越权）。
- 现场证据 4：`presets/en/ce-executor.yml:316` 附近（`verdict_gate.topic: "REVIEW_COMPLETE"` 大写，与 schemas 小写点分并存）。
- 现场证据 5：`presets/en/ce-executor.yml` 顶层 `tasks.coordinator_hats: [executor, coordinator]`（本 preset 配齐，启发式可工作）。
- 现有 doc：`2026-06-02-payload-contract-validation-requirements.md`（payload 字段层 lint，与本次正交）。
- 现有 doc：`2026-05-31-event-origin-guard-requirements.md`（运行时 event origin 校验，与本次编排期 lint 互补）。

## 实现计划指引

给后续 ce-plan 的参考信息。

### 修改文件列表

1. **`crates/ralph-core/src/config.rs`**
   - `RalphConfig` 新增可选字段 `topic_owners: Option<HashMap<String, String>>`
   - `RalphConfig` 新增可选字段 `topic_format_whitelist: Option<Vec<String>>`
2. **`crates/ralph-core/src/preset_lint.rs`**（新文件）
   - 实现 `pub fn lint_preset(config: &RalphConfig) -> LintReport`
   - 4 个独立检查函数：`check_topic_owners` / `check_cross_hat_publish` / `check_topic_format` / `check_coordinator_hats`
   - 报告结构 `LintReport { errors: Vec<LintError>, warnings: Vec<LintError> }`
   - 每个 error 携带 `file: String, line: u32, error_type: String, ...`
3. **`crates/ralph-core/src/preset_validator.rs`**
   - 把 `lint_preset` 集成进 `validate_preset` 流程，作为编排期 gate
4. **`crates/ralph-cli/src/loop_runner.rs`**（或 `event_loop/mod.rs`）
   - `ralph run` 启动前调用 `lint_preset`；失败则输出 R5.4 格式 + 写 json 报告 + 退出
5. **`crates/ralph-cli/src/hats.rs`**
   - `HatsCommands::Validate` 新增 `--strict` 标志，调用 `lint_preset` 走完整规则
6. **`presets/en/ce-executor.yml`**
   - 仓库维护者补 `topic_owners` 字段（10 个 hat，映射所有 publish 出去的 topic）
   - 改 `verdict_gate.topic: "REVIEW_COMPLETE"` → `"review.complete"`
   - 必要时加 `topic_format_whitelist: ["LOOP_COMPLETE"]`
7. **`presets/en/{autoresearch,code-assist,debug,hatless-baseline,merge-loop,pdd-to-code-assist,research,review}.yml`**
   - 逐 preset 补 `coordinator_hats`（如缺）
   - 视情况补 `topic_owners`（仅 strict 需要）

### 测试策略

- **单元测试**（`preset_lint.rs` 内或独立 `tests/`）：
  - 4 个检查函数各 3+ 用例：合法 / 越权 / 缺 owner / 格式不匹配 / 白名单豁免 / coordinator_hats 缺
- **集成测试**：
  - 故意构造坏 preset → 跑 `ralph hats validate --strict` → 验证 exit code 2 与 json 报告字段
  - `ralph run` 启动 lint 拦截 → 验证无 events 文件、无 agent 进程
- **冒烟测试**：
  - 8 个内置 preset 全部 strict 通过 0 errors
  - 现有未配 `topic_owners` 的 preset 默认模式下不报错（向后兼容）
  - Mirror-drift guard 测试继续通过

### 增量交付顺序

1. PR 1：lint 模块 + `topic_owners` schema + 4 个检查函数 + 单测 + 默认模式开启
2. PR 2：现有 8 个 preset 补 `topic_owners` + `coordinator_hats`，`ralph hats validate --strict` 在 CI 跑
3. PR 3：strict 模式默认开启（编排启动时强制），与现有 payload 契约 gate 串联
