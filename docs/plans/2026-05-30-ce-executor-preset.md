# ce-executor Preset 开发计划

## 目标

为 Ralph Orchestrator 添加一个新的内置 preset `ce-executor`，实现 **Plan → Work → Code Review → Auto-fix → Shipping → Manager Report** 的完整开发工作流。

该 preset 完全内嵌 `ce-work` 和 `ce-code-review` 的核心逻辑，不依赖 compound-engineering-plugin 是否安装，任何 Ralph 用户都能使用。

## 交付物清单

### 1. Preset YAML 文件（2个）

| 文件 | 说明 |
|------|------|
| `presets/ce-executor.yml` | 英文版 preset |
| `presets/ce-executor-zh.yml` | 中文版 preset |

### 2. 内嵌镜像（1个）

| 文件 | 说明 |
|------|------|
| `crates/ralph-cli/presets/ce-executor.yml` | 英文版镜像（供 `include_str!` 使用） |

> 注：中文版 `ce-executor-zh.yml` 不内嵌，仅作为 repo 中的可选 preset 文件使用。

### 3. Rust 代码更新（1个文件）

| 文件 | 修改内容 |
|------|---------|
| `crates/ralph-cli/src/presets.rs` | 添加 `ce-executor` 到 `PRESETS` 数组（中文版不内嵌） |

### 4. 同步脚本更新（1个文件）

| 文件 | 修改内容 |
|------|---------|
| `scripts/sync-embedded-files.sh` | 添加 `ce-executor.yml` 到 `MIRRORED_FILES` 数组（中文版不内嵌） |

### 5. Zsh 插件更新（1个文件）

| 文件 | 修改内容 |
|------|---------|
| `scripts/ralph-zsh-plugin.zsh` | 添加 `builtin:ce-executor` 到补全列表（中文版不加入 builtin） |

### 6. 索引文件更新（1个文件）

| 文件 | 修改内容 |
|------|---------|
| `presets/index.json` | 添加 `ce-executor` 条目（中文版不加入 index） |

### 7. 文档更新（1个文件）

| 文件 | 修改内容 |
|------|---------|
| `presets/README.md` | 添加 `ce-executor` 到 Supported Builtins 表格 |

## 架构设计

### Hat 设计总览

```
work.start → Coordinator → work.ready → Executor → work.done
                                              ↓
                                        review-coordinator
                                              ↓
                              review.wave.ready (wave emit)
                                              ↓
                    ┌─────────────────────────────────────────┐
                    │  dimension-reviewer (concurrency: 9)    │
                    │  ├── correctness                        │
                    │  ├── testing                            │
                    │  ├── maintainability                    │
                    │  ├── standards                          │
                    │  ├── requirements                       │
                    │  ├── agent-native                       │
                    │  ├── learnings                          │
                    │  ├── security (conditional)             │
                    │  ├── performance (conditional)          │
                    │  └── api-contract/reliability/adversarial│
                    └─────────────────────────────────────────┘
                                              ↓
                              review.dimension.done × N
                                              ↓
                                        review-synthesizer (aggregate)
                                              ↓
                    ┌─────────────┬──────────────┬─────────────────┐
                    ↓             ↓              ↓                 ↓
              review.passed  review.failed  review.complete  (residuals)
                    ↓             ↓              ↓                 ↓
                 Shipper        Fixer          Shipper          Shipper
                    ↓             ↓              ↓                 ↓
                                                         REVIEW_COMPLETE
                                                                ↓
                                                             Reporter → LOOP_COMPLETE
```

| Hat | 触发事件 | 发布事件 | 职责 |
|-----|---------|---------|------|
| **Coordinator** | `work.start` | `work.ready` | 解析 plan 文件，提取元数据，评估复杂度，初始化工作目录，分解任务 |
| **Executor** | `work.ready`, `queue.advance`, `work.retry` | `work.done`, `work.failed`, `queue.advance` | 执行 plan 中的 Implementation Units，TDD/characterization-first，增量提交，simplify as you go |
| **review-coordinator** | `work.done`, `fix.applied` | `review.wave.ready` | diff 分析、intent discovery、选择 review dimensions、发射 wave |
| **dimension-reviewer** | `review.wave.ready` | `review.dimension.done` | Wave worker，专注单一 dimension 的 code review，输出结构化 findings JSON |
| **review-synthesizer** | `review.dimension.done` | `review.passed`, `review.failed`, `review.complete` | 聚合所有 dimension findings，去重、校准、Quality Gates、输出最终 findings.md |
| **Fixer** | `review.failed` | `fix.applied`, `fix.exhausted` | 应用 safe_auto 修复，验证不破坏现有功能，管理轮次计数，排除 pre_existing |
| **Shipper** | `review.passed`, `review.complete`, `fix.exhausted` | `REVIEW_COMPLETE` | Final Validation，更新 plan status，创建 commit/PR，准备 Operational Validation Plan |
| **Reporter** | `REVIEW_COMPLETE` | `report.done`, `LOOP_COMPLETE` | 生成 Manager 汇报文档 |

### Event Loop 配置

```yaml
event_loop:
  prompt_file: "PROMPT.md"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.passed", "review.complete"]
  starting_event: "work.start"
  max_iterations: 50
  max_runtime_seconds: 14400  # 4 hours
  checkpoint_interval: 5
```

### 全局配置

```yaml
cli:
  backend: "kiro"
  prompt_mode: "arg"

core:
  specs_dir: ".agents/scratchpad/"
  guardrails:
    - "Fresh context each iteration — save learnings to memories for next time"
    - "Verification is mandatory — tests/typecheck/lint/audit must pass"
    - "YAGNI ruthlessly — no speculative features"
    - "KISS always — simplest solution that works"
    - "Confidence protocol: score decisions 0-100. >80 proceed autonomously; 50-80 proceed + document in .ralph/agent/decisions.md; <50 choose safe default + document."
```

### 工作目录与文件结构

```
.agents/scratchpad/ce-executor/{plan_name}/
├── context.md          # 执行上下文：plan 摘要、repo 模式、约束条件、分支信息
├── plan.md             # 从原始 plan 提取的编号步骤计划
├── progress.md         # 执行进度：当前步骤、活跃 wave、验证日志、已完成步骤
├── findings.md         # Review 发现：按 severity 分组的问题清单（review-synthesizer 写入）
├── fix-log.md          # 修复记录：每轮修复的内容、结果、测试验证（Fixer 写入）
├── shipping.md         # Shipping 记录：final validation 结果、plan status、PR 信息（Shipper 写入）
├── decisions.md        # 决策记录：confidence score <= 80 的决策（各 hat 写入）
└── logs/               # 构建/测试输出摘要
```

**文件格式规范**：

- `context.md` 必须包含：plan 来源类型（文件路径/粗略描述）、原始请求摘要、repo 模式、集成点、验收标准、约束条件、复杂度评估（trivial/small/large）
- `plan.md` 必须是编号步骤计划，每个步骤定义名称、可演示结果、预期子任务 wave
- `progress.md` 必须包含：`## Current Step`、`## Active Wave`、`## Verification Notes`、`## Completed Steps`
- `findings.md` 必须包含：`## Scope`、`## Findings`（按 P0-P3 分组表格）、`## Pre-existing Issues`、`## Testing Gaps`、`## Residual Risks`、`## Requirements Completeness`
- `fix-log.md` 必须包含：每轮 Round 的 Applied/Failed/Verification 记录，以及当前 `fix_round` 计数器
- `shipping.md` 必须包含：`## Final Validation Results`、`## Plan Status`、`## PR Info`、`## Known Residuals`、`## Operational Validation Plan`
- `decisions.md` 必须包含：ID（DEC-NNN, sequential）、confidence score、alternatives、reasoning、reversibility

### 事件 Payload 规范

每个事件的 payload 必须通过 `ralph emit` 携带，确保下游 hat 能正确解析状态：

| 事件 | 必须携带的 payload 字段 | 说明 |
|------|----------------------|------|
| `work.ready` | `plan_path`, `plan_name`, `complexity` | Coordinator 解析出的 plan 文件路径、名称、复杂度评估 |
| `work.done` | `task_id`, `task_key`, `commit_count`, `changed_lines` | 最后一个完成的 runtime task 的 ID 和 key，总提交数，改动行数 |
| `work.failed` | `task_id`, `task_key`, `reason` | 失败的任务 ID 和原因 |
| `review.passed` | `findings_count: 0`, `changed_lines` | 通过时 findings 为 0 |
| `review.failed` | `fix_round`, `safe_auto_count`, `gated_manual_count`, `findings_summary` | fix 轮次计数器、各类问题数量、摘要 |
| `review.complete` | `fix_round`, `verdict`, `residual_findings_count`, `findings_summary` | 最终 review 结果，含 residual findings |
| `fix.applied` | `fix_round`, `applied_count`, `failed_count` | 递增后的轮次、本次应用数量、失败数量 |
| `fix.exhausted` | `fix_round`, `residual_findings` | 达到最大轮次后的剩余问题清单 |
| `REVIEW_COMPLETE` | `verdict`, `final_findings_count`, `pass_or_fail` | 最终裁决 |
| `report.done` | `report_path` | 生成的报告文件路径 |
| `LOOP_COMPLETE` | — | loop 终止信号 |

---

### Coordinator Hat 详细设计

**触发**：`work.start`
**发布**：`work.ready`
**默认发布**：`work.ready`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## COORDINATOR MODE — Plan Parsing, Task Decomposition, and Complexity Assessment

You own plan parsing, task decomposition, complexity assessment, and queue initialization.
Do not implement. Do not review.
```

**2. Plan 文件解析协议**

- 从 prompt 中解析 plan 文件路径。支持 `.md` 和 `.html` 格式
- **错误处理**：
  - 如果文件不存在 → 发布 `work.failed`，payload 携带 `reason: "plan file not found: <path>"`，停止
  - 如果文件存在但无法读取（权限问题等）→ 发布 `work.failed`，`reason: "plan file unreadable: <path>"`，停止
  - 如果文件缺少 `Implementation Units`、`Work Breakdown` 或等效执行章节 → 发布 `work.failed`，`reason: "plan missing implementation units"`，停止
  - 如果 plan 内容为空或纯占位符 → 发布 `work.failed`，`reason: "plan content empty or placeholder"`，停止
- 验证文件存在且可读
- 读取 plan 文件时提取以下字段：
  - `Implementation Units` / `Work Breakdown` → 任务列表来源
  - `Execution note` → test-first / characterization-first 执行策略
  - `Deferred to Implementation` / `Implementation-Time Unknowns` → 执行前需解决的问题
  - `Scope Boundaries` → 明确非目标，防止 scope creep
  - `Patterns to follow` → 参考文件和约定
  - `Verification` → 每个单元的完成标准
  - `Test Scenarios` → 测试场景
  - `Requirements` / `Requirements Trace` → R-IDs 列表，用于后续验证
- 保留 plan 中的 U-IDs（如 U1, U2）作为 runtime task key 的前缀
- 保留 plan 中的 R-IDs（如 R1, R2）用于后续 requirements verification
- HTML plan 文件与 Markdown 等效处理（按 section 名匹配，忽略 HTML 包装噪音）

**3. 复杂度评估**

根据 plan 内容评估工作复杂度：

| 复杂度 | 信号 | 行为 |
|--------|------|------|
| **Trivial** | 1-2 文件，无行为改变（typo, config, rename） | 在 `context.md` 中标记 `complexity: trivial`。不创建详细 task list，只创建一个 summary task |
| **Small / Medium** | 清晰 scope，under ~10 文件，有 Implementation Units | 在 `context.md` 中标记 `complexity: small`。创建标准 task list |
| **Large** | Cross-cutting，架构决策，10+ 文件，触及 auth/payments/migrations | 在 `context.md` 中标记 `complexity: large`。创建标准 task list，并在 context.md 中记录风险标记 |

**4. 工作目录初始化**

- 确定 `plan_name`：从 plan 文件路径提取文件名（不含扩展名）
- 创建工作目录：`.agents/scratchpad/ce-executor/{plan_name}/`
- 创建 `context.md`：plan 来源类型、原始请求摘要、repo 模式、集成点、验收标准、约束条件、**复杂度评估**、**R-IDs 列表**
- 创建 `plan.md`：编号步骤计划，从 Implementation Units 映射为编号步骤
- 创建 `progress.md`：初始状态（Current Step = Step 1，Active Wave 为空，Completed Steps 为空）
- 创建 `decisions.md`：空文件（用于记录 confidence <= 80 的决策）
- 创建 `logs/` 目录

**5. Runtime Task 创建**

- 将 Step 1 的 Implementation Units 转换为 runtime tasks
- 使用 `ralph tools task ensure` 创建任务，stable key 格式：
  - `ce-executor:{plan_name}:step-01:{slug}`
  - `ce-executor:{plan_name}:step-02:{slug}`
- 每个 task 描述必须包含：unit 的 Goal、Files、Approach、Execution note、Verification 标准
- **只创建当前步骤的 tasks**，不提前创建未来步骤的 tasks
- **Trivial 模式**：只创建一个 summary task，key 为 `ce-executor:{plan_name}:trivial`，描述包含完整改动意图

**6. 环境检查**

- 检查当前分支：`git branch --show-current`
- 如果不在 feature branch 上，在 `context.md` 中记录建议的分支名（基于 plan 标题）
- 不主动创建分支（Executor 负责）

**7. 事件发布**

- 发布 `work.ready`，payload 包含 `plan_path`, `plan_name`, `complexity`
- 停止，不继续执行其他工作

**Constraints**：
- 必须 NOT 实现代码
- 必须 NOT 创建未来步骤的 tasks（trivial 模式除外）
- 必须 NOT 修改 plan 文件本身
- 必须在发布 `work.ready` 前完成所有文件创建
- **必须在 plan 不存在/不可读/无 Implementation Units 时发布 `work.failed`，而不是卡住或继续**

---

### Executor Hat 详细设计

**触发**：`work.ready`, `queue.advance`, `work.retry`
**发布**：`work.done`, `work.failed`, `queue.advance`
**默认发布**：`work.done`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## EXECUTOR MODE — Plan-Driven Task Execution

You own implementation. Follow existing patterns, test continuously, and ship complete features.
```

**2. 读取状态**

每次激活时按顺序读取：
1. 从事件 payload 获取 `task_id`, `task_key`, `plan_path`, `plan_name`, `complexity`
2. 读取 runtime task：`ralph tools task show <task_id> --format json`
3. 读取 `context.md`（执行上下文）
4. 读取 `plan.md`（编号步骤计划）
5. 读取 `progress.md`（当前进度）
6. 如有需要，搜索 memories 获取相关模式

**3. 环境设置**

- 检查当前分支
- 如果不在 feature branch 上，创建 feature branch（基于 plan 标题命名，如 `feat/plan-name`）
- 如果已在 feature branch 且分支名无意义（如 worktree 自动生成名），建议重命名
- 记录分支信息到 `context.md`

**4. 复杂度路由**

- **Trivial 模式**（`complexity: trivial`）：
  - 直接实现改动（1-2 文件）
  - 运行最基本的验证（如语法检查、相关测试）
  - 直接提交：`git add <相关文件>` + `git commit -m "fix(scope): description"`
  - 发布 `work.done`，`changed_lines: <实际改动行数>`
  - **跳过详细的 task-by-task 执行和 per-task 测试发现**
- **Small / Large 模式**：进入标准任务执行循环

**5. 任务执行循环（核心协议，Small/Large 模式）**

```
while (当前步骤有未完成的任务):
  1. 标记任务为 in-progress: ralph tools task start <task_id>
  2. 读取任务相关的文件和模式
  3. 实现代码（根据 Execution note 选择策略）
  4. 运行测试验证
  5. 评估测试覆盖
  6. 标记任务完成: ralph tools task close <task_id>
  7. 评估增量提交
```

**执行策略**（根据 Execution note）：

| Execution note | 策略 |
|---------------|------|
| test-first | RED → GREEN → REFACTOR。先写 failing test，再实现代码，最后重构。不要跳过验证 test 失败的步骤 |
| characterization-first | 先捕获现有行为（characterization test），确认通过后再修改 |
| 无 | pragmatic 执行，但新行为必须有测试 |

**Test Discovery**：
- 修改实现文件前，找到其对应的 test 文件（搜索 import/引用/命名模式）
- plan 指定了 test 场景的，从那里开始，再检查是否有额外覆盖

**System-Wide Test Check**（每次修改行为后必须执行）：

| 问题 | 检查内容 |
|------|---------|
| What fires when this runs? | 回调、中间件、观察者、事件处理器 — 追溯两层 |
| Do my tests exercise the real chain? | 至少一个集成测试用真实对象，不用 mock |
| Can failure leave orphaned state? | 失败路径是否清理状态，重试是否幂等 |
| What other interfaces expose this? | Mixin、DSL、替代入口点是否同步更新 |
| Do error strategies align across layers? | 重试中间件 + 应用回退 + 框架错误处理是否冲突 |

**6. 增量提交策略**

| 提交时机 | 不提交时机 |
|---------|-----------|
| 逻辑单元完成（model、service、component） | 大单元的小部分 |
| 测试通过 + 有意义的进展 | 测试失败 |
| 即将切换上下文（backend → frontend） | 纯脚手架，无行为 |
| 即将尝试风险/不确定的改动 | 需要 "WIP" 提交信息 |

提交流程：
1. 验证测试通过
2. `git add <相关文件>`（不要 `git add .`）
3. `git commit -m "feat(scope): description"`

**7. Simplify as You Go**

- 每完成 2-3 个相关的 Implementation Units（或每个自然的 phase boundary），审查最近改动的文件
- 寻找简化机会：合并重复模式、提取共享 helper、改进代码复用
- 不要每个 unit 后都简化 — 早期模式可能在后续 units 中有意分化
- 如果 `/simplify` 技能可用，使用它；否则自行审查

**8. 步骤推进**

- 当前步骤的所有 tasks 完成后，更新 `progress.md`：移动步骤到 Completed Steps
- 如果 plan.md 还有后续步骤，创建下一步的 runtime tasks，发布 `queue.advance`
- 如果所有步骤完成，运行完整测试套件 + build/lint/typecheck
- 全部通过后，计算总改动行数：`git diff --stat origin/$(git branch --show-current) | tail -1`
- 发布 `work.done`，payload 携带 `task_id`, `task_key`, `commit_count`, `changed_lines`

**9. 失败处理**

- 如果命令失败、依赖缺失或被阻塞：
  - 记录 `fix` memory：`ralph tools memory add`
  - 如果当前迭代无法解决，发布 `work.failed` 并携带 `task_id`, `task_key`, `reason`

**Constraints**：
- 一次只实现一个 runtime task（trivial 模式除外）
- 必须从当前 payload 的任务开始，不是 `progress.md` 中的下一个
- 测试必须在实现之前写（test-first 模式）
- 不能添加任务描述之外的功能
- 代码必须放在 repo 中，文档放在共享目录中
- 不能在此 preset 中使用 `[Tool] Agent` 或任何并行 subagent 工具

**Confidence-Based Decision Protocol**：
- 遇到歧义或需要决策时，confidence score 0-100
- >80：自主执行
- 50-80：执行 + 记录到 `decisions.md`
- <50：选择安全默认值 + 记录到 `decisions.md`
- `decisions.md` 格式：ID（DEC-NNN）、confidence score、alternatives、reasoning、reversibility

---

### Reviewer Hat 详细设计（已迁移至 Wave Review 架构）

> ⚠️ **本节为单 hat Reviewer 的原始设计，已被 Phase 5 的 Wave Review 架构替代。**
> 实际实现时，使用 `review-coordinator` → `dimension-reviewer`（wave） → `review-synthesizer`（aggregate） 三个 hats 替代本节内容。
> 详见「Phase 5: Wave Review 架构升级」。

**触发**：`work.done`, `fix.applied`
**发布**：`review.passed`, `review.failed`, `review.complete`
**默认发布**：`review.failed`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## REVIEWER MODE — Adversarial Code Review

You are not the builder. That separation matters.
Your job is to look at all changes with fresh eyes and try to find what the Executor missed.
Be skeptical, concrete, and adversarial.
```

**2. Scope 检测**

- 检测 diff base：默认 `origin/main`，回退 `origin/master`，再回退 `main`/`master` 本地分支
- **Review base 无法解析的处理**：
  - 尝试 `git merge-base HEAD origin/main`
  - 如果失败，尝试 `git merge-base HEAD origin/master`
  - 如果失败，尝试 `git merge-base HEAD main`
  - 如果全部失败，使用 `git log --oneline -20` 提取最近 commit 作为 intent context，diff 范围使用 `HEAD~1`（如果只有1个 commit）或提示用户
  - **Never fall back to `git diff HEAD`** — 这只会显示 uncommitted changes，遗漏所有 committed work
- 计算 merge-base：`git merge-base HEAD <base>`
- 生成 diff：`git diff -U10 <base>`
- 生成文件列表：`git diff --name-only <base>`
- 检查 untracked 文件：`git ls-files --others --exclude-standard`
- 如果 untracked 文件非空，记录但不阻塞 review。在 findings.md 的 Coverage 中注明
- **Empty Diff 处理**：如果 diff 为空（无改动），发布 `review.passed`，`findings_count: 0`，在 `findings.md` 中记录 "No changes detected — work may have been completed in a prior session"

**3. Intent Discovery**

- 从 branch name、commit messages（`git log --oneline ${BASE}..HEAD`）、plan 内容中提取 intent summary（2-3 句话）
- 如果 intent 不明确（如分支名是 generic 的 `fix-bug`），阅读 plan.md 的原始请求摘要来补充

**4. Plan Requirements Verification**

- 读取 `plan.md` 和 `context.md` 中的 R-IDs / Implementation Units
- 对每个 requirement / unit，检查 diff 中是否有对应的实现
- 在 `findings.md` 的 `## Requirements Completeness` 中记录：
  - `met`：diff 中明确有对应实现
  - `not addressed`：plan 要求了但 diff 中没有
  - `partially addressed`：有部分实现但不完整
- **未满足的需求**：标记为 P1 finding，`autofix_class: manual`，`owner: downstream-resolver`

**5. Learnings Research**

- 在发布 findings 前，搜索 `docs/solutions/` 目录中与本次改动模块/模式相关的历史问题
- 使用 `glob` 或 `grep` 查找关键词（从 changed files 和 plan 目标中提取）
- 如果找到相关 solutions，在 findings.md 的 `## Learnings & Past Solutions` 中引用
- 如果 past solution 指出的问题在本次 diff 中仍然存在，提升为 finding

**6. Agent-Native Gaps 检查**

- 检查新功能是否 agent-accessible：
  - 是否有 CLI 入口或 Agent 可调用的接口？
  - 新 API endpoint 是否有对应的文档或 discoverability？
  - 新配置项是否在 agent 的上下文范围内？
- 如果存在 gap，在 findings.md 的 `## Agent-Native Gaps` 中记录（`autofix_class: advisory`）

**7. Severity Scale**

所有 findings 使用 P0-P3 分级：

| Level | 含义 | 行动 |
|-------|------|------|
| **P0** | 关键故障、可利用漏洞、数据丢失/损坏 | 必须修复才能 merge |
| **P1** | 正常使用中很可能触发的高影响缺陷 | 应该修复 |
| **P2** | 有意义的 downside（边界情况、性能回归、可维护性陷阱） | 如果修复简单则修复 |
| **P3** | 低影响、窄范围、小改进 | 用户自行决定 |

**8. Confidence Scale**

Confidence 使用 5 个离散锚点：

| Anchor | 含义 | 行为 |
|--------|------|------|
| **100** | 可从代码本身验证（编译错误、类型不匹配、明确逻辑 bug、可引用的标准违规） | 必须报告 |
| **75** | 高度确信，会影响用户或正常运行时的行为 | 必须报告 |
| **50** | 已验证为真问题，但可能是 nitpick、窄边界情况、影响很小 | P0 时报告；其他情况仅在合成后路由到 advisory/soft buckets 时报告 |
| **25** | 推测性；无法从 diff 和 surrounding code 验证 | **不报告** |
| **0** | 假阳性或 pre-existing 问题，非本次引入 | **不报告** |

**Confidence Gate**：低于 anchor 75 的 findings 被抑制，不进入 primary findings。例外：P0 findings 在 anchor 50+ 时保留（关键但不确定的问题不能静默丢弃）。

**9. Autofix Classification**

每个 finding 必须分类为以下之一：

| `autofix_class` | 含义 | `owner` | 路由 |
|----------------|------|---------|------|
| **safe_auto** | 本地、确定性的修复，适合自动应用 | `review-fixer` | Fixer Hat |
| **gated_auto** | 有具体修复方案，但改变行为/契约/权限边界 | `downstream-resolver` | Shipper 记录到 PR / Reporter 报告 |
| **manual** | 需要跨团队输入或业务规则上下文 | `downstream-resolver` | Shipper 记录到 PR / Reporter 报告 |
| **advisory** | 仅报告，如学习记录、发布注意事项 | `human`/`release` | Reporter 报告 |

**Routing 规则**：
- 合成拥有最终路由权
- 意见不一致时选择更保守的路由
- `safe_auto → review-fixer` 是唯一自动进入 Fixer 队列的类别
- `requires_verification: true` 表示修复后需要针对性测试或 follow-up review

**10. Review Checklist**

对每个变更进行以下检查：

**A. 需求保真度**
- 代码是否满足 plan 中对应的 Implementation Unit 描述？
- 是否悄悄缩小了范围或跳过了边界情况？
- 是否实现了 plan 中 `Deferred to Implementation` 的问题？
- **Requirements Completeness**：每个 R-ID 是否被满足？

**B. 逻辑正确性**
- 是否有逻辑错误、边界情况 bug、状态错误？
- 错误传播是否正确？
- 是否有 race condition、cascade failure？

**C. 测试覆盖**
- 新行为是否有测试？
- 测试断言是否有效（不是 tautology）？
- 是否有 brittle test（过度 mock、硬编码值）？

**D. 代码质量**
- 是否违反 YAGNI（过度工程）？
- 是否违反 KISS（不必要的复杂度）？
- 代码是否与项目模式一致？
- 是否有死代码、重复代码？

**E. 安全审查**（如 diff 触及 auth、public endpoint、用户输入、权限）
- 输入是否验证？
- 权限检查是否到位？
- 是否有 injection 风险？

**F. 性能审查**（如 diff 触及 DB 查询、数据转换、缓存、异步）
- 是否有 N+1 查询？
- 大数据集处理是否高效？

**G. 项目标准合规**
- 是否符合 `CLAUDE.md` / `AGENTS.md` 中的约定？
- frontmatter、references、命名、跨平台可移植性是否正确？

**H. Agent-Native 可访问性**
- 新功能是否有 agent 可发现的入口？
- CLI/Agent 接口是否完整？

**11. Protected Artifacts**

以下路径是保护目录，**不得**标记为删除、移除或 gitignore：
- `docs/brainstorms/*`
- `docs/plans/*.md`
- `docs/solutions/*.md`

如果 reviewer 建议清理这些目录中的文件，**丢弃该 finding**。

**12. Runtime Verification**

- 如果变更涉及可运行的代码，使用最强的 harness 验证：
  - Browser/UI：Playwright 或等价工具
  - Terminal/TUI：tmux 或等价工具
  - API/CLI：运行真实命令
- 至少尝试一个对抗性或失败路径场景
- 记录命令、动作和观察结果到 `findings.md`

**13. Quality Gates（输出前强制自检）**

在发布 findings 前，逐条验证：

1. **Every finding is actionable.** 如果 finding 说 "consider"、"might want to"、"could be improved" 但没有具体修复方案，重写它。
2. **No false positives from skimming.** 验证 surrounding code 确实被读了。"bug" 是否在同一个函数的其他地方已处理？"unused import" 是否用于 type annotation？"missing null check" 是否由调用方保证？
3. **Severity is calibrated.** Style nit 绝不能是 P0。SQL injection 绝不能是 P3。
4. **Line numbers are accurate.** 验证每个引用的行号与文件内容匹配。指向错误行号的 finding 比没有 finding 更糟。
5. **Protected artifacts are respected.** 丢弃任何建议删除或 gitignore `docs/brainstorms/`、`docs/plans/`、`docs/solutions/` 的 finding。
6. **Findings don't duplicate linter output.** 不标记 linter/formatter 能捕获的问题（缺少分号、缩进错误）。聚焦语义问题。

**14. 决策逻辑**

- 如果没有 findings → 发布 `review.passed`，payload 携带 `findings_count: 0`, `changed_lines`
- 如果有 findings：
  - 从 `fix-log.md` 读取当前 `fix_round`（如不存在则为 0）
  - 提取所有 `safe_auto` findings（排除 `pre_existing: true`）
  - 如果 `safe_auto` 数量 > 0 且 `fix_round < 3`：
    - 发布 `review.failed`，payload 携带 `fix_round`, `safe_auto_count`, `gated_manual_count`, `findings_summary`
  - 如果 `safe_auto` 数量 == 0 或 `fix_round >= 3`：
    - 发布 `review.complete`，payload 携带 `fix_round`, `verdict`, `residual_findings_count`, `findings_summary`
    - `verdict` 取值：
      - `pass`：0 findings
      - `pass_with_residuals`：有 gated/manual/advisory 但无阻塞性问题
      - `fail`：有 P0 未解决，或有 safe_auto 但 fix 轮次已耗尽

**15. Findings 输出格式**

Reviewer 将 findings 写入 `.agents/scratchpad/ce-executor/{plan_name}/findings.md`：

```markdown
# Code Review Findings

## Scope
Base: <base-branch>
Files: <changed files>
Intent: <2-3 sentence intent summary>

## Requirements Completeness
| R-ID | Status | Notes |
|------|--------|-------|
| R1 | met | — |
| R2 | not addressed | Missing validation logic |

## Findings

### P0 — Critical
| # | File | Line | Issue | autofix_class | Owner | Confidence | suggested_fix |
|---|------|------|-------|--------------|-------|------------|---------------|
| 1 | src/foo.rs | 42 | Missing null check | safe_auto | review-fixer | 100 | Add `if x.is_none() { return Err(...) }` |

### P1 — High
| # | File | Line | Issue | autofix_class | Owner | Confidence | suggested_fix |
|---|------|------|-------|--------------|-------|------------|---------------|
| 2 | src/bar.rs | 15 | Race condition in cache update | gated_auto | downstream-resolver | 75 | Add mutex around cache.write() |

### P2 — Moderate
...

### P3 — Low
...

## Pre-existing Issues
| # | File | Line | Issue | Reviewer |
|---|------|------|-------|----------|
| 1 | src/old.rs | 10 | Broad rescue masking error | correctness |

## Testing Gaps
- No test for concurrent export requests
- Missing error-path coverage for CSV serialization failure

## Residual Risks
- No rate limiting on export endpoint
- Memory usage unbounded for large accounts

## Learnings & Past Solutions
- [Known Pattern] `docs/solutions/export-pagination.md` — previous export pagination fix applies

## Agent-Native Gaps
- New export endpoint has no CLI/agent equivalent

## Coverage
- Suppressed: 2 findings below anchor 75 (1 at anchor 50, 1 at anchor 25)
- Mode-aware demotion: 1 testing/maintainability P3 advisory suppressed
- Untracked files excluded: <file1>, <file2>
```

**每个 finding 必须包含的字段**（在表格中体现）：
- `title`（Issue 列）
- `severity`（P0-P3 分组）
- `file`（File 列）
- `line`（Line 列）
- `autofix_class`（autofix_class 列）
- `owner`（Owner 列）
- `confidence`（Confidence 列）
- `suggested_fix`（suggested_fix 列，safe_auto 和 gated_auto 必须有具体修复方案）
- `why_it_matters`（在 Issue 描述中体现影响，不是"什么问题"而是"什么会坏"）
- `evidence`（在 Issue 描述中引用代码片段或行引用）
- `pre_existing`（Pre-existing Issues 部分单独列出，不从 primary findings 中计数）
- `requires_verification`（在 suggested_fix 后标注 `[needs-verification]`）

**Constraints**：
- 必须 NOT 假设 Executor 已经检查了明显的东西
- 必须 NOT 用 "minor issues to fix later" 批准
- 必须 NOT 重写整个实现，除非当前方法根本错误
- 必须 NOT 在此 preset 中使用 `[Tool] Agent` 或任何并行 subagent 工具
- 必须尝试破坏增量，不只是确认 happy path
- 必须遵守 Quality Gates 的 6 条输出前自检
- 必须尊重 Protected Artifacts
- Pre-existing issues 必须分离到独立部分，**不**进入 Fixer 队列

---

### Fixer Hat 详细设计

**触发**：`review.failed`
**发布**：`fix.applied`, `fix.exhausted`
**默认发布**：`fix.exhausted`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## FIXER MODE — Safe Auto-Fix Application

You apply only safe_auto findings. You verify every fix does not break existing functionality.
```

**2. 轮次管理**

- 从 `review.failed` payload 提取 `fix_round`（如不存在则为 0）
- 从 `fix-log.md` 读取当前 `fix_round`（文件中的值优先于 payload，因为 payload 可能在 loop 重启后丢失）
- 取两者较大值，递增到 `fix_round + 1`
- 如果 `fix_round + 1 > 3`：
  - 发布 `fix.exhausted`，payload 携带 `fix_round: 3`, `residual_findings`（全部 findings 清单）
  - 停止

**3. 读取 Findings**

- 读取 `.agents/scratchpad/ce-executor/{plan_name}/findings.md`
- **只处理 `autofix_class: safe_auto` 的 findings**
- **必须排除 `pre_existing: true` 的 findings**（无论 autofix_class 是什么）
- `gated_auto`, `manual`, `advisory` **不处理**

**4. 修复应用协议**

对每个 safe_auto finding：

1. **证据匹配检查**：验证 cited 代码仍然与 finding 描述匹配（至少一个标识符或特征 token 仍然存在，行没有被删除）
2. **应用修复**：按 finding 的 `suggested_fix` 修改代码
3. **验证修复**：
   - 运行与修改文件相关的测试
   - 运行 broader build/lint/typecheck（如果修改跨子系统边界）
   - **如果 finding 有 `requires_verification: true`**：运行针对性验证（聚焦测试或操作检查）
   - 如果测试失败 → 回滚修改，标记为 `failed`，记录原因
4. **记录**：将修复结果写入 `fix-log.md`

**5. 修复后处理**

- 所有 safe_auto 修复完成后：
  - 运行完整测试套件
  - 如果全部通过 → 发布 `fix.applied`，payload 携带 `fix_round`, `applied_count`, `failed_count`
  - 如果有测试失败 → 回滚**所有**修改，发布 `fix.exhausted`，payload 携带 `fix_round`, `reason: "post-fix verification failed"`, `residual_findings`

**6. 回滚策略**

- 在开始修复前，创建临时保存点：
  ```bash
  git stash push -m "ce-executor-fix-round-N-$(date +%s)" --include-untracked
  ```
- 如果任何修复失败或验证不通过：
  ```bash
  git stash pop  # 或 git checkout -- . && git clean -fd
  ```
- **不要依赖手动逐文件回滚** — 使用 git stash 做原子性恢复

**fix-log.md 格式**：

```markdown
# Fix Log
current_fix_round: 2

## Round 1

### Applied
- #1 src/foo.rs:42 — Missing null check → added guard

### Failed
- #3 src/bar.rs:20 — Line moved, evidence no longer matches

### Verification
- Tests passed: src/foo_test.rs
- Build: cargo build ✓
- Requires verification: payment_spec (ran and passed)

## Round 2
...
```

- `current_fix_round` 必须保持最新值，作为持久化状态
- 每轮修复后更新此文件

**Constraints**：
- 必须 NOT 修改 `gated_auto` 或 `manual` findings
- 必须 NOT 修改 `pre_existing` findings
- 必须 NOT 超过 3 轮
- 必须在发布 `fix.applied` 前验证测试通过
- 必须处理 `requires_verification: true` 的 findings
- 如果修复导致测试失败，**必须回滚所有修改**（使用 git stash），而不是继续

---

### Shipper Hat 详细设计

**触发**：`review.passed`, `review.complete`, `fix.exhausted`
**发布**：`REVIEW_COMPLETE`
**默认发布**：`REVIEW_COMPLETE`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## SHIPPER MODE — Final Validation, Plan Completion, and Delivery

You are the final gate before delivery.
You validate the complete tree, update the plan status, and prepare the final commit.
Do NOT create pull requests — the user handles PR creation manually.
```

**2. 读取状态**

- 从事件 payload 读取：`verdict`, `final_findings_count`, `fix_round`
- 读取 `context.md`：plan 目标、分支信息、复杂度
- 读取 `progress.md`：执行内容摘要
- 读取 `findings.md`：最终 findings 清单、residual findings
- 读取 `fix-log.md`：修复记录

**3. Final Validation Checklist**

在 shipping 前必须完成：

1. **Run full test suite** — 使用项目测试命令（`cargo test`, `npm test`, `pytest` 等）
2. **Run build / lint / typecheck** — 使用项目最强的验证命令
3. **Verify plan requirements** — 对照 `plan.md` 和 `context.md` 中的 R-IDs，确认每个 requirement 都已满足
4. **Verify deferred questions** — 如果 plan 有 `Deferred to Implementation`，确认它们已在执行中解决
5. **Verify no regressions** — 运行与改动区域相关的 broader 测试
6. **Real harness pass**（如适用）— Playwright / tmux / real CLI commands

任何验证失败 → 发布 `REVIEW_COMPLETE`，但 payload 中 `pass_or_fail: "fail"`，`reason: "final validation failed: <detail>"`。不要尝试修复 — 修复是 Executor/Fixer 的职责。

**4. Simplify Check**

- 如果 `changed_lines >= 30` 且不是纯机械改动（格式化、依赖升级、生成产物）：
  - 审查最近改动的文件，寻找简化机会：合并重复模式、提取共享 helper、删除死代码
  - 如果 `/simplify` 技能可用，使用它
  - 简化后重新运行 Final Validation
- 如果 `< 30` 行或纯机械改动，跳过

**5. Update Plan Status**

- 如果 plan 文件有 YAML frontmatter（`.md`）：编辑 `status: active → status: completed`
- 如果 plan 是 HTML（`.html`）：编辑 `<span class="status">active</span> → <span class="status">completed</span>`
- 如果没有 status 字段，跳过
- 记录到 `shipping.md`

**6. Prepare Operational Validation Plan**

- 在 `shipping.md` 中添加 `## Operational Validation Plan`：
  - 如果有生产/runtime 影响：
    - Log queries / search terms 监控
    - Metrics / dashboards 关注
    - Expected healthy signals
    - Failure signals 和 rollback/mitigation trigger
    - Validation window 和 owner
  - 如果无生产/runtime 影响：记录 `No additional operational monitoring required` + 一行原因

**7. Commit**

- 检查是否有未提交的改动：`git status --porcelain`
- 如果有未提交的改动：
  - `git add <相关文件>`（不要 `git add .`）
  - `git commit -m "feat(scope): <plan summary>"`
- **Do NOT push to origin** — the user manages push and PR creation manually.
- 记录最终 commit hash 到 `shipping.md`

**8. 事件发布**

- 所有 shipping 步骤完成后，发布 `REVIEW_COMPLETE`，payload 携带：
  - `verdict`：`pass` | `pass_with_residuals` | `fail`
  - `final_findings_count`：最终 findings 数量（不含 pre_existing）
  - `pass_or_fail`：`pass`（无 P0 且 final validation 通过）| `fail`（有 P0 或 final validation 失败）
  - `residual_findings_summary`：gated/manual/advisory 的简要列表

**Constraints**：
- 必须 NOT 修改代码（simplify 除外，且 simplify 后必须 re-validate）
- 必须 NOT 在 final validation 失败时静默通过
- 必须记录所有 residual findings 到 `shipping.md`
- 必须更新 plan status（如果存在）
- 必须 NOT 创建 PR 或 push 到 origin — 用户自行管理 PR 创建

---

### Reporter Hat 详细设计

**触发**：`REVIEW_COMPLETE`
**发布**：`report.done`, `LOOP_COMPLETE`
**默认发布**：`report.done`

#### Instructions 必须包含的内容

**1. 启动协议**

```
## REPORTER MODE — Manager-Facing Completion Report

Generate a concise, human-readable report for a Manager audience.
Speak plainly. Lead with conclusions. Support with evidence.
```

**2. 读取状态**

- 从 `REVIEW_COMPLETE` payload 读取：`verdict`, `final_findings_count`, `pass_or_fail`, `pr_url`, `residual_findings_summary`
- 读取 `context.md`：plan 目标摘要、复杂度
- 读取 `progress.md`：执行内容摘要（执行了多少 tasks、多少轮 review/fix）
- 读取 `findings.md`：最终 findings 清单
- 读取 `fix-log.md`：修复记录
- 读取 `shipping.md`：shipping 结果、PR 信息、Operational Validation Plan

**3. 报告路径**

- 基础路径：`docs/report/`
- 文件名：`YYYY-MM-DD-ce-executor-{plan_name}-report.md`
- **路径冲突处理**：如果同一天同一 plan 运行多次，追加 `-HHMM` 时间戳：`YYYY-MM-DD-ce-executor-{plan_name}-HHMM-report.md`
- 自动创建 `docs/report/` 目录

**4. 报告结构**

输出到 `docs/report/YYYY-MM-DD-ce-executor-{plan_name}[-HHMM]-report.md`：

```markdown
# CE-Executor Report — YYYY-MM-DD

## 1. 一句话结论
本次 ce-executor 的结果是：[全部通过 / 有 safe_auto 问题已修复 / 有 gated/manual 问题待处理 / Final Validation 失败]。

## 2. 本次目标
<从 plan 提取的目标摘要，1-2 句话>

## 3. 本次执行内容
- 读取 plan：`<plan 文件名>`
- 执行了 N 个 Implementation Units
- Code review 发现 N 个问题
- 自动修复了 N 轮（Round 1/2/3）
- Final Validation：[通过 / 失败]
- 最终 commit hash：<hash>

## 4. 当前结果
- Work 完成：是/否
- Review 通过：是/否
- Final Validation 通过：是/否
- 剩余问题：N 个（P0: x, P1: y, P2: z, P3: w）

## 5. 关键证据
<最重要的 1-3 个 finding，包含 file:line 和一句话描述>

## 6. 问题影响
<剩余问题是否阻塞 merge，风险等级>

## 7. 当前判断
- 已确认事实：...
- 初步判断：...
- 待验证假设：...

## 8. 下一步计划
<如果有剩余问题，给出具体可执行的下一步>

## 9. 需要 Manager 关注/决策的点
<是否需要人工介入、是否接受当前状态>

## 10. Operational Validation Plan
<从 shipping.md 复制>

---

## Appendix: Full Findings
<details>
<summary>Click to expand</summary>
<完整的 findings 列表>
</details>

## Appendix: Fix Log
<details>
<summary>Click to expand</summary>
<完整的 fix-log>
</details>

## Appendix: Shipping Record
<details>
<summary>Click to expand</summary>
<shipping.md 内容>
</details>
```

**5. 写作规范**

- **说人话**：避免技术黑话，用 Manager 能理解的语言
- **结论先行**：每节第一句给出结论，后面给证据
- **区分事实、判断和假设**：明确标注哪些是已确认的，哪些是推断的
- **控制长度**：正文控制在一屏到两屏，大量 log/diff 放到附录
- **不同 verdict 的语调**：
  - `pass`：简洁确认，列出完成的工作
  - `pass_with_residuals`：说明修复了什么，验证结果，列出 residual 问题
  - `fail`：清晰列出失败原因、影响、建议行动

**6. 事件发布**

- 报告写入完成后，发布 `report.done`，payload 携带 `report_path`
- 然后发布 `LOOP_COMPLETE`，loop 终止

**Constraints**：
- 必须 NOT 修改任何代码
- 必须 NOT 包含时间估算
- 必须区分事实和假设
- 报告路径必须可访问（repo-relative）
- **必须处理同一天多次运行的路径冲突**（加 HHMM 时间戳）

---

### 关键设计决策

1. **Wave Review 架构（方案 B）**：将原单 hat Reviewer 拆分为 `review-coordinator` → `dimension-reviewer`（wave，concurrency 9） → `review-synthesizer`（aggregate）的三段式架构。完整引入 compound-engineering 的 9 个 review dimension，通过 Ralph wave 系统并行执行。详见下方「Phase 5: Wave Review 架构升级」章节。

2. **不内嵌 subagent 调度**：compound-engineering 使用自身 subagent 模型做并行 review，Ralph 使用原生 wave 系统替代。Dimension reviewer  workers 共享同一个 hat 定义，通过 `WAVE_DIMENSION` 环境变量 + payload 条件分支切换 checklist。

2. **safe_auto 自动修复**：Fixer hat 只处理 `safe_auto` 级别问题，`gated_auto` 和 `manual` 在3轮后作为 residual work 由 Shipper 记录到 `shipping.md`，Reporter 在报告中呈现。

3. **轮次计数器持久化**：`fix_round` 通过 event payload 传递，同时 Fixer 将其持久化到 `fix-log.md` 中的 `current_fix_round` 字段。Payload 丢失时（如 loop 重启），从文件恢复。

4. **工作目录**：使用 `.agents/scratchpad/ce-executor/{plan_name}/` 存放进度和日志，内部文件结构标准化（context.md, plan.md, progress.md, findings.md, fix-log.md, shipping.md, decisions.md, logs/）。

5. **Reporter 输出目录**：`docs/report/YYYY-MM-DD-ce-executor-{plan_name}[-HHMM]-report.md`。同一天多次运行时追加 `-HHMM` 时间戳避免覆盖。

6. **中文版不内嵌**：`ce-executor-zh.yml` 只存在于 `presets/` 目录，不进入 `crates/ralph-cli/presets/` 内嵌镜像，因此不作为 builtin preset 暴露给 `ralph init --list-presets`。用户可以通过文件路径直接引用。

7. **与 code-assist 的区分**：`ce-executor` 从 plan 文件驱动（读取 Implementation Units、Execution notes），`code-assist` 从 prompt 直接驱动（自适应输入类型）。两者 Executor/Builder hat 的底层执行协议相似（TDD、增量提交、测试验证），但入口和任务管理方式不同。

8. **Test Discovery 和 System-Wide Test Check**：从 `ce-work` 对齐，确保 Executor 在执行每个 unit 时都能找到并更新对应的测试，并在修改行为时验证真实调用链。

9. **Shipping Workflow 内嵌**：Shipper hat 承担 `ce-work` Phase 3-4 的职责（Final Validation、Plan Status Update、Final Commit、Operational Validation Plan），确保执行完成后工作被真正交付，而不是停留在 `work.done`。Shipper 不创建 PR — 用户自行管理 PR 创建。

10. **Requirements Completeness**：Reviewer 在 review 时对照 plan 的 R-IDs 验证需求完整性，未满足的需求标记为 P1 finding，确保 Executor 没有遗漏 plan 中要求的功能。

11. **Learnings Research**：Reviewer 搜索 `docs/solutions/` 中的历史问题，将过去解决方案与当前 diff 关联，避免重复踩坑。

12. **Quality Gates**：Reviewer 在输出 findings 前必须经过 6 条自检，确保 findings 是可操作的、准确的、不过度重复的。

13. **Protected Artifacts**：Reviewer 不得建议删除 `docs/brainstorms/`、`docs/plans/`、`docs/solutions/` 中的文件，这些目录是受保护的知识资产。

14. **Fixer 回滚策略**：使用 `git stash` 做原子性保存点，修复失败时原子性恢复，避免手动逐文件回滚的遗漏风险。

15. **Trivial Work 快速路径**：Coordinator 评估复杂度为 trivial 时（1-2 文件，无行为改变），Executor 跳过详细的 task-by-task 执行，直接实现并提交，减少不必要的 overhead。

16. **Shipper 的 Residual Work 处理**：自动化 preset 中不使用阻塞式 Residual Work Gate。Shipper 将 residual findings（gated_auto/manual/advisory）自动记录到 `shipping.md` 的 "Known Residuals" 部分。Manager 通过 Reporter 的报告了解 residual 状态。用户手动创建 PR 时可参考 `shipping.md` 中的 residual 记录。

---

## 实现步骤

### Step 1: 创建英文版 preset YAML

文件：`presets/ce-executor.yml`

内容结构：
- 头部注释（描述、用法、架构模式）
- `event_loop` 配置
- `cli` 配置
- `core` 配置（含 guardrails）
- `hats` 定义（Coordinator, Executor, review-coordinator, dimension-reviewer, review-synthesizer, Fixer, Shipper, Reporter）
- 每个 hat 的 instructions 按本计划的详细规范编写

### Step 2: 创建中文版 preset YAML

文件：`presets/ce-executor-zh.yml`

将英文版翻译成中文，保持完全相同的架构和事件流。instructions 中的技术术语（severity、autofix_class、owner、confidence、fix_round、pre_existing、requires_verification、safe_auto、gated_auto、downstream-resolver、review-fixer 等）保留英文，确保与 compound-engineering 的术语一致。操作命令（git、ralph emit、ralph tools）保持原样。

### Step 3: 更新 Rust 代码

在 `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组中添加 `ce-executor` 一个条目（中文版不内嵌）。

更新测试：
- `test_list_presets_returns_all`：6 → 7
- `test_preset_names_returns_all_names`：6 → 7

### Step 4: 更新同步脚本

在 `scripts/sync-embedded-files.sh` 的 `MIRRORED_FILES` 数组中添加一个映射（仅英文版）：
```
"presets/ce-executor.yml:crates/ralph-cli/presets/ce-executor.yml"
```

### Step 5: 更新 Zsh 插件

在 `scripts/ralph-zsh-plugin.zsh` 的 `_RALPH_BUILTIN_HAT_VALUES` 和 `_RALPH_BUILTIN_HAT_DESCRIPTIONS` 数组中添加 `builtin:ce-executor` 一个条目（中文版不加入 builtin 补全）。

### Step 6: 更新索引文件

在 `presets/index.json` 中添加 `ce-executor` 一个条目的元数据（中文版不加入 index）：
```json
{
  "name": "ce-executor",
  "description": "Plan-driven work execution with adversarial code review, auto-fix loop, shipping workflow, and manager report",
  "category": "development"
}
```

### Step 7: 更新文档

在 `presets/README.md` 的 Supported Builtins 表格中添加 `ce-executor`。

### Step 8: 运行同步脚本

```bash
./scripts/sync-embedded-files.sh
```

验证镜像文件已同步到 `crates/ralph-cli/presets/ce-executor.yml`。

### Step 9: 运行测试

```bash
cargo test -p ralph-cli
```

验证：
- `test_list_presets_returns_all` 通过（preset 数量 6→7）
- `test_preset_names_returns_all_names` 通过
- `test_public_presets_have_completion_path` 通过
- `test_public_presets_have_required_events` 通过

### Step 10: 安装 Zsh 插件

```bash
cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
```

---

## 验收标准

- [ ] `ralph init --list-presets` 显示 `ce-executor`（中文版不显示在 builtin 列表中）
- [ ] `ralph run -H builtin:ce-executor -p "docs/plans/my-plan.md"` 可以正常启动
- [ ] `cargo test -p ralph-cli` 全部通过
- [ ] Zsh 补全 `ralph run -H builtin:ce-executor<TAB>` 正常工作
- [ ] 同步脚本 `./scripts/sync-embedded-files.sh check` 通过
- [ ] Preset YAML 解析正确，所有 hats 的 triggers/publishes 定义完整
- [ ] 每个 hat 的 instructions 包含完整的操作协议（Setup → Execute → Validate → Emit）
- [ ] 事件 payload 格式在 instructions 中明确定义
- [ ] 工作目录文件结构在 instructions 中明确定义
- [ ] Severity scale（P0-P3）、Confidence scale（0/25/50/75/100）、autofix_class（safe_auto/gated_auto/manual/advisory）、owner（review-fixer/downstream-resolver/human/release）在 dimension-reviewer 和 review-synthesizer instructions 中定义
- [ ] Fixer 的 3 轮限制、safe_auto-only、pre_existing 排除、requires_verification 处理、git stash 回滚策略在 instructions 中定义
- [ ] Shipper 的 Final Validation、Plan Status Update、Final Commit（不创建 PR）、Operational Validation Plan、Residual Work 记录在 instructions 中定义
- [ ] Reporter 的 10 节报告结构、路径冲突处理、写作规范在 instructions 中定义
- [ ] Coordinator 的 Plan 错误处理（不存在/不可读/无 Implementation Units）和复杂度评估在 instructions 中定义
- [ ] Wave Review System 的 Quality Gates（6 条）、Protected Artifacts、Learnings Research、Agent-Native Gaps、Requirements Completeness 在 review-coordinator / dimension-reviewer / review-synthesizer instructions 中定义
- [ ] review-coordinator 能正确选择 conditional dimensions（security/performance/api-contract/reliability/adversarial）
- [ ] dimension-reviewer 的 instructions 包含所有 11 个 dimension 的条件分支 checklist
- [ ] review-synthesizer 能聚合所有 dimension findings 并去重
- [ ] Executor 的 Simplify as You Go、Confidence-Based Decision Protocol、Trivial 快速路径在 instructions 中定义

---

## 风险与注意事项

1. **Preset 文件大小**：每个 hat 的 instructions 很长（预计 150-400 行），6 个 hat 加上 guardrails 会使 preset YAML 非常大（可能 2000+ 行）。需要确保 YAML 格式正确，不会在编译时超出合理大小。考虑将部分通用内容（如 Confidence Protocol）提取为引用，减少重复。

2. **与现有 preset 的冲突**：`ce-executor` 的 Executor hat 和 `code-assist` 的 Builder hat 有相似职责，但 `ce-executor` 从 plan 文件驱动，而 `code-assist` 从 prompt 直接驱动。两者可以共存，但需确保术语和模式不冲突。

3. **测试覆盖**：需要确保新增的 preset 通过所有现有的 preset 验证测试（completion path、required events 等）。新增测试用例验证 `ce-executor` 的 hat 结构（类似 `test_code_assist_uses_upstream_artifact_layout_and_builder_workflow`）。

4. **Instructions 复杂度**：`dimension-reviewer` hat 需要包含 11 个 dimension 的条件分支 checklist（每个 50-100 行），instructions 可能达到 1000+ 行。需要在 checklist 精简度和 coverage 之间平衡。缓解：每个 dimension 只保留最核心的 3-5 条检查点，通用部分（severity、confidence、output format）提取到顶部共享。

5. **Fixer 的 safe_auto 判断**：Fixer 需要足够聪明地判断哪些 finding 是 safe_auto。如果 dimension-reviewer 误判 safe_auto，或 review-synthesizer 在路由时未降级，Fixer 可能引入回归。instructions 中需要强调 Fixer 的验证义务（运行测试后才发布 `fix.applied`）和回滚义务（使用 git stash）。

6. **Shipper 不管理 PR**：Shipper 只创建 final commit，不 push 到 origin，不创建 PR。用户自行管理 PR 创建流程。这避免了 `gh` CLI 依赖和权限问题，但也意味着用户需要手动将 residual findings 等信息复制到 PR 描述中（这些信息已记录在 `shipping.md` 和 Reporter 报告中）。

7. **中文版维护成本**：`ce-executor-zh.yml` 需要与英文版保持同步。由于不内嵌，更新频率可以较低，但架构变更时必须同步更新。中文 instructions 中混用中文 prose 和英文技术术语可能导致 LLM 困惑，需要测试验证。

8. **Wave Review vs 单 hat 效果**：Wave Review 通过多个并行 worker 分别专注不同 dimension，比单 hat Reviewer 更接近 compound-engineering 的多 persona 效果。但所有 worker 共享同一个 instructions 文件，通过条件分支切换，可能在某些专业领域（如 security 的深度检查）不如完全独立的 agent 深入。需要在实际使用中验证效果。

9. **Loop 终止的可靠性**：如果 Reporter 失败（如无法写文件），`LOOP_COMPLETE` 不会发布，loop 可能运行到 `max_iterations`。这是 Ralph event loop 的固有限制，不是 preset 特有的。`max_iterations: 50` 提供了安全上限。

10. **Shipper 的 Simplify 步骤**：Shipper 中的 Simplify 在 review 之后执行，如果 simplify 引入了新问题，Shipper 不会重新 review。这是为了流程简洁做的 trade-off。如果 simplify 改动较大（>=30 行），理论上应该重新 review，但这会显著复杂化事件流。当前设计依赖 Executor 的 "Simplify as You Go" 来减少最终 simplify 的范围。

---

## Phase 5: Wave Review 架构升级（融入 compound-engineering agents）

> **前提**：前述 6-Hat 架构中的 **Reviewer Hat** 被本阶段的 Wave Review System 替代。原 Reviewer Hat 的 Scope 检测、Intent Discovery、Requirements Verification、Learnings Research、Quality Gates、Protected Artifacts、Output Format 等职责被重新分配到 `review-coordinator`、`dimension-reviewer`、`review-synthesizer` 三个 hats 中。

### 5.1 融入的 Agent 清单与文件位置

| # | Agent/Persona | 源文件（compound-engineering-plugin） | 融入方式 | 目标位置（ce-executor preset） |
|---|---------------|--------------------------------------|---------|------------------------------|
| 1 | **correctness-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/correctness-reviewer.agent.md` | 作为 `dimension-reviewer` wave worker 的 **correctness** dimension checklist | `presets/ce-executor.yml` → `hats.dimension-reviewer.instructions` 的条件分支 `WAVE_DIMENSION == "correctness"` |
| 2 | **testing-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/testing-reviewer.agent.md` | 作为 **testing** dimension checklist | 同上，条件分支 `WAVE_DIMENSION == "testing"` |
| 3 | **maintainability-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/maintainability-reviewer.agent.md` | 作为 **maintainability** dimension checklist | 同上，条件分支 `WAVE_DIMENSION == "maintainability"` |
| 4 | **project-standards-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/project-standards-reviewer.agent.md` | 作为 **standards** dimension checklist | 同上，条件分支 `WAVE_DIMENSION == "standards"` |
| 5 | **agent-native-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/agent-native-reviewer.agent.md` | 作为 **agent-native** dimension checklist | 同上，条件分支 `WAVE_DIMENSION == "agent-native"` |
| 6 | **learnings-researcher** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/learnings-researcher.agent.md` | 作为 **learnings** dimension checklist | 同上，条件分支 `WAVE_DIMENSION == "learnings"` |
| 7 | **security-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/security-reviewer.agent.md` | 作为 **security** dimension checklist（条件触发） | 同上，条件分支 `WAVE_DIMENSION == "security"` |
| 8 | **performance-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/performance-reviewer.agent.md` | 作为 **performance** dimension checklist（条件触发） | 同上，条件分支 `WAVE_DIMENSION == "performance"` |
| 9 | **api-contract-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/api-contract-reviewer.agent.md` | 作为 **api-contract** dimension checklist（条件触发） | 同上，条件分支 `WAVE_DIMENSION == "api-contract"` |
| 10 | **reliability-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/reliability-reviewer.agent.md` | 作为 **reliability** dimension checklist（条件触发） | 同上，条件分支 `WAVE_DIMENSION == "reliability"` |
| 11 | **adversarial-reviewer** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/adversarial-reviewer.agent.md` | 作为 **adversarial** dimension checklist（条件触发） | 同上，条件分支 `WAVE_DIMENSION == "adversarial"` |
| 12 | **subagent-template** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/subagent-template.md` | findings schema、confidence rubric、autofix classification rules | `presets/ce-executor.yml` → 通用 output format 规范（三个 review hats 共用） |
| 13 | **review-output-template** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/review-output-template.md` | findings 输出模板格式（pipe-delimited markdown table） | `presets/ce-executor.yml` → `review-synthesizer.instructions` 的输出格式规范 |
| 14 | **findings-schema.json** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/findings-schema.json` | 结构化 JSON schema（P0-P3、confidence、autofix_class、owner） | `presets/ce-executor.yml` → 通用 schema 引用（嵌入 instructions 或作为注释） |
| 15 | **persona-catalog** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/persona-catalog.md` | persona 分层（always-on / conditional / stack-specific）和触发条件 | `presets/ce-executor.yml` → `review-coordinator.instructions` 的 dimension 选择逻辑 |
| 16 | **ce-code-review SKILL** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/skills/ce-code-review/SKILL.md` | 6 阶段 review 流程（scope → intent → select reviewers → spawn → merge → synthesize） | 映射为 Wave Review 的事件流：`work.done` → coordinator → wave emit → dimension workers → synthesizer |
| 17 | **scope-guardian** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/scope-guardian.agent.md` | scope 边界守卫逻辑 | `presets/ce-executor.yml` → `review-coordinator.instructions` 的 scope 检测 + `dimension-reviewer` 的 `requirements` dimension |
| 18 | **coherence** | `/home/chaowen/Dev/agent_tools/compound-engineering-plugin/plugins/compound-engineering/agents/coherence.agent.md` | 跨维度一致性检查（发现冲突的 findings） | `presets/ce-executor.yml` → `review-synthesizer.instructions` 的去重/冲突解决逻辑 |

> **不融入的 agents**：`data-migration-reviewer`（过于特定，通用 preset 不需要）、`julik-frontend-races`（React 特定）、`swift-ios`（iOS 特定）、`previous-comments`（需要 PR 评论上下文，Shipper 不管理 PR）、`ce-work` 的执行协议（已在前述 Executor hat 中完整融入，不重复）。

### 5.2 Wave Review 架构设计

```
work.done ──→ review-coordinator ──→ review.wave.ready (wave emit N dimensions)
                                          ↓
                    ┌─────────────────────────────────────────────────┐
                    │  dimension-reviewer (concurrency: 9)            │
                    │  ├── worker-1: correctness                      │
                    │  ├── worker-2: testing                          │
                    │  ├── worker-3: maintainability                  │
                    │  ├── worker-4: standards                        │
                    │  ├── worker-5: requirements                     │
                    │  ├── worker-6: agent-native                     │
                    │  ├── worker-7: learnings                        │
                    │  ├── worker-8: security (conditional)           │
                    │  ├── worker-9: performance (conditional)        │
                    │  └── ... api-contract, reliability, adversarial │
                    └─────────────────────────────────────────────────┘
                                          ↓
                              review.dimension.done × N
                                          ↓
                    review-synthesizer (aggregate: wait_for_all)
                                          ↓
                    ┌─────────────┬──────────────┬─────────────────┐
                    ↓             ↓              ↓                 ↓
              review.passed  review.failed  review.complete  (residuals)
                    ↓             ↓              ↓                 ↓
                 Shipper        Fixer          Shipper          Shipper
```

#### Hat 定义更新

```yaml
hats:
  review-coordinator:
    triggers: ["work.done", "fix.applied"]
    publishes: ["review.wave.ready"]
    # concurrency: 1（默认，单 worker 做 diff 分析和 wave 调度）

  dimension-reviewer:
    triggers: ["review.wave.ready"]
    publishes: ["review.dimension.done"]
    concurrency: 9
    # 每个 worker 通过 event payload 的 dimension 字段知道自己是哪个 persona

  review-synthesizer:
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    aggregate:
      mode: wait_for_all
      timeout: 300
    # 等待所有 dimension 的 review.dimension.done 到达后合并输出
```

### 5.3 每个 Hat 的职责映射

#### review-coordinator（替代原 Reviewer 的"分析"阶段）

**职责来源**：`ce-code-review/SKILL.md` Stage 1-2（Scope Detection + Intent Discovery） + `persona-catalog.md` 的 dimension 选择逻辑 + `scope-guardian.agent.md`

**必须包含的 instructions**：

1. **Scope 检测**（与原 Reviewer 的 Scope 检测一致，但移动到 coordinator）
   - diff base 解析（`origin/main` → `origin/master` → `main` → `HEAD~1` fallback）
   - `git merge-base` + `git diff -U10`
   - untracked 文件检测
   - empty diff 处理 → 直接发布 `review.passed`

2. **Intent Discovery**
   - 从 branch name、commit messages、plan.md 提取 intent summary

3. **Dimension 选择逻辑**（来自 `persona-catalog.md`）

   **Always-on dimensions**（每个 review 都触发）：
   - `correctness`：逻辑正确性
   - `testing`：测试覆盖
   - `maintainability`：代码质量
   - `standards`：项目标准合规
   - `requirements`：需求保真度（含 R-ID 验证）
   - `agent-native`：Agent 可访问性
   - `learnings`：`docs/solutions/` 历史问题搜索

   **Conditional dimensions**（根据 diff 分析决定是否添加）：
   | 触发条件 | 添加的 dimension | 来源 agent |
   |---------|----------------|-----------|
   | diff 触及 auth/public endpoint/用户输入/权限 | `security` | `security-reviewer.agent.md` |
   | diff 触及 DB 查询/数据转换/缓存/异步 | `performance` | `performance-reviewer.agent.md` |
   | diff 触及 API 变更/接口修改/序列化 | `api-contract` | `api-contract-reviewer.agent.md` |
   | diff 触及错误处理/重试/超时/降级/队列 | `reliability` | `reliability-reviewer.agent.md` |
   | diff >= 50 lines 或触及关键路径（auth/payments/data） | `adversarial` | `adversarial-reviewer.agent.md` |

4. **Wave 发射**
   - 使用 `ralph wave emit review.wave.ready --payloads '{"dimension":"correctness",...}' '{"dimension":"testing",...}' ...`
   - 每个 payload 包含：
     - `dimension`: string（persona 名称）
     - `focus`: string（该 dimension 的 focus 描述，用于 worker 快速定位）
     - `depth`: `"quick" | "standard" | "deep"`（来自 `adversarial-reviewer.agent.md` 的 review depth 校准）
     - `diff_base`: string（ coordinator 确定的 diff base）
     - `intent_summary`: string（intent 摘要）
     - `changed_files`: string[]（变更文件列表）
   - 将 diff 内容写入共享位置：`.agents/scratchpad/ce-executor/{plan_name}/wave-diff.patch`，供所有 worker 读取

5. **停止**
   - 发射 wave 后停止，不等待结果

#### dimension-reviewer（Wave Worker，共享 Hat 定义）

**职责来源**：上述 11 个 reviewer persona agent 的 instructions

**必须包含的 instructions**：

1. **启动协议**
   ```
   ## DIMENSION REVIEWER MODE — Focused {dimension} Review

   You are a specialized code reviewer focused on ONE dimension only.
   Your dimension is: {{WAVE_DIMENSION}} (from event payload or env var)
   Do NOT review other dimensions. Depth level: {{depth}}.
   ```

2. **读取共享上下文**
   - 从 event payload 读取 `dimension`, `focus`, `depth`, `diff_base`, `intent_summary`, `changed_files`
   - 读取共享 diff：`.agents/scratchpad/ce-executor/{plan_name}/wave-diff.patch`
   - 读取 `plan.md` 和 `context.md`（用于 requirements dimension）

3. **Dimension Checklists**（条件分支，每个 dimension 对应一个 agent 的完整 checklist）

   以下为每个 dimension 的来源文件和核心检查点：

   **A. correctness**（来源：`correctness-reviewer.agent.md`）
   - 逻辑错误、边界情况 bug、状态错误
   - 错误传播正确性
   - Race condition、cascade failure
   - 假设违反检查

   **B. testing**（来源：`testing-reviewer.agent.md`）
   - 新行为是否有测试
   - 测试断言是否有效（非 tautology）
   - Brittle test（过度 mock、硬编码值）
   - 测试缺口识别

   **C. maintainability**（来源：`maintainability-reviewer.agent.md`）
   - YAGNI、KISS 违规
   - 项目模式一致性
   - 死代码、重复代码
   - 代码异味

   **D. standards**（来源：`project-standards-reviewer.agent.md`）
   - `CLAUDE.md` / `AGENTS.md` 合规
   - frontmatter、references、命名规范
   - 跨平台可移植性

   **E. requirements**（来源：`scope-guardian.agent.md` + `correctness-reviewer.agent.md` 的 requirements 部分）
   - 每个 R-ID 是否被满足
   - `Deferred to Implementation` 是否已解决
   - Scope boundaries 是否被遵守
   - 未满足的需求 → P1 finding

   **F. agent-native**（来源：`agent-native-reviewer.agent.md`）
   - 新功能是否有 CLI 入口或 Agent 可调用的接口
   - 新 API endpoint 是否有 discoverability
   - 新配置项是否在 agent 上下文范围内

   **G. learnings**（来源：`learnings-researcher.agent.md`）
   - 搜索 `docs/solutions/` 中与 changed files / plan 目标相关的历史问题
   - 使用 `glob`/`grep` 查找关键词
   - 如果 past solution 指出的问题仍存在 → 提升为 finding

   **H. security**（条件，来源：`security-reviewer.agent.md`）
   - 输入验证
   - 权限检查到位性
   - Injection 风险（SQL、command、XSS）
   - Auth/token/cookie 处理

   **I. performance**（条件，来源：`performance-reviewer.agent.md`）
   - N+1 查询
   - 大数据集处理效率
   - 缓存策略
   - 异步/并发模式

   **J. api-contract**（条件，来源：`api-contract-reviewer.agent.md`）
   - 接口变更是否破坏向后兼容
   - 序列化/反序列化一致性
   - 错误响应格式

   **K. reliability**（条件，来源：`reliability-reviewer.agent.md`）
   - 错误处理完整性
   - 重试策略合理性
   - 超时和降级机制
   - 队列/状态机一致性

   **L. adversarial**（条件，来源：`adversarial-reviewer.agent.md`）
   - 按 `depth` 级别选择检查深度：
     - `quick`（<50 lines）：假设违反检查
     - `standard`（50-199 lines）：+ 组合失败 + 滥用场景
     - `deep`（>=200 lines）：全部 4 项技术 + cascade 构造
   - 4 项技术：假设违反、组合失败、滥用场景、级联放大

4. **Output Format**
   - 每个 finding 输出为 JSON 对象（遵循 `findings-schema.json`）：
     ```json
     {
       "title": "...",
       "severity": "P0|P1|P2|P3",
       "file": "...",
       "line": 42,
       "why_it_matters": "...",
       "autofix_class": "safe_auto|gated_auto|manual|advisory",
       "owner": "review-fixer|downstream-resolver|human|release",
       "requires_verification": true|false,
       "suggested_fix": "...",
       "confidence": 0|25|50|75|100,
       "evidence": ["..."],
       "pre_existing": false,
       "dimension": "correctness"
     }
     ```
   - 所有 findings 写入 `.agents/scratchpad/ce-executor/{plan_name}/findings-{dimension}.json`

5. **事件发布**
   - 发布 `review.dimension.done`，payload 包含：
     - `dimension`: string
     - `findings_count`: int
     - `findings_file`: string（`findings-{dimension}.json` 的路径）
     - `p0_count`, `p1_count`, `p2_count`, `p3_count`
     - `safe_auto_count`, `gated_auto_count`, `manual_count`, `advisory_count`

#### review-synthesizer（Aggregate Hat）

**职责来源**：`ce-code-review/SKILL.md` Stage 5-6（Merge Findings + Synthesize） + `coherence.agent.md` + `subagent-template.md` 的 synthesis 规则 + `review-output-template.md`

**必须包含的 instructions**：

1. **启动协议**
   ```
   ## REVIEW SYNTHESIZER MODE — Merge, Deduplicate, and Route

   You receive findings from multiple review dimensions. Your job:
   1. Merge all findings into a single coherent set
   2. Deduplicate (same file:line, same root cause)
   3. Resolve conflicts (contradictory findings from different dimensions)
   4. Calibrate severity and confidence
   5. Apply Quality Gates
   6. Route findings to the right downstream hat
   ```

2. **读取输入**
   - 等待所有 `review.dimension.done` 事件（aggregate `wait_for_all` 自动处理）
   - 从每个事件的 payload 读取 `findings_file` 路径
   - 读取所有 `findings-{dimension}.json` 文件

3. **合并与去重**（来自 `coherence.agent.md`）
   - 相同 `file:line` + 相同 root cause → 合并为一个 finding，保留最具体的描述
   - 相同 `file:line` + 不同 root cause → 保留两个，标注为独立问题
   - 冲突解决：如果一个 dimension 标记为 P0，另一个标记为 P2 → 采用 P0（保守原则）

4. **Severity 校准**
   - Style nit 绝不能是 P0
   - SQL injection 绝不能是 P3
   - 与 `coherence.agent.md` 的 cross-dimension consistency 检查一致

5. **Confidence 校准**
   - 合成拥有最终路由权
   - 意见不一致时选择更保守的路由
   - Confidence Gate：低于 anchor 75 的 findings 被抑制（P0 除外）

6. **Quality Gates（6 条）**
   与原 Reviewer 的 Quality Gates 一致，但在合成阶段执行：
   1. Every finding is actionable
   2. No false positives from skimming
   3. Severity is calibrated
   4. Line numbers are accurate
   5. Protected artifacts are respected
   6. Findings don't duplicate linter output

7. **输出**
   - 将合并后的 findings 写入 `findings.md`（与原 Reviewer 的输出格式一致）
   - 发布事件（与原 Reviewer 的决策逻辑一致）：
     - 无 findings → `review.passed`
     - 有 safe_auto 且 fix_round < 3 → `review.failed`
     - 无 safe_auto 或 fix_round >= 3 → `review.complete`

### 5.4 事件流更新

| 事件 | 发布者 | 订阅者 | Payload 变更 |
|------|--------|--------|-------------|
| `review.wave.ready` | review-coordinator | dimension-reviewer | `payloads[]` 包含 dimension + focus + depth + diff 元数据 |
| `review.dimension.done` | dimension-reviewer | review-synthesizer | `dimension`, `findings_count`, `findings_file`, `p0-p3 counts`, `autofix counts` |
| `review.passed` | review-synthesizer | Shipper | 同原 Reviewer |
| `review.failed` | review-synthesizer | Fixer | 同原 Reviewer |
| `review.complete` | review-synthesizer | Shipper | 同原 Reviewer |

### 5.5 工作目录更新

新增文件：

```
.agents/scratchpad/ce-executor/{plan_name}/
├── wave-diff.patch              # review-coordinator 生成的共享 diff（新增）
├── findings-correctness.json    # correctness dimension 输出（新增）
├── findings-testing.json        # testing dimension 输出（新增）
├── findings-maintainability.json # maintainability dimension 输出（新增）
├── findings-standards.json      # standards dimension 输出（新增）
├── findings-requirements.json   # requirements dimension 输出（新增）
├── findings-agent-native.json   # agent-native dimension 输出（新增）
├── findings-learnings.json      # learnings dimension 输出（新增）
├── findings-security.json       # security dimension 输出（条件，新增）
├── findings-performance.json    # performance dimension 输出（条件，新增）
├── findings-api-contract.json   # api-contract dimension 输出（条件，新增）
├── findings-reliability.json    # reliability dimension 输出（条件，新增）
├── findings-adversarial.json    # adversarial dimension 输出（条件，新增）
└── findings.md                  # review-synthesizer 合并后的最终输出（原设计保留）
```

### 5.6 与原计划的衔接

**需要修改的原计划章节**：

1. **架构设计 → Hat 设计总览**：将 Reviewer 替换为 `review-coordinator → dimension-reviewer → review-synthesizer`
2. **架构设计 → Event Loop 配置**：`required_events` 保持 `["review.passed", "review.complete"]`，不变
3. **Reviewer Hat 详细设计**：整节替换为 5.3 中的三个 hat 定义
4. **交付物清单**：无新增文件，但 `presets/ce-executor.yml` 中的 hats 定义从 6 个变为 8 个（+ review-coordinator, + dimension-reviewer, + review-synthesizer, - 原 Reviewer）
5. **验收标准**：Reviewer 相关验收标准更新为 Wave Review System 的标准
6. **风险**：新增"Wave worker instructions 过长"的风险

### 5.7 更新后的验收标准（Wave Review 相关）

- [ ] `review-coordinator` 能正确检测 diff base、计算 merge-base、生成 diff
- [ ] `review-coordinator` 能根据 diff 内容正确选择 conditional dimensions（security/performance/api-contract/reliability/adversarial）
- [ ] `ralph wave emit review.wave.ready` 发射的事件数量等于 always-on + conditional dimensions 的总数
- [ ] `dimension-reviewer` 的 instructions 包含所有 11 个 dimension 的条件分支 checklist
- [ ] 每个 dimension worker 只输出属于自己 dimension 的 findings，不越界
- [ ] 每个 dimension 的输出遵循 `findings-schema.json` 的 JSON 结构
- [ ] `review-synthesizer` 能正确聚合所有 `review.dimension.done` 事件（`wait_for_all`）
- [ ] `review-synthesizer` 能去重相同 file:line 的 findings
- [ ] `review-synthesizer` 能正确解决 cross-dimension severity 冲突（保守原则）
- [ ] `review-synthesizer` 的 Quality Gates 在输出前执行 6 条自检
- [ ] `review-synthesizer` 的最终输出格式与原 Reviewer 的 `findings.md` 格式一致
- [ ] `fix-log.md` 的 `current_fix_round` 计数器在 wave 架构下仍然正确工作
- [ ] 条件未触发时（如 diff 不触及 DB），`performance` dimension 不被发射
- [ ] `adversarial` dimension 的 depth 级别（quick/standard/deep）根据 diff 大小正确设置

### 5.8 新增风险

11. **Wave worker instructions 过长**：`dimension-reviewer` 需要包含 11 个 dimension 的完整 checklist（每个 50-100 行），instructions 可能达到 1000+ 行。这可能导致 prompt token 消耗过高，或 LLM 在处理条件分支时混淆 dimension。缓解：每个 dimension 的 checklist 尽量精简（只保留最核心的 3-5 条检查点），通用部分（severity scale、confidence scale、output format）提取到顶部共享。

12. **Wave 事件聚合可靠性**：如果某个 dimension worker 失败（如 LLM timeout），`review-synthesizer` 的 `wait_for_all` 可能永远等不到全部事件。缓解：设置 `timeout: 300`（5 分钟），超时后 synthesizer 用已收到的 findings 继续工作，并在 `findings.md` 中标注缺失的 dimension。

13. **Dimension 选择误判**：`review-coordinator` 的 conditional dimension 选择依赖简单的关键词匹配（如 diff 触及 `auth` → security），可能漏判或误判。缓解：always-on dimensions 覆盖核心场景，conditional dimensions 作为增强。即使漏选 security，correctness + standards 也能捕获大部分问题。
