---
title: refactor(presets): 移除 builtin preset ce-executor-pipeline-loop
type: refactor
date: 2026-10-29
baseline: 0a9a913f6f9d7856475fea072d5d80a8d6bc8879
execution: code
status: READY
---

# 移除 builtin preset ce-executor-pipeline-loop

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`0a9a913f6f9d7856475fea072d5d80a8d6bc8879`（2026-10-29 复核刷新；原基线 `84d3f531` 之上共 9 个提交，全部不触碰注册面，详见下方「基线漂移复核」）。
- **调查范围**：`presets/`（manifest/index/en/schemas/monitor/scenarios/zh/templates）、`crates/ralph-cli`（`presets.rs`、`build.rs`、`commands/inspect/descriptor.rs`、`loop_runner/{sync_timeout_tests,inner_prologue,tests/legacy/diagnosis.rs}`）、`crates/ralph-core`（`preset_lint/{instructions_opac,workflow_activation,payload_consistency,finding_id}.rs`、`emit_schema_hint.rs`、tests/scenarios.rs、tests/scenarios/*.yml）、`skills/tests/test_html_deliverables.py`、`scripts/`（ralph-zsh-plugin.zsh、sync-embedded-files.sh、validate-preset-authoring.sh、validate-builtin-presets.sh、check-cli-doc-drift.sh）、`tests/a7_e2e_lint_gate_fires.sh`、文档面（README、AGENTS/CLAUDE、`.cursor/rules/multi-hat-isolation.mdc`、`docs/guide/*`、`CONCEPTS.md`、`PROMPT.pipeline.md`、`skills/*/references/patterns.md`）、`backend/ralph-monitor/src/events/progress.ts`、`ralph.ce-executor-pipeline.yml` 与 `PROMPT.ce-executor-pipeline.md`（本计划的 bootstrap 运行套件，E24）、`docs/achieved|report|handoff|plans` 历史档案。
- **已执行验证命令**：`git status` / `git log`（工作树基线）；`rg "ce-executor-pipeline-loop|ce_executor_pipeline_loop"`（files_with_matches + 分批 content）；`git grep -l`（baseline↔HEAD 精确对比：tracked 命中文件 75→78，新增即本计划与 bootstrap 两文件）；`rg -l --hidden -g '!.git'`（捕获 hidden `.cursor/` 面）；`rg --files presets/` 家族文件枚举；`rg "U7_|emit_feedback"`（lint 规则面）；`rg -l "trigger_context"`（验证 implementation-review 是除 loop 外唯一在用方）；`git check-ignore .claude .devin`（确认镜像副本不受 git 追踪）；`ls presets/scenarios/`（确认无 loop 场景）；逐文件 `git diff --quiet baseline..HEAD` 复核全部注册点与测试点（均 byte-identical）；`sed`/`read` 读取全部注册点与测试点上下文。
- **尚未执行的验证**（留给对应 Unit）：`cargo build` / `cargo nextest` 全量与 targeted 子集；`.venv/bin/python -m pytest skills/tests/`；`./scripts/validate-preset-authoring.sh`；`./scripts/validate-builtin-presets.sh`；`rg` 最终扫尾。
- **阻塞项**：无。
- **工作树注意**：当前基线工作树干净；此前基线上的未提交修改 `backend/ralph-monitor/src/server.ts`、`crates/ralph-cli/src/web.rs` 已随 `b496a761`/`0a9a913f` 入库。**执行任何 Unit 时不得触碰** `ralph.ce-executor-pipeline.yml` 与 `PROMPT.ce-executor-pipeline.md`——它们是 commit `88cecf31` 接入的、以 `builtin:ce-executor-pipeline` 执行本计划的 bootstrap 运行套件，其中唯一含 loop 字符串的是 `plan:` 字段指向本计划文件的路径（属允许残留，非 preset 引用）。
- **基线漂移复核（`84d3f531`→`0a9a913f`，对本计划的影响）**：
  - `88cecf31 chore(bootstrap)`：新增 `ralph.ce-executor-pipeline.yml` + `PROMPT.ce-executor-pipeline.md`（E24，允许残留+禁改）。
  - `0a9a913f refactor(web)`：删除整个 `backend/ralph-web-server/` 与 `--legacy-node-api`；`docs/api/rest-api.md` 同步删除；均不含 loop 引用，不影响本计划删除面（`backend/ralph-monitor/` 仍在，`progress.ts` 未变）。
  - `c8929b36 feat(skills)`：preset author/review/diagnosis skills 接入 ralph-devtools，新增 `references/commands.md` 两份——无 loop 引用；`patterns.md` 两份未变。
  - `f0f12b73 chore(deps)` / `83a36abf docs(rules)` / monitor×2 / `b496a761 fix(web)`：不改本计划任一注册点或测试点。
  - **路径勘误**：`.cursor/rules/agents.mdc` 在原基线即不存在（计划书写误）；loop 的 builtin 列表实际位于 `.cursor/rules/multi-hat-isolation.mdc:75`（E12 已更正）。`workflow_activation.rs`/`emit_schema_hint.rs`/`payload_consistency.rs`/`finding_id.rs` 位于 `crates/ralph-core`（非 `ralph-cli/loop_runner`），§7 各处路径已更正。
  - **行号漂移**：README builtin 行 `:319`→`:141`；AGENTS.md/CLAUDE.md builtin 列表 `:143`→`:163`；CONCEPTS.md `:62`→`:63`。
  - **rg 盲区**：裸 `rg` 跳过 hidden 目录 `.cursor/`（`git grep` 证实其为 tracked 命中），扫尾命令统一升级为 `rg --hidden -g '!.git'`（E25）。

## 1. 功能目标

业务目标：把 builtin preset `ce-executor-pipeline-loop`（`ce-executor-pipeline` 的环形 review/fix 版本）连同为它存在的全部专属资产与注册点一次性退役，线性 `ce-executor-pipeline` 保留为唯一的 CE 主路径。执行后 `builtin:ce-executor-pipeline-loop` 不再可解析、不可见、不可运行，仓库中不再存在指向它的活跃生产引用。

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | `builtin:ce-executor-pipeline-loop` 不再可被解析：`ralph preset builtin list` 不列出它、`get_preset("ce-executor-pipeline-loop")` 返回 `None`、`ralph run -H builtin:ce-executor-pipeline-loop` 走未知 preset 错误路径 | 现状：`presets.rs::PRESETS` 第 41-49 行登记、9 个 public builtin 之一（E3）→ 目标：条目删除，public builtin 降为 8 个 |
| R2 | 构建产物不再携带该 preset：`presets/en/ce-executor-pipeline-loop.yml`、`-preset-author-notes.md`、`presets/schemas/ce-executor-pipeline-loop.yml`、`presets/monitor/ce-executor-pipeline-loop.yml` 全部删除；`manifest.yml` embedded 列表同步移除 | 现状：4 个专属文件 + manifest 第 31 行 embedded 条目（E4-E7）→ 目标：文件与条目均不存在，`build.rs` 自然停止拷贝（无需改构建逻辑，E8） |
| R3 | `inspect run` 描述符查找不到该 preset：`descriptor.rs::BUILTIN_DESCRIPTORS` 条目删除，未知 preset 走既有 fallback | 现状：第 34-65 行含 loop 元组（E9）→ 目标：条目删除 |
| R4 | 用户可见面不再出现该 preset：`presets/index.json` 条目删除、`scripts/ralph-zsh-plugin.zsh` 的 `_RALPH_BUILTIN_HAT_VALUES`/`_RALPH_BUILTIN_HAT_DESCRIPTIONS` 双数组同步减项、活文档（README/AGENTS/CLAUDE/cursor rule/guide/CONCEPTS/prompt/patterns.md）不再列出它 | 现状：index.json 条目、zsh 数组前两项、README:141「9 Supported Builtins」计数行、AGENTS.md:163 builtin 列表、guide 表格行均引用（E10-E14）→ 目标：全部清除，残留仅限允许的历史档案区 |
| R5 | loop 专属测试资产整体移除：8 个 BDD scenario yml + `scenarios.rs` 对应测试函数、`presets.rs` 内 loop 专属测试、diagnosis.rs 豁免元组、`test_html_deliverables.py` 参数化项、`tests/a7_e2e_lint_gate_fires.sh` | 现状：场景注册、route single-consumer、fix.done reentry、U8 field docs 等测试锚定该 preset（E15-E17）→ 目标：删除而非迁移；linear preset 与通用 lint 的既有覆盖不减损 |
| R6 | `lint.preset.instructions_emit_feedback_skill_reference_missing`（U7）规则与白名单保留休眠：builtin 面不再触发，但任何同名 file preset 仍是该规则的合法 opt-in | 现状：whitelist `&["ce-executor-pipeline-loop"]` 仅在 `lint_active` 且 instructions 提到 payload 构造时生效（E18）→ 目标：规则与名单原样保留，注释更新说明 builtin 试点已退役；不改挂 `ce-executor-pipeline`（其 instructions 不含 U7 要求的引用 token，挂上会打破 builtin lint gate，E19） |

范围：上述 R1-R6 覆盖的全部注册点、文件、测试资产、活文档。

非目标：`ce-executor-pipeline` 本体与语义；`merge-loop`/`parallel-forge`/`implementation-review`/`red-team-attack` 等其它 builtin；lint/runtime 通用机制（`trigger_context`、`field_docs`、schema parity 管线、multi-hat isolation、workflow guard 等）一概保留；`presets/templates/ce-executor-pipeline/`（线性 preset 的模板，属线性资产）；`docs/achieved/`、`docs/report/`、`docs/handoff/`、`docs/plans/` 历史档案与 `presets/en/ce-executor-pipeline-preset-author-notes.md` 内的历史提及（档案不改写，见 D5）；gitignored 的 `.claude/skills`、`.devin/skills` 本地镜像（不在 git 变更面，E20）。

输入：无运行时输入；变更为静态仓库内容。输出：删除后的文件树 + 更新后的注册点 + 绿测试基线。

状态变化：`$OUT_DIR` 构建产物少 2 个文件（preset yml、monitor descriptor）；`PRESETS`/`BUILTIN_DESCRIPTORS`/`preset_names()`/index/zsh 数组均少一项；BDD 场景套件少 8 个。

错误语义：不新增错误类型；`builtin:ce-executor-pipeline-loop` 走现有 unknown-preset 错误路径（与 `builtin:ce-executor-supervisor` 等已退役名字一致）。

兼容要求：其它 builtin 的行为一字节不变；`trigger_context`、`field_docs`、schema merge、descriptor fallback、hidden preset 机制全部保留；同名 **file preset**（用户本地 `ralph.yml`/`-p` 路径）仍可存在，U7 规则对其仍生效。

性能：无运行时影响；编译产物略小。

安全/权限：无新面。

已确认假设：A1 不存在第二个 `ce-executor-pipeline-loop` 变体（`presets/zh/` 无 zh 版，`presets/scenarios/` 无 loop 场景，`presets/templates/` 无 loop 目录，E21）；A2 `build.rs`/`sync-embedded-files.sh` 为通用目录拷贝，删除源文件即停供（E8、E22）。

待验证假设：无（全部已在 Unit 内以 Red/扫尾方式处理）。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

builtin preset 注册链（每个箭头均有代码证据）：

```text
presets/en/ce-executor-pipeline-loop.yml          ── canonical SSOT
    │  build.rs 读 manifest embedded 列表 → 拷贝到 $OUT_DIR/；schema SSOT merge 进 event_policy.schemas
presets/manifest.yml (embedded 列表 SSOT)         ── E4
crates/ralph-cli/build.rs                         ── E8（通用逻辑，无 loop 硬编码，本计划不改）
crates/ralph-cli/src/presets.rs::PRESETS          ── include_str!(OUT_DIR...) + EmbeddedPreset 元组 → get_preset/preset_names
presets/index.json                                ── public 索引（与 PRESETS 由 presets.rs 测试维持一致）
scripts/ralph-zsh-plugin.zsh                      ── _RALPH_BUILTIN_HAT_VALUES/DESCRIPTIONS（与 PRESETS/index 由 presets.rs 测试维持一致）

presets/monitor/ce-executor-pipeline-loop.yml     ── sidecar SSOT
crates/ralph-cli/build.rs                         ── 整目录拷贝到 $OUT_DIR/monitor-descriptors/
crates/ralph-cli/src/commands/inspect/descriptor.rs::BUILTIN_DESCRIPTORS ── include_str! + 元组
```

移除先例：`builtin:ce-executor-supervisor` 于 plan `2026-08-09-001` 以同管线退役（manifest 墓碑注释、registry 删除、`get_preset(...).is_none()` removal-lock 测试，`presets.rs` 中对应测试即仿其形态），本计划沿用同一套移除套路。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ----------- | ---- | ---- | ---- | --- |
| E1 | `rg ce-executor-pipeline-loop` 全仓 + `git grep` baseline↔HEAD | tracked 命中 78 文件（含 hidden `.cursor/` 1 个，裸 `rg` 漏报——E25）：注册点/专属资产/测试/活文档/历史档案/gitignored 镜像；新基线较原基线净增 3 个命中即本计划文件与 bootstrap 两文件 | 定义移除爆炸半径与「活跃 vs 历史」分类 | 高 |
| E2 | `presets/en/` 目录 | `ce-executor-pipeline-loop.yml` + `-preset-author-notes.md` 两个专属文件 | U1 删除清单 | 高 |
| E3 | `presets.rs:41-49,907-922` | `PRESETS` loop 元组；`test_preset_names_returns_all_names` 断言 public=9 且含 loop | U1 删条目+改计数+改断言 | 高 |
| E4 | `presets/manifest.yml:29-33` | embedded 列表含 `ce-executor-pipeline-loop`；头部注释说明两边均以此为 SSOT 并 panic on mismatch | U1 删 manifest 条目（含惯例性墓碑注释） | 高 |
| E5 | `presets/schemas/` | `ce-executor-pipeline-loop.yml` 为 loop 专属 schema SSOT | U1 删除 | 高 |
| E6 | `presets/monitor/` | `ce-executor-pipeline-loop.yml` sidecar 存在 | U1 删除（sidecar 离 preset 无意义） | 高 |
| E7 | `presets/en/…-preset-author-notes.md` | 为 loop 专属作者的笔记文件 | U1 删除 | 高 |
| E8 | `crates/ralph-cli/build.rs:1-41` | 通用逻辑：按 manifest 拷贝 en/*.yml、merge schemas、整目录拷 monitor/；无 preset 硬编码 | 本计划不改 build.rs | 高 |
| E9 | `descriptor.rs:34-65` + `descriptor_topics_subset_of_preset_publishes` | `BUILTIN_DESCRIPTORS` 含 loop 元组；pairs 测试列表含 loop | U1 删元组+删 pairs 项 | 高 |
| E10 | `presets/index.json` | 含 loop public 条目 | U1 删除；index↔PRESETS↔zsh 一致性由 presets.rs 测试兜底 | 高 |
| E11 | `ralph-zsh-plugin.zsh` | `_RALPH_BUILTIN_HAT_VALUES`/`DESCRIPTIONS` 前两项为 loop | U1 删 value+description（保数组对齐） | 高 |
| E12 | `README.md:141`（"9 Supported Builtins" 行）、`AGENTS.md:163`、`CLAUDE.md:163`、`.cursor/rules/multi-hat-isolation.mdc:75` | builtin 计数/列表含 loop；`.cursor/rules/agents.mdc` 不存在（原引用勘误，builtin 列表在 multi-hat-isolation.mdc） | U3 更新活文档 | 高 |
| E13 | `docs/guide/html-deliverables.md:67`（全文件唯一字面命中） | 契约表含 loop 行（`report.done→LOOP_COMPLETE`）；monitor/schemas/scenarios 章节为泛指表述、无字面点名 | U3 删行；泛指章节无需改 | 高 |
| E14 | `docs/guide/payload-contracts.md:341,497-590` | `:341`「试用例」指向 `presets/schemas/ce-executor-pipeline-loop.yml`（U1 删除后成死链）；`:497-590` trigger_context 工作示例与「当前只有 loop 配置」陈述 | U3 两处均重定向到 `implementation-review`（其 schemas 含 trigger_context/known_fields，E19 关联） | 高 |
| E15 | `crates/ralph-core/tests/scenarios/*.yml` + `scenarios.rs` | 9 个 loop 相关 scenario：`ce_executor_pipeline_loop{,_executor_partial_done,_fixer_fail_stop,_fixer_partial_continue,_fix_reentry,_fix_stabilizer_reentry,_max_round_blocked,_review_artifact_blocked}` + `verdict_gate_fail_keeps_loop_open`（内容确认仅 loop 使用）+ scenarios.rs 对应注册/断言 | U2 整删 | 高 |
| E16 | `presets.rs:424-520,1841-1910` 等 | loop 专属测试：route single-consumer、`fix.done` reentry `next_review_plan`、U8 field docs、loop topology/schema/trigger-context | U1 删除；`test_ce_executor_test_stabilizer_terminal_authority` 遍历 linear+loop → 保留 linear 分支 | 高 |
| E17 | `skills/tests/test_html_deliverables.py`、`loop_runner/tests/legacy/diagnosis.rs`、`tests/a7_e2e_lint_gate_fires.sh`、`loop_runner/sync_timeout_tests.rs` | 参数化项/豁免元组/deferred no-op 脚本/U7 wiring 测试模块均锚定 loop 名 | U1/U2 删参数化项、豁免元组、脚本；sync_timeout 模块见 D3 | 高 |
| E18 | `instructions_opac.rs:112,181-203,442-452,1060-1105` | U7 gate：`lint_active && mentions_payload_construction` 才触发；whitelist 仅含 loop；测试覆盖白名单行为 | D3：规则与名单休眠保留、更新注释；lint 测试零改动 | 高 |
| E19 | `rg "trigger_context"` 全仓 | preset 级 trigger_context 仅 loop + `implementation-review`（schemas 3 处+en preset）；linear preset 无 U7 要求的引用 token | payload-contracts 改述对象选定 impl-review；不可把白名单改挂 linear | 高 |
| E20 | `git check-ignore .claude .devin` | 两目录 gitignored | 不在 git 变更面 | 高 |
| E21 | `rg --files presets/` 家族枚举 | 无 `*-zh`/`scenarios`/`templates`/`extras`/`minimal` 的 loop 变体 | 删除清单完备 | 高 |
| E22 | `scripts/sync-embedded-files.sh` | preset mirror 机制已退役（MIRRORED_FILES 不含 preset），与删除无关 | 不需改动 | 高 |
| E23 | `PROMPT.pipeline.md`、`ralph-tools-emit.md`、`CONCEPTS.md:63`、`progress.ts:7`、`loop_runner/inner_prologue.rs`、`loop_runner/sync_timeout_tests.rs`（ralph-cli）；`preset_lint/{workflow_activation,payload_consistency,finding_id}.rs`、`emit_schema_hint.rs`（ralph-core，路径勘误后） | 共享/注释级引用：多为「以 loop 为例」或兼容说明 | U3 改为 generic/linear 表述，保留通用规则语义 | 中 |
| E24 | `ralph.ce-executor-pipeline.yml`、`PROMPT.ce-executor-pipeline.md`（commit `88cecf31` 新增） | 以 `builtin:ce-executor-pipeline` 执行本计划的 bootstrap 运行套件；唯一 loop 命中是 `plan:` 字段的本计划文件路径 | 允许残留+禁改；不改语义不影响 loop 退役 | 高 |
| E25 | `rg -l` vs `rg -l --hidden -g '!.git'` vs `git grep -l` 三方对比 | 裸 `rg` 跳过 hidden `.cursor/`（tracked，含 loop 命中 1 处）；`.claude`/`.devin` gitignored 需 `-u` 才可见（本就属允许残留） | 全部扫尾/验收命令改用 `rg --hidden -g '!.git'` | 高 |

### 2.3 受影响范围

- **生产/构建文件**：`presets/en/ce-executor-pipeline-loop.yml`、`presets/en/ce-executor-pipeline-loop-preset-author-notes.md`、`presets/schemas/ce-executor-pipeline-loop.yml`、`presets/monitor/ce-executor-pipeline-loop.yml`（删）；`presets/manifest.yml`、`presets/index.json`、`crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/src/commands/inspect/descriptor.rs`、`scripts/ralph-zsh-plugin.zsh`（改）。
- **测试资产**：`crates/ralph-core/tests/scenarios.rs` + 9 个 scenario yml（删）；`skills/tests/test_html_deliverables.py`、`crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs`（改）；`tests/a7_e2e_lint_gate_fires.sh`（删）；`presets.rs` 内部 loop 专属测试块（删/缩）。
- **活文档**：`README.md`、`AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc`、`docs/guide/{html-deliverables,payload-contracts}.md`、`CONCEPTS.md`、`PROMPT.pipeline.md`、`skills/ralph-preset-author/references/patterns.md`、`skills/ralph-preset-review/references/patterns.md`、`backend/ralph-monitor/src/events/progress.ts`、上述 ralph-cli/ralph-core 源码注释。
- **明确不受影响**：`build.rs`、`sync-embedded-files.sh`、`presets/templates/`、`presets/scenarios/`、`presets/zh/`、`presets/{extras,minimal}/`、`merge-loop` 及其 hidden 机制、`trigger_context`/`field_docs`/schema-parity 通用机制、`docs/achieved|report|handoff|plans` 档案、gitignored `.claude`/`.devin` 镜像、`ralph.ce-executor-pipeline.yml` 与 `PROMPT.ce-executor-pipeline.md`（bootstrap 套件，E24）、`backend/ralph-web-server/` 与 `docs/api/rest-api.md`（新基线上游已删除，本就无 loop 引用）。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| ----------- | ---- | ---- | ---- | ---- | --------- | --- |
| D1 | 退役方式：整体删除还是隐藏 | (a) 文件+注册全删 (b) 保留文件改 hidden (c) 保留文件改 deprecation 标记 | (a) 整体删除 | 用户指令「一定要移除出去，相关内容也要移除」；先例 `ce-executor-supervisor` 整体退役（plan 2026-08-09-001） | (b)(c) 均为用户明确否定的「软移除」；hidden/deprecated 仍残留全部专属资产 | 1.00（session-settled: user-directed） |
| D2 | 移除管线入口 | (a) 删 manifest 条目 + 源文件，registry/index/zsh 同步 (b) 在 build.rs 加排除逻辑 (c) 恢复 crate 内镜像再删 | (a) | E8：build.rs 纯通用拷贝无硬编码；E22：mirror 机制已退役 | (b) 引入无谓分支；(c) 逆向复活已退役机制 | 0.95 |
| D3 | U7 emit-feedback 规则去向 | (a) 规则+白名单休眠保留，更新注释 (b) 连规则本体删除（连带 ~5 个 lint 测试+finding id+sync_timeout 模块） (c) 白名单改挂 linear | (a) | E18：gate 仅 preset-name 匹配，保留即休眠；E19：linear 缺引用 token，改挂必破 builtin lint gate；(b) 删除面与收益不成比例且丢失 file-preset opt-in 能力 | (c) 已证伪；(b) 越界删除通用机制 | 0.85（session-settled: user-approved，见 scope call-out） |
| D4 | loop 专属测试资产处置 | (a) 整删 (b) 迁移到 linear/泛化 | (a) | E15-E17：场景与断言均锚定 loop 拓扑；linear 已有独立覆盖 | (b) 迁移产出的是对不存在拓扑的死测试 | 0.90 |
| D5 | 历史档案处置 | (a) `docs/achieved|report|handoff|plans` + 兄弟 preset author-notes 历史提及原样保留 (b) 同步删改 | (a) | 档案是已完成工作证据，改写破坏可追溯性；用户 scope 确认 | (b) 为「全仓无字符串」牺牲历史证据 | 0.90 |
| D6 | monitor sidecar 处置 | (a) 与 preset 同删 + descriptor.rs 元组删除 (b) 保留 sidecar | (a) | E6/E9：sidecar 无 preset 即孤儿；descriptor fallback 已有未知 preset 路径 | (b) 残留无效资产 | 0.95 |
| D7 | `payload-contracts.md` 示例去向 | (a) 改写为 `implementation-review` 的真实 trigger_context 示例 (b) 整节删除 | (a) | E19：impl-review 是移除 loop 后唯一在用 trigger_context 的 builtin，示例仍有真实锚点 | (b) 把仍在用的通用机制文档一并抹掉 | 0.85 |
| D8 | gitignored `.claude`/`.devin` 镜像 | (a) 不在 git 变更面 (b) 同步清理 | (a) | E20 | (b) 本地文件，非本计划责任 | 0.95 |

所有执行关键决策 ≥ 0.85。`sync_timeout_tests.rs` 的 U7 wiring 测试模块在 D3 下**原样保留**（它把 preset 名作为参数传入 lint，白名单仍在 → 测试仍表达「该名字属 opt-in」的有效契约；仅注释中对该 builtin 的指代由 U3 更新为 file-preset 语义）。

---

## 4. BDD 行为规格

```gherkin
Feature: builtin preset ce-executor-pipeline-loop 退役

  Scenario: preset 注册面不再解析该名字
    Given 仓库移除了 PRESETS 中的 loop 条目
    When 调用方查询 get_preset("ce-executor-pipeline-loop") 或列出 preset_names()
    Then 返回 None / 名称列表不含该 preset
    And 其它 builtin（ce-executor-pipeline、merge-loop、parallel-forge、implementation-review…）仍可解析

  Scenario: 构建与嵌入面不再携带该 preset
    Given manifest.yml embedded 列表已移除 loop
    When 执行 cargo build -p ralph-cli
    Then 构建成功（无 include_str! 缺文件编译错误、无 manifest↔PRESETS parity panic）
    And $OUT_DIR 中不存在 loop preset 与 loop monitor descriptor

  Scenario: 用户可见面不再出现该 preset
    Given index.json 与 zsh 补全数组已移除 loop 条目
    When 校验 presets_array_matches_manifest / index↔PRESETS↔zsh 一致性测试
    Then 全部通过且数组顺序保持并行对齐
    And builtin list / zsh 补全中无 builtin:ce-executor-pipeline-loop

  Scenario: inspect run 描述符对退役 preset 走 fallback
    Given BUILTIN_DESCRIPTORS 已移除 loop 元组
    When ralph inspect run 遇到 preset 名 ce-executor-pipeline-loop
    Then 走既有 unknown-preset fallback，不 panic

  Scenario: U7 emit-feedback 规则休眠但不失效
    Given 白名单仍含 ce-executor-pipeline-loop
    When 一个 file preset 名为 ce-executor-pipeline-loop 且 instructions 提到 payload 构造且缺引用
    Then lint 仍产生 instructions_emit_feedback_skill_reference_missing finding
    And 其它任意 preset 名不触发该 finding

  Scenario: loop 专属 BDD 场景不再注册
    Given scenarios.rs 已移除 loop 场景函数
    When 执行 cargo nextest run -p ralph-core --test scenarios
    Then 场景套件通过且不再发现 loop 场景
    And linear pipeline、parallel-forge 等其余场景不受影响

  Scenario: 活跃引用扫尾为零
    Given 全部三个 Unit 完成
    When 执行 rg -n --hidden -g '!.git' "ce-executor-pipeline-loop" 全仓扫描
    Then 命中仅剩允许的历史档案（docs/achieved|report|handoff|plans、兄弟 author-notes 历史段、gitignored 镜像）、bootstrap 套件两文件的 plan 路径引用（E24）与白名单/墓碑注释处的名字本身
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| -------- | ---- | ---- | ------ | ------ | -------- |
| 注册面不再解析 | `get_preset` 返回 None、列表不含名 | 新增 `test_ce_executor_pipeline_loop_returns_unknown_after_removal`（仿 supervisor 先例） | 单元 | — | 否 |
| 构建无残引 | build 绿 + manifest↔PRESETS parity 绿 | `presets_array_matches_manifest`（authoring 脚本亦跑） | 构建/单元 | 构建脚本 panic 即失败信号 | 否 |
| index/zsh 一致 | 四组 parity 测试绿（index⊆public、index↔zsh、hidden 不在 index、数组对齐） | 既有 `test_public_preset_names_in_index_json`、`test_index_json_entries_have_zsh_completion`、`test_hidden_presets_not_in_index_json`、`test_zsh_builtin_completion_arrays_consistent` | 单元 | — | 否 |
| descriptor fallback | unknown-preset resolve 不 panic | 既有 descriptor 测试 + 移除 pairs 项 | 单元 | — | 否 |
| U7 休眠有效 | 白名单行为测试不变 | 既有 `instructions_opac.rs` 白名单测试 + `sync_timeout_tests` 模块 | 单元 | — | 否 |
| 场景移除 | scenarios 套件绿且无 loop 场景 | `cargo nextest run -p ralph-core --test scenarios` | 集成（BDD） | — | 否 |
| python 契约 | html deliverable 契约测试绿 | `.venv/bin/python -m pytest skills/tests/test_html_deliverables.py` | 单元 | — | 否 |
| 活跃引用扫尾 | rg 命中仅剩允许区 | `rg -n "ce-executor-pipeline-loop"` 人工/脚本检查 | 门禁 | — | 否 |

不需要 E2E：删除是静态内置资源的退役，全部契约均由 build + 单测 + BDD 套件 + rg 扫尾覆盖；`tests/a7_e2e_lint_gate_fires.sh` 本来就是因 compile-time embedding 被阻塞的 deferred no-op（E17），删除而非补活。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
| -------------- | -- | -------- | ---- | ---- | ------- | --- | -------- |
| R1 | 名字不再可解析 | 场景1 | removal-lock 测试 | `presets.rs` 计数/contains/lookup 测试更新 | — | 否 | E3 |
| R2 | 构建面移除 | 场景2 | build + parity | `presets_array_matches_manifest` | `validate-preset-authoring.sh` | 否 | E2,E4-E8 |
| R3 | 用户面移除 | 场景3 | 4 组 parity 测试 | index/zsh 数组断言更新 | `validate-builtin-presets.sh` | 否 | E10-E14 |
| R4 | 测试资产移除 | 场景6 | scenarios 套件 | 删除项本身即断言 | BDD 套件 | 否 | E15-E17 |
| R5 | U7 休眠保留 | 场景5 | 既有白名单测试 | 白名单行为测试 + sync_timeout 模块 | — | 否 | E18,E19 |
| R6 | descriptor fallback | 场景4 | descriptor 测试 | pairs 列表缩减 | — | 否 | E9 |
| R7 | 扫尾为零 | 场景7 | rg 门禁 | — | — | 否 | E1 |

---

## 7. 严格串行开发单元

```text
Unit 1 ──→ Unit 2 ──→ Unit 3
（每 Unit 完成全部测试、重构与回归后才进入下一 Unit）
```

### Unit 1：builtin 注册面与 preset 本体文件移除

#### 1. Unit 目标
使 `ce-executor-pipeline-loop` 不再是可构建、可解析的 builtin：删除 4 个专属文件 + manifest/index 条目 + `PRESETS`/`BUILTIN_DESCRIPTORS` 元组 + zsh 双数组项，同步处理所有因此立即变红的测试点，并新增 removal-lock 测试。

#### 2. 对应需求与 Scenario
R1、R2、R3、R6；BDD 场景 1/2/3/4；D1、D2、D6；E2-E11。

#### 3. 外部可观察结果
`ralph preset builtin list` / zsh 补全 / index.json 不再含 `builtin:ce-executor-pipeline-loop`；`cargo build` 与 parity 测试绿。

#### 4. 当前行为基线
`presets.rs:41-49` 登记 loop 元组；`test_preset_names_returns_all_names` 断言 public=9 含 loop；`get_preset` 可解析；manifest:31 embedded；index.json 有条目；zsh 数组前两项为 loop；descriptor.rs 有元组（E3,E4,E9-E11）。

#### 5. 输入与输出
输入：静态文件树编辑。输出：删除 4 文件 + 5 处注册点缩减 + `PRESETS` public 降为 8。错误：不新增；unknown-preset 复用既有路径。不变量：其它 9 个 `PRESETS` 条目（8 public+merge-loop）与各自 schema/descriptor/index/zsh 对应关系不变。

#### 6. 修改位置
- 删除：`presets/en/ce-executor-pipeline-loop.yml`、`presets/en/ce-executor-pipeline-loop-preset-author-notes.md`、`presets/schemas/ce-executor-pipeline-loop.yml`、`presets/monitor/ce-executor-pipeline-loop.yml`。
- 修改 `presets/manifest.yml`：删 embedded 条目；按 supervisor 先例在列表处留一行墓碑注释说明该 builtin 已退役（防止名称复用）。
- 修改 `presets/index.json`：删 loop 对象。
- 修改 `presets.rs`：删 `PRESETS` 元组；删 loop 专属测试（route single-consumer `test_ce_executor_pipeline_loop_routes_*`、`fix.done` reentry `next_review_plan`、U8 field docs、loop topology/schema/trigger-context 各测试）；`test_preset_names_returns_all_names` 计数 9→8 且断言不含 loop；`test_preset_names_contains_pipeline` 等断言更新为只断言 linear；`test_ce_executor_test_stabilizer_terminal_authority` 的遍历收窄为仅 linear（保留 linear 覆盖）；`descriptor_topics_subset_of_preset_publishes`（若其 pairs 在 presets.rs 侧）删 loop 对；所有遍历 `PRESETS` 的 strict-lint/stabilization-schema/handoff-owner 测试自动收窄（不改代码，验证时确认其断言为动态遍历而非硬编码 9 个名）。
- 修改 `descriptor.rs`：删 `BUILTIN_DESCRIPTORS` loop 元组 + pairs 测试中的 loop 项。
- 修改 `ralph-zsh-plugin.zsh`：删 `_RALPH_BUILTIN_HAT_VALUES` 第 2 项与对应 description（保两个数组等长、顺序不变）。
- 修改 `skills/tests/test_html_deliverables.py`：删 `PRESET_PROMPT_FILES` 中 loop 参数化项、`test_html_deliverables_contract_registered` 中 loop 断言项（它们读即将删除的文件/断言 doc 行，U1 内必须同步，否则 Red 不可恢复）。
- 不修改：`build.rs`（E8）、`sync-embedded-files.sh`（E22）、任何 ralph-core 通用机制。

#### 7. 可依赖能力
既有 unknown-preset fallback、既有 index↔PRESETS↔zsh parity 测试、`manifest↔PRESETS` parity（authoring 脚本）、`merge-loop` hidden 测试样本、`ce-executor-supervisor` removal-lock 先例。

#### 8. 禁止依赖的未来能力
不删 scenario 文件与 scenarios.rs 注册（U2）；不改活文档正文（U3）；不改 U7 规则/白名单语义与 sync_timeout 模块（D3/U3 注释除外）。

#### 9. 验收测试
- 新增 `test_ce_executor_pipeline_loop_returns_unknown_after_removal`：`get_preset("ce-executor-pipeline-loop")` 断言 `is_none()`（removal-lock，仿 supervisor 先例形态）。
- `test_preset_names_returns_all_names`：断言 8 个 public 名且不含 loop。
- index/zsh 四组 parity 测试：全绿。
- `presets_array_matches_manifest`：绿（authoring 脚本同名验证）。
- `cargo build -p ralph-cli`：绿。
- 运行命令：`cargo nextest run -p ralph-cli --bin ralph -- preset`（覆盖 presets.rs 全部测试）、`cargo nextest run -p ralph-cli --bin ralph -- descriptor`、`cargo build -p ralph-cli`、`.venv/bin/python -m pytest skills/tests/test_html_deliverables.py`。

#### 10. Acceptance Red
先写 removal-lock 测试并运行 `cargo nextest run -p ralph-cli --bin ralph -- ce_executor_pipeline_loop` → **失败**：`get_preset` 当前返回 `Some`（断言 `is_none()` 失败）——证明测试锚定了目标行为。无效 Red：编译失败、测试名拼错导致的 no-match、环境变量污染。

#### 11. 单元测试拆分
- removal-lock：输入名字符串 → `None`；边界：`builtin:` 前缀形式同样不可解析。
- names 列表：断言 `len()==8` 且 `!contains("ce-executor-pipeline-loop")` 且仍含 `ce-executor-pipeline`/`merge-loop`（merge-loop 为 hidden 不在 public 列表时按其既有语义断言）。
- zsh 数组：values/descriptions 等长、每个 public preset 恰好一次、无 loop 项。
- index：每条目有 zsh 补全、hidden 不在 index。
- manifest↔PRESETS parity：embedded 集合 == PRESETS 集合。
- descriptor：无 loop 元组后 `resolve` 对该名走 fallback 不 panic。
- python 参数化：剩余条目读取各自文件仍断言 heading/skill hook 存在。
- 不允许 Mock 的真实行为：`include_str!` 构建产物对齐（由 build 验证）。

#### 12. Red → Green → Refactor 顺序
```text
removal-lock Red（断言 is_none 失败）
→ 删除 4 文件 + manifest/index/PRESETS/descriptor/zsh 注册点
→ 修复所有因此变红的既有测试（计数、contains、loop 专属测试删除、python 参数化）
→ cargo build 绿
→ removal-lock + parity + descriptor + python 测试绿
→ Refactor：检查 PRESETS 区注释/排序、manifest 墓碑注释格式与其它墓碑一致
→ 全量本 Unit 测试复跑
```

#### 13. 最小实现范围
必须：删除 4 文件与全部注册条目；同步缩减/删除因文件消失而必然失败的既有断言与测试；新增 removal-lock；保 `PRESETS` 其余 9 项逐字节不动（除 loop 元组）。必须保持：linear preset 全部测试名与语义、hidden 机制、descriptor fallback。明确不实现：scenario 文件删除、文档正文、U7 注释更新。

#### 14. 集成验证
真实模块联合：`build.rs`（manifest→OUT_DIR）、`presets.rs`（include_str）、`descriptor.rs`、zsh 插件文本、`index.json`。命令：`cargo build -p ralph-cli` → 绿；`cargo nextest run -p ralph-cli --bin ralph -- preset` → 绿；`cargo nextest run -p ralph-cli --bin ralph -- descriptor` → 绿；`./scripts/validate-preset-authoring.sh` → 绿。预期：`$OUT_DIR` 无 loop 文件（可由 build 成功 + parity 测试间接证明）。

#### 15. 风险驱动测试
- Characterization：`test_ce_executor_test_stabilizer_terminal_authority` 收窄前确认其 linear 分支独立成立（遍历仅余 linear 仍断言同一 authority 语义）。
- Contract：manifest↔PRESETS、index↔zsh↔PRESETS 三组 parity 是本 Unit 的核心契约测试。
- Differential：不适用（删除而非替换实现）。
- 幂等/并发/Fault Injection：不适用——静态编译期内置资源，无持久化迁移、无并发状态。

#### 16. 回归范围
`cargo nextest run -p ralph-cli --bin ralph`（全 ralph bin 单测，因 PRESETS 遍历测试全部收窄）；`cargo nextest run -p ralph-cli` 其余测试目标（loop_runner 等引用 preset 名的测试）；`.venv/bin/python -m pytest skills/tests/`；`cargo build` workspace 级（其它 crate 可能引用 builtin 名——先 `rg "ce-executor-pipeline-loop" crates/` 确认生产代码仅剩本计划白名单与注释处）。理由：registry 收窄影响所有遍历型断言。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `presets/en/ce-executor-pipeline-loop.yml` | 删生产文件 | 本体 | E2 |
| `presets/en/ce-executor-pipeline-loop-preset-author-notes.md` | 删文档文件 | 专属笔记 | E7 |
| `presets/schemas/ce-executor-pipeline-loop.yml` | 删 SSOT | schema | E5 |
| `presets/monitor/ce-executor-pipeline-loop.yml` | 删 sidecar | descriptor | E6 |
| `presets/manifest.yml` | 改配置 | embedded 条目+墓碑 | E4 |
| `presets/index.json` | 改配置 | public 索引 | E10 |
| `crates/ralph-cli/src/presets.rs` | 改生产+测试 | PRESETS 元组、loop 测试、计数断言 | E3,E16 |
| `crates/ralph-cli/src/commands/inspect/descriptor.rs` | 改生产+测试 | descriptor 元组+pairs | E9 |
| `scripts/ralph-zsh-plugin.zsh` | 改脚本 | 补全数组 | E11 |
| `skills/tests/test_html_deliverables.py` | 改测试 | 参数化/断言项 | E17 |

#### 18. 完成标准
removal-lock 与全部 parity 测试绿；`cargo build` 绿；python 契约测试绿；`rg -n "ce-executor-pipeline-loop" crates/ralph-cli/src presets/ scripts/ralph-zsh-plugin.zsh skills/tests/` 仅剩墓碑注释/白名单处（scenario yml 属 U2 范围、文档属 U3 范围，此阶段允许存在）；无新增 skip/.only/削弱断言；可独立提交。

#### 19. 停止条件
`include_str!` 路径与 manifest 拷贝实际机制与 E8 不符；发现 PRESETS 之外的第二处 builtin registry；linear preset 测试被意外破坏且无法定位；需要引入未计划依赖；D3 前提（白名单保留不动）被新证据推翻。

#### 20. 风险与注意事项
- **风险**：`presets.rs` 中存在对 public 数量的其它硬编码断言（除 9→8 外）。检测：`rg "len\(\) == 9\|== 9" crates/ralph-cli` 在 U1 开始时复核。缓解：全部改为动态计数或更新数值。
- **风险**：scenario yml 若在 U1 提前删，`scenarios.rs` 注册编译即红——本 Unit **不**删 scenario（U2 负责），保持编译面稳定。
- **风险**：index.json 若被 `validate-builtin-presets.sh` 作为 builtin 列表 SSOT（它确实如此），漏删会导致该脚本尝试运行不存在的 preset → parity 测试先行兜住。

### Unit 2：loop 专属测试资产移除

#### 1. Unit 目标
删除仅为 loop 存在的 BDD 场景、场景测试函数、lint-gate 豁免元组与 deferred e2e 脚本，测试套件在无这些资产后全绿。

#### 2. 对应需求与 Scenario
R4、R5；BDD 场景 6；D3、D4；E15-E17。

#### 3. 外部可观察结果
`cargo nextest run -p ralph-core --test scenarios` 不再运行 9 个 loop 场景；`tests/a7_e2e_lint_gate_fires.sh` 不复存在；套件整体绿。

#### 4. 当前行为基线
9 个 scenario yml（含 `verdict_gate_fail_keeps_loop_open`，经内容确认仅 loop 使用）注册在 `scenarios.rs`；`diagnosis.rs` 豁免元组把 lint gate finding 对该 preset 名豁免；a7 脚本为 deferred no-op 且逻辑锚定 loop 文件；sync_timeout U7 wiring 模块传 loop 名作参数（D3 下保留模块，仅 U3 更新注释）。

#### 5. 输入与输出
输入：文件删除+测试函数删除。输出：场景套件收窄。不变量：linear pipeline、parallel-forge、其它场景与 diagnosis lint-gate 测试对其它名字的覆盖不变。

#### 6. 修改位置
- 删除：`crates/ralph-core/tests/scenarios/` 下 9 个 loop 场景 yml（E15 列名；`verdict_gate_fail_keeps_loop_open.yml` 删除前先读文件确认其 hats/topics 仅为 loop 拓扑——若发现被通用 guard 测试复用则保留并改名为通用语义，属停止条件）。
- 修改 `scenarios.rs`：删 8+1 个注册/断言测试函数。
- 修改 `crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs`：删豁免 tuple 中 loop 项。
- 删除 `tests/a7_e2e_lint_gate_fires.sh`。
- 不修改：`instructions_opac.rs` 规则与白名单（D3）、`sync_timeout_tests.rs` 测试逻辑（注释属 U3）。

#### 7. 可依赖能力
U1 已完成的注册面移除（scenario 引用 builtin 与否均不再可用）；BDD runner 动态加载场景文件。

#### 8. 禁止依赖的未来能力
不改活文档与注释（U3）；不动 `ce-executor-pipeline` 的 scenario（`presets/scenarios/ce-executor-pipeline-*.yml` 与任何 linear 场景）。

#### 9. 验收测试
- `cargo nextest run -p ralph-core --test scenarios`：绿且无 loop 场景执行记录。
- `cargo nextest run -p ralph-cli --bin ralph -- diagnosis`：绿。
- `cargo nextest run -p ralph-cli --bin ralph -- sync_timeout`：绿（模块未改，证明 D3 决策不破坏 lint wiring）。
- `rg -n "ce_executor_pipeline_loop" crates/ralph-core/tests/` → 0 命中。

#### 10. Acceptance Red
本 Unit 为纯资产移除，不新增先红测试；Red 形态 = 移除前运行 scenarios 套件观察到 loop 场景确实执行（`cargo nextest run -p ralph-core --test scenarios -- --list | rg loop` 或套件输出含场景名），移除后同一入口不再出现——「存在→消失」对照由 rg/list 输出承担。若 `verdict_gate_fail_keeps_loop_open` 的内容确认步骤发现非 loop 依赖，按停止条件处理。

#### 11. 单元测试拆分
本 Unit 不新增单元测试；删除对象即测试本身。必须确认的真实行为：每个被删 scenario 的 yml 中 hats/topics 不含被其它场景引用的共享定义（逐一 grep 场景名互引）。

#### 12. Red → Green → Refactor 顺序
```text
确认基线（scenarios 套件含 loop 场景）
→ 删 9 个 yml + scenarios.rs 对应函数 + diagnosis 豁免项 + a7 脚本
→ scenarios 套件 + diagnosis + sync_timeout 测试绿
→ Refactor：检查 scenarios.rs 顶部注释/文档串是否残留 loop 列表
→ 复跑本 Unit 全部命令
```

#### 13. 最小实现范围
必须：删 9 yml、删注册函数、删豁免 tuple、删 a7 脚本。必须保持：其余场景与 diagnosis 测试逐字节语义。明确不实现：文档更新、U7 注释更新。

#### 14. 集成验证
`cargo nextest run -p ralph-core --test scenarios` 全量；`cargo nextest run -p ralph-core`（同 crate 其余测试确认场景文件删除无连带）；`cargo nextest run -p ralph-cli --bin ralph`（diagnosis/sync_timeout/loop_runner 测试面）。

#### 15. 风险驱动测试
Characterization：删 `verdict_gate_fail_keeps_loop_open` 前先读其内容确认仅服务 loop（计划中已注明为条件步骤）。其余风险驱动类型不适用——删除静态测试资产。

#### 16. 回归范围
ralph-core 全包 nextest（场景文件可能被多个 harness 引用）；ralph-cli bin 测试（diagnosis/loop_runner）；`rg -n "verdict_gate_fail_keeps_loop_open\|ce_executor_pipeline_loop" crates/` 确认无残引。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop*.yml`（8 个） | 删测试 fixture | loop 专属 | E15 |
| `crates/ralph-core/tests/scenarios/verdict_gate_fail_keeps_loop_open.yml` | 删测试 fixture（条件确认后） | loop 专属 | E15 |
| `crates/ralph-core/tests/scenarios.rs` | 改测试注册 | 删对应函数 | E15 |
| `crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs` | 改测试 | 豁免 tuple | E17 |
| `tests/a7_e2e_lint_gate_fires.sh` | 删脚本 | deferred no-op 失效 | E17 |

#### 18. 完成标准
scenarios 套件绿且 `--list`/输出无 loop 场景；diagnosis/sync_timeout 测试绿；`rg` 上述范围 0 命中；无 skip/削弱断言；可独立提交。

#### 19. 停止条件
`verdict_gate_fail_keeps_loop_open` 或非 loop 场景被其它场景复用；scenarios.rs 中有共享 helper 仅被 loop 场景使用且删除会破坏其它场景；diagnosis 豁免元组删除导致其它名字测试变红。

#### 20. 风险与注意事项
- **风险**：scenario yml 间存在共享定义引用。检测：删除前 `rg` 场景名互引。缓解：只删 loop 专属。
- **风险**：`scenarios.rs` 顶部注释列出场景清单，删函数后注释残留 loop 名。缓解：Refactor 步检查注释。

### Unit 3：活文档与注释引用面清理

#### 1. Unit 目标
所有面向用户的活文档与源码注释不再把 `ce-executor-pipeline-loop` 当作存在的 builtin；历史档案不动。

#### 2. 对应需求与 Scenario
R3、R5、R7；BDD 场景 3/5/7；D3、D5、D7；E12-E14、E23。

#### 3. 外部可观察结果
README/AGENTS/CLAUDE/cursor rule/guide/CONCEPTS/prompt 中 builtin 列表与该 preset 无涉；`payload-contracts.md` 的 trigger_context 示例改挂 `implementation-review`；`rg` 扫尾仅剩允许区。

#### 4. 当前行为基线
README:141 计数、AGENTS.md:163/CLAUDE.md:163/multi-hat-isolation.mdc:75 builtin 列表、`html-deliverables.md:67` 契约表行、`payload-contracts.md` `:341` 试用例 + `:497-590` 示例+「只有 loop」陈述、`CONCEPTS.md:63`、`ralph-tools-emit.md:269`、`PROMPT.pipeline.md:3-4`、两份 `patterns.md`、`progress.ts:7` 注释、ralph-cli `loop_runner/{inner_prologue,sync_timeout_tests}.rs` 与 ralph-core `preset_lint/{workflow_activation,payload_consistency,finding_id,instructions_opac}.rs`、`emit_schema_hint.rs` 注释均点名该 preset（E12-E14、E18、E23）。

#### 5. 输入与输出
输入：文档/注释编辑。输出：活文档零活跃引用。不变量：`implementation-review` 的 trigger_context 字段描述以其真实 schemas/en 声明为准（不虚构字段）；`PROMPT.pipeline.md` 保留为 linear preset 的通用 prompt（不整删）。

#### 6. 修改位置
- `README.md:141`：builtin 计数与描述（按 PRESETS 实际 9 个/8 public 的事实更新，顺手修正与事实不符的计数）。
- `AGENTS.md:163`、`CLAUDE.md:163`、`.cursor/rules/multi-hat-isolation.mdc:75`：builtin 列表删 loop 项。
- `docs/guide/html-deliverables.md`：删 `:67` 契约表内 loop 行（全文件唯一字面命中，E13）。
- `docs/guide/payload-contracts.md:341`：「试用例」对 `presets/schemas/ce-executor-pipeline-loop.yml` 的指向改挂 `presets/schemas/implementation-review.yml` 中真实的 trigger_context/known_fields 字段（U1 删除 schema 后此处成死链，E14）。
- `docs/guide/payload-contracts.md:497-590`：示例与「当前只有 loop」陈述改写为 `implementation-review` 的真实 trigger_context（其 schemas/en 声明的 summary_fields/routing_hints，E19）。
- `CONCEPTS.md:63`：改为对「同一 preset 内 review/fix 环」概念的泛化表述或删去 preset 名。
- `crates/ralph-core/data/ralph-tools-emit.md:269`：non-scope 例子改只列 linear 或 generic pipeline。
- `PROMPT.pipeline.md:3-4`：表述改为仅服务 `ce-executor-pipeline`（文件不删）。
- `skills/ralph-preset-author/references/patterns.md`、`skills/ralph-preset-review/references/patterns.md`：删/改 loop 引用段。
- `backend/ralph-monitor/src/events/progress.ts`：注释中的 loop 例子改 generic/linear。
- `crates/ralph-cli/src/loop_runner/{inner_prologue,sync_timeout_tests}.rs`、`crates/ralph-core/src/emit_schema_hint.rs`、`crates/ralph-core/src/preset_lint/{workflow_activation,payload_consistency,finding_id,instructions_opac}.rs`：注释中对该 builtin 的指代更新（U7 注释说明「builtin 试点已退役，名单对同名 file preset 仍生效」）。
- 不修改：`docs/achieved|report|handoff|plans`、`presets/en/ce-executor-pipeline-preset-author-notes.md`（历史）、`.claude`/`.devin` 镜像、`ralph.ce-executor-pipeline.yml` 与 `PROMPT.ce-executor-pipeline.md`（E24，其 plan 路径引用合法残留）。

#### 7. 可依赖能力
U1/U2 已完成的移除面；`implementation-review` 真实 trigger_context 声明。

#### 8. 禁止依赖的未来能力
无后续 Unit；不得趁机重写 `PROMPT.pipeline.md` 通用规则段落或删除 `ralph-tools-emit.md` 规则本身。

#### 9. 验收测试
- `.venv/bin/python -m pytest skills/tests/test_html_deliverables.py`：绿（doc 行删除后 inventory 断言仍对齐——U1 已删 loop 断言项）。
- `rg -n --hidden -g '!.git' "ce-executor-pipeline-loop" --glob '!docs/achieved/**' --glob '!docs/report/**' --glob '!docs/handoff/**' --glob '!docs/plans/**' --glob '!presets/en/ce-executor-pipeline-preset-author-notes.md' --glob '!.claude/**' --glob '!.devin/**' --glob '!ralph.ce-executor-pipeline.yml' --glob '!PROMPT.ce-executor-pipeline.md'` → 命中仅剩白名单、墓碑注释（`--hidden` 必需：捕获 `.cursor/` 面，E25；逐条人工审阅确认均为允许区）。
- `cargo doc --no-deps`（可选）：无死链警告显著增加。
- `cargo nextest run -p ralph-cli --bin ralph -- sync_timeout` + `-- instructions_opac`/`emit_feedback`（ralph-core）：绿，证明注释改动未动逻辑。

#### 10. Acceptance Red
不新增代码 Red；Red 形态 = `rg` 扫尾当前仍命中活文档区（证明有工作），完成后归零。`html-deliverables` 的 doc↔test 一致性由 python 测试兜底。

#### 11. 单元测试拆分
不新增；`payload-contracts.md` 改写以 impl-review schemas 字段清单为准（executor 逐字段对照 `presets/schemas/implementation-review.yml` 与 `presets/en/implementation-review.yml`，不得虚构 `review_round`/`must_fix_now_count` 等 loop 专属字段名）。

#### 12. Red → Green → Refactor 顺序
```text
rg 扫尾基线（活文档区有命中）
→ 逐文件改写/删除引用（README→rules→guide×2→CONCEPTS→emit md→PROMPT→patterns×2→progress.ts→各 rs 注释）
→ pytest + sync_timeout + emit_feedback 测试绿
→ Refactor：复读每处改写，确认 linear/generic 表述仍准确（不得把 loop 语义错挂到 linear）
→ 最终 rg 扫尾归零
```

#### 13. 最小实现范围
必须：上述清单内活引用全部清除或改述；payload-contracts 示例真实。必须保持：文档结构与剩余内容语义、linear/其它 preset 的描述精度。明确不实现：历史档案改写、`.claude`/`.devin` 清理、`payload-contracts.md` 整节删除。

#### 14. 集成验证
`rg` 全仓扫尾（允许区列表见 §9 验收）；`mkdocs build` 不强求本地执行（docs.yml CI 负责，见风险）；`pytest skills/tests/` 全量。

#### 15. 风险驱动测试
不适用——文档/注释改动；doc↔test 一致性已由 inventory 测试覆盖。

#### 16. 回归范围
`pytest skills/tests/` 全量；`cargo nextest run -p ralph-cli --bin ralph`（注释改动文件的测试，防误改代码）；`cargo nextest run -p ralph-core`（instructions_opac/emit_schema_hint 注释所在 crate）；`cargo fmt --check` + `cargo clippy`（注释改动不应引入，但 rustdoc 链接类注释可能有影响——`.rs` 注释改动后跑 `cargo doc --no-deps` 抽查无 broken intra-doc link）。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `README.md`、`AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc` | 改文档 | builtin 列表/计数 | E12 |
| `docs/guide/html-deliverables.md` | 改文档 | 契约表+点名段 | E13 |
| `docs/guide/payload-contracts.md` | 改文档 | 示例重定向 impl-review | E14,D7 |
| `CONCEPTS.md`、`crates/ralph-core/data/ralph-tools-emit.md`、`PROMPT.pipeline.md` | 改文档 | 引用改述 | E23 |
| `skills/ralph-preset-{author,review}/references/patterns.md` | 改文档 | patterns 引用 | E1 |
| `backend/ralph-monitor/src/events/progress.ts` | 改注释 | 示例改述 | E23 |
| `loop_runner/{inner_prologue,sync_timeout_tests}.rs`、`ralph-core emit_schema_hint.rs` 与 `preset_lint/{workflow_activation,payload_consistency,finding_id,instructions_opac}.rs` | 改注释 | builtin 指代 | E18,E23 |

#### 18. 完成标准
pytest + 相关 rust 测试绿；`rg` 扫尾仅剩允许区；每处改写经复读确认；可独立提交。

#### 19. 停止条件
`implementation-review` 的 trigger_context 实际字段与本计划预期不符（executor 以真实 schemas 为准重写，若发现该 preset 也不在用 trigger_context 则按「删示例+改陈述」降级并记录证据）；某处注释实为行为契约文本（非示例）需保留。

#### 20. 风险与注意事项
- **风险**：把 loop 语义误改述到 linear preset（如「按 must_fix_now 收敛」是 loop 专属）。检测：逐处复读 diff。缓解：改写只删名不改规则。
- **风险**：`docs/guide/` 可能还有未列出的次级引用页。检测：Unit 内先跑 `rg -l "ce-executor-pipeline-loop" docs/guide/` 全量枚举再动。

---

## 8. Unit 串行依赖图

```text
Unit 1 ──→ Unit 2 ──→ Unit 3
```

- U1→U2：U2 删除的场景/豁免断言预设文件已删（U1）；若 U2 先行，scenario 测试可能引用尚存 builtin 但文件未删或反之——顺序保证「先注册面、后测试资产」，中间态始终可编译。
- U2→U3：U3 的文档改述引用 impl-review 的最终语义，且 rg 扫尾以 U1+U2 完成后的最小命中集为基线。
- 不可交换的原因：任何顺序颠倒都会让 parity/parity-adjacent 测试或 rg 扫尾出现计划外失败窗口。

## 9. 执行命令清单

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败是否允许继续 |
| ---- | ---- | ---- | ---- | ---- |
| `cargo nextest run -p ralph-cli --bin ralph -- ce_executor_pipeline_loop` | U1 Red | removal-lock 先红 | 测试失败（preset 仍存在） | 是——这是预期 Red |
| `cargo build -p ralph-cli` | U1 移除后 | 无 include_str 缺文件/parity panic | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- preset` | U1 绿 | PRESETS/index/zsh/manifest parity | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- descriptor` | U1 绿 | descriptor 元组与 fallback | 绿 | 否 |
| `.venv/bin/python -m pytest skills/tests/test_html_deliverables.py` | U1 绿 + U3 | python 契约 | 绿 | 否 |
| `./scripts/validate-preset-authoring.sh` | U1 绿 | manifest↔PRESETS↔index 脚本级对齐 | 全 pass | 否 |
| `cargo nextest run -p ralph-core --test scenarios` | U2 | 场景套件收窄绿 | 绿、无 loop 场景 | 否 |
| `cargo nextest run -p ralph-core` | U2 回归 | ralph-core 全包 | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- diagnosis` | U2 | lint-gate 豁免缩减 | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- sync_timeout` | U2+U3 | U7 wiring 契约（D3 保留） | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph` | U2+U3 回归 | ralph bin 全量单测 | 绿 | 否 |
| `rg -n --hidden -g '!.git' "ce-executor-pipeline-loop"`（允许区过滤见 U3-§9；`--hidden` 捕获 `.cursor/` 面，E25） | U1-U3 每步 + 最终 | 活跃引用扫尾 | 每步递减，最终仅剩允许区 | 否 |
| `cargo clippy`、`cargo fmt --check`、`cargo doc --no-deps` | U3 Close | lint/format/doc 无退化 | 绿 | 否 |
| `.venv/bin/python -m pytest skills/tests/` | U3 | skills 测试全量 | 绿 | 否 |
| `./scripts/run-tests.sh` | 最终门禁 | 全 workspace nextest + doctest | 全绿 | 否 |
| `./scripts/validate-builtin-presets.sh` | 最终门禁（若可用） | index 驱动 builtin 校验 | 全 pass | 否 |

注意：测试一律走 `cargo nextest run`（仓库 HARD RULE）；`validate-preset-authoring.sh` 内部虽含 `cargo test`，那是脚本自身实现，直接调用脚本即可，不要手工复现其内部命令。

## 10. 最终质量门禁

- 三个 Unit 全部完成且各自完成标准达成；
- `./scripts/run-tests.sh` 全绿（含 doctest）；
- `cargo clippy` / `cargo fmt --check` 绿；
- `.venv/bin/python -m pytest skills/tests/` 绿；
- `./scripts/validate-preset-authoring.sh`、`./scripts/validate-builtin-presets.sh`（如适用）绿；
- `rg -n --hidden -g '!.git' "ce-executor-pipeline-loop"` 全仓命中仅剩：manifest 墓碑注释、U7 白名单字面量及其注释、`docs/achieved|report|handoff|plans` 历史档案、`presets/en/ce-executor-pipeline-preset-author-notes.md` 历史段、本计划文件、`ralph.ce-executor-pipeline.yml` 与 `PROMPT.ce-executor-pipeline.md` 中指向本计划的 `plan:` 路径引用（E24）、gitignored `.claude`/`.devin`；
- `rg -n "ce_executor_pipeline_loop" crates/` 0 命中；
- 无新增失败/skip/`.only`/无解释 snapshot 更新/削弱断言；
- `ce-executor-pipeline` 全部既有测试与场景行为不变（由其既有测试矩阵证明）；
- 未触碰与本计划无关的已提交改动面（`0a9a913f` web 重构、`b496a761`/`a62671dd`/`113d54e4` monitor 系列、`f0f12b73` 依赖升级、`c8929b36` skills/devtools、`83a36abf` rules SOP），以及 bootstrap 套件 `ralph.ce-executor-pipeline.yml`、`PROMPT.ce-executor-pipeline.md`（E24）；
- 每个 Unit 可独立提交、严格按序落地。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| -------------------------- | --- | ----- |
| 这是实施计划而不是 Roadmap 吗 | 是 | 每 Unit 有真实文件、测试名、命令、Red/Green 序列 |
| Executor 是否仍需做关键设计决策 | 否 | 唯一条件分支（verdict_gate 场景内容确认、impl-review 字段对照）均给出判定标准与停止条件 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E25 全部为实际读取结果 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D8 最低 0.85 |
| 是否存在未处理的低置信度假设 | 否 | 两个条件确认点均为可执行判定，非假设伪装 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1=注册面不可解析、U2=测试资产不存在、U3=文档面零引用 |
| 每个 Unit 是否可以独立验证 | 是 | 各有独立命令集与完成标准 |
| 每个 Unit 是否有真实 Red | 是 | U1 有 removal-lock 失败断言；U2/U3 为纯移除 Unit，Red 形态为「存在→消失」rg/套件对照，已如实标注不新增假测试 |
| 每个 Unit 是否包含回归范围 | 是 | §16 逐 Unit 列出 |
| 是否存在未来 Unit 依赖 | 否 | U1 自带本单元内所有边界 |
| 是否存在泛化任务描述 | 否 | 全部落到真实文件/测试/命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5/§6 矩阵 |
| 所有关键决策是否有 Evidence | 是 | D 列均引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | U1→U2→U3 线性依赖 |
