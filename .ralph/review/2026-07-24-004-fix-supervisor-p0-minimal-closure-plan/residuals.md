# Residuals — 2026-07-24-004 supervisor P0 最小化闭合

本文件记录本计划**明确不修**或**仅部分闭合**的残留。`progress-steward` / `shipper` 两个 hat 已永久删除，**不得恢复**；下列残留指 runtime 硬编码与配置合并语义，不是 hat 去留问题。

## M1 机制残留：U5 / stall fail-close 仍 `target=shipper`

- **事实**：runtime（`event_loop` stall / U5 escalation）合成 `plan.blocked` 时仍 `.with_target(HatId::new("shipper"))`。`shipper` hat 已删，EventBus 对该 target **静默丢弃**，链路不会因此走到 reporter。
- **本计划处置**：按用户约束 A / KTD2，**不改** shipper hardcode、不做 registry lookup、不恢复 hat。preset 侧用 `progress_steward.enabled: false` 钉死，降低「未注册 steward 计数 → U5」诊断主链被误开的概率。
- **闭合程度**：M1 **部分**闭合（preset 钉死）；机制根因未修。
- **后续路由**：单独计划把 fail-close target 改为 `reporter`（或 registry lookup），并补 schema 五字段与测试。

## R2 配置合并：preset 钉死无法覆盖 operator overlay

- **事实**：`progress_steward` 不在 `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS`，也不在 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`（见 `crates/ralph-cli/src/preflight.rs`）。因此：
  - operator **省略**该键 → preset 值可能被 filter 并打 warning，回落框架默认 `false`（与钉死同效，但可能有 warning 噪音）；
  - operator 在 `ralph.yml` **显式** `event_loop.progress_steward.enabled: true` → **operator 胜出**，preset 的 `false` 盖不住。
- **本计划处置**：不改 `config_resolution` / preflight allowlist（plan out of scope）。YAML 显式 `enabled: false` + 注释警告；本 residuals 诚实记录。
- **操作建议**：supervisor 拓扑下 **不要** 在 operator 配置里打开 `progress_steward`。

## 已闭合（对照，非残留）

| 诊断 | Unit | 状态 |
|------|------|------|
| M2 `plan.blocked` allowlist | U1 | 完整 |
| M3 coordinator 抢发终态 | U3 | instructions 硬约束（非机制强制） |
| M5/P1 重复 LOOP_COMPLETE 拒收噪音 | U4 | fingerprint dedup |
| M4 task ↔ event bus | — | 明确不做 |
