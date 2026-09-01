# parallel-forge preset author notes

## Preset Intent Confirmation（2026-08-31 关键阶段证据门禁）

- **目标：** 只有证据完整、payload 自洽、目标 worktree 身份稳定的关键 handoff 才能推进状态或执行最终 `git merge --ff-only`。非法 / 空 / 矛盾 / 证据不足的关键事件从「prompt 自律后仍推进」变为 `reject_with_resume` → 单 owner 修复；3 次耗尽后 runtime 注入 `forge.plan.blocked{kind=precheck_exhausted}`。
- **操作者与启动路径：** 沿用 `ralph run -H builtin:parallel-forge --plan …`。
- **输入与事实源：** producer 声明的路径 / 身份 / 分数字段；长文证据在 `.ralph/forge/<plan-key>/` 业务 artifact；目标稳定性证据为 `target_start_sha`（`git rev-parse HEAD`）与 `target_status_fingerprint`（canonical porcelain 的 SHA-256）。
- **成功条件：** 仅 accepted 关键事件推进状态：accepted `forge.wave.settled` 才触发 `CloseTaskBatch`；accepted 且 `verdict=ACCEPTED` 的 `forge.audit.done` 才激活 finalizer 的真实 FF merge。
- **阻塞条件：** 任一关键位置连续 3 次被 precheck 拒绝 → runtime 发出 `forge.plan.blocked(reason=precheck_failed)` 且 `payload.kind=precheck_exhausted`，cleanup / reporter 由该 topic 唤醒。
- **允许的修改范围：** 仅 builtin `parallel-forge` 的 preset / schema / projection / hat instructions / ownership、对应 BDD 与本 notes；不改 consistency matcher、`build_exhausted_payload` 等 Rust 生产逻辑。
- **必须独立执行的评审：** 双 guard 位置（guard_selection=both）由合成 precheck gate hat 独立判断，gate 只检查与转发 / 拒绝，不替 producer 修证据；`payload_consistency` 在 LLM 之前做零成本结构矛盾拒绝。
- **重要 artifact：** execution-plan / worktree-map / review / settlement / verification / failures 等业务 artifact 由对应 hat 写入 `.ralph/forge/<plan-key>/`，payload 只携带路径与身份。
- **execution_model：** **supervisor+wave**
  **why：** 与 YAML `supervisor.enabled: true` 及 dispatcher `ralph wave emit exec.unit.ready` 波次调度一致；worker admission 由 `depends_on` DAG 决定（handoff 阶段按 DAG 重算最早安全 wave），`integration_order` 只用于 deterministic merge，不得把 `execution_wave` 当硬串行边。
- **Gate Scope mode：** **hard** — `Confidence >= 85`、`Evidence Coverage >= 80`、`Verifiability >= 80`、`Impact Certainty >= 75`；`Critical Ambiguities = 0`、`Critical Unverified Assumptions = 0`（用户已批准；merge / task 边界必须 fail-closed）。
- **非目标：** 不改 supervisor / wave / slot retry / 三轮 final correction 业务语义；不给 `exec.unit.done` 加 guard；不对 `forge.finalized` 做 LLM precheck；不扩展 consistency DSL（数值阈值只写进 precheck prompt）。
- **用户确认：** 已确认（开发计划 `docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` D1–D3、D16–D17）。

## Key-stage event gate（2026-08-31 证据门禁；0e 字段，confirmation_status=confirmed）

逐位置 guard 选择与各自独立 retry budget；双 guard 的 consistency `rule.topic` 必须写 `<T>.proposed`（先于 LLM 拒绝，避免打在 gate 转发上）。`payload_consistency_retry_budget: 3` 是记录值：consistency 走现有 3-strike runtime 语义，YAML **无**该字段，禁止发明。

| key_stage | topic | guard_selection | precheck_guard | precheck_retry_budget | payload_consistency_guard | payload_consistency_retry_budget | reason | confirmation_status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| initial fan-out | `forge.worktrees.ready` | both | true | 3 | true（规则 topic=`.proposed`） | 3 | 扇出身份 / 快照证据不足会污染全部波次，必须 fail-closed | confirmed |
| lazy wave fan-out | `forge.wave.worktrees.ready` | both | true | 3 | true | 3 | 惰性波次扇出同属关键 handoff，复用同一身份证据纪律 | confirmed |
| review | `forge.wave.reviewed` | both | true | 3 | true | 3 | review artifact 与 unit_verdicts 一致才可进入 integration | confirmed |
| settlement | `forge.wave.settled` | both | true | 3 | true | 3 | 仅 accepted settlement 才允许 `CloseTaskBatch` 原子关 task | confirmed |
| merge auth | `forge.audit.done` | both | true | 3 | true | 3 | 最终 merge 只能被 ACCEPTED 且目标未漂移的 audit 激活 | confirmed |
| terminal failure | `work.failed` | both | true | 3 | true | 3 | 死胡同账单唯一 publisher，需 dead-end 证据阈值 precheck | confirmed |
| dev fan-in | `forge.exec.development.done` | payload_consistency | false | null | true | 3 | 高频收据 topic，仅零 LLM 结构矛盾拒绝，不加 LLM gate | confirmed |
| full verify | `forge.full.verified` | payload_consistency | false | null | true | 3 | 同上；false-success 由 consistency 拒绝 | confirmed |
| post-merge | `forge.finalized` | payload_consistency | false | null | true | 3 | 同上；不可逆动作已在 audit 门禁前挡住 | confirmed |
| report | `forge.report.done` | payload_consistency | false | null | true | 3 | 同上；status / final_audit 矛盾由 consistency 拒绝 | confirmed |
| slot done | `exec.unit.done` | neither | false | null | false | null | 高频 slot 完成事件；加 LLM gate 会串行化 slot，明确禁止 | confirmed |

## schema 结论

本增量在 `presets/schemas/parallel-forge.yml` 新增 / 收紧关键 topic 的 `required_fields` 与 `field_docs`（扇出身份字段、typed failure 收据等），并新增 `forge.full.verification.failed` topic；具体字段契约以 schema SSOT 与各 Unit 的 BDD 为准，本 notes 不复述字段级清单。
