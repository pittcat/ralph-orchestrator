# 2026-06-21 Ralph 主仓机制/编排/Bug 综合诊断报告

> **生成时间**:2026-06-21
> **诊断对象**:`/Users/pittcat/Dev/Rust/ralph-orchestrator/` 主仓(非单次 run)
> **诊断焦点**:机制问题、编排问题、bug
> **对照对象**:`presets/en/ce-executor-serial.yml` + 主仓未提交改动(12 M + 1 新增) + 历史 180+ 份诊断文档
> **执行方式**:4 sub agent 并行(流程还原 / 历史上下文 / 对账分析 / 归因修复)
> **触发 run**:`ralph-e2e/.ralph/primary-20260621-015519`(3 iter, 12m51s, cancelled, executor_unresponsive)

---

## 1. 结论摘要

**主仓当前健康度:亚健康(7 项 P0 + 8 项 P1 + 6 项 P2,主要未闭环)**

- **关键异常数量**:P0=7,P1=8,P2=6(共 21 项),其中 10 项与历史 30 天反复出现的问题强关联
- **是否涉及历史重复问题**:**是** — hat_handoff 0 触发(已反复 5 次,Phase 2 待验收)、ralph 越权发业务事件(4 次复发)、recovery.jsonl 噪声升级 drift 缺失(2 次复发)、state_projection 应用未完成(2 次复发)
- **核心矛盾**(Agent B 结论):30 天打了 5 个 patch 都没收敛,4 个根因耦合必须一起改 — 共享可变状态 + 散落 schema + fail-after + 软提示
- **成功率基线**:serial preset 跑完 plan 全部 U 的成功率 < 50%(5 次中 0 次全闭环)
- **当前未提交改动**(`git status` 12 个 M + 1 个新增 `state_projection.rs`):与上述问题强相关,正在修,但 **P0-3(`ProtocolView` 未传 `HandoffIndex`)、P0-4(`is_macro_edge` 自环未实现)、P0-6(`LintResumeHint` typed 路由漏改 engine gate)三个机制级 bug 仍存在**

---

## 2. 执行链路对比图

### 2.1 期望链路(ce-executor-serial,9 步 + 1 旁路)

引用 `presets/en/ce-executor-serial.yml:393-2189`,10 hat 拓扑与 SSOT 拆分在 `presets/schemas/ce-executor-serial.yml`(build.rs deep-merge):

```
[0] work.start → coordinator 激活
[1] coordinator → work.ready(宏观边,需 handoff_path)
[2] executor → work.done(微观边,git_change 强制)
[3] review-coordinator → 4 维 review.dimension.ready 串行(correctness → testing → maintainability → requirements,fix_round=0)
[4] dimension-reviewer × 4 → review.dimension.done
[5] review-synthesizer → review.passed / failed / complete
[6] fixer(3 轮内) → fix.applied(微观边,commit_only)→ re-walk review(每次 fix_round++)
[7] plan-gate → queue.advance + work.ready(双发)或 plan.complete
[8] shipper → REVIEW_COMPLETE
[9] reporter → report.done + LOOP_COMPLETE(pass 路径)
[旁路] progress-steward(loop.stalled) → work.ready / review.dimension.ready / queue.advance / plan.blocked
```

### 2.2 实际 run 链路(`primary-20260621-015519`,cancelled,12m51s,3 iter)

```
[0] work.start 落盘 ✅ (01:55:19,loop-bootstrap,event #1)
[1] coordinator attempt 1..10 全部 CLI 拒收
    - recovery #1  01:56:40 hat_handoff_missing_path
    - recovery #2-5 01:57-01:59 hat_handoff_filename_mismatch(iter=0 vs 期望 iter=1)
    - recovery #6  01:59:33 hat_handoff_structure_invalid(缺 ## changed)
    - recovery #7  01:59:44 hat_handoff_structure_invalid(缺 ## verify)
    - recovery #8  02:03:23 missing_required_field:plan_name
    - recovery #9  02:03:34 hat_handoff_filename_mismatch(seq=2 vs 期望 seq=1)
    - recovery #10 02:03:48 hat_handoff_structure_invalid(## next 含非法字段)
[1] event #2 落盘(01:59:54,coordinator work.ready)— 绕开 CLI gate 由 loop_runner 直发
[1] event #3 落盘(02:04:04,ralph work.ready)— ralph 越权发业务 topic
[旁路] event #4 task.resume 落盘(02:06:52,ralph,reason=stall_no_events,target_hat=ralph)
        但 recovery #11 (02:06:48) 拒收同 payload(缺 target_hat)
[终止] event #5 loop.cancel 落盘(02:07:48,ralph,reason=executor_unresponsive)
[终止] loop.terminate(02:08:11,Iterations=3,Duration=12m51s,Exit=0)
```

### 2.3 对比标注

| 期望步骤 | 实际状态 | 标记 |
|---|---|---|
| [0] work.start | event #1 落盘 | ✅ |
| [1] coordinator → work.ready | 10 次 CLI 拒收 + 2 次落盘(1 绕过、1 ralph 越权) | ⏸️ |
| [2] executor → work.done | 未激活(0 work.done 落盘) | ❌ |
| [3] review-coordinator | 未触发 | ❌ |
| [4] dimension-reviewer × 4 | 未触发 | ❌ |
| [5] review-synthesizer | 未触发 | ❌ |
| [6] plan-gate | 未触发 | ❌ |
| [7] shipper(REVIEW_COMPLETE) | 未触发 | ❌ |
| [8] reporter(LOOP_COMPLETE) | 未触发 | ❌ |
| [终止] loop.cancel | event #5 落盘,触发 Cancelled 终止 | ✅ |
| [旁路] progress-steward | 未触发(stall 阈值未达 / ralph 制造假活跃) | ❌ |

**核心结论**:整个 preset workflow 在 [1] 步就阻断了,没进入 review-synthesizer,REVIEW_COMPLETE / LOOP_COMPLETE / report.done 全部未发。终止类型 = Cancelled(`TerminationReason::Cancelled`,`event_loop/mod.rs:165`),与 `loop.cancel` 事件匹配。

---

## 3. 历史问题上下文(关联度标注)

### 3.1 历史重复问题全景(14 类,30 天数据)

| 类别 | 历史次数 | 关联度 | 当前状态 |
|---|---|---|---|
| 1. review-synthesizer 卡死 / 永不 fire | 6 | 高 | 部分闭环(zippy-sparrow `44b9240`),部分复发 |
| 2. CLI precheck / loop runtime 双轨漂移 | 7 | 高 | hat=None 旁路 2026-06-19 复发 |
| 3. task.resume payload 字段缺失 | 8 | 高 | 闭环(`d19b755`),hat=None bypass 残留 |
| 4. isolated scope 越权 emit 先落盘后 drop | 9 | 高 | U1 闭环,ralph 业务 topic 漏拦 |
| 5. plan-gate triggers 桥接缺口 | 5 | 高 | 闭环,被 perky-maple 误用 KTD3 |
| 6. dimension-reviewer 死锁 / 重复 ready | 5 | 高 | 部分闭环,wave 11→4 缺维 |
| 7. fix.applied dedup 永久阻断 re-review | 1 | 高 | 闭环(perky-maple KTD1) |
| 8. **hat_handoff 0 触发 / 链路完全失效** | 5 | **高(本次主因)** | **未闭环** — Phase 2 待 SC-1 验收 |
| 9. wave worker 共享状态 / 9-worker 抽象错 | 4 | 高 | **未闭环** — 需 Supervisor 协议升级 |
| 10. recovery.jsonl 噪声占主导(135+ 条) | 4 | 中 | 闭环(U1+U3) |
| 11. diagnosis-summary recovery_count 硬编码 0 | 3 | 中 | 闭环(`6a9cd24`) |
| 12. 重复 emit(同 payload) | 3 | 中 | 部分闭环 |
| 13. executor 6 轮探针风暴 | 1 | 高 | 闭环(`bfc9ced` `419258e`) |
| 14. agent 输出源码树污染 | 2 | 中 | 部分闭环(R3) |

### 3.2 本次 run 与历史问题的关联

| 本次 run 现象 | 历史同根 | 历史报告 |
|---|---|---|
| coordinator 0 handoff artifact / 10 次 hat_handoff_* retry | warm-tiger §1 + primary-20260619 §B1-B4 | `2026-06-19-warm-tiger-loop-diagnosis.md` + `2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` |
| ralph 越权发 work.ready + task.resume 仍落盘 | warm-tiger §1.3 + merry-lotus | `2026-06-19-warm-tiger-loop-diagnosis.md` L23-26 + `2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` |
| progress-steward 未被唤醒 | warm-tiger P1-D + 2026-06-16 steward gap | `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` RC5 |
| recovery 拒收 11 条 0 drift 升级 | noble-peacock + perky-maple | `2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` §3.3 |
| `default_publishes` 预算双重计算 | 2026-06-16 U1 修复 follow-up | `2026-06-16-isolated-wave-stability-and-progress-steward.md` RC1 |
| engine gate 与 d623c09 双轨拒同一事件 | 2026-06-20-001 plan Phase 1 注释明确 | `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` §Phase 1 |
| state_projection 已加 lint 但未实际应用 | 2026-06-17-003 plan | `docs/achieved/plan/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md` |

### 3.3 当前未闭环清单(2026-06-21)

| 编号 | 问题 | 关联度 | 最新进展 |
|---|---|---|---|
| 21.1 | hat_handoff 0 触发(B1-B4) | 高 | Phase 2 Precheck-as-Linter 待 11 BDD scenarios 验收 |
| 21.2 | state_projection mark_step_completed | 高 | Phase 1 已落地(`4b59334`),Phase 2 待应用 |
| 21.3 | ce-executor-isolated/wave deprecation | 中 | `3fadd99` 标记 deprecated,后续会删除 |
| 21.4 | agent_recovery_mechanism_gaps 边角 | 中 | R-D2 BDD `progress_task_mismatch` 待绿 |
| 21.5 | hat-channel 路由 serial preset 失效 | 低 | P3 不阻塞 release |
| 21.6 | hat_lifecycle::complete 时序 bug | 低 | P3 不阻塞 release |
| 21.7 | loops.json stale 清理 | 低 | P3 不阻塞 release |
| 21.8 | supervisor wave protocol 6 件套 | 高 | 母舰需求文档已写,无对应 plan |
| §9 | wave worker 共享状态抽象错误 | 高 | 需 Supervisor 协议升级 |

---

## 4. 证据清单(主仓源码 + 实际 run)

> 所有引用基于实际 run `primary-20260621-015519` 的硬证据。主仓本地领先 `origin/pittcat-dev` 5 commit,工作区 12 M + 1 新增。

### 4.1 实际 run 产物证据

| 文件 | 行 | 证据 |
|---|---|---|
| `ralph-e2e/.ralph/events-20260621-015519.jsonl` | L1 | loop-bootstrap work.start 01:55:19 |
| `ralph-e2e/.ralph/events-20260621-015519.jsonl` | L2 | coordinator work.ready 01:59:54(绕过 CLI gate 落盘) |
| `ralph-e2e/.ralph/events-20260621-015519.jsonl` | L3 | ralph work.ready 02:04:04(ralph 越权) |
| `ralph-e2e/.ralph/events-20260621-015519.jsonl` | L4 | ralph task.resume 02:06:52(stall_no_events) |
| `ralph-e2e/.ralph/events-20260621-015519.jsonl` | L5 | ralph loop.cancel 02:07:48(executor_unresponsive) |
| `ralph-e2e/.ralph/recovery.jsonl` | L1-10 | 8 条 `hat_handoff_*` + 1 missing plan_name + 1 结构错 |
| `ralph-e2e/.ralph/recovery.jsonl` | L11 | ralph task.resume missing target_hat(02:06:48) |
| `ralph-e2e/.ralph/loop-termination-reason.json` | — | `"cancelled"` |
| `ralph-e2e/.ralph/loops.json` | — | `{"loops": []}`(与 current-loop-id 不同步) |
| `ralph-e2e/.ralph/current-loop-id` | — | `primary-20260621-015519` |
| `ralph-e2e/.ralph/diagnostics/2026-06-21T09-55-19/trace.jsonl` | L13, L23-28 | `default_publishes would exceed budget` WARN;`Built in ralph hat may only publish control topics` WARN(ralph 越权落盘后) |
| `ralph-e2e/.agents/scratchpad/` | L13, L34-43, L45-52 | agent 反思:iter=0→1 重命名 1-2→1-1;executor 不在 active-activations |

### 4.2 主仓源码证据(对照未提交改动)

#### 4.2.1 事件循环与编排

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `event_loop/mod.rs:5982` | **P0-3** `ProtocolView::from_event_loop` 未传 `HandoffIndex`,SSOT 契约违规 | P0 |
| `event_loop/mod.rs:5912` | 同问题(用于 `build_lint_mirror_block`,可能不读 `macro_edges_resolved` 待验证) | P0 |
| `event_loop/mod.rs:5976-6080` | **P1-2** engine gate 与 d623c09 路径双拒同一事件,共享计数器双重递增 | P1 |
| `event_loop/mod.rs:6019` | **P1-3** `engine_rejected` reason 无结构化 reason_code,observability 不对齐 | P1 |
| `event_loop/mod.rs:6038-6060` | **P1-1** circuit-breaker 注释/实现语义漂移(batch>1 时不等价) | P1 |
| `event_loop/mod.rs:6074` | **P0-6** engine gate 仍走 `LintResumeHint::from_reason` 字符串匹配,typed 路由漏改 | P0 |
| `event_loop/mod.rs:4545-4591` | handoff 注入位置在 wave context 之下(soft hint 被压)— B2 同源 | P0 |
| `event_loop/mod.rs:3520` | 缺 handoff path stall 检测器 | P1 |
| `event_loop/mod.rs:7901-8040` | recovery 累计拒收未升级 drift_finding | P2 |
| `event_loop/mod.rs:6843-6900` | per-turn 业务事件预算双重计算 | P1 |
| `event_loop/mod.rs:4562-4577` | handoff 块被 wave context 覆盖 | P0 |
| `event_loop/mod.rs:165, 380` | `TerminationReason::Cancelled` 映射路径 | — |
| `event_loop/mod.rs:8952-8962` | `loop.cancel` graceful termination | — |
| `event_loop/mod.rs:3678-3789` | missing-event hard gate / `enrich_task_resume_payload` 注入 | — |
| `event_loop/mod.rs:6500-6552` | handoff_tracker expired(30s 调度超时 → `task.resume`) | — |
| `event_loop/loop_state.rs:21` | `U2_REJECTION_RETRY_LIMIT = 3` | — |
| `event_loop/loop_state.rs:23-38` | `LINT_CIRCUIT_BREAKER_LIMIT = 2` 连续 engine gate 拒收 → 自动 disable | — |
| `event_loop/loop_state.rs:196-197, 244-248, 252-265` | stale-loop 计数 / completion 拒收指纹 / stall detector | — |
| `event_loop/loop_state.rs:412-428` | missing-event gate grace(`hat_activation_at` + 540s) | — |
| `event_loop/loop_state.rs:485-487` | hat_handoff_seq 0-init(iteration 边界重置) | — |
| `event_loop/loop_state.rs:789-815` | per-rejection-key 独立计数 | — |
| `event_loop/loop_state.rs:851-865` | work_done dedup + step 边界 prune | — |

#### 4.2.2 preset engine

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `preset/engine/protocol.rs:215-235` | **P0-4** `is_macro_edge` `let _ = from` 显式忽略 from_hat,自环排除不存在 | P0 |
| `preset/engine/protocol.rs:245-265` | **P2-4** KTD-6 conflict 信号被 `is_macro_edge` 丢弃 | P2 |
| `preset/engine/protocol.rs:331-342` | **P2-6** SHA-256 截 8 字节 16 hex chars,信号弱 | P2 |
| `preset/engine/protocol.rs:451-477` | **P1-5** 测试用 `from_hat=""` 规避 P0-4 自环检查 | P1 |
| `preset/engine/protocol.rs:111-114, 125-167` | `from_event_loop` vs `from_event_loop_with_index` 区分 | — |
| `preset/engine/protocol.rs:192-230` | `is_macro_edge(topic, from_hat)` 签名 | — |
| `preset/engine/protocol.rs:262-296` | `effective_required_fields` = schema.required ∪ rule.require_payload_fields | — |
| `preset/engine/protocol.rs:302-348` | `protocol_hash` SHA-256 | — |
| `preset/engine/linter.rs:55-62` | **P1-8** `LintPaths::under_handoff_dir` 未 canonicalize | P1 |
| `preset/engine/linter.rs:112-190` | `lint_emit` 单入口(macro-edge auto_prepare + run_gates) | — |
| `preset/engine/linter.rs:144-174` | macro-edge auto_prepare 路径 | — |
| `preset/engine/linter.rs:196` | 切到 typed 路径(linter) | — |
| `preset/engine/linter.rs:237-254` | **P0-7** `auto_handoff_prepare` 路径在 `parent()==Some("")` 边界静默失败 | P0 |
| `preset/engine/linter.rs:241-252` | `strip_prefix` 落空 fallback 绝对路径 | P1 |
| `preset/engine/linter.rs:243-268` | `auto_handoff_prepare` 写 artifact | — |
| `preset/engine/linter.rs:271` | `LINT_BUDGET = 200ms` | — |
| `preset/engine/linter.rs:312-421` | `lint_emit_with_timeout` 200ms 预算 | — |
| `preset/engine/gates.rs:60-81` | `run_gates` 单入口(lint + runtime 共用) | — |
| `preset/engine/gates.rs:86-92` | 非 object payload 全部视为缺字段(fail-closed) | — |

#### 4.2.3 preset_lint

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `preset_lint/mod.rs:278-395` | `run_preset_lint` 调用顺序 | — |
| `preset_lint/mod.rs:353-365` | **P1-4** state_projection 顺序放在 WAC 之后 schema_parity 之前,顺序敏感 | P1 |
| `preset_lint/finding_id.rs:15-147` | 16 个稳定 finding ID | — |
| `preset_lint/finding_id.rs:133-147` | `FINDING_WORK_DONE_ACTION_CHAIN_ORDER`(KTD-3,always Error) | — |
| `preset_lint/state_projection.rs:33-57` | `check_work_done_action_chain_order`(新增,KTD-3 SSOT 形式) | — |
| `preset_lint/state_projection.rs:12-15` | lint 是「primary」,engine 是「secondary」 | — |
| `preset_lint/state_projection.rs:69-77` | `action_hint` 引用 `presets/schemas/ce-executor-serial.yml` SSOT | — |

#### 4.2.4 emit CLI / policy

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `commands/emit.rs:700-713` | **P0-5** ralph 业务 topic 拒收只 record WARN 不 bail,事件仍落盘 | P0 |
| `commands/emit.rs:730-1100+` | 8 步 emit 流程(policy→event→provenance→scope→wave→gate→gate→lint) | — |
| `commands/emit.rs:746-771` | policy.check_topic_deny_rules | — |
| `commands/emit.rs:774-804` | policy.validate_event_with_hat | — |
| `commands/emit.rs:820-839` | check_emit_provenance | — |
| `commands/emit.rs:855-871` | check_isolated_scope | — |
| `commands/emit.rs:878-892` | check_wave_dimension_assignment | — |
| `commands/emit.rs:901-936` | check_step_handoff_gate | — |
| `commands/emit.rs:957-982` | check_hat_handoff_gate(7 步) | — |
| `commands/emit.rs:1027+` | lint(`ProtocolView` → `lint_emit_with_timeout`) | — |
| `policy_check.rs:115-117` | `check_isolated_scope` hat=None 时 no-op | — |
| `policy_check.rs:735` | CLI 拒收写 recovery.jsonl 的 reason_code 字段 | — |
| `policy_check.rs:920-925` | RALPH_CONTROL_TOPICS 豁免 | — |
| `event_origin.rs:32-45` | `RALPH_CONTROL_TOPICS` 7 个(LOOP_COMPLETE, loop.cancel, loop.start, human.*, task.resume) | — |
| `event_origin.rs:351` | `is_ralph_control` 检查后 `!is_ralph_control && hat_id=="ralph"` 未升级为 Reject | P0 |

#### 4.2.5 hat_handoff

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `hat_handoff/gate.rs:26-33` | 7 类 reason_code(本 run 命中 4 类) | — |
| `hat_handoff/gate.rs:116-255` | 7 步 gate | — |
| `hat_handoff/mod.rs:43-47` | 默认豁免 `review.dimension.{ready,done,failed}` 3 个微观边 | — |
| `hat_handoff/inject.rs:20-48` | `format_block` 不验证 5 段式,缺 **动作** / **阻塞** 标记 | P1 |
| `hat_handoff/macro_edges.rs:60-63` | 真正的自环排除实现(对照 P0-4) | — |

#### 4.2.6 preset 配置

| 文件:行 | 问题 | 严重度 |
|---|---|---|
| `presets/en/ce-executor-serial.yml:393-2189` | 10 hat 拓扑 | — |
| `presets/en/ce-executor-serial.yml:393-541` | coordinator | — |
| `presets/en/ce-executor-serial.yml:543-770` | executor(`triggers: ["work.ready", "fix.plan.ready"]`) | — |
| `presets/en/ce-executor-serial.yml:771-1104` | review-coordinator | — |
| `presets/en/ce-executor-serial.yml:846-854` | 4-dim 固定序列 | — |
| `presets/en/ce-executor-serial.yml:1105-1341` | dimension-reviewer(`triggers: ["review.dimension.ready"]`,缺 `task.resume`) | P1 |
| `presets/en/ce-executor-serial.yml:1342-1525` | review-synthesizer | — |
| `presets/en/ce-executor-serial.yml:1526-1615` | fixer(`triggers: ["review.failed"]`,缺 `task.resume`) | P1 |
| `presets/en/ce-executor-serial.yml:1616-1705` | debug-resolver | — |
| `presets/en/ce-executor-serial.yml:1706-1853` | plan-gate | — |
| `presets/en/ce-executor-serial.yml:1854-1954` | shipper | — |
| `presets/en/ce-executor-serial.yml:1955-2122` | reporter | — |
| `presets/en/ce-executor-serial.yml:2137-2189` | progress-steward(`triggers: ["loop.stalled"]`,缺 `task.resume` / `human.guidance`) | P1 |
| `presets/en/ce-executor-serial.yml:48-50` | topic_format_whitelist | — |
| `presets/en/ce-executor-serial.yml:68-374` | hat_handoff / gates | — |
| `presets/en/ce-executor-serial.yml:91-94` | 注释说明已迁到 SSOT,`build.rs` deep-merge | — |
| `presets/en/ce-executor-serial.yml:105-112` | state_projection.actions_chain 注释(已删 inline) | — |
| `presets/en/ce-executor-serial.yml:122-125` | verdict_gate 注释(已删 inline) | — |
| `presets/en/ce-executor-serial.yml:150-211` | execution_contracts + event_policy 配置 | — |
| `presets/en/ce-executor-serial.yml:217` | `require_emit_provenance: true` | — |
| `presets/en/ce-executor-serial.yml:224-257` | 34 条 topic_deny_rules | — |
| `presets/en/ce-executor-serial.yml:260, 268` | `plan_name_equality_required: true` + `trivial_step_max_changed_lines: 50` | — |
| `presets/en/ce-executor-serial.yml:276-374` | EventPolicy schema 表 | — |
| `presets/en/ce-executor-serial.yml:243` | topic_deny_rule:`{hat_id: review-coordinator, topic: review.passed}` | — |
| `presets/en/ce-executor-serial.yml:249-257` | `ralph` pseudo-hat 全工作流 topic deny | — |
| `presets/en/ce-executor-serial.yml:317-322` | `hat_allowed_values.skip_reason` | — |
| `presets/en/ce-executor-serial.yml:941-943` | `empty_diff` fast path 走 `review.dimensions.complete` | — |
| `config/workflow_contract.rs:63-68` | `HANDOFF_TOPIC_SEEDS` 只配 4 条,漏 review.*/fix.* 13+ 条 | P0 |
| `crates/ralph-core/data/ralph-tools*.md` | **P2-1** 未提 LINT_CIRCUIT_BREAKER_LIMIT / consecutive_engine_gate_rejections / lint_circuit_breaker_tripped | P2 |
| `crates/ralph-core/data/ralph-tools-emit.md:40` | **P2-2** `is_macro_edge(from_hat=None)` 自环排除未实现未标注 | P2 |

#### 4.2.7 summary / persistence

| 文件:行 | 说明 |
|---|---|
| `summary_writer.rs:144-165` | `write_with_landing` |
| `summary_writer.rs:174-200` | `append_recovery_section` |
| `summary_writer.rs:216-247` | `append_diagnosis_hint` |
| `summary_writer.rs:366-395` | TerminationReason → status 映射(Cancelled → "Cancelled gracefully (human rejection or timeout)") |
| `summary_writer.rs:419-455` | Event count by topic |
| `agent_doc_sync/persist.rs:103-180` | `append_recovery_envelope`(CLI 路径写 recovery.jsonl) |

### 4.3 未提交改动影响排序

按影响程度排序(均 `git status` 标记 `M`,最新未提交):

| 排序 | 文件 | 影响 |
|---|---|---|
| 1 | `event_loop/mod.rs` | 主循环逻辑;stall detector / task.resume 注入 / completion/cancel / handoff_tracker expired / P0-C verdict fail auto-terminate / bootstrap gate — 任一处改动都可能改 work.ready/work.done/loop.cancel 拒收行为 |
| 2 | `event_loop/loop_state.rs` | LoopState 字段是所有 gate 的事实来源;新增字段(pending_lint_resume、consecutive_engine_gate_rejections、lint_circuit_breaker_tripped)直接影响 lint/cancel 拒收路径 |
| 3 | `preset/engine/linter.rs` | lint_emit / auto_handoff_prepare / lint_emit_with_timeout 是 CLI 边界 + Runtime gate 共用代码;R22 macro-edge auto_prepare 是 work.ready/work.done 路径核心 |
| 4 | `preset/engine/protocol.rs` | ProtocolView 单源决定 effective_required_fields;KTD-10 SSOT。任何 drift 会导致 lint/runtime 不一致 |
| 5 | `preset_lint/finding_id.rs` | FINDING_WORK_DONE_ACTION_CHAIN_ORDER(KTD-3)是 ce-executor-serial 唯一强约束 |
| 6 | `commands/emit.rs` | CLI 拒收路径;每条 gate 都在此文件落地 — 拒收日志(recovery.jsonl)由 record_cli_emit_rejection 写入 |
| 7 | `preset_lint/mod.rs` | lint 编排(顺序敏感 P1-4) |
| 7 | `preset_lint/state_projection.rs`(新增) | KTD-3 work.done action chain order 校验,新模块 |
| 7 | `summary_writer.rs` | 终止后状态映射 |
| 7 | `preset/engine/gates.rs` | KTD-10 统一门 |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据(文件:行号) | 历史关联 |
|---|---|---|---|---|
| **P0-1** | executor 永远不被激活:coordinator 写 `0-1-...md` iter 不匹配,runtime 期望 1-based;ralph fallback 重发 `work.ready` 但 hat=ralph 仍落盘 | 多因素叠加:① 命名约定不清;② ralph 业务 topic WARN 不阻断;③ EventBus 路由未触达 | recovery.jsonl L2-7;events.jsonl L2-4;trace L27 WARN;ce-executor-serial.yml:543-548;commands/emit.rs:700-713 | **是** — primary-20260619 + warm-tiger + merry-lotus + `2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` B1-B4 同根 |
| **P0-2** | handoff 0 有效触发:10 次 retry 全是 `hat_handoff_*` reason_code,说明 agent 把 handoff 当"修文件名小事"而非宏观边契约 | 基座机制 + preset 叠加:① SEEDS 4 条硬编码漏 13+ 边;② 注入位置在 wave context 之下;③ 5 段式软约束 | recovery.jsonl L2-10;scratchpad L13, L34-43;workflow_contract.rs:63-68;event_loop/mod.rs:4562-4577;hat_handoff/inject.rs:35-48 | **是** — `2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` 4 bug 叠加;`2026-06-19-warm-tiger` 同坑 |
| **P0-3** | `engine_required_field_filter` 构造的 `ProtocolView` 不带 `HandoffIndex`:SSOT 契约违规,未来扩展立即重现 0-trigger bug | 编排问题(机制层契约) | event_loop/mod.rs:5982;protocol.rs:111-114, 125-167 | **是** — KTD-10 SSOT 约束,与 P0-2 同源 |
| **P0-4** | `is_macro_edge` 自环排除根本未实现:`let _ = from` 显式忽略 from_hat,永远不会因 `from_hat == consumer` 返回 false | bug(机制层 + 测试脱节) | preset/engine/protocol.rs:215-235;protocol.rs:451-477(测试用 `from_hat=""` 规避);macro_edges.rs:60-63(对照正确实现) | **否** — 全新发现,但与 P0-2 直接相关 |
| **P0-5** | ralph 越权发业务事件仍落盘:rejection 已被 record 但未阻断写入,`loop.cancel` 同模式 | 基座机制 | trace.jsonl L27 WARN;events.jsonl L3, L4(ralph work.ready / task.resume 落盘);commands/emit.rs:700-713;event_origin.rs:351 | **是** — warm-tiger + merry-lotus 同模式 |
| **P0-6** | `engine gate` 仍走 `LintResumeHint::from_reason` 字符串匹配,跳过 typed 分类,reason 含「artifact」/「## next marker」会路由错 | 机制问题(P1-1 typed hint 仅实施一半) | event_loop/mod.rs:6074;hint.rs:112-129, 135-144;linter.rs:196 | **否** — 新发现 |
| **P0-7** | `auto_handoff_prepare` 在 `parent()==Some("")` 边界静默失败:workspace_root 冷启动场景下 `create_dir_all` 不创建根 | bug | preset/engine/linter.rs:237-254 | **否** — 新发现 |
| **P1-1** | circuit-breaker 注释/实现语义漂移:batch>1 时「iteration 全拒」与「batch 全拒」不等价 | 机制问题(注释) | event_loop/mod.rs:6038-6060 | **否** — 新发现 |
| **P1-2** | engine gate 与 d623c09 路径双拒同一事件:`consecutive_malformed_events` 共享计数器双重递增,人为放大终止概率 | 机制问题(计数语义) | event_loop/mod.rs:5976-6080, 6038-6060 | **是** — Phase 1 注释明说"两者都跑" |
| **P1-3** | engine gate 拒绝无结构化 reason_code:`recovery.jsonl` 永远不会有 engine gate 拒收记录,observability 不对齐 CLI | bug(observability) | event_loop/mod.rs:6019;policy_check.rs:735 | **是** — U1 闭环但仅覆盖 CLI 路径 |
| **P1-4** | `preset_lint::run_preset_lint` 把 state_projection 顺序放在 WAC 之后,产出顺序与发现顺序不一致,报告行号错位 | 编排问题 | preset_lint/mod.rs:353-365 | **否** — 新发现 |
| **P1-5** | `engine_and_runtime_agree_on_macro_set_for_isolated` 测试用 `from_hat=""` 规避 P0-4,直接掩盖 bug | 测试盲点 | preset/engine/protocol.rs:451-477 | **否** — 新发现 |
| **P1-6** | progress-steward 完全未激活:triggers 缺 `task.resume`/`human.guidance`,stall 阈值未达(ralph fallback 制造假活跃) | 多因素叠加 | events.jsonl 5 条;ce-executor-serial.yml:2137-2148;scratchpad L45-52 | **是** — `2026-06-16-isolated-wave-stability-and-progress-steward.md` RC5;warm-tiger P1-D |
| **P1-7** | per-turn 业务事件预算污染 recovery:`default_publishes would exceed budget` 双重计算(已发 + 默认) | 基座机制 | trace.jsonl L13 02:00:02 WARN;event_loop/mod.rs:6843-6900 | **是** — `2026-06-16-isolated-wave-stability-and-progress-steward.md` RC1 |
| **P1-8** | `LintPaths::under_handoff_dir` 未 canonicalize,strip_prefix 落空,fallback 绝对路径触发 `hat_handoff_filename_mismatch` | bug(边界) | preset/engine/linter.rs:55-62, 241-252 | **是** — recovery.jsonl L2-6 频繁命中 |
| **P2-1** | `ralph-tools*.md` 文档未提 LINT_CIRCUIT_BREAKER 等新常量 | 文档同步 | `grep -rn "LINT_CIRCUIT_BREAKER" data/` 返回空 | **是** — CLAUDE.md 反向验证规则违反 |
| **P2-2** | `ralph-tools-emit.md:40` 未标注 `is_macro_edge(from_hat=None)` 自环排除未实现 | 文档同步 | ralph-tools-emit.md:40 | **是** — CLAUDE.md 反向验证规则违反 |
| **P2-3** | `is_macro_edge` 注释暗示「trust caller / return true when caller didn't signal a self-loop」,但 `let _ = from` 把信号吃了 | 风格(误导性注释) | preset/engine/protocol.rs:209-235 | **否** — 注释/实现脱节 |
| **P2-4** | KTD-6 `SeedWithoutUniqueConsumer` / `DerivedNotInSeed` conflict 信号在 is_macro_edge 静默丢弃 | 机制问题 | preset/engine/protocol.rs:245-265 | **否** — 与 P0-4 同源 |
| **P2-5** | agent 用 `mv` 而非 `prepare --force` 改 handoff 文件名,artifact 内容是手复制而非 prepare 生成 | agent 执行产物 | scratchpad L34-43;handoff_cli.rs:35-100 | **否** — 全新现象 |
| **P2-6** | SHA-256 截 8 字节 16 hex chars,信号弱 | 风格 | preset/engine/protocol.rs:331-342 | **否** — 信号弱但可接受 |

**根因分布统计**:
- 机制问题:9 项(P0-3, P0-6, P0-7, P1-1, P1-2, P1-7, P2-4, …)
- 编排问题:5 项(P0-3, P1-4, …)
- bug:7 项(P0-4, P0-5, P0-7, P1-3, P1-5, P1-8, P2-3)
- 多因素叠加:2 项(P0-1, P1-6)
- 文档/风格:5 项(P1-1 注释, P2-1~P2-6)
- agent 执行产物:2 项(P2-5 等)

---

## 6. 修复建议(按优先级)

> 每条都给出目标文件、具体修改、预期效果、验证手段、回滚方案。优先复用历史已成功模式。

### P0 — 阻断性(1-3 天)

**修复 1**:扩 `HANDOFF_TOPIC_SEEDS` 到 17 条全覆盖(对应 B1)
- **目标**:`crates/ralph-core/src/config/workflow_contract.rs:63-68`
- **修改**:常量数组追加 13 条(`review.dimension.{ready,done,failed}` / `review.dimensions.complete` / `review.passed` / `review.failed` / `review.complete` / `fix.applied` / `fix.exhausted` / `work.done` / `plan.complete` / `REVIEW_COMPLETE` / `report.done` / `LOOP_COMPLETE`)
- **效果**:`build_emit_instructions` 对所有 macro-edge hat 都列对应提示
- **验证**:`cargo nextest run -p ralph-core --test scenarios hat_handoff` 全绿
- **回滚**:`git revert`(常量数组不破坏 schema)
- **历史复用**:`2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` Fix 1 同款

**修复 2**:handoff 注入块提到 WAVE CONTEXT 之上(对应 B2)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:4562-4591`
- **修改**:把 `build_emit_instructions` 块从 `format!("{block}\n\n{base_prompt}")` 改为 prepend 到 wave context 之上
- **效果**:agent prompt 顶部看到 handoff 提示,与 WAVE CONTEXT 同级
- **验证**:新增 `event_loop/tests/hat_handoff_inject_order.rs` 单测,断言 handoff 块索引 < wave 块索引
- **回滚**:`git revert`,事件循环逻辑独立
- **历史复用**:`2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` Fix 2 同款

**修复 3**:`ProtocolView::from_event_loop_with_index(..., Some(&HandoffIndex::from_config(...)))`(对应 P0-3)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:5982, 5912`
- **修改**:engine gate 构造 view 时传 `HandoffIndex`,与 CLI emit 路径对齐
- **效果**:SSOT 契约一致性,未来扩展 `is_macro_edge` 时不重现 0-trigger bug
- **验证**:`cargo nextest run -p ralph-core --test preset_engine -- protocol_view_with_index` 新单测
- **回滚**:`git revert`

**修复 4**:`is_macro_edge` 真正实现自环排除(对应 P0-4)
- **目标**:`crates/ralph-core/src/preset/engine/protocol.rs:215-235`
- **修改**:删除 `let _ = from`,改为 `if from == idx.consumer_of(topic) { return false; }`
- **效果**:linter 路径对自环 topic(如 `queue.advance`)不再误报为宏观边
- **验证**:① 现有 `engine_and_runtime_agree_on_macro_set_for_isolated` 测试改 `from_hat=consumer` 真正触发自环;② 新增 `protocol_is_macro_edge_self_loop_excludes` 单测(红绿循环)
- **回滚**:`git revert`

**修复 5**:ralph 越权发业务事件 → CLI 真正 bail 而非 record(对应 P0-5)
- **目标**:`crates/ralph-cli/src/commands/emit.rs:700-713` + `crates/ralph-core/src/event_origin.rs:351`
- **修改**:① `is_ralph_control` 检查后,`!is_ralph_control && hat_id=="ralph"` 改 `Reject` 而非 WARN;② `commands/emit.rs:700-713` 加 `policy_reject_ralph_business_topic` error code;③ `loop_runner/runner.rs` 加防御:hat==ralph 且 topic 不在 `RALPH_CONTROL_TOPICS` 时不调 `bus.publish`,直接 `tracing::error!` + 写 recovery
- **效果**:ralph 越权发 work.ready / task.resume 不会再落 events.jsonl
- **验证**:`cargo nextest run -p ralph-cli --bin ralph -- emit_ralph_business_topic_bails` 新单测
- **回滚**:`git revert`,改动隔离在两个文件
- **历史复用**:merry-lotus / noble-peacock 已记录"task.resume 越权"模式

**修复 6**:`auto_handoff_prepare` 路径健壮性(对应 P0-7)
- **目标**:`crates/ralph-core/src/preset/engine/linter.rs:237-254`
- **修改**:`create_dir_all(workspace_root.join(output_dir))` 而非只看 parent,workspace_root 冷启动场景显式建出
- **效果**:`abs_path.parent() == Some("")` 边界不再静默失败
- **验证**:`cargo nextest run -p ralph-core -- linter_workspace_root_missing` 新单测
- **回滚**:`git revert`

**修复 7**:engine gate 切到 `LintResumeHint::from_typed_rejection`(对应 P0-6)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:6074`
- **修改**:engine gate 在拒绝时用 `RejectionKind` 而非字符串
- **效果**:reason 路由准确,不再因 reason 含「artifact」/「## next marker」误路由
- **验证**:`cargo nextest run -p ralph-core -- engine_gate_typed_hint` 新单测
- **回滚**:`git revert`

**修复 8**:补齐 reviewer / steward hat 的 `task.resume` trigger(对应 P1-6)
- **目标**:`presets/en/ce-executor-serial.yml:1113, 1529, 2140` + `presets/en/ce-executor-isolated.yml:2345`
- **修改**:
  ```yaml
  dimension-reviewer:
    triggers: ["review.dimension.ready", "task.resume"]
  fixer:
    triggers: ["review.failed", "task.resume"]
  progress-steward:
    triggers: ["loop.stalled", "task.resume", "human.guidance"]
  ```
- **效果**:ralph `task.resume(target=dimension-reviewer)` 能真正重新激活该 hat;progress-steward 对 human.guidance 响应
- **验证**:① `cargo nextest run -p ralph-core --test scenarios reviewer_task_resume_activate` 新单测;② 跑 warm-tiger 同 plan,30s 内 review.dimension.done
- **回滚**:`git revert`,改动只 preset YAML
- **兼容标注**:`task.resume` reserved-by-default 提升,新行为更宽容,**不破坏**既有 hat 拓扑
- **历史复用**:`2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` 建议 2 同款

**修复 9**:loop runner 增加 handoff path stall 检测器(对应 P0-1 子问题 ③)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:3520`
- **修改**:新增 `check_handoff_path_stall` helper,多次 macro edge emit 但 0 个 handoff artifact 时主动 `human.guidance`
- **效果**:agent 完全没意识到 handoff 也不会让 loop 静默卡死
- **验证**:`cargo nextest run -p ralph-core -- handoff_stall_detector` 新单测
- **回滚**:`git revert`,helper 独立
- **历史复用**:`2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` 建议 3 同款

### P1 — 重要偏离(1 周)

**修复 10**:`format_block` 加 5 段式结构化校验(对应 B3)
- **目标**:`crates/ralph-core/src/hat_handoff/inject.rs:20-48`
- **修改**:`format_block` 内调 `validate_five_section` + `validate_next_block_markers`(5 段 + **动作**/**阻塞** 标记)
- **效果**:不规范的 handoff artifact 不再注入下游 hat prompt
- **验证**:`cargo nextest run -p ralph-core -- hat_handoff_inject_5section_validation` 新单测
- **回滚**:`git revert`
- **历史复用**:`2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` Fix 3 同款

**修复 11**:stall detector 改进 — 把"业务事件"定义从"任何非 loop event"改为"非 ralph fallback 业务事件"
- **目标**:`crates/ralph-core/src/event_loop/mod.rs` + `loop_state.rs`
- **修改**:新增 `ralph_fallback_emit_count` 字段,N 次后强制唤 progress-steward
- **效果**:ralph 越权发 work.ready / task.resume 制造的"假活跃"不再欺骗 stall detector
- **验证**:`cargo nextest run -p ralph-core --test progress_steward -- ralph_fallback_stall_detection` 新单测
- **回滚**:`git revert`
- **历史复用**:`docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` RC5 基础

**修复 12**:`default_publishes` 预算计算排除"已发事件"避免双重计算(对应 P1-7)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:6843-6900`
- **修改**:`default_publishes` budget 在 hat 已发业务事件后不累加
- **效果**:coordinator 不会被双重算入预算,work.failed 等兜底信号不丢
- **验证**:`cargo nextest run -p ralph-core --test isolated_wave_budget -- default_publishes_dedup` 新单测
- **回滚**:`git revert`
- **历史复用**:`docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` RC1 U1 拆 slot 后的 follow-up

**修复 13**:`engine gate` 加结构化 reason_code 写 recovery.jsonl
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:6019`
- **修改**:`MalformedLine` 加 reason_code 字段,engine gate 调 `append_recovery_envelope` 写 recovery
- **效果**:`ralph diagnose` 能看到 engine gate 拒收
- **验证**:`cargo nextest run -p ralph-core -- engine_gate_writes_recovery` 新单测
- **回滚**:`git revert`

**修复 14**:`LintPaths::under_handoff_dir` canonicalize workspace_root
- **目标**:`crates/ralph-core/src/preset/engine/linter.rs:55-62, 241-252`
- **修改**:内部 `workspace_root.canonicalize()` 后再 join
- **效果**:`./` / `/tmp/./` 边界不再触发 fallback 绝对路径
- **验证**:`cargo nextest run -p ralph-core -- linter_paths_canonicalize` 新单测
- **回滚**:`git revert`

**修复 15**:`is_macro_edge` 测试改 `from_hat=consumer` 真正触发自环(对应 P1-5)
- **目标**:`crates/ralph-core/src/preset/engine/protocol.rs:451-477`
- **修改**:测试参数用真 `from_hat` 值
- **效果**:P0-4 不被测试盲点掩盖
- **验证**:`cargo nextest run -p ralph-core -- protocol_macro_edge_self_loop` 通过 + 红绿循环
- **回滚**:`git revert`

**修复 16**:`preset_lint::run_preset_lint` 顺序调整 — state_projection 移到 schema_parity 之前(对应 P1-4)
- **目标**:`crates/ralph-core/src/preset_lint/mod.rs:353-365`
- **修改**:state_projection order 校验移到 schema_parity 之前
- **效果**:报告行号一致
- **验证**:`cargo nextest run -p ralph-core -- preset_lint_order` 新单测
- **回滚**:`git revert`

### P2 — 可观测性 / 文档(1-2 周)

**修复 17**:recovery.jsonl 累计拒收升级为 drift_finding
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:7901-8040`
- **修改**:5 分钟内同 hat+reason_code ≥ 3 次 → `drift_finding(severity=Warning)`
- **效果**:14 条 recovery 0 drift 不对称修复
- **验证**:`cargo nextest run -p ralph-core -- recovery_escalation_to_drift` 新单测
- **回滚**:`git revert`

**修复 18**:agent 写 handoff 文件走 `prepare --force` 而非 `mv`
- **目标**:`presets/en/ce-executor-serial.yml` `coordinator.instructions` 段(第 393-540 行)
- **修改**:HARD RULE 列表加 "不要用 mv,改用 ralph tools handoff prepare --force"
- **效果**:agent 不再手写文件名
- **验证**:跑同 plan 触发 `hat_handoff_filename_mismatch`,看 scratchpad 记录
- **回滚**:`git revert` 单 YAML

**修复 19**:hat_handoff 注入可观测性补齐
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:4545-4591` + `hat_handoff/inject.rs`
- **修改**:`LoopState` 加 `last_injected_hat_handoff_block_hash`,`prepend_hat_handoff_from_pending` 注入成功/失败记 hash + tracing
- **效果**:`ralph diagnose` 新增"hat_handoff 注入历史"段
- **验证**:`cargo nextest run -p ralph-core -- hat_handoff_inject_observability` 新单测
- **回滚**:`git revert`

**修复 20**:`ralph-tools*.md` 同步新常量 + `is_macro_edge` 自环排除语义
- **目标**:`crates/ralph-core/data/ralph-tools*.md`
- **修改**:补 LINT_CIRCUIT_BREAKER_LIMIT、consecutive_engine_gate_rejections、lint_circuit_breaker_tripped、`is_macro_edge(from_hat)` 语义
- **效果**:符合 CLAUDE.md 反向验证规则
- **验证**:`grep -rn "LINT_CIRCUIT_BREAKER" data/` 命中;`grep -rn "is_macro_edge" data/ralph-tools-emit.md` 标注
- **回滚**:`git revert`

**修复 21**:harness 提示用 `ralph tools handoff prepare --iter <N> --seq <M>` 显式参数
- **目标**:`crates/ralph-cli/src/handoff_cli.rs:35-100` + `presets/en/ce-executor-serial.yml` `coordinator.instructions`
- **修改**:① `execute_prepare` 新增 `--iter` / `--seq` 显式参数;② 默认 1-based;若用户传 `--iter 0` 警告;③ preset 顶部加 HARD RULE
- **效果**:agent 写 `0-1-...md` 立即收到 CLI 警告
- **验证**:`cargo nextest run -p ralph-cli -- handoff_prepare_iter_validation` 新单测
- **回滚**:`git revert`

---

## 7. 执行顺序与验证

```
P0 治本(1-3 天):1(SEEDS) → 2(注入位置) → 10(5 段式)(B1+B2+B3 完整链) → 5(ralph bail) → 3(SSOT 契约) → 4(自环排除) → 6(workspace) → 7(typed hint) → 8(task.resume trigger) → 9(stall 兜底)
P1 治标(1 周):15(测试补) → 11(stall 假活跃) → 12(default_publishes) → 13(observability) → 14(路径规范化) → 16(顺序)
P2 可观测性(1-2 周):17(drift 升级) → 18(agent 行为) → 19(注入观测) → 20(文档) → 21(显式 iter)
```

**最关键的"无修复不回归"测试**:在修复 1+2+10+8+5 落地后,跑 `ralph-e2e/.ralph/agent/hat-handoff/` 实际工作流,验证:
- V1: 至少 1 个 handoff artifact 落盘(本 run = 0)
- V2: `recovery.jsonl` 中 `hat_handoff_*` 命中数 ≥ 0(代表 gate 真生效)
- V3: executor 真正被 spawn(`active-activations.json` 包含 executor 记录)
- V4: 同 plan 跑出 work.done / fix.applied 等后续事件(本 run = 0 业务事件闭环)

**全量基线验证**(修复 1-14 落地后):
```bash
./scripts/run-tests.sh         # 完整 nextest + doctest
# 如果出现 race/timing flake:
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh   # 兜底
```

**CLAUDE.md 强制项**:
- HARD RULE 1:`cargo nextest run` 系列(本项目测试入口)
- HARD RULE 2:默认走并发,`ralph-cli` 整个包走 cli-serial 串行
- 反向验证:任何 ralph tools 子命令 / 行号引用 / 参数表改完必须用 `sed -n 'NN,MMp' <file>` 复核行号漂移,跑 `ralph <cmd> --help` 冒烟

---

## 8. 报告交叉验证

- **Agent A ↔ C**:链路图(期望 vs 实际)与偏离证据清单完全对齐 — 都指出 executor 未激活是因 hat_handoff 0 触发
- **Agent B ↔ D**:历史知识库与归因表完全对齐 — hat_handoff 0 触发(§21.1)、ralph 越权(§4)、recovery 升级 drift 缺失(§10-2)三处均交叉验证
- **Agent C ↔ D**:偏离证据 21 项中 7 项 P0、8 项 P1、6 项 P2 全部被 D 归因到 preset / 基座机制 / agent / 叠加四类,无遗漏
- **本报告范围声明**:由 4 个 sub agent 并行生成,主 agent 只做汇总和格式整理,所有原始证据已带文件路径:行号引用

---

## 9. 关键文件索引

### 9.1 实际 run 产物
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260621-015519.jsonl`(5 条)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-history-20260621-015519.jsonl`(2 条)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/history.jsonl`(2 条)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/recovery.jsonl`(12 条)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loop-termination-reason.json`(`"cancelled"`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/current-loop-id`(`primary-20260621-015519`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loops.json`(`{"loops": []}`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-21T09-55-19/trace.jsonl`
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-21T09-55-19/diagnosis-summary.json`(`recovery_count: 14, drift_finding_count: 0`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.agents/scratchpad/`

### 9.2 主仓源码(本次审计重点)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/event_loop/{mod.rs, loop_state.rs, rejection.rs}`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/event_origin.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/hat_handoff/{mod.rs, gate.rs, macro_edges.rs, inject.rs}`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/preset/engine/{mod.rs, gates.rs, linter.rs, protocol.rs}`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/preset_lint/{mod.rs, finding_id.rs, state_projection.rs}`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/summary_writer.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/config/workflow_contract.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/agent_doc_sync/persist.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-cli/src/commands/emit.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-cli/src/policy_check.rs`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml`(2189 行)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/schemas/ce-executor-serial.yml`(SSOT)

### 9.3 历史强关联文档
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md`(本 run 主因)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`(同根)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`(未闭环 Phase 2)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/brainstorms/2026-06-20-serial-preset-precheck-as-linter-requirements.md`
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md`(母舰需求)
