---
date: 2026-06-02
topic: payload-contract-validation
---

# Payload 契约强制校验与运行时诊断系统

## Summary

为 Ralph Orchestrator 建立一套**双重防护**机制：编排阶段强制校验 preset/workflow 的 payload 契约，运行时严格检查事件负载字段匹配性，出错时立即暂停 loop 并生成结构化诊断报告，指导开发者修复 preset。

当前 preset（如 `ce-executor.yml`）的 instructions 中存在大量隐式的 payload 字段约定（如 `"From event payload: dimension, focus, depth, plan_name, task_id..."`），但系统既不在编排时校验上游 hat 是否保证提供这些字段，也不在运行时发现字段缺失时给出明确诊断。这导致开发者只能在运行时观察 agent 行为异常后人工排查，调试成本极高。

本需求通过引入**显式 schema 定义**、**跨 hat 契约静态分析**和**运行时 enforce 模式**，将 payload 错误从"运行时黑盒故障"转变为"编排期可拦截/运行时可定位"的明确问题。

---

## Problem Frame

### 现状问题

1. **无编排期校验**：`ralph run` 启动前只检查拓扑连通性（`preset_validator.rs`），不检查 hat 之间传递的 payload 字段是否匹配。 preset 作者可能在 `coordinator` 的 instructions 中漏写 `task_id`，而 `dimension-reviewer` 的 instructions 中依赖该字段 —— 系统对此完全无感知。

2. **schema 能力闲置**：`event_policy.schemas` 配置项和 `event_policy.rs` 的运行时校验逻辑已经存在，但所有内置 preset（包括 `ce-executor.yml`）均未配置 schema，运行时校验形同虚设。

3. **运行时错误模糊**：当 agent 收到缺少字段的 payload，通常表现为 agent 输出混乱、重试、或产生错误事件，但没有结构化错误报告指出"哪个 hat、哪个事件、缺哪个字段"。

4. **preset 文件臃肿顾虑**：如果要求把所有事件的 schema 定义都写进 preset YAML，会导致文件膨胀（`ce-executor.yml` 已有 1100+ 行），降低可维护性。

### 根本矛盾

编排者（preset 开发者）在 instructions 中用自然语言描述了 payload 约定，但系统无法将这些自然语言约定转化为可机器校验的契约。需要一套机制将隐式约定显式化，并在编排期和运行期双重 enforce。

---

## Requirements

### R1. 外部 Schema 文件支持（解决臃肿问题）

- R1.1 `event_loop.event_policy` 支持引用外部 schema 文件，例如：
  ```yaml
  event_loop:
    event_policy:
      enabled: true
      mode: enforce
      schema_file: "ce-executor/schemas.yml"
  ```
- R1.2 若 `schema_file` 存在，系统从该文件加载 schema 定义；若不存在，回退到 `event_loop.event_policy.schemas` 内联定义。
- R1.3 外部 schema 文件格式与现有 `EventSchema` 结构一致（`payload` 类型、`required_fields`、`allowed_values`）。
- R1.4 schema 文件路径支持相对于 preset 文件所在目录的相对路径。

### R2. 编排期强制校验（Preset Gate）

- R2.1 `ralph run` 启动前必须自动执行 payload 契约校验，**不可跳过**（无 `--skip-payload-check` 类参数）。
- R2.2 校验基于 `event_policy.schemas` 中定义的事件 schema，检查每个 hat 的 instructions 中显式引用的 payload 字段是否在对应 trigger 事件的 schema 中有声明。
- R2.3 校验检查跨 hat 字段传递一致性：对每条 topic，检查所有发布该 topic 的 hat 的 schema，与所有订阅该 topic 的 hat 的 instructions 中引用的字段是否兼容。
- R2.4 若校验失败，`ralph run` 直接退出（exit code 非零），并在终端输出人类可读的错误摘要（包含文件路径和行号）。
- R2.5 校验失败时**不启动 loop**，不产生任何事件文件或 agent 调用。

### R3. Instructions 字段引用静态分析

- R3.1 系统从 hat 的 instructions 文本中提取显式的 payload 字段引用。识别模式包括但不限于：
  - ``payload's `field_name` ``
  - ``From event payload: `field1`, `field2` ``
  - ``payload MUST include: `field_name` ``
  - ``event payload `field_name` field``
  - Markdown 代码块中以反引号包裹的标识符，出现在 payload 相关语境中
- R3.2 提取出的字段名必须与该 hat 的任一 trigger topic 的 schema `required_fields` 匹配；若未匹配，报校验错误。
- R3.3 若某个 hat 的 trigger topic 在 schema 中无定义，且该 hat 的 instructions 中引用了 payload 字段，报校验错误（强制要求所有被引用的 trigger 事件必须有 schema 定义）。

### R4. 运行时严格校验（Runtime Guard）

- R4.1 运行时 `event_policy` 默认以 `mode: enforce` 执行（若 preset 未显式配置，系统默认启用 enforce）。
- R4.2 运行时校验覆盖：payload 类型、`required_fields` 存在性、`allowed_values` 合法性。
- R4.3 运行时校验失败不触发 `task.resume` 或重试，而是触发 **Loop Pause**（见 R5）。

### R5. Loop 暂停与诊断报告

- R5.1 运行时 payload 校验失败时，loop 立即停止调度后续 iteration，进入暂停状态。
- R5.2 系统生成结构化诊断报告，写入 `.ralph/diagnostics/payload-contract-error-{timestamp}.json`。
- R5.3 诊断报告必须包含以下字段：
  - `error_type`: 错误类型（`missing_required_field` / `payload_type_mismatch` / `invalid_field_value` / `schema_undefined`）
  - `timestamp`: ISO 8601 格式
  - `event.topic`: 出错的事件 topic
  - `event.source_hat`: 发布该事件的 hat ID
  - `event.target_hat`: 接收该事件的 hat ID（即当前激活的 hat）
  - `field`: 具体出错的字段名
  - `severity`: 固定为 `"error"`
  - `message`: 人类可读的错误描述
  - `details.schema_defined_in`: schema 定义位置（文件路径:行号）
  - `details.downstream_reference`: 下游 hat 引用该字段的位置（preset 文件路径:行号）
  - `details.upstream_reference`: 上游 hat 发布该事件的 instructions 位置（preset 文件路径:行号）
  - `fix_hint`: 修复建议文本
- R5.4 终端输出必须同时显示错误摘要，格式示例：
  ```
  [PAYLOAD CONTRACT VIOLATION] Loop paused.
  Field `task_id` missing in event `review.wave.ready`
    Expected by: dimension-reviewer (presets/ce-executor.yml:390)
    Published by: review-coordinator (presets/ce-executor.yml:184)
    Schema: presets/ce-executor/schemas.yml:45
  See .ralph/diagnostics/payload-contract-error-20260602.json for details.
  ```
- R5.5 暂停状态下，loop 不自动恢复；开发者修复 preset/schema 后，重新执行 `ralph run` 继续。

### R6. Schema 完备性强制

- R6.1 在严格模式下（`ralph hats validate --strict` 或编排期 gate），所有被 hat triggers 引用的事件 topic 必须在 schema 中有定义。
- R6.2 若缺少 schema 定义，校验错误信息必须明确指出"topic `xxx` 被 hat `yyy` 订阅，但 schema 中未定义"。

---

## Success Criteria

- [ ] `ralph run` 启动前，若 preset 存在 payload 契约问题（如下游依赖字段上游未提供），直接拒绝启动并给出明确的文件位置。
- [ ] `ce-executor.yml` 的 schema 定义可独立存放在外部文件（如 `presets/ce-executor/schemas.yml`），preset 主文件保持精简。
- [ ] 运行时若 agent 发布的事件缺少 `required_fields` 中的字段，loop 立即暂停，生成 JSON 诊断报告。
- [ ] 诊断报告包含足够信息，使开发者能在 1 分钟内定位到 preset 中需要修改的具体行。
- [ ] 现有未配置 schema 的 preset 在升级后仍能运行（向后兼容：未启用 `event_policy` 时保持现有行为，或默认以 observe 模式运行）。
- [ ] `ralph hats validate --strict` 能捕获所有 payload 契约违规，并以非零 exit code 返回。

---

## Scope Boundaries

### 包括（In Scope）
- 外部 schema 文件加载机制
- 编排期 payload 契约静态校验（`preset_validator.rs` 扩展）
- Instructions 字段引用启发式提取
- 运行时 `event_policy` enforce 模式默认启用
- Loop 暂停与结构化诊断报告生成

### 不包括（Out of Scope）
- **自动修复 preset**：本需求只生成诊断报告，由人手动修复（用户明确选择）。
- **Instructions 语义理解**：不尝试用 NLP 理解自然语言，只基于约定格式（如 `` `field` ``、"payload MUST include" 等模式）做启发式提取。
- **Payload 值的内容校验**：除 `allowed_values` 外，不校验字段值的具体业务含义（如 `plan_name` 是否指向存在的文件）。
- ** retroactive 修复已有事件文件**：只针对运行时新产生的事件做校验。
- **Wave 子事件的特殊契约**：wave worker 的 payload 契约校验先复用普通 hat 的逻辑，wave 特有的字段聚合逻辑本次不单独处理。

---

## Key Decisions

### 1. Schema 定义方式：显式手写 + 外部文件
- 用户明确要求手写 schema 以保证严格性，但担心 YAML 臃肿。
- 决策：支持 `schema_file` 引用外部文件，将 schema 与 preset 主文件分离。
- 不选择自动从 instructions 推导作为主方案（虽然可以后续作为辅助工具），因为推导有误报风险，与"十分严格"的要求冲突。

### 2. 运行时错误处理：Loop Pause（不终止，不重试）
- 用户选择"立即停车，修好再走"。
- 决策：暂停 loop（不继续调度新 iteration），保留当前状态，等待开发者修复后重新 `ralph run`。
- 不选择 `task.resume` 重试（避免无限循环），也不选择终止 loop 后从零开始（保留已有进度）。

### 3. 修复方式：人工作 + 诊断报告
- 用户选择由开发者根据诊断报告手动修复 preset。
- 决策：诊断报告采用 JSON 结构化格式，包含文件路径、行号、修复提示，便于人快速定位。
- 不实现 Agent 自动修改 preset 或 `ralph preset fix` 命令（超出本次范围）。

### 4. 严格程度：Hard Gate（不可跳过）
- 编排期校验失败直接阻止 `ralph run`。
- 不保留 `--skip` 开关，确保 preset 开发者无法绕过契约检查。
- 向后兼容策略：未配置 `event_policy` 的现有 preset 默认不启用 enforce（但会在 `ralph hats validate --strict` 中报 schema 缺失警告）。

---

## Dependencies / Assumptions

- `event_policy.rs` 已具备运行时 schema 校验能力（类型、required_fields、allowed_values），本次主要扩展其编排期静态分析和默认 enforce 行为。
- `preset_validator.rs` 已具备拓扑图构建能力（`TopologyGraph`），跨 hat 契约校验可复用该图结构做上下游字段匹配。
- `ralph-cli/src/hats.rs` 的 `validate_hats` 是编排期校验的 CLI 入口，可将 payload 契约校验集成至此。
- 假设 preset 开发者愿意为每个事件编写 schema 定义（这是用户明确接受的权衡）。
- 假设 instructions 中的 payload 字段引用遵循可提取的文本模式（如反引号包裹、"payload" 关键词附近）。

---

## 实现计划指引

给后续 ce-plan 的参考信息：

### 修改文件列表

1. **`crates/ralph-core/src/config.rs`**
   - `EventPolicyConfig` 新增 `schema_file: Option<String>` 字段
   - 配置加载时，若 `schema_file` 存在，自动读取并合并到 `schemas` 中

2. **`crates/ralph-core/src/preset_validator.rs`**
   - 新增 `validate_payload_contracts(config, registry) -> ContractValidationResult`
   - 实现跨 hat 字段传递一致性检查
   - 实现 instructions 字段引用提取（启发式正则/文本分析）
   - 与现有 `validate_preset_topology` 合并为统一的编排期校验流程

3. **`crates/ralph-core/src/event_policy.rs`**
   - 修改默认行为：当 `event_policy.enabled` 为 true 且未显式设置 `mode` 时，默认使用 `Enforce`（当前默认是 `Observe`）
   - 增强错误信息生成，为诊断报告提供结构化数据

4. **`crates/ralph-cli/src/hats.rs`**
   - `validate_hats` 中集成 payload 契约校验输出
   - `HatsCommands::Validate` 新增 `--strict` 标志，启用 schema 完备性强制检查

5. **`crates/ralph-cli/src/loop_runner.rs`**（或 `event_loop/mod.rs`）
   - 在 loop 启动前调用编排期 payload 契约校验（Hard Gate）
   - 运行时 `event_policy` 校验失败时触发 Loop Pause，写入诊断报告

6. **`crates/ralph-cli/src/doctor.rs`**（可选）
   - `ralph doctor` 输出中增加 payload schema 完备性检查项

### 新增文件

7. **`presets/ce-executor/schemas.yml`**（示例）
   - 为 `ce-executor.yml` 定义所有事件的 schema，作为第一个采用新机制的 preset

### 测试策略

- **单元测试**：
  - `preset_validator.rs`：上游提供字段 vs 下游依赖字段 —— 匹配时通过，缺失时报错
  - `preset_validator.rs`：instructions 字段提取 —— 测试各种文本模式（反引号、payload 关键字等）
  - `config.rs`：外部 schema 文件加载 —— 文件存在/不存在/路径解析
- **集成测试**：
  - 构造一个故意缺少 payload 字段的 preset，验证 `ralph run` 启动前被拦截
  - 构造运行时 payload 字段缺失场景，验证 loop 暂停和诊断报告生成
- **冒烟测试**：
  - `ce-executor.yml` 补全 schema 后，`ralph hats validate --strict` 必须通过
  - 现有未配置 schema 的 preset，`ralph run` 行为不受影响（向后兼容）

---

## 附录：Schema 文件示例

```yaml
# presets/ce-executor/schemas.yml
schemas:
  work.start:
    payload: json_object
    required_fields: ["plan_file"]

  work.ready:
    payload: json_object
    required_fields: ["plan_name", "complexity", "steps", "task_id", "task_key"]

  review.wave.ready:
    payload: json_object
    required_fields:
      - "dimension"
      - "focus"
      - "depth"
      - "diff_base"
      - "intent_summary"
      - "changed_files"
      - "plan_name"
      - "task_id"
      - "task_key"
      - "step"

  review.dimension.done:
    payload: json_object
    required_fields:
      - "dimension"
      - "findings_count"
      - "findings_file"
      - "plan_name"
      - "task_id"
      - "task_key"
      - "step"
      - "p0_count"
      - "p1_count"
      - "p2_count"
      - "p3_count"
      - "safe_auto_count"
      - "gated_auto_count"
      - "manual_count"
      - "advisory_count"

  LOOP_COMPLETE:
    payload: json_object
    required_fields: []
```
