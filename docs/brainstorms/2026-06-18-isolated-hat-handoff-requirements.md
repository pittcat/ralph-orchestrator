---
date: 2026-06-18
topic: isolated-hat-handoff
title: "Isolated 模式 hat→hat 简短 roadmap 交接（全局单模板）"
---

## Summary

在 `execution_mode: isolated` 下，为 **跨角色宏观 handoff 边** 引入统一的 **roadmap 交接文件**：上游 hat 在 emit 前写入固定五段式 Markdown（以 repo 相对路径为主），event payload 携带 `handoff_path`，机制层在下游 `build_prompt` 注入（可截断）。**不按 preset 分模板**；**微观 ping-pong 边豁免**（如 serial review 的 coordinator↔reviewer 内环）。与现有事件 handoff（topic + payload + `HandoffTracker`）互补，不替代 loop 结束的 `.ralph/agent/handoff.md`。

---

## Problem Frame

Isolated 模式下，hat B 激活时的上下文来自：trigger 事件 payload、`## ORCHESTRATOR CONTEXT`、scratchpad、以及 preset 各 hat 长指令里重复的「去读 context.md / plan.md / findings.md…」。机制层有 **事件级 handoff**（`work.ready`、`queue.advance` 等 + SLA），但 **没有** 统一的、简短的、按文件指针组织的 **hat→hat 导航地图**。

结果是：下游 hat 每轮重复翻文件、token 浪费、易漏读；不同 preset（`ce-executor-isolated`、`ce-executor-serial` 等）若各写一套 handoff 模板，维护成本不可接受。用户明确要求：**一个全局模板**，`## next` 必须写清下游优先动作与 blockers。

---

## Key Decisions

- **全局单模板，preset 无关** — 所有 isolated preset 共用同一五段式结构与同一组 `##` 标题；preset 差异通过 agent 填入的 **文件链接** 体现，不在 YAML 里嵌 handoff 模板。
- **宏观边强制、微观边豁免** — 「跨角色」边必须写 handoff；「同环 ping-pong」边（典型：serial 的 `review-coordinator ↔ dimension-reviewer` 每维一次）豁免，依赖已有 payload + 磁盘产物（如 `review-sequence.json`、`wave-diff.patch`）。
- **文件载体 + payload 指针** — 交接正文落盘；payload 带 `handoff_path`（repo 相对路径），不把全文塞进 payload。
- **多轮不覆写** — 文件名含 loop iteration 与单调序号，避免同一文件多轮互相覆盖。
- **硬门只校验结构** — 缺固定 `##` 节、或某节既无路径又无明确「无/未验证」占位 → 拒收 emit；**不**按 preset 校验语义字段。
- **与现有 handoff 分工** — 事件 payload = **做什么**（task_id、step、dimensions 数组等）；roadmap 文件 = **去哪看 + 下游第一步**；loop 结束 `handoff.md` 仍只服务 **下次 session**，不在 hat 切换时注入。

---

## Actors

- **A1 — 上游 hat（发布方）** — emit 前写 roadmap 文件；保证 `## next` 可执行。
- **A2 — 下游 hat（消费方）** — 激活时从 prompt 顶部看到注入块，按 `## next` 行动，再按需读 `## artifacts` / `## changed` 中的链接。
- **A3 — 机制层（orchestrator）** — 校验结构、注入、截断；配置宏观边清单与全局开关。
- **A4 — Preset 维护者** — 仅在宏观边的 hat instructions 中加一句「emit 前写 handoff」；**不**维护 per-preset 模板。

---

## Requirements

### 机制与配置

- R1. 新增全局配置项（名称由实现定，如 `event_loop.hat_handoff.enabled`），默认 **关闭**；开启后仅对配置的 **宏观边** 强制 handoff。
- R2. **宏观边**初始集合对齐 `workflow_contract` 中 **唯一消费者** handoff topic（如 `work.done`、`work.ready`、`queue.advance`、`plan.complete`、`fix.plan.ready`、`review.dimensions.complete`、`review.complete` 等——以 preset 已登记的 consumer 为准），且可通过配置扩展/收缩；**微观边**默认在豁免列表（至少包含：`review.dimension.ready`、`review.dimension.done`、`review.dimension.failed`）。
- R3. 交接文件路径约定：`.ralph/agent/hat-handoff/{iteration}-{seq}-{from_hat}-{to_hat}.md`。`iteration` 为当前 loop iteration；`seq` 为当轮单调递增序号（同 iteration 内每写一份 +1）。禁止覆盖同名文件。
- R4. 上游 emit 的 JSON payload **必须**包含 `handoff_path` 字段（repo 相对路径，指向 R3 文件），且路径必须存在、可读。宏观边缺字段或文件不存在 → 结构硬门拒收（`task.resume` 提示修复 handoff）。
- R5. 下游 hat `build_prompt`（isolated 路径）在 prepend 管线中注入 `## HAT HANDOFF` 块：读取 `handoff_path` 文件全文；超过上限（建议 ≤ 2KB 或 ≤ 500 token，实现可调）时 **保留头部 + 截断提示**，不静默丢节标题。
- R6. 注入块内须标明 `from_hat`、`to_hat`、`handoff_path`，便于 agent 与诊断对齐。

### 全局单模板（所有 preset 共用）

- R7. 交接文件 **必须**使用下列五级标题，顺序固定，标题字面量不可改：

  ```markdown
  # Handoff: {from_hat} → {to_hat}

  ## context
  ## artifacts
  ## changed
  ## verify
  ## next
  ```

- R8. 每节内容规则：
  - **以 repo 相对路径的 bullet 列表为主**；允许每行末尾 `—` 后接 **≤15 词**备注。
  - 某节无内容时 **必须**写占位单行：`无` / `未验证` / `不适用`（三选一，语义见 R9–R13）；**禁止**留空节或删除 `##` 标题。
  - **禁止**大段 prose、禁止粘贴 diff 全文、禁止复述 event payload 已带的结构化字段（如完整 `dimensions[]` JSON）。

- R9. `## context` — 下游理解局面所需的 **只读状态文件** 指针（如 `context.md`、`progress.md`、`plan.md`、`.ralph/agent/tasks.jsonl`）。无则写 `无`。

- R10. `## artifacts` — 本轮或本边产生的 **产物文件** 指针（如 `wave-diff.patch`、`review-sequence.json`、`findings.md`、各维度 `findings-*.json`）。无则写 `无`。

- R11. `## changed` — 本边相关 **源码/测试改动** 路径；每行一条，备注说明改动性质（如「U2 实现」）。无改动写 `无`。

- R12. `## verify` — 已运行的验证命令或证据文件路径；未跑则写 `未验证`；本边不需要验证则写 `不适用`。

- R13. `## next` — **下游第一步行动契约**（本节为硬要求，见下节专述）。

### `## next` 专述（下游优先动作）

- R14. `## next` **有且仅有 1–3 行**，每行一条 bullet，**禁止**子列表、禁止多段落。三行语义固定（可按需省略中间行，但顺序不变）：

  1. **动作行（必填）** — 以动词开头，写明下游 hat **本轮第一个动作**，须可执行、可判定完成。格式：`- **动作**: <动词短语>`  
     - 合格：`- **动作**: 读 wave-diff.patch，对 correctness 维度出 findings JSON 后 emit review.dimension.done`  
     - 合格：`- **动作**: 对照 progress.md 与 tasks.jsonl，emit queue.advance`  
     - 不合格：`- **动作**: 继续 review`（不可判定）  
     - 不合格：`- **动作**: 按 preset 指令执行`（甩锅给 preset）

  2. **阻塞行（必填）** — 明确是否存在 blocker。格式：`- **阻塞**: <无 | 具体阻塞一句话>`  
     - `无`：下游可直接执行动作行。  
     - 有阻塞：写 **一个** 主阻塞（多阻塞时写最高优先级一条，其余放 `## artifacts` 指向的日志/文件）。  
     - 合格：`- **阻塞**: 无`  
     - 合格：`- **阻塞**: progress.md 与 task u2-foo 未对齐，先 close task 再 emit`  
     - 不合格：`- **阻塞**: 可能有问题`（不可操作）

  3. **优先读行（可选）** — 当 `## artifacts` / `## context` 有多文件时，指明 **先读哪一个**。格式：`- **先读**: <repo 相对路径>`  
     - 仅在一行内；无优先顺序则省略此行。

- R15. `## next` 的 **动作行** 必须与当前 emit 的 **topic 与消费者 hat 角色一致**；不得指引下游 emit 其 `publishes` 之外的 topic（与 isolated 越权规则一致）。

- R16. 硬门校验 `## next`：缺少 `**动作**:` 或 `**阻塞**:` 行 → 拒收；动作行或阻塞行为空 → 拒收；动作行超过一行 → 拒收。

### 与现有机制的关系

- R17. 本机制 **不**替代：`HandoffTracker` SLA、`step_handoff` progress 硬门、`## ORCHESTRATOR CONTEXT`、`## WAVE CONTEXT`（wave synthesizer）、loop 结束 `HandoffWriter` → `handoff.md`。
- R18. `ralph-tools-handoff.md`（按需 skill）可增补 **如何修 handoff 硬门拒收** 的深参考；**不**进入每轮 auto-inject（token 预算），与 `ralph-tools.md` 自动纠偏段分工不变。
- R19. 可选：提供 `agent-handoff` 类 skill 的 **薄封装**，生成符合 R7–R16 的文件到 R3 路径；skill 为便利层，**非**唯一写入路径。

---

## Key Flows

- F1. 宏观边 emit（上游写 handoff）
  - **Trigger:** 上游 hat 即将 emit 宏观边 topic，且 `hat_handoff.enabled` 为 true。
  - **Actors:** A1, A3
  - **Steps:** 按 R7 模板写文件 → 硬门校验结构（含 R16）→ payload 带 `handoff_path` → emit。
  - **Outcome:** 磁盘有交接文件；事件带指针。

- F2. 下游激活注入
  - **Trigger:** 下游 hat `build_prompt`，pending 事件 payload 含 `handoff_path`。
  - **Actors:** A2, A3
  - **Steps:** 读文件 → 注入 `## HAT HANDOFF` → 下游先读 `## next` 再执行。
  - **Outcome:** 下游无需从长 preset 指令推断「第一步」。

- F3. 微观边豁免
  - **Trigger:** emit 的 topic 在微观豁免列表（如 `review.dimension.ready`）。
  - **Actors:** A1, A3
  - **Steps:** 不要求 `handoff_path`；下游仅依赖 payload + 既有磁盘产物。
  - **Outcome:** serial 四维内环不产生 4× 冗余 handoff 文件。

---

## Acceptance Examples

- AE1. **合格 handoff（executor → review-coordinator）**  
  - **Covers:** R7–R14  
  - **Given:** executor 完成 `work.done`，改动 2 个文件，测试已跑。  
  - **When:** 写入 handoff 并 emit。  
  - **Then:** 五节齐全；`## next` 含 `**动作**:` 与 `**阻塞**: 无`；`handoff_path` 可被下游注入；无 payload 全文重复。

- AE2. **`## next` 拒收**  
  - **Covers:** R14, R16  
  - **Given:** `## next` 仅写「继续处理」。  
  - **When:** 宏观边 emit。  
  - **Then:** 硬门拒收；`task.resume` 提示补全 `**动作**:` / `**阻塞**:` 行。

- AE3. **微观边豁免**  
  - **Covers:** R2, F3  
  - **Given:** `review-coordinator` emit `review.dimension.ready`，无 `handoff_path`。  
  - **When:** `hat_handoff.enabled` 且微观边在豁免列表。  
  - **Then:** emit 通过；`dimension-reviewer` 靠 payload + `wave-diff.patch` 工作。

- AE4. **多轮不覆写**  
  - **Covers:** R3  
  - **Given:** 同一 loop 两轮 `executor → review-coordinator`。  
  - **When:** 各写一份 handoff。  
  - **Then:** 两个不同 `{iteration}-{seq}-...` 文件均存在。

---

## Success Criteria

- 开启 `hat_handoff` 后，宏观边下游 hat 的 prompt 顶部 **总能** 看到可解析的 `## next`（动作 + 阻塞），无需读完整 preset 即可知第一步。
- Preset 仓库 **零** per-preset handoff 模板文件；`ce-executor-isolated` 与 `ce-executor-serial` 共用 R7 模板。
- Serial review 单 step 内环 **不** 因 handoff 产生 >1 份 coordinator↔reviewer 强制文件（豁免生效）。
- 硬门误拒率可通过 BDD/场景测试覆盖 AE1–AE3。

---

## Scope Boundaries

### Deferred for later

- 机制自动从 `git diff` 生成 `## changed` 骨架（agent 仍手写亦可）。
- Coordinator 模式下的 hat handoff（非 isolated）。
- `handoff_path` 历史索引 UI / `ralph diagnose` 专节展示。
- HTML 输出模式（本需求文档为 markdown；与 `ce-plan` 独立）。

### Outside this product's identity

- 按 preset 维护不同 handoff 模板或不同 `##` 标题集。
- 用 handoff 文件替代 event payload 业务字段（task_id、step、dimensions 数组等）。
- 在微观 ping-pong 边上强制完整五节 handoff（防止 serial 文件爆炸）。

---

## Dependencies / Assumptions

- 假设仅在 `execution_mode: isolated` 启用；coordinator 多 hat 单 prompt 路径不在本轮范围。
- 依赖现有 `build_prompt` isolated prepend 管线扩展注入点。
- 依赖 preset 已配置的 `workflow_contract` / `HandoffIndex` 可枚举宏观边（实现时可读配置，不要求维护者手抄边列表）。
- 假设上游 agent 有能力写 Markdown 文件到 `.ralph/agent/hat-handoff/`（与现有 scratchpad / tools 写盘能力一致）。

---

## Outstanding Questions

### Resolve Before Planning

- （无——对话已确认全局单模板与 `## next` 三行契约。）

### Deferred to Planning

- `handoff_path` 是否进入各 topic 的 JSON schema `required_fields`（仅宏观边）还是机制层旁路校验。
- 注入上限默认值（2KB vs 500 token）与截断时是否保留完整 `## next` 节。
- 豁免列表是否仅内置默认 + 配置覆盖，还是完全配置驱动。

---

## Sources / Research

- `crates/ralph-core/src/event_loop/mod.rs` — isolated `build_prompt` prepend 管线（`prepend_orchestrator_context`、`prepend_scratchpad` 等）。
- `crates/ralph-core/src/handoff.rs` — loop 结束 `handoff.md`（与 hat 切换无关）。
- `crates/ralph-core/data/ralph-tools-handoff.md` — 现有 step handoff **事件**深参考（topic 归属、progress 修复），与本需求的 **roadmap 文件**互补。
- `presets/en/ce-executor-serial.yml` — serial review 状态机 `review-sequence.json`；微观边豁免依据。
- `presets/en/ce-executor-isolated.yml` — wave review 拓扑；宏观边与 `review.wave.ready` 多消费者区分。
- `.cursor/rules/multi-hat-isolation.mdc` — isolated 终态 authority 与公平调度。
