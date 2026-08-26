---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# refactor: 让 ralph-project-bootstrap 形成可验证的完整流水线

## 0. 计划状态

**状态：READY**

本计划针对 `ralph-project-bootstrap` 的可用性问题：现有审计、生成、静态验证、smoke 和 handoff helper 已分别存在，但 skill 没有统一编排入口，agent 需要自行拼接调用链，因而容易漏步骤、误判验证等级或在失败后不知道下一步动作。

本计划不改变 Ralph runtime、preset 规则或目标项目的业务代码，只为这个 public skill 增加一条可执行的 bootstrap pipeline，并把生成物、阶段结果和最终 handoff 统一起来。

**基线：**分支 `pittcat-dev`，HEAD `d9060d6f`。

**已调查范围：**

- `skills/ralph-project-bootstrap/SKILL.md`、`agents/openai.yaml` 与 references；
- `scripts/audit.py`、`pipeline_suite.py`、`agent_docs.py`、`cli_probe.py`、`smoke_runner.py`、`handoff.py`；
- `skills/tests/test_project_bootstrap_contract.py`、`test_project_bootstrap_e2e.py`、`test_project_bootstrap_real_cli.py`、`conftest.py`；
- `skills/install.py`、`skills/README.md`、`docs/guide/project-bootstrap.md`；
- 相关 Git 历史，特别是 preset-bound 两文件套件和静态验证契约的提交。

**已执行的验证/调查命令：**

- `git -C . rev-parse --show-toplevel`：确认目标仓库根目录；
- `rg --files`、`rg -n`、`sed`：确认实现入口、调用关系、测试和文档引用；
- `command -v ralph`、`ralph --version`：确认本机存在 `ralph 0.1.0`；
- `ralph preset list --format json`：确认 builtin source 到 template name 的实际映射格式；
- `ralph preset show ce-executor-lite --format yaml` 与两个 `--help`：确认 builtin 解析必须先查 manifest，再按 template name 取 YAML；
- `cmp`：确认当前 `skills/`、`.agents/skills/`、`.claude/skills/` 中的 bootstrap `SKILL.md` 当前字节一致。

**尚未执行：**本计划尚未修改生产代码，因此未执行本计划新增测试；最终测试命令在第 9 节定义。当前调查没有发现需要外部网络资料或新第三方依赖的决策。

**阻塞项：**无。所有实施关键决策均有现有代码、测试、文档或实际 CLI 输出支持，置信度均不低于 0.85。

## 1. 功能目标

### 1.1 业务目标与调用方

调用方是使用 agent skill 的 operator/agent。输入一个目标项目 cwd 和一个 file preset 或 builtin preset，系统应给出可重复的 bootstrap 结果，而不是要求调用方自己拼接五个 helper。

### 1.2 当前行为

当前 skill 文档要求 agent 依次自行完成：读取并解析 preset、执行 root/input audit、调用 `compose_preset_bound_suite`、调用 `agent_docs` 写文档、执行 `cli_probe.validate_pipeline`、可选执行 `smoke_runner.run_smoke`，最后手工构造 `handoff.HandoffInputs`。现有跨层测试 `test_project_bootstrap_e2e.py::TestCrossLayerBootstrap::test_e2e_blank_project_to_complete` 正是这样手工串联的。

现有 helper 没有一个属于 `ralph-project-bootstrap` 的统一 `run_pipeline` 入口；不能复用其他 bootstrap 流程的 sandbox/change-plan 语义替代本 skill 的目标项目语义。

### 1.3 目标行为

新增 `skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py`，提供 `run_pipeline(...)` 和 `python -m bootstrap_pipeline`/脚本 CLI 入口。入口按固定顺序执行：

1. 校验 repo root、preset、plan/prompt 等输入；
2. 解析 file preset 或 builtin preset 的完整 YAML；
3. 生成或 reconcile preset-bound config/prompt 与 AGENTS/CLAUDE managed sections；
4. 在写入后 reopen 并验证生成物和 provenance；
5. 严格执行 capability → preset check → preflight → dry-run；
6. 仅对 skill 自带的 `content_fixed_replay` 执行 bounded smoke；
7. 根据 typed smoke outcome 构造 handoff，并返回结构化结果与 Markdown 报告。

每一步失败都返回明确阶段、错误码、证据和下一步状态；后续阶段不得在前置阶段阻塞后继续执行。

### 1.4 行为差异

- operator 不再手工拼接 helper 调用；
- 生成物清单、`created/updated/noop/blocker` 结果和验证证据由一个 `PipelineResult` 汇总；
- dry-run 通过只表示 `incomplete_static_only`，不能单独升级为 `complete`；
- 只有 typed smoke outcome 为 `bounded_terminal_reached` 才能输出官方 launch command；
- 缺少首次运行 plan 不阻塞可生成的 preset-native 或 plan-driven fallback suite，但 handoff 必须保留 `PLAN_PATH` 模板；
- 任一 ownership、root、preset 解析或生成物校验冲突在写入前阻塞，不能 best-effort 覆盖。

### 1.5 输入与输出

**输入：**

- `project_root`：通常为当前 cwd；
- `preset`：repo-relative YAML 路径或 `builtin:<id>`；
- 可选 `plan_path`、`prompt_file`、`binary`、backend/budget/runtime 参数、worktree 参数；
- 可选 `runner`，仅供测试注入，不改变 production 默认的 `subprocess.run`；
- 可选 `smoke_backend`，只有 `content_fixed_replay` 自动授权。

**成功输出：**

- `ralph.<stem>.yml`；
- 在 preset inline prompt 被拥有时生成 `PROMPT.<stem>.md`；
- 需要时更新 `AGENTS.md` 和 `CLAUDE.md` 的 `RALPH-BOOTSTRAP-*` managed section；
- 结构化 `PipelineResult`，包含阶段结果、文件 disposition、验证证据、smoke outcome、handoff level、launch argv 和报告。

**状态：**

- `blocked`：输入、root、preset、ownership、生成物或静态 gate 失败；不执行后续阶段，不提供可执行命令；
- `incomplete_static_only`：静态 gate 全绿，但 smoke 未授权/未执行或首次运行动态参数缺失；只提供带模板或 `[CANDIDATE]` 的命令；
- `complete`：静态 gate 全绿且 typed smoke 到达 `bounded_terminal_reached`；提供官方命令。

### 1.6 错误语义、不变量与约束

- 所有 target-project 写入仍由现有 `AtomicWriter` 完成；pipeline 不直接绕过 pure compose helper 写文件。
- 生成物只允许位于确认的项目 root 下，所有持久化和 handoff 路径必须 repo-relative。
- config/prompt 是成对拥有的；缺一、provenance 损坏、摘要不匹配或人工修改均阻塞 refresh。
- 用户文档 managed section 外的字节、未拥有的 YAML key 和未命名文件必须保持不变。
- 任何验证命令必须显式带 `-c ralph.<stem>.yml -H <preset>`。
- smoke 默认不触碰 real/mock/custom backend；unsafe backend 必须产生 `not_authorized`，不得创建 subprocess。
- 不新增第三方依赖；沿用 skill 当前 Python stdlib + 可选 PyYAML 解析约束。

### 1.7 范围与非目标

**范围：**统一 bootstrap 编排入口、preset 解析、结果/状态归一、写入后生成物校验、静态 gate/smoke/handoff wiring、CLI/skill 文档和端到端 contract tests。

**非目标：**修改 Ralph runtime CLI、改变 preset lint/schema、创建或切换 git branch/worktree、改写目标项目非 owned 内容、增加新的 backend、把 loop 日常操作并入 skill。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

当前可复用调用链是：

`audit.run_audit` → `pipeline_suite.compose_preset_bound_suite` / `reconcile_preset_bound_suite` → `agent_docs.compose_agent_docs` + `AtomicWriter` → `pipeline_suite.verify_preset_bound_files` → `cli_probe.validate_pipeline` → `smoke_runner.run_smoke` → `handoff.build_handoff`。

其中前四个 helper 主要是纯计算或 typed result；`cli_probe` 和 `smoke_runner` 通过 `runner` 注入支持确定性测试；`handoff` 明确要求 typed smoke outcome 才能提升等级。缺少的是把这条调用链放入一个 failure-short-circuit 的外部入口。

### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `skills/ralph-project-bootstrap/scripts/audit.py::run_audit` | 已有 root、input、project facts 的 typed `AuditDecision`，并在 blocking 时可停止写入 | 新入口复用 audit，不新增第二套 root/facts 解析 | 高 |
| E2 | `audit.py::audit_inputs` 与 `_paths.py` | audit 已验证文件存在性，但统一入口必须在任何 IO 前统一执行 `_paths.is_safe_relative`/`contain` | 把路径边界放在 orchestrator 输入门，而不是让各 helper 各自猜 | 高 |
| E3 | `pipeline_suite.py::compose_preset_bound_suite` | 已生成 preset-specific config/prompt，并将 `generator_version`、`input_signature`、`profile_sha256`、`prompt_sha256` 写进 `_bootstrap` | 新入口只负责参数装配、reconcile 和持久化，不重写 YAML 生成器 | 高 |
| E4 | `pipeline_suite.py::reconcile_preset_bound_suite`、`verify_preset_bound_files` | 已有成对文件、摘要匹配、prompt source mismatch、stale/incomplete blocker | 写入后必须调用 verify；不能只相信 compose 返回值 | 高 |
| E5 | `agent_docs.py::compose_agent_docs`、`AtomicWriter` | managed section 外内容保持字节不变，marker/sync conflict 阻塞，批量写入支持 rollback | 新入口将 docs 与 suite 作为一个写入批次 | 高 |
| E6 | `cli_probe.py::validate_pipeline` | capability → preset_check → preflight → dry_run 严格顺序；前一阶段失败后续记录 skipped | PipelineResult 必须保留每个 `StageDecision`，不能只输出 bool | 高 |
| E7 | `smoke_runner.py::run_smoke` | nine typed outcomes；只有 replay 自动可信；`bounded_terminal_reached` 表示 bounded terminal | smoke 只能在 static gate 之后执行，并由 typed outcome 驱动 handoff | 高 |
| E8 | `handoff.py::build_handoff`、`_enforce_typed_outcome` | 无 typed outcome 或 unsafe/失败 outcome 会降级/阻塞，blocked 不生成命令 | 复用 handoff 的反伪阳性规则，不在 orchestrator 重复实现等级判断 | 高 |
| E9 | `skills/tests/test_project_bootstrap_e2e.py::test_e2e_blank_project_to_complete` | 当前跨层测试必须手工调用 6 个阶段，证明 helper 能协作但没有 public pipeline | 新增测试应直接调用 `bootstrap_pipeline.run_pipeline`，验证外部行为 | 高 |
| E11 | `skills/ralph-project-bootstrap/SKILL.md`、`references/{context-audit,validation,handoff}.md` | 文档分别描述输入、校验和 handoff，但没有一个可调用入口及完整状态表 | U5 必须把文档改为入口驱动，并列出产物/状态/证据 | 高 |
| E12 | `skills/tests/conftest.py` | 测试通过 flat-module preload 加载 bootstrap helper；e2e helper 使用独立 module name | 新模块必须加入 conftest，测试不应自行修改 sys.path | 高 |
| E13 | `skills/install.py`、当前三份 `SKILL.md` `cmp` 结果 | public skill 从 `skills/` physical-copy 到 `.claude/skills` 与 `.agents/skills`；当前 SKILL 副本一致 | 更新源文件后必须通过 installer 重新物化并增加 parity 检查 | 高 |
| E14 | `ralph preset list --format json`、`ralph preset show --help`、实际 `preset show ce-executor-lite --format yaml` | manifest 项有 `source: builtin:...` 和独立 `name`；show 接受 template name，输出完整 YAML | builtin 解析必须 list→source match→show template，不能 strip `builtin:` 猜 name | 高 |
| E15 | `git log`：`67fd6f99`、`0eb23c34`、`d9060d6f` | 最近变更集中在 preset-bound 两文件、项目 facts、静态 dry-run prompt source 契约 | 计划只扩展编排层，必须保留现有 helper/fixture 契约 | 高 |

### 2.3 受影响范围

**生产/skill helper：**新增 `skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py`；复用并可能只补充公开调用所需的 `audit.py`、`pipeline_suite.py`、`agent_docs.py`、`cli_probe.py`、`smoke_runner.py`、`handoff.py`，不重写其既有规则。

**测试：**新增 `skills/tests/test_project_bootstrap_pipeline.py`；修改 `skills/tests/conftest.py`；必要时在 `skills/tests/test_project_bootstrap_contract.py` 增加缺失的输入边界 characterization；修改 `skills/tests/test_project_bootstrap_e2e.py` 以保留 helper-level 回归并新增统一入口覆盖。

**fixtures：**复用 `skills/ralph-project-bootstrap/fixtures/projects/*` 和 `fixtures/cli/*`；如 builtin resolver 需要稳定 transcript，新增只描述 `preset list`/`preset show` 的 fixture 文件，不修改现有 green/blocking fixture 语义。

**文档/安装：**`skills/ralph-project-bootstrap/SKILL.md`、`references/context-audit.md`、`references/validation.md`、`references/handoff.md`、`agents/openai.yaml`、`skills/README.md`、`docs/guide/project-bootstrap.md`；通过 `skills/install.py` 同步 `.agents/skills/` 和 `.claude/skills/` 副本。

## 3. 决策记录与置信度

### D1：新增 project-bootstrap 统一入口，而不是只改文档

- **候选：**A. 仅改 `SKILL.md`；B. 新增 `bootstrap_pipeline.py` 并让文档指向它。
- **选择：**B。
- **证据：**E9 证明当前只能手工串联；E10 证明仓库已有适合的编排结果模式；E11 证明文档本身无法消除调用歧义。
- **排除：**A 不能保证 agent 实际执行所有阶段；C 会把 sandbox/change-plan 语义错误带入目标项目 bootstrap，扩大范围并破坏边界。
- **置信度：**0.96。

### D2：builtin preset 解析采用 list source 映射后 show template

- **候选：**A. `builtin:<id>` 去掉前缀直接作为 template name；B. `ralph preset list --format json` 查找 `source`，再 `ralph preset show <name> --format yaml`；C. 复制本仓库 `presets/` 到目标项目。
- **选择：**B。
- **证据：**E14 的真实 CLI 输出显示 `source` 与 `name` 分离；现有 `SKILL.md` 也明确该规则。
- **排除：**A 会在 `builtin:ce-executor-pipeline` → `ce-executor-pipeline` 不存在或不等价时失败；C 违反目标项目可独立运行和不触碰 source preset 的边界。
- **置信度：**0.98。

### D3：pipeline 只做 orchestration，生成和写入继续委托现有 pure helper/AtomicWriter

- **候选：**A. 在新入口重新拼 YAML/Markdown；B. 调用现有 compose/reconcile/verify，并统一组织 AtomicWriter；C. 把 helper 合并成一个大模块。
- **选择：**B。
- **证据：**E3、E4、E5 已覆盖 provenance、幂等、冲突、回滚和用户内容保留。
- **排除：**A 会产生第二套生成规则；C 会扩大 diff、降低已有契约可定位性。
- **置信度：**0.95。

### D4：阶段状态由 typed result 组成，不用自由文本推断

- **候选：**A. 一个最终 bool；B. 每个阶段保留 typed result，再由 handoff 归一 level；C. 由报告字符串匹配 `complete`/`error`。
- **选择：**B。
- **证据：**E6、E7、E8；`handoff.py` 已明确拒绝只凭 free-text 升级 complete。
- **排除：**A 丢失失败阶段和 skipped 证据；C 会重现已被 F7 契约禁止的 fake positive。
- **置信度：**0.97。

### D5：缺少首次运行 plan 不阻塞 provisioning，但阻塞“ready/complete”

- **选择：**沿用现有 preset-native/plan-driven fallback 语义：可生成则生成；最终 handoff 为 `incomplete_static_only`，命令使用 `--plan PLAN_PATH` 模板。
- **证据：**E3、E8、`SKILL.md` 当前 workflow 90-99 行、contract tests 中 `test_missing_required_plan_produces_template_not_blocker`。
- **置信度：**0.94。

### D6：不新增持久化报告文件

- **候选：**A. 目标项目写一个 report 文件；B. `PipelineResult` 返回结构化证据和 `handoff_report`，CLI 默认打印，`--json` 输出机器可读结果；C. 只打印命令。
- **选择：**B。
- **证据：**E10 已有 `PipelineResult.to_json` 模式；E8 的 `HandoffArtifact.report` 已是纯字符串；现有 skill 明确只拥有 suite/docs，不拥有额外 report 文件。
- **排除：**A 会扩展 target-project ownership 和清理契约；C 无法让 agent 稳定消费阶段 evidence。
- **置信度：**0.91。

CLI 退出码固定沿用仓库已有 pipeline 习惯：`0` 表示 provisioning 成功（`complete` 或 `incomplete_static_only`），`2` 表示 `blocked`；JSON 与文本输出不得改变该规则。CLI 默认不执行 smoke；只有传入 repo-relative `--replay-transcript` 时才构造 skill 自带的 `SafeBackend`。

### D7：运行参数从 resolved preset 派生，入口不再要求 operator 重复填写

- **候选：**A. 要求调用方额外传 backend、max iterations 和 wall-clock budget；B. 从完整 resolved preset 的 `cli.backend`、`event_loop.max_iterations`、`event_loop.max_runtime_seconds` 派生；C. 复制 baseline 的占位值。
- **选择：**B；字段缺失、类型错误或非正数返回 `preset_runtime_contract_missing` blocker，不猜默认值。smoke 的 `idle_timeout_secs` 继续使用现有 `SmokeConfig` 默认值，因它是 smoke harness cap，不是 preset runtime budget。
- **证据：**E14 的真实 `debug` 和 `ce-executor-lite` 输出均包含 `event_loop.max_iterations`、`event_loop.max_runtime_seconds` 和 `cli.backend`；`compose_preset_bound_suite` 明确要求 backend/budget 参数；`SmokeConfig` 已定义独立的三重 cap。
- **排除：**A 让 operator 重复输入并可能与 preset 不一致；C 会把 `ralph.pipeline.base.yml` 的 `PROJECT_*` 占位符误写入目标项目。
- **置信度：**0.94。

## 4. BDD 行为规格

### Feature: 通过单一入口 bootstrap 一个 preset-bound target project

  **Background:**

  - Given target cwd、preset、可选 plan/prompt 已作为 repo-relative 输入提供
  - And pipeline 使用现有 `audit`、`pipeline_suite`、`agent_docs`、`cli_probe`、`smoke_runner`、`handoff` helper

  **Scenario B1: 输入和 root 通过后才允许继续**

  Given 唯一 VCS root、可读取 preset、可选输入路径均在 root 内
  When operator 调用 `run_pipeline`
  Then pipeline 返回非 blocking audit result 并进入 preset resolution
  And 在 audit 失败前目标项目没有任何 owned file 写入

  **Scenario B2: root 歧义在写入前阻塞**

  Given cwd 暴露互相冲突的 VCS root 与 AGENTS/CLAUDE scope
  When operator 调用 `run_pipeline`
  Then result 为 `blocked` 且错误码包含 `root_ambiguous`
  And 不调用生成、写入、static gate 或 smoke

  **Scenario B3: builtin 使用 manifest source 找 template**

  Given `ralph preset list --format json` 返回一个 `source=builtin:debug` 且 `name=debug` 的条目
  When operator 传入 `builtin:debug`
  Then pipeline 使用 `ralph preset show debug --format yaml` 得到完整 preset YAML
  And 不使用字符串 strip 直接猜 template name

  **Scenario B4: invalid preset 或无 prompt 的输入在生成前阻塞**

  Given preset file 不可读、builtin 映射不存在、YAML 无效，或无 plan/prompt 且 preset 没有非空 `event_loop.prompt`
  When pipeline 解析 preset
  Then result 为 `blocked`，包含可定位错误码和证据
  And config、prompt、AGENTS、CLAUDE 均不被写入

  **Scenario B5: 新项目生成 preset-bound 两文件和 managed docs**

  Given audit 和 preset resolution 成功，preset 有非空 inline prompt，项目没有受管文件
  When pipeline 运行 generation stage
  Then只生成 `ralph.<stem>.yml`、`PROMPT.<stem>.md` 以及需要的 AGENTS/CLAUDE managed section
  And config 的 `event_loop.prompt_file` 指向同一 `<stem>` prompt
  And `_bootstrap` 含四个 provenance 字段

  **Scenario B6: 相同输入第二次运行是 noop**

  Given第一次运行已生成且磁盘 bytes 与 provenance 一致
  When以相同 preset/input 再次调用 pipeline
  Then config、prompt、docs disposition 均为 `noop`
  And 用户内容、mtime 之外的文件内容不被重写

  **Scenario B7: ownership 或 mirror conflict 阻止部分写入**

  Given已有 config/prompt 被人工修改，或 AGENTS/CLAUDE managed bodies 不一致
  When pipeline 进入 reconcile stage
  Then result 为 `blocked`，错误码为现有 `owned_value_user_modified`、`provenance_corrupt` 或 `sync_mirror_conflict`
  And AtomicWriter 不提交任何 target

  **Scenario B8: static gate 按固定顺序短路**

  Given capability 或 preset check 失败
  When pipeline 进入 validation stage
  Then只执行直到第一个失败阶段，后续 stage 记录 skipped
  And result 保存每个 stage 的 argv、outcome、reason、evidence

  **Scenario B9: dry-run 全绿但没有 smoke 时只能 static-only**

  Given四个 static stage 全部 `ok`，但 smoke 未授权或没有运行
  When pipeline 构造 handoff
  Then level 为 `incomplete_static_only`
  And命令带 `[CANDIDATE]` 或 `PLAN_PATH` 模板，不称为 loop closed

  **Scenario B10: replay smoke 到达终态才 complete**

  Given static gate 全绿且 backend kind 为 `content_fixed_replay`
  When smoke 返回 typed `bounded_terminal_reached`
  Then handoff level 为 `complete`
  And命令包含显式 config、preset、必要 plan/prompt 参数且不带 candidate 前缀

  **Scenario B11: unsafe 或失败 smoke 不得伪造 ready**

  Given backend kind 为 `mock`/`custom`/`real` 未获授权，或 smoke 返回 timeout/non-zero/error event
  When pipeline 构造 handoff
  Then unsafe 未执行 subprocess 并降为 `incomplete_static_only`，失败 outcome 变为 `blocked`
  And blocked handoff 的 command 为空

  **Scenario B12: CLI 和 JSON 输出表达同一结果**

  Given pipeline 返回任一 `blocked`、`incomplete_static_only` 或 `complete`
  When运行 CLI 默认模式和 `--json`
  Then默认输出包含阶段摘要、生成文件 disposition、smoke 状态、报告和命令
  And JSON 字段与 `PipelineResult` 一致，退出码仅由 pipeline level/blocked 规则决定

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口/层级 | 风险补充 | E2E |
|---|---|---|---|---|
| B1-B2 | root/input blocker 在任何 write/CLI/smoke 前返回；无 target 变化 | `skills/tests/test_project_bootstrap_pipeline.py`，pipeline integration | 复用 `ambiguous-root`、missing input fixtures；不 mock audit | 是 |
| B3-B4 | builtin argv 顺序和 YAML 解析正确；无映射/无 prompt 阻塞 | 新 `preset resolution` tests，module unit + fake subprocess runner | characterization real CLI output；invalid YAML fixture | 否/契约 |
| B5 | exact owned file set、prompt binding、四 provenance keys 和 project guardrails 存在 | pipeline integration，临时 target project | 不允许只断言文本片段；解析 YAML 和 reopen bytes | 是 |
| B6-B7 | second run noop；手改 config/prompt/docs conflict 使整批 rollback | integration + existing `test_atomic_writer_*` | idempotency、fault injection、dirty-tree | 是 |
| B8 | 四阶段顺序、argv、skipped evidence 和 blocker 分类保持现有契约 | `cli_probe` existing contract + pipeline integration | 使用现有 `fixtures/cli/*`，不连接真实 backend | 是 |
| B9-B11 | handoff level 只能由 typed outcome 推导；命令/报告状态正确 | existing `handoff`/`smoke` contract + pipeline integration | unsafe no-spawn、timeout、error bucket、anti-fake-positive | 是 |
| B12 | text/JSON 仅是同一 `PipelineResult` 的两种渲染 | CLI subprocess integration | 退出码和 JSON 可解析性 | 是 |

测试必须保留现有 helper contract suite；新增测试不能用 byte-equality 锁定整个 preset prompt，只能验证结构化行为、provenance、argv、状态和真实 runtime path。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 单一入口按固定顺序完成 bootstrap | B1-B5 | `test_pipeline_success_creates_owned_outputs` | input normalization、resolver | pipeline contract | blank project | E1-E5,E9 |
| R2 | blocker 在写入前停止 | B2,B4,B7 | `test_pipeline_blocker_does_not_write_or_validate` | blocker mapping | atomic no-write | conflict project | E1,E4,E5 |
| R3 | builtin 正确解析完整 preset | B3 | `test_builtin_resolution_uses_source_then_template` | JSON mapping/duplicate source | fake runner transcript + real CLI characterization | 可选 real CLI | E14 |
| R4 | 生成物安全、可刷新、幂等 | B5-B7 | `test_pipeline_second_run_is_noop`; `test_pipeline_conflict_rolls_back` | disposition mapping | provenance reopen/AtomicWriter | existing-suite/dirty-tree | E3-E5 |
| R5 | static gate 证据完整且短路 | B8-B9 | `test_pipeline_preserves_stage_decisions_and_skips` | stage result mapping | existing CLI fixtures | blank project | E6 |
| R6 | smoke/handoff 等级不造假 | B9-B11 | `test_pipeline_handoff_level_follows_typed_smoke` | outcome-to-level mapping | fake replay/unsafe/failure | green + unsafe | E7-E8 |
| R7 | CLI/JSON 提供可操作 handoff | B12 | `test_pipeline_cli_json_matches_result` | serialization | subprocess CLI | CLI invocation | E10-E13 |
| R8 | skill 文档与 installed copies 描述同一流程 | B1-B12 | `test_project_bootstrap_skill_copies_are_in_sync` | 不适用 | installer contract + doc drift | 安装 smoke | E11-E13 |

## 7. 严格串行开发单元

### U1. 建立统一 pipeline 的输入与 preset 解析入口

**目标：**调用方可以通过新 `bootstrap_pipeline.run_pipeline` 进入，并在 root/input/preset resolution 失败时得到结构化 blocker，且不产生写入。

**对应：**R1、R2、R3；B1-B4；D1、D2、D7；E1、E2、E10、E14。

**新 public interface（本 Unit 新增，字段固定）：**

- `run_pipeline(*, cwd: Path | str, preset: str, plan_path: str | None = None, prompt_file: str | None = None, binary: str = "ralph", refresh_existing: bool = False, use_worktree: bool = False, reuse_worktree: bool = False, plan_arg: str | None = None, worktree_name: str | None = None, runner: Callable[..., object] | None = None, smoke_backend: SafeBackend | UnsafeBackend | None = None) -> PipelineResult`；`smoke_backend` 为空时不运行 smoke。
- `PipelineResult` 必须包含：`level`（`blocked | incomplete_static_only | complete`）、`blocked`、`stage`、`code`、`message`、`root`、`preset`、`config_path`、`prompt_path`、`files_created`、`files_updated`、`files_noop`、`validation_evidence`、`stage_decisions`、`smoke_outcome`、`smoke_evidence`、`handoff_command`、`handoff_report`、`next_action`。
- resolver 成功后产生内部 `ResolvedPreset`：`preset_id`、`source_kind`、`template_name`（builtin 时）、`text`、`backend`、`max_iterations`、`max_runtime_seconds`、`inline_prompt_present`。这些字段全部来自 file YAML 或 `ralph preset show --format yaml`，不得由入口猜测。
- 入口只接受上述 operator inputs；backend/budget 不作为重复参数暴露，统一从 `ResolvedPreset` 派生。`runner` 只用于测试注入，production 默认使用 `subprocess.run`。

**外部结果：**file preset 读取 repo-relative YAML；builtin 按 list source→show template 解析 YAML；任何 blocker 都不进入 generation。

**当前基线与 Red：**当前 `skills/ralph-project-bootstrap/scripts/` 没有 `bootstrap_pipeline.py`，`conftest.py` 也不加载它；先新增 `test_pipeline_blocker_does_not_write_or_validate` 和 `test_builtin_resolution_uses_source_then_template`，运行后应因 `ModuleNotFoundError`/入口不存在而失败。若失败来自 pytest 环境、fixture import 或 runner argv 错误，不算有效 Red，必须先修测试。

**修改位置：**

- 新增 `skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py`：定义固定字段的 `PipelineResult`/内部 `ResolvedPreset`、preset resolution、输入规范化、`run_pipeline` 的 audit/resolution 前半段；不在此单元写 docs/config。
- 修改 `skills/tests/conftest.py`：按现有 flat-module preload 模式加载新模块。
- 新增 `skills/tests/test_project_bootstrap_pipeline.py`：只覆盖输入 blocker、repo-relative path、file/builtin resolver。
- 复用 `skills/ralph-project-bootstrap/scripts/audit.py` 和 `_paths.py`；只有当 characterization test 证明 audit 入口未阻止绝对/escape 输入时，才在本单元补最小边界修复。

**输入/输出/错误：**缺 preset、root ambiguous、非法绝对/escape path、preset file unreadable、builtin source missing、show 非零、YAML parse error 或 `cli.backend`/`event_loop.max_iterations`/`event_loop.max_runtime_seconds` 缺失或非法，均输出 blocker；不创建任何 target 文件。

**最小 TDD 顺序：**

1. B2 acceptance test Red；实现 pipeline 的 audit short-circuit；Green。
2. B3 resolver test Red；实现 file read 与 list JSON source/template 映射；Green。
3. B4 invalid resolver tests Red；实现明确 blocker code/evidence；Green。
4. 加入 runtime field extraction/validation、固定 result fields 和 JSON 基础序列化；运行本单元集成回归。

**禁止依赖/实现：**不生成 config/prompt，不调用 `AtomicWriter`、static gate 或 smoke；不改变 preset lint；不把 builtin id 直接 strip 成 template name。

**完成标准：**B1-B4 测试通过；所有失败都在写入前发生；fake runner 捕获的 resolver argv 与 E14 一致；绝对/escape 路径不进入 filesystem call；现有 audit/path tests 仍绿。

**停止条件：**真实 CLI 的 `preset list` schema 与 E14 冲突、发现多个同 source 且无法依据现有契约选唯一 template、或 resolver 需要新增未计划依赖时停止并重新记录证据。

### U2. 通过 pipeline 生成、reconcile 并验证 owned artifacts

**目标：**audit/resolution 成功后，统一入口原子生成 preset-bound config/prompt 与 docs managed sections，并在提交后 reopen 验证。

**对应：**R1、R4；B5-B7；D3、D5；E3-E5、E9。

**外部结果：**新项目获得正确 `<stem>` 两文件；已有相同输入返回 `noop`；手改、缺文件、provenance mismatch、mirror conflict 或 AtomicWriter fault 返回 blocker，目标项目回到写入前状态。

**当前基线与 Red：**现有跨层测试手工调用 `_apply_compose`、`_write_agent_docs` 和 `_write_pipeline_suite`，不存在直接 pipeline generation path。先增加 `test_pipeline_success_creates_owned_outputs`、`test_pipeline_second_run_is_noop`、`test_pipeline_conflict_rolls_back`，应因 U1 pipeline 尚未实现 generation stage 而失败。

**修改位置：**

- 修改 `skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py`：用 `compose_preset_bound_suite`、`reconcile_preset_bound_suite`、`compose_agent_docs`、`verify_preset_bound_files` 组装结果；以显式 operations 列表交给 `agent_docs.AtomicWriter`。
- 只在需要时修改 `pipeline_suite.py` 的公开参数适配；不得复制 YAML emitter。
- 新增/扩展 `skills/tests/test_project_bootstrap_pipeline.py`；复用 `fixtures/projects/existing-suite`、`conflicting-docs`、`dirty-tree`、`invalid-yaml`。

**写入边界：**owned files 只能是 preset-bound config/prompt；AGENTS/CLAUDE 只能写 managed section；不创建旧的 `ralph.pipeline.yml`、`PROMPT.pipeline.md` 或 `ralph.bootstrap.yml`；不写 `.ralph/`。

**最小 TDD 顺序：**

1. B5 验收 Red；接入 compose + atomic write；Green。
2. 加入 reopen `verify_preset_bound_files`，让 prompt binding/provenance mismatch 测试先 Red 再 Green。
3. 加入 AGENTS/CLAUDE mirror body 和单批 operations；冲突/rollback 测试 Red→Green。
4. 加入第二次 run noop 与 dirty-tree 断言；refactor result disposition 汇总。

**禁止依赖/实现：**不执行 CLI static gate/smoke；不改变既有 helper 的 provenance 算法、marker grammar 或用户 key 保留规则。

**完成标准：**生成物集合、prompt source、四 provenance keys、guardrails 和文件 disposition 全部可断言；相同输入二次运行 no-op；任何 blocker 都不留下 `.bootstrap.tmp` 或半批更新；现有 `pipeline_suite`、`agent_docs` 全部 contract tests 通过。

### U3. 将 static validation 接入 pipeline 并保留阶段证据

**目标：**生成物 verify 成功后，统一入口严格执行 capability → preset check → preflight → dry-run，并把所有 `StageDecision` 传给 handoff。

**对应：**R5；B8-B9；D4；E6、E11。

**当前基线与 Red：**`cli_probe.validate_pipeline` 已可独立工作，但现有 project-bootstrap 跨层测试在测试文件中手工调用它。先增加 `test_pipeline_preserves_stage_decisions_and_skips`，让 fake fixture 在 capability/preset/preflight/dry-run 逐阶段验证；当前新入口未接入时应失败。

**修改位置：**

- 修改 `bootstrap_pipeline.py`：在 U2 verify 成功后调用 `validate_pipeline`，传入由 `derive_preset_bound_paths` 得到的 config/prompt，以及显式 preset；根据第一 blocker 终止 smoke/handoff 或继续。
- 只有当新入口需要携带额外 evidence 时才扩展 `cli_probe.StageDecision`；优先不改其既有字段和 stage state machine。
- 新增 `skills/tests/test_project_bootstrap_pipeline.py` 的 green、preset fail、backend fail、dry-run source mismatch 场景。

**外部结果：**任何 static blocker 返回 `blocked`，后续 stage 是 `blocked_unknown/skipped` 的既有 decision；四阶段全绿的结果明确标记 `static_load_passed` 而非 `loop_closed`。

**最小 TDD 顺序：**

1. static stage order/argv acceptance Red；接入现有 `validate_pipeline`；Green。
2. blocker short-circuit 与 evidence preservation Red→Green。
3. 生成 `validation_evidence` 面向 handoff 的稳定摘要；确保不改变 helper 的 stage decision。

**禁止依赖/实现：**不在本单元调用 backend；不使用 `--skip-preflight`；不把 dry-run green 映射成 complete。

**完成标准：**复用现有 `fixtures/cli/green`、`preset-strict-fail`、`backend-missing`、`dry-run-source-mismatch` 证明顺序、argv、分类和 skipped stage；所有 static blocker 都阻止 smoke。

### U4. 将 bounded smoke 与 handoff level 接入 pipeline

**目标：**static gate 全绿后，统一入口按 backend authorization 规则决定 smoke，并用 typed outcome 生成正确 handoff。

**对应：**R6；B9-B11；D4、D5；E7-E8。

**当前基线与 Red：**`smoke_runner` 与 `handoff` 已分别有完整 contract，但 project-bootstrap 跨层测试手工构造 `HandoffInputs`。先增加 pipeline-level tests：green replay、unsafe mock、timeout/error outcome；新入口未接线时失败。

**修改位置：**

- 修改 `bootstrap_pipeline.py`：构造 `SmokeConfig`，只自动传 `SafeBackend(kind=content_fixed_replay)`；unsafe backend 不 spawn；将 `SmokeResult` 全量 evidence 和 typed outcome 传给 `handoff.build_handoff`。
- 不修改 `smoke_runner.py` 的 outcome 常量、三重 cap、failure bucket 或 child-group reap 规则。
- 不修改 `handoff.py` 的 anti-fake-positive level reconciliation；只将 `HandoffInputs` 从 pipeline result 正确装配。
- 新增测试覆盖 `bounded_terminal_reached`、`not_authorized`、`timeout_no_event`/`non_zero_exit` 和缺 plan template。

**外部结果：**

- replay terminal：`complete` + 官方 command；
- static green/no smoke 或 unsafe：`incomplete_static_only` + candidate/template command；
- typed smoke failure：`blocked` + 空 command + blocker report。

**最小 TDD 顺序：**replay happy path Red→Green；unsafe no-spawn Red→Green；typed failure blocked Red→Green；缺 plan template 和 worktree reuse-key 回归；refactor handoff assembly。

**禁止依赖/实现：**不自动授权 real/mock/custom；不根据 evidence 文本猜 outcome；不让 smoke 在 static blocker 后运行；不把 smoke report 写进目标项目。

**完成标准：**所有 level、command prefix/empty rule、typed outcome、failure bucket 和 residual risks 可通过结构化断言验证；复用现有 smoke/handoff contract suite 和跨层 fixture。

### U5. 暴露 CLI/JSON 入口并同步 skill 文档与安装副本

**目标：**operator 能直接运行 project-bootstrap pipeline，且 skill 文档、metadata、README、guide 和 installed copies 对入口、产物、状态、校验顺序保持一致。

**对应：**R7、R8；B12；D1、D6；E10-E13。

**修改位置：**

- 修改 `bootstrap_pipeline.py`：增加 `argparse` CLI，固定支持 `--cwd`、`--preset`、`--plan`、`--prompt-file`、`--binary`、`--refresh-existing`、`--replay-transcript`、`--json`；CLI 只调用 `run_pipeline`，不复制业务逻辑。
- 修改 `skills/ralph-project-bootstrap/SKILL.md`：把手工 helper 清单改为入口驱动；用“输入→产物→验证等级→失败动作”说明实际流程；明确 `dry-run != loop closed`。
- 修改 `skills/ralph-project-bootstrap/agents/openai.yaml`：补齐统一入口、JSON/报告输出和三种 handoff level。
- 修改 `skills/README.md`：修正当前仍写 `ralph.pipeline.yml`/`PROMPT.pipeline.md` 的过时描述，改为 preset-bound 两文件和统一 pipeline。
- 修改 `docs/guide/project-bootstrap.md`：加入统一入口的 operator contract，同时保留 preset-bound/provenance/static gate 事实。
- 仅在 references 与新流程不一致时修改 `references/context-audit.md`、`validation.md`、`handoff.md`；不把 skill-owner-only 实现细节泄漏给 end agent。
- 通过 `skills/install.py` 更新 `.agents/skills/ralph-project-bootstrap` 和 `.claude/skills/ralph-project-bootstrap`；不手工维护三份不同内容。
- 新增 `test_pipeline_cli_json_matches_result` 与 `test_project_bootstrap_skill_copies_are_in_sync`，必要时扩展 `skills/tests/test_install.py`。

**当前行为基线与 Red：**当前 public skill 没有 project-bootstrap 自己的 CLI；README 仍描述旧 generic filenames。先运行新增 CLI/parity tests，应分别因入口不存在和描述/副本不满足契约而失败。

**最小 TDD 顺序：**CLI green JSON/text path；验证 `complete`/`incomplete_static_only` 返回 0、`blocked` 返回 2；验证 `--replay-transcript` 才启用 SafeBackend；skill metadata/README/guide anchors；installer physical-copy parity；最后运行 doc drift check。

**禁止依赖/实现：**不把 CLI 变成 Rust `ralph` 子命令；不修改 `skills/install.py` 的全局 catalog 语义，除非测试证明新入口目录未被 physical copy；不引入第三方 CLI parser。

**完成标准：**operator 有一个可复制入口；默认输出和 JSON 表达相同 `PipelineResult`；所有文档说法与当前 preset-bound 实现一致；源目录与两份 installed copy 的 bootstrap skill 文件 parity 通过。

## 8. Unit 串行依赖图

```text
U1 输入/root/preset resolution
 ↓ 提供已验证的 PipelineResult blocker 与 ResolvedPreset
U2 生成/reconcile/verify owned artifacts
 ↓ 提供已验证的 config/prompt/docs 与 disposition
U3 static gate evidence
 ↓ 提供已验证的 StageDecision tuple
U4 smoke + typed handoff
 ↓ 提供已验证的 level/command/report
U5 CLI + skill/docs/install parity
```

U2 不能在 U1 前执行，因为生成必须使用 U1 确认的 root 和完整 preset YAML；U3 不能在 U2 前执行，因为 static gate 必须验证刚写入且 reopen 通过的 config/prompt；U4 不能在 U3 前执行，因为 smoke 必须受 static gate backpressure 保护；U5 最后执行，因为文档必须描述已经验证的 pipeline result 和状态，不得提前承诺未实现接口。

## 9. 执行命令清单

以下命令按仓库现有 Python skill 测试方式定义；所有 Python 测试使用 `.venv/bin/python`，不使用裸 `python`。

| 时机 | 命令 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| U1 每个 Red/Green | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_pipeline.py -k 'input or preset'` | root/input/builtin resolver | 目标测试通过 | 不得进入 U2 |
| U1 回归 | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k 'audit or path'` | 保持 audit/path contracts | 全部通过 | 修复后再继续 |
| U2 每个 Red/Green | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_pipeline.py -k 'generation or reconcile or idempotent or conflict'` | owned artifacts/write safety | 全部通过 | 不得进入 U3 |
| U2 helper 回归 | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k 'agent_docs or pipeline_suite or atomic'` | 既有生成/marker/provenance/rollback | 全部通过 | 修复后再继续 |
| U3 每个 Red/Green | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_pipeline.py -k 'static or stage'` | stage ordering/evidence | 全部通过 | 不得进入 U4 |
| U3 CLI contract | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k 'cli_probe'` | 既有 fixture/argv/classification | 全部通过 | 修复后再继续 |
| U4 每个 Red/Green | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_pipeline.py -k 'smoke or handoff'` | level/command/typed outcome | 全部通过 | 不得进入 U5 |
| U4 helper contract | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k 'smoke or handoff'` | no-spawn、timeout、anti-fake-positive | 全部通过 | 修复后再继续 |
| U5 CLI/parity | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_pipeline.py -k 'cli or docs or install'` | user-facing entry and copies | 全部通过 | 不得宣称完成 |
| U5 real CLI | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_real_cli.py` | real `ralph` accepts emitted argv | 全部通过 | 检查 CLI contract，不改弱断言 |
| U5 doc drift | `./scripts/check-cli-doc-drift.sh` | 检查 CLI/docs 引用漂移 | exit 0 | 修正文档后重跑 |
| 最终 skill 回归 | `.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py skills/tests/test_project_bootstrap_e2e.py skills/tests/test_project_bootstrap_real_cli.py skills/tests/test_project_bootstrap_pipeline.py` | 全部 bootstrap 行为 | 全部通过 | 不进入全量门禁 |
| 最终仓库门禁 | `./scripts/run-tests.sh` | 仓库规定的最终测试入口 | 全部通过 | 按 AGENTS.md 的 nextest/serial fallback 规则处理；不得裸跑 ralph-cli cargo test |

## 10. 最终质量门禁

- B1-B12 全部有可执行测试且通过；
- U1-U5 严格按顺序完成，每个 Unit 的 acceptance Red 确实针对当前缺失能力；
- root/input/preset blocker 不写盘；生成物 verify 在 static gate 前完成；
- config/prompt/docs ownership、provenance、prompt source、path confinement、idempotency、rollback 和 dirty-tree 规则未回退；
- static gate 顺序、argv、分类和 skipped evidence 未回退；
- smoke 只授权 content-fixed replay，typed outcome 是唯一 complete 依据；
- blocked handoff 不含可执行 command，static-only 不被描述为 loop closed；
- CLI text/JSON 与结构化 PipelineResult 一致；
- `skills/`、`.agents/skills/`、`.claude/skills/` 的 skill 内容通过 installer parity；
- `skills/README.md`、`docs/guide/project-bootstrap.md`、metadata 和 references 不再描述旧 generic suite 文件名；
- 没有新增 skip/only、弱化断言、无解释 snapshot/golden 或未授权 backend spawn；
- `.bootstrap.tmp`、`.ralph/`、git branch/worktree/remote refs 未被 pipeline 修改；
- 仓库未发现针对 `skills/` 自身的独立 Python build/typecheck 配置；因此不编造 ruff/mypy 命令，Python skill 的可执行门禁是 `.venv/bin/python -m pytest`、real CLI contract、installer parity、doc drift 和最终仓库门禁；
- 无新增第三方依赖；
- 计划外发现若改变 public API、preset resolver schema 或 target ownership，必须停止并更新 evidence/decision/unit，而不是在执行中自行扩 scope。

## 11. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| builtin manifest schema 变化 | `preset list --format json` 缺少 `source`/`name` | U1 fake + real CLI characterization | blocker，不猜 template name | 未来 CLI 版本需同步 resolver contract |
| 生成写入部分成功 | AtomicWriter 操作集或目标 symlink 异常 | U2 fault injection/dirty-tree | 只用现有 AtomicWriter，失败立即 rollback | 外部进程并发改目标仍由 writer 的 symlink guard 处理 |
| dry-run 被误报为 ready | pipeline 只看最后 stage bool | U3/U4 typed handoff assertions | level 由 handoff typed outcome 归一 | 未授权 operator 仍可能手工执行 candidate，报告必须明确风险 |
| 文档与代码再次漂移 | 只改 `skills/`，未重装副本 | U5 parity + installer test + doc drift | 源目录唯一编辑，installer 物化副本 | 外部全局安装副本不在 repo 测试范围 |
| target project facts 误报 | audit 发现空或不完整命令 | existing audit facts tests + generated guardrail assertions | 只使用 `ProjectFacts` 已证实命令；unknown stack 明示未发现 | 目标项目自己的命令定义可能运行时失败，static gate/handoff 只能报告 |

## 12. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | 5 个串行 Unit 均绑定真实入口、行为、测试和完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D6 已确定入口、resolver、写入、状态、报告边界 |
| 所有文件和接口是否有代码库证据 | 是 | 现有位置由 E1-E15 支持；新增入口/测试明确标记为新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D6 为 0.91–0.98 |
| 是否存在未处理的低置信度假设 | 否 | 没有把未确认事项写成事实；resolver 已用真实 CLI 输出验证 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 分别是入口阻塞、生成物、static gate、handoff、CLI/docs |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有 Red、测试入口、回归和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 各 Unit 明确新增测试在当前缺失入口/接线时的预期失败 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 均指定现有 contract/e2e/real CLI 回归 |
| 是否存在未来 Unit 依赖 | 否 | 依赖只沿 U1→U2→U3→U4→U5，未提前实现后续行为 |
| 是否存在泛化任务描述 | 否 | 未使用“完善逻辑/增加测试”等替代具体对象 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | B1-B12 → R → U → 测试矩阵完整映射 |
| 所有关键决策是否有 Evidence | 是 | D1-D6 均引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | 每个 Unit 的前置能力、停止条件、门禁和后续依赖已明确 |

**Product Contract preservation：**本次没有独立的 requirements-only artifact；产品范围来自当前对话，未改写既有产品契约。实现范围仅限 `ralph-project-bootstrap` skill 的编排、验证和文档可用性。
