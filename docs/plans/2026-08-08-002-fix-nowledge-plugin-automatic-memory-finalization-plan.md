---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
executor_head_sha: 5bb55e663c64a89c62167f480641a7e826852880
---

# canonical executor anchor for review trace; do not hand-write elsewhere.
# If the executor branch is rebased/amended, update this field AND regenerate
# the synthesized-review.md so the reviewer chain stays auditable.

# Nowledge Mem 插件自动 Memory Finalization 开发计划

## Goal Capsule

### Problem

`nowledge-mem-ralph` 当前的 `Stop` 与 `SubagentStop` hook 只记录审计信息，明确不读取 transcript、不开启 `nmem`、不调用现有 `save-memory` 写入链路。因此 Claude session 可以被 watcher 保存为 Thread，但插件不会在 session 结束时形成 Memory。现有 `memory.py` 与 `memory_writer.py` 已经具备校验、去重和写入能力，缺失的是插件 hook 到这条链路之间的自动触发和候选输入契约。

### Outcome

在不修改 Ralph loop、Ralph Rust runtime、preset 或 Ralph 配置的前提下：

1. Claude 的 `Stop` / `SubagentStop` hook 从 `last_assistant_message` 中只提取一个受限的结构化 Memory candidate；
2. 只有带有明确 finalization 标记、通过现有 `memory.py` policy 的 candidate 才进入现有 `memory_writer.py`；
3. 重复 Stop、重复 SubagentStop 或相同 candidate 只产生一次 nmem 写入；
4. 没有结构化 candidate、非 Ralph session、非法输入、nmem 402/超时/解析失败都不会破坏 Claude session；
5. raw transcript、Thread 内容、`transcript_path` 永远不进入插件保存逻辑；
6. 审计结果能区分 `SKIPPED`、`REJECTED`、`SAVED`、`ALREADY_SAVED`、`FAILED_OPEN` 和 `UNKNOWN`。

### Scope boundaries

本计划只允许修改 `plugins/nowledge-mem-ralph` 及其直接插件文档和测试。明确不修改：

- `crates/ralph-*`、Ralph loop 的 hook engine、termination lifecycle、adapter、preset 和配置 schema；
- Ralph 的 Thread 保存逻辑、nmem desktop watcher、社区 `nowledge-mem` 插件；
- raw Claude transcript 的上传、解析或持久化；
- 让插件凭空生成摘要的 LLM 调用；候选必须由 Claude 输出一个经过 schema 约束的结构化块；
- Codex 专属 hook。Codex 线程来源不是本插件的可验证入口，不能在本计划中伪装成已接入。

## Product Contract

### Summary

插件的职责是“自动触发已有 save-memory 流程”，不是“把每个 Thread 自动变成 Memory”。Claude 只有在最终消息中提供明确、可校验、可复用的 Memory candidate 时才保存 Memory；普通过程消息、日志、进度和 Thread 不保存。

### Requirements

- **R1 Hook 自动触发**：Ralph 环境存在且 `RALPH_NOWLEDGE_ENABLED=1` 时，`Stop` 与 `SubagentStop` 读取 hook stdin 的 `last_assistant_message` 字段，并尝试处理其中的 finalization candidate。
- **R2 结构化边界**：只接受一个固定 fenced marker 中的 JSON object；不读取 `transcript_path`，不读取任意 transcript 文件，不扫描当前目录寻找候选。
- **R3 显式终态**：candidate 必须包含 `finalize: true`；缺失或为 false 时只记录 `SKIPPED`，不能写 nmem。
- **R4 复用既有链路**：candidate 必须通过已有 `memory.py::save`；只有 `ACCEPTED` record 才调用已有 `memory_writer.py::write_from_save_result`。
- **R5 幂等**：沿用 `memory_writer.py` 的 digest ledger；重复 candidate 返回 `ALREADY_SAVED`，不得第二次调用 `nmem --json m add`。
- **R6 失败隔离**：提取、JSON 解析、policy、evaluator、nmem 或本地审计失败均不让 hook 非零退出；但非法 hook invocation 本身仍按现有 hook runtime 的安全约束处理。
- **R7 可观察性**：`memory-results.jsonl` 和 `audit.jsonl` 能关联 loop、hat、session、hook event、candidate digest 和最终状态，但不得写入完整 assistant message。
- **R8 非 Ralph 兼容**：缺少 Ralph env 时保持现有 no-op 行为，不创建插件状态目录，不调用 nmem。
- **R9 文档契约**：更新插件 README、`commands/save-memory.md`、`skills/save-memory/SKILL.md` 和相关测试说明，明确“hook 自动提交结构化 candidate”与“Thread 不等于 Memory”的边界。

### Marker contract

Unit 1 必须固定并测试下面的唯一包装格式，不允许 Executor 自行发明 marker 名称或 JSON 位置：

```text
<!-- nowledge-memory-finalize
{"finalize":true,"memory_type":"durable_root_cause",...}
-->
```

规则如下：marker 名称必须精确为 `nowledge-memory-finalize`；开始标记和结束标记各只能出现一次；中间内容必须是一个 UTF-8 JSON object；`finalize` 必须为 JSON boolean `true`；marker payload 的 UTF-8 字节数上限为 16 KiB；解析器只返回 JSON object，不保存包装文本；同一 assistant message 出现多个 marker 时拒绝而不是择一处理。

### Non-goals

- 不把所有 Claude Stop 消息保存成 Memory；
- 不通过 `transcript_path` 回读完整会话；
- 不改变 `memory_schema.py`、`memory_policy.py` 的现有阈值和 Memory 类型；
- 不绕过 `memory_writer.py` 直接运行 nmem；
- 不对 402 进行无限重试；
- 不在 Ralph loop 内增加一个 Memory hat、termination hook 或 Rust bridge。

### Assumptions

已确认的假设：

- Claude hook payload 在真实形态下可能包含 `last_assistant_message`；已有 fixture 已验证该字段存在。
- 结构化 marker 由当前 Claude agent 产生，插件只做提取、校验和写入，不承担摘要生成。
- 现有插件安装方式和 `plugin.json` hook 入口继续有效，只需扩展脚本行为和文档。

待验证假设及验证方式：

- **A1**：Claude Code 对 Stop 与 SubagentStop 都把 `last_assistant_message` 作为 JSON string 传入。进入 Unit 1 前用现有 fixture、一个本地 subprocess fixture 和真实 `claude` hook dry-run（若当前环境可用）验证；若真实 payload 不是 string，必须在 Unit 1 更新 parser contract，不能由 Executor 自行选择。
- **A2**：hook 默认 cwd 可稳定解析 `CLAUDE_PLUGIN_ROOT` 下脚本，但 candidate 不应依赖 cwd。用 subprocess 测试把 cwd 改为临时目录并检查仍能完成本地状态写入；失败时必须改为显式 plugin-root 路径，不可依赖当前目录。
- **A3**：现有 writer 的 `FAILED_OPEN` / `UNKNOWN` 语义足以表达自动保存失败。用已有 writer fake runner 验证；若自动 hook 需要额外状态，只能扩展现有 result/audit，不得新增第二套 remote-write ledger。

## Planning Contract

### Repository baseline

- 工作区：`/Users/pittcat/Dev/Rust/ralph-orchestrator`
- 分支：`pittcat-dev`
- 基线：调查时 `HEAD=691f98f6`，工作区无未提交变更。
- 规划文档根目录：`docs/plans`；仓库未发现 `.compound-engineering/config.yaml` 或 `.compound-engineering/config.local.yaml`。

### Execution constraints

- 严格按 Unit 1 → Unit 2 → Unit 3 → Unit 4 执行。
- 每个 Unit 必须先写 Acceptance Red，再写最小实现；不得先改 hook 再补测试。
- 只允许 Python 标准库和现有插件模块；除非 Unit 1 的验证发现现有运行环境明确已有依赖，否则不得引入新依赖。
- 不得修改 `crates/ralph-*`。若实现时发现必须改 Ralph 代码，立即停止并将本计划标记为 BLOCKED。

### Key technical decisions

本计划中的关键决策均达到 ≥0.85 置信度；具体证据见 Decision Ledger。

## Implementation Units

### Unit 1：固定自动 finalization marker 的解析与旧行为基线

#### 1. Unit 目标

让插件能够从 `last_assistant_message` 中提取最多一个明确标记的 JSON candidate，并保持无 marker / 非 Ralph 输入的旧 no-op 行为。

#### 2. 对应需求与 Scenario

- Requirements：R1、R2、R3、R8
- Scenarios：S1、S2、S3、S4
- Decisions：D1、D2
- Evidence：E1、E2、E3、E7

#### 3. 外部可观察结果

给定包含合法 marker 的 Stop payload，hook 内部能得到一个 candidate；给定普通文本、多个 marker、超长 marker、非法 JSON、缺少 Ralph env 或 `finalize` 非 true 时，插件不调用 nmem，并产生可断言的 skip/reject reason。

#### 4. 当前行为基线

当前 `hook_runtime.py::_handle_stop` 与 SubagentStop handler 只审计，不读 `last_assistant_message`，测试 `test_stop_audit_placeholder` 明确锁定该行为。先保留一个 characterization assertion：旧输入没有 marker 时仍不产生 writer 调用；随后新增合法 marker 的 Acceptance Red。

#### 5. 输入与输出

- 输入：hook stdin JSON；必需检查 `last_assistant_message` 是否为 string；环境由 `resolve_nowledge_env()` 处理。
- 输出：内部 parser result 为 `SKIPPED`、`PARSE_ERROR` 或 candidate；不得输出完整消息。
- 错误：stdin 非 object、字段类型不符、marker 重复、JSON 非 object、超过 byte 上限均为可审计拒绝，不抛出未捕获异常。
- 状态变化：本 Unit 不写 nmem；只允许追加 bounded parser/audit result。
- 不变量：不访问 `transcript_path` 指向的文件，不读取 `last_assistant_message` 以外的 transcript 数据。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/scripts/hook_runtime.py` | 解析 hook event、env gate、分发 Stop/SubagentStop | 调用独立 parser 并把有限结果交给后续保存入口 | SessionStart recall、Ralph env 命名和 exit contract |
| `plugins/nowledge-mem-ralph/scripts/memory_marker.py`（新增） | 当前无 marker parser | 只实现上述固定 marker、16 KiB 上限和 JSON shape 检查 | memory policy、nmem writer |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | hook subprocess contract | 增加合法/非法 marker 和无 marker characterization | 删除或削弱旧 no-op 断言 |

#### 7. 可依赖能力

- `resolve_nowledge_env()` 的现有 gate；
- 已确认的 `hook_runtime.py` subprocess entry；
- `test_ralph_bridge_e2e.py` 的 `_run()` subprocess fixture；
- Python 标准库 JSON、字符串和 UTF-8 byte length。

#### 8. 禁止依赖的未来能力

- 不调用 `memory.py` 或 `memory_writer.py`；
- 不修改 hook manifest；
- 不处理 nmem 错误；
- 不实现最终 prompt 文档。

#### 9. 验收测试

测试入口为 `hook_runtime.py` subprocess。输入合法 marker 时断言 parser 得到 candidate 且没有完整消息写入状态文件；无 marker、非法 JSON、重复 marker、超长 marker 和 `finalize:false` 均断言不产生 nmem 调用。运行：`python3 -m pytest plugins/nowledge-mem-ralph/tests/test_hook_runtime.py plugins/nowledge-mem-ralph/tests/test_ralph_bridge_e2e.py -q`。

#### 10. Acceptance Red

首先新增合法 marker 的 subprocess 测试，在当前实现上预期失败：没有 candidate extraction result，Stop 仍只写 audit-only marker。这个失败证明测试经过真实 hook entry 且目标能力缺失。以下不算有效 Red：pytest 收集失败、JSON fixture 语法错误、PATH 配置导致 Python 无法启动、或测试未触发 `hook_runtime.py`。

#### 11. 单元测试拆分

1. 合法 marker：输入一个 UTF-8 JSON object，期望返回一个 candidate，保留字段值，不保留 marker 包装。
2. `finalize:false`：期望 `SKIPPED`，不得产生 candidate。
3. 无 marker：期望 `SKIPPED`。
4. 非法 JSON / 非 object：期望 `PARSE_ERROR`，不得抛异常。
5. 多 marker：期望拒绝，避免不确定选择。
6. 超过限定 UTF-8 byte 数：期望拒绝，防止 hook 输出或状态膨胀。
7. transcript safety：`transcript_path` 指向一个可读敏感 fixture，期望内容未被打开；测试只验证访问行为，不把敏感内容写入断言输出。

#### 12. Red → Green → Refactor 顺序

`test_valid_finalization_marker` Red → 新增 parser 最小实现 → Green；
`test_non_final_message_is_skipped` Red → 增加 `finalize:true` gate → Green；
`test_malformed_duplicate_and_oversized_marker_are_rejected` Red → 增加边界拒绝 → Green；
`test_transcript_path_is_never_read` Red → 约束 parser 只接收 message string → Green；
最后在测试保护下抽取 bounded constants / result type，并重跑 Unit 全部测试。

#### 13. 最小实现范围

必须实现固定 marker 的提取、单 marker 约束、byte 上限、JSON object 校验和 `finalize:true` 校验。不得实现摘要生成、policy、writer、重试或 Ralph runtime 接入。

#### 14. 集成验证

真实验证 `hook_runtime.py` subprocess；parser 本身可直接单测。nmem 和 writer 在本 Unit 使用明确禁止调用的 fake guard；不允许 mock parser 本身。

#### 15. 风险驱动测试

增加 Characterization Test 和 security test：旧 no-op 必须保持，且防止未来改动偷偷读取 transcript。无需 property-based/fuzz，因本 Unit 的主要风险是边界和数据泄露，不是复杂业务转换。

#### 16. 回归范围

运行现有 hook runtime、Ralph bridge E2E、security contracts；SessionStart recall 测试必须通过，因为 parser 不应改变 SessionStart 分支。不得运行或修改 Ralph Rust 测试作为本 Unit 的实现手段。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/scripts/hook_runtime.py` | 修改现有生产文件 | 接入 parser result | E1、E2 |
| `plugins/nowledge-mem-ralph/scripts/memory_marker.py` | 新增插件模块 | bounded marker extraction | E2、E3 |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | 新增测试 | Acceptance Red 与 characterization | E7 |
| `plugins/nowledge-mem-ralph/tests/test_security_contracts.py` | 修改测试 | 固定 transcript 不读取约束 | E3 |

#### 18. 完成标准

当前 Scenario、Unit 测试、插件回归、Python syntax check 全部通过；无新 skip/only；parser 不改变无 env no-op；证据记录补充真实 Red 结果；Unit 可独立提交。

#### 19. 停止条件

如果真实 payload 没有 `last_assistant_message`、字段不是 string、marker 与现有命令文档冲突，或必须读取 transcript 才能完成，则停止并更新 D1/D2，不得自行改用另一字段。

#### 20. 风险与注意事项

风险是 assistant message 中出现伪 marker 或超长内容。检测方式是 duplicate/size tests；缓解为单 marker、固定 byte 上限和 `finalize:true`。剩余风险是没有 marker 时不会保存 Memory，这是有意的安全降级，Unit 3 的 agent-facing contract 负责降低发生率。

### Unit 2：把合法 candidate 接入现有 save-memory → writer 链路

#### 1. Unit 目标

让合法 candidate 自动经过现有 `memory.py::save`、`memory_writer.py::write_from_save_result`，并记录最终结果；不创建第二套 nmem 写入路径。

#### 2. 对应需求与 Scenario

- Requirements：R1、R4、R5、R6、R7
- Scenarios：S1、S5、S6、S7、S8
- Decisions：D3、D4
- Evidence：E4、E5、E6、E8

#### 3. 外部可观察结果

合法 finalization marker 出现在 Stop/SubagentStop 后，nmem fake runner 观察到一次标准 `nmem --json m add` argv；相同 digest 的第二次 hook 返回 `ALREADY_SAVED` 且 fake runner 调用次数仍为一次。policy reject、evaluator reject、402、超时和 invalid JSON 均返回已有状态，不让 hook 失败。

#### 4. 当前行为基线

当前 `memory.py` 文档和实现说明 U03 不负责 nmem，U04 writer 只接受 `ACCEPTED` record；当前 Stop handler 不调用二者。现有 writer tests 已锁定 accepted-only、digest dedupe、remote failure 状态。

#### 5. 输入与输出

- 输入：Unit 1 输出的 candidate、现有 Ralph env source metadata、当前 hook event/session metadata。
- 输出：现有 `MemoryWriteResult` 加 bounded audit record；成功为 `SAVED` 或 `ALREADY_SAVED`。
- 错误：policy result 原样保留；writer failure 使用现有 `FAILED_OPEN` / `UNKNOWN` 语义。
- 副作用：只有 accepted record 可产生 nmem side effect；audit 文件不得包含完整 candidate message。
- 不变量：writer 的 ledger lock、pending/saved 语义和 digest 不改变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/scripts/hook_runtime.py` | Stop/SubagentStop handler | 将 parser candidate 交给新增 finalization coordinator | SessionStart、recall、env gate |
| `plugins/nowledge-mem-ralph/scripts/memory.py` | schema/policy/dedupe save entry | 仅在现有 import contract 不足时增加最小可复用调用入口；优先不改 | schema 字段和阈值 |
| `plugins/nowledge-mem-ralph/scripts/memory_writer.py` | accepted-only nmem writer | 仅复用现有函数；不得复制 ledger 或 nmem argv | lock、ledger、client 协议 |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | hook integration tests | 加 fake runner/monkeypatch 验证调用链 | 旧安全断言 |
| `plugins/nowledge-mem-ralph/tests/test_memory_writer.py` | writer contract | 作为回归，不改弱断言 | writer 设计 |

#### 7. 可依赖能力

Unit 1 parser；现有 `memory.save`、`write_from_save_result`、`record_result` 和 `memory-ledger.json`。

#### 8. 禁止依赖的未来能力

不修改 agent prompt；不增加 semantic evaluator 的默认实现；不读取 Thread；不增加 Ralph lifecycle hook。

#### 9. 验收测试

用 `test_hook_runtime.py` 的 subprocess 入口和可注入 fake nmem runner：合法 candidate 断言 policy accepted、writer 调用一次、audit 含 loop/session/event/digest/status；重复 Stop 断言第二次为 `ALREADY_SAVED`；reject 和 remote failure 断言不抛异常且结果可重试/可诊断。运行：`python3 -m pytest plugins/nowledge-mem-ralph/tests/test_hook_runtime.py plugins/nowledge-mem-ralph/tests/test_memory.py plugins/nowledge-mem-ralph/tests/test_memory_writer.py -q`。

#### 10. Acceptance Red

在当前实现上运行合法 marker + fake nmem 测试，预期 fake runner 调用次数为 0，因为 Stop handler 仍 audit-only；这是有效 Red。若测试因 import path、fixture 或 subprocess 启动失败，不是有效 Red，必须先修测试 harness。

#### 11. 单元测试拆分

1. accepted candidate reaches writer exactly once。
2. rejected candidate never reaches writer。
3. `NEEDS_REWRITE` never reaches writer。
4. duplicate digest returns `ALREADY_SAVED` without a second runner call。
5. nmem 402/非零退出、超时、invalid JSON preserves existing writer failure status。
6. missing loop env remains no-op even with a valid marker。
7. audit record contains bounded source metadata, never full assistant message or transcript path content。

#### 12. Red → Green → Refactor 顺序

`test_stop_accepted_candidate_calls_writer` Red → coordinator calls existing save/writer → Green；
`test_rejected_candidate_does_not_call_writer` Red → result gate → Green；
`test_duplicate_stop_is_idempotent` Red → use existing writer ledger → Green；
`test_nmem_failure_is_fail_open_and_audited` Red → bounded exception/result handling → Green；
随后抽取 coordinator 并重跑现有 memory tests。

#### 13. 最小实现范围

实现一个插件内部 coordinator：解析结果为 candidate 时调用 `memory.save`，accepted 时调用 writer，所有结果交给 audit。不得修改 schema/policy thresholds，不得直接调用 nmem，不得吞掉 writer 的 `UNKNOWN` 语义。

#### 14. 集成验证

真实组合 hook runtime + memory policy + memory writer；只把 nmem client 外部进程替换为 fake runner。必须真实验证 policy gate、digest ledger 和 audit wiring。

#### 15. 风险驱动测试

必须做 Idempotency Test 和 Fault Injection：用户观察到的主要风险是重复 hook 与 nmem 402 导致重复 Memory 或不清晰状态。沿用现有 writer test 的 fake runner，不 mock policy/writer。

#### 16. 回归范围

完整插件 Python tests、security contracts、recall tests、bridge E2E。旧的 SessionStart no-op、Stop no-env no-op、accepted-only writer 和 UNKNOWN ledger tests 都必须保持通过。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/scripts/hook_runtime.py` | 修改现有生产文件 | 自动调用 coordinator | E1、E4 |
| `plugins/nowledge-mem-ralph/scripts/memory_finalization.py`（新增） | 新增插件模块 | 连接现有 save 与 writer | E4、E5 |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | 修改测试 | 证明 hook 到 writer 的真实链路 | E7、E8 |
| `plugins/nowledge-mem-ralph/tests/test_memory.py` | 修改或新增测试 | 固定 coordinator 使用 policy 的行为 | E4 |

#### 18. 完成标准

accepted-only、dedupe、failure isolation、audit contract 全通过；没有 direct nmem call；插件完整测试通过；Unit 可独立提交。

#### 19. 停止条件

若必须修改 writer ledger、改变现有 result enum、绕过 memory.py，或发现 nmem client API 与现有 writer contract 不兼容，停止并重新决策。

#### 20. 风险与注意事项

远端已接受而本地 ledger 提交失败时只能保留 `UNKNOWN`，不得改成 `SAVED` 或盲目重试；这是现有 writer 的安全约束，不能因为自动 hook 而放宽。

### Unit 3：建立 agent-facing finalization 输出契约

#### 1. Unit 目标

让 Claude agent 在真正有可复用结论时知道如何输出结构化 finalization marker，减少“插件已运行但没有 candidate”的误解和漏保存。

#### 2. 对应需求与 Scenario

- Requirements：R3、R9
- Scenarios：S1、S2、S9
- Decisions：D2、D5
- Evidence：E4、E9

#### 3. 外部可观察结果

插件文档和 save-memory skill 明确说明：普通回答不保存；只有最终消息中的合法 marker 才自动提交；candidate 仍必须满足固定 schema、质量指标和无 critical assumption/ambiguity。

#### 4. 当前行为基线

`commands/save-memory.md` 和 `skills/save-memory/SKILL.md` 当前要求 agent 显式调用 `memory.py`，并将 save-memory 描述成唯一写入路径，但没有 Stop hook 自动消费 marker 的说明。README 仍写着 Stop audit-only/no save，与目标行为冲突。

#### 5. 输入与输出

- 输入：agent-facing 文档中的固定 marker 示例和字段说明。
- 输出：Claude 最终消息可产生 Unit 1 能识别的单一 JSON candidate。
- 错误：文档必须要求 agent 把 policy reject/rewrite reason 暴露给用户或停止，不得伪造成功。
- 不变量：文档不能鼓励保存 progress/log/command/transcript。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/commands/save-memory.md` | CLI command contract | 增加 hook auto-finalization 说明、marker 约束和结果解释 | 固定 schema、阈值和命令入口 |
| `plugins/nowledge-mem-ralph/skills/save-memory/SKILL.md` | agent-facing save guidance | 增加“终态消息输出 marker”与“不把 Thread 当 Memory”规则 | 不增加 Ralph runtime 命令 |
| `plugins/nowledge-mem-ralph/README.md` | plugin lifecycle/user docs | 将 Stop/SubagentStop contract 改为 bounded automatic finalization | 安装、卸载、隐私边界 |
| `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py` | plugin text/manifest contract tests | 只断言稳定用户契约和 manifest wiring | 不锁定整篇 prompt 文案 |

#### 7. 可依赖能力

Unit 1 marker parser 和 Unit 2 coordinator 已通过。

#### 8. 禁止依赖的未来能力

不增加自动摘要模型、不改 Ralph prompt injection、不改变 community watcher。

#### 9. 验收测试

文档 contract test 断言 marker 名称、`finalize:true`、raw transcript 禁止、policy/writer 结果语义和 Thread/Memory 区分存在；测试不得做整文件 byte equality。运行：`python3 -m pytest plugins/nowledge-mem-ralph/tests/test_plugin_contract.py plugins/nowledge-mem-ralph/tests/test_security_contracts.py -q`。

#### 10. Acceptance Red

在当前文档上运行新的 contract assertions，预期 README 仍包含 audit-only/no save，且缺少自动 marker 说明；这证明文档与新行为契约不一致。若测试只因任意文案变化失败，测试设计不合格，必须改成稳定锚点。

#### 11. 单元测试拆分

1. README lifecycle table describes automatic bounded finalization。
2. command/skill describe marker and `finalize:true`。
3. docs forbid transcript/raw Thread as Memory。
4. docs preserve schema/policy/writer result semantics。

#### 12. Red → Green → Refactor 顺序

contract anchor Red → 更新 README lifecycle contract → Green；
command/skill marker anchor Red → 更新 agent-facing guidance → Green；
security boundary anchor Red → 更新禁止事项 → Green；
最后检查文档是否泄漏内部实现路径或未解释术语。

#### 13. 最小实现范围

只修改插件用户可见 contract。不要顺带修改 Ralph `crates/ralph-core/data/*.md`，因为本计划没有新增 Ralph CLI/event/config 能力，且范围硬约束为插件-only。

#### 14. 集成验证

把文档 contract tests 与 Unit 1/2 的 subprocess tests 一起运行，证明文档描述的 marker 能真正被 parser/ coordinator 接受。

#### 15. 风险驱动测试

只做 contract test；不做 snapshot/golden，因为 prompt 文案是可演进内容，稳定契约应通过关键锚点验证。

#### 16. 回归范围

完整 plugin tests、README installation smoke anchors、security tests。Ralph 文档和 Rust tests 不在本 Unit 范围。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/README.md` | 修改文档 | 修正 lifecycle contract | E1、E9 |
| `plugins/nowledge-mem-ralph/commands/save-memory.md` | 修改文档 | 说明自动 finalization 输入 | E4 |
| `plugins/nowledge-mem-ralph/skills/save-memory/SKILL.md` | 修改文档 | agent 可执行输出契约 | E4 |
| `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py` | 修改测试 | 稳定契约验收 | E9 |

#### 18. 完成标准

文档不再声称 Stop 永不补保存；agent 能按文档生成合法 candidate；没有整文件 snapshot、无解释断言放宽或新跳过测试。

#### 19. 停止条件

如果目标 marker 无法在 Claude hook payload 中稳定表达，或文档必须要求读取 transcript 才能生成 candidate，停止并回到 D1/D2。

#### 20. 风险与注意事项

agent 可能不输出 marker，导致 no-op。该风险不能通过“保存完整 Thread”解决；只能通过明确 skill、合法示例和 audit 可见性降低。

### Unit 4：插件安装契约、端到端回归与故障可观测性收口

#### 1. Unit 目标

证明安装后的 plugin manifest 仍加载正确，Stop/SubagentStop 的自动保存链路在 subprocess 形态下端到端工作，并覆盖成功、重复、拒绝和 nmem 失败。

#### 2. 对应需求与 Scenario

- Requirements：R1–R9
- Scenarios：S1–S10
- Decisions：D1–D5
- Evidence：E1–E9

#### 3. 外部可观察结果

测试能从真实 plugin script entry 看到：SessionStart recall 不受影响；Stop 和 SubagentStop 对同一 candidate 只写一次；audit 有 bounded outcome；nmem 失败不破坏 hook exit code；无 Ralph env 仍完全 no-op。

#### 4. 当前行为基线

已有 `test_ralph_bridge_e2e.py` 证明 worker SessionStart 和 SubagentStop audit subprocess 形态；已有 `test_hook_runtime.py`、`test_memory_writer.py` 和 security tests 分别覆盖入口、writer 和隐私边界。Unit 4 只组合这些真实模块，不重写已有契约。

#### 5. 输入与输出

- 输入：两种 hook event、合法/非法 payload、临时 plugin data、fake nmem runner。
- 输出：exit 0、bounded audit/result 文件、fake nmem argv/count、ledger 状态。
- 不变量：单 candidate 单 digest 单 remote write；任何失败不生成误报的 SAVED。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/tests/test_ralph_bridge_e2e.py` | subprocess bridge smoke | 增加 Stop/SubagentStop finalization E2E | 不接 Ralph Rust |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | hook contract | 组合成功/失败路径 | 不删旧测试 |
| `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py` | manifest/docs contract | 验证 hook manifest 仍声明两个 event | 不锁定全部文案 |

#### 7. 可依赖能力

Units 1–3 的 parser、coordinator 和文档 contract。

#### 8. 禁止依赖的未来能力

不新增 Ralph loop 集成测试、不改 Rust workspace 测试、不把 desktop watcher 当作本插件的测试 oracle。

#### 9. 验收测试

运行插件完整 pytest；额外运行临时目录 subprocess E2E，断言真实脚本入口、两个 event、fake nmem call count、ledger 和 audit。命令：`python3 -m pytest plugins/nowledge-mem-ralph/tests -q`，再运行 `python3 -m compileall -q plugins/nowledge-mem-ralph/scripts`。

#### 10. Acceptance Red

先运行新 E2E，在现有 audit-only 实现上预期“合法 candidate 产生 SAVED / fake call=1”失败；该 Red 必须来自未接入自动 finalization，而非 fixture 或环境错误。

#### 11. 单元测试拆分

1. Stop success。
2. SubagentStop success。
3. duplicate cross-event idempotency。
4. policy rejection。
5. nmem 402 / timeout / invalid JSON。
6. no Ralph env no-op。
7. transcript path safety。
8. audit/result boundedness。

#### 12. Red → Green → Refactor 顺序

先让 Stop E2E Red → 接入 coordinator → Green；
再让 SubagentStop E2E Red → 复用同一 handler → Green；
再让 duplicate/failure E2E Red → 接入现有 ledger/result handling → Green；
最后清理重复测试 helper，运行完整插件回归。

#### 13. 最小实现范围

只完成插件端到端闭环和测试收口；不新增产品能力，不改外部 watcher，不改 Ralph。

#### 14. 集成验证

真实 Python module import + subprocess hook entry + existing memory policy/writer；仅 nmem process 使用 fake。必须检查 JSONL 内容不含完整消息。

#### 15. 风险驱动测试

Idempotency、Fault Injection、Characterization 和 security regression 均有直接证据支持；不添加无证据的并发测试。若并发 Stop 在实际测试中复现 ledger race，再追加最小 concurrency test，否则不扩大范围。

#### 16. 回归范围

插件全量 pytest、compileall、manifest JSON parse、现有 SessionStart recall、memory policy、memory writer、security 和 bridge E2E。最终不要求 Rust workspace 全量测试，因为本计划明确不修改 Rust；若工作区验证流水线强制要求，再执行仓库规定的 `./scripts/run-tests.sh` 作为环境回归，但失败不能归因到本插件实现而跳过。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `plugins/nowledge-mem-ralph/tests/test_ralph_bridge_e2e.py` | 修改测试 | 端到端自动 finalization | E7、E8 |
| `plugins/nowledge-mem-ralph/tests/test_hook_runtime.py` | 修改测试 | 组合成功/失败路径 | E7 |
| `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py` | 修改测试 | manifest/文档 contract | E9 |

#### 18. 完成标准

全部 plugin tests、compileall、manifest checks 通过；无 direct nmem path、无 raw transcript read、无新增 skip/only、没有改动 Ralph loop；每个前置 Unit 已完成完整 TDD 闭环。

#### 19. 停止条件

发现真实安装 manifest 与测试入口不一致、插件脚本需要 Ralph Rust 改动、或成功结果无法区分远端成功与 UNKNOWN 时停止，不得把 E2E 改成 source-only assertion。

#### 20. 风险与注意事项

desktop watcher 可能同时写 Thread，但不应影响 plugin writer 的 digest ledger；E2E 只能断言 plugin-owned fake nmem call 和本地 audit，不能把 Thread 数量当 Memory 成功指标。

## Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `plugins/nowledge-mem-ralph/hooks/hooks.json` | 已声明 `SessionStart`、`Stop`、`SubagentStop`，均调用 `scripts/hook_runtime.py` | 自动行为应扩展现有 hook，不新增第三套入口 | 高 |
| E2 | `scripts/hook_runtime.py::_handle_stop` | 当前 Stop handler docstring 明确 audit-only、无 transcript、无 nmem | 直接解释插件为何没有自动 Memory | 高 |
| E3 | `scripts/audit_hook.py`、`tests/test_hook_runtime.py::test_stop_audit_placeholder` | 当前审计只记录状态，测试锁定不读 transcript | 新实现必须改为只读受限 message marker，不能放开 transcript | 高 |
| E4 | `scripts/memory.py`、`commands/save-memory.md`、`skills/save-memory/SKILL.md` | 已存在 schema → policy → dedupe 的 save entry；accepted record 才交 writer | 不新建 Memory policy 或 writer | 高 |
| E5 | `scripts/memory_writer.py`、`tests/test_memory_writer.py` | writer 已有 accepted-only、ledger lock、digest dedupe、SAVED/UNKNOWN/FAILED_OPEN | 自动 hook 只调用现有 writer，复用幂等和故障语义 | 高 |
| E6 | `scripts/memory_evaluator.py` | semantic evaluator 为可选配置且失败 closed | 自动流程不能把 evaluator 失败伪装成保存成功 | 高 |
| E7 | `tests/test_ralph_bridge_e2e.py` fixture | Stop/SubagentStop 真实 payload 包含 `last_assistant_message`；现有测试已能 subprocess 调 hook | marker parser 的输入字段有直接 fixture 证据 | 高 |
| E8 | `crates/ralph-adapters/src/cli_executor.rs`、`pty_executor.rs` tests | Ralph adapter 已注入 `RALPH_NOWLEDGE_ENABLED=1` 等环境 | 插件-only 方案可复用已有 env，不需改 Ralph loop | 高 |
| E9 | `plugin.json`、`README.md` | plugin manifest 已挂载 hooks；README 当前明确 Stop audit-only/no save | 需要同步文档和稳定 contract tests | 高 |
| E10 | 2026-08-08 nmem feed/threads/memories 实际诊断 | 当日 Thread 有新增，但新 Memory 为 0；feed 中 insight/skill 处理出现 402 | 现实结果符合“Thread watcher 在工作、插件 Memory writer 未触发/外部服务失败”的分离现象；测试需区分本地触发和远端成功 | 高 |

## Decision Ledger

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 自动入口放在哪里 | 改 Ralph loop；改 Claude Stop/SubagentStop；新增 watcher bridge | 只改现有 Claude Stop/SubagentStop hook | E1、E2、E7、E8 | 用户明确不改 Ralph；插件已有 hook 且 payload 有 message；watcher 不属于本插件 | 0.97 |
| D2 | 候选输入是否读取 transcript | 读 `transcript_path`；读完整 message；只读受限 marker | 只读 `last_assistant_message` 中的单一 bounded marker | E3、E7、README privacy contract | transcript 会破坏当前隐私边界；完整 message 无法区分过程和终态；marker 可验证且不需文件访问 | 0.94 |
| D3 | Memory 写入路径 | hook 直接运行 nmem；复制 writer；复用现有 save→writer | 复用 `memory.py::save` → `memory_writer.py::write_from_save_result` | E4、E5 | 直接 nmem 绕过 policy；复制 writer 会产生双 ledger 和幂等漂移 | 0.98 |
| D4 | 自动失败语义 | hook 失败阻塞 Claude；静默忽略；沿用现有 result 并 fail-open | 记录 bounded result，hook fail-open，保留 UNKNOWN/FAILED_OPEN | E5、E6、E9 | Memory 外部服务不是 Claude 主流程；阻塞会把 402 变成 session failure；静默会无法诊断 | 0.92 |
| D5 | 是否自动把每个 Thread 转 Memory | 全量 Thread；最终 assistant message；显式结构化终态 candidate | 只保存 `finalize:true` 的结构化 candidate | E4、E9、E10 | Memory policy 禁止 transcript/log/progress；用户误把 Thread 当 Memory 正是当前 gap | 0.96 |

## BDD 行为规格

### Feature: Claude hook 自动提交结构化 Memory candidate

  Background:
    Given plugin manifest 已声明 Stop 和 SubagentStop hook
    And `RALPH_NOWLEDGE_ENABLED=1` 与合法 `RALPH_CURRENT_LOOP_ID` 存在
    And现有 memory policy 与 writer 可被测试替身调用

  Scenario: 合法终态 candidate 自动保存
    Given last_assistant_message 包含一个 `finalize:true` 的合法 Memory JSON marker
    When Stop hook 被调用
    Then candidate 通过现有 memory policy
    And writer 只调用一次 nmem
    And audit 记录 `SAVED` 与 digest

  Scenario: 普通最终消息不产生 Memory
    Given last_assistant_message 没有 marker
    When Stop hook 被调用
    Then不调用 nmem
    And audit 记录 `SKIPPED`

  Scenario: 非终态 marker 不产生 Memory
    Given marker 的 `finalize` 为 false
    When SubagentStop hook 被调用
    Then不调用 nmem
    And不产生 `SAVED`

  Scenario: 非法 candidate 被拒绝
    Given marker JSON 缺少固定 schema 字段或包含非法 Memory type
    When Stop hook 被调用
    Then现有 policy 返回 `REJECTED`
    And writer 不被调用
    And hook exit code 仍为 0

  Scenario: 重复 Stop 幂等
    Given同一 candidate 已经返回 `SAVED`
    When相同 digest 的 SubagentStop 再次被调用
    Then返回 `ALREADY_SAVED`
    And nmem 调用次数仍为一次

  Scenario: nmem 外部失败不破坏 session
    Given policy 接受 candidate
    And nmem 返回 402、超时或无法解析 JSON
    When Stop hook 被调用
    Then保留现有 `FAILED_OPEN` 或 `UNKNOWN` 语义
    And hook exit code 为 0
    And不记录虚假的 `SAVED`

  Scenario: 缺少 Ralph env 时保持 no-op
    Given hook payload 有合法 marker
    And Ralph env 不存在
    When Stop hook 被调用
    Then不写插件状态
    And不调用 nmem

  Scenario: transcript 永不被读取
    Given `transcript_path` 指向敏感 fixture
    And last_assistant_message 只含合法 marker
    When Stop hook 被调用
    Then敏感 fixture 未被打开
    And audit 不包含 transcript 内容

## Verification Contract

### Scenario-to-test matrix

| Scenario | 验收测试入口 | 核心断言 | 层级 | E2E |
|---|---|---|---|---|
| S1 | `test_hook_runtime.py` + bridge E2E | accepted → writer once → `SAVED` | integration | 是 |
| S2/S3 | `test_hook_runtime.py` | no marker/false → no writer | integration | 是 |
| S4 | `test_memory.py` + hook integration | policy reject never reaches writer | integration | 否 |
| S5 | `test_memory_writer.py` + hook integration | duplicate digest → `ALREADY_SAVED`, one call | integration | 是 |
| S6 | `test_memory_writer.py` + hook integration | 402/timeout/invalid JSON preserve failure state | fault injection | 否 |
| S7 | `test_hook_runtime.py` | missing env no-op | characterization | 是 |
| S8 | `test_security_contracts.py` + hook test | no transcript open/content persistence | security | 是 |
| S9 | `test_plugin_contract.py` | docs describe marker and Thread/Memory boundary | contract | 否 |
| S10 | `test_ralph_bridge_e2e.py` | SubagentStop uses same bounded path | integration | 是 |

### Requirement traceability

| Requirement ID | Requirement | Scenario | Acceptance test | Unit | Evidence |
|---|---|---|---|---|---|
| R1 | Stop/SubagentStop 自动触发 | S1/S10 | hook subprocess E2E | U1/U2/U4 | E1/E7 |
| R2 | bounded marker、无 transcript | S8 | security + parser tests | U1/U4 | E3/E7 |
| R3 | finalize true | S2/S3 | parser tests | U1/U3 | E4 |
| R4 | 复用 save→writer | S1/S4 | coordinator integration | U2 | E4/E5 |
| R5 | digest 幂等 | S5 | writer fake runner | U2/U4 | E5 |
| R6 | fail-open | S6/S7 | fault/no-env tests | U2/U4 | E5/E6 |
| R7 | bounded audit | S1/S6/S8 | JSONL assertions | U2/U4 | E3/E5 |
| R8 | non-Ralph no-op | S7 | characterization | U1/U2/U4 | E1/E7 |
| R9 | 文档契约 | S9 | stable anchor tests | U3/U4 | E9 |

### Unit serial dependency graph

```text
Unit 1
  ↓ parser 和安全边界已验证
Unit 2
  ↓ 自动保存链路和幂等/失败语义已验证
Unit 3
  ↓ agent-facing marker contract 与实现一致
Unit 4
```

Unit 2 不能先于 Unit 1，因为 writer 不应接收未经边界验证的输入；Unit 3 不能先于 Unit 2，因为文档必须描述已经存在的行为；Unit 4 必须最后执行，以避免 E2E 在基础 contract 尚未稳定时掩盖单元失败。即使 Unit 1/2 的部分测试可以技术上并行，也禁止并行提交或交替开发。

### Commands

| 时机 | 命令 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| 每个 Unit Red/Green | `python3 -m pytest <当前相关测试> -q` | 验证真实 Red 与最小 Green | 只出现目标失败/通过 | 非目标失败必须停止 |
| Unit 1 | `python3 -m pytest plugins/nowledge-mem-ralph/tests/test_hook_runtime.py plugins/nowledge-mem-ralph/tests/test_ralph_bridge_e2e.py -q` | parser + hook bridge | 通过 | 不得进入 U2 |
| Unit 2 | `python3 -m pytest plugins/nowledge-mem-ralph/tests/test_hook_runtime.py plugins/nowledge-mem-ralph/tests/test_memory.py plugins/nowledge-mem-ralph/tests/test_memory_writer.py -q` | save/writer/ledger | 通过 | 不得进入 U3 |
| Unit 3 | `python3 -m pytest plugins/nowledge-mem-ralph/tests/test_plugin_contract.py plugins/nowledge-mem-ralph/tests/test_security_contracts.py -q` | docs/security contract | 通过 | 回到 U3 修正 |
| Unit 4 | `python3 -m pytest plugins/nowledge-mem-ralph/tests -q` | 插件全量回归 | 通过且无 skip | 不得宣称完成 |
| Unit 4 | `python3 -m compileall -q plugins/nowledge-mem-ralph/scripts` | Python syntax/build check | 通过 | 修复后重跑 |
| 最终 | `git diff --check` | whitespace/diff hygiene | 无输出 | 修复后重跑 |
| 最终可选工作区门禁 | `./scripts/run-tests.sh` | 若项目交付流程要求全 workspace 回归 | 按仓库 nextest 规则通过 | Rust 失败需区分环境/范围，不得跳过插件测试 |

本计划不要求新增契约测试、E2E 服务或 TypeScript；仓库当前相关插件测试使用 pytest，Rust workspace 不是本计划的修改对象。

## Definition of Done

- 所有 S1–S10 均有可执行测试和对应 Unit；
- 每个 Unit 完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close；
- Stop 与 SubagentStop 的合法 candidate 会真正进入现有 save-memory → writer 链路；
- accepted-only、digest dedupe、`FAILED_OPEN`、`UNKNOWN`、policy rejection 语义未被削弱；
- 无 Ralph loop/runtime/preset/config Rust 文件变更；
- 不读取 transcript、不写 raw Thread、不把 desktop watcher 结果当插件 Memory 结果；
- 插件完整 pytest、compileall、manifest/contract 检查通过；
- 没有新增 `.skip`、`.only`、忽略标记、弱化断言或无解释 snapshot/golden 更新；
- 审计中没有完整 assistant message、transcript 内容或敏感文件内容；
- 所有 Decision 置信度保持 ≥0.85；
- 计划内文件变更与本文件一致，每个 Unit 可独立提交。

## Final self-check

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指向真实插件入口、输入、Red、测试和完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D5 已固定插件-only、marker、writer、失败和范围边界 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E9；新增 parser/coordinator 明确标为新增插件模块 |
| 所有关键决策置信度是否 ≥0.85 | 是 | D1–D5 为 0.92–0.98 |
| 是否存在未处理的低置信度假设 | 否 | A1–A3 都有 Unit 1/2 的验证动作和停止条件 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 分别是解析边界、自动写入、agent contract、端到端收口 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有独立 pytest 命令和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 每个 Unit 明确当前 audit-only/文档缺失导致的失败 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 16 节列出直接和相邻插件回归 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图仅允许 U1→U2→U3→U4，当前 Unit 不实现未来行为 |
| 是否存在泛化任务描述 | 否 | 未使用“完善逻辑/增加测试”等孤立描述 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | Scenario-to-test matrix 与 traceability matrix |
| 所有关键决策是否有 Evidence | 是 | Decision Ledger 绑定 E1–E10 |
| 计划是否可以严格串行执行 | 是 | 明确 U1→U2→U3→U4，失败即停止 |
