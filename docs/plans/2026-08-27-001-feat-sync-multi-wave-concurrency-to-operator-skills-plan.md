---
title: "feat: 同步多 wave 并发能力到 operator skills"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
status: active
---

# feat: 同步多 wave 并发能力到 operator skills

## Problem Frame

`358acd7b`（parallel-forge 多 wave 并发调度）已在 runtime、preset、schema 和 `ralph-tools-wave.md` 落地，但三个 operator skill（`ralph-preset-author` / `ralph-preset-review` / `ralph-project-bootstrap`）尚未同步。当前状态：

- `ralph-preset-author` / `ralph-preset-review` 的 references 只覆盖单 wave 的 `ralph wave emit` / `ralph wave verify`，未覆盖 ready-set 选择、dispatch budget、integration turn、跨 wave 资源审计等多 wave 并发概念。
- `ralph-preset-review` 的 `finding-rubric.md` 和 `agent-skill-audit.md` 缺少多 wave 并发相关的 finding_id。
- `max_concurrent_waves` 在 `SupervisorConfig` 中不存在（grep 全仓零命中），仅在 plan 文档中作为「配置建议」提出，实际并发上限由 `max_concurrent_workers` 和 dispatcher instructions 的「最多 3 个 wave」约定控制。
- `ralph-project-bootstrap` 不涉 preset 拓扑设计，无需 wave 并发增强。

## Scope Boundaries

**包含：**
- `skills/ralph-preset-author/references/author-checklist.md` — 新增多 wave 并发 Hard questions
- `skills/ralph-preset-author/references/patterns.md` — 新增 multi-wave 并发 preset 模式
- `skills/ralph-preset-author/references/finding-rubric.md` — 新增 multi-wave 并发 finding
- `skills/ralph-preset-review/references/finding-rubric.md` — 新增 multi-wave 并发 finding（与 author 镜像）
- `skills/ralph-preset-review/references/agent-skill-audit.md` — 新增 wave capability 的 multi-wave 子项
- `skills/ralph-preset-review/fixtures/` — 新增 multi-wave 负样本 fixture
- `skills/ralph-preset-review/fixtures/README.md` — 同步 fixture 清单与验收命令

**不包含：**
- 修改 runtime 代码（`crates/ralph-core/` / `crates/ralph-cli/`）
- 修改 `presets/en/parallel-forge.yml` 或 `presets/schemas/parallel-forge.yml`
- 新增或修改 `ralph wave` CLI 命令
- `ralph-project-bootstrap` 的 wave 并发增强（该 skill 不涉 preset 拓扑设计）
- `max_concurrent_waves` 的 runtime 实现（当前不存在，仅作「预留」在文档中说明）

## Requirements Trace

| 需求 | 来源 | 验收标准 |
|---|---|---|
| author 起草 supervisor+wave preset 时，必须回答多 wave 并发相关问题 | 用户请求 + `358acd7b` 改动 | `author-checklist.md` 新增 5 条 Hard questions，覆盖 ready-set / dispatch budget / integration turn / 资源审计 / 恢复语义 |
| review 评审 supervisor+wave preset 时，必须发现多 wave 并发违规 | 用户请求 + `358acd7b` 改动 | `finding-rubric.md` 新增 5 条 review-only finding，含 fixture 验收 |
| agent-skill-audit 必须检查 `ralph-tools-wave.md` 的多 wave 并发段 | 用户请求 | `agent-skill-audit.md` 新增 4 条检查项，覆盖多 wave 并发段的完整性 |
| 新增 fixture 必须被 review skill 验收 | 用户请求 | fixture README 新增 §11，含 anti-pattern 轴、expected finding、验收命令 |
| `max_concurrent_waves` 的落地状态必须被记录 | 用户请求 | plan 中明确记录「runtime 未实现，当前由 instructions 约定 3 wave 上限」 |

## Key Technical Decisions

1. **`max_concurrent_waves` 处理策略**：runtime 未实现该字段（`SupervisorConfig` 无此字段，全仓 grep 零命中）。当前多 wave 并发上限由 `presets/en/parallel-forge.yml` 的 dispatcher instructions 约定「最多 3 个 wave」控制。因此 author/review 的检查项标记为「预留：runtime 未实现 `max_concurrent_waves`，当前由 instructions 约定上限」，而非强制检查 YAML 字段。

2. **finding 全部 review-only**：多 wave 并发依赖「hat 视角能 Observe / 调用什么」的判断，机械 lint 无法从 YAML 形状断言（例如 dispatcher 是否在一次 activation 中混组多个 wave 的 payload）。因此所有新增 finding 均为 review-only，不进 `ralph preset check` JSON。

3. **fixture 保持 preset-neutral**：新增 multi-wave fixture 不复制 `parallel-forge.yml` 全拓扑，只保留触发 capability 的最小信号（`execution_model: supervisor+wave` + dispatcher hat instructions），顶部注释明示 anti-pattern 轴与 expected finding。

4. **author 与 review 镜像同步**：`author-checklist.md` 的 Hard questions 与 `finding-rubric.md` 的 finding 保持一一对应，确保 author 自检和 review 独立审使用同一套词汇。

## Implementation Units

### U1. 更新 `skills/ralph-preset-author/references/author-checklist.md` — 新增多 wave 并发 Hard questions

**Goal:** 在「Hard questions — supervisor orchestration」段后新增「Hard questions — multi-wave concurrency」段，覆盖 ready-set 选择、dispatch budget、integration turn、资源审计、恢复语义。

**Requirements:** R1

**Dependencies:** 无

**Files:**
- `skills/ralph-preset-author/references/author-checklist.md`

**Approach:**
- 在「Hard questions — supervisor orchestration」段（约第 265 行）后插入新段。
- 触发条件：`execution_model ∈ {supervisor+wave}` 或 `event_loop.supervisor.enabled: true` 且 dispatcher 调 `ralph wave emit`。
- 5 条 Hard questions，每条 ✓ / ✗ + 证据：
  1. **ready-set 定义**：`execution_wave` 字段是否表达 DAG 依赖（哪些 wave 可并发），而非串行序号？
  2. **dispatch budget**：dispatcher 是否声明每次 activation 最多 dispatch N 个 ready wave（N ≤ 3），且每个 wave 独立 payload 文件、独立 `wave verify` → `wave emit`？
  3. **integration turn**：integrator 是否按 `integration_order` 串行 merge，而非按完成时间抢 merge？
  4. **资源审计**：guardian 是否审计跨 wave 资源冲突（端口、数据库、容器、缓存、生成文件），并定义资源命名空间隔离规则？
  5. **恢复语义**：多 active wave 中任一 wave 失败是否只影响该 wave 的 correction/failure 路径，不阻断其它 wave？
- 段末注明：`max_concurrent_waves` 为预留能力（runtime 未实现），当前由 instructions 约定上限。

**Test scenarios:**
- 起草一个 `supervisor+wave` preset，按新 Hard questions 自检，5 条全部 ✓
- 起草一个 `supervisor+wave` preset，故意违反 dispatch budget（混组两个 wave 的 payload），自检应发现 ✗

**Verification:**
- `author-checklist.md` 新增段与 `finding-rubric.md` 新增 finding 一一对应
- 段内引用 `ralph-tools-wave.md`「多个并发 wave」章节

### U2. 更新 `skills/ralph-preset-author/references/patterns.md` — 新增 multi-wave 并发 preset 模式

**Goal:** 在 patterns.md 中新增「Multi-wave concurrency pattern（supervisor+wave）」段，描述 ready-set 选择、dispatch budget、integration turn、资源命名空间隔离的通用模式。

**Requirements:** R1

**Dependencies:** U1

**Files:**
- `skills/ralph-preset-author/references/patterns.md`

**Approach:**
- 在「Wave slot 自动重试」段后插入新段。
- 内容：执行模型 `supervisor+wave` 下的多 wave 并发通用模式，包含：
  - `execution_wave` 字段的 DAG 语义（依赖就绪后可并发启动）
  - dispatcher 每次 activation 最多 dispatch 3 个 ready wave（payload 隔离）
  - integrator 按 `integration_order` 串行 merge（integration turn）
  - 跨 wave 资源命名空间隔离（`plan_key + wave_id + slot_index`）
  - 恢复边界：单 wave 失败不影响其它 active wave
- 引用 `presets/en/parallel-forge.yml` 作为参考实现（但不复制其拓扑）

**Test scenarios:**
- 阅读 patterns.md 后，能复述 multi-wave 并发与单 wave 串行的关键差异
- 能指出 `execution_wave` 与 `integration_order` 的语义区别

**Verification:**
- 段内引用 `ralph-tools-wave.md`「多个并发 wave」章节
- 不复制 `parallel-forge.yml` 的具体 hat 名或 topic 名

### U3. 更新 `skills/ralph-preset-author/references/finding-rubric.md` 和 `skills/ralph-preset-review/references/finding-rubric.md` — 新增 multi-wave 并发 finding

**Goal:** 在「Wave capability audit」段后新增「Multi-wave concurrency audit」段，定义 5 条 review-only finding。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- `skills/ralph-preset-author/references/finding-rubric.md`
- `skills/ralph-preset-review/references/finding-rubric.md`

**Approach:**
- 触发条件：`execution_model ∈ {supervisor+wave}` 或 `event_loop.supervisor.enabled: true` 且 dispatcher 调 `ralph wave emit`；capability-triggered，禁止按 preset 名称门控。
- 新增 finding（全部 review-only，不进 `ralph preset check` JSON）：

| finding_id | default_severity | default_confidence | aaf_question | category | 含义 |
|---|---|---|---|---|---|
| `preset.wave_ready_set_not_dag` | P0 | 90 | Q4 | topology | `execution_wave` 字段被当作串行序号而非 DAG 依赖；可并发的 wave 被串行化 |
| `preset.wave_dispatch_budget_exceeded` | P0 | 90 | Q3 / Q4 | feasibility | dispatcher 一次 activation dispatch 超过声明上限的 wave，或混组多个 wave 的 payload 到同一 `wave emit` |
| `preset.wave_integration_turn_violated` | P0 | 90 | Q4 / Q5 | topology | integrator 未按 `integration_order` 串行 merge，而是按完成时间抢 merge |
| `preset.wave_resource_conflict_unaudited` | P1 | 80 | Q2 / Q3 | feasibility | guardian 未审计跨 wave 资源冲突（端口、数据库、容器、缓存、生成文件） |
| `preset.wave_failure_cross_contamination` | P0 | 90 | Q4 / Q5 | topology | 单 wave 失败导致其它 active wave 被错误重放、阻断或污染 |

- 段末注明：`max_concurrent_waves` 为预留能力（runtime 未实现），当前由 instructions 约定上限；若未来 runtime 实现该字段，review 需同步检查 `event_loop.supervisor.max_concurrent_waves` 的 YAML 配置。

**Test scenarios:**
- 阅读 finding-rubric.md 后，能指出 5 条 finding 的触发条件与修复面
- 能将 fixture 中的 anti-pattern 映射到对应 finding_id

**Verification:**
- author 和 review 两份 `finding-rubric.md` 的新增段完全一致
- 每条 finding 的 `category` / `aaf_question` / `default_severity` / `default_confidence` 与现有表格格式一致

### U4. 更新 `skills/ralph-preset-review/references/agent-skill-audit.md` — 新增 wave capability 的 multi-wave 子项

**Goal:** 在「Scope contract skill audit」段后新增「Multi-wave concurrency skill audit」段，检查 `ralph-tools-wave.md` 的多 wave 并发段是否完整。

**Requirements:** R3

**Dependencies:** U3

**Files:**
- `skills/ralph-preset-review/references/agent-skill-audit.md`

**Approach:**
- 触发条件：reviewer 选择「同时审查注入 skill 文档」且 preset 触发 `supervisor+wave` capability。
- 新增 4 条检查项：

| 检查项 | finding_id | default_severity | default_confidence | 含义 |
|---|---|---|---|---|
| `ralph-tools-wave.md` 缺少「多个并发 wave」章节（ready-set / dispatch budget / integration turn / 资源隔离） | `agent_skill.unreadable` | P1 | 85 | skill 文档缺少关键多 wave 并发约束 |
| `ralph-tools-wave.md` 未说明 dispatcher 每次 activation 最多 dispatch N 个 wave 的上限约定 | `agent_skill.unreadable` | P1 | 85 | 关键并发上限描述缺失 |
| `ralph-tools-wave.md` 未说明每个 wave 必须独立 payload 文件、独立 `wave verify` → `wave emit` | `agent_skill.unreadable` | P1 | 85 | payload 隔离约束描述缺失 |
| `ralph-tools-wave.md` 未说明 integrator 按 `integration_order` 串行 merge 而非按完成时间抢 merge | `agent_skill.unreadable` | P1 | 85 | integration turn 约束描述缺失 |

**Test scenarios:**
- 阅读 agent-skill-audit.md 后，能按 4 条检查项审 `ralph-tools-wave.md`
- 若 `ralph-tools-wave.md` 缺少「多个并发 wave」章节，能命中 `agent_skill.unreadable`

**Verification:**
- 新增段与 `finding-rubric.md` 的 `agent_skill.*` finding 表格式一致
- 引用 `ralph-tools-wave.md` 的具体章节名

### U5. 新增 `skills/ralph-preset-review/fixtures/multi-wave-concurrency-negative-fixture.yml`

**Goal:** 新增一个 preset-neutral 的负样本 fixture，覆盖 5 条 multi-wave 并发 anti-pattern。

**Requirements:** R4

**Dependencies:** U3

**Files:**
- `skills/ralph-preset-review/fixtures/multi-wave-concurrency-negative-fixture.yml`

**Approach:**
- fixture 保持 preset-neutral（不复制 `parallel-forge.yml` 全拓扑），只保留触发 capability 的最小信号：
  - `event_loop.execution_mode: isolated`
  - `event_loop.supervisor.enabled: true`
  - dispatcher hat `instructions` 含 `ralph wave emit` / `ralph wave verify`
- 5 条 anti-pattern 轴，每条 inline comment 标注 expected finding：
  - Axis (a)：`execution_wave` 被当作串行序号（wave 1 → wave 2 → wave 3 必须顺序执行）→ `preset.wave_ready_set_not_dag`
  - Axis (b)：dispatcher 混组两个 wave 的 payload 到同一 `wave emit` → `preset.wave_dispatch_budget_exceeded`
  - Axis (c)：integrator 按完成时间抢 merge，不按 `integration_order` → `preset.wave_integration_turn_violated`
  - Axis (d)：guardian 未审计跨 wave 资源冲突 → `preset.wave_resource_conflict_unaudited`
  - Axis (e)：单 wave 失败导致其它 wave 被错误重放 → `preset.wave_failure_cross_contamination`

**Test scenarios:**
- `ralph preset check -H skills/ralph-preset-review/fixtures/multi-wave-concurrency-negative-fixture.yml --strict --format json` 可解析（不崩溃）
- 软性 AAF review 按 fixture 顶部注释对照命中 5 条 finding

**Verification:**
- fixture 可被 `ralph preset check` 加载，JSON 输出可解析
- 5 条 anti-pattern 与 `finding-rubric.md` 新增 finding 一一对应

### U6. 更新 `skills/ralph-preset-review/fixtures/README.md` — 同步 fixture 清单与验收命令

**Goal:** 在 README 中新增 §11「Multi-wave concurrency fixture」，记录 anti-pattern 轴、expected finding、验收命令。

**Requirements:** R4

**Dependencies:** U5

**Files:**
- `skills/ralph-preset-review/fixtures/README.md`

**Approach:**
- 在 §10「Scope contract fixtures」后插入 §11。
- 表格格式与现有 fixture 一致：Axis / What the YAML contains / Expected finding / Source。
- 验收命令：
  ```bash
  # CLI 冒烟 — 不要求 strict 模式吐出新 finding（review-only 不进 lint）；
  # 仅验证 fixture 可被加载 + 结构化 lint 不崩。
  ralph preset check -H skills/ralph-preset-review/fixtures/multi-wave-concurrency-negative-fixture.yml --strict --format json

  # 软性 AAF (multi-wave concurrency) 是 review-only, 通过阅读 fixture 配合
  # references/finding-rubric.md 「Multi-wave concurrency audit」段对照命中。
  ```
- 注明「Review-only finding 必须显式人工审」与现有 fixture 一致。

**Test scenarios:**
- README §11 的表格与 fixture 顶部注释一致
- 验收命令可直接复制执行

**Verification:**
- README 新增段与现有 fixture 段格式一致
- 验收命令中的 fixture 路径与 U5 新增文件路径一致

## Deferred to Follow-Up Work

- `max_concurrent_waves` 的 runtime 实现：当前 `SupervisorConfig` 无此字段，若未来实现，需同步更新 `agent-skill-audit.md` 和 `finding-rubric.md` 的检查项。
- `ralph-project-bootstrap` 的 wave 并发声明：若未来 bootstrap 需要支持 supervisor+wave preset 的项目接入，需新增「bootstrap 不覆盖 wave 拓扑」的显式声明。

## System-Wide Impact

- **Developers**: preset author 和 reviewer 需要学习新的 multi-wave 并发检查项；新增 fixture 可用于 skill 培训。
- **Operators**: 无直接影响；多 wave 并发是 preset 内部行为。
- **Other teams**: 无跨团队影响；改动仅限 `skills/` 目录下的文档和 fixture。

## Risk Analysis & Mitigation

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| author/review 遗漏 `max_concurrent_waves` 未实现的状态 | 中 | 低 | 在 plan、author-checklist、finding-rubric 中显式注明「预留：runtime 未实现」 |
| fixture 与 `parallel-forge.yml` 过度耦合 | 低 | 中 | fixture 保持 preset-neutral，只保留最小 capability 信号 |
| 新增 finding 与现有 finding 冲突 | 低 | 低 | 全部 review-only，不进 `ralph preset check` JSON；与现有 `preset.wave_*` / `preset.supervisor_*` 段独立 |
| `ralph-tools-wave.md` 未来改动导致 agent-skill-audit 过时 | 中 | 低 | 在 agent-skill-audit 中注明「若 `ralph-tools-wave.md` 新增章节，需同步更新检查项」 |

## Verification

1. 所有新增/修改的文件通过 `scripts/check-cli-doc-drift.sh` 静态 drift 扫描。
2. fixture 通过 `ralph preset check --strict --format json` 加载验证（不崩溃）。
3. author 和 review 两份 `finding-rubric.md` 的新增段内容一致。
4. `author-checklist.md` 的 Hard questions 与 `finding-rubric.md` 的 finding 一一对应。
5. 运行 `./scripts/run-tests.sh` 确保无回归（本次改动为纯文档 + fixture，不涉及代码）。
