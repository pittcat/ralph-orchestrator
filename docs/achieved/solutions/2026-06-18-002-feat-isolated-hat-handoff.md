---
title: 2026-06-18-002 — isolated hat handoff 失效排查 runbook
date: 2026-06-18
type: solution
origin:
  - docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md
  - docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md
---

# 2026-06-18-002 — isolated hat handoff 失效排查 runbook

## 适用症状

下列任一症状命中即启用本 runbook：

- `.ralph/agent/hat-handoff/` 不存在,但 `event_loop.hat_handoff.enabled: true`
- review 序列卡在 N 维(`review.dimension.done` 仅 emit 2/4 维)
- `task.resume` 连续发到同一 hat 但该 hat 不响应
- agent 反复用相同 payload 调 `ralph emit` 但 events.jsonl 不增加
- progress.md 卡在某 step 不前进

## 一句话背景

`ce-executor-serial` 在 isolated 模式下,hat handoff 由 5 道机制串联:CLI 门 (U1) → EventBus 路由 (U2) → scope enforcement (U5) → runtime gate (U4) → agent prompt 反馈 (U6)。任一道失守会导致上游症状。

## 5 步快速排查

按以下顺序执行,90% 案例在前两步内能定位:

### Step 1: 确认 hat_handoff.enabled 与 execution_mode

```bash
grep -A 3 "hat_handoff:" ralph.yml
grep -A 1 "execution_mode:" ralph.yml
```

期望:
- `event_loop.hat_handoff.enabled: true`
- `event_loop.execution_mode: isolated`

如果 execution_mode 不是 isolated,preset 不会启用 handoff gate,看起来"handoff 不工作"实则是配置问题。

### Step 2: 检查 handoff 产物是否存在

```bash
# 直接看目录
ls -la .ralph/agent/hat-handoff/

# 用 audit 子命令(U8)
scripts/audit-hat-handoff-artifacts.sh .
```

如果目录为空:
- 检查 `hat_handoff.enabled` 是否在 ralph.yml 真的设为 true
- 检查 `presets/en/ce-executor-serial.yml` 的 `event_loop.hat_handoff` 段(2026-06-18-002 U11 启用后才默认开启)

如果文件名格式不符(`^\d+-\d+-[A-Za-z0-9_-]+-[A-Za-z0-9_-]+\.md$`),audit 退出码 3,提示文件名手写错误。回退用 `ralph tools handoff prepare` 重新生成。

### Step 3: 检查 task.resume 信号路由

```bash
# 查看最近的 task.resume 事件
jq 'select(.topic == "task.resume")' .ralph/events.jsonl | tail -5

# 检查 EventBus U2 修复:target 字段是否被正确路由
jq 'select(.topic == "human.guidance" and .target != null)' .ralph/events.jsonl
```

如果 `task.resume(target=dimension-reviewer)` 发出后该 hat 仍不响应:
- 确认 dimension-reviewer 的 instructions 含 Recovery Signals 段(2026-06-18-001 U3)
- 确认没有 reserved-trigger lint 报错(`cargo run -p ralph-cli -- preset check --strict -H builtin:ce-executor-serial`)

### Step 4: 检查 prompt 注入

```bash
# 用 ralph diagnose 看 prompt 注入历史
ralph diagnose --session latest
```

期望看到:
- `## HAT HANDOFF` 块出现在 dimension-reviewer 的 prompt 顶部(说明 U4 + handoff_path 注入正常)
- 没有 `event.hat_handoff.inject_failed` 事件(否则文件缺失)
- 看 `## RECENT REJECTIONS` 块(U6),如果同一 reason_code 出现 ≥ 3 次,说明 agent 没在读 recovery.jsonl

### Step 5: 检查 recovery.jsonl 拒收原因

```bash
# 列出所有 hat_handoff_* 拒收
jq 'select(.envelope.reason_code | startswith("hat_handoff_"))' \
   .ralph/diagnostics/latest/recovery.jsonl | tail -5

# 列出所有 isolated_scope_violation
jq 'select(.envelope.reason_code == "isolated_scope_violation")' \
   .ralph/diagnostics/latest/recovery.jsonl | tail -5
```

常见 reason_code 与修复:

| reason_code | 含义 | 修复动作 |
|-------------|------|---------|
| `hat_handoff_missing_path` | 宏观边 payload 无 handoff_path | `ralph tools handoff prepare --from <X> --to <Y> --topic <Z>` 重新生成 |
| `hat_handoff_file_not_found` | handoff_path 指向不存在的文件 | 检查 `.ralph/agent/hat-handoff/` 目录是否被清理 |
| `hat_handoff_filename_mismatch` | 文件名 from/to 与 caller 不一致 | 重新 prepare 或检查 hat_id 大小写 |
| `hat_handoff_path_escape` | handoff_path 含 `..` 试图逃逸 workspace | 路径 jail 拦截,必须用相对路径 |
| `isolated_scope_violation` | 当前 hat 不允许发该 topic | 检查 preset 的 `publishes` 字段 |
| `isolated_anonymous_business_topic` | 事件完全无 hat/source/triggered provenance | 用 `ralph emit --hat <id>` 或 agent backend 带 hat |
| `topic_format_rejected` | topic 不在白名单 | 检查 ralph.yml `event_policy.allowed_topics` |

## 决策树:handoff 反复失败

```
agent emit 失败
  ├─ recovery.jsonl 有条目? ──── 否 ──→ 检查 CLI gate (U1)
  │                                  ralph emit --policy-check 手动跑
  │
  └── 是 ──→ reason_code 是?
           ├─ hat_handoff_* ──→ 跑 handoff prepare 重做文件
           ├─ isolated_scope_* ──→ 修正 preset publishes 或 hat_id
           ├─ isolated_anonymous ──→ 补 hat/source/triggered provenance
           └─ topic_format_* ──→ 修正 topic 字符串
```

## 与 recovery responder 的关系

`recovery_responder` 会根据这些 envelope 自动升级到 `task.resume(target=<stuck-hat>, reason=<reason_code>)`,所以**重复用同一 payload 触发 task.resume 不会自我修复**——必须按本 runbook 步骤真正修复 payload 字段。

## 与 progress-steward 的关系

如果 stall detector 多次未收到 business event,会唤醒 `progress-steward` hat,它在 U7 修复后即使 `suppress_human_guidance=true` 仍能收到 `human.guidance` 内容。steward 的决策树:
1. 读 state_projection 看哪个维度卡住
2. emit `work.ready` 重启流程,或
3. emit `queue.advance` 跳过当前 step,或
4. emit `plan.blocked(reason=...)` 终止循环

如果 steward 也不响应,检查 `progress_steward.exempt_from_suppress_human_guidance: true`(默认)。

## 相关文档

- `docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md` — 完整 9 单元修复计划
- `docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md` — hat handoff 机制原始设计
- `crates/ralph-core/data/ralph-tools-handoff.md` §5.5.3 — reason_code 全表
- `docs/guide/runtime-diagnosis.md` §13 — Step Handoff 诊断