# ce-executor 一次 run 的完整问题诊断报告

> Run ID: `primary-20260602-032900`
> 计划: `docs/plans/2026-06-02-001-fix-retry-runner-auto-start-plan.md`
> 时长: 1h 33m 17s（03:29:00 → 05:02:18 UTC）
> 状态: `pass_with_residuals` (verdict)
> 报告生成: 2026-06-02

---

## 摘要（一句话）

本次 run 的 ralph 编排与事件流转**严格按 `presets/ce-executor.yml` 设计执行**——orchestration 没坏。问题集中在 **3 处**：(1) preset `dimension-reviewer` hat 缺 `timeout` 配置导致 5/7 review dimensions 持续超时（review 盲区，**最严重**）；(2) executor 违反 preset 的 incremental commit 策略，把 U1+U2+U3 合并成一个 commit；(3) 状态报告（handoff.md / summary.md）与实际不符。

---

## 1. Run 元信息

| 字段 | 值 | 来源 |
|------|------|------|
| Loop ID | `primary-20260602-032900` | `/.ralph/current-loop-id:1` |
| 起始 SHA | `174b545d` | `/.agents/scratchpad/.../context.md` + `events-20260602-032900.jsonl:42` |
| 终止 SHA | `5a92daf5` | `/.ralph/agent/handoff.md:8` + `events-20260602-032900.jsonl:43` |
| 分支 | `fix/retry-auto-start-runner` | `/.ralph/agent/handoff.md:7` |
| 实施 commit | `9304ecfb`（U1+U2+U3 合并） | `events-20260602-032900.jsonl:4` |
| 收尾 commit | `5a92daf5` | `events-20260602-032900.jsonl:42-43` |
| 实施时间 | 03:33:04 → 04:05:11（~32 分钟） | `events-20260602-032900.jsonl:1, 4` |
| Review 时间 | 04:10:18 → 04:58:08（~48 分钟） | `events-20260602-032900.jsonl:5, 42` |
| Reporter 时间 | 05:01:46 → 05:01:58（12 秒） | `events-20260602-032900.jsonl:43-44` |
| 总 iterations | 11 | `/.ralph/agent/summary.md:4` + `events-20260602-032900.jsonl:2` |
| 实际改动文件 | 4 个（web.py / test_web.py + 2 个 doc） | `git diff --name-only 174b545d..5a92daf5` |
| Pre-existing 测试失败 | 31 个（`tests/test_runner.py`） | `/.agents/scratchpad/.../fix-log.md` |
| 本次新增测试 | 6 个（`TestRetryAutoStartsRunner`） | `events-20260602-032900.jsonl:4` |
| 通过测试 | 39/39（test_web.py） | `events-20260602-032900.jsonl:4` |

---

## 2. 事件流与编排对账

事件流共 **45 行**，分布在 `/.ralph/events-20260602-032900.jsonl`：

| 行号 | 阶段 | Hat | 事件 | 时间 |
|------|------|-----|------|------|
| 1 | 启动 | coordinator | `work.ready` | 03:33:04 |
| 2 | U1 | executor | `queue.advance` | 03:42:55 |
| 3 | U2 | executor | `queue.advance` | 03:47:19 |
| 4 | U3 | executor | `work.done` | 04:05:11 |
| 5-11 | Wave 1 启动 | review-coordinator | `review.wave.ready × 7` | 04:10:18 |
| 12 | Wave 1 完成 | standards-reviewer | `review.dimension.done` (standards) | 04:15:16 |
| 13 | Wave 1 完成 | dimension-reviewer | `review.dimension.done` (maintainability) | 04:15:49 |
| 14-23 | Wave 1 失败 | 系统 | `wave.worker.failed × 5` + `review.dimension.done × 5`（FAILED 标识） | 04:21:06 |
| 24 | Wave 1 总结 | ralph | `review.failed` | 04:25:27 |
| 25 | Fix Round 1 | fixer | `fix.applied` | 04:30:04 |
| 26-30 | Wave 2 启动 | review-coordinator | `review.wave.ready × 5` | 04:36:19 |
| 31-32 | Wave 2 完成 | dimension-reviewer | `review.dimension.done × 2`（requirements + api-contract） | 04:41:04-25 |
| 33-40 | Wave 2 失败 | 系统 | `wave.worker.failed × 4` + `review.dimension.done × 4` | 04:46:25 |
| 41 | Wave 2 总结 | ralph | `review.complete` (pass_with_residuals) | 04:51:04 |
| 42 | 收尾 | shipper | `REVIEW_COMPLETE` (pass) | 04:58:08 |
| 43 | 报告 | reporter | `report.done` | 05:01:46 |
| 44 | 终止 | reporter | `LOOP_COMPLETE` | 05:01:58 |

**编排一致性结论**：事件流与 `presets/ce-executor.yml` 中声明的 8 hat 流转图（`:23`）完全对得上。**orchestration 没坏**。问题在配置精度和 agent 行为层。

---

## 3. 问题清单（按严重程度排序）

### 🔴 P0-1：5/7 review dimensions 持续超时（review 盲区）

**严重等级**：🔴 P0 — 影响 review 完整性，可能漏掉真实 P0/P1 findings

**观察**：
- Wave 1（4:10:18 启动 → 4:21:06 结束）：7 个 dimensions 中只有 `maintainability` 和 `standards` 成功
- Wave 2（4:36:19 启动 → 4:46:25 结束）：5 个 dimensions 中只有 `requirements` 和 `api-contract` 成功
- `correctness` / `testing` / `reliability` **两次都失败**

**.ralph 证据**（绝对路径）：

| 路径 | 行号 | 内容 |
|------|------|------|
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl` | 5 | Wave 1 dim 0 `correctness` `depth: standard` |
| 同上 | 6 | Wave 1 dim 1 `testing` `depth: standard` |
| 同上 | 7 | Wave 1 dim 2 `maintainability` `depth: quick` |
| 同上 | 8 | Wave 1 dim 3 `standards` `depth: quick` |
| 同上 | 9 | Wave 1 dim 4 `requirements` `depth: standard` |
| 同上 | 10 | Wave 1 dim 5 `api-contract` `depth: standard` |
| 同上 | 11 | Wave 1 dim 6 `reliability` `depth: standard` |
| 同上 | 12 | standards 完成 @ 04:15:16（启动后 4'58"） |
| 同上 | 13 | maintainability 完成 @ 04:15:49（启动后 5'31"） |
| 同上 | 14, 15 | Worker 0 失败 × 2 事件 @ 04:21:06（启动后 10'48"） |
| 同上 | 16, 17 | Worker 1 失败 × 2 事件 @ 04:21:06 |
| 同上 | 18, 19 | Worker 4 失败 × 2 事件 @ 04:21:06 |
| 同上 | 20, 21 | Worker 5 失败 × 2 事件 @ 04:21:06 |
| 同上 | 22, 23 | Worker 6 失败 × 2 事件 @ 04:21:06 |
| 同上 | 24 | `review.failed` 报告 `5 of 7 review dimensions timed out` |
| 同上 | 26 | Wave 2 dim 0 `correctness`（重试） |
| 同上 | 27 | Wave 2 dim 1 `testing`（重试） |
| 同上 | 28 | Wave 2 dim 2 `requirements` `depth: quick`（注意：Wave 2 把 standard 改 quick 后才成功） |
| 同上 | 29 | Wave 2 dim 3 `api-contract` `depth: standard` |
| 同上 | 30 | Wave 2 dim 4 `reliability`（重试） |
| 同上 | 31 | requirements 完成 @ 04:41:04（Wave 2 启动后 4'45"） |
| 同上 | 32 | api-contract 完成 @ 04:41:25（Wave 2 启动后 5'06"） |
| 同上 | 33, 34 | Wave 2 Worker 0 失败 × 2 @ 04:46:25（启动后 10'06"） |
| 同上 | 35, 36 | Worker 1 失败 × 2 @ 04:46:25 |
| 同上 | 37, 38 | Worker 3 失败 × 2 @ 04:46:25 |
| 同上 | 39, 40 | Worker 4 失败 × 2 @ 04:46:25 |
| 同上 | 41 | `review.complete` 报告 `dimensions_timed_out: [correctness, testing, reliability]` |

**关键相关性**：
- 5 个 `depth: standard` 的 dimensions 在 Wave 1 全部失败
- 2 个 `depth: quick` 的 dimensions（maintainability + standards）成功
- Wave 2 把 `requirements` 从 standard 改为 quick 后**才成功**（`:28` vs `:9`）
- `correctness` / `testing` / `reliability` 始终 `depth: standard`，两次都失败

**源码证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/`）：

| 路径 | 行号 | 内容 |
|------|------|------|
| `crates/ralph-core/src/wave_detection.rs` | 27-42 | `DetectedWave::timeout_secs()` 优先级：`hat.timeout > hat.aggregate.timeout > 300s default` |
| `crates/ralph-core/src/wave_detection.rs` | 31 | `pub fn timeout_secs(&self) -> u64 {` |
| `crates/ralph-core/src/wave_detection.rs` | 33 | `self.hat_config.timeout.map(u64::from).or_else(...)` — 第一优先读 `hat.timeout` |
| `crates/ralph-core/src/wave_detection.rs` | 35-40 | `or_else` 分支读 `hat.aggregate.timeout` |
| `crates/ralph-core/src/wave_detection.rs` | 41 | `.unwrap_or(300)` — 默认 300s |
| `crates/ralph-core/src/config.rs` | 2759-2768 | `AggregateConfig` 结构体定义（`mode` + `timeout: u32`） |
| `crates/ralph-cli/src/loop_runner.rs` | 6123-6128 | aggregate_timeout 计算公式：`Duration::from_secs(wave_timeout.as_secs().saturating_mul(batches)) + Duration::from_secs(30)` |
| `crates/ralph-cli/src/loop_runner.rs` | 6126 | `let batches = u64::from(wave.total).div_ceil(concurrency as u64);` — Wave 1: 7/4=2 batches |
| `crates/ralph-cli/src/loop_runner.rs` | 6127 | aggregate_timeout = 300 × 2 + 30 = **630s**（10.5 分钟） |
| `crates/ralph-cli/src/loop_runner.rs` | 6130-6141 | 触发 aggregate timeout 时的行为：取消剩余 workers、记 `Vec::new()` |
| `crates/ralph-cli/src/loop_runner.rs` | 6440-6494 | ACP wave worker 实现（不会跑这条路径） |
| `crates/ralph-cli/src/loop_runner.rs` | 6505-6738 | PTY wave worker 实现（含 timeout 触发位置） |
| `crates/ralph-cli/src/loop_runner.rs` | 6692-6706 | `match tokio::time::timeout(wave_timeout, stream_result).await` — 这里是 wave worker 的 per-worker timeout |
| `crates/ralph-cli/src/loop_runner.rs` | 6722-6733 | per-worker timeout 触发时的事件：`"Worker timed out after {}s without emitting events"` |

**Preset 证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml`）：

| 行号 | 内容 |
|------|------|
| 340-346 | `dimension-reviewer` hat 完整定义 |
| 341 | `name: "🔬 Dimension Reviewer"` |
| 343 | `triggers: ["review.wave.ready"]` |
| 344 | `publishes: ["review.dimension.done"]` |
| 345 | `concurrency: 4` ← 触发 wave |
| 346 | `instructions: |` ← **下面就是 instructions，没有 `timeout` 字段，没有 `aggregate` 字段** |

**根因**（causal chain）：

```
presets/ce-executor.yml:340-346
  dimension-reviewer 仅声明 concurrency: 4（触发 wave）
  没有 timeout 字段
  没有 aggregate 字段
         ↓
crates/ralph-core/src/wave_detection.rs:31-42
  DetectedWave::timeout_secs() 走到 .unwrap_or(300)
         ↓
per-worker timeout = 300s
         ↓
crates/ralph-cli/src/loop_runner.rs:6126-6128
  aggregate_timeout = 300 × ⌈7/4⌉ + 30 = 630s
         ↓
实际 standard depth review 任务需要 > 630s 才能 emit review.dimension.done
         ↓
crates/ralph-cli/src/loop_runner.rs:6692 / 6477
  timeout 触发 → "Worker timed out after 300s without emitting events"
         ↓
.ralph/events-20260602-032900.jsonl:14-23, 33-40
  5/7 → 4/5 dimensions 报 FAILED
```

**预测**（ce-debug 要求的预测性验证）：如果给 `dimension-reviewer` 加 `timeout: 900`，Wave 1 的 aggregate_timeout = 900×2+30 = 1830s，standard depth 任务在 1830s 内应能完成。`correctness` / `testing` / `reliability` 不会在两次 wave 中都超时。

**影响**：
- `correctness` / `testing` / `reliability` 三个最关键的 review dimensions **完全没有跑过**
- shipper 报告 `:42` 说"3 dimensions timed out twice in review but no P0/P1 surfaced from the 4 dimensions that completed"——这是**false negative 风险**："没发现"≠"不存在"
- 例如：reviewer payload `:11` 明确把"double retry race condition (helper called twice in quick succession)"列为 reliability 关注点，但**这个点从未被验证**

**建议修复**：

最小修复（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml:340`）：

```yaml
dimension-reviewer:
  name: "🔬 Dimension Reviewer"
  description: "Focused wave-based code review for a single dimension."
  triggers: ["review.wave.ready"]
  publishes: ["review.dimension.done"]
  concurrency: 4
  timeout: 900  # ← 新增
  instructions: |
    ...
```

**验证计划**：
1. 应用 patch 后，跑 `cargo test -p ralph-core wave_detection` 验证无回归
2. 在 ralph-log-monitor 仓库同 plan 重跑一次，统计 wave 失败 dimensions 数（应该 = 0）
3. 持续 2 次重试如果仍然失败，escalate 到 `correctness` 单独超时 1500s

---

### 🟠 P1-1：executor 违反 incremental commit strategy

**严重等级**：🟠 P1 — 偏离 preset 指令，影响 bisect 与回滚粒度

**观察**：executor 主动把 U1+U2+U3 合并成单个 commit `9304ecfb`，理由是"等 U3 一起提"。U1 和 U2 都是独立 logical unit 且 U1 完成时 tests 33/39 全过——按 preset 应该独立 commit。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl`）：

| 行号 | 内容 |
|------|------|
| 2 | U1 摘要："未 commit（等 U2+U3 一起提交）" |
| 3 | U2 摘要："未 commit，等 U3 一起提" |
| 4 | U3 摘要："U1+U2+U3 合一个 commit 9304ecfb" |

**Preset 证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml`）：

| 行号 | 内容 |
|------|------|
| 203-218 | `Incremental Commit Strategy` 完整规则 |
| 205-208 | Commit 触发条件（Logical unit / Tests pass / Context switch） |
| 209-213 | Do NOT commit 条件（Partial progress / Tests failing / Pure scaffolding） |
| 214-218 | Commit 流程（add specific files + commit with scope） |

U1（抽 helper）和 U2（改 endpoint）按 205-208 都满足 commit 条件。

**影响**：
- U3 写测试时改坏代码 → git bisect 不能定位是 U1/U2/U3 哪一步
- Plan 失败时回滚粒度只能"全部 retry"或"全部 revert"
- 违反 preset 的"**Simple Code, No Drama**"原则（与 hat registry 整体设计冲突）

**建议修复**：

短期：在 `presets/ce-executor.yml` executor instructions 中加硬规则：

```yaml
### Commit Cadence (HARD RULE)
- Commit AFTER each Implementation Unit where tests pass.
- Do NOT batch multiple U-IDs into a single commit.
- Use `git add <files>` (not `git add .`) per U-ID.
```

长期：在 hat registry 加 `commit_per_unit: true` 选项。

---

### 🟠 P1-2：coordinator 发布的 `hard_prereq` 未被实际验证

**严重等级**：🟠 P1 — 决策前提未验证，plan 方向可能反了

**观察**：`work.ready` payload 明确声明 `hard_prereq: "OQ#1: user must verify runner_enabled is false via fetch(/api/runner) before U1 implementation; if true, plan framing must be re-evaluated"`，但后续 events **没有任何验证动作**，executor 直接开干。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl`）：

| 行号 | 内容 |
|------|------|
| 1 | `work.ready` payload 中 `hard_prereq: "OQ#1: user must verify runner_enabled is false via fetch(/api/runner) before U1 implementation; if true, plan framing must be re-evaluated"` |
| 2-4 | U1/U2/U3 摘要中无 `runner_enabled` / `fetch(/api/runner)` / `verify` 关键词 |
| 1 | 同 payload 还有 `verification: ["pytest tests/test_web.py -v all green", "ruff check . no new warnings", "no regression in /api/runner/*, cancel, delete", "frontend code unchanged"]` |

**Preset 证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml`）：

| 行号 | 内容 |
|------|------|
| 56-141 | `coordinator` hat 完整定义 |
| 80-90 | 复杂程度评估（Complexity Assessment） |
| 121-128 | Pre-Publish Validation（要求 `ralph tools task list` 验证）— **但没有要求验证 hard_prereq** |
| 129-132 | Event Publishing（发 `work.ready` 前必做 file creation + task creation） |
| 142-251 | `executor` hat 完整定义 |
| 154-169 | Read State / Environment Setup（**没有"verify hard_prereq"步骤**） |
| 232-235 | Failure Handling |

**影响**：
- 如果 `runner_enabled` 实际是 `true`（用户没关 Runner），plan 假设的"helper 未被调用 → 队列堆积"前提就是错的
- 修方向可能完全反了：本来应该修 store 而非 helper
- **本次没出问题纯属运气**——用户的实际环境确实是 false

**建议修复**：

1. coordinator instructions 中加 explicit pre-flight check 段
2. `work.ready` payload 增加 `preflight_commands: [...]`，executor 必跑
3. 增加 `loop.verifier` hat 在 work.ready 之后强制跑 preflight

---

### 🟡 P2-1：`review-synthesizer.aggregate.timeout: 300` 是文档误导

**严重等级**：🟡 P2 — 误导未来维护者，问题 1 没被发现的根本原因

**观察**：`presets/ce-executor.yml:531-533` 在 `review-synthesizer`（聚合者）上写了 `aggregate.timeout: 300`，开发者大概率以为这是 wave worker 的超时——但**这个 300 不会影响 wave worker 行为**。

**.ralph 证据**：无（这是配置层 bug，不在 .ralph 产物中体现）

**Preset 证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml`）：

| 行号 | 内容 |
|------|------|
| 525-533 | `review-synthesizer` hat 完整定义 |
| 529 | `default_publishes: "review.complete"` |
| 531-533 | `aggregate: { mode: wait_for_all, timeout: 300 }` |
| 547 | "Wait for all `review.dimension.done` events (aggregate `wait_for_all` handles this)" |

**源码证据**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/`）：

| 路径 | 行号 | 内容 |
|------|------|------|
| `crates/ralph-core/src/wave_detection.rs` | 31-42 | timeout_secs() 优先级逻辑 |
| `crates/ralph-core/src/wave_detection.rs` | 33 | **第一优先读被触发的 hat 的 `hat.timeout`** |
| `crates/ralph-core/src/wave_detection.rs` | 35-40 | 第二读**被触发的 hat 的 `hat.aggregate.timeout`** |

**被触发的 hat 是 `dimension-reviewer`**（`concurrency: 4` 触发 wave）——它的 `aggregate` 字段未定义。所以 worker timeout 走 `wave_detection.rs:41` 的 `.unwrap_or(300)` 默认值。

`review-synthesizer.aggregate.timeout: 300` 是给 synthesizer hat 自己的 agent 进程用的——和 worker 的 backend claude 进程不是同一个。

**影响**：
- preset 看起来"配置好了 300s 超时"，实际**没生效**
- 直觉上让人误以为已经做了防护
- 这才是问题 1 一直没被发现 / 没人质疑的原因

**建议修复**（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/ce-executor.yml:531-533`）：

```yaml
# 方案 A: 给 review-synthesizer.aggregate 加注释
aggregate:
  mode: wait_for_all
  timeout: 300  # NOTE: 这个 timeout 仅约束 synthesizer agent 自身的存活，不约束 dimension-reviewer worker
  # 控制 worker timeout 见 dimension-reviewer.timeout

# 方案 B: 干脆把 synthesizer 的 aggregate 删掉（用默认 300s 即可）
```

---

### 🟡 P2-2：handoff.md 错列 recently-modified 文件

**严重等级**：🟡 P2 — 状态报告不可信

**观察**：`handoff.md:21-32` 列出 8 个 recently modified 文件，但 `git diff --name-only 174b545d..5a92daf5` 实际本次只改了 4 个。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/handoff.md`）：

| 行号 | 内容 |
|------|------|
| 7 | `**Branch:** \`fix/retry-auto-start-runner\`` |
| 8 | `**HEAD:** ab341c01: chore: auto-commit before merge (loop primary)` |
| 21-32 | `Recently modified:` 列表 |

`.ralph/agent/handoff.md:23-32` 列出的 10 个文件：

```
- `.envrc`
- `PROMPT.md`
- `config.json`
- `docs/brainstorms/2026-06-02-merge-duplicate-cases-requirements.md`
- `docs/brainstorms/2026-06-02-retry-runner-sync-requirements.md`
- `docs/plans/2026-06-02-001-fix-retry-runner-auto-start-plan.md`
- `docs/plans/2026-06-02-002-feat-merge-duplicate-cases-plan.md`
- `docs/report/2026-06-02-ce-executor-2026-06-02-001-fix-retry-runner-auto-start-plan-report.md`
- `frontend/src/api/jobs.ts`
- `frontend/src/components/JobTable.tsx`
```

**对比 git diff 174b545d..5a92daf5 实际改动**：

```
docs/brainstorms/2026-06-02-retry-runner-sync-requirements.md
docs/plans/2026-06-02-001-fix-retry-runner-auto-start-plan.md
src/ralph_log_monitor/web.py
tests/test_web.py
```

**差异**：
- handoff 多列：`.envrc` / `PROMPT.md` / `config.json`（pre-existing dirty） + `merge-duplicate-cases-requirements.md` / `002-feat-merge-duplicate-cases-plan.md` / `frontend/src/api/jobs.ts` / `frontend/src/components/JobTable.tsx`（**完全无关**）
- handoff 漏列：`src/ralph_log_monitor/web.py` / `tests/test_web.py`（**实际改动的核心文件**）

**推测**：loop_runner 写 handoff 时用 `git status --porcelain` 当"changed"而不是 `git diff start_sha..HEAD --name-only`，导致混入了 pre-existing dirty + 跨 plan 污染。

**影响**：下次 handoff 报告 "已修改" 不可信；frontend 文件被列为"本次改动"会让人误以为 ralph 改 frontend。

**建议修复**：
1. `loop_runner.rs` 写 handoff 时改用 `git diff --name-only <start_sha>..HEAD`
2. 加测试覆盖 handoff "recently modified" 列表必须 = `git diff --name-only` 输出

---

### 🟡 P2-3：summary.md 状态与实际不符

**严重等级**：🟡 P2 — 状态报告不可信

**观察**：`summary.md:9-13` 写：
> No scratchpad found.
> No events recorded.

但实际：
- `agent/scratchpad.md` 存在（5638 字节）
- `events-20260602-032900.jsonl` 存在 45 行

**.ralph 证据**（绝对路径）：

| 路径 | 行号 | 内容 |
|------|------|------|
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/summary.md` | 1-3 | Header（Status / Iterations / Duration） |
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/summary.md` | 9 | `_No scratchpad found._` ← **错误** |
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/summary.md` | 13 | `_No events recorded._` ← **错误** |
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/summary.md` | 17 | `5a92daf5: fix(retry): 收尾 retry-runner-auto-start plan` ← 这条是对的 |

**实际存在**：

| 路径 | 大小/行数 |
|------|----------|
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/agent/scratchpad.md` | 5638 字节 |
| `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl` | 45 行 |

**推测**：summary 生成器从错误路径读，或读了已归档/清空的字段。

**建议修复**：在 `loop_runner.rs` 中加测试覆盖 summary.md 反映真实状态。

---

### 🟢 P3-1：shipper vs reporter `final_findings_count` 数字不一致

**严重等级**：🟢 P3 — 数字不一致，manager 报告不可信

**观察**：shipper 报 `final_findings_count: 3`，reporter 报 `final_findings_count: 4`。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl`）：

| 行号 | Hat | 字段值 |
|------|-----|--------|
| 42 | shipper | `"final_findings_count": 3` |
| 43 | reporter | `"final_findings_count": 4` |

**推测**：reporter 把 pre-existing 1 项 + 实际 residual 3 项 = 4；shipper 只算 3。两者定义不统一。

**影响**：manager 看到的两份数字不一样，无法做决策。

**建议修复**：
- reporter 显式区分 `in_diff_findings` 和 `pre_existing_findings`
- 或在 shipper 和 reporter 之间共享 finding 计数

---

### 🟢 P3-2：wave worker 失败事件重复发布

**严重等级**：🟢 P3 — 事件重复，消费者需去重

**观察**：每个失败的 worker 都发 2 条事件（`wave.worker.failed` + `review.dimension.done` 携带 FAILED payload）。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-032900.jsonl`）：

| 行号 | 事件类型 | 描述 |
|------|---------|------|
| 14 | `wave.worker.failed` | Worker 0 failed @ 04:21:06 |
| 15 | `review.dimension.done` (FAILED) | Worker 0 @ 04:21:06 |
| 16 | `wave.worker.failed` | Worker 1 failed @ 04:21:06 |
| 17 | `review.dimension.done` (FAILED) | Worker 1 @ 04:21:06 |
| 18 | `wave.worker.failed` | Worker 4 failed @ 04:21:06 |
| 19 | `review.dimension.done` (FAILED) | Worker 4 @ 04:21:06 |
| 20 | `wave.worker.failed` | Worker 5 failed @ 04:21:06 |
| 21 | `review.dimension.done` (FAILED) | Worker 5 @ 04:21:06 |
| 22 | `wave.worker.failed` | Worker 6 failed @ 04:21:06 |
| 23 | `review.dimension.done` (FAILED) | Worker 6 @ 04:21:06 |
| 33 | `wave.worker.failed` | Wave 2 Worker 0 failed @ 04:46:25 |
| 34 | `review.dimension.done` (FAILED) | Wave 2 Worker 0 @ 04:46:25 |
| 35 | `wave.worker.failed` | Wave 2 Worker 1 failed @ 04:46:25 |
| 36 | `review.dimension.done` (FAILED) | Wave 2 Worker 1 @ 04:46:25 |
| 37 | `wave.worker.failed` | Wave 2 Worker 3 failed @ 04:46:25 |
| 38 | `review.dimension.done` (FAILED) | Wave 2 Worker 3 @ 04:46:25 |
| 39 | `wave.worker.failed` | Wave 2 Worker 4 failed @ 04:46:25 |
| 40 | `review.dimension.done` (FAILED) | Wave 2 Worker 4 @ 04:46:25 |

**推测**：loop_runner 在 worker 失败时既发 `wave.worker.failed`（for TUI）又发 `review.dimension.done` with FAILED payload（for review-synthesizer 的 aggregate），两次消费场景不同。

**影响**：消费者（如 TUI / 监控）需要去重，否则会闪两遍失败信息。

**建议修复**：明确两种事件的语义边界（一个 for TUI，一个 for synthesizer），加注释说明。

---

### 🟢 P3-3：diagnostic log 文件重复创建

**严重等级**：🟢 P3 — diagnostic 子系统初始化并发问题

**观察**：`diagnostics/logs/` 下两个 log 文件时间戳只差 2ms（`-674-32059` vs `-676-32059`）。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/diagnostics/logs/`）：

| 文件名 | 大小 | mtime |
|--------|------|-------|
| `ralph-2026-06-02T11-29-00-674-32059.log` | 566 字节 | 6月 2 11:29 |
| `ralph-2026-06-02T11-29-00-676-32059.log` | 17649 字节 | 6月 2 11:29 |

**推测**：diagnostic 子系统初始化时并发创建了两次。小文件（566 字节）可能是空初始化，大文件（17649 字节）是真实日志。

**影响**：debug 时需先确认哪个 log 才是有内容的；log 文件数量预期外增长。

**建议修复**：加锁或单例化 diagnostic log writer。

---

### 🟢 P3-4：`loops.json` 始终为空

**严重等级**：🟢 P3 — loop registry 可能不记录 primary loop

**观察**：`loops.json` 内容是 `{"loops": []}`。本次主 loop 跑完 1.5 小时，registry 没记录。

**.ralph 证据**（`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/loops.json:1-3`）：

```json
{
  "loops": []
}
```

**推测**：`loops.json` 可能只记录 worktree 子 loop 或 explicit 注册的 loop，主 primary loop 默认不入册。

**影响**：用 `ralph loops` 命令查不到这次 run 的记录，loop 历史不可见。

**建议修复**：
1. 文档说明 `loops.json` 的记录范围
2. 或把 primary loop 也写入

---

## 4. 守住约束的部分（避免一面之词）

`.ralph/agent/handoff.md:1-40` 和事件流中可以看出 agent **守住了多条硬规则**：

| 约束 | 证据 | 路径 | 行号 |
|------|------|------|------|
| 没 push 到 origin | reflog 仅有 checkout + commit | `/home/chaowen/Dev/log_analyze/ralph-log-monitor/.git/` | reflog |
| 没创建/切换分支 | `HEAD@{0}: checkout: moving from master to fix/retry-auto-start-runner`（**用户操作**） | 同上 | reflog HEAD@{0} |
| 没改 frontend | `git diff --name-only 174b545d..5a92daf5` 仅 web.py / test_web.py + 2 doc | git diff 输出 | — |
| verdict_gate 配置正确 | `:32-38` preset 写明 | `presets/ce-executor.yml` | 32-38 |
| shipper 流程正确 | `REVIEW_COMPLETE` with `pass_or_fail: "pass"` | `/.ralph/events-20260602-032900.jsonl` | 42 |
| reporter 流程正确 | `LOOP_COMPLETE` 在 `report.done` 后 12 秒发出 | 同上 | 43-44 |
| 收尾 commit 收录 brainstorm | `5a92daf5` commit message 显式列 brainstorm 文档 | `git log 5a92daf5` | — |
| 31 个 pre-existing 测试失败已验证非本次引入 | `git stash` 验证前后 fail 数量一致 | `/.agents/scratchpad/.../fix-log.md` | Round 1 Verification 段 |

---

## 5. 修复优先级（按"信噪比 / 修复成本"排序）

| 序 | 问题 | 修复 | 修复成本 | 风险 |
|----|------|------|----------|------|
| 1 | P0-1 wave 超时 | `presets/ce-executor.yml:340-346` 加 `timeout: 900` | 1 行 yml | 低（仅延长，不改行为） |
| 2 | P2-1 synthesizer 文档误导 | `presets/ce-executor.yml:531-533` 加注释或删除 | 1-2 行 yml | 低 |
| 3 | P1-1 incremental commit | `presets/ce-executor.yml` executor instructions 加硬规则 | 5 行 yml | 中（agent 行为变化） |
| 4 | P1-2 hard_prereq 验证 | coordinator instructions 加 preflight + executor 强制跑 | 20-30 行 yml | 中 |
| 5 | P2-2 handoff recently-modified | `loop_runner.rs` 改用 `git diff --name-only <start_sha>..HEAD` | 5 行 rust | 低 |
| 6 | P2-3 summary.md 不准 | `loop_runner.rs` 改路径或加测试 | 10 行 rust | 低 |
| 7 | P3-* | 按需修 | — | — |

---

## 6. 验证计划

1. **短期（24h 内）**：修问题 1 + 问题 2，跑 `cargo test -p ralph-core` 无回归
2. **中期（一周内）**：在 ralph-log-monitor 仓库同 plan 重跑一次，对比 wave 失败 dimensions 数
3. **长期（一月内）**：把 ce-executor preset 的关键配置（timeout / aggregate / preflight）抽成 JSON schema 校验

---

## 7. 附录：核心源码交叉引用

| 主题 | 文件 | 行号 |
|------|------|------|
| Wave timeout 优先级 | `crates/ralph-core/src/wave_detection.rs` | 27-42 |
| AggregateConfig 定义 | `crates/ralph-core/src/config.rs` | 2759-2768 |
| aggregate_timeout 计算 | `crates/ralph-cli/src/loop_runner.rs` | 6123-6128 |
| per-worker timeout 触发 | `crates/ralph-cli/src/loop_runner.rs` | 6692-6706 |
| Timeout 错误消息生成 | `crates/ralph-cli/src/loop_runner.rs` | 6722-6733 |
| `dimension-reviewer` hat | `presets/ce-executor.yml` | 340-346 |
| `review-synthesizer` hat | `presets/ce-executor.yml` | 525-533 |
| Coordinator 完整定义 | `presets/ce-executor.yml` | 56-141 |
| Executor 完整定义 | `presets/ce-executor.yml` | 142-251 |
| Incremental Commit Strategy | `presets/ce-executor.yml` | 203-218 |
| 事件流 | `/.ralph/events-20260602-032900.jsonl` | 1-45（行 5-44 为 hat 事件） |
| handoff | `/.ralph/agent/handoff.md` | 1-40 |
| summary | `/.ralph/agent/summary.md` | 1-20 |
| scratchpad | `/.ralph/agent/scratchpad.md` | 1-145（5638 字节） |
| tasks | `/.ralph/agent/tasks.jsonl` | 3 条 task（U1/U2/U3 全部 closed） |
| fix-log | `/.agents/scratchpad/ce-executor/2026-06-02-001-fix-retry-runner-auto-start-plan/fix-log.md` | 1-30 |
| 实际改动文件 | `git diff 174b545d..5a92daf5 --name-only` | 4 个 |

---

**报告置信度**：高（10 个问题中 6 个有完整 .ralph 证据 + 源码证据 + 因果链可追溯；4 个 P3 是观察推断，需要后续验证）。

**报告作者**：ralph-debug session
**报告路径**：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-06-02-ce-executor-run-analysis.md`
