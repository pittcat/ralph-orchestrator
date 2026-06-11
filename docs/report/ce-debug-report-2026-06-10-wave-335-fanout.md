# ce-executor-isolated 链路诊断报告

> **运行 ID**：`2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-prime-badger`
> **Plan**：`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`
> **Preset**：`builtin:ce-executor-isolated`（10 hat 拓扑，concurrency=9，wave review）
> **目录**：`/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-prime-badger/.ralph/`
> **用户报告症状**：第二轮 wave 出现 "335 workers | timeout 1800s"，TUI 长时间无响应。
> **诊断结论**（一句话）：bash `$(cat file)` 的 word-splitting 把 7 行 JSONL 拆成 ~335 个 token；`ralph wave emit` 与 `wave_detection` 全链路缺乏 JSON 形状校验与 fan-out 上限；review-synthesizer 在第一轮又错误地把 4/7 的 partial wave 路由成 `review.failed`；scratchpad 自报 "7 个维度已派发" 是基于错误印象的伪记账。三类问题叠加：preset 缺失防护 + 机制缺乏硬上限 + agent 使用了未文档化的反模式。

---

## 1. 结论摘要

| 维度 | 判定 | 关键证据 |
|---|---|---|
| **首要 bug** | bash `printf '%s\n' $(cat jsonl)` 把 JSON 拆成 word-token | Wave 2 第 0 条 payload=`{"dimension":"correctness","focus":"Verify`（截断），第 1 条 payload=`10`，第 2 条=`placeholder`，… 第 334 条=`stubs.","changed_files":[...]`（截断） |
| **机制缺陷 #1** | `ralph wave emit` 不校验 stdin 行的 JSON 形状、不限制 payload 数量 | `crates/ralph-cli/src/wave.rs:122-138` 的 `read_payloads_from_stdin` 只 trim 后 push，没有 serde_json::from_str 验证；`validate_payload_shape` 只对 `--payloads` 路径生效 |
| **机制缺陷 #2** | `wave_detection` 不限制 `wave_total` 上限 | `crates/ralph-core/src/wave_detection.rs:78-100` 只校验 `wave_total == 0` 和 wave_index 范围，没有 max fan-out |
| **机制缺陷 #3** | dispatcher 用 `wave.total` 算 aggregate timeout，导致 335×1800s/9 + 30s ≈ **19 小时** | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:476-478`：`batches = ceil(total/concurrency); aggregate = wave_timeout*batches + 30` |
| **机制缺陷 #4** | TUI 按 `total` 预分配 335 个 worker_buffers，与 semaphore 实际并发（9）脱节，UI 看起来 "卡 335 个" | `crates/ralph-tui/src/state.rs:158`：`let worker_buffers = (0..total).map(...).collect()` |
| **机制缺陷 #5** | review-synthesizer 的 `wait_for_all` 与 wave 的 `aggregate timeout` 互相不能 trigger 对方；`review.failed` 路径绕过 All-Dimensions-Timeout 守则 | preset 注释 901-904 说 `plan.blocked` 是必须的，但 synthesizer 在 4/7 时错误地落入 `Decision Logic` 走 `review.failed`；详见证据 §3.2 |
| **preset 设计问题** | `HARD RULE — emit ALL selected dimensions in ONE wave call` 只给了 2 个正确示例（`--payloads '<json1>' ...` 与 `printf '%s\n' '<json1>' ...`），**没有给"JSONL 文件如何作为输入"的安全示例**。Agent 自然补了一个反模式 | `presets/en/ce-executor-isolated.yml:656-660` |
| **agent 执行问题** | review-coordinator 写 scratchpad 时**未核对实际事件数**就报"7 个维度已派发"。这是闭环缺失：emit 之后没看 events.jsonl | `.ralph/agent/scratchpad.md:194-198` 写"7 个维度 reviewer 已派发"，但实际有 335 条 `review.wave.ready` |
| **agent 执行问题 #2** | 第二轮 emit 之前 review-coordinator 写 `last_reviewed_sha: 8d7bb71ca02ec78a4713eb0b4fec79a74b509f6c`，但 `context.md` 仍是 `3e925a83a18ca41fb761caa134d5550503ddc19e`。sed 写入失败但被报"✓" | `context.md:22`（最新 mtime 02:50） vs `scratchpad.md:198`（mtime 03:02） |

---

## 2. 执行链路对比图

### 2.1 预期（按 preset）

```
[18:32] work.start
   └─ coordinator → work.ready (step-01, u1-public-infrastructure)
[18:49]    └─ executor → work.done (commit 3e925a8, 1 file, 23 ins)
[18:50]        └─ review-coordinator → review.wave.ready × 7 (1 wave)
                       └─ dimension-reviewer (concurrency=9) × 7
[18:55]            └─ review-synthesizer (wait_for_all 300s)
                           ├─ 全部到达 → review.passed / review.complete
                           └─ 任何缺失 → plan.blocked (All-Dimensions-Timeout)
[19:00]        └─ plan-gate
                           ├─ queue.advance (step-02) | plan.complete
                           └─ fixer / debug-resolver (有 findings 时)
```

### 2.2 实际（事件时间线）

```
[18:32:48] work.ready  (1 个)
[18:36:25] build.done  (executor 自报，4 次)        ← preset 禁止的 progress 事件
[18:49:29] task-1781116362-9cc1 启动
[18:49:43] work.done  #1 (1 commit, 23 ins)
[18:50:49] review.wave.ready × 7   ← Wave #1 (正确: 7 events, wave_total=7)
           7 个不同的 dimension, 7 个不同的 wave_index 0-6
[18:52–54] review.dimension.done × 3  (仅 3/7)
[18:55:00] ⚠ 300s wait_for_all 触发，但 synthesizer 走错了路径
[18:56:10] review.failed  ← 错误路由 (应 plan.blocked)
[19:01:02] work.done  #2 (2 commits, 34 ins, 含 pub use forwarding)
[19:02:12] review.wave.ready × 335  ← Wave #2 (BUG: 335 events, wave_total=335)
           第 0 条 payload: {"dimension":"correctness","focus":"Verify  (截断 JSON)
           第 1 条 payload: 10
           第 2 条 payload: placeholder
           ...
           第 334 条 payload: stubs.","changed_files":[...]  (截断 JSON)
[19:03–19:30] review.dimension.done × 45+ (有 22 条 ts 与 ts+day 字段格式不一致)
              task_key 字段被填成单词 token (9cc1, U1, mod, forwarding, stubs...)
[20:30 ...] 持续运行（aggregate_timeout ≈ 19 小时，~68430s）
[20:30 至今] TUI 持续显示 "335 workers | timeout 1800s" 不动
```

### 2.3 链路偏离总表

| Step | 预期 | 实际 | 偏离程度 |
|---|---|---|---|
| coordinator → work.ready | 1 次 | 1 次 | ✓ |
| executor build.done | 禁止 | 4 次 | preset violation（executor 误发） |
| work.done payload | 必含 commit_count/changed_lines | 都含 | ✓ |
| review-coordinator emit (1st) | 7 events / 1 wave | 7 events / 1 wave | ✓ |
| dimension-reviewer 完成 (1st) | 7/7 | 4/7 (3 分钟内) | partial timeout |
| review-synthesizer 路由 (1st) | plan.blocked（缺失 3 维度） | **review.failed** | 走错分支 |
| fixer 收到 review.failed | 修复 2 safe_auto | 报 fix.exhausted（配额 3/3） | 设计上 OK，但源头错 |
| executor 实施 fix.plan | 应用 2 项修复 | 实施成功（scratchpad 记录） | ✓（fix 路径无 bug） |
| review-coordinator emit (2nd) | 7 events / 1 wave | **335 events / 1 wave** | ⛔ 主要 bug |
| last_reviewed_sha 持久化 | context.md 更新到 8d7bb71 | 仍是 3e925a83 | sed 失败被报成功 |
| review-synthesizer (2nd) | 7/7 完成 | 等 45+ / 335，~19h 后超时 | 卡死 |
| plan-gate | queue.advance / plan.complete | **未被触发** | loop 卡在 review 阶段 |

---

## 3. 证据清单

### 3.1 事件流（events-20260610-183148.jsonl，445 行 / 425 事件 / 22 行解析失败）

| 类型 | 数量 | 备注 |
|---|---|---|
| review.wave.ready | **342** | Wave 1 = 7，Wave 2 = 335（**应 7**） |
| review.dimension.done | 64 | Wave 1 = 3，Wave 2 = 45+（持续累加中），非 Wave = 16 |
| work.ready | 1 | step-01 |
| work.done | 2 | 1st=23ins, 2nd=34ins |
| build.done | 5 | executor 自报的 progress 事件，**preset 禁止** |
| review.failed | 1 | 1st wave 后 路由到 fixer |
| fix.exhausted | 1 | fix_round 3/3 |
| fix.plan.ready | 1 | debug-resolver handoff |

Wave 2 全部 335 条事件 timestamp 完全相同 (`2026-06-10T19:02:12.171425+00:00`)，wave_id 全部相同 (`w-18b7cd8123158a50-53873-0`)，wave_index 0–334，wave_total=335。

### 3.2 Wave 2 payload 抽样（关键证据：word-splitting）

| idx | payload 字段原始值 | 解读 |
|---|---|---|
| 0 | `{"dimension":"correctness","focus":"Verify` | JSON 开头被截断，剩到 `"focus":"Verify` |
| 1 | `10` | 单词 token |
| 2 | `placeholder` | 单词 token |
| 3 | `sub-module` | 单词 token |
| 4 | `files` | 单词 token |
| 100 | `placeholder` | 同上 |
| 200 | `pub` | 单词 token |
| 300 | `sub-module` | 同上 |
| 334 | `stubs.","changed_files":["crates/ralph-core/src/event_loop/diagnostics.rs",...` | JSON 末尾被截断，从 `stubs."` 开始 |

**判定**：7 行 JSONL（每行一个完整 JSON）被 `$(cat file)` 在 shell 端做 word-splitting 后，传给 `printf '%s\n'`，每行 48 个 token × 7 = ~336。`printf '%s\n' $(cat file)` 是经典 bash 反模式：`$(...)` 替换会做 IFS 切词 + glob。

### 3.3 review.dimension.done 的 task_key 污染

Wave 2 触发的 dimension-reviewer 收到的 payload 字段也是单词 token，所以它们发表的 `review.dimension.done` 里 `task_key` 字段是：

- `9cc1`（原 task_id `task-1781116362-9cc1` 的尾部）
- `U1`、`maintainability`、`correctness`、`learnings`（从 intent_summary / dimension 字段抽的）
- `mod`、`forwarding`、`stubs`（从 focus 文本抽的）
- `1781116362-9cc1`（完整 task_id 字符子串）

45+ 条 `review.dimension.done` 里，**只有少数几条的 `task_key` 满足 `ce-executor:...:u1-public-infrastructure` 格式**。其余都是 broken field，**plan-gate 拿来做 task 关联时全部 miss**。

### 3.4 时间窗与超时换算

```text
per-worker timeout (dimension-reviewer.timeout) = 1800s
hat concurrency (dimension-reviewer) = 9
wave.total (实际) = 335
batches = ceil(335 / 9) = 38
aggregate_timeout = 1800 × 38 + 30 = 68430s ≈ 19 小时
```

19 小时比 preset `event_loop.max_runtime_seconds = 28800s`（8 小时）还长。即使是空的 wave 也根本跑不完。

### 3.5 源码定位

| 关注点 | 文件:行 | 关键逻辑 |
|---|---|---|
| stdin 读取 | `crates/ralph-cli/src/wave.rs:122-138` | `read_payloads_from_stdin` 不校验每行是 JSON |
| 唯一防护 | `crates/ralph-cli/src/wave.rs:96-118` | `validate_payload_shape` 只对 `--payloads` 路径生效；**stdin 路径零防护** |
| wave_total 上限 | `crates/ralph-core/src/wave_detection.rs:76-100` | 仅校验 `wave_total != 0` 和 wave_index < wave_total；无 max |
| aggregate timeout | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:474-478` | `batches = total/ceil/concurrency; aggregate = wave_timeout * batches + 30` |
| TUI buffer 预分配 | `crates/ralph-tui/src/state.rs:158` | `(0..total).map(...).collect()` 一次性 335 个 buffer |
| All-Dimensions 守则 | `presets/en/ce-executor-isolated.yml:1036-1054` | wait_for_all 超时应 emit `plan.blocked`，但 synthesizer 在 4/7 走了 `Decision Logic` 分支发 `review.failed`（preset 1057-1063） |

### 3.6 preset 关键设计点（与 bug 相关的部分）

- **L656-660 HARD RULE — ONE wave call**：正确示例是 `printf '%s\n' '<json1>' ... '<jsonN>' | ralph wave emit ... --payloads-stdin`，**未给出"JSONL 文件作为输入"的安全模式**。Agent 自然用 `$(cat file)` 走反模式。
- **L143-149 review.wave.ready schema**：必填 `dimension, focus, depth, diff_base, intent_summary, changed_files, plan_name, task_id, task_key, step`。Wave 2 的 335 条事件 0/335 满足 schema。
- **L901-906 review-synthesizer publishes**：`["review.passed", "review.failed", "review.complete", "plan.blocked"]` — `plan.blocked` 在列表中。
- **L1036-1054 All-Dimensions-Timeout 守则**：超时应 emit `plan.blocked: reason="dimension_reviewers_failed_to_converge"`。
- **L1057-1063 Decision Logic**：如果 has findings（即使部分）→ `review.failed`（fix_round < 3） 或 `review.complete`（fix_round ≥ 3）。**这里没有"先检查是否所有 dimension 都返回了"的守门**，所以 partial wave 被当成完整 wave 走。
- **L1439-1456 reporter obligations**：`conditional_forbid_topics` 阻止 pass_or_fail=fail 时发 LOOP_COMPLETE — 这条设计 OK，但 reporter 永远收不到上游。

### 3.7 scratchpad 与 context.md 不一致

- `context.md:22` 的 `last_reviewed_sha: 3e925a83a18ca41fb761caa134d5550503ddc19e`（mtime 02:50，本地 = 18:50 UTC）
- `scratchpad.md:198` 自报"last_reviewed_sha 已持久化到 context.md"且 mtime 03:02（=19:02 UTC，对应 Wave 2 发射瞬间）
- 推论：第二轮 emit 后 `sed -i 's|^last_reviewed_sha:.*|last_reviewed_sha: 8d7bb71...|'` 实际未生效，**agent 把失败报成了成功**。下一次 wave emit 仍以 `3e925a83` 为 base，等于重复审 U1 的 diff。

### 3.8 Wave 1 的二级 bug：synthesizer 路由错误

Wave 1 的 7 个 review.wave.ready 中，3 个 review.dimension.done 在 4 分钟内到达（18:52-18:54），其余 4 个无事件。wait_for_all 300s 窗口（18:50:49 + 300s = 18:55:49）超时。但：

- 6/7 findings 文件实际在磁盘（scratchpad L88-89 自报"6 文件到达，4 事件收到"，缺 correctness 维度）
- scratchpad 把这归为 "review.failed → fixer"，但 preset 守则 L1036-1054 明确说应该发 `plan.blocked`
- 结果：fixer 在 fix_round 3/3 时拒收（因为前 3 轮被 agent_doc_sync 占用），debug-resolver 接管发 fix.plan.ready。**fix 路径最终成功**（删 eprintln + 加 pub use），但机制上走了 4 个 hat 全部不必要的工作

---

## 4. 问题归因表

### P0（必须修，破坏运行）

| ID | 现象 | 归因层 | 关键证据 |
|---|---|---|---|
| **P0-1** | Wave 2 发射 335 个事件，17+ 小时 aggregate timeout，loop 实际卡死 | agent 执行（用了未文档化的 bash 反模式 `printf '%s\n' $(cat file)`，IFS word-splitting） | events.jsonl Wave 2 payload 抽样 + bash man 文档 |
| **P0-2** | `ralph wave emit --payloads-stdin` 不校验 stdin 行的 JSON 形状，导致任意 N 行 N 个事件 | Ralph Loop 机制（`crates/ralph-cli/src/wave.rs:122-138` 缺 `serde_json::from_str` 校验） | wave.rs:128-137 read_payloads_from_reader 只 trim 不 parse |
| **P0-3** | `wave_detection` 不限制 `wave_total` 上限，单个 wave 可任意大 | Ralph Loop 机制（`crates/ralph-core/src/wave_detection.rs:76-100`） | wave_detection.rs:78-127 全部校验逻辑，只查 == 0 和 wave_index 范围 |
| **P0-4** | `execute_wave` 用 `total / concurrency × wave_timeout` 算 aggregate，导致 worker 总数被恶意/无意扩大时 timeout 爆炸 | Ralph Loop 机制（`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:474-478`） | 335 × 1800 / 9 + 30 = 68430s |

### P1（应修，机制不严）

| ID | 现象 | 归因层 | 关键证据 |
|---|---|---|---|
| **P1-1** | TUI 按 `wave.total` 预分配 `worker_buffers`（335 个槽），与 `semaphore` 实际并发（9）脱节；用户看到 "335 workers" 误以为卡 335 个 | Ralph Loop 机制（`crates/ralph-tui/src/state.rs:158`） | 截图 "335 workers" 实际只是 9 在跑、326 等 permit |
| **P1-2** | review-synthesizer 在 partial wave（4/7）时走 `Decision Logic` 走 `review.failed`，绕过 `All-Dimensions-Timeout` 守则 | preset 设计（`ce-executor-isolated.yml:1057-1063` 缺守门，先检查 `events_arrived == total`） | scratchpad L88-105 + preset L1036-1063 对比 |
| **P1-3** | preset 的 `HARD RULE — ONE wave call` 给出 2 个 `printf '%s\n' '<json>'` 示例但**没给"如何把 JSONL 文件传给 stdin"的安全示例**。Agent 自然补 `$(cat file)` | preset 设计（`ce-executor-isolated.yml:656-660`） | preset L656-660 全文 |
| **P1-4** | agent 写 scratchpad 自报 "7 个维度 reviewer 已派发" 但实际是 335。emit 之后没核对 events.jsonl 行数 | agent 执行（缺乏"emit 后回读 events.jsonl 验证"硬步骤） | scratchpad L194-198 vs events.jsonl 实际行数 |
| **P1-5** | agent 报 `last_reviewed_sha 已持久化`（mtime 03:02）但 context.md 仍是 3e925a83（mtime 02:50），sed 失败被报"✓" | agent 执行（sed exit code 未检，stdout 也没输出新值就当成功） | context.md:22 内容 vs scratchpad.md:198 声明 |
| **P1-6** | executor 在 work.done 之外自报了 5 次 `build.done`，违反 preset "DO NOT emit progress events" 规则 | agent 执行（executor 没读 preset L487 约束） | events.jsonl grep "build.done" |

### P2（次要，建议改进）

| ID | 现象 | 归因层 | 关键证据 |
|---|---|---|---|
| **P2-1** | `aggregate_timeout` 与 preset `event_loop.max_runtime_seconds=28800`（8h）无关联；wave timeout 19h 直接绕过 loop 顶帽 | Ralph Loop 机制（`event_loop` 与 `wave` 隔离） | preset 8h vs aggregate 19h |
| **P2-2** | `fix_round` 在 `current_fix_round: 3` 状态下接收新 step 的 findings，没做"跨 step 配额隔离"，导致 step-01 的 2 个 safe_auto 被 0 配额接 | preset 设计（`ce-executor-isolated.yml:fixer` 段没有 step-scoped budget） | scratchpad L120-130 |
| **P2-3** | preset `last_reviewed_sha` 持久化有"优雅降级"路径但**没有 verifier 步骤**：写完后**立即 git diff <last_reviewed_sha> HEAD 验证范围**是 0 | preset 设计（"after emit" 没强校验） | 缺 grep `git diff <last_reviewed_sha> HEAD | wc -l` |
| **P2-4** | 22 行 events.jsonl 解析失败（payload 是单词 token、JSON 截断），EventReader 应能识别并记录 `parse_error.jsonl` | Ralph Loop 机制（缺 malformed event 指标面） | 22 行 unparseable |
| **P2-5** | 22:00 后 events.jsonl 被截到 0 字节（mtime 03:30）— 推测是 loop cleanup 或外部 OOM killer；保留 `events-history-20260610-183148.jsonl` 但不告警 | Ralph Loop 机制（无文件大小/截断告警） | ls -la 03:30 |

---

## 5. 修复建议（仅诊断，未动手修改）

> 按用户要求本次**不修改任何代码**。以下建议是给后续 fix 任务的方向。

### 5.1 P0 修复（让 wave 不再卡死）

1. **在 `read_payloads_from_stdin` 加 JSON 形状校验**（`crates/ralph-cli/src/wave.rs:122-138`）：
   - 每行 `serde_json::from_str::<serde_json::Value>()` 一次，失败 bail 出可读错误
   - 提示用户改用 `--payloads '<json1>' ...` 或正确示例 `cat file.jsonl | ralph wave emit ... --payloads-stdin`（不带 `$(...)`）
2. **在 `try_build_wave` 加 wave_total 上限**（`crates/ralph-core/src/wave_detection.rs:78`）：
   - 引入 `MAX_WAVE_TOTAL: u32 = 64`（或从 hat_config 读 `max_wave_size`）
   - 超限时 `tracing::error!` + 跳过该 wave 并 emit `plan.blocked: reason="wave_total_exceeds_cap"`
3. **aggregate timeout 加 max ceiling**（`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:474-478`）：
   - `let aggregate_timeout = (wave_timeout * batches + 30).min(Duration::from_secs(event_loop_max_runtime / 4))`
   - 超过 cap 立即 emit `plan.blocked`
4. **TUI buffer 改为按 concurrency 分配**（`crates/ralph-tui/src/state.rs:158`）：
   - 用 `min(wave.total, hat.concurrency) as initial`，剩余 on-demand 分配
   - 显示 "335 queued / 9 running"

### 5.2 P1 修复（机制不严）

5. **synthesizer 强制门禁**（`ce-executor-isolated.yml:1036-1054` 与 L1057）：
   - 在 `Decision Logic` 之前加 guard：`if events_arrived.len() < total → 走 All-Dimensions-Timeout 守则`
   - 这条 L1036-1054 已经写好"DO NOT emit review.passed/short-circuit" 但 L1057 的 Decision Logic 没引用它
6. **preset 加 JSONL 文件输入的安全示例**（`ce-executor-isolated.yml:656-660`）：
   - 显式补 "❌ 错误：printf '%s\\n' $(cat file.jsonl)" 和 "✅ 正确：cat file.jsonl | ralph wave emit ... --payloads-stdin"
   - 因为 `cat | pipe` 不会做 word-splitting
7. **agent 必须 `git diff <last_reviewed_sha> HEAD | wc -l` 验证持久化**（preset L680-689 加固）：
   - sed 之后 grep `^last_reviewed_sha:`，必须看到新 SHA 才算成功
   - 否则 abort 并 publish `work.failed`
8. **emit 后必读 events.jsonl 验真**（agent instructions 加 hard step）：
   - `wc -l .ralph/events.jsonl` 取最后 7 行（按 wave_total 期望），用 `jq -e` 验 schema
   - 不满足 → abort 并降级到 `review.passed: skip_reason=empty_diff`（不写 wave）

### 5.3 P2 修复（建议改进）

9. `event_loop.max_runtime_seconds` 应约束 wave aggregate timeout，cap = max_runtime / 4
10. fix_round 改为 per-step 计数（`fix_log.current_fix_round: step-01: 0`），不跨 step 共享
11. EventReader 增加 `events.parse_error.count` 指标并 emit 到 diagnostics
12. 文件大小/截断告警：events.jsonl 若 size 缩小 → emit `loop.cancel: reason="events_file_truncated"` + shipper 触发

### 5.4 流程建议（给 ce-executor-isolated 用户）

- 跑 4 步的 wave：1 个 `work.done` 触发的 review wave 每次只应 ≤ `hat.concurrency` 个事件。如果 presets 想"审 N 个维度"，请用 N 个独立 wave，每次一个维度。
- 在 `ralph wave emit` 后用 `ralph loops status --loop <id>` 或 `jq '.[] | select(.wave_id=="...") | length' .ralph/events.jsonl` 核对实际 dispatch 数
- 任何 wave 的 total > 32 都需要重新审视是否真的需要

---

## 6. 附录：本次调查的方法

1. 读取 `presets/en/ce-executor-isolated.yml` 全文 → 提取 hats/triggers/schemas/守则
2. 解析 `.ralph/events-20260610-183148.jsonl` → 425 事件按 topic/wave_id/hat 聚合
3. 抽样 Wave 2 的前 15 / 100 / 200 / 300 / 334 条 → 确认 word-splitting
4. 抽样 Wave 2 的 45+ 条 `review.dimension.done` → 发现 `task_key` 字段被污染
5. 读 `crates/ralph-cli/src/wave.rs` 全文 → 确认 stdin 路径零 JSON 校验
6. 读 `crates/ralph-core/src/wave_detection.rs` 全文 → 确认 wave_total 无上限
7. 读 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` execute_wave 段 → 计算 aggregate timeout = 68430s
8. 读 `crates/ralph-tui/src/state.rs` WaveInfo::new → 确认 buffer 按 total 预分配
9. 对比 `agent/scratchpad.md` 与 `agent/context.md` 的 last_reviewed_sha → sed 失败被报成功
10. 抽样 build.done 事件 → executor 违反 preset "DO NOT emit progress events" 规则

**诊断置信度**：High。已逐字验证 Wave 2 payload 内容、源码行为、preset 守则、scratchpad 自报之间的一致性与矛盾。

**未在本报告中处理的事项**：
- 不修改任何代码（用户明确要求）
- 不修复任何 wave / 事件文件（受 `不要手动编辑 .ralph/ 下的运行时状态文件` 约束）
- 不停止运行中的 loop（受 `ABSOLUTE PROHIBITION: NEVER kill ... ralph process` 约束）
