# parallel-forge preset author notes

## Scheduler Mode 执行面记录（2026-09-08，DAG runtime cutover）

- **现状（如实）**：YAML 声明 `event_loop.supervisor.scheduler_mode: dag`。DAG runtime 已接入 EventLoop acceptance 路径：审批后从 execution-plan dependency graph 做 admission，按 durable launch journal 启动 executor/reviewer/verifier，恢复未决 job，按 `integration_order` 串行 FF integration，并在 accepted `forge.unit.integrated`（含 close-task projection）后解锁后继 Unit。`forge.exec.development.done` 由 runtime 的 terminal fence 恰好一次发射。
- **已完成**：旧 wave 正常路径 hats（forge-dispatcher / worktree / LLM integrator / wave-fixer）及其 wave-only schema/topic 已从 builtin DAG 拓扑退役；静态 WAC/runtime-contract 已按 runtime control-plane 接缝同步，scripted preset verification 与 parallel-forge mock E2E 场景已通过。
- **仍属后续收口**：BDD×9、真实 authoritative DAG canary/crash matrix 仍待处理。author/review skill references、mock E2E、DAG 资源准入接线与 PMI-004/TG-S13 的 process-group 清理已完成。
- **inspect 语义**：`scheduler_mode ≠ wave` 时 `ralph inspect loop --format json` 输出只读 `scheduler` 块（空计数 = 零观测的如实反映，非伪造）。
- **promote 前置义务清单（follow-up 载体，本 notes 即 git-tracked 认领）**——任何「把 DAG 调度器接进 EventLoop」的 PR 必须同批交付，缺一即半接线（TG-S05 变红 = P0）：
  1. `DagSchedulerDriver::observe_accepted` 接入真实 EventLoop acceptance 路径。**已完成**：driver 消费 `forge.unit.*` per-unit 族，runtime 在 accepted 边界调用并继续推进 pipeline。
  2. `JobPipeline`/`DagPools`/`RuntimeJob` kernel promote 到非 `#[cfg(test)]`(**Step 1 已落地**,item 级 `#[allow(dead_code)]` 过渡标注),容量模型与 `max_concurrent_workers` 收敛为单一权威(plan D16:`dag_pools` 默认各等于 `max_concurrent_workers`)——**已落地**。
  3. U5 shadow parity（dag_shadow 在共同边界 exact parity）+ U10 §7 第 7 条 authoritative DAG canary（真实 runtime jobs + 临时 git worktrees + 完整 crash matrix）。**运行时主链已接通；canary 与 crash matrix 回归仍待补齐。**
  4. BDD ×9（plan U10 第 9 条：immediate refill / receipt crash / attempt forgery / env-path guard / sibling candidate / integrated task close / correction / resume / final once）。mock E2E 已完成；九个真实 EventLoop 场景仍待补齐。
  5. **已完成**：退休 wave 正常路径 hats（forge-dispatcher / worktree hat / LLM integrator / wave-fixer），清理孤儿 topic 并同步 schema。
  6. 文档同步：`CLAUDE.md`/`AGENTS.md` 过渡态段翻转 + `crates/ralph-core/data/*.md`（若 agent-facing 命令语义实际改变）。
  7. **已完成**：PMI-004 / TG-S13 第 3 项 process-group 语义已在 PTY kernel 实现，并由真实 shell descendant 超时回归测试覆盖。
- **回归 pin（不可删）**：TG-S05（`tg_s05_scheduler_mode_dag_wave_equivalence`，dag≡wave 事件序列逐条相等——变红即半接线，升 P0）+ TG-S06（**Step 2 已按 pin 内建指引翻转**:「真实 topic 喂 driver 必须 Ignored」随映射决策删除;静态 pin 翻转为正包含——driver topic 集合必须 ⊆ schema 的 `forge.unit.*` 族且族成员精确等于钉住的 6 个 topic;PMI-003 的 `compute_resource_aware_digest` 零生产消费 + `PHASE_*` 无 `as u8` 绑定两段保持原样——变红即族/接线失配,先同步再推进）。

## Preset Intent Confirmation（2026-08-31 关键阶段证据门禁）

- **目标：** 只有证据完整、payload 自洽、目标 worktree 身份稳定的关键 handoff 才能推进状态或执行最终 `git merge --ff-only`。非法 / 空 / 矛盾 / 证据不足的关键事件从「prompt 自律后仍推进」变为 `reject_with_resume` → 单 owner 修复；3 次耗尽后 runtime 注入 `forge.plan.blocked{kind=precheck_exhausted}`。
- **操作者与启动路径：** 沿用 `ralph run -H builtin:parallel-forge --plan …`。
- **输入与事实源：** producer 声明的路径 / 身份 / 分数字段；长文证据在 `.ralph/forge/<plan-key>/` 业务 artifact；目标稳定性证据为 `target_start_sha`（`git rev-parse HEAD`）与 `target_status_fingerprint`（canonical porcelain 的 SHA-256）。
- **成功条件：** 仅 accepted 关键事件推进状态：accepted `forge.unit.integrated` 的 runtime receipt 才允许对应 Unit 入账并关闭 task；accepted 且 `verdict=ACCEPTED` 的 `forge.audit.done` 才激活 finalizer 的真实 FF merge。
- **阻塞条件：** 任一关键位置连续 3 次被 precheck 拒绝 → runtime 发出 `forge.plan.blocked(reason=precheck_failed)` 且 `payload.kind=precheck_exhausted`，cleanup / reporter 由该 topic 唤醒。
- **允许的修改范围：** 仅 builtin `parallel-forge` 的 preset / schema / projection / hat instructions / ownership、对应 BDD 与本 notes；不改 consistency matcher、`build_exhausted_payload` 等 Rust 生产逻辑。
- **必须独立执行的评审：** 双 guard 位置（guard_selection=both）由合成 precheck gate hat 独立判断，gate 只检查与转发 / 拒绝，不替 producer 修证据；`payload_consistency` 在 LLM 之前做零成本结构矛盾拒绝。
- **重要 artifact：** execution-plan / worktree-map / review / settlement / verification / failures 等业务 artifact 由对应 hat 写入 `.ralph/forge/<plan-key>/`，payload 只携带路径与身份。
- **execution_model：** **supervisor+dag**
  **why：** runtime 以 `depends_on` DAG 做 admission，持久化 Unit job、attempt、worktree 与 recovery receipt；`integration_order` 只用于 deterministic merge，不得把 `execution_wave` 当硬串行边。
- **Gate Scope mode：** **hard** — `Confidence >= 85`、`Evidence Coverage >= 80`、`Verifiability >= 80`、`Impact Certainty >= 75`；`Critical Ambiguities = 0`、`Critical Unverified Assumptions = 0`（用户已批准；merge / task 边界必须 fail-closed）。
- **非目标：** 不改 supervisor 的 correction / attempt / 三轮 final correction 业务语义；不对 `forge.finalized` 做 LLM precheck；不扩展 consistency DSL（数值阈值只写进 precheck prompt）。
- **用户确认：** 已确认（开发计划 `docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` D1–D3、D16–D17）。

## Key-stage event gate（2026-08-31 证据门禁；0e 字段，confirmation_status=confirmed）

逐位置 guard 选择与各自独立 retry budget；双 guard 的 consistency `rule.topic` 必须写 `<T>.proposed`（先于 LLM 拒绝，避免打在 gate 转发上）。`payload_consistency_retry_budget: 3` 是记录值：consistency 走现有 3-strike runtime 语义，YAML **无**该字段，禁止发明。

| key_stage | topic | guard_selection | precheck_guard | precheck_retry_budget | payload_consistency_guard | payload_consistency_retry_budget | reason | confirmation_status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| admission | `forge.concurrency.approved` | both | true | 3 | true | 3 | 并发许可与 Unit 身份 / 计划摘要一致才可启动 job | confirmed |
| unit review | `forge.unit.reviewed` | both | true | 3 | true | 3 | review artifact 与 Unit verdict 一致才可进入 integration | confirmed |
| unit integration | `forge.unit.integrated` | both | true | 3 | true | 3 | 仅 accepted integration receipt 才允许 runtime 投影 task close | confirmed |
| merge auth | `forge.audit.done` | both | true | 3 | true | 3 | 最终 merge 只能被 ACCEPTED 且目标未漂移的 audit 激活 | confirmed |
| terminal failure | `work.failed` | both | true | 3 | true | 3 | 死胡同账单唯一 publisher，需 dead-end 证据阈值 precheck | confirmed |
| dev fan-in | `forge.exec.development.done` | payload_consistency | false | null | true | 3 | 高频收据 topic，仅零 LLM 结构矛盾拒绝，不加 LLM gate | confirmed |
| full verify | `forge.full.verified` | payload_consistency | false | null | true | 3 | 同上；false-success 由 consistency 拒绝 | confirmed |
| post-merge | `forge.finalized` | payload_consistency | false | null | true | 3 | 同上；不可逆动作已在 audit 门禁前挡住 | confirmed |
| report | `forge.report.done` | payload_consistency | false | null | true | 3 | 同上；status / final_audit 矛盾由 consistency 拒绝 | confirmed |

## schema 结论

本增量在 `presets/schemas/parallel-forge.yml` 新增 / 收紧关键 topic 的 `required_fields` 与 `field_docs`（扇出身份字段、typed failure 收据等），并新增 `forge.full.verification.failed` topic；具体字段契约以 schema SSOT 与各 Unit 的 BDD 为准，本 notes 不复述字段级清单。
