# SSOT 护栏：禁止过时概念与错误路径

诊断**只认当前代码库**。历史 `docs/report/` 若与本文冲突，**以本文 + 源码为准**，不得把旧报告里的机制当作本次 run 的对账项。

---

## 已删除（禁止查、禁止写、禁止当 P0 根因）

| 概念 | 说明 |
|------|------|
| **hat_handoff** | 2026-06-23-006 全量删除；无代码、无 `.ralph/agent/hat-handoff/` |
| **Handoff linter / 五段式 macro handoff 文件** | 同上，与 session `handoff.md` 无关 |
| **`review.passed` / `review.failed` 作链路期望** | 当前 review 终态为 `review.complete`（见 preset schema 注释） |
| **`human.guidance` topic** | 已移除（2026-06-28-005） |

---

## 不存在的路径（禁止假设）

| 错误 | 正确 |
|------|------|
| `loop_state_snapshot.json` | `ralph inspect loop` / `replay_events_to_snapshot`（内存回放） |
| `run_dir/events/`、`run_dir/tasks/` | `.ralph/` + `.ralph/agent/tasks.jsonl`；events 由 `current-events` 指向 |
| workspace 根 `.ralph/drift.jsonl` | `.ralph/diagnostics/<session>/drift.jsonl`（有 session 时） |
| `ralph hats show -H ... --format yaml` | 读 `presets/en/<name>.yml` 或 `ralph preset show <name> --format yaml` |

---

## 术语（仅保留以下 handoff 语义）

| 术语 | 产物/机制 |
|------|-----------|
| **Session handoff** | `.ralph/agent/handoff.md`（`handoff.rs`，终止时） |
| **step_handoff** | `step_handoff/`：tasks.jsonl ↔ progress.md 对齐门 |
| **Hat-channel** | `.ralph/agent/events-hat-{hat_id}-{loop_id}-{iteration}.jsonl` |

---

## recovery.jsonl 字段

用 `reason_code` / `source`（如 `payload_contract`、`semantic_gate_violation`），**无**顶层字段名 `semantic_gate`。

---

## `debug.md` 使用范围

- ✅ 采用：强制四问、机制 vs 编排归因思路
- ❌ 不采用：其「输入」里的 `events/`、`tasks/` 目录布局（已过时）；以本 skill `artifact-manifest.md` 为准

---

## 改动本 skill 前

新增产物/机制引用须 `rg` 主仓确认实现存在，禁止从旧 diagnosis 报告照抄。
