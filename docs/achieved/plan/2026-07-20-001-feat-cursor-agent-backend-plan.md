---
date: 2026-07-20
topic: cursor-agent-backend
title: Cursor Agent Backend（`agent`）开发计划
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: docs/brainstorms/2026-07-20-cursor-agent-backend-requirements.md
execution: code
---

# Cursor Agent Backend（`agent`）开发计划

> 面向 Coding Agent 的可执行计划。**禁止在规划阶段编写生产代码。**  
> 产品契约来源：`docs/brainstorms/2026-07-20-cursor-agent-backend-requirements.md`  
> 外部契约参考：[Cursor Headless CLI](https://cursor.com/docs/cli/headless)、[Output format](https://cursor.com/docs/cli/reference/output-format)、[Parameters](https://cursor.com/docs/cli/reference/parameters)

---

## 1. 功能目标

### 业务目标

在 Ralph 中新增一等公民 CLI backend **`agent`**，对接 Cursor Headless CLI（二进制 `agent`），提供：

- 可显式选择（`-b agent` / `--backend agent`）的无人值守执行路径；
- 固定 headless 落盘契约（`-p --force --trust --output-format stream-json`）；
- 可观测性：助手文本 + 工具开始/完成事件映射到现有 `StreamHandler` 语义；
- `auto` 可发现但不抢占现有更高优先级后端。

### 本次范围（v1）

| ID | 要求摘要 |
|---|---|
| R1 | backend 名 `agent` ↔ 二进制 `agent` |
| R2 | 显式 `-b agent` / `--backend agent`（或等价配置）可用 |
| R3 | `auto` 识别 PATH 上 `agent`，优先级低于现有后端 |
| R3a | 进入 `VALID_BACKENDS` + zsh `_RALPH_BACKENDS` |
| R4 | 默认 `-p` / `--print` |
| R5 | **固定**附带 `--force` + `--trust`（工厂写死，不可用 backend args 关掉） |
| R6 | 默认 `--output-format stream-json` |
| R7 | 解析 Cursor NDJSON：助手文本 + tool start/complete → stream 回调 |
| R8 | 非零退出走现有失败路径，保留/透传 stderr |
| R9–R10 | 信任本机 `agent login`；缺 `CURSOR_API_KEY` 不硬失败预检 |

### 非目标（v1 明确不做）

- `agent acp` / ACP
- session resume / `--continue`
- Cursor 自带 `--worktree`
- Ralph model → `--model` 透传
- `agent_interactive()` 交互工厂（`ralph plan` 交互路径）
- `CURSOR_API_KEY` 硬门禁 / 鉴权流程发明
- Sandbox / MCP approve 一等配置面
- PATH 二次校验（version/fingerprint）
- 默认追加 `--stream-partial-output`（字符级增量）—— v1 接受**工具间消息级**增量

### 已知约束与假设

**已核实的仓库事实（规划时核对，实现前仍需再读源码）：**

- 后端不是独立 trait，而是 `CliBackend` 工厂 + `OutputFormat` 分发（`crates/ralph-adapters/src/cli_backend.rs`）。
- 现有 stream 格式：`Text` | `StreamJson` | `PiStreamJson` | `TraeStreamJson`。
- **实时**工具回调：`PtyExecutor::run_observe_streaming`（TUI/RPC/interactive）。
- **headless**（`--no-tui`）走 `CliExecutor`：对 Claude/Pi **不** live 分发；`TraeStreamJson` 仅抽文本；完成后 `normalize_cli_output_for_parsing`（`crates/ralph-cli/src/loop_runner/output_parsing.rs`）抽文本供完成检测。
- 白名单：`crates/ralph-cli/src/backend_support.rs` → `VALID_BACKENDS`。
- 自动检测优先级：`crates/ralph-adapters/src/auto_detect.rs` → `DEFAULT_PRIORITY`（当前末位 `traecli`）。
- CLI 旗标：`-b` / `--backend`（`commands/run.rs`）。
- zsh：`scripts/ralph-zsh-plugin.zsh` → `_RALPH_BACKENDS`。
- doctor：`auth_env_vars` 对未知 backend 返回 `None`（不查 key）；`agent` 应保持 `None`，满足 R9。

**规划期锁定的技术决策（原 brainstorm Deferred）：**

| 议题 | 决策 | 理由 |
|---|---|---|
| 新 `OutputFormat` | 新增 `AgentStreamJson`，**禁止**复用 Claude/Pi/Trae 解析器 | Cursor 事件 schema 不同（`tool_call.started/completed` 等） |
| stream 解析模块 | 新增 `crates/ralph-adapters/src/agent_stream.rs`（对齐 `trae_stream.rs` / `pi_stream.rs` 模式） | 先例清晰 |
| `--stream-partial-output` | **不**默认追加 | v1 成功标准只需消息级文本 + 工具事件；减少重复 flush 过滤复杂度 |
| 工具可见性验收路径 | **PtyExecutor** 为 R7 主验收路径；headless 必须至少完成**文本抽取**（对齐 Claude/Pi） | 与现有架构一致，避免 Unit 内扩张 CliExecutor 全量 StreamHandler |
| `auto` 插入位置 | `DEFAULT_PRIORITY` **末尾**（`traecli` 之后） | R3「靠后」 |
| PromptMode | `PromptMode::Arg`，`prompt_flag: None`（位置参数传 prompt），对齐 `traecli()` | Cursor 文档示例为 `agent -p ... "prompt"` |
| 交互工厂 | **不**实现 `agent_interactive`；`for_interactive_prompt("agent")` 返回 `Err` | 非目标 |
| doctor 鉴权 | `auth_env_vars("agent")` → `None` | R9：不因缺 API key 硬提示为失败 |
| 大 prompt | 沿用现有 `build_command_pty` 大 prompt temp-file 间接机制；若 Cursor 对 `-p`+大 prompt 卡住，记入残余风险，不在 v1 另造通道 | 既有 #280 路径 |

---

## 2. BDD 行为规格

```gherkin
Feature: Cursor Agent backend (`agent`)
  作为 Ralph 用户
  我想用 Cursor Headless CLI 作为可观测的 CLI backend
  以便在已 login 的本机上无人值守跑 hat/loop，并看到文本与工具事件

  # --- 正常流程 ---

  Scenario: S1 显式选择 agent backend
    Given Ralph 已识别合法 backend 名集合
    When 用户以 `-b agent`（或 `--backend agent`，或等价配置）选择 backend
    Then 解析得到命令为 `agent`
    And 默认参数固定包含 `-p`（或 `--print`）、`--force`、`--trust`
    And 默认参数包含 `--output-format stream-json`
    And 输出格式为 `AgentStreamJson`（或等价枚举变体）

  Scenario: S2 解析助手文本事件
    Given 一段 Cursor `stream-json` NDJSON，含 `type=assistant` 且带文本 content
    When 解析器处理该行
    Then 触发文本可见回调（`on_text`），内容为该段助手文本
    And 不因存在未知附加字段而失败

  Scenario: S3 解析工具开始与完成事件
    Given NDJSON 含 `type=tool_call` 且 `subtype=started`（例如 `readToolCall`）
    And 随后含同 `call_id` 的 `subtype=completed`
    When 解析器依次处理这些行
    Then 工具开始映射为 `on_tool_call`
    And 工具完成映射为 `on_tool_result`（或现有成对语义）
    And 至少覆盖一类工具（read 或 write）即可满足 v1

  Scenario: S4 Pty 流式观测路径消费 AgentStreamJson
    Given backend 输出格式为 `AgentStreamJson`
    And 执行走 `PtyExecutor::run_observe_streaming`
    When 进程 stdout 逐行输出合法 Cursor NDJSON
    Then StreamHandler 收到对应文本与工具回调
    And `type=system` / 未知 `type` 被忽略且不中断流

  Scenario: S5 headless 文本抽取供完成检测
    Given 完整一次 run 的 `stream-json` 多行输出（含 assistant 与终态 `result`）
    When `normalize_cli_output_for_parsing`（或 CliExecutor 等价抽取）处理该输出
    Then 抽出的纯文本包含助手可见内容（非原始 NDJSON 整包）
    And 可用于现有完成检测/日志路径

  Scenario: S6 auto 不抢占更高优先级后端
    Given PATH 上同时可检测到 `claude` 与 `agent`
    When backend 选择为 `auto`
    Then 选中更高优先级后端（现有列表中先于 `agent` 者，如 `claude`）
    And 不选中 `agent`

  Scenario: S7 仅 agent 可用时 auto 选中 agent
    Given PATH 上仅 `agent` 在当前 `DEFAULT_PRIORITY` 检测中成功
    And 更高优先级命令均不可用
    When backend 选择为 `auto`
    Then 选中 `agent`

  # --- 非法输入 / 边界 ---

  Scenario: S8 畸形或未知 NDJSON 行 fail-soft
    Given 某行不是合法 JSON，或 `type` 未知，或缺关键字段
    When 解析器处理该行
    Then 不 panic
    And 不产生虚假工具回调
    And 流可继续处理后续合法行

  Scenario: S9 非法 backend 名仍被拒绝
    Given 用户传入未知 backend 名（非 `agent` 亦非现有合法名）
    When 调用 `CliBackend::from_name`
    Then 返回错误（与现有 `CustomBackendError` 行为一致）
    And 不静默落到 `claude`（`from_name` 路径）

  Scenario: S10 agent 不提供 interactive 工厂
    Given 调用方请求 interactive prompt 工厂且名为 `agent`
    When 调用 `CliBackend::for_interactive_prompt("agent")`
    Then 返回错误
    And 不产生去掉 `--force/--trust` 的「伪交互」配置

  # --- 权限 / 状态 / 失败 ---

  Scenario: S11 缺少 CURSOR_API_KEY 不构成 Ralph 硬失败预检
    Given 环境未设置 `CURSOR_API_KEY`
    And backend 为 `agent`
    When Ralph 做 doctor/启动侧 backend 鉴权 env 检查
    Then 不因「缺少 CURSOR_API_KEY」将 `agent` 判为硬失败
    And 实际鉴权成败交给 Cursor CLI 运行时（本机 login 或 CLI 自身报错）

  Scenario: S12 进程非零退出走现有失败路径
    Given `agent` 进程以非零码退出
    When executor 结束本次调用
    Then 失败按现有 backend 失败语义上报
    And stderr 诊断信息按现有路径尽可能保留/可见

  Scenario: S13 force/trust 工厂固定不可关
    Given `CliBackend::agent()` 工厂产物
    When 检查默认 `args`
    Then 含 `--force` 与 `--trust`
    And v1 不提供「工厂级关闭」API；覆盖关闭不在范围内
    # 注：若上层 `NamedWithArgs` 追加额外 args，不得被实现成「删除」force/trust；
    # 工厂自身始终带上这两项。
```

---

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
|---|---|---|---|
| S1 | `from_name("agent")` / `from_config` 得到 cmd=`agent`，args 含 `-p`、`--force`、`--trust`、`--output-format stream-json`，format=`AgentStreamJson`；`VALID_BACKENDS` 含 `agent` | 单元（`cli_backend` + `backend_support`） | 否 |
| S2 | fixture 行 → `on_text` 含期望片段 | 单元（`agent_stream`） | 否 |
| S3 | started/completed → `on_tool_call` / `on_tool_result` | 单元（`agent_stream`） | 否 |
| S4 | PTY observe 路径对 `AgentStreamJson` 分发到 handler | 集成（`pty_executor_integration` 风格，可用假 stdout 行） | 否 |
| S5 | `normalize_cli_output_for_parsing` / extract 得到纯文本 | 单元（`output_parsing`）+ 可选 CliExecutor 单测 | 否 |
| S6 | priority 中 `agent` 在 `claude` 之后；同在 PATH 时选 `claude` | 单元（`auto_detect`） | 否 |
| S7 | 仅 agent 可检测时返回 `agent` | 单元（`auto_detect`，stub 检测） | 否 |
| S8 | 坏行不 panic、不假回调 | 单元（`agent_stream`）+ 建议少量 property/fuzz 式乱行 | 否 |
| S9 | 未知名 `from_name` Err | 单元 | 否 |
| S10 | `for_interactive_prompt("agent")` Err | 单元 | 否 |
| S11 | `auth_env_vars("agent")` 为 `None`（或不要求 CURSOR_API_KEY） | 单元（`doctor`） | 否 |
| S12 | 非零退出失败语义不变 | 复用现有 executor 失败用例 / 轻量表征；不新开 live E2E | 否 |
| S13 | 工厂 args 断言含 force/trust | 单元 | 否 |
| 回归 | 既有 backend（claude/pi/traecli/…）工厂与 stream 测试全绿 | 包级 nextest | 否 |
| 人工冒烟（可选） | 本机已 `agent login` 时 `-b agent` 跑一次真实 hat | 手动；**不**进 CI E2E 门禁 | 可选，非 CI |

**风险驱动补充（按需，非机械全上）：**

- **Parser**：S2/S3/S8 → 单元 + 对乱行做轻量 fuzz/随机行（可选，Unit 1 内）。
- **Characterization**：改 `PtyExecutor` match 前，若触及共享分支，先跑现有 `run_observe_streaming_pi_*` / claude 用例作表征。
- **Differential**：不要求；无替换旧实现。
- **Live E2E / 真 Cursor API**：默认 **不做**（依赖本机 login + 网络）；CI 用 fixture/replay。

**测试入口硬约束（仓库规则）：** 一律 `cargo nextest run ...`；禁止裸 `cargo test -p ralph-cli`。

---

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E |
|---|---|---|---|---|---|
| R1 | S1 | `from_name("agent")` cmd | `cli_backend` | — | — |
| R2 | S1 | allowlist + from_config | `backend_support` + `cli_backend` | — | — |
| R3 | S6, S7 | auto 优先级 | `auto_detect` | — | — |
| R3a | S1 | VALID_BACKENDS + zsh 数组含 `agent` | `backend_support` + `presets` zsh 同步测 | — | — |
| R4 | S1 | args 含 `-p` 或 `--print` | `cli_backend` | — | — |
| R5 / S13 | S1, S13 | args 含 `--force` `--trust` | `cli_backend` | — | — |
| R6 | S1 | `--output-format stream-json` + `AgentStreamJson` | `cli_backend` | — | — |
| R7 | S2–S4 | parser + PTY dispatch | `agent_stream` | `pty_executor` 集成 | — |
| R8 | S12 | 失败路径 | 复用 executor 测 | 可选轻量表征 | — |
| R9–R10 / S11 | S11 | doctor 不要求 API key | `doctor` | — | — |
| 边界 | S8–S10 | fail-soft / Err | `agent_stream` + `cli_backend` | — | — |
| headless 文本 | S5 | normalize/extract | `output_parsing` | 可选 `cli_executor` | — |

---

## 5. 严格串行开发单元

> **执行纪律**：Unit N 的实现、测试、重构、回归全部完成并满足完成标准后，才能开始 Unit N+1。禁止并行交替。  
> **TDD 闭环（每个 Unit 强制）**：① 写/启用验收测 → ② 确认以正确原因红 → ③ 拆最小单元测 Red→Green→Refactor → ④ 相关集成 → ⑤ 回归 → ⑥ 关单 → ⑦ 下一 Unit。  
> **禁止**：删断言、skip、`.only`、无解释改 golden、Mock 掉被测行为、只跑局部就宣称完成。

---

### Unit 1 — Cursor `stream-json` 解析器（纯逻辑）

- **Unit 目标**：在 adapters 内落地可独立验证的 Cursor NDJSON 解析与 `StreamHandler` 分发，不依赖真实 `agent` 进程。
- **对应 Scenario**：S2, S3, S8
- **外部可观察结果**：给定官方文档形状的 fixture 行，回调序列可断言；坏行 fail-soft。
- **输入 / 输出**：
  - 入：NDJSON 行字符串（assistant / tool_call started|completed / system / result / 垃圾行）
  - 出：对 test double `StreamHandler` 的调用序列；可选 session 摘要字段（若对齐 trae 的 result 处理，仅记录不阻塞）
- **可依赖**：现有 `StreamHandler` trait；`pi_stream.rs` / `trae_stream.rs` 模式（只读参考）
- **禁止依赖**：`CliBackend::agent()`、Pty/Cli 接线、`VALID_BACKENDS`、auto_detect（后续 Unit）
- **验收测试**：
  - `crates/ralph-adapters/src/agent_stream.rs` 内 `#[cfg(test)]`（或同 crate 测试模块）覆盖 S2/S3/S8
  - Fixture 来源：Cursor 文档示例事件形状（assistant content text、readToolCall/writeToolCall started/completed、system init、result success）
- **需拆分的单元测试**：
  - parse assistant → text
  - parse tool started/completed（至少 read 或 write 一类）
  - ignore system / unknown type
  - invalid JSON / missing fields 不 panic
- **Red 预期失败原因**：模块/`AgentStreamParser`/`dispatch_agent_stream_event` 尚不存在 → compile fail 或断言未实现
- **最小实现范围**：
  - 新增 `crates/ralph-adapters/src/agent_stream.rs`
  - `crates/ralph-adapters/src/lib.rs` 声明 `mod` + 必要 `pub use`
  - **本 Unit 可不改** `OutputFormat` 枚举（若编译需要，仅加枚举变体但**不**接线 executor——优先把解析 API 做成与 format 无关的纯函数；若必须先有枚举，变体可先加但 Pty match 留到 Unit 3，并用 `#[allow(dead_code)]` 仅当仓库惯例允许——更推荐：解析器完全独立，Unit 3 再加枚举）
- **集成验证**：本 Unit 无跨 crate 集成；跑 `cargo nextest run -p ralph-adapters -- agent_stream`
- **回归范围**：`cargo nextest run -p ralph-adapters -- pi_stream trae_stream claude_stream`（确认未误改共享类型）
- **完成标准**：S2/S3/S8 全绿；解析未知字段忽略；无 panic
- **风险与注意事项**：
  - 若启用 `--stream-partial-output` 的重复 flush 规则：v1 **不**发该旗标，解析器可忽略 `timestamp_ms`/`model_call_id` 去重逻辑，或实现为「无 timestamp 过滤时全量 assistant 文本都转发」（与文档「完整 message 段」一致）
  - tool 形态异构（read/write/function）：v1 最少映射一类 + 通用 fallback 名称字符串即可

---

### Unit 2 — `CliBackend::agent()` 工厂 + 注册表面

- **Unit 目标**：`agent` 成为可解析、可白名单的 backend；工厂 args 满足 R4–R6/R5 固定契约。
- **对应 Scenario**：S1, S9, S10, S13
- **外部可观察结果**：`from_name("agent")` / `from_config(agent)` 成功；interactive 与非法名行为符合规格；`VALID_BACKENDS` 含 `agent`。
- **输入 / 输出**：
  - 入：backend 名 / `CliConfig { backend: "agent", ... }`
  - 出：`CliBackend { command: "agent", args: [...], prompt_mode: Arg, output_format: AgentStreamJson, ... }`
- **可依赖**：Unit 1 解析器（可选尚未被工厂引用）；现有 `CliBackend` / `OutputFormat` 扩展点
- **禁止依赖**：Pty/Cli 分发接线、auto_detect 优先级、zsh（可在本 Unit 或 Unit 5；**本 Unit 必须完成 VALID_BACKENDS**，zsh 若测试已绑定可一并改，见下）
- **验收测试**：
  - `cli_backend.rs`：`test_agent_backend`（args/format/command）、`test_from_name_agent`、`test_from_config_agent`、`test_for_interactive_prompt_agent_errors`、`test_agent_args_include_force_and_trust`
  - `backend_support.rs`：合法集合含 `agent`（跟现有断言风格）
- **需拆分的单元测试**：上表各断言独立失败信息清晰
- **Red 预期失败原因**：`from_name("agent")` → `Err`；或 `VALID_BACKENDS` 断言失败
- **最小实现范围**：
  - `OutputFormat::AgentStreamJson` 变体
  - `CliBackend::agent()`：args **固定**为至少：`-p`（或 `--print`）、`--force`、`--trust`、`--output-format`、`stream-json`（具体短/长选项与 Cursor CLI 实测一致；实现前用 `agent -h` 核对，但默认按文档）
  - `from_config` / `from_name` match 臂加入 `"agent"`
  - **不**加 `agent_interactive`；`for_interactive_prompt` **不**匹配 `agent`（走现有 invalid → Err）
  - `crates/ralph-cli/src/backend_support.rs`：`VALID_BACKENDS` + `VALID_BACKENDS_LABEL`
  - 若 `OutputFormat` 穷尽 match 导致编译失败：仅加 `todo!`/`unreachable` **禁止**；必须在本 Unit 把编译期穷尽处补上 **显式分支**——对尚未接线的 executor 分支可暂时 `Text` 回退**仅当**该路径本 Unit 有测试锁住「未接线」？**更好**：本 Unit 完成后立刻进入 Unit 3 前，若无法编译，把 Pty/Cli/normalize 的 match 臂加为空实现并标 `unimplemented` **禁止**。正确做法：本 Unit 同步在所有 `match output_format` 处加 `AgentStreamJson =>` 行为与「未支持」明确错误或与 Text 相同的临时行为，并在 Unit 3/4 替换为真解析——**但**临时 Text 行为会弱化验收。  
  **规定**：Unit 2 结束时工作区必须 `cargo check -p ralph-adapters -p ralph-cli` 通过；所有 `match OutputFormat` 对 `AgentStreamJson` 有**显式臂**。若 Unit 3 前尚未解析，臂内行为定为：Pty 路径先 `on_text(raw_line)` 或跳过（二选一写进实现注释），并在 Unit 2 用测试锁住「工厂已选 AgentStreamJson」；**完整回调语义的验收属于 Unit 3，不得在 Unit 2 宣称 S4 完成。**
- **集成验证**：`cargo nextest run -p ralph-adapters -- agent`；`cargo nextest run -p ralph-cli --bin ralph -- backend_support`（或现有校验合法 backend 的测试名）
- **回归范围**：既有 `test_*_backend` / `from_name_*` 全绿
- **完成标准**：S1/S9/S10/S13 绿；force/trust 在工厂 args 中；interactive Err
- **风险与注意事项**：`from_config` 对未知名静默 fallback `claude()` 是既有行为——**不要**改；仅保证 `"agent"` 被识别。`NamedWithArgs` 追加 args 不得用于「删除」force/trust（无删除语义即可）

---

### Unit 3 — PtyExecutor 实时分发（R7 主路径）

- **Unit 目标**：`AgentStreamJson` 在 `PtyExecutor::run_observe_streaming` 中走 Unit 1 解析器，产生与 Claude/Pi/Trae 同级的文本/工具回调。
- **对应 Scenario**：S4（及复用 S2/S3 行为于 executor 边界）
- **外部可观察结果**：注入/模拟的 NDJSON 行序列触发 test handler 的 `on_text` / `on_tool_call` / `on_tool_result`
- **输入 / 输出**：stdout 行流 → StreamHandler 回调
- **可依赖**：Unit 1、Unit 2（`AgentStreamJson`）
- **禁止依赖**：auto_detect、doctor、zsh、真实 Cursor 网络调用
- **验收测试**：
  - 优先扩展 `crates/ralph-adapters/tests/pty_executor_integration.rs`（参考 `run_observe_streaming_pi_*`）
  - 或 `pty_executor.rs` 内可测的 dispatch 单测（若现有 pi/trae 有更轻量模式则对齐）
- **需拆分的单元测试**：dispatch 包装函数（若抽出 `handle_agent_stream_line`）的行级测
- **Red 预期失败原因**：match 臂仍把 AgentStreamJson 当 Text，工具回调缺失
- **最小实现范围**：
  - `pty_executor.rs`：`is_agent_stream` 分支 → `AgentStreamParser` + `dispatch_agent_stream_event`
  - 不改调度策略 / 不改 TUI 框架
- **集成验证**：`cargo nextest run -p ralph-adapters -- pty_executor`
- **回归范围**：现有 `run_observe_streaming_pi_*`、claude/trae 相关 PTY 测
- **完成标准**：S4 绿；system/unknown 不中断；至少一类工具回调可观测
- **风险与注意事项**：改 PTY 共享循环前先跑表征用例；保持 fail-soft

---

### Unit 4 — Headless 文本归一化（S5）

- **Unit 目标**：`--no-tui` / CliExecutor 路径下，AgentStreamJson 输出可被抽成纯文本，避免完成检测吃到原始 NDJSON。
- **对应 Scenario**：S5
- **外部可观察结果**：多行 fixture → `extract_agent_stream_text` / `normalize_cli_output_for_parsing` 返回聚合助手文本
- **输入 / 输出**：原始 stdout 字符串 → 纯文本
- **可依赖**：Unit 1–3
- **禁止依赖**：auto_detect、真实 agent 二进制
- **验收测试**：
  - `crates/ralph-cli/src/loop_runner/output_parsing.rs`（或其所在测试模块）新增 extract 测
  - 可选：`cli_executor.rs` 中对齐 `test_execute_trae_stream_*` 的抽文本行为（若 Agent 路径需要与 Trae 对称的 live extract）
- **需拆分的单元测试**：仅 result/assistant 聚合；忽略 tool 行；坏行跳过
- **Red 预期失败原因**：`normalize_cli_output_for_parsing` match 未覆盖 `AgentStreamJson` → 非穷尽编译失败或返回 raw
- **最小实现范围**：
  - `output_parsing.rs`：`AgentStreamJson => extract_agent_stream_text`
  - 复用 Unit 1 的 extract API（在 adapters 导出，cli 依赖调用）——**保持与 `extract_trae_stream_text` 相同的跨 crate 模式**
  - 若 `CliExecutor` 对 Trae 有特殊 live extract：为 Agent 做**同等最小**对称，避免 headless 行为差于 traecli
- **集成验证**：`cargo nextest run -p ralph-cli --bin ralph -- output_parsing`（或精确测试名）；`cargo nextest run -p ralph-adapters -- cli_executor`
- **回归范围**：claude/pi/trae extract 测试
- **完成标准**：S5 绿；raw NDJSON 不会整包作为「助手最终文本」
- **风险与注意事项**：不要在本 Unit 把 CliExecutor 做成完整 StreamHandler 分发（超出 v1 决策）

---

### Unit 5 — auto 发现 + doctor + zsh 补全（R3 / R3a / R9）

- **Unit 目标**：`auto` 低优先级发现 `agent`；doctor 不因缺 API key 硬失败；zsh 补全与白名单一致。
- **对应 Scenario**：S6, S7, S11
- **外部可观察结果**：优先级表末位含 `agent`；检测命令名为 `agent`；zsh 数组含 `agent`；doctor auth 不要求 `CURSOR_API_KEY`
- **输入 / 输出**：PATH 检测 stub / backend 名 → 选中名；doctor 检查结果
- **可依赖**：Unit 2（`from_name("agent")` 可用）
- **禁止依赖**：未完成的「二次校验」、live Cursor
- **验收测试**：
  - `auto_detect.rs`：`DEFAULT_PRIORITY` 含 `agent` 且位于 `traecli` 之后；S6/S7 用现有检测 stub 模式
  - `doctor`：`auth_env_vars("agent")` 为 `None`（或不把 CURSOR_API_KEY 列为必查）
  - `scripts/ralph-zsh-plugin.zsh` + `presets.rs` 中 `test_zsh_plugin_backend_array_*` 同步
- **需拆分的单元测试**：priority 顺序；detection_command 默认同名；zsh 数组成员
- **Red 预期失败原因**：priority 断言失败；zsh 测试失败
- **最小实现范围**：
  - `auto_detect.rs`：`DEFAULT_PRIORITY` push `agent`（末尾）
  - `doctor.rs`：`canonical_backend_name` 若有 basename 白名单则加 `agent`；**不要**把 `CURSOR_API_KEY` 加进 `auth_env_vars`
  - `scripts/ralph-zsh-plugin.zsh`：`_RALPH_BACKENDS` 增加 `agent`
  - 按仓库惯例：改 zsh 后执行 `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`（若环境存在；写入计划备注，由执行者处理）
- **集成验证**：`cargo nextest run -p ralph-adapters -- auto_detect`；`cargo nextest run -p ralph-cli --bin ralph -- doctor zsh backend`
- **回归范围**：其它 backend 的 auto/doctor/zsh 测
- **完成标准**：S6/S7/S11 绿；R3a 满足
- **风险与注意事项**：PATH 同名误检为已接受残余风险；不做 fingerprint

---

### Unit 6 — 文档同步 + 全量回归门禁

- **Unit 目标**：文档/规则与代码表面一致；全量相关回归通过；明确残余风险。
- **对应 Scenario**：全部 S1–S13 的最终确认；无新 Scenario
- **外部可观察结果**：文档不再声称「无 Cursor backend」；测试基线绿
- **输入 / 输出**：文档 diff；测试报告
- **可依赖**：Unit 1–5 全部完成
- **禁止依赖**：新功能范围外的重构
- **验收测试**：无新功能测；跑门禁命令
- **需拆分的单元测试**：无（除非发现文档引用的命令漂移）
- **Red 预期失败原因**：若先写「文档宣称已支持」而代码未合——本 Unit 只在代码完成后改文档，避免红文档
- **最小实现范围**：
  - 若 `.cursor/rules/architecture-modules.mdc` / `CLAUDE.md`/`AGENTS.md` crate map 仍列过时 backend：仅在本任务触及的「当前支持列表」处补 `agent`（保持 `cp CLAUDE.md AGENTS.md` 同步）
  - **不要**把实现细节写进 `crates/ralph-core/data/ralph-tools*.md`，除非确认 agent prompt 需要知道 `-b agent`（通常 backend 选择是 operator 侧；若 `ralph-tools` 无 backend 列表则跳过）
  - 更新 requirements 的 Next Steps 指向本计划（可选）
- **集成验证**：见第 6 节门禁
- **回归范围**：见第 6 节
- **完成标准**：门禁全过；残余风险写入计划附录/PR 描述
- **风险与注意事项**：禁止用 live Cursor 作为 CI 门禁；人工冒烟可选

---

## 6. 最终质量门禁

执行者在宣称完成前必须满足：

1. **计划内 Scenario S1–S13** 均有对应自动化测试且通过（S12 允许以既有失败路径表征 + 工厂/executor 既有断言覆盖）。
2. **单元测试**：`agent_stream` / `cli_backend` agent 工厂 / `auto_detect` / `output_parsing` / `doctor` auth 相关断言通过。
3. **集成测试**：`pty_executor` AgentStreamJson 分发相关用例通过。
4. **E2E**：v1 **无**强制 CI E2E；可选本机手动：`ralph run -b agent ...`（需已 `agent login`）。
5. **Lint / 构建**：
   - `cargo check -p ralph-adapters -p ralph-cli`
   - `cargo clippy -p ralph-adapters -p ralph-cli -- -D warnings`（或仓库惯用 clippy 入口）
   - `cargo fmt --check`（若 hook 要求）
6. **测试命令（最低）**：
   ```bash
   cargo nextest run -p ralph-adapters -- agent_stream agent cli_backend pty_executor auto_detect
   cargo nextest run -p ralph-cli --bin ralph -- backend_support output_parsing doctor zsh
   ```
   准备合并前再跑：
   ```bash
   ./scripts/run-tests.sh
   ```
7. **无新增** `#[ignore]` / 跳过 / 削弱断言。
8. **未验证 / 残余风险（必须在完成说明中显式列出）**：
   - 未在 CI 对真实 Cursor API / 本机 login 做强制验收
   - 未默认 `--stream-partial-output`（仅消息级增量）
   - PATH 同名非 Cursor `agent` 可能误检
   - 固定 `--force`+`--trust` 的写盘爆炸半径（依赖 worktree/用户环境）
   - 大 prompt + `-p` 是否在极端体积下卡住：若未实测，保持风险开放
   - headless 路径无实时工具回调（与 Claude/Pi 现状一致）

---

## Appendix A — 建议触及的文件清单（实现时再确认）

| 区域 | 路径 |
|---|---|
| 解析器 | `crates/ralph-adapters/src/agent_stream.rs`（新） |
| 工厂 | `crates/ralph-adapters/src/cli_backend.rs` |
| 导出 | `crates/ralph-adapters/src/lib.rs` |
| PTY | `crates/ralph-adapters/src/pty_executor.rs` |
| 自动检测 | `crates/ralph-adapters/src/auto_detect.rs` |
| 可选 Cli 抽文本 | `crates/ralph-adapters/src/cli_executor.rs` |
| 白名单 | `crates/ralph-cli/src/backend_support.rs` |
| 归一化 | `crates/ralph-cli/src/loop_runner/output_parsing.rs` |
| Doctor | `crates/ralph-cli/src/doctor.rs` |
| zsh | `scripts/ralph-zsh-plugin.zsh` |
| PTY 集成测 | `crates/ralph-adapters/tests/pty_executor_integration.rs` |
| zsh 同步测 | `crates/ralph-cli/src/presets.rs`（既有 test） |

**不要求**新建 `presets/minimal/agent.yml`（现有亦无 pi/traecli minimal preset）。

---

## Appendix B — Outside-In 依赖方向（示意）

```text
[Operator] -b agent / auto
    → VALID_BACKENDS + CliBackend::agent()          (Unit 2, 5)
    → PtyExecutor + AgentStreamParser               (Unit 1 → 3)
    → normalize/extract for headless completion     (Unit 4)
    → Cursor CLI process (force/trust/print/stream) (工厂契约)
```

---

## Appendix C — 与产品需求文档的关系

- 需求：`docs/brainstorms/2026-07-20-cursor-agent-backend-requirements.md`
- 本计划将 brainstorm 中 Deferred 项全部落成**非阻塞技术决策**（见 §1 表格）；执行期若实测推翻（例如 `-p` 大 prompt 必挂），应开 follow-up，而不是静默扩大 v1 范围（ACP/model/resume 等仍禁止塞入）。
