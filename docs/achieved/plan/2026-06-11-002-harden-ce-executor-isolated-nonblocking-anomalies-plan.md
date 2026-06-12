---
date: 2026-06-11
plan-id: 2026-06-11-002
type: fix
status: done
preset: ce-executor-isolated
origin: docs/report/2026-06-11-ce-executor-isolated-nonblocking-anomalies-corrected-diagnosis.md
scope: 小范围加固 review 语义门、wave emit 幂等、诊断时区与 loop 路径解析
---

# 加固 ce-executor-isolated 非阻塞异常处理

## Summary

本计划不改变现有 10-hat 拓扑，不迁移 `queue.advance` 接收者，不重写 aggregator，也不调整 EventBus。

只处理本次有直接证据的四类问题：

1. 阻止不符合条件的 `trivial_step` 绕过 Fixer。
2. 为 `ralph wave emit` 增加显式幂等键，从写入源头阻止重复 wave。
3. 统一诊断时间与活跃 loop 工作目录解析。
4. 对根 `.ralph` current 指针与 `loops.json` 不一致给出明确诊断。

## 范围边界

### 在范围内

- preset 指令和已有 event policy 的小范围约束。
- `ralph wave emit` 的可选幂等参数和 loop 内去重记录。
- 针对性单元测试与 replay/BDD 场景。
- `ralph diagnose` 或共享诊断读取逻辑的时区和 workspace 选择。
- loop 状态展示的一致性告警。

### 不在范围内

- 不改变 `queue.advance → executor` 当前设计。
- 不新增 hat。
- 不重构 EventBus、wave dispatcher 或 task store；重复写入在 CLI 层解决。
- 不删除或重写 `.ralph` 历史文件。
- 不改变 worktree/primary 的 canonical 判定。
- 不把所有 prompt 规则改造成 Rust 硬门。
- 不修改当前运行中的 `.ralph` 状态。

## Requirements

| ID | 要求 | 验收标准 |
|---|---|---|
| R1 | 非微小 diff 或存在 actionable findings 时，不允许 `review.passed(skip_reason=trivial_step)` | 事件被拒并定向恢复 review-coordinator |
| R2 | 合法 trivial step 仍可快速通过 | 空 diff 或显式 trivial step 的现有场景不退步 |
| R3 | 同一 loop、hat、topic、幂等键的重复 `wave emit` 不得产生第二组事件 | 第二次调用返回首个 `wave_id`，events 文件行数不增加 |
| R4 | 诊断时间必须同时显示 UTC 和配置时区，耗时只用时间戳差值计算 | 不再把 13:11 UTC 与 20:50 CST 直接相减 |
| R5 | 诊断目标目录优先由 `loops.json.workspace` 确定 | worktree loop 能读到实际 events/tasks |
| R6 | 根 current 指针不一致时给出 warning，不阻止活跃 worktree loop | `ralph loops`/diagnose 显示 stale-current warning |

## Key Decisions

| 决策 | 方案 | 原因 |
|---|---|---|
| review 约束位置 | 优先扩展现有 event policy 语义检查 | 已有拒绝与 `task.resume` 恢复链，改动集中 |
| trivial 判定 | 使用 payload 中已有 `changed_lines`、`findings_count` 和 `skip_reason` | 不引入新的跨文件状态依赖 |
| findings 阈值 | 首版只禁止 `findings_count > 0` 的 `trivial_step` | 规则清楚，避免重做 synthesizer 的分类逻辑 |
| wave 防重复 | 增加显式 `--idempotency-key`，同键重复调用返回已有结果 | 自动按 payload 推断会误伤合法重复业务；显式 key 可审计、可预测 |
| 去重作用域 | `(loop_id, hat, topic, idempotency_key)` | 不同 loop、hat 或 topic 的同名业务不能互相吞掉 |
| 去重存储 | events 文件同目录下的 `.wave-idempotency.jsonl`，文件锁内完成 check-and-append | 与 loop/worktree 自然隔离；不污染 task store；支持进程重启 |
| 重复调用返回值 | 退出码 0，返回原 `wave_id`、`deduplicated=true` | 重试应是幂等成功，不应让 agent误判为失败并继续补偿 |
| current 指针修复 | 先告警，不自动改写 | 避免运行中修改共享 `.ralph` 状态 |

## Implementation Units

### U1. 增加 `trivial_step` 语义门

**目标**

拒绝明显不成立的 fast-path：

```text
topic == review.passed
AND skip_reason == trivial_step
AND (findings_count > 0 OR changed_lines >= 50)
```

**文件**

- `crates/ralph-core/src/event_policy.rs`
- `crates/ralph-core/src/event_loop/tests/event_policy.rs`
- `presets/en/ce-executor-isolated.yml`
- 对应 ZH/schema 镜像，仅在实际内容存在相同规则时同步

**实现**

1. 在现有 `review.passed` schema 校验后增加专用语义检查。
2. 命中时返回现有可恢复 rejection，target 为 source hat。
3. reason code 使用稳定名称，例如 `invalid_trivial_step_bypass`。
4. recovery payload 写明：
   - observed `changed_lines`
   - observed `findings_count`
   - expected action：进入 synthesizer/Fixer 或使用正确终态
5. 不改变 `empty_diff` 和 `aggregate_timeout` 行为。

**测试**

- `changed_lines=80, findings_count=20`：拒绝并产生 `task.resume`。
- `changed_lines=5, findings_count=0`：接受。
- `skip_reason=empty_diff`：保持现有行为。
- payload 缺字段：继续由 schema 层拒绝。

### U2. 根治重复 wave：给 `ralph wave emit` 增加显式幂等键

**目标**

同一业务 wave 即使因 agent 误操作、命令重试或超时重跑，也只允许写入一次。

#### ce-debug 根因链

本次重复 wave 的完整因果链如下：

```text
review-coordinator 准备 7 个 payload
  → 第一次执行 `ralph wave emit`
  → CLI 调用 generate_wave_id()
  → 7 行事件 append 到 events.jsonl（Wave A）
  → agent 想“验证命令是否成功”
  → 再次执行同一条写命令
  → CLI 再次 generate_wave_id()
  → 又 append 7 行（Wave B）
  → EventLoop 读取同一批两个 distinct wave_id
  → isolated authority 丢弃 Wave B
```

直接根因位于 `crates/ralph-cli/src/wave.rs`：

- `execute_emit()` 每次调用都会进入 `write_wave_events()`。
- `write_wave_events_with_provenance()` 无条件调用 `generate_wave_id()`。
- 随后用 append 模式把整批事件写入 events 文件。
- CLI 没有业务幂等键，也没有“相同请求已经成功”的持久记录。

isolated authority 的 `IsolatedMultipleBusinessEmissions` 只在 EventLoop 读取事件后生效。它避免了重复下游执行，但此时 Wave B 已经写入原始 JSONL，并产生 recovery 噪音。因此它是**事后止损**，不是重复 wave 的根治机制。

#### 方案选择

采用显式幂等键：

```bash
ralph wave emit review.wave.ready \
  --idempotency-key "${plan_name}:${task_id}:${step}:${fix_round}" \
  --payloads-stdin \
  --output json
```

不采用以下方案：

- **仅依赖 prompt**：agent 仍可能因重试、超时或工具误用再次执行。
- **按 payload hash 自动去重**：两个合法 review round 可能 payload 完全相同，自动去重会吞掉真实新业务。
- **在 EventBus 去重**：事件已经写盘，诊断与事件游标仍受污染，且改动范围大。
- **复用调用方指定的 wave_id**：调用方容易制造冲突；幂等 key 应映射到由 Ralph 生成的 wave_id。

**文件**

- `crates/ralph-cli/src/wave.rs`
- `crates/ralph-cli/src/cli.rs` 或 wave clap 定义所在文件（如果参数定义被拆分）
- `crates/ralph-cli/src/loop_runner/tests.rs` 中 wave CLI 集成测试区域
- `presets/en/ce-executor-isolated.yml`
- `presets/zh/ce-executor-isolated-zh.yml`
- `crates/ralph-cli/src/presets.rs` 的 preset 指令回归测试
- `crates/ralph-core/data/ralph-tools.md`，仅当该文档已列出 `ralph wave emit`

**实现**

1. 给 `WaveEmitArgs` 增加可选参数：

   ```rust
   #[arg(long)]
   pub idempotency_key: Option<String>,
   ```

2. 定义稳定作用域：

   ```text
   loop_id + current_hat + topic + idempotency_key
   ```

   - `loop_id` 从现有 runtime 环境或 `.ralph/current-loop-id` 解析。
   - `current_hat` 使用现有 `RALPH_CURRENT_HAT`。
   - 缺少 loop id 时使用 events 文件 canonical path 作为隔离作用域。
   - key 为空、全空白或超过 256 字节时直接拒绝。

3. 在 events 文件同目录维护：

   ```text
   .wave-idempotency.jsonl
   ```

   每条记录至少包含：

   ```json
   {
     "scope_key": "<sha256(loop|hat|topic|key)>",
     "idempotency_key": "<原始 key>",
     "wave_id": "w-...",
     "topic": "review.wave.ready",
     "hat": "review-coordinator",
     "payload_digest": "<sha256(canonical payload array)>",
     "count": 7,
     "created_at": "..."
   }
```

4. 写入过程必须在同一个文件锁临界区内完成：

   ```text
   获取 wave emit lock
     → 查找 scope_key
     → 已存在：校验 payload_digest/count
     → 不存在：生成 wave_id
     → append events
     → append idempotency record
     → fsync/flush
     → 释放 lock
   ```

   锁文件放在 events 文件旁，例如 `.wave-emit.lock`。不能只在内存中去重，否则 Ralph/CLI 进程重启后会再次写入。

5. 已存在相同 key 时：

   - digest 和 count 相同：
     - 不写新事件；
     - 退出码 0；
     - text 输出原 `wave_id`；
     - JSON 输出：

       ```json
       {
         "wave_id": "w-existing",
         "topic": "review.wave.ready",
         "count": 7,
         "events_file": "...",
         "deduplicated": true
       }
       ```

   - digest 或 count 不同：
     - 退出非零；
     - 报错 `idempotency key reused with different payload`;
     - 不修改 events 和幂等记录。

6. 未传 `--idempotency-key` 时保持当前行为，避免破坏其他 preset 和人工调用。

7. 更新 ce-executor-isolated 的 review-coordinator：

   - key 使用：

     ```text
     ce-review:{plan_name}:{task_id}:{step}:round-{fix_round}
     ```

   - 首次发送和命令重试使用同一个 key。
   - 真正的新复审必须增加 `fix_round`，从而产生新 wave。
   - `ralph wave emit` 仍标明为 write command；验证必须读取其 JSON 输出或 events 文件，不能重新构造不同 key 调用。

8. JSON 输出无论首次还是 dedup 都包含 `deduplicated`：

   - 首次：`false`
   - 重复：`true`

   这样 agent 不需要通过再次读写命令判断是否成功。

**测试**

- 首次同 key调用：写入 N 行，返回 `deduplicated=false`。
- 第二次同 key、同 payload：返回相同 `wave_id`，events 行数保持 N，返回 `deduplicated=true`。
- 同 key、不同 payload：失败且 events 行数不变。
- 不同 key、相同 payload：允许写入第二个合法 wave。
- 相同 key、不同 loop：互不去重。
- 相同 key、不同 hat/topic：互不去重。
- 不传 key：保持当前每次生成新 wave 的兼容行为。
- 两个并发进程使用同 key：最终只存在一个 wave，两个调用获得相同 `wave_id`。
- CLI 进程退出后再次调用同 key：仍能去重，证明记录持久化。
- idempotency record 已写但 events 不完整的故障注入：返回明确错误，不静默宣称 dedup 成功。
- preset snapshot 检查稳定 key 包含 `plan_name/task_id/step/fix_round`。

**故障一致性**

小范围实现不引入数据库，但必须避免“记录说已写、事件实际没写”：

- 推荐顺序为先 append events，再 append idempotency record。
- 若 events 成功、record 失败，命令返回失败；重试时在持锁状态下扫描 events：
  - 如果存在带相同 `idempotency_key` 元数据的完整 N 行，则补写 record 并返回原 wave_id；
  - 如果不完整，则报 `incomplete prior wave emission`，不自动追加第二批。
- 为支持恢复，每个 wave event 顶层增加可选 `idempotency_key` 或 `idempotency_hash`。未传 key 的历史事件格式不变。

**源码引用反向验证**

若更新 `crates/ralph-core/data/ralph-tools.md`：

1. 核对其中所有 `wave.rs:NN-MM` 引用是否漂移。
2. 运行 `ralph wave emit --help`，确认参数表包含 `--idempotency-key`。
3. 执行一次首次 emit + 一次 dedup emit 冒烟测试，确认第二次未增加 events 行数。

### U3. 修正诊断时间和运行目录解析

**目标**

诊断报告不再因时区或根 `.ralph` 指针误判活跃 loop。

**文件**

- 优先定位并修改 `ralph diagnose` 的 session/workspace 解析模块
- 对应 diagnosis reporter 测试

**实现**

1. 输入为主仓 `.ralph` 时：
   - 读取 `loops.json`；
   - 按 loop id 或 latest active entry 选择目标；
   - 使用 entry 的 `workspace` 读取 events/tasks/diagnostics。
2. 时间解析全部转为 UTC instant 后计算 duration。
3. 报告展示：
   - 原始 UTC；
   - 配置时区本地时间；
   - 精确 duration。
4. 若 `current-loop-id`、`loop.lock` 与 selected loop 不一致，只输出 warning。

**测试**

- UTC 13:11 与 CST 21:13 计算结果为约 2 分钟。
- primary current 指针陈旧、worktree loop 活跃时，选择 worktree workspace。
- `loops.json` 不存在时保持当前单目录 fallback。

### U4. progress/task 轻量对账提示

**目标**

避免 `progress.md` 显示 pending，而 task store 已经 in_progress。

**文件**

- `presets/en/ce-executor-isolated.yml`
- `presets/zh/ce-executor-isolated-zh.yml`

**实现**

在 executor 的 `queue.advance` 路径中，task 创建并 start 后立即更新：

- Current Step
- Runtime Task ID
- 状态 `in_progress`
- 更新时间

不引入自动文件同步器，不修改 task store。

**测试**

- preset 指令 snapshot。
- replay 场景检查 queue.advance 后 progress 与 task id 一致。

## 验证策略

按风险从小到大执行：

```bash
rtk cargo test -p ralph-core event_policy
rtk cargo test -p ralph-core ce_executor
rtk cargo test -p ralph-core scenarios
rtk cargo test -p ralph-cli diagnosis
rtk cargo test --workspace --exclude ralph-e2e
rtk cargo test --workspace --exclude ralph-e2e --doc
```

涉及 preset 后额外执行：

```bash
ralph preset check builtin:ce-executor-isolated
ralph preflight -H builtin:ce-executor-isolated
```

最后运行项目要求的完整测试：

```bash
./scripts/run-tests.sh
```

## 完成条件

- 非法 `trivial_step` fast-path 被拒并可恢复。
- 合法 trivial/empty-diff 路径不退步。
- 重复 wave 的 preset 诱因被明确消除。
- 同一幂等键重复执行不会新增第二个 `wave_id` 或事件行。
- diagnose 对本次运行计算出 `queue.advance → U2 task start ≈ 2 分钟`。
- stale root current 指针只产生 warning，不影响 worktree loop。
- 不改变 hat 数量、事件拓扑和 `queue.advance` 当前路由。

## 风险与回滚

| 风险 | 控制 |
|---|---|
| 语义门误伤合法 trivial step | 仅使用已有字段，保留小 diff + 0 findings |
| diagnose workspace 选择错误 | 保留显式 loop id 优先和单目录 fallback |
| preset 文案膨胀 | 每处只增加一段硬规则，不复制完整工作流 |
| 幂等记录和 events 不一致 | 文件锁 + payload digest + 故障恢复扫描 |
| 合法新 review 被错误去重 | key 必须包含 `fix_round`；不同 key 永远允许新 wave |
| 并发重复调用竞态 | check 与 append 在同一跨进程文件锁内执行 |
| current 指针自动修复引发竞争 | 本计划只告警，不自动写盘 |

每个 U 独立提交；若出现回归，可按 U 单独回滚。
