---
module: ralph-core/supervisor, ralph-cli/loop_runner
tags: [redrive, supervisor-store, descriptor, boot-dispatch, rusqlite]
problem_type: mechanism-fix
---

# Redrive descriptor 持久化与 resume boot 派发闭环

## 问题

Wave slot redrive 链路在同一根因链上有四层缺口，导致 `ralph wave redrive` 创建的子 wave 永远停在 `Pending`：

1. `bind_slot` branch 命名不含 `wave_id`，同 loop 跨 wave 同 slot_index 撞名；
2. `persist_slot_descriptor` 生产零调用，没有 descriptor 被持久化；
3. rusqlite store 无 descriptor 存取实现；
4. `create_redrive_wave` 不复制父 descriptor 到子 wave，runner boot 也没有 redrive 派发步骤。

## 修复要点（plan 2026-07-28-002）

- **命名空间**：branch/worktree 命名统一为 `{loop_id}-{kind}-{wave_id}-{slot_index}`；删除 prompt-prefix 模糊匹配复用路径。
- **双 store descriptor 闭环**：dispatcher 在 spawn 前 persist；`create_redrive_wave` 复制父 descriptor 到子行（`slot_index_in_parent` 锚 + `payload_digest`）；enriched `list_redrive_pending_child_waves` 携带 `expected_digest`。
- **A1 封口（follow-up 发现）**：rusqlite `persist_slot_descriptor` 最初只有 UPDATE 没有 INSERT —— 首次 persist 命中 0 行却返回 `Ok(())`，descriptor 静默丢失。修复为 UPDATE 未命中时回退 INSERT；UPDATE 路径保持 `COALESCE(slot_index_in_parent, ?)` 保护父锚。**教训：静默 0 行写入是 fail-open，store 写路径必须区分"更新"与"首次写入"。**
- **C3 多 slot 线程化**：`execute_wave_via_supervisor_with_executor` 增加 `slot_index_override`，redrive 单事件合成 wave 用真实 child slot 下标绑定，不再全部误绑 slot 0。pre-registered 派发跳过重复 persist（保住 parent 锚）。
- **G1 boot 接线**：`runner.rs` 两个 supervisor boot seam 在 `recover_active_waves_at_startup` + backend 构造之后、主循环之前调用 `dispatch_pending_redrive_waves`，**仅 `--resume` 触发**（fresh boot 不消费残留 redrive 状态）。take 是非破坏读 + digest 校验（不 DELETE）；幂等靠 `list_redrive_pending_child_waves` 只返回 Pending slot（二次扫描在 slot 离 Pending 后自然为空）。
- **Fail-closed**：`expected_digest = None`（legacy 无 descriptor）/ digest 冲突 / take 失败 → 跳过该 slot、不 spawn、warn 诊断。

## 验证锚点

- `test_s3_rusqlite_backed_wave_supervisor_dispatch`（rusqlite 真实 backed，抓住 A1 静默丢失）
- `test_u4_redrive_boot_dispatch_in_memory_multi_slot`（3 slot 绑定 0/1/2 + 二次扫描幂等 + R9 wave_total）
- `rusqlite_take_survives_reopen_crash_window`（take → reopen → 仍 Dispatchable）
- `test_u4_redrive_boot_legacy_slot_fail_closed`、`test_s2a_persist_failure_fails_closed_no_spawn`

## 残留（P1/P2，留待后续 plan）

- A2 显式 `dispatched_at`/`dispatched_by_boot_id` marker 列（当前幂等依赖 Pending 过滤；Dispatched-but-not-terminal 由 `recover_active_waves_at_startup` 接手）
- A3/A4 fail-closed 诊断升级为 `RecoveryDiagnosisEnvelope`（当前 tracing::warn）
- `list_redrive_pending_child_waves` rusqlite N+1 查询改单次 JOIN（M4）
