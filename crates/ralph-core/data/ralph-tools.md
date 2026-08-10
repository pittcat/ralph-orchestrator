---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---

# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

## Runner 注入的环境变量

当 `ralph run` 启动一个 hat 时，子进程会自动获得以下环境变量：

| 变量 | 含义 | 何时为空 |
|------|------|----------|
| `RALPH_CURRENT_HAT` | 当前激活的 hat id，例如 `worker`、`reviewer` | 非 loop 场景下直接调用 `ralph emit` 时 |
| `RALPH_CURRENT_LOOP_ID` | 当前 loop id | 同上 |
| `RALPH_EVENTS_FILE` | 当前 loop 的事件文件路径 | 同上 |
| `RALPH_TRIGGERED_HAT` | `ralph emit` 时默认的 `triggered` 回退值 | isolated 模式下 runner 不再注入；多消费者/无明确下游 topic 时也可能为空 |
| `RALPH_HATS_SOURCE` | hats 来源标签，例如 `builtin:<preset-name>` | 无预设时 |

这些变量由 runner 在每次 hat activation 前注入，agent 可直接读取，但**不要**假设某个变量一定非空。

## AI 决策速查：我现在该做什么？

| 场景 | 下一步 | 参考 |
|------|--------|------|
| Loop 拒收事件 / 收到 `task.resume` | 读下方「收到 `task.resume` 时」段 + `ralph-tools-recovery-directives`；用 `ralph emit --schema <TOPIC>` 预检字段 | 本 skill + `ralph tools skill load ralph-tools-recovery-directives` |
| 要发射单个事件或预检 schema | `ralph emit <TOPIC> --policy-check -j '{...}'` | `ralph tools skill load ralph-tools-emit` |
| loop 里出现 `.proposed` / `.rejected` / `precheck-*` gate | 读 topic 表，勿手 emit 衍生 topic | `ralph tools skill load ralph-tools-precheck` |
| 要派发 review wave / 作为 wave worker 返回 | `ralph wave emit <TOPIC> --payloads-stdin` | `ralph tools skill load ralph-tools-wave` |
| 要管理 runtime task | `ralph tools task add/ensure/start/close` | `ralph-tools-tasks`（已注入，若 tasks.enabled） |
| 要记录/查找记忆或 decision journal | `ralph tools memory add/search/prime` | `ralph-tools-memories`（已注入，若 memories.enabled） |
| 要预览 profiles 或查阅低频 CLI 命令 | `ralph inspect profiles` | `ralph tools skill load ralph-tools-cmdref` |
| 要落盘 builtin artifact 填写模板（binary-only；部署机无源码 templates 目录时） | `ralph preset materialize-artifacts <preset> --plan-key <key>` | `ralph tools skill load ralph-tools-cmdref`「materialize-artifacts」 |
| 要校验 hat 拓扑 | `ralph hats validate [--strict]` | `ralph hats --help`（strict 时启用 lint 所有权检查） |
| Loop 崩溃/ledger 损坏需恢复 | `ralph loops clean --ledger` + `ralph diagnose --session latest` | `docs/guide/runtime-diagnosis.md`（JSON 含 `dup_storm_topics` + findings `hint`） |
| `ralph emit` 报 `triggered_not_in_topology` | `--triggered` 不在 preset `hats[]`；改 hat id 或省略 | `ralph tools skill load ralph-tools-emit` |
| prompt 顶部出现 `## TRIGGER CONTEXT` 区块 | 先读完它，再按 hat instructions 执行；不要从完整 payload / ledger 重新推断 | 本 skill「核心规则」第 8 条 |
| 本轮要写操作者可读文件，且即将 emit 的 topic schema 要求路径字段 | 先落盘并 `test -f` → `--schema` 确认字段名 → policy-check → emit → 回复打印 `DELIVERABLE_PATH:`；细则按需 load emit skill | `ralph tools skill load ralph-tools-emit`「操作者交付文件路径」（非每轮自动注入） |

**新增 flag 一句话说明**：
- `ralph run --no-default-profiles`：跳过 `ralph.yml` 中的 `profiles.default`，仅保留 CLI `--profile`。
- `ralph run --no-sync-agent-docs`：跳过启动前对 `CLAUDE.md` / `AGENTS.md` 的 managed block 同步。
- `ralph inspect profiles [--no-default-profiles]`：只读预览最终生效的 profile overlay 解析结果。
- `ralph emit --schema <TOPIC>`：只读打印 `<TOPIC>` 的 embedded 协议 + `protocol_hash`，用于 drift 检测。
- `ralph loops clean --ledger`：截断损坏的 loop 状态文件，崩溃后自动重建时会降级到 cold start。

## 核心规则

1. **绝不用 echo/cat 写 tasks 或 memories** — 必须用 CLI 工具
2. **emit 后必须完成 Confirm** — 加载 `ralph-tools-emit`，按其中规定的公开证据确认；证据不足或不一致时停止
3. **task/memory 操作后必须确认状态** — 按已注入的对应专项 skill 检查其规定的公开证据；不要套用其它操作的确认方式
4. **失败时先查 `--help`** — 不要猜测参数，文档可能已更新
5. **emit step handoff 事件前，先用 schema 预检** — `ralph emit --schema <TOPIC>` 会列出 `required_fields`；payload 必须包含全部 required fields，且字段之间不自相矛盾（例如 `step` 与 `task_key` 中的 step 段必须一致）。不要凭记忆构造 payload。
6. **isolated 模式:一个 activation 只发 1 个业务事件** —— 运行时只保留你这一回合最先发出的那个,其后的全部静默丢弃(不分 topic)。发完即停,**终态事件(如 `plan.complete`)前面绝不要夹带 `work.ready` 等其它 emit**,否则终态事件会被丢弃。细则见 `ralph emit` 深参考「isolated mode 单业务事件 / 重发规则」。
7. **先读 `## TRIGGER CONTEXT` 区块，再执行 hat instructions** —— 当 prompt 顶部出现 `## TRIGGER CONTEXT` 时:
   - **触发条件**：preset/schema 为当前 trigger topic 声明了 `trigger_context`（`summary_fields` + 可选 `routing_hints`）。
   - **agent 动作**：先读该区块，再按 hat instructions 执行；区块已把当前 trigger payload 的关键字段（`source topic`、可选 `source hat`、summary fields、命中 routing hints）整理好，直接作为本轮任务指导。
   - **`source hat` 是 optional** —— v1 runtime 不知道哪个 hat 实际发布了这个 trigger event，渲染器会显示 `(unknown source hat)`。**不要依赖 `source hat` 决定分支判断或假设当前 hat 之前的链路是某个具体 hat**；需要查链路用 `ralph tools task list` / `ralph inspect loop`。
   - **关键字段从哪里取得**：注入的 `## TRIGGER CONTEXT` 区块，不是 runtime 内部 ledger 或事件历史。Summary 字段是 schema 声明字段的当前 trigger payload 切片；missing 字段显示 `<missing>`，不要推断成 `0` / `false` / 空字符串。
   - **失败停止条件**：若 Trigger Context 与 hat instructions 冲突，按 hat instructions 与既有恢复机制（`task.resume` / `plan.blocked`）处理，不要自行猜测；若 Trigger Context 显示某字段为 `<missing>` 而你又必须用它，先 `ralph inspect loop --format json` / `ralph tools task list` 复核当前任务状态，再决定继续、阻塞或报告。

## 收到 `task.resume` 时（policy / origin / contract 拒收后自动注入）

`task.resume` 是 runtime 已经定向投递给当前 hat 的恢复信号 — 本 hat prompt 中出现的 `target_hat` 字段就是 runtime 解析出的恢复目标，与 hat id 一致；payload 其它字段记录恢复原因与允许的事件范围。当 runtime 无法解析出安全目标时，不会把 `task.resume` 投递给任何 hat，而是进入既有 `plan.blocked` / diagnostic 通道。**不要为 `task.resume` 额外订阅或修改 preset trigger** — preset 缺 `task.resume` 触发器时，本 hat 仍会因 runtime 的定向投递而收到该事件。读取 payload 后继续原任务即可。

编排器拒收后会在 PENDING EVENTS 注入 `task.resume`（payload 形状见 `ralph-tools-recovery-directives` skill）。**不要重发同样 payload**，按以下顺序修复：

> **例外**：payload 的 `reason=manifest_resume` 时，这是 runtime 提供的恢复引导，**不是** emit 拒收纠正——读 `original_trigger_payload` 从原始触发继续工作，不要走下方拒收修复流程（完整规范见 `ralph tools skill load ralph-tools-recovery-directives` 的 `RD-MANIFEST-RESUME-CONTINUE` 段）。

1. **读 PENDING EVENTS 里 `task.resume` 的 JSON payload**，关键字段：
   - `stage`：`origin` / `policy` / `execution_contract` / `payload_contract`
   - `topic`：被拒收的事件主题
   - `violation`：人类可读原因（含字段名 / 类型不匹配）
   - `required_fields`：当前 topic 缺失或类型错的字段清单
   - `allowed_topics`：当前 hat 可发布的所有 topic（**只在这列里挑**）
   - `reason` / `kind`：结构化 reason code
   - `target_hat`：应当修复并重发的目标 hat（与本 hat 一致——runtime 已定向）
2. **若 prompt 含 `## CORRECTION CONTEXT`**：runtime correction **高于** agent narrative；只执行 correction 的 `required_action`，遵守 `forbidden_action`；细则见 `ralph tools skill load ralph-tools-recovery-directives`。
3. **对照 `required_fields` 补齐 payload**；用 `ralph emit <topic> --policy-check -j '...'` 预检（与 loop gate 同源 schema，**不写盘**）；通过后再正式 `ralph emit` 落盘。部分 preset（agent 上下文且无 event-policy 管线）会在 `--policy-check` 通过时打印一行 `policy_check_token`，apply 时必须用 `--policy-check-token <token>` 带上它（`missing_policy_check_token` / `policy_check_token_mismatch` 即此路径；细则见 `ralph-tools-emit`「Evaluation Token」段）。
4. **bounded retry**：同类协议违规（同一 hat + topic + `task_key` + step + violation code）**第一次**给 structured correction；**第二次**同类违规 runtime 阻塞 loop（`plan.blocked(reason=protocol_violation_repeated:…)`），**不得** infinite `task.resume` 或在没有 `LOOP_COMPLETE` 的情况下静默继续。post-terminal 业务 emit **无 retry**。
5. **确认 hat 作用域**：isolated 模式下未在 `allowed_topics`（与 hat `publishes` 交集）的 topic 越权 — 改用 hat 实际可发的 topic，不要靠 `--unsafe-no-policy-check` 绕过。
6. **复杂 violation**：按需加载 `ralph-tools-emit`（EmitResult `ok`/`recorded`/`errors[].code`/`suggested_command`）与 `ralph-tools-recovery-directives`。
7. **仍不明**：`RALPH_DIAGNOSTICS=1` 启的 loop 把 envelope 写到 `recovery.jsonl`；`ralph diagnose --session latest` 出报告（`docs/guide/runtime-diagnosis.md` §10）。
8. **缺少可用上下文时停止**：如果 `task.resume` payload 的关键字段（`target_hat` / `original_trigger_topic` / `allowed_topics`）为 `<missing>` 或与本 hat 不一致，不要自己重新广播或重发同一 payload；按既有 `task.resume` / `plan.blocked` 恢复机制处理，或读 `ralph inspect loop --format json` 复核当前 loop / hat 状态。

## 命令速查表

### `ralph tools` 命名空间（已注入，按需读取对应子 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph tools task` | 任务管理 | 已注入（`ralph-tools-tasks` skill，仅当 `tasks.enabled`） |
| `ralph tools memory` | 记忆管理 | 已注入（`ralph-tools-memories` skill，仅当 `memories.enabled`） |
| `ralph tools skill` | 加载 skill | `ralph tools skill load ralph-tools-cmdref` |

> `ralph tools interact` 与 `ralph bot` 已随 `ralph-telegram` crate 一起删除;运行时不再提供人工通道(`human.guidance` 已废弃;`task.resume` 恢复通道保留)。

### 顶层命令（按需加载对应 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph emit` | 发射事件（最常用） | `ralph tools skill load ralph-tools-emit` |
| 协议违规 correction / bounded retry | 读 `## CORRECTION CONTEXT` 与 EmitResult | `ralph tools skill load ralph-tools-recovery-directives` + `ralph-tools-emit` |
| `ralph emit --schema <TOPIC>` | 只读查 topic 协议 + protocol_hash | 同上 |
| Precheck gate（`event_loop.precheck`） | 非 CLI；loop 内 `.proposed`/gate 行为 | `ralph tools skill load ralph-tools-precheck` |
| `ralph wave emit` | 并行 wave 调度；带 key 路径走 supervisor store（`applied_via: store`）；agent context 默认 enforce `--policy-check` | `ralph tools skill load ralph-tools-wave` |
| `ralph wave verify --payloads-stdin` | 不写业务事件，只写一次性 ticket 的 wave batch Precheck（dispatcher hat） | `ralph tools skill load ralph-tools-wave` |
| `ralph wave inspect <wave_id>` | OPAC Confirm 公开只读入口；返回 phase / 计数 / availability / `applied`；不含 `events_file` | `ralph tools skill load ralph-tools-wave` |
| `ralph tools task verify <verb>` | 零写盘 Precheck（含 `verify-emit-bridge` 三字段同源） | `ralph tools skill load ralph-tools-tasks` |
| `ralph tools task confirm <task_id> --reference <ref> --digest <digest>` | gate 内 protected Apply 成功后，consume 该行的 pending confirmation，放行同 scope 下一次 protected mutation | `ralph tools skill load ralph-tools-tasks` |
| `ralph inspect loop` | 机器可读 loop + hat 身份摘要；JSON 可能含 `supervisor` 块（见 `ralph-tools-opac` Observe）（OPAC Observe） | `ralph tools skill load ralph-tools-cmdref` |
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph inspect profiles` | 预览 profile overlay 解析结果（只读，不启动 loop） | `ralph tools skill load ralph-tools-cmdref` |
| `ralph inspect prompt` | 预览 hat 的 prompt 渲染结果（auto-inject + on-demand + 块标题），OPAC Observe 一手数据；与 `ralph inspect loop` 配对校验 hat 身份 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph hats validate [--strict]` | 拓扑/payload/orphan/lint 校验 | `ralph hats validate --help`（strict 时启用 lint 所有权检查） |

> **OPAC 纪律（agent context）**: 所有 state-changing 操作遵循 Observe → Precheck → Apply → Confirm 四阶段；详见 always-injected `ralph-tools-opac` skill。本表所有 `ralph emit` / `ralph tools task` / `ralph wave emit` 行均按 OPAC 纪律执行——agent 上下文默认 enforce `--policy-check`。

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置，否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

## 通用错误恢复

| 错误场景 | 可能原因 | 修复方式 |
|----------|---------|---------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE`/`--file` 指向了非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目。**非 wave worker**：可去掉显式 `--file` 让 `ralph emit` 走 marker。**wave worker（`RALPH_WAVE_WORKER=1`）**：必须保留 runtime 注入的 `RALPH_EVENTS_FILE`（指向 `wave-<id>-<idx>.jsonl`），禁止 `unset` 改走 main / marker（否则会 `wave_worker_main_fallthrough` / `empty_worker_result`） |
| `topic is required` | 缺少必需的位置参数 | 补上 topic 参数 |
| `policy check failed` | 事件不符合策略 | 读 stderr / `--output json` 取 `validation_errors[].field`；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再正式发出 |
| `payload_consistency:*`（gate 前缀） | payload 字段之间不自洽（runtime gate） | 与 `policy check failed` 同源：`validation_errors[].field` / `expected` / `message` / `gate` 是修复入口；详见 `ralph-tools-emit` 「Payload 字段自洽检查」段 |
| `task not found` | task ID 不存在或属于其他 loop | `ralph tools task list` 确认当前可用任务 |
| `memory not found` | memory ID 不存在或无权访问 | `ralph tools memory list` 确认可用记忆 |
| `skill not found` | skill 名称错误或对当前 hat 不可见 / `RALPH_CURRENT_HAT` 未设 | `ralph tools skill list` 确认可用 skill；检查 `RALPH_CURRENT_HAT` |
| `progress rate limited` | 5 秒内重复发送 | 等待 5 秒后重试 |
| 退出码 2 (lint gate) | preset 静态 lint 在 strict 模式下发现 error | 修复 preset 配置后重试；查看 `.ralph/diagnostics/preset-lint-error-*.json` |
| `policy validation failed` (`ralph wave emit`) | 任一 payload 违反 required_fields，整批拒绝 | 用 `--output json` 读 `validation_errors[].field` 一次性拿到全部缺失字段，修正后重发 |
| `StateLedger replay failed` / ledger 损坏 | 进程崩溃导致 loop 状态文件不完整 | `ralph loops clean --ledger` 截断损坏文件；下次启动自动 cold start 重建 |
| `plan.blocked` / `progress_task_mismatch` | 当前 step 的 task 状态与预期不一致 | 检查 `ralph tools task list`，按需关闭/重开相关 task |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
