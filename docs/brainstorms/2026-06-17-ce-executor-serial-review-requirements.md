---
date: 2026-06-17
topic: ce-executor-serial-review
related:
  - docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md
  - docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md
  - presets/en/ce-executor-isolated.yml
  - presets/en/ce-executor-wave.yml
---

# ce-executor-serial Review Preset — 需求文档

## Problem Frame

Operator 对 `ce-executor-isolated` 的并行 review wave 失去信心：多次运行中 worker 要么不返回、要么审错维度，最终触发 `plan.blocked(reason=dimension_reviewers_failed_to_converge)`，loop 无法完成。虽然机制层做了大量恢复（task.resume、fixer、debug-resolver、progress-steward），但这些恢复路径被 R6 `plan.blocked` 结构性截断——一旦 wave 不收敛，loop 就 fail。

本需求提供一个 **串行 review 版本** 的 preset，把 4 个 dimension-reviewer 从并发执行改成顺序执行，先让 loop 能稳定跑完，再视情况叠加维度强制校验。

---

## Actors

- A1. **review-coordinator agent**：选择并 emit `review.wave.ready`。
- A2. **dimension-reviewer worker agent**：按分配维度审代码并 emit `review.dimension.done`。
- A3. **review-synthesizer agent**：聚合全部维度结果。
- A4. **operator**：使用新的串行 preset 跑 plan，希望 review 阶段稳定完成。

---

## Key Flows

- F1. **正常 4 维串行 review（回归/目标路径）**
  - **Trigger:** work.done 触发 review-coordinator。
  - **Actors:** A1, A2 × 4, A3
  - **Steps:** review-coordinator emit 4 维 wave → dispatcher concurrency=1 逐个 spawn worker → worker 1 correctness → worker 2 testing → worker 3 maintainability → worker 4 requirements → 4 条 dimension.done 全部返回 → synthesizer 聚合 → verdict
  - **Outcome:** wave 正常关闭，无 `dimension_reviewers_failed_to_converge`。
  - **Covered by:** R1, R2, R4

- F2. **单 worker 超时/失败（串行下的局部失败）**
  - **Trigger:** worker 2 (testing) 超时或崩溃。
  - **Actors:** A2, dispatcher
  - **Steps:** dispatcher 等待该 worker 到 per-worker timeout → 标记 failure → 继续/终止取决于配置
  - **Outcome:** 失败被定位到具体维度，不因为并发混乱而淹没信号。
  - **Covered by:** R3, R5

---

## Requirements

**Preset 定义**

- R1. 新增 builtin preset `ce-executor-serial`（或等价的 `ce-executor-isolated` 配置开关），拓扑与 `ce-executor-isolated` 一致，仅 review 阶段改为串行。
- R2. `dimension-reviewer.concurrency` 必须设为 `1`，确保同一 wave 内 worker 顺序执行。
- R3. `dimension-reviewer.timeout` 保持 per-worker 上限；`review-synthesizer.aggregate.timeout` 必须覆盖 4 个串行 worker 的最坏情况（建议从 1800s 上调到 3600s 或按 `per_worker_timeout × wave_total` 自动计算）。
- R4. `review-coordinator.instructions` 必须明确：串行 preset 下仍 emit **一个** `review.wave.ready` wave（wave_total=4），由 dispatcher 负责顺序派发；不要改成 4 个独立 wave。

**Dispatcher 行为**

- R5. dispatcher 必须识别 `hat_config.concurrency == 1` 并进入串行派发模式： worker 0 完成后才启动 worker 1，依次类推。
- R6. 串行模式下，dispatcher 的 `partial_deadline` 计算必须基于已启动 worker 的累计耗时，避免第一个 worker 还没跑完就被 partial 阈值误杀；或干脆在串行模式下禁用 partial early-exit，只保留 aggregate timeout。
- R7. 串行模式下，任意 worker 失败（超时/非零退出/mismatch）必须立即记录 `wave.worker.failed`，并可选择：
  - 继续下一个维度（把失败维度记为 missing）；
  - 暂停 wave 并注入 targeted `task.resume` 重试该 slot。
  默认策略必须在 preset 中显式声明。

**Synthesizer 与终态**

- R8. `review-synthesizer` 保持 `aggregate.mode: wait_for_all`；它应在 4 条 dimension.done（含 synthetic missing/failure 信号）全部到达后激活。
- R9. 当串行 wave 因某个 worker 失败而无法收齐 4 维时，synthesizer 必须收到 `missing_dimensions` 列表，并按现有 preset 协议决定 verdict 或 `plan.blocked`。

**Preset 清单同步**

- R10. `presets/manifest.yml`、`crates/ralph-cli/src/presets.rs`、`presets/index.json`、`scripts/ralph-zsh-plugin.zsh` 必须同步新增 `ce-executor-serial` 条目（与 AGENTS.md Presets & Hats 段要求一致）。
- R11. 新增 preset 文件 `presets/en/ce-executor-serial.yml`；中文变体 `presets/zh/ce-executor-serial-zh.yml` 可选但建议同步。

**回归与验证**

- R12. `cargo nextest run --workspace --exclude ralph-e2e` 通过；现有 wave 测试不应因新增 preset 而失败。
- R13. 新增 smoke/replay fixture 或 BDD scenario：4 维串行 wave 正常完成，验证 dispatcher 确实只并发 1 个 worker。
- R14. `ralph preset check builtin:ce-executor-serial` 通过 strict check。

---

## Acceptance Examples

- AE1. **Covers R1, R2, R5, F1**
  - **Given:** operator 跑 `ralph -H builtin:ce-executor-serial run ...`
  - **When:** review-coordinator emit 4 维 `review.wave.ready`。
  - **Then:** dispatcher 在同一迭代内顺序启动 4 个 worker，任意时刻只有 1 个 dimension-reviewer 在运行；最终 4 条 dimension.done 全部写入。

- AE2. **Covers R3, R6**
  - **Given:** 每个 worker 最坏 8 分钟，串行共需 ~32 分钟。
  - **When:** wave 运行。
  - **Then:** aggregate timeout ≥ 32 分钟（或 3600s），不在 24 分钟时被 R6/incomplete_wave_gate 截断。

- AE3. **Covers R7, F2**
  - **Given:** worker 2 (testing) 超时。
  - **When:** dispatcher 处理该 failure。
  - **Then:** 记录 `wave.worker.failed` 含 wave_index=2、dimension=testing；后续策略（继续或重试）按 preset 声明执行。

---

## Success Criteria

- SC1：operator 使用 `ce-executor-serial` 跑 keen-fern 类 plan 时，review wave 能稳定收敛到 4 个维度结果，不再因并发调度失败。
- SC2：串行 preset 与并行 preset 行为隔离，切换 preset 不影响其他用户/loop。
- SC3：现有并行 wave 测试与 scenario 不回归。
- SC4：preset 清单 4 处同步完成，无遗漏。

---

## Scope Boundaries

- **本次覆盖**：
  - 新增串行 review preset；
  - dispatcher 对 `concurrency=1` 的串行派发语义；
  - aggregate timeout 适配；
  - 预设清单同步。
- **本次不覆盖**：
  - 删除或修改 `ce-executor-isolated` 并行 preset；
  - 移除 wave 抽象或改成非 wave 串行 review；
  - 维度强制校验（见姊妹文档 `2026-06-17-wave-dimension-assignment-enforcement-requirements.md`，可与本文并行或后续叠加）；
  - 修改 review-coordinator 维度选择策略；
  - 修复 keen-fern 报告中 U1 残留。

---

## Key Decisions

- **单 wave 内串行，不是 4 个独立 wave**：保留 `review-synthesizer.aggregate.wait_for_all` 语义，改动面最小；4 个独立 wave 会迫使 synthesizer 改为计数器触发，引入新复杂度。
- **新增 preset，不改默认**：保留 `ce-executor-isolated` 作为并行版本，让 operator 按需选择；默认仍是并行，避免影响其他用户。
- **per-worker timeout × wave_total 作为 aggregate timeout 参考**：串行总 wall time 是并行的 N 倍，timeout 必须跟上，否则会从「并发失败」变成「串行超时失败」。
- **先串行、后强制**：串行解决的是并发调度类 failure；若串行后仍出现审错维度，再叠加 `wave-dimension-assignment-enforcement` 的绑定/校验。

---

## Dependencies / Assumptions

- 假设 dispatcher 已支持 `concurrency` 参数控制并发度（`wave.hat_config.concurrency`）。
- 假设 `review-synthesizer.aggregate.timeout` 可被 preset 覆盖。
- 假设新增 builtin preset 的流程与 `ce-executor-isolated` 一致（manifest / presets.rs / index.json / zsh 补全 4 处同步）。

---

## Outstanding Questions

### Resolve Before Planning

（无 — 已明确走新增 preset + 单 wave 内串行。）

### Deferred to Planning

- **[Technical]** 串行模式下的 `partial_deadline` 是否直接禁用？还是改为「当前 worker 已运行时间 > per-worker timeout × 0.8」？
- **[Technical]** worker failure 后的默认策略：继续下一个维度 vs 暂停并重试该 slot？是否暴露为 preset 配置项？
- **[Technical]** 是否需要新增 dispatcher 集成测试断言「concurrency=1 时同时运行的 worker 数 ≤ 1」？
- **[User decision]** 中文变体 `presets/zh/ce-executor-serial-zh.yml` 是否首版就提供？

---

## Next Steps

→ `/ce-plan` 生成实施计划（建议文件 `docs/plans/2026-06-17-002-feat-ce-executor-serial-review-plan.md`）。

→ 可与 `2026-06-17-wave-dimension-assignment-enforcement` 并行或分阶段落地：先串行 preset 让 loop 能跑完，再叠加维度强制校验把审错维度也兜底。
