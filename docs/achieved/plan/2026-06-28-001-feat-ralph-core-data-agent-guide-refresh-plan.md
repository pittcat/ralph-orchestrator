---
title: ralph-core/data AI 指南刷新与场景导航
type: feat
status: active
date: 2026-06-28
origin: docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md
---

# ralph-core/data AI 指南刷新与场景导航

## Overview

刷新 `crates/ralph-core/data/*.md` 内置 skill 文档，使其与当前 CLI 源码和运行时行为对齐；在唯一自动注入入口 `ralph-tools.md` 顶部增加场景导航层，帮助 loop 内 agent 从决策点快速定位应执行的命令或应加载的 skill。计划不改动运行时代码，只更新文档与校验脚本感知范围。

---

## Problem Frame

`crates/ralph-core/data/*.md` 是 Ralph 注入给 loop 内 agent 的内置 skill 源文件。近两个月运行时能力已多次迭代（U11 StateLedger、`ralph emit --schema`、worktree 显式复用、`--no-default-profiles` / `--no-sync-agent-docs`、task/memory flag 调整、policy-check unified pipeline 等），但 data 文档存在三类问题：

1. 参数/命令与当前 `--help` 不一致，`scripts/check-cli-doc-drift.sh --strict` 对 task/memory/cmdref/emit/wave 命令报出多处漂移。
2. 新行为/概念（StateLedger、`--schema`、profile overlay 等）讲解不足或缺失。
3. AI 不易判断「什么时候该用哪个 skill」——命令按命名空间平铺，缺少以 agent 实际决策点为入口的导航。

结果导致 agent 在 loop 里要么用错命令/flag，要么在多个 skill 文件之间空转。

（完整问题框架、Actor、Key Flows、Acceptance Examples 见 origin 文档。）

---

## Requirements Trace

- R1. 审计并修正 `crates/ralph-core/data/*.md` 中所有 `*.rs:NN` 源码行号引用，确保与当前代码对齐。
- R2. 更新 `ralph-tools-tasks.md`、`ralph-tools-memories.md`、`ralph-tools-cmdref.md`、`ralph-tools-emit.md`、`ralph-tools-wave.md` 的参数表与示例，使 `check-cli-doc-drift.sh --strict` 对本次刷新覆盖的命令无新漂移。
- R3. 在 `ralph-tools.md` 的「收到 `task.resume` 时」段补充当前 rejection payload 字段解释，并明确修复顺序；继续禁止 unsafe bypass 和直写 `events.jsonl`。
- R4. 在 `ralph-tools.md` 或 `ralph-tools-emit.md` 中加入 `ralph emit --schema <TOPIC>` 的用途说明。
- R5. 在 `ralph-tools.md` 中加入 U11 StateLedger / `ralph loops clean --ledger` 的简要说明。
- R6. 在 `ralph-tools.md` 中加入 `ralph run` 新增 flag（`--no-default-profiles`、`--no-sync-agent-docs`）与 `ralph inspect profiles` 的说明。
- R7. 在 `ralph-tools.md` 顶部新增「AI 决策速查 / 何时看哪份 skill」场景导航小节（25–40 行）。
- R8. 保留现有命令速查表，但将其后置或并入场景导航。
- R9. 为每个主要场景补充一条完整命令示例。
- R10. 在 `ralph-tools.md` 通用错误恢复表中新增/刷新当前常见错误。
- R11. 若新增 `.md` 文件，必须按 `skill_registry.rs` 的 built-in skill 模式注册。
- R12. 刷新后执行：nextest skill registry 测试、`check-cli-doc-drift.sh --strict`、`guard-prompt-size.sh`。

**Origin actors:** A1 — Loop 内 agent；A2 — 维护者。  
**Origin flows:** F1 `task.resume` 修复；F2 emit vs wave 选择；F3 task/memory/decision 管理；F4 崩溃/恢复/诊断。  
**Origin acceptance examples:** AE1–AE5。

---

## Scope Boundaries

- **Inside:** `crates/ralph-core/data/*.md` 内容更新；`scripts/check-cli-doc-drift.sh` 中 KNOWN_DRIFTS / COMMANDS_TO_DOCS 映射如有必要同步调整；`scripts/guard-prompt-size.sh` 阈值不变。
- **Outside：** 不修改 CLI 行为或运行时注入策略；不把完整 JSON Schema / preset YAML 复制进 data；不为 IDE/Claude Code 单独维护另一套 skill 文档。

### Deferred to Follow-Up Work

- 全量自动化的 CI doc ↔ `--help` 漂移门禁（本轮只保证覆盖命令无漂移）。
- `ralph-tools-handoff.md` handoff 深参考补全（若尚未落地）。
- 把 scenario navigation 拆成独立 auto-inject skill（本轮先放在 `ralph-tools.md` 内验证效果）。
- 重写 preset instructions 中的 skill 引用。

---

## Context & Research

### Relevant Code and Patterns

- **Skill 注册与加载：** `crates/ralph-core/src/skill_registry.rs` 通过 `include_str!` 注册 6 个 built-in skill（`ralph-tools`、`ralph-tools-tasks`、`ralph-tools-memories`、`ralph-tools-emit`、`ralph-tools-wave`、`ralph-tools-cmdref`）；`build_index` 生成 `ralph tools skill list` 表格。
- **自动注入逻辑：** `crates/ralph-core/src/event_loop/mod.rs:4479-4582` `inject_memories_and_tools_skill`：
  - `ralph-tools` 在 `memories.enabled || tasks.enabled` 时注入；
  - `ralph-tools-tasks` 仅在 `tasks.enabled` 时注入；
  - `ralph-tools-memories` 仅在 `memories.enabled` 时注入。
  旧引用 `4365-4380` 已漂移，需更新。
- **`task.resume` payload：** `crates/ralph-core/src/event_loop/rejection.rs:424-500+` `build_task_resume_payload` 当前字段包括 `stage`、`topic`、`violation`、`allowed_topics`、`required_fields`、`original_trigger_topic`、`original_trigger_payload`、`retry_key`、`wave_id`/`wave_index`/`wave_total`（wave 相关时）。旧引用 `386-460` 已漂移，需更新。
- **事件文件解析：** `crates/ralph-cli/src/cli/emit_path.rs:32-145+` `resolve_emit_path` 实现 allowlist + 三级回退；`crates/ralph-cli/src/wave.rs:580-590` `resolve_events_file` 使用 `current-events` 而非 `current-candidate-events`。
- **Skill 可见性：** `crates/ralph-cli/src/skill_cli.rs:77-87` `resolve_skill_hat_filter` 要求 agent 上下文必须设置 `RALPH_CURRENT_HAT`。
- **`ralph hats validate`：** `crates/ralph-cli/src/hats.rs:170` `validate_hats` 入口；`--strict` 启用 lint 所有权检查。

### Current Drift Snapshot（来自 `scripts/check-cli-doc-drift.sh --strict`）

- **Task commands：** `tools task add/ensure` 缺少 `--description`、`--priority`；所有 task 子命令缺少 `--root`；`tools task list` 缺少 `--days`、`--limit`、`--status`；文档仍列出已移除的 `--all`、`--key`、`--blocked-by`、`--format`（子命令间共享段导致误报）。
- **Memory commands：** `memory prime` 新增 `--budget`；文档仍列出子命令不再共有的 `--all`、`--force`、`--last`、`--private`、`--recent`、`--tags`、`--type` 等。
- **Emit：** 参数表整体较新，但需确认 `--schema` 说明和行号引用。
- **Wave：** `ralph-tools-wave.md` 仍列出 `--arg`、`--argjson`，实际 `--help` 已移除。
- **Cmdref：** `ralph run` 缺少 `--no-default-profiles`、`--no-sync-agent-docs`；`ralph clean` 缺少 `--diagnostics`、`--dry-run`；`ralph web` 缺少多 port/workspace flag；`ralph tools skill load` 不应列出 `--format`/`--quiet`；`ralph tools skill list` 不应列出 `--quiet`。

### Drift Script行为注意

当前 `scripts/check-cli-doc-drift.sh` 对 `--strict` 的实际退出码为 0（疑似脚本中 STRICT 分支未正确生效），但漂移消息本身可信。实施时应以**消除消息**为准，不以退出码 0 作为通过依据。

---

## Key Technical Decisions

1. **保持 skill 文件拆分，新增场景导航层**：不改 `ralph-tools-*.md` 的边界，仅在 `ralph-tools.md` 顶部加场景索引，降低维护成本。
2. **`ralph-tools.md` 作为唯一自动注入入口**：所有新概念的入口指针放在这里；深参考仍按需 load，避免 token 膨胀。
3. **先修正 drift，再补概念和示例**：不先重构结构，而是确保现有命令表准确后，再叠加场景导航和示例。
4. **行号引用用「函数/区间」而非单点**：例如 `rejection.rs:424-500+` 覆盖 `build_task_resume_payload` 主体，减少下次小改动导致的漂移。
5. **`check-cli-doc-drift.sh` 本次不重构脚本，只更新文档**：脚本的 strict 退出码问题作为独立 follow-up，不在本轮修复。

---

## Open Questions

### Resolved During Planning

- **场景导航小节位置？** 放在 `ralph-tools.md` 顶部（紧接前提说明之后），让 agent 每轮第一眼看到决策索引。
- **新 flag 详细程度？** `--no-default-profiles` / `--no-sync-agent-docs` 在 `ralph-tools.md` 顶部导航表用 1 句话说明，详细参数仍放在 `ralph-tools-cmdref.md`。
- **覆盖命令集合？** 优先修复 agent 高频使用的 emit / task / memory / wave / run，cmdref 中其他低频命令以「查 `--help`」方式处理，不追求全部消除漂移。

### Deferred to Implementation

- `check-cli-doc-drift.sh` strict 退出码异常是否要在本 PR 顺手修复？待实施时评估：若只是 `set -e` 与参数解析顺序问题，可小修；若涉及脚本逻辑重构，则另开任务。

---

## Implementation Units

- [ ] U1. **Audit current drift and source references**

**Goal:** 建立 data 文档与当前源码/CLI 的精确差异清单，为后续编辑提供依据。

**Requirements:** R1, R2, R12

**Dependencies:** 无

**Files:**
- Read: `crates/ralph-core/data/*.md`
- Read: `crates/ralph-core/src/event_loop/mod.rs`（注入段）
- Read: `crates/ralph-core/src/event_loop/rejection.rs`
- Read: `crates/ralph-cli/src/cli/emit_path.rs`
- Read: `crates/ralph-cli/src/wave.rs`
- Read: `crates/ralph-cli/src/skill_cli.rs`
- Read: `crates/ralph-cli/src/hats.rs`
- Run: `scripts/check-cli-doc-drift.sh --strict`
- Run: `scripts/guard-prompt-size.sh`

**Approach:**
- 用 `ralph <cmd> --help` 逐一核对 `check-cli-doc-drift.sh` 中 `COMMANDS_TO_DOCS` 映射的命令。
- 记录所有 forward drift（help 有而文档无）和 reverse drift（文档有而 help 无）。
- 记录所有 `*.rs:NN` 行号引用，与当前源码比对。

**Test scenarios:**
- Happy path: 生成差异清单，无遗漏高频命令。
- Edge case: 识别共享 section 导致的跨命令误报（如 `--all` 被映射到每个 task 子命令）。

**Verification:**
- 差异清单覆盖 task / memory / emit / wave / run / skill / clean / web / init / plan / code-task / tui。
- `guard-prompt-size.sh` 当前通过（`ralph-tools.md` ≤ 200 行）。

---

- [ ] U2. **Refresh `ralph-tools.md` injection reference, scenario navigation, and new concepts**

**Goal:** 让每轮自动注入的入口文档既准确又具备场景导航能力。

**Requirements:** R1, R3, R4, R5, R6, R7, R8, R9, R10

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools.md`

**Approach:**
- 将注入逻辑引用从旧行号更新为 `crates/ralph-core/src/event_loop/mod.rs:4479-4582`。
- 将 `task.resume` payload 引用更新为 `crates/ralph-core/src/event_loop/rejection.rs:424-500+`。
- 在 `# Ralph CLI 核心参考` 后新增「AI 决策速查」小节（25–40 行），按场景给出「下一步该做什么 / 该 load 哪份 skill」：
  - loop 拒绝 / `task.resume` → 读本段 + 按需 `ralph-tools-emit.md` / handoff skill
  - 发射单个事件 / schema 预检 → `ralph-tools-emit.md`
  - 派发 review wave / 作为 wave worker 返回 → `ralph-tools-wave.md`
  - task 管理（含 R4 Single-U）→ `ralph-tools-tasks.md`
  - memory / decision journal → `ralph-tools-memories.md`
  - worktree 复用 / `ralph run` 参数 → `ralph-tools-cmdref.md`
  - 诊断 / ledger 恢复 → `docs/guide/runtime-diagnosis.md`
- 为每个场景补充 1 条完整命令示例。
- 新增/刷新 `ralph emit --schema`、U11 StateLedger / `ralph loops clean --ledger`、`--no-default-profiles`、`--no-sync-agent-docs`、`ralph inspect profiles` 的简要说明。
- 刷新通用错误恢复表，加入 `events file not in allowlist`、`policy validation failed`、`progress_task_mismatch` / `plan.blocked`、`skill not found`（含 `RALPH_CURRENT_HAT`）、ledger 损坏降级提示。
- 保留原有命令速查表，但将其后置到场景导航之后。

**Patterns to follow:**
- 保持现有中文撰写风格（面向人类输出必须中文）。
- 行号引用使用函数级区间而非单点。
- 不突破 `guard-prompt-size.sh` 200 行上限。

**Test scenarios:**
- Happy path: 刷新后 `ralph-tools.md` ≤ 200 行且 `guard-prompt-size.sh` 通过。
- Edge case: 场景导航表能在 1 步内定位到目标 skill。
- Error path: 若行号引用再次漂移，后续 `check-cli-doc-drift.sh` 虽不检行号，但 AGENTS.md 反向验证规则要求人工复核。

**Verification:**
- `scripts/guard-prompt-size.sh` 通过。
- 文档中所有 `*.rs:NN` 引用与当前源码一致。

---

- [ ] U3. **Refresh `ralph-tools-tasks.md` Task Commands flag table**

**Goal:** 消除 task 子命令的 drift。

**Requirements:** R2, AE1

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-tasks.md`

**Approach:**
- 将 Task Commands 示例代码块和参数说明改为按当前 `ralph tools task <subcmd> --help` 对齐：
  - `add` / `ensure` 增加 `--description`、`-p/--priority`、保留 `--blocked-by`、`--format`、`--root`。
  - `list` 增加 `--days`、`-l/--limit`、`--status`、`--root`。
  - `ready` 增加 `--root`。
  - 其余 mutate 子命令（`start`/`close`/`fail`/`reopen`/`show`）增加 `--root`。
- 删除文档中暗示所有子命令共有的 `--all`、`--key` 等已移除 flag。
- 如果按命令展示 flag 会重复，可采用「公共 flags + 各子命令特有 flags」两段式，但需与 drift 脚本提取方式兼容（当前脚本按 section 整体提取 flags）。

**Patterns to follow:**
- 与 `ralph-tools.md` 中 R4 Single-U 契约描述保持一致。

**Test scenarios:**
- Covers AE1. `ralph tools task add --help` 包含 `--description`、`--priority`、`--root`，文档 Task Commands 段也列出。
- Error path: 文档不再列出 `--help` 已移除的 `--all`、`--key`。

**Verification:**
- `scripts/check-cli-doc-drift.sh --strict` 对 task 子命令无新漂移消息。

---

- [ ] U4. **Refresh `ralph-tools-memories.md` Memory Commands flag table**

**Goal:** 消除 memory 子命令的 drift。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-memories.md`

**Approach:**
- 更新命令示例：
  - `memory prime` 增加 `--budget`。
  - 各命令增加 `--root`（如 `--help` 所示）。
  - `memory list` 保留 `--last`（如 help 有则保留，无则删除）。
- 删除「Note: All memory commands accept ...」段中已不共有的 `--all`、`--force`、`--last`、`--private`、`--recent`、`--tags`、`--type` 等笼统声明。
- 改为按实际命令列出可用 flags，避免共享 section 误报。

**Test scenarios:**
- Happy path: `memory prime --help` 包含 `--budget`，文档列出。
- Error path: 文档不再把 `delete`/`init` 等子命令没有的 flag 说成公共 flag。

**Verification:**
- `scripts/check-cli-doc-drift.sh --strict` 对 memory 子命令无新漂移消息。

---

- [ ] U5. **Refresh `ralph-tools-emit.md` line references and `--schema` guidance**

**Goal:** 保证 emit 深参考与源码一致，`--schema` 用法清晰。

**Requirements:** R1, R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-emit.md`

**Approach:**
- 更新 `*.rs:NN` 引用为当前实际位置（如 `emit_path.rs:32-145+`、`wave.rs:580-590`）。
- 校验 `--schema` 参数说明是否完整：只读、与 payload/policy-check 互斥、`protocol_hash` 用途、常见 drift 检测流程。
- 保留 `NULL payload 拒收白名单` 和 `Unified Pipeline(U11)` 段，确认行号引用。

**Test scenarios:**
- Covers AE3. `ralph emit --schema work.done | jq -r .protocol_hash` 用法在文档中明确。
- Edge case: `--schema` 与 `-j` / `--policy-check` 互斥说明准确。

**Verification:**
- `scripts/check-cli-doc-drift.sh --strict` 对 `emit` 命令无新漂移消息。

---

- [ ] U6. **Refresh `ralph-tools-wave.md` flag table**

**Goal:** 消除 wave emit 的 drift。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-wave.md`

**Approach:**
- 删除 `--arg` / `--argjson` 引用（已从 `--help` 移除）。
- 核对 `--payloads` / `--payloads-stdin` / `--output` / `--idempotency-key` / `--policy-check` / `--unsafe-no-policy-check` 与当前 help 一致。
- 更新 `*.rs:NN` 引用。

**Test scenarios:**
- Error path: 文档不再列出 `--help` 已移除的 `--arg`、`--argjson`。

**Verification:**
- `scripts/check-cli-doc-drift.sh --strict` 对 `wave emit` 无新漂移消息。

---

- [ ] U7. **Refresh `ralph-tools-cmdref.md` run flags and skill section**

**Goal:** 消除 cmdref 中高频命令的 drift。

**Requirements:** R2, R6

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-cmdref.md`

**Approach:**
- 在 `ralph run` 参数表中加入 `--no-default-profiles`、`--no-sync-agent-docs`。
- 更新 `ralph run` 与 `ralph inspect profiles` 的互引说明。
- 清理 `ralph tools skill load` 段：删除 `--format`、`--quiet`；`ralph tools skill list` 删除 `--quiet`。
- 对低频命令（clean/web/init/plan/code-task/tui 等），在「其他命令」表中仅保留命令名和 1 句说明，不逐条列出所有 flags，引导 agent 用 `ralph <cmd> --help`。这同时减少 drift 脚本的正向误报。

**Patterns to follow:**
- 与 `ralph-tools.md` 中 worktree 复用规则描述一致。

**Test scenarios:**
- Covers AE1/AE3. `ralph run --help` 包含 `--no-default-profiles`、`--no-sync-agent-docs`，文档列出。
- Error path: `ralph tools skill load --help` 不含 `--format`，文档不再列出。

**Verification:**
- `scripts/check-cli-doc-drift.sh --strict` 对 `run`、`tools skill list`、`tools skill load` 无新漂移消息。

---

- [ ] U8. **Run validation gates and fix residual drift**

**Goal:** 确保刷新后的文档通过所有门禁。

**Requirements:** R12

**Dependencies:** U2, U3, U4, U5, U6, U7

**Files:**
- Run: `scripts/check-cli-doc-drift.sh --strict`
- Run: `scripts/guard-prompt-size.sh`
- Run: `cargo nextest run -p ralph-core -- skill_registry`
- Optional modify: `scripts/check-cli-doc-drift.sh`（若顺手修复 strict 退出码）

**Approach:**
- 依次运行三门禁，对任何新增漂移消息返回对应 U 重新修正。
- 若 `check-cli-doc-drift.sh --strict` 仍以退出码 0 通过但屏幕打印 drift，以**消除消息**为准。
- 若发现脚本 strict 逻辑明显小 bug（如参数解析顺序），可顺手修复；否则记录为 follow-up。

**Test scenarios:**
- Integration: 修改 data 文件后 built-in skill 仍能被 `SkillRegistry::register_builtins` 正确加载。
- Error path: 任一漂移消息出现时，定位到具体 skill 文件和命令并重跑。

**Verification:**
- `cargo nextest run -p ralph-core -- skill_registry` 通过。
- `scripts/guard-prompt-size.sh` 通过。
- `scripts/check-cli-doc-drift.sh --strict` 对本次覆盖命令无漂移消息。

---

## System-Wide Impact

- **Interaction graph:** data 文件通过 `include_str!` 嵌入 `ralph-core` 二进制；修改后影响所有使用 built-in skill 的 loop，但无运行时行为变更。
- **Error propagation:** 文档错误可能导致 agent 用错命令，间接产生 `task.resume` 或 loop 终止；刷新旨在降低此类错误。
- **State lifecycle risks:** 无状态变更。
- **API surface parity:** 文档仅跟随现有 CLI；不新增/删除 CLI 接口。
- **Integration coverage:** skill registry 测试验证 built-in skill 仍可加载；drift 脚本验证 doc ↔ help 一致性。
- **Unchanged invariants:** 运行时注入策略、skill 注册机制、CLI 行为本身不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| `ralph-tools.md` 场景导航加入后超过 200 行 | 严格控制新增内容在 25–40 行；优先用表格而非段落；必要时把示例合并到场景行内。 |
| `check-cli-doc-drift.sh` strict 退出码异常，导致误判通过 | 实施时以漂移消息是否存在为准，不以退出码 0 为准；必要时顺手修复脚本。 |
| 共享 section 导致漂移脚本持续误报 | 对 task/memory 改为按命令展示 flags，避免笼统「所有命令接受」声明。 |
| 行号引用在后续代码改动中再次漂移 | 使用函数级区间；AGENTS.md 反向验证规则要求修改相关源码后同步文档。 |

---

## Documentation / Operational Notes

- 完成后需在 PR 描述中说明：更新了哪些 data 文件、消除了哪些漂移、是否修改了校验脚本。
- `.claude/skills/ralph-tools/SKILL.md` 是 `crates/ralph-core/data/ralph-tools.md` 的 symlink，无需单独编辑，但需确认 symlink 未损坏。
- 若 `check-cli-doc-drift.sh` strict 退出码在本轮修复，需同步更新脚本注释与 KNOWN_DRIFTS 说明。

---

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md](../brainstorms/2026-06-28-ralph-core-data-agent-guide-refresh-requirements.md)
- Related code:
  - `crates/ralph-core/src/skill_registry.rs`
  - `crates/ralph-core/src/event_loop/mod.rs:4479-4582`
  - `crates/ralph-core/src/event_loop/rejection.rs:424-500+`
  - `crates/ralph-cli/src/cli/emit_path.rs:32-145+`
  - `crates/ralph-cli/src/wave.rs:580-590`
  - `crates/ralph-cli/src/skill_cli.rs:77-87`
  - `crates/ralph-cli/src/hats.rs:170`
- Related docs:
  - `docs/guide/runtime-diagnosis.md`
  - `docs/handbook/serial-preset-development.md`
- Validation scripts:
  - `scripts/check-cli-doc-drift.sh`
  - `scripts/guard-prompt-size.sh`
