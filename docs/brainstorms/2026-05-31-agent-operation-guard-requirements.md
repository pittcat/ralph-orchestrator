---
date: 2026-05-31
topic: agent-operation-guard
status: source-verified
---

# Agent 操作防护体系 (Agent Operation Guard)

## Summary

在 Ralph 源码层面补齐 Agent 可调用操作的授权和完整性防护，重点覆盖 runtime tasks、memories、human-in-the-loop、wave、loop 管理、event file 写入、event history 清理、progress 消息和 skill 读取。

本需求文档不再把 event-origin-guard 作为本次新增范围。当前工作区已经有 event-origin 相关实现改动，剩余需求应建立在该层完成并通过测试之后。本需求只保留对 event-origin 的前置条件和回归测试要求。

## Source Verification

以下核验基于 2026-05-31 当前工作区源码。工作区已有未提交的 event-origin 相关改动，因此本表按“当前源码状态”判断，不按历史 main 分支判断。

| Area | Verification | Source Evidence | Requirement Impact |
|---|---|---|---|
| Event origin guard | 部分已实现，不能再作为 004 的主要功能范围 | `crates/ralph-core/src/event_origin.rs`, `crates/ralph-core/src/hat_registry.rs`, `crates/ralph-cli/src/main.rs`, `crates/ralph-cli/src/wave.rs` | 作为 004 前置条件和回归测试 |
| `ralph emit --ts` | 当前 `EmitArgs` 已无 `--ts` 字段，timestamp 由 `chrono::Utc::now()` 生成 | `crates/ralph-cli/src/main.rs` | 原“移除 --ts”需求已过期，只保留文档/补全回归核验 |
| Wave provenance stamping | 当前 `write_wave_events()` 已读取 `RALPH_CURRENT_HAT` 并写入 `hat` 字段，但没有在 CLI 写入前验证 env 是否可信 | `crates/ralph-cli/src/wave.rs` | 需求应改为“入口验证声明的 hat 和 topic”，不能声称可验证 env 来源 |
| Wave origin validation | `event_loop` 引用了 `filter_wave_dispatch_by_origin`，但当前 `event_origin.rs` 未定义该函数 | `crates/ralph-core/src/event_loop/mod.rs`, `crates/ralph-core/src/event_origin.rs` | 003 必须先修到可编译状态，004 不应建立在破损依赖上 |
| Control topic allowlist | `validate_event_origin()` 对注册 hat 先执行 `can_publish()`，再检查 control topic allowlist，注释与行为不一致 | `crates/ralph-core/src/event_origin.rs` | 003 前置修复，防止 `human.interact` 等被误拒 |
| Task cross-loop guard | Task 有 `loop_id`，创建时会从 `.ralph/current-loop-id` 写入，但 `start/close/fail/reopen` 不检查目标 task loop | `crates/ralph-core/src/task.rs`, `crates/ralph-cli/src/task_cli.rs` | 缺口真实存在 |
| Task cross-hat guard | Task 没有 owner hat 字段，操作命令不读取 `RALPH_CURRENT_HAT` | `crates/ralph-core/src/task.rs`, `crates/ralph-cli/src/task_cli.rs` | 缺口真实存在 |
| Task key loop scope | `TaskStore::ensure()` 只按 key 去重，不区分 loop | `crates/ralph-core/src/task_store.rs` | 缺口真实存在 |
| Memory hat isolation | `Memory` 没有 owner hat 字段，CLI list/show/search/delete/prime 不按 hat 过滤 | `crates/ralph-core/src/memory.rs`, `crates/ralph-cli/src/memory.rs` | 缺口真实存在，但必须保留 shared memory 语义 |
| Payload evidence validation | `parse_backpressure_evidence()` 和 `parse_review_evidence()` 已存在，但为字符串包含式解析，缺少结构化语义校验 | `crates/ralph-core/src/event_parser.rs`, `crates/ralph-core/src/event_loop/mod.rs` | 原“函数不存在”说法不成立，需求改为“加强现有解析器” |
| Human response forgery | `TelegramService::wait_for_response()` 轮询 JSONL 中的 `human.response`，Agent 可写入同 topic 伪造响应 | `crates/ralph-telegram/src/service.rs` | 缺口真实存在 |
| Human interact payload | event loop 会解析 `human.interact` payload，但缺少明确的非空字符串/结构校验失败路径 | `crates/ralph-core/src/event_loop/mod.rs` | 缺口真实存在 |
| Loop operation auth | `loops logs/history/diff/stop/discard/attach/merge` 未基于 current loop 或 owner hat 做授权 | `crates/ralph-cli/src/loops.rs`, `crates/ralph-core/src/loop_registry.rs` | 缺口真实存在，但必须区分 Agent 上下文和人工 CLI |
| Emit target path | `ralph emit` 可从 `RALPH_EVENTS_FILE`、marker 或 `--file` 解析写入路径，当前没有将最终路径限制为当前 loop marker 指向文件 | `crates/ralph-cli/src/main.rs` | 缺口真实存在，需同时防 CLI flag 和 env override |
| Events clear | 当前是 `ralph events --clear`，没有 `--confirm` 或 loop_id 确认 | `crates/ralph-cli/src/main.rs` | 缺口真实存在，需求应使用当前 CLI 形态 |
| Interact progress | `send_progress()` 只发送原始消息，没有长度、频率、来源标记 | `crates/ralph-cli/src/interact.rs` | 缺口真实存在，但无法验证“内容是否真实完成” |
| Skill access | `SkillRegistry` 已支持 `hats` 过滤，但 CLI load/list 使用 `skills_for_hat(None)`，绕过当前 hat 限制 | `crates/ralph-core/src/skill_registry.rs`, `crates/ralph-cli/src/skill_cli.rs` | 缺口真实存在，可优先复用现有 `hats` 元数据 |

## Corrected Problem Frame

Ralph 的 orchestration loop 依赖 Agent 自主调用本地 CLI。Agent 可以写事件、操作 runtime tasks、读写 memories、触发 wave、发送 progress、读取 skills，以及操作 loops。event-origin guard 解决的是“JSONL 事件是否能进入可信事件流”的问题，但 Agent 仍可以通过其他 CLI surface 修改共享状态或伪造人类响应。

真实存在的问题是：

1. **Runtime task 操作缺少 loop 和 hat 授权**：任务创建时已有 `loop_id`，但生命周期操作不验证；任务没有 owner hat 信息。
2. **Task key 和 blocked_by 未按 loop 隔离**：`ensure()` 全局按 key 复用，`blocked_by` 未验证同 loop。
3. **Memory 系统缺少 owner/visibility 元数据**：所有 hat 都能 show/search/delete/prime 所有 memories。
4. **Human response 可通过 JSONL 伪造**：等待响应逻辑信任事件文件里的 `human.response`。
5. **Payload evidence 解析太宽松**：已有 parser 只做字符串包含判断，不能区分结构化证据、文件路径、状态枚举和数值边界。
6. **Wave provenance 只能作为声明，不能信任 env 本身**：`RALPH_CURRENT_HAT` 可由进程设置，因此必须在 ingestion 端验证声明 hat 已注册且可发布该 wave dispatch topic。
7. **Loop 操作缺少 Agent 上下文授权**：Agent 可停止、读取或 discard 其他 loop；人工 CLI 使用不能被同等锁死。
8. **Event file 写入路径可被重定向**：`--file` 和 `RALPH_EVENTS_FILE` 都可能指向当前 loop 之外。
9. **Event history 清理缺少显式确认**：`ralph events --clear` 可直接清除当前检测到的事件文件。
10. **Progress 消息缺少基本 guard**：无法证明消息内容真实，但可以限制空消息、超长、频率和来源标记。
11. **Skill CLI 未应用已有 hat visibility**：注册表有 `hats` 限制，但 CLI load/list 未传入 current hat。

不成立或需要降级的原问题表述：

- “验证 `RALPH_CURRENT_HAT` 环境变量来源是否可信”无法在普通子进程 CLI 中可靠完成。可实现的是：把 env 当作声明，在事件入口和命令授权层用 registry/current-loop marker 校验。
- “interact progress 必须验证消息内容来自真实任务进展”无法靠静态 CLI 判断。可实现的是：标记来源、频率限制、长度限制和 diagnostics 记录。
- “parse_backpressure_evidence/parse_review_result 不存在”不成立。现有函数存在，问题是解析模型不够结构化。
- “所有 memory 默认只读写自己 hat”会破坏 Ralph 作为跨 iteration 学习系统的设计。正确模型应支持 `shared` 和 `hat_private`，legacy memory 默认 shared。

## Actors

- A1. **Active Hat**：当前 iteration 中被选中的 hat，通过 `RALPH_CURRENT_HAT` 注入到 Agent 进程。
- A2. **Wave Worker**：由 wave dispatch 启动的并发 Agent 实例，带 `RALPH_WAVE_WORKER=1`、`RALPH_WAVE_ID`、`RALPH_WAVE_INDEX`。
- A3. **Task Owner Hat**：创建 runtime task 的 hat，记录为 `owner_hat_id`。
- A4. **Loop Owner**：创建 loop 的进程/hat/工作区组合，记录在 loop registry。
- A5. **Human Operator**：通过 Telegram 或本地人工 CLI 监督 Ralph。
- A6. **CLI Caller**：执行 `ralph` 命令的进程。若设置了 `RALPH_CURRENT_HAT` 或 loop runtime env，则视为 Agent 上下文；否则视为人工 CLI 上下文。
- A7. **Telegram Service**：可信 human response 来源，不能与 Agent 写入的 JSONL 同信任等级。

## Requirements

### R0. Event-Origin 前置条件和回归保护

- R0.1. 004 实施前，当前工作区必须能编译；`filter_wave_dispatch_by_origin` 的引用和定义必须一致。
- R0.2. `validate_event_origin()` 必须先允许 control topic，再对普通业务 topic 执行 `can_publish()`，避免注册 hat 的 `human.interact` 被误拒。
- R0.3. `ralph emit` 不得重新引入 `--ts`；zsh completion 和 CLI reference 不得提及 `--ts`。
- R0.4. `ralph wave emit` 写入的 `hat` 只作为声明，必须由 ingestion guard 验证注册状态和发布范围。
- R0.5. ce-executor 和 wave-review 的正常 wave dispatch 不得因 004 改动回归。

### R1-R8. Task Operation Guard

- R1. Task 数据模型增加 `owner_hat_id: Option<String>`，创建时从 `RALPH_CURRENT_HAT` 写入；无 hat 上下文创建的 task 为空。
- R2. Task 生命周期操作 `start/close/fail/reopen` 必须先加载目标 task，再验证 `task.loop_id == current_loop_id`。
- R3. 如果当前存在 `.ralph/current-loop-id`，目标 task 缺少 `loop_id` 时不得被 Agent 上下文操作，除非显式人工 CLI override。
- R4. Agent 上下文只能操作 `owner_hat_id` 等于当前 hat 的 task。
- R5. 跨 hat 操作只能由配置中明确授权的 coordinator hats 执行，不能靠名称硬编码。
- R6. `task ensure` 的 key 去重必须按 `(loop_id, key)` 匹配；不同 loop 的同名 key 应创建不同 task。
- R7. `blocked_by` 中的 blocker task 必须存在且与当前 task 同属一个 loop；跨 loop blocker 必须拒绝。
- R8. `task ready` 默认继续只显示当前 loop task，`--all` 保留人工诊断用途。

### R9-R16. Memory Operation Guard

- R9. Memory 数据模型增加 `owner_hat_id: Option<String>` 和 `visibility`，visibility 至少支持 `shared` 与 `hat_private`。
- R10. 现有 legacy memory 没有 owner/visibility 时视为 `shared`，保证跨 session 学习不被切断。
- R11. Agent 创建 memory 时默认 `visibility=shared`，除非 CLI 提供 `--private` 或配置指定默认 private。
- R12. `memory delete <id>` 在 Agent 上下文只能删除自己 hat 创建的 memory；shared legacy memory 只能由人工 CLI 或明确授权 hat 删除。
- R13. `memory show/search/list/prime` 在 Agent 上下文默认返回 shared + 当前 hat private memory，不返回其他 hat private memory。
- R14. `memory add` 必须拒绝空内容和超过阈值的内容，默认阈值为 10000 chars。
- R15. Memory flooding guard 必须按 owner hat 计数，默认每 hat private memory 上限 1000；shared memory 上限单独配置。
- R16. Markdown storage 格式必须能 round-trip 新元数据；旧格式必须继续可解析。

### R17-R22. Payload Evidence Guard

- R17. 加强现有 `EventParser::parse_backpressure_evidence()`，支持结构化 JSON evidence，并保留当前字符串证据解析作为兼容输入。
- R18. build evidence 必须验证必需检查项、状态枚举、数值范围；文件路径类 evidence 若出现，必须验证路径存在且在 workspace 内。
- R19. `build.done` evidence 缺失或非法时生成 `build.blocked`，原因必须区分 missing、invalid-status、path-not-found、out-of-range。
- R20. 加强 `parse_review_evidence()` 或新增结构化 review parser，验证 tests/build 和 review dimensions。
- R21. `review.done` 非 wave 事件缺失或非法验证证据时继续生成现有 `review.blocked`，不要在同一系统里混用新的 `review.incomplete` topic，除非同步更新 presets 和 docs。
- R22. Wave worker 的 `review.done` 继续走现有豁免路径，避免要求每个只读 dimension reviewer 跑 build/tests。

### R23-R28. Human Interaction Integrity

- R23. `human.interact` payload 必须是非空字符串或包含非空 `question` 字段的 JSON object。
- R24. 无效 question 不得发送到 Telegram；loop 应记录 `human.interact.invalid` 或等价 diagnostics，并继续可恢复。
- R25. Telegram active 时，`wait_for_response()` 不得信任 Agent 可写的 JSONL `human.response`。
- R26. 可信 response 应来自 Telegram polling handler 到 waiting loop 的专用通道，或带不可由 Agent 构造的进程内 token/source marker。
- R27. Agent 直接写入 JSONL 的 `human.response` 必须被 origin/policy 层拒绝或在 wait path 忽略。
- R28. 无 Telegram 或测试 fallback 模式若继续使用 JSONL polling，必须显式标记为 degraded mode，并用测试限制只在 mock/test 配置启用。

### R29-R34. Wave Guard

- R29. `ralph wave emit` 在 wave worker 内继续拒绝 nested wave。
- R30. Wave dispatch event 必须带 `wave_id`、`wave_index`、`wave_total`，且 `wave_index < wave_total`。
- R31. Wave dispatch event 在执行 worker 前必须验证 declared `hat` 已注册。
- R32. Declared hat 必须被允许发布 wave dispatch topic。若该 topic 语义上是“触发目标 worker”，仍应由 dispatching hat 的 `publishes` 声明，而不是目标 worker 的 `triggers` 代替。
- R33. 无 hat registry 的 solo mode 可继续允许无 hat wave，用于测试和 hatless baseline。
- R34. Wave worker result 由 `ralph emit` 自动带上 `wave_id` 和 `wave_index`，并由 tracker 拒绝重复 index 或 unknown wave。

### R35-R42. Loop Operation Guard

- R35. `LoopEntry` 增加 owner metadata，至少包含 `owner_hat_id: Option<String>` 和 `workspace` 已有字段。
- R36. Agent 上下文调用 `loops logs/history/diff <loop_id>` 时，只能访问当前 loop 或配置允许的 shared loop。
- R37. 人工 CLI 上下文保留诊断能力，但 destructive 操作必须有显式确认。
- R38. Agent 上下文调用 `loops stop <loop_id>` 只能停止当前 loop；停止其他 loop 必须拒绝。
- R39. `loops discard <loop_id>` 必须验证 owner 或人工确认；Agent 不得 discard 非自己 owner 的 loop。
- R40. `loops attach <loop_id>` 在 Agent 上下文默认拒绝；人工 CLI 必须记录 diagnostics。
- R41. `loops merge <loop_id>` 必须保留已有 queue state 检查，禁止 merged/discarded/running loop 被误合并。
- R42. loop authorization helper 必须集中实现，避免每个 subcommand 写不同规则。

### R43-R48. Emit File Guard 和 Event History Guard

- R43. `ralph emit` 最终解析出来的 events file 必须等于 `.ralph/current-candidate-events` 或 `.ralph/current-events` marker 指向路径；无 marker 时只能写 `.ralph/events.jsonl`。
- R44. 上述限制必须同时覆盖 CLI `--file` 和 `RALPH_EVENTS_FILE` env，不能只检查 `--file`。
- R45. 路径比较必须使用 canonical/normalized path，防止 `..` 或 symlink 绕过。
- R46. `ralph events --clear` 必须新增确认参数，例如 `--confirm <loop_id>`，无确认时拒绝。
- R47. `--confirm <loop_id>` 必须匹配 `.ralph/current-loop-id` 或目标事件文件所属 loop。
- R48. 清理前必须写 diagnostics 或 tracing log，至少包含 caller context、target path、loop_id。

### R49-R52. Interact Progress Guard

- R49. `ralph tools interact progress` 必须拒绝空消息和超过 2000 chars 的消息。
- R50. Progress 消息必须自动追加或前置 Agent 来源标记，例如 `[via Ralph agent]`。
- R51. Progress 发送必须限频，默认同一进程 5 秒内只能发送一次；跨进程限频可用 `.ralph/telegram-progress-rate-limit` marker。
- R52. 系统不得声称已验证 progress 内容真实完成；只能在 diagnostics 中记录可疑模式或过频行为。

### R53-R57. Skill Access Guard

- R53. `ralph tools skill list/load` 必须读取当前 hat context，并传给 `SkillRegistry::skills_for_hat(Some(hat))`。
- R54. 若 skill frontmatter 或 config override 设置 `hats`，非匹配 hat 不得 list 或 load。
- R55. 无 hat 的人工 CLI 可 list/load 全部 skill，用于诊断；Agent 上下文缺少 hat 时应 fail closed。
- R56. 如需 “restricted skill” 概念，应优先用现有 `hats` allowlist 表达；只有现有 metadata 不足时才新增 `restricted` 字段。
- R57. CLI error message 只能列出当前 caller 可见的 skills，不能把隐藏 skill 名称泄露给未授权 hat。

### R58-R64. Documentation and Adaptation

- R58. 修改 `ralph tools task` 行为时，更新 `crates/ralph-core/data/ralph-tools-tasks.md`。
- R59. 修改 `ralph tools memory` 行为时，更新 `crates/ralph-core/data/ralph-tools-memories.md` 和 shared `ralph-tools.md`。
- R60. 修改 `ralph tools interact` 或 `skill` 行为时，更新 `crates/ralph-core/data/ralph-tools.md`。
- R61. 修改 `ralph emit`、`ralph events`、`ralph wave`、`ralph loops` CLI 参数时，更新 `docs/guide/cli-reference.md`。
- R62. 新增 config 字段时，更新 `docs/guide/configuration.md`。
- R63. 若改动 builtin presets 或 mirrored preset files，必须同步 `presets/`、`crates/ralph-cli/presets/` 和 `scripts/ralph-zsh-plugin.zsh`；本需求优先不改 preset YAML。
- R64. 修改 zsh completion 后，按项目要求安装到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` 并验证加载。

## Acceptance Examples

- AE1. Given loop-a task and current-loop-id loop-b, when Agent runs `ralph tools task close <task>`, command fails with cross-loop authorization error.
- AE2. Given task owned by `review-coordinator`, when `executor` runs `task fail <task>`, command fails unless executor is explicitly configured as task coordinator.
- AE3. Given existing task key `fix:lint` in loop-a, when loop-b runs `task ensure --key fix:lint`, a new loop-b task is created.
- AE4. Given memory with `visibility=hat_private` and `owner_hat_id=reviewer`, when executor runs `memory search`, that memory is absent.
- AE5. Given legacy memory with no owner metadata, when any hat runs `memory prime`, the memory remains visible as shared.
- AE6. Given `build.done` with JSON evidence referencing a non-existent file path, event loop emits `build.blocked` with `path-not-found`.
- AE7. Given Agent writes fake JSONL `human.response`, when Telegram service is active, waiting loop ignores or rejects it and times out unless Telegram actually responds.
- AE8. Given `RALPH_WAVE_WORKER=1`, `ralph wave emit` fails with nested wave error.
- AE9. Given `RALPH_EVENTS_FILE=.worktrees/loop-b/.ralph/events.jsonl`, when current marker points to loop-a, `ralph emit` rejects the path.
- AE10. Given `ralph events --clear` without confirm, command fails and does not modify events file.
- AE11. Given Agent on loop-a runs `ralph loops stop loop-b`, command fails.
- AE12. Given skill with `hats: [reviewer]`, when executor runs `ralph tools skill load <skill>`, command fails and does not reveal hidden skill content.
- AE13. Given progress messages sent twice within 5 seconds, second call fails with rate-limit error.
- AE14. Given human CLI with no `RALPH_CURRENT_HAT`, `ralph loops logs <loop-id>` still works for diagnostics.

## Success Criteria

- Runtime task lifecycle operations are isolated by loop and owner hat.
- Memories support shared learning while preventing one hat from reading/deleting another hat's private memories.
- Agent-authored JSONL cannot satisfy a Telegram human response wait.
- Evidence payload validation rejects malformed or meaningless evidence without breaking wave reviewer results.
- Wave dispatch remains usable in ce-executor and wave-review while fake or malformed wave events are rejected.
- Agent context cannot write to another loop's events file or clear event history without explicit confirmation.
- Skill CLI respects existing hat visibility rules.
- Documentation, tool skills, CLI reference, configuration docs, and zsh completion are updated wherever behavior changes.
- Full verification includes `cargo test -- --test-threads=1`, smoke tests for affected Ralph core flows, and targeted CLI tests listed in the plan.
