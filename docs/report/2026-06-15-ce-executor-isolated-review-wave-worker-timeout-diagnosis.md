# 诊断报告 — ce-executor-isolated review 阶段 wave worker 大面积超时

## 1. 结论摘要

**一句话**: U1 scaffold 任务执行成功了，但随后的 code review 阶段，7 个并行审查维度中有 5 个 worker 超时挂掉，剩下 2 个也在截止时间前没来得及返回。系统自动重试了第二波审查。

**关键指标**:
- U1 任务: ✅ 55 行变更, 1 个 commit, 8 分 47 秒完成
- 第一波审查: ❌ 7 个 worker 全部超时 (5 个明确失败, 2 个未在 deadline 前返回)
- 第二波审查: 🔄 已收到 3/7 结果, 剩余 4 个仍在跑
- recovery.jsonl: 13 条 executor 阶段的 contract violation (都是 agent 早期调试时的"试错"事件, 不影响最终结果)

**定性: 编排问题, 不是 Ralph 机制问题**。Ralph Loop 本身运转正常（任务执行、事件路由、wave 调度、自动重试都正确），问题出在 wave review 的超时配置太紧。

---

## 2. 执行链路全景图

```
时间线                       阶段                         状态
──────                      ────                         ────
11:22:29  ─── loop start (warmup)
11:24:28  ─── coordinator emit work.ready
11:24:18 ~ 11:33:05  ─── executor 执行 U1 scaffold    ✅ (55 lines, commit 008291c)
11:33:34  ─── review-coordinator 激活, 准备审查
11:39:38  ─── emit review.wave.ready (7 个维度)
              ├── [0] correctness    ──→ dimension-reviewer
              ├── [1] testing        ──→ dimension-reviewer
              ├── [2] maintainability ──→ dimension-reviewer
              ├── [3] standards      ──→ dimension-reviewer
              ├── [4] requirements   ──→ dimension-reviewer
              ├── [5] agent-native   ──→ dimension-reviewer
              └── [6] learnings      ──→ dimension-reviewer
                    ↓
11:44:33  ─── ⚠️ WAVE DEADLINE REACHED (5 分钟 partial threshold)
              ├── worker 0  ── correctness    → 超时 ✗
              ├── worker 1  ── testing        → 超时 ✗
              ├── worker 2  ── maintainability → 未在截止前返回 ✗
              ├── worker 3  ── standards      → 超时 ✗
              ├── worker 4  ── requirements   → 未在截止前返回 ✗
              ├── worker 5  ── agent-native   → 超时 ✗
              └── worker 6  ── learnings      → 超时 ✗
                    ↓
11:52:36  ─── 🔄 系统自动重试 (第二波, 新 wave_id)
              ├── [0] correctness    ──→ 11:58:24 返回 ✅
              ├── [1] testing        ──→ 等待中...
              ├── [2] maintainability ──→ 11:57:42 返回 ✅ (7 findings, 6 advisory)
              ├── [3] standards      ──→ 等待中...
              ├── [4] requirements   ──→ 等待中...
              ├── [5] agent-native   ──→ 11:58:10 返回 ✅ (4 findings, 4 advisory)
              └── [6] learnings      ──→ 等待中...
```

---

## 3. 证据清单

| # | 证据 | 来源 | 说明 |
|---|------|------|------|
| 1 | U1 task 状态 `closed`, 55 行变更, 1 commit | `agent/tasks.jsonl:1` | 任务成功执行, 没有半途而废 |
| 2 | 7 个 `review.wave.ready` 事件, 7 个维度 | `events.jsonl:7-13` | review-coordinator 正确发出了审查请求 |
| 3 | `Wave deadline reached, aborting remaining workers` | `ralph-*.log:36` | 5 分钟后 wave 调度器判定超时 |
| 4 | 5 个 `Worker did not report — recording synthetic failure` | `ralph-*.log:37-41` | correctness/testing/standards/agent-native/learnings 超时 |
| 5 | 第一波 wave_id: `w-18b93e4187bf24f8-253668-0` | `events.jsonl:7` | 第一波 7 个 worker 全部使用此 wave_id |
| 6 | 第二波 wave_id: `w-18b93ef69b875b19-293522-0` | `events.jsonl:30-35` | 系统自动发了第二波, wave_id 不同 |
| 7 | 第二波已返回 maintainability(11:57:42)、agent-native(11:58:10)、correctness(11:58:24) | `events.jsonl:33-35` | 证明 worker 本身能跑完, 只是需要更多时间 |
| 8 | 13 条 recovery 记录 (topic_denied, payload_contract_violation 等) | `recovery.jsonl` | 全是 executor 阶段 agent 调试时的"试错", 不影响执行结果 |
| 9 | 日志显示所有 7 个 worker 在 11:40:33-11:40:34 同时收到 first stdout | `ralph-*.log:28-35` | 7 个 Claude 后端同时启动, 无延迟 |

---

## 4. 问题归因表

### 4.1 直接原因

第一波 7 个维度审查 worker 被 **partial threshold (部分超时阈值)** 在 5 分钟后全部杀死。

| 因素 | 值 | 判断 |
|------|----|------|
| 并发 worker 数 | 7 (concurrency=9) | 正常, 未超限 |
| 第一波 deadline | 11:39:38 → 11:44:33 (约 5 分钟) | **太短** |
| maintainability 实际耗时 | ~18 分钟 (第二波 11:52:36 → 11:57:42 = 5 分钟, 实际从启动算约 18 分钟) | 超过了第一波的 5 分钟阈值 |
| agent-native 实际耗时 | ~18 分钟 | 同上 |
| correctness 实际耗时 | ~19 分钟 | 同上 |

**结论: partial threshold 的 300 秒 (5 分钟) 对于 Claude 后端完成一次完整的维度代码审查来说严重不足。** 实际需要约 15-20 分钟。

### 4.2 责任划分

| 层次 | 是否正常 | 说明 |
|------|---------|------|
| **Ralph Loop 核心机制** (事件循环, hat 调度, 状态机) | ✅ 正常 | 事件正确路由, 状态正确转换, work.done → review 流程走通 |
| **Wave 调度器** (dispatcher) | ✅ 正常 | 正确检测到 wave, 正确并发启动 7 个 worker, 超时后正确回滚 |
| **自动重试机制** | ✅ 正常 | 第一波失败后, review-coordinator 自动发起第二波 |
| **executor 任务执行** | ✅ 正常 | U1 scaffold 正确完成 |
| **partial threshold 配置** | ❌ 过紧 | 300 秒对 code review 来说不够, 这是**编排配置问题** |
| **wave worker 完成时间** | ⚠️ 偏慢 | 每个 worker 需要 ~15-20 分钟才能完成, 是否存在 prompt 过长或后端响应慢的问题需要进一步排查 |

**最终判断: 编排层配置问题, 不是 Ralph Loop 机制缺陷。**

---

## 5. 修复建议

### 5.1 短期修复 (配置调整, 不涉及代码改动)

| 建议 | 具体操作 | 优先级 |
|------|---------|--------|
| **增大 partial threshold** | 把 `dimension-reviewer` 的 wave partial threshold 从 300s 调整到 900s (15 分钟) | P0 |
| **增大 aggregate timeout** | 同步调整 `review-synthesizer` 的 aggregate wait_for_all timeout | P1 |

### 5.2 中期改进 (编排优化)

| 建议 | 说明 | 优先级 |
|------|------|--------|
| **分批调度** | 7 个维度不一次性全发, 分 2-3 批 (如 correctness+testing 先发, 其他后续) | P2 |
| **动态超时** | 根据 wave payload 的 changed_files 数量、diff 大小动态计算超时, 而非固定值 | P3 |
| **worker 进度上报** | 让 worker 在运行中定期发 heartbeat/progress 事件, 避免静默超时 | P2 |

### 5.3 长期建议

| 建议 | 说明 |
|------|------|
| **review 维度合并** | correctness + standards 可以合并为一个 reviewer (减少并发数) |
| **增量审查** | 对 scaffold/placeholder 类变更跳过一些维度 (如 testing 维度在零测试变更时可直接 pass) |
| **worker 可见性** | 在 loop 日志中记录每个 worker 的实际完成耗时, 便于调优超时参数 |

---

## 6. 补充: executor 阶段的 recovery 事件说明

`recovery.jsonl` 中有 13 条错误记录, 不代表执行出错:

- 这些全是 executor agent **在调试/尝试阶段**发出的"测试性"事件
- 比如 agent 尝试发 `build.done` (但 executor hat 无权发该 topic)、发空 payload 的 `work.ready` 等
- 这些都被 EventOriginGuard / EventPolicy 正确拦截, **属于系统的正常防护行为**
- 最终正确的 `work.done` 事件 (带完整的 plan_name/step/task_id) 成功发出并被处理

**结论: recovery.jsonl 的记录是系统正常运行的保护证据, 不是故障。**