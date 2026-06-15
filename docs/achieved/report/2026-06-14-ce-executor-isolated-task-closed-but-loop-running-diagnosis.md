# 2026-06-14 — ce-executor-isolated 任务被「直接关闭」诊断报告

> **范围**:loop 37859(worktree `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed`)的 `U1 公共基础设施`任务在 U11 HARD RULES 加固后,反而出现「loop 死了但 task 仍显示 closed、CLI 进程未退出」的两段不一致现象。
>
> **触发根因**:`executor` agent 把 `work.done` 事件以 **STRING payload** 发出,触发 schema 预检 `payload_type_mismatch`,loop 进程按 `TerminationReason::PayloadContractViolation` 收尾,但 CLI TUI 主进程没接住这个终止信号继续挂着,导致用户在终端看到的「任务被关掉」与「loop 还在跑」互相矛盾。
>
> **深度根因(防线穿透)**:`ralph emit` CLI 预检路径与 loop runner 实际运行的预检路径**走了两套 config loader**——CLI 走 `load_config_with_overrides`(只读 `ralph.yml`,不合并 preset `event_policy`),loop 走 `load_config_for_preflight`(合并 builtin preset 的 `event_policy`)。结果 CLI 预检是 `Skip` 模式直接放行,直到 loop 读 jsonl 时才被 `validate_event` 拒掉。

---

## 1. 结论摘要

| 维度 | 结论 |
|---|---|
| **任务真的完成了吗** | **是**。Commit `464b4d6`(U1 scaffold: 10 placeholder submodules + mod.rs +41 line diff + pub use forwarders)已落地,worktree HEAD 已推进,`cargo build 0/0` + `cargo nextest 4118/41/13` 通过。 |
| **Loop 实际为什么死** | executor 发出 `work.done` 时 payload 写成 STRING 而非 JSON object,触发 `event_policy` schema 预检 `payload_type_mismatch` → `PayloadContractViolation` → `TerminationReason::PayloadContractViolation` → 进程收尾。 |
| **为什么 CLI 预检没拦** | `ralph emit` 路径使用 `load_config_with_overrides`(`crates/ralph-cli/src/cli/config_loader.rs:125-226`),**不**走 `merge_hats_overlay`,因此读到的是 `ralph.yml`(无 `event_policy` 段)→ `resolve_policy_check_mode` 返回 `Skip` → 预检根本就没跑。 |
| **为什么 task 状态错位** | task `task-1781374262-6313` 的 `closed` 字段是 task system 监听到 `work.done` 落地后的副作用写入,并不是 loop 终止的信号。loop 实际终止时刻 `2026-06-13T18:23:19.486460Z`,在 executor emit `work.done`(`2026-06-13T18:23:13.256326Z`)6 秒之后。 |
| **为什么用户感觉「被关掉」** | 终端进程是 `ralph run` 的 TUI/stdio 父进程,loop 内部 termination 并不会主动 kill 该父进程(目前没有 `TerminationReason → stdio process exit code` 联动);父进程一直挂到下一次 `LOOP_COMPLETE` 事件或外部信号。 |
| **P0 根因** | **`ralph emit` 的 precheck 路径不合并 builtin preset 的 event_policy**(`config_loader.rs:125-226` vs `preflight.rs:197-218`),出现「CLI 放行 / loop 拒绝」防线穿透。 |
| **P1 根因** | executor agent 没遵守 U11 HARD RULES(`presets/en/ce-executor-isolated.yml:397-430`):① 没把 `work.done` 写成 `{"ok":true, ...}` JSON object;② `build.done` 触发了 isolated 模式下 `executor` hat `publishes` 名单拒绝(本身在 schema 上合规但 scope 越界);③ 没有先 commit 再 emit `work.done` 之前的 progress.md 同步步骤。 |
| **P2 根因** | `ralph.yml` 缺 `event_policy` 段,且 `RALPH_CLI_POLICY_CHECK` 也不强制开启;`event_loop.event_policy.require_policy_check_for_cli_emit: true` 这道强制门仅在 preset 内生效(被 loop runner 加载),CLI 子命令本身读不到。 |

---

## 2. 执行链路对比图

### 2.1 期望(应拦截)的链路

```
executor agent
  └─ ralph emit work.done --payload "U1 scaffold complete: ..."
        │
        ▼
[CLI precheck] should_policy_check_emit → resolve_policy_check_mode
        │   ← 期望读到 preset event_policy(强制门)
        ▼
[validate_event] payload_type_mismatch → RejectWithResume
        │
        ▼
CLI 进程 exit code ≠ 0,event 不落地 JSONL
executor 看到失败,重写成 --json 形式重发
```

### 2.2 实际(穿透)的链路

```
executor agent
  └─ ralph emit work.done --payload "U1 scaffold complete: ..."
        │
        ▼
[CLI precheck] should_policy_check_emit
        │   ← load_config_with_overrides() 只读 ralph.yml,无 event_policy
        │   ← resolve_policy_check_mode() 返回 Skip
        ▼
(预检被跳过,直接写盘)
        │
        ▼
.r Ralph/events-20260613-180812.jsonl 落地 {"hat":"executor","payload":"<STRING>","topic":"work.done"}
        │
        ▼
loop_runner: process_events → event_loop::process_event
        │   ← load_config_for_preflight() 合并了 preset event_policy
        ▼
[event_loop/mod.rs:777] validate_event → payload_type_mismatch finding
        │
        ▼
event_origin: emit "event.isolation.boundary_violation" (deny rule 拒)
        │
        ▼
recovery.jsonl: payload_contract CRITICAL / not_retriable
        │
        ▼
runner.rs:3124 TerminationReason::PayloadContractViolation → return Ok(...)
        │
        ▼
loop internal state = "terminated"
BUT
  TUI/stdio 父进程没拿到终止信号,继续挂着
  task system 已经因为 work.done 落地把 task 标 closed
        │
        ▼
用户看到:
  - terminal: ralph run 还在
  - tasks.jsonl: task closed
  - worktree HEAD: 已经前进到 464b4d6
  → 三者互相矛盾,体感「被关掉」
```

### 2.3 关键节点对比表

| 节点 | 应做 | 实际做 | 出处 |
|---|---|---|---|
| `ralph emit` 加载 config | 读 preset 合并 event_policy | 只读 ralph.yml,无合并 | `cli/config_loader.rs:125-226` |
| `should_policy_check_emit` | 返回 `Enforce` | 返回 `Skip` | `policy_check.rs:54-77` |
| `validate_topic_payload_against_config` | 跑 precheck, 拒 string payload | 不跑,直接写盘 | `policy_check.rs:222-234` |
| JSONL 落地 | event 不应有非法 payload | 1 个 `work.done` STRING payload 落盘 | `events-20260613-180812.jsonl` 第 5 行 |
| `event_loop::process_event` | 读到就拒 | 拒,记 recovery.jsonl,触发 termination | `event_loop/mod.rs:777-782` |
| loop runner | 终止 + 通知父进程退出 | 终止,但父进程未收到退出信号 | `runner.rs:3100-3145` |
| task system | work.done 落地后 closed | 已 closed | `agent/tasks.jsonl` |
| TUI 父进程 | 感知 loop 终止,自身退出 | 继续挂着 | 缺 `TerminationReason → stdio exit` 联动 |

---

## 3. 证据清单

### 3.1 运行时事件流(`events-20260613-180812.jsonl`)

5 个事件,顺序如下:

| # | ts (UTC) | hat | topic | payload 形态 | 状态 |
|---|---|---|---|---|---|
| 1 | 18:11:31.073817 | coordinator | work.ready | JSON object | OK |
| 2 | 18:15:21.099292 | executor | build.done | `{"ok":true}` | scope 违规(deny rule)但 schema 通过 |
| 3 | 18:17:34.551743 | executor | build.done | `{"ok":true}` | 同上 |
| 4 | 18:20:05.249908 | executor | build.done | `{"ok":true}` | 同上 |
| 5 | 18:23:13.256326 | executor | work.done | **STRING**(自由文本) | **payload_type_mismatch** |

### 3.2 Payload 契约错误报告

`/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/.../lucky-reed/.ralph/diagnostics/payload-contract-error-2026-06-13T18-23-19-460274+00-00.json`:

```json
{
  "error_type": "payload_type_mismatch",
  "timestamp": "2026-06-13T18:23:19.460274+00:00",
  "topic": "work.done",
  "field": null,
  "source_hat": ["executor"],
  "target_hat": ["review-coordinator"],
  "schema_defined_in": "inline",
  "payload_excerpt": "U1 scaffold complete: 10 placeholder submodules + mod.rs +41 line diff + pub use forwarders; cargo build 0/0; nextest 4118/41/13 (与基线一致); commit 464b4d6; 临时基线 /tmp/event-loop-split-baseline.txt 已生成"
}
```

### 3.3 Recovery Envelopes(`recovery.jsonl`)

| # | source | severity | reason_code | outcome | retry_attempt | safe_target |
|---|---|---|---|---|---|---|
| 1 | `agent_doc_sync` | info | `sync_up_to_date` | recovered | 0 | false |
| 2 | `workflow_guard` | warning | `isolated_scope_violation` (`executor` → `build.done`) | escalated | 0 | false |
| 3 | `drift_monitor` | warning | `recovery_outcome_update` | pending | 0 | true |
| 4 | `payload_contract` | **critical** | `payload_contract_violation` | **not_retriable** | 0 | false |

第 4 条 envelope 直接定义「不可重试」并指向 payload 错误报告,触发 `TerminationReason::PayloadContractViolation`。

### 3.4 Loop Summary

```
Status: Failed: payload contract violation
Iterations: 2
Duration: 15m 6s
Events: 5 (1 work.ready, 3 build.done, 1 work.done)
Final Commit: 464b4d6 — refactor(event_loop): U1 scaffold — 10 placeholder submodules
```

### 3.5 Task 系统状态(`agent/tasks.jsonl`)

- `task-1781374262-6313`(`U1 公共基础设施:建立 event_loop 拆分脚手架 + 全套测试基线`)状态:`closed`,`closed: 2026-06-13T18:23:09.657733+00:00`(注意:closed 时刻**早于** `work.done` emit 时刻 `18:23:13.256326Z`,略早 3.6s;`closed_at` 字段为 null,task store 写回的是 `closed` 字段。具体触发点可进一步查 task store 的 closed hook)
- 这就解释了用户为什么看到「task 被关掉」——closed 钩子由 task store 自己监听 JSONL 落地触发,**和** loop 终止解耦。

### 3.6 Preset 关键定义(`presets/en/ce-executor-isolated.yml`)

- `event_loop.execution_mode: isolated`(line 49-69)
- `event_loop.event_policy.require_policy_check_for_cli_emit: true`(line 136-137)
- `event_loop.event_policy.allow_unsafe_cli_emit: false`(line 136-137)
- `event_policy.topic_deny_rules`:`- hat_id: executor, topic: build.done`(line 147-153)
- `event_policy.schemas.work.done.payload_type: json_object` + `required_fields: [plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines]`(line 171-178)
- executor hat `instructions` 段写明 U11 HARD RULES(line 397-430):`NEVER emit build.done`、`commit before work.done`、`update progress.md`

### 3.7 关键代码引用

- **CLI 预检 loader**:`crates/ralph-cli/src/cli/config_loader.rs:125-226` `load_config_with_overrides`——**只**读 `ralph.yml`,不调用 `merge_hats_overlay`。
- **Loop 预检 loader**:`crates/ralph-cli/src/preflight.rs:197-218` `load_config_for_preflight`——调用 `merge_hats_overlay` 合并 preset。
- **Preset 合并实现**:`crates/ralph-cli/src/preflight.rs:587-680` `merge_hats_overlay`——把 builtin preset 的 `event_policy` 段 merge 进 `core_config`(`ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` 包含 `"event_policy"`,line 511-520)。
- **Policy 模式解析**:`crates/ralph-cli/src/policy_check.rs:54-77` `resolve_policy_check_mode`——config 无 `event_policy` → 返回 `Skip`。
- **CLI 预检调用**:`crates/ralph-cli/src/commands/emit.rs:88-94, 159-188, 237-299`——`should_policy_check_emit` 跳过 → 直接 `append_to_jsonl`。
- **Loop 内预检**:`crates/ralph-core/src/event_loop/mod.rs:777-782` `validate_event` 调用 + 5440-5538 `apply_event_policy_validation` 应用。
- **Schema 校验细节**:`crates/ralph-core/src/event_policy.rs:402-557` `validate_event` 函数,line 501-512 给出 STRING payload 触发 `PayloadTypeMismatch` 的判定。
- **Loop 终止逻辑**:`crates/ralph-cli/src/loop_runner/runner.rs:3100-3145`——`PayloadContractViolation` → return Ok(TerminationReason::PayloadContractViolation)。在 `--rpc`/子进程 TUI 模式下 `handle_termination` 会发送 `LoopTerminated` RPC 事件,但该事件不保证 stdio/TUI 父进程立即退出;非 RPC 模式下父进程完全无通知。
- **ralph.yml**:`/Users/pittcat/Dev/Rust/ralph-orchestrator/ralph.yml`——57 行,**无** `event_policy` 段;`telemetry.runtime_diagnosis` 已开。

---

## 4. 问题归因表

| 级别 | 编号 | 位置 | 问题描述 | 影响 |
|---|---|---|---|---|
| **P0** | P0-1 | `crates/ralph-cli/src/cli/config_loader.rs:125-226` | `load_config_with_overrides` **不**调用 `merge_hats_overlay`,CLI 子命令(`ralph emit`)读不到 preset 的 `event_policy`,`should_policy_check_emit` 永远退化为 `Skip` | 防线穿透:CLI 放行 → JSONL 落地 → loop 内 validate_event 兜底,坏 payload 在 disk 上停留 ~1 帧 |
| **P0** | P0-2 | `crates/ralph-cli/src/loop_runner/runner.rs:3100-3145` | `TerminationReason::PayloadContractViolation` 返回 Ok 后,stdio/TUI 父进程在 non-RPC 模式下完全收不到终止通知;RPC 模式下虽发送 `LoopTerminated` 事件,但不保证父进程立即退出 | 用户体感矛盾:loop 内部已死,父进程仍挂;task 状态已 closed,worktree HEAD 已前进,三者各自为政 |
| **P1** | P1-1 | `presets/en/ce-executor-isolated.yml:397-430` (U11 HARD RULES) | executor agent 没遵守 U11 三件套:① work.done payload 写成 STRING 而非 JSON object(`required_fields: [plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines]`);② 在 commit 之前发了 3 次 `build.done` 触发 isolated scope 违规(但 schema 上是合规的);③ 提交后没补 progress.md 段 | agent 自己不守规矩,导致后续所有防线都需要被触发,放大 P0-1 的穿透后果 |
| **P1** | P1-2 | `presets/en/ce-executor-isolated.yml:147-153` | `topic_deny_rules` 把 `executor` → `build.done` 列了 deny,但 executor `publishes` 段没把 `build.done` 拉黑;U11 HARD RULES 又允许 executor 在 commit 前发 `build.done` 自检 | U11 规则和 deny rule 自相矛盾:hard rule 文字说「NEVER emit build.done」,但 schema/deny 没显式拒,导致 3 个 `build.done` 进了事件流而不是被即拒 |
| **P2** | P2-1 | `ralph.yml`(项目根) | 缺 `event_policy` 段,也没有 `event_loop.event_policy.require_policy_check_for_cli_emit: true` | 单独运行 `ralph emit` 的人为操作(无 loop 上下文)完全无 schema 保护 |
| **P2** | P2-2 | `crates/ralph-cli/src/commands/emit.rs:88-94` | `should_policy_check_emit` 在 `Skip` 模式下连 `info` 日志都不打 | 排查时不易发现「CLI 预检被跳过了」这个事实,recovery.jsonl 也记不到「CLI 侧放过」这条记录 |
| **P2** | P2-3 | `crates/ralph-core/src/event_policy.rs:402-557` | `validate_event` 在 `payload_type_mismatch` 之外,没区分「致命 / 可修复」(STRING vs 缺字段) | 一次性 STRING payload 跟缺 `plan_name`/`commit_count` 等字段被同判 `not_retriable`,把「再补一版 payload 重发」的可能性堵死 |
| **P2** | P2-4 | `crates/ralph-cli/src/loop_runner/runner.rs`(整文件) | 没有「loop 终止 → 父进程退出」的统一事件出口;各 `TerminationReason` 各自处理 | 5 种 termination reason 中只有「LOOP_COMPLETE」会让父进程自然退出(子进程显式 exit 0),其余全部需要外部信号 |
| **P3** | P3-1 | 文档 | `docs/guide/runtime-diagnosis.md` 写「PayloadContractViolation 不被 Responder Soft/Hard 覆盖」,但没写「什么场景会让父进程感知不到」 | 用户阅读 runtime diagnosis guide 找不到父进程挂死这个症状的解释路径 |

---

## 5. 修复建议

> 优先级:**P0-1 → P0-2 → P1-1 → P1-2 → P2-\***。P0-1 一旦修复,P1 的破坏面会缩小到「agent 自己写的 payload 内容不对」,不再有「payload 类型错了」这种最粗糙的错误。

### 5.1 P0-1(必须):让 `ralph emit` 走 preset 合并后的 config

**方案 A(推荐)**:把 `load_config_with_overrides` 改成「先按 ralph.yml 加载,再叠 `merge_hats_overlay`」,但只叠 `event_policy` 段(以及必要时 `cli.*` 中与 emit 相关的开关)。`ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` 已包含 `"event_policy"`。
- 文件:`crates/ralph-cli/src/cli/config_loader.rs:125-226`
- 改动点:在 `load_config_with_overrides` 末尾增加一段「如果有 -H/--hat-collection 指定 builtin preset,且 preset 暴露 event_policy 段,合并之」
- 验证:跑 `ralph emit work.done --payload "test"`,预期 `should_policy_check_emit` 命中 `Enforce`,CLI 直接 exit 1,`recovery.jsonl` 写入 `cli_emit` 来源 envelope(若已实现)

**方案 B(更窄)**:在 `policy_check.rs:54-77` 的 `resolve_policy_check_mode` 中,新增 fallback:如果存在 `RALPH_HAT_COLLECTION` env 或 CLI 传入了 `--hat-collection` 名字,就去读 builtin preset 的 event_policy 段,即使 ralph.yml 没配。
- 改动点:抽 `builtin_preset_event_policy(name: &str) -> Option<EventPolicyConfig>` 工具函数,被 `resolve_policy_check_mode` 调
- 验证同上

**方案 C(应急)**:在 `ralph.yml` 顶部加一段最小 `event_policy`(透传 preset 的内容);但这只是绕过,不解决根因。

### 5.2 P0-2(必须):loop 终止时联动父进程退出

- 文件:`crates/ralph-cli/src/loop_runner/runner.rs:3100-3145`
- 方案:在 `TerminationReason::PayloadContractViolation`(以及其余非 LOOP_COMPLETE 的 reason)返回前,emit 一条内部 `loop.fatal_termination` 事件;loop 入口的 stdio/TUI loop 监听这条事件后,触发 `std::process::exit(non_zero_code)`。
- 备选:在 TUI 主循环 `crates/ralph-cli/src/commands/run.rs` 的 poll 循环里加一行:每次 tick 读 `.ralph/loops.json` 中当前 loop entry 的 `status`,若 `failed` 且 `terminated_at` 已设 5s,主动 `exit(2)`。
- 验证:故意构造一个 `work.done` STRING payload,跑完一个 loop iteration,确认父进程在 termination 出现后 5s 内退出,exit code ≠ 0。

### 5.3 P1-1(强烈推荐):让 U11 HARD RULES 落到 executor 的 first-step 提示

- 文件:`presets/en/ce-executor-isolated.yml:397-430`
- 改动:在 executor `instructions` 段开头新增「**PAYLOAD SCHEMA CHECKLIST**」一节,直接列 `work.done` 的 5 个必填字段名 + 一个 `--json` 完整命令模板;并明确「executor 在 U1/U2 等 commit-driven 阶段,只允许发 work.done(不准发 build.done)」。
- 验证:dogfood——在测试 preset 里,直接 grep executor instructions 是否包含所有 `required_fields`。

### 5.4 P1-2(推荐):消除 `build.done` 的自相矛盾

- 选项 1(强烈):从 executor hat 的 `publishes` 段**直接删掉** `build.done`(假设某处目前允许),或在 executor `instructions` 段硬编码「禁止 ralph emit build.done」。
- 选项 2:从 `topic_deny_rules` 中**移除** `executor` → `build.done` 这一条,改由 agent instruction 软约束。
- 当前两者并存最危险,需要选一个边界。

### 5.5 P2-1 ~ P2-4(可延后)

- **P2-1**:在 `ralph.yml` 显式补一段 `event_policy.schemas.work.done`(透传 preset 即可),使「脱离 loop 单独跑 ralph emit」也有 schema 保护。
- **P2-2**:`should_policy_check_emit` 在 `Skip` 分支增加 `tracing::info!("cli precheck skipped, no event_policy in resolved config")`,让日志里能看到「跳过了」,便于回溯。
- **P2-3**:`validate_event` 区分致命(STRING / 缺顶层 type)与可修复(缺 `plan_name`/`commit_count` 等字段);前者 `not_retriable`,后者可降级为 `recovered` + 注入 `task.resume`。
- **P2-4**:建立统一的 `loop.terminated` 事件,所有 `TerminationReason` 都走它,TUI/stdio 父进程统一监听,不再各自处理。
- **P3-1**:在 `docs/guide/runtime-diagnosis.md` 加一节「常见症状 → 父进程未退出」对应表。

### 5.6 验证步骤(任何修复后必跑)

1. `cargo nextest run -p ralph-cli --bin ralph -- policy_check` —— 走 cli-serial(见 CLAUDE.md HARD RULE 1)
2. `cargo nextest run -p ralph-cli --bin ralph -- cli_emit` —— 跑 CLI 预检相关测试
3. 故意构造 `ralph emit work.done --payload "test"`,确认:
   - CLI exit code ≠ 0
   - JSONL 中**无**该 event
   - `recovery.jsonl` 中有 cli 来源的 envelope
4. 跑一个完整 wave 循环,确认合法 payload 仍能正常落盘并被 loop 接收。
5. 跑 `cargo nextest run --workspace --exclude ralph-e2e`(见 CLAUDE.md HARD RULE 2,默认并行),确保无回归。

### 5.7 不建议的「快速修复」

- ❌ 把 `validate_event` 在 loop 入口的 strict mode 改成 warning:会失去 schema 保护,等于把 P0-1 的破坏面换到 P1-1,问题没解决。
- ❌ 直接在 `ralph emit` 命令行里 hardcode schema 检查:会让 builtin preset 改了 schema 但 CLI 不跟着改,产生第二条「CLI/loop 不一致」链路。
- ❌ 把 task `closed` 钩子从「work.done 落地」改成「loop.terminated」:task 系统的语义是「工作项完成」,跟 loop 终止是两件事,改了就破坏其他场景(executor 一次失败、wave 中途等)的语义。

---

## 6. 时间线

| 时刻(UTC) | 事件 | 来源 |
|---|---|---|
| 18:08:12.907887 | agent_doc_sync: synced=0 skipped=2 failed=0 | `recovery.jsonl:1` |
| 18:11:31.073817 | coordinator → work.ready(plan: U1 公共基础设施) | `events.jsonl:1` |
| 18:15:21.099292 | executor → build.done(scope 违规,但 schema 合规) | `events.jsonl:2` |
| 18:17:34.551743 | executor → build.done | `events.jsonl:3` |
| 18:20:05.249908 | executor → build.done | `events.jsonl:4` |
| 18:23:09.657733 | task `task-1781374262-6313` closed(task store 钩子) | `agent/tasks.jsonl` |
| 18:23:13.256326 | executor → work.done(STRING payload) | `events.jsonl:5` |
| 18:23:19.459998 | workflow_guard: isolated_scope_violation(executor→build.done) | `recovery.jsonl:2` |
| 18:23:19.460274 | payload-contract-error-2026-06-13T18-23-19 写入 | `payload-contract-error-*.json` |
| 18:23:19.460333 | drift_monitor: outcome=pending | `recovery.jsonl:3` |
| 18:23:19.460503 | payload_contract: not_retriable | `recovery.jsonl:4` |
| 18:23:19.486460 | loop terminated at, status=failed | `diagnosis-summary.json:5` |
| 464b4d6 | commit landed: refactor(event_loop): U1 scaffold — 10 placeholder submodules | worktree HEAD |
| 02-08-12 之后 | ralph diagnose 重新跑了一遍,生成新的 diagnosis-summary | session dir |

---

## 7. 与近 30 天同类报告的对比

| 报告 | 主题 | 与本次的关系 |
|---|---|---|
| 2026-06-13-ce-executor-isolated-wave-not-firing-u2-stuck-diagnosis | U2 子 hat stuck | 同样在事件流上,同样不 fire 上游——本次是 payload 不对导致彻底死 |
| 2026-06-13-ce-executor-isolated-wave-synthesizer-no-fire-diagnosis | synthesizer 不 fire | 同样是 wave 收尾错位——本次是 wave 还没进入就被 schema 拒 |
| 2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis | dispatch gap | 同样 CLI/loop 链路错位——本次复现了同一个反模式 |
| 2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis | ralph hat 伪装 | U11 HARD RULES 是同源补丁——本次是它在极端场景下的失效样本 |
| 2026-06-13 e695b6c commit | 「close 5 P0 attack surfaces from ce-executor-isolated incident」 | U5/U7 加固的 5 道防线没覆盖到 CLI 预检 loader,本次正好补这个口子 |

**趋势观察**:U5/U7/U11 三次加固都集中在「loop 内部 / preset 内部」,**没有一条补丁触及 `ralph emit` 的 config loader 路径**。本次 P0-1 是这个空白的「第一个用户可见症状」。建议下个迭代把 CLI 子命令的 config 加载路径作为独立 audit 项,跑一遍 schema gate。

---

## 8. 报告复核记录

本报告在 2026-06-14 由代码层面二次核实,确认/修正如下:

| 原报告论断 | 复核结果 |
|---|---|
| `cli.emit.require_policy_check_for_cli_emit` | **修正**:实际位于 `event_loop.event_policy` 下(`presets/en/ce-executor-isolated.yml:136-137`),`cli:` 段(line 251)仅含 `backend`/`prompt_mode` |
| `work.done` required_fields 为 `[ok, summary, commit_sha, files_changed, next_step]` | **修正**:实际为 `[plan_name, plan_path, task_id, task_key, step, commit_count, changed_lines]`(`presets/en/ce-executor-isolated.yml:177`) |
| task `closed_at` 字段 | **修正**:task store 写回的是 `closed` 字段,`closed_at` 为 null |
| runner 完全不通知父进程 | **修正**:RPC/子进程 TUI 模式下会发送 `LoopTerminated` RPC 事件,但不保证父进程立即退出;non-RPC 模式下父进程完全无通知 |
| `ralph emit -H builtin:...` 会被功能性地使用 | **补充**:`-H` 是全局参数,`emit` 子命令分发时忽略 `hats_source`,因此即使命令行带上 `-H` 也不会加载 preset |
| 其余核心论断(P0-1 防线穿透、P0-2 父进程挂死、事件流 5 条、recovery 4 条、task closed、commit 已落地) | **确认属实** |

复核方式:源码阅读(`config_loader.rs`、`preflight.rs`、`policy_check.rs`、`emit.rs`、`runner.rs`、`event_policy.rs`)+ worktree 产物检查(events JSONL / recovery.jsonl / tasks.jsonl / summary.md / diagnosis-summary.json / git HEAD)。

---

## 9. 附录:复现命令

```bash
# 1. 切到 worktree
cd /Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed

# 2. 查看 task 状态
jq '.[] | select(.status=="closed")' .ralph/agent/tasks.jsonl

# 3. 查看 loop summary
cat .ralph/agent/summary.md

# 4. 查看诊断报告
cat .ralph/diagnostics/2026-06-14T02-08-12/diagnosis-summary.json

# 5. 触发 schema 预检(模拟 agent 的 STRING payload 行为)
ralph --hat-collection builtin:ce-executor-isolated \
      emit work.done --payload "free text" \
      --hat executor

# 预期(P0-1 修复后):CLI exit code 1,stdout 提示 "payload_type_mismatch"
# 当前实际:CLI exit 0,event 落 .ralph/current-events 文件
```

```bash
# 6. 跑回归测试
cd /Users/pittcat/Dev/Rust/ralph-orchestrator
cargo nextest run -p ralph-cli --bin ralph -- policy_check cli_emit
cargo nextest run --workspace --exclude ralph-e2e
```

---

**报告完。**
