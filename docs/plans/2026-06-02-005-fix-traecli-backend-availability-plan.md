---
title: 修复 traecli backend 不可用问题
type: fix
status: active
date: 2026-06-02
---

# 修复 traecli backend 不可用问题

## Overview

ralph-orchestrator 使用 `traecli` 作为 backend 时完全不可用。根因已定位到两个 ralph 端问题：

1. `traecli()` 未传递 `--output-format stream-json`，导致 `trae-cli --yolo --print` stdout 为空、exit code = 1
2. ralph 缺少 trae-cli 的 NDJSON stream parser，即使手动传入 stream-json 参数，输出也被当作纯文本处理

本计划修复这两个问题，使 `ralph run --backend traecli` 能够正常调用 trae-cli 并解析其流式输出。

---

## Problem Frame

用户配置了 `cli.backend: traecli` 后，`ralph run` 无法产生任何有效输出，迭代直接失败。诊断显示：

- `trae-cli --yolo --print "prompt"`（ralph 当前生成的命令）→ exit 1，stdout 为空
- `trae-cli --yolo --print --output-format stream-json "prompt"` → exit 0/1，stdout 有 NDJSON 流
- ralph 的 `OutputFormat::Text` 分支没有 NDJSON 解析能力

trae-cli 服务端存在额外的 API 400 错误（`temperature is deprecated for this model`），但这属于 trae-cli 自身问题，不在本修复范围内。

---

## Requirements Trace

- R1. `traecli()` backend 必须生成包含 `--output-format stream-json` 的命令行参数
- R2. ralph 必须能够解析 trae-cli 的 NDJSON 流式输出，提取 assistant 文本、tool call、tool result 和 session result
- R3. 修复后的 traecli backend 在 `/home/chaowen/Dev/agent_tools/trae_test` 测试环境中可验证 stdout 有输出且 parser 工作正常
- R4. 所有变更必须通过 `cargo test -p ralph-adapters` 单元测试
- R5. 不破坏其他 backend（claude, pi, copilot 等）的现有行为

---

## Scope Boundaries

- **非目标**：修复 trae-cli 服务端的 `temperature is deprecated for this model` 400 错误（trae-cli 自身问题）
- **非目标**：为 trae-cli 添加 PTY/TUI 模式支持（超出当前修复范围）
- **非目标**：修改 trae-cli 的自动检测优先级（已在 `DEFAULT_PRIORITY` 末尾）

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-adapters/src/cli_backend.rs` — `traecli()` 定义（第 630-639 行），当前 `args` 只有 `["--yolo", "--print"]`，`output_format` 为 `Text`
- `crates/ralph-adapters/src/cli_executor.rs` — executor 主循环，stdout 处理分支（第 191-210 行）。`CopilotStreamJson` 有专门的 `extract_text` 处理，其他 stream format 需要类似集成
- `crates/ralph-adapters/src/claude_stream.rs` — Claude NDJSON parser 参考实现（`ClaudeStreamEvent` enum + `ClaudeStreamParser::parse_line`）
- `crates/ralph-adapters/src/pi_stream.rs` — Pi NDJSON parser + `dispatch_pi_stream_event` 参考模式（更完整的 dispatch 函数 + `PiSessionState`）
- `crates/ralph-adapters/src/copilot_stream.rs` — Copilot parser + `dispatch_copilot_stream_event` 参考
- `crates/ralph-adapters/src/stream_handler.rs` — `StreamHandler` trait（`on_text`, `on_tool_call`, `on_tool_result`, `on_error`, `on_complete`）
- `crates/ralph-adapters/src/lib.rs` — 模块注册和 re-export
- `scripts/ralph-zsh-plugin.zsh` — zsh completion，需要确认 `traecli` 是否已在 builtin 补全列表中

### Institutional Learnings

- 从 `pi_stream.rs` 的模式可知：NDJSON parser 采用 "only events Ralph needs are modeled, all others captured by `#[serde(other)]`" 策略，提供前向兼容性
- `cli_backend.rs` 的测试模式：每个 backend 都有 `test_<backend>_backend()` 验证 args、stdin、output_format
- `cli_executor.rs` 的集成测试模式：使用 `printf` 模拟 NDJSON 输出，验证 parser 正确提取文本

### External References

- trae-cli `--help` 确认支持 `--output-format stream-json`（values: `text (default), json, or stream-json`）
- 测试目录 `/home/chaowen/Dev/agent_tools/trae_test` 包含可直接运行的验证脚本 `test-direct.sh` 和 `test-ralph.sh`

---

## Key Technical Decisions

- **Parser 设计策略**：参考 `pi_stream.rs` 的保守策略 —— 只建模 Ralph 需要的事件类型（text delta、tool call、tool result、session result），未知类型用 `#[serde(other)]` 静默忽略。trae-cli 的 NDJSON 格式与 Claude 类似（顶层有 `type` 字段），但具体字段结构需在实现时通过实际运行样本确认。
- **OutputFormat 扩展**：新增 `OutputFormat::TraeStreamJson` 枚举值，与 `StreamJson`（Claude）、`PiStreamJson`（Pi）、`CopilotStreamJson`（Copilot）保持一致的命名风格。新增 variant 必须放在 enum 末尾或已有 stream json variants 之后，避免影响已有 match 分支的编译检查。
- **Dispatch 模式**：采用 `pi_stream.rs` 的 `dispatch_trae_stream_event` 模式，将解析后的事件映射到 `StreamHandler` trait 方法，维护 `TraeSessionState` 以累积 session 元数据。
- **Prompt 传递方式**：保持 trae-cli 的 Arg 模式 + positional prompt 不变（与当前一致），不改为 stdin 模式。
- **零回归原则**：所有变更必须遵循"只增不改"原则——新增 enum variant、新增 match 分支、新增文件，不修改任何现有 backend 的 args / prompt_mode / output_format / 过滤逻辑。executor 中对 `Text` / `StreamJson` / `CopilotStreamJson` / `PiStreamJson` / `Acp` 的处理路径完全保持原样。

---

## Open Questions

### Resolved During Planning

- **trae-cli 是否支持 `--output-format stream-json`**：已确认支持。`trae-cli --help` 明确列出 `--output-format string` 的合法值为 `text (default), json, or stream-json`。Test 3 也验证了其 stdout 输出 NDJSON 流。

### Deferred to Implementation

- **trae-cli NDJSON 的完整字段结构**：从诊断样本中仅看到 `{"type":"system","subtype":"init",...}` 和 `{"type":"result","subtype":"error_during_execution",...}` 的片段。`assistant` 事件的 content block 结构（text/tool_use）是否与 Claude 完全一致，需在实现时通过实际运行 `trae-cli --output-format stream-json` 捕获完整样本来确认。这是 parser 设计的必要输入，但不阻塞计划。
- **`--include-partial-messages` 是否需要**：trae-cli help 提到该 flag 与 stream-json 相关。实现时需测试加与不加的差异，决定是否默认加入 args。

---

## Output Structure

无需新增目录结构，所有变更在现有 `crates/ralph-adapters/src/` 目录下。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
┌─────────────────────────────────────────────────────────────┐
│  trae-cli --yolo --print --output-format stream-json        │
│           "<prompt>"                                         │
│                    ↓ NDJSON lines                            │
│  {"type":"system",...}                                      │
│  {"type":"assistant",...}                                   │
│  {"type":"result",...}                                      │
│                    ↓                                         │
│  ┌─────────────────┐    ┌──────────────────┐               │
│  │ TraeStreamParser│ →  │TraeStreamEvent   │               │
│  │ ::parse_line()  │    │ enum              │               │
│  └─────────────────┘    └──────────────────┘               │
│                                ↓                            │
│  ┌─────────────────────────────────────────┐               │
│  │ dispatch_trae_stream_event()            │               │
│  │   → on_text() / on_tool_call() /        │               │
│  │     on_tool_result() / on_error() /     │               │
│  │     on_complete()                       │               │
│  └─────────────────────────────────────────┘               │
└─────────────────────────────────────────────────────────────┘
```

与现有 Claude/Pi/Copilot parser 的集成点：
- `cli_backend.rs`：`traecli()` 的 `output_format` 改为 `OutputFormat::TraeStreamJson`
- `cli_executor.rs`：在 stdout line 处理分支中，增加 `OutputFormat::TraeStreamJson => { ... }` 分支，调用 `TraeStreamParser::parse_line()` 和 `dispatch_trae_stream_event()`

---

## Implementation Units

- [ ] U1. **新增 `OutputFormat::TraeStreamJson` 并修复 `traecli()` 配置**

**Goal:** 让 `traecli()` backend 生成正确的命令行参数并声明正确的输出格式。

**Requirements:** R1, R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-adapters/src/cli_backend.rs`
- Test: `crates/ralph-adapters/src/cli_backend.rs`（现有测试文件内新增）

**Approach:**
1. 在 `OutputFormat` enum 中新增 `TraeStreamJson` variant（放在 `PiStreamJson` 之后、`Acp` 之前）
2. 修改 `traecli()`：args 添加 `"--output-format"` 和 `"stream-json"`，`output_format` 改为 `OutputFormat::TraeStreamJson`
3. 修改 `traecli_interactive()` 的注释，说明 interactive 模式不使用 stream-json（与 copilot_interactive 等一致）

**Patterns to follow:**
- `claude()` 的 args 中 `--output-format stream-json` 的写法
- `copilot()` 的 `output_format: OutputFormat::CopilotStreamJson` 模式

**Test scenarios:**
- **Happy path**: `CliBackend::traecli()` 构建的命令包含 `--output-format stream-json`，`output_format` 为 `TraeStreamJson`
- **Happy path**: `CliBackend::traecli_interactive()` 构建的命令不包含 `--yolo` 和 `--print`，`output_format` 为 `Text`
- **Edge case**: `CliBackend::from_name("traecli")` 返回的 backend `output_format` 为 `TraeStreamJson`
- **Edge case**: `CliBackend::for_interactive_prompt("traecli")` 返回的 backend `output_format` 为 `Text`
- **Integration**: `build_command("test prompt", false)` 生成的 args 顺序正确：`--yolo`, `--print`, `--output-format`, `stream-json`, `"test prompt"`

**Verification:**
- `cargo test -p ralph-adapters test_traecli` 全部通过
- `traecli()` 的 `output_format` 不再是 `Text`
- **回归防护**: `cargo test -p ralph-adapters test_claude` / `test_copilot` / `test_pi` / `test_kiro` / `test_roo` 等所有现有 backend 测试仍全部通过，确保 enum 扩展和 `filter_args_for_interactive` 改动未影响其他 backend

---

- [ ] U2. **新增 `trae_stream.rs` NDJSON parser 模块**

**Goal:** 实现 trae-cli stream-json 输出格式的解析器，参考 Claude/Pi parser 的设计模式。

**Requirements:** R2, R5

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-adapters/src/trae_stream.rs`

**Approach:**
1. 定义 `TraeStreamEvent` enum（`#[serde(tag = "type", rename_all = "snake_case")]`），初步包含：
   - `System { session_id: String, model: Option<String> }`
   - `Assistant { message: TraeAssistantMessage }`
   - `User { message: TraeUserMessage }`
   - `Result { duration_ms: u64, is_error: bool, ... }`
   - `#[serde(other)] Other`
2. 定义 content block 类型：`TraeContentBlock` enum（`Text`, `ToolUse`）
3. 实现 `TraeStreamParser::parse_line(line: &str) -> Option<TraeStreamEvent>`
4. 定义 `TraeSessionState` 结构体（累积 `num_turns`, `duration_ms`, `is_error`）
5. 实现 `dispatch_trae_stream_event<H: StreamHandler>(...)` 函数：
   - `Assistant` with `Text` content → `handler.on_text()` + 累积到 `extracted_text`
   - `Assistant` with `ToolUse` → `handler.on_tool_call()`
   - `User` with `ToolResult` → `handler.on_tool_result()` 或 `handler.on_error()`
   - `Result` → `handler.on_complete(&session_result)`
   - `Other` → no-op

**Execution note:** 由于 trae-cli NDJSON 的完整字段结构尚未完全确认，实现时应先通过实际运行 `trae-cli --output-format stream-json` 捕获 2-3 条完整 NDJSON 样本，再精确定义 `TraeStreamEvent` 的字段。若发现结构与 Claude 高度一致，可复用 `claude_stream.rs` 的部分类型定义（但不直接依赖，保持独立模块）。

**Technical design:** *(directional guidance, not specification)*

```rust
// TraeStreamEvent 初步设计（需在实现时根据实际样本调整）
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeStreamEvent {
    System { session_id: String, model: String },
    Assistant { message: AssistantMessage },  // 可能复用 claude_stream 的 AssistantMessage
    User { message: UserMessage },            // 可能复用 claude_stream 的 UserMessage
    Result { duration_ms: u64, is_error: bool },
    #[serde(other)]
    Other,
}
```

**Patterns to follow:**
- `claude_stream.rs` 的 enum 结构、`parse_line` 实现、`truncate` helper
- `pi_stream.rs` 的 `dispatch_pi_stream_event` 模式、`PiSessionState` 设计
- `copilot_stream.rs` 的 `extract_text` / `extract_all_text` 辅助函数（如需要）

**Test scenarios:**
- **Happy path**: `parse_line` 正确解析 `system` 事件
- **Happy path**: `parse_line` 正确解析 `assistant` 事件的 `text` content block
- **Happy path**: `parse_line` 正确解析 `assistant` 事件的 `tool_use` content block
- **Happy path**: `parse_line` 正确解析 `result` 事件
- **Happy path**: `dispatch_trae_stream_event` 将 `assistant.text` 路由到 `on_text`
- **Happy path**: `dispatch_trae_stream_event` 将 `assistant.tool_use` 路由到 `on_tool_call`
- **Happy path**: `dispatch_trae_stream_event` 将 `result` 路由到 `on_complete`
- **Edge case**: `parse_line("")` 返回 `None`
- **Edge case**: `parse_line("malformed json")` 返回 `None`
- **Edge case**: 未知 `type` 解析为 `TraeStreamEvent::Other`
- **Integration**: 多个 NDJSON 行连续解析并 dispatch，累积的 `extracted_text` 与预期一致

**Verification:**
- `cargo test -p ralph-adapters trae_stream` 全部通过
- parser 对空行、畸形 JSON、未知类型都能优雅处理

---

- [ ] U3. **在 `cli_executor.rs` 中集成 `TraeStreamJson` parser**

**Goal:** 让 executor 在读取 trae-cli stdout 时调用新的 parser，而非当作纯文本打印。

**Requirements:** R2, R5

**Dependencies:** U1, U2

**Files:**
- Modify: `crates/ralph-adapters/src/cli_executor.rs`
- Test: `crates/ralph-adapters/src/cli_executor.rs`（现有测试文件内新增）

**Approach:**
1. 在 `cli_executor.rs` 顶部 import `TraeStreamParser` 和 `dispatch_trae_stream_event`
2. 在 `execute` 方法的 stdout line 处理分支（第 191-210 行）中，增加 `OutputFormat::TraeStreamJson` 处理分支：
   - 对每个 stdout line，调用 `TraeStreamParser::parse_line(&line)`
   - 对解析成功的事件，调用 `dispatch_trae_stream_event(...)`
   - 对解析失败的行，降级为直接写入 `output_writer`（保留调试可见性）
3. 维护 `trae_session_state` 和 `extracted_text` 局部变量（参考 copilot 分支的模式）

**Patterns to follow:**
- `CopilotStreamJson` 的处理模式（第 197-206 行）：`if backend.output_format == OutputFormat::CopilotStreamJson { ... }`
- `pi_stream.rs` / `copilot_stream.rs` 的 dispatch 函数签名

**Test scenarios:**
- **Happy path**: 使用 `printf` 模拟 trae-cli NDJSON 输出，`execute()` 正确提取 assistant 文本到 output_writer
- **Happy path**: NDJSON 流包含 tool call 和 tool result，正确路由到 handler callbacks
- **Happy path**: NDJSON 流以 `result` 事件结束，正确触发 `on_complete`
- **Edge case**: 混合有效 NDJSON 行和空行/畸形行，有效行被解析，其余被忽略或降级输出
- **Integration**: 完整执行模拟（printf 输出多条 NDJSON），返回的 `ExecutionResult.success` 为 true，accumulated_output 包含原始 NDJSON

**Verification:**
- `cargo test -p ralph-adapters test_execute_trae_stream` 通过
- 新增测试的 pattern 与 `test_execute_copilot_stream_writes_extracted_text` 一致
- **回归防护**: `cargo test -p ralph-adapters` 全部通过（特别是 `test_execute_copilot_stream_writes_extracted_text`、`test_execute_echo`、`test_execute_stdin`、`test_execute_timeout` 等现有 executor 测试），确认新增 match 分支未改变其他 output format 的处理路径

---

- [ ] U4. **在 `lib.rs` 中注册并导出 `trae_stream` 模块**

**Goal:** 使新模块成为 crate 的公开 API 的一部分。

**Requirements:** R5

**Dependencies:** U2

**Files:**
- Modify: `crates/ralph-adapters/src/lib.rs`

**Approach:**
1. 添加 `mod trae_stream;`
2. 添加 `pub use trae_stream::{...}`，导出必要的 public types（`TraeStreamEvent`, `TraeStreamParser`, `TraeSessionState`, `dispatch_trae_stream_event`）

**Test expectation:** none — 纯模块注册，无行为变更

**Verification:**
- `cargo check -p ralph-adapters` 通过，无未使用 import 警告

---

- [ ] U5. **更新 zsh 补全插件和 skill 文档**

**Goal:** 确保 CLI 补全和 skill 文档反映 traecli backend 的可用性。

**Requirements:** R5

**Dependencies:** U1

**Files:**
- Modify: `scripts/ralph-zsh-plugin.zsh`
- 检查: `crates/ralph-core/data/ralph-tools.md` 等 skill 文档中是否有 backend 列表需要更新

**Approach:**
1. 检查 `scripts/ralph-zsh-plugin.zsh` 中 `builtin:` 补全列表是否已包含 `traecli`（根据 AGENTS.md 规则，添加/修改 preset 或 builtin backend 时需同步更新）
2. 检查 `crates/ralph-core/data/*.md` 中是否有 backend 列表提到 `traecli`，确认行号引用是否准确
3. 如有变更，按 AGENTS.md 的反向验证规则：修改后用 `ralph --help` 或相关命令做冒烟测试

**Test expectation:** none — 纯文档/配置变更

**Verification:**
- `grep traecli scripts/ralph-zsh-plugin.zsh` 确认存在
- 如需修改，执行 `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`

---

- [ ] U6. **端到端测试验证（trae_test 测试目录）**

**Goal:** 在真实 trae-cli 环境中验证修复后的集成效果。

**Requirements:** R3

**Dependencies:** U1, U2, U3, U4

**Files:**
- 修改: `/home/chaowen/Dev/agent_tools/trae_test/test-ralph.sh`（更新为使用修复后的 ralph binary）
- 创建: `/home/chaowen/Dev/agent_tools/trae_test/test-ralph-stream-parser.sh`（可选，验证 parser 输出）

**Approach:**
1. 重新编译 ralph：`cargo build --release -p ralph-cli`
2. 复制新的 binary 到 PATH 或直接在测试目录使用 `cargo run -p ralph-cli --`
3. 在 `trae_test` 目录下运行 `./test-ralph.sh`，验证：
   - Test A（`traecli` backend）：stdout 有 NDJSON 输出，不再是空
   - ralph 的 event loop 能够处理 trae-cli 的输出
4. 运行 `./test-direct.sh` 确认 trae-cli 本身行为未变
5. 在 `ralph.yml` 中使用 `backend: traecli` 运行 `ralph run -p "Say hello" --max-iterations 1`，观察输出

**Test scenarios:**
- **Happy path**: `ralph run --backend traecli` 能成功启动 trae-cli 并收到 stdout
- **Happy path**: ralph 的 stream handler 正确渲染 assistant 文本（非原始 NDJSON）
- **Error path**: 如果 trae-cli 返回 API 400 错误，ralph 应能捕获 stderr 或解析 result 事件中的 `is_error: true`
- **Edge case**: 大 prompt（>7000 chars）时，ralph 的 temp-file 机制正常工作（这是 `build_command` 的通用逻辑，traecli 继承）

**Verification:**
- `./test-ralph.sh` 输出显示 Test A 有 stdout 内容
- `ralph run -p "Say hello in one word" --max-iterations 1` 在 `trae_test` 目录下运行，迭代完成（或正确报错而非静默失败）
- **回归防护**: 在 `trae_test` 目录下临时将 `ralph.yml` 改为 `backend: claude`（或其他可用 backend），确认 `ralph run` 仍能正常工作，确保修改未破坏非 traecli 场景

---

## System-Wide Impact

- **Interaction graph**: `cli_backend.rs` → `cli_executor.rs` → `trae_stream.rs` → `StreamHandler`。新增一条与 Claude/Pi/Copilot 平行的数据流，不影响现有 backend 的调用路径。
- **Error propagation**: parser 失败时降级为纯文本输出（保留原始 NDJSON 行到 output_writer），不中断执行。
- **API surface parity**: `OutputFormat` enum 新增 variant，这是 public API 变更。下游消费者（如有）需要匹配新 variant，但 `OutputFormat` 主要在 `ralph-adapters` 内部使用，外部暴露有限。
- **Unchanged invariants**: 
  - 其他 backend（claude, pi, copilot, kiro, gemini, codex, amp, opencode, roo）的 args、prompt_mode、output_format 完全不变
  - `auto_detect.rs` 的 `DEFAULT_PRIORITY` 和 `detection_command("traecli")` 无需修改
  - `filter_args_for_interactive` 中 trae-cli 的过滤逻辑（移除 `--yolo` 和 `--print`）保持不变
  - `cli_executor.rs` 中对 `Text`、`StreamJson`、`CopilotStreamJson`、`PiStreamJson`、`Acp` 的 stdout 处理分支代码一字不改
  - `lib.rs` 中现有模块的 `pub use` 声明不受新增模块影响
  - `OutputFormat` enum 的 `Default` 实现仍指向 `Text`，不影响任何默认行为

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| **引入回归问题——修改了其他 backend 的行为** | **核心原则"只增不改"**：不修改任何现有 backend 的构造参数；在 `cli_executor.rs` 中只新增 match arm 或独立 if 分支；每次修改后运行 `cargo test -p ralph-adapters` 全量测试作为门禁 |
| trae-cli NDJSON 格式与预期不一致（非标准 Claude 兼容格式） | 实现 U2 时先捕获实际样本再精确定义结构；保持 `#[serde(other)]` 容错 |
| trae-cli 服务端的 400 错误导致即使修复后也无法完成完整迭代 | 明确告知用户这是 trae-cli 服务端问题，非 ralph 问题；测试时可用简单 prompt 验证 parser 工作 |
| `TraeStreamJson` 处理分支引入 executor 中的代码重复 | 观察是否可抽象通用的 NDJSON dispatch 模式；当前保持与其他 backend 一致的显式分支风格 |

---

## Documentation / Operational Notes

- 更新 `/home/chaowen/Dev/agent_tools/trae_test/README.md` 的验证结果表格，标记修复后的状态
- 如 trae-cli 的 API 400 问题持续，建议在 README 中注明："traecli backend 集成已修复，但 trae-cli 服务端可能存在 model 配置问题"

---

## Sources & References

- **诊断报告**: `/home/chaowen/Dev/agent_tools/trae_test/README.md`
- **测试脚本**: `/home/chaowen/Dev/agent_tools/trae_test/test-direct.sh`, `/home/chaowen/Dev/agent_tools/trae_test/test-ralph.sh`
- **相关代码**: 
  - `crates/ralph-adapters/src/cli_backend.rs` (traecli 定义)
  - `crates/ralph-adapters/src/cli_executor.rs` (executor 主循环)
  - `crates/ralph-adapters/src/claude_stream.rs` (parser 参考)
  - `crates/ralph-adapters/src/pi_stream.rs` (dispatch 参考)
  - `crates/ralph-adapters/src/copilot_stream.rs` (extract_text 参考)
