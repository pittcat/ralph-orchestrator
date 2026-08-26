---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
type: refactor
title: "refactor: 删除 ce-executor-supervisor builtin preset"
date: 2026-08-09
---

# refactor: 删除 `ce-executor-supervisor` builtin preset

## Goal Capsule

- **目标：** 从 Ralph 的 builtin preset 产品面中删除 `ce-executor-supervisor`，使它不再能被 manifest、嵌入 registry、CLI、index、zsh completion、项目 overlay、活动测试、活动 skill 或当前用户文档发现或启动。
- **保留边界：** 保留通用 supervisor runtime、`supervisor-db` feature、wave dispatcher、SupervisorStore、origin guard、通用 supervisor lint，以及已经独立采用 `execution_model: supervisor` 的 `parallel-forge`。
- **执行方式：** 严格按 Unit 1 → Unit 2 → Unit 3 串行执行；每个 Unit 完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression 后才能进入下一个 Unit。
- **权威顺序：** 当前源码和可执行测试 > 构建/配置 > 活动文档与 skill > Git 历史。历史报告和已归档计划只作为历史记录，不是当前产品注册表。
- **停止条件：** 若删除后发现 `parallel-forge` 不能通过 supervisor 相关回归、某个活动调用方仍需要旧 preset、或未知引用改变了公开行为，立即停止当前 Unit，补充 Evidence、重评 Decision，不得临时创建兼容别名。
- **尾部所有权：** Coding Agent 负责实现与验证；本计划不授权提交、推送、发布或删除运行时 `.ralph/` 状态。

---

## Product Contract

### 0. 计划状态

- **状态：READY。** 所有实施关键决策的当前置信度均不低于 0.85；详见第 3 节。
- **基线：** 分支 `pittcat-dev`，HEAD `81881076`（`chore(docs): archive 17 completed plans from docs/plans/ to docs/achieved/plan/`）。复查后的工作树仅包含本计划新增的未跟踪文件，没有其他待处理变更；Executor仍不得重置或清理计划范围外的工作。
- **调查范围：** builtin manifest/build embedding/CLI registry/index/zsh completion；目标 preset 与 schema；专属 Rust integration/unit/BDD 测试；supervisor runtime 和 `parallel-forge`；活动 operator 文档、AGENTS/CLAUDE、`.cursor` 规则、项目 overlay；相关 Git 历史。
- **已执行的验证命令：**
  - `git status --short`
  - `git log -1 --format='%h %s'`
  - `git branch --show-current`
  - `rg --files docs/plans`
  - `rg -n 'ce-executor-supervisor|ralph\.supervisor\.yml' ...`
  - `sed` 读取 manifest、build.rs、registry、目标 preset/schema、测试、skill、文档和配置。
  - `ralph preset builtin list --format json`：确认当前 CLI 输出包含 `ce-executor-supervisor`。
  - `git log --oneline --all --follow -- presets/en/ce-executor-supervisor.yml`：确认该 preset 曾被持续维护，不能按“遗留死代码”处理。
- **尚未执行的验证：** 本次只做 Planner 调查，未运行 build、nextest、Python pytest、lint、CLI smoke、zsh 安装或完整回归；这些命令全部留在第 9 节，必须由 Executor 在代码变更后执行。
- **阻塞项：** 无。当前未确认事项仅限执行过程中重新扫出的隐藏调用方；它们不是已选实现路径的依赖。

### 1. 功能目标

- **业务目标：** 减少一个已不再作为推荐 Ralph primary path 的大型并行执行 preset 所带来的维护、文档、测试和 operator surface 成本。
- **用户/调用方：** 使用 `ralph preset ...` 查询或运行 builtin 的 CLI 用户；使用 `ralph run -H builtin:<name>` 的 shell/zsh 用户；维护 builtin catalog、preset lint 和 supervisor runtime 的开发者。
- **当前行为：** `ce-executor-supervisor` 出现在 `presets/manifest.yml`、Rust `PRESETS`、`presets/index.json` 和 zsh completion；`get_preset` 可解析它；目标 YAML/schema、项目级 `ralph.supervisor.yml`、专属 supervisor primary integration test、专属 preset lint test 和多个 BDD/结构测试都直接加载或描述它。
- **目标行为：**
  - `ralph preset builtin list --format json` 不再列出该名称。
  - `ralph run -H builtin:ce-executor-supervisor ...`、`ralph preset builtin show ce-executor-supervisor`、`ralph preset check -H builtin:ce-executor-supervisor` 不再解析为 builtin；不得引入兼容别名或隐式 fallback。
  - manifest、Rust registry、index 和 zsh completion 保持结构化一致；公开 builtin 数量从 10 变为 9。
  - `parallel-forge` 继续使用 supervisor runtime，并继续通过它已有的结构测试、BDD、wave supervisor 测试和严格 preset lint。
  - 活动文档/skill 不再把已删除 preset 当作可用入口；历史归档材料可以保留原名称作为事实记录。
- **行为差异：** 仅删除 `ce-executor-supervisor` 的可发现、可加载、可运行和活动文档引用；通用 supervisor 事件、数据库、worktree、fan-in、wave kind、lint 规则和 `parallel-forge` 行为不变。
- **本次范围：** 目标 preset YAML/schema、嵌入注册、公开索引、zsh completion、专属 overlay、专属测试与场景命名/注册、活动 Rust/skill/operator 文档引用。
- **非目标：** 不删除 `crates/ralph-core/src/supervisor/**`；不删除 `supervisor-db`；不删除 `crates/ralph-cli/src/loop_runner/wave/**`；不重构 `parallel-forge`；不删除 `preset_lint::supervisor`；不修改历史 `docs/achieved/**`、`docs/report/**`、`docs/reports/**`、`docs/handoffs/**`、`docs/solutions/**` 或历史 `docs/plans/**`，除非执行前重新确认它们是活动入口而非历史记录；不清理 `.ralph/` 运行时状态。
- **输入：** builtin name、preset catalog、活动文档/skill 引用、现有 supervisor/parallel-forge 测试配置。
- **输出：** 删除后的 builtin catalog、unknown-preset 解析结果、同步后的活动文档/skill、保留下来的 supervisor/parallel-forge 验证结果。
- **状态变化：** `PRESETS`/manifest/index/zsh 的可见集合减少一项；build.rs 不再复制目标 YAML/schema；目标 overlay 和专属 fixtures 不再成为活动文件；运行时 supervisor 状态模型无变化。
- **错误语义：** 旧 builtin name 进入 preset 解析入口时返回现有 unknown/missing preset 的非零错误路径；不得改成静默回退到 `parallel-forge` 或 `ce-executor-pipeline`。具体错误文本沿用现有 CLI 行为，只要求非零和 unknown 语义，不新增文本 snapshot。
- **兼容性要求：** 不兼容旧 builtin 名称，这是明确的删除行为；仍支持的 builtin 名称和旧的 supervisor/parallel-forge 配置格式保持原行为。
- **性能要求：** 删除后 build 不再嵌入目标 YAML/schema；不增加 runtime 路径、数据库迁移或新依赖。没有额外性能阈值。
- **安全/权限要求：** 不放宽 event policy、hat scope、origin guard 或 supervisor coordination topic 权限；删除旧入口不得把普通 agent 事件升级为 system-injected 事件。
- **已知约束：** builtin 真正的 single source of truth 是 `presets/manifest.yml`；Rust registry、index、schema、zsh 和活动文档必须同步；测试入口必须使用 `cargo nextest run` 系列，Python 测试必须使用 `skills/.venv/bin/python`。
- **已确认假设：** `parallel-forge` 是当前保留的 supervisor-enabled builtin；专属 old preset tests 不代表通用 runtime API；目标 overlay 没有仓库内其他调用方。
- **待验证假设：** 是否有仓库外脚本仍调用旧名称。验证方法是实现后在仓库内执行全量 `rg`，并让 Executor 在发现仓库外依赖时停止请求用户决定；这不阻塞当前仓库内的删除计划。

---

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口

- **外部入口：** CLI 的 builtin preset 命令和 `ralph run -H builtin:<name>`；Rust registry 在 `crates/ralph-cli/src/presets.rs` 的 `PRESETS`、`get_preset`、`list_presets`、`preset_names` 提供解析和列表。
- **构建调用链：** `presets/manifest.yml` 的 `embedded` → `crates/ralph-cli/build.rs` 读取 `presets/en/<name>.yml` 并合并 `presets/schemas/<name>.yml` → `$OUT_DIR/presets/<name>.yml` → `presets.rs` 的 `include_str!` → CLI registry。
- **用户可见 catalog：** `presets/index.json` 提供 public names/description/category；`scripts/ralph-zsh-plugin.zsh` 提供 `builtin:<name>` 补全；Rust tests 交叉检查 index、manifest、PRESETS 和 zsh。
- **目标 preset 边界：** `presets/en/ce-executor-supervisor.yml` 与 `presets/schemas/ce-executor-supervisor.yml` 是目标 preset 的 canonical YAML/schema；`ralph.supervisor.yml` 是仓库根目录明确命名为该 preset overlay 的 operator 配置。
- **通用 supervisor 边界：** `crates/ralph-core/src/supervisor/**`、`crates/ralph-cli/src/loop_runner/wave/**`、`event_loop.supervisor.enabled`、`supervisor-db` feature；`presets/en/parallel-forge.yml` 明确启用 `execution_model: supervisor`、isolated mode 和 `event_loop.supervisor.enabled: true`。
- **专属测试边界：** `crates/ralph-cli/tests/integration_supervisor_primary.rs`、`crates/ralph-core/src/preset_lint/supervisor_preset_test.rs`、`wave.rs` 中的目标 lease test、`handoff_dispatch.rs` 中 include 目标 YAML 的测试，以及 `supervisor/ce_executor_supervisor_minimal.yml` 对应的注册测试直接绑定目标 preset。
- **测试与构建方式：** Rust 使用 `cargo nextest run` 系列，完整入口是 `./scripts/run-tests.sh`；builtin 严格校验使用 `./scripts/validate-builtin-presets.sh --strict`；skill tests 使用 `skills/.venv/bin/python -m pytest skills/tests -q`；CLI 文档 drift 使用 `scripts/check-cli-doc-drift.sh`。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `presets/manifest.yml`、`crates/ralph-cli/build.rs`、`crates/ralph-cli/src/presets.rs`、`ralph preset builtin list --format json` | 目标 preset 是 manifest embedded 项、build 输入、Rust embedded public 项和 CLI 可见项。 | 必须同时删除 YAML/schema、manifest 项、Rust entry，不能只删单一文件。 | 高 |
| E2 | `presets/index.json`、`scripts/ralph-zsh-plugin.zsh`、`presets.rs` 中 `presets_array_matches_manifest`/index/zsh parity tests | public index、zsh completion 和 Rust registry 有交叉校验；当前 public count/assertion 为 10。 | 删除后必须同步 index、zsh 和 count/集合断言，目标 count 为 9。 | 高 |
| E3 | `presets/en/ce-executor-supervisor.yml`、`presets/schemas/ce-executor-supervisor.yml`、`ralph.supervisor.yml` | 目标 preset 有独立 YAML、schema 和明确命名的项目级 overlay；overlay 的唯一仓库内引用是自身文档。 | 这些是目标产品面，应删除；不得迁移为第二个 supervisor 默认配置。 | 高 |
| E4 | `presets/en/parallel-forge.yml`、`crates/ralph-cli/src/presets.rs` 中 parallel-forge tests、`crates/ralph-core/tests/scenarios.rs`、`wave_supervisor/**` | `parallel-forge` 已独立声明 supervisor execution model、enabled supervisor、isolated mode，并已有真实 runtime/BDD/wave 覆盖。 | 保留通用 supervisor 和 parallel-forge；其回归是删除后的保留能力验收。 | 高 |
| E5 | `integration_supervisor_primary.rs`、`preset_lint/supervisor_preset_test.rs`、`wave.rs` target lease test | 这些测试直接 include/启动/断言 `ce-executor-supervisor` 或其 schema，不能在目标文件删除后继续作为活动测试。 | 删除目标专属测试；不把旧 topology 伪装成 parallel-forge 测试。 | 高 |
| E6 | `crates/ralph-core/tests/scenarios.rs` 与 `tests/scenarios/supervisor/ce_executor_supervisor_*.yml` | supervisor 目录有三个带目标名称的场景；其中 minimal 走真实 `InMemoryCoordinatorBridge`，另外两个是 fixture-neutral 的 origin/event 测试。 | 保留行为覆盖但改为 generic supervisor 场景名；同步 test registration 和 bridge 注释，避免保留删除对象名称。 | 高 |
| E8 | `crates/ralph-core/src/supervisor/**`、`crates/ralph-cli/src/loop_runner/wave/**`、`integration_supervisor_runtime_p0.rs` | supervisor store/bridge/worker env/runtime fixture 是通用层；runtime P0 fixture 只有 env source 字符串绑旧名称。 | 不删通用 fixture；将其 source 改为仍存活的 `builtin:parallel-forge` 或改成可注入 generic source，保持测试语义。 | 高 |
| E9 | `AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc`、`README.md`、`docs/guide/presets.md`、`docs/guide/project-usage.md` | 活动 operator/product 文档列出或推荐旧 preset；AGENTS/CLAUDE 还把其 topology 当当前 builtin 描述。 | 同步删除活动入口和推荐；保留 generic supervisor/parallel-forge 说明。 | 高 |
| E11 | `AGENTS.md` Build & Test、`skills/README.md`、`crates/*/Cargo.toml`、`scripts/run-tests.sh` | 仓库规定 Rust 用 nextest、Python 用 `.venv`、全量入口是 `./scripts/run-tests.sh`。 | 计划中的验证命令必须遵守这些入口，禁止裸 `cargo test -p ralph-cli`。 | 高 |
| E12 | `git status --short`（计划写入后复查） | 工作树仅显示本计划新增文件，没有其他待处理变更。 | Executor 只能修改本计划列出的文件，不得重置或清理计划范围外的工作。 | 高 |
| E13 | `crates/ralph-core/src/capability_inventory.rs` | `supervisor-emit` capability 的 evidence source 指向已删除目标 schema。 | 将 evidence source 改到仍存在且启用 supervisor 的 `presets/en/parallel-forge.yml`，保持 compile-time inventory 可构建。 | 高 |
| E14 | `git log --oneline --all --follow -- presets/en/ce-executor-supervisor.yml` | 目标 preset 最近仍有维护提交。 | 删除必须被视为明确的产品 surface removal，而非猜测的死代码清理；需完整检查调用方和回归。 | 中 |

#### 2.3 受影响范围

| 范围 | 已确认位置 | 变更边界 |
|---|---|---|
| 生产/构建 | `presets/en/ce-executor-supervisor.yml`、`presets/schemas/ce-executor-supervisor.yml`、`presets/manifest.yml`、`crates/ralph-cli/build.rs`、`crates/ralph-cli/src/presets.rs` | 删除目标输入/entry，保留 build/parity 机制。 |
| 测试 | `crates/ralph-cli/src/presets.rs`、`integration_supervisor_primary.rs`、`integration_supervisor_runtime_p0.rs`、`loop_runner/tests/wave.rs`、`preset_lint/supervisor_preset_test.rs`、`event_loop/tests/handoff_dispatch.rs`、`tests/scenarios.rs` | 删除/改名目标专属测试，保留 generic supervisor/parallel-forge tests。 |
| 配置/数据 | `ralph.supervisor.yml`、`presets/index.json`、三份 supervisor 场景 YAML | 删除 overlay；更新 catalog 和 generic fixture 文件名。 |
| CLI/API | builtin list/show/run/preset check；registry parity tests | 旧 name 变为 unknown；其他 builtin 不变。 |
| UI/操作面 | `scripts/ralph-zsh-plugin.zsh`、README、guide、AGENTS/CLAUDE、`.cursor` | 去除旧 completion/推荐/拓扑说明。 |
| 保留调用方 | `parallel-forge`、`crates/ralph-core/src/supervisor/**`、`crates/ralph-cli/src/loop_runner/wave/**`、`wave_supervisor/**` | 只做名称/注释清理或回归，不删除 runtime 能力。 |
| 历史资料 | `docs/achieved/**`、`docs/report/**`、`docs/reports/**`、`docs/handoffs/**`、`docs/solutions/**`、历史 `docs/plans/**` | 默认只读；不得为了全局 `rg` 零结果而改写历史。 |

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 删除到什么边界？ | 只删 YAML；删 catalog；删全部活动产品面 | 删除目标 YAML/schema、catalog、registry、index、zsh、overlay、专属 tests/fixtures、活动文档/skill 引用；历史资料保留 | E1、E2、E3、E5、E6、E7、E9、E10、E14 | 只删 YAML 会让 build/include/parity 或活动入口漂移；改历史资料会扩大范围且不改变 runtime | 0.97 |
| D2 | 是否删除 supervisor runtime？ | 连 supervisor runtime 一起删；保留 runtime 并迁移旧 preset；保留 runtime 与 `parallel-forge` | 保留 runtime、feature、dispatcher、store、lint 和 `parallel-forge` | E4、E8 | `parallel-forge` 当前真实启用 supervisor；删除 runtime 会破坏仍受支持的 builtin | 0.99 |
| D3 | 旧名称是否保留 alias/fallback？ | alias 到 `parallel-forge`；alias 到 pipeline；完全 unknown | 完全 unknown，沿用现有 unknown preset 错误路径 | 目标是删除 builtin，且 `get_preset`/CLI registry 没有 alias 层；兼容旧名称不是本次要求 | alias 会让已删除产品仍可运行并掩盖调用方迁移问题 | 0.95 |
| D4 | 目标专属测试如何处理？ | 全部保留并改 path；全部删除；generic runtime 场景改名保留，旧 topology/lease/primary test 删除 | generic supervisor BDD/origin 场景改名保留；旧 preset primary integration、preset-lint topology pin、旧 preset lease test 删除 | E5、E6、E8、E4 | `parallel-forge` hat topology 不等于旧 preset，不能机械改 path；通用 store/runtime 已有独立覆盖 | 0.94 |
| D6 | capability inventory 的 evidence source 放哪里？ | 删除 source；保留已删除 path；指向 surviving supervisor preset | 指向 `presets/en/parallel-forge.yml`，保留 `supervisor-emit` capability | E4、E13 | 删除 source 会让 capability 失去可审计证据；已删除 path 会令 compile-time artifact 悬空 | 0.96 |
| D7 | supervisor BDD 场景是否删除？ | 删除全部；保留目标名称；保留行为并改为 generic supervisor 文件名 | 保留行为，重命名 `ce_executor_supervisor_*` fixtures/registration 为 generic supervisor 名称，并同步 bridge 注释 | E6、E8 | 这些场景中 minimal 真实驱动 supervisor coordinator；删除会降低通用 runtime 证据 | 0.92 |
| D8 | 活动文档和 skill 是否要求仓库内旧字符串为零？ | 全仓库零字符串；只清理活动引用；只改代码 | 清理活动入口/代码/skill/docs；历史资料保留原字符串 | E9、E10、项目历史目录用途 | 全仓库零字符串会篡改审计历史；只改代码会留下用户可执行的失效入口 | 0.97 |

所有决策均达到 0.85；没有需要带入 Executor 的未决架构选择。若执行中新发现证据使任一置信度低于 0.85，必须按 Unit 的停止条件回退调查，不得自行选新方案。

### 4. BDD 行为规格

```gherkin
Feature: 删除 ce-executor-supervisor builtin preset

  Background:
    Given 当前 checkout 使用 manifest 驱动的 builtin registry
    And `parallel-forge` 是仍保留的 supervisor-enabled builtin

  Scenario: builtin 列表不再公开已删除 preset
    Given builtin catalog 已完成删除
    When 用户运行 `ralph preset builtin list --format json`
    Then 返回的公开名称不包含 `ce-executor-supervisor`
    And 返回的公开名称包含 `parallel-forge`
    And 公开名称数量为 9

  Scenario: 已删除 preset 不再解析
    Given 用户传入 `builtin:ce-executor-supervisor`
    When CLI 解析 preset 用于 show、check 或 run
    Then 命令以非零状态结束
    And 结果使用现有 unknown/missing preset 错误语义
    And 不会静默切换到另一个 preset

  Scenario: builtin catalog 的四方注册保持一致
    Given manifest、Rust PRESETS、index.json 和 zsh completion 已更新
    When 运行现有 manifest/index/zsh parity tests
    Then 所有 public names 在四个注册面一致
    And hidden `merge-loop` 仍不出现在 public index/zsh values

  Scenario: 保留的 parallel-forge 继续通过 supervisor
    Given `parallel-forge` 配置包含 isolated mode 和 `event_loop.supervisor.enabled: true`
    When 运行 parallel-forge 的结构测试、BDD 场景和 wave supervisor 回归
    Then supervisor wave dispatch/fan-in/失败恢复的既有可观察结果仍通过
    And 不产生针对 `ce-executor-supervisor` 的 fallback 或 alias

  Scenario: 通用 supervisor BDD 场景仍可驱动真实 coordinator
    Given generic supervisor minimal fixture 使用 `InMemoryCoordinatorBridge`
    When worker 发布 `exec.unit.done`
    Then runtime 产生 `exec.wave.complete`
    And downstream integrator 行为仍按 fixture 断言工作

    Given bootstrap 使用仍存活的 builtin 或任意 preset
    When preset check/preflight 返回未被通用规则允许的 finding
    Then static gate 保持 blocked
    And 不因 preset 名称获得旧 supervisor 专属放行

  Scenario: 活动 operator 文档不再提供失效入口
    Given用户查阅活动 README、guide、AGENTS/CLAUDE、`.cursor` 规则或 zsh completion
    When用户寻找 builtin preset
    Then看不到 `ce-executor-supervisor` 作为可运行/可补全入口
    And能找到 `parallel-forge` 的保留 supervisor 入口或当前 builtin catalog
```

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 列表删除 | CLI JSON 无目标名、含 parallel-forge、public count 为 9；无文件/registry 副作用 | `crates/ralph-cli/src/presets.rs` registry tests + CLI smoke | Rust unit/integration | 结构化 manifest/index/zsh parity | 否 |
| S2 unknown 解析 | `get_preset` 对旧 name 返回 None；CLI 非零且不 fallback | `crates/ralph-cli/src/presets.rs` 新增/调整稳定的 unknown-name contract test；`ralph preset ...` smoke | 单元 + CLI integration | 不断言易变全文，只断言非零与 unknown 语义 | 否 |
| S3 四方 parity | manifest、PRESETS、index、zsh values 全部一致；hidden preset 规则仍成立 | 现有 `presets_array_matches_manifest`、`test_public_preset_names_in_index_json`、`test_index_json_entries_have_zsh_completion`、`test_zsh_builtin_completion_arrays_consistent` | 结构化集成测试 | `./scripts/validate-builtin-presets.sh --strict` | 否 |
| S4 parallel-forge 保留 | supervisor enabled/isolated 配置、flow、payload、wave failure/success 场景和 `wave_supervisor` 测试通过 | `crates/ralph-cli/src/presets.rs` parallel-forge tests；`crates/ralph-core/tests/scenarios.rs` `parallel_forge_*`；`wave_supervisor` | 集成/BDD | supervisor-db branch 与 `integration_worktree_isolation`/resume 回归 | 不新增 E2E；已有真实 runner 覆盖足够 |
| S5 generic supervisor BDD | generic minimal 场景真实经过 EventLoop/coordinator，不是 stub；fan-out/origin guard 既有断言通过 | `crates/ralph-core/tests/scenarios.rs` generic renamed scenarios | BDD integration | 运行 `-p ralph-core --test scenarios` 相关子集 | 否 |
| S7 活动入口清理 | active source/docs/skills 不再把旧名称作为可运行入口；历史目录不改 | `rg` audit + `scripts/check-cli-doc-drift.sh` + skill contract tests | 静态 contract/integration | 检查生成 stem、overlay、zsh install/load | 否 |

每项验收必须同时断言主结果、副作用和不变量：不得生成旧 preset 文件、不得删除 `parallel-forge`、不得改变 supervisor DB schema/runtime、不得新增 skip/only/弱断言。由于本次是删除/重命名和 catalog 行为变更，不引入 snapshot、golden、property-based、mutation 或 fuzz；这些测试对本次风险没有额外收益。

### 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 删除目标 preset 的 public/embedded/catalog 入口 | S1、S3 | registry count/list + parity | `list_presets`、`preset_names` | manifest/index/zsh tests | 否 | E1、E2 |
| R2 | 旧名称必须 unknown，不得 fallback | S2 | old-name unknown contract + CLI smoke | `get_preset` None | preset command integration | 否 | E1、D3 |
| R3 | 删除 target YAML/schema/overlay 与专属测试输入 | S3、S7 | build + `rg` active audit | include/parity compile | strict builtin validation | 否 | E3、E5 |
| R4 | 保留 supervisor runtime 和 `parallel-forge` | S4、S5 | parallel-forge structured/BDD/wave tests | existing supervisor store/dispatcher tests | scenarios + supervisor-db integration | 否 | E4、E8 |
| R5 | 删除旧 preset 专属 E2E gate 豁免 | S6 | approved finding no longer accepted by name | gate helper tests | full skill contract suite | 否 | E7、D5 |
| R6 | 活动文档/skill/zsh 不提供失效入口，历史记录不改 | S7 | active `rg` allowlist + help/doc drift + zsh load | none beyond existing parity | skill tests | 否 | E9、E10、D8 |

---

## Implementation Units

### Unit 1：删除 builtin 注册面并建立旧名称 unknown 契约

#### 1. Unit 目标

使 `ce-executor-supervisor` 从 build 输入、Rust registry、public index、zsh catalog 和项目 overlay 中消失；CLI 对旧名称走现有 unknown 语义。只处理 builtin 注册/输入面，不改 supervisor runtime。

#### 2. 对应需求与 Scenario

- Requirements：R1、R2、R3。
- Scenarios：S1、S2、S3。
- Decisions：D1、D2、D3、D6。
- Evidence：E1、E2、E3、E13。

#### 3. 外部可观察结果

- `ralph preset builtin list --format json` 返回 9 个 public preset，缺少旧名称且包含 `parallel-forge`。
- `ralph preset builtin show ce-executor-supervisor`、`ralph preset check -H builtin:ce-executor-supervisor` 与 `ralph run -H builtin:ce-executor-supervisor` 不再解析成功；不得执行另一个 preset。
- build 仍能从 manifest 生成所有剩余 builtin，`supervisor-emit` capability 的 evidence source 指向仍存在的 `parallel-forge`。

#### 4. 当前行为基线

E1/E2 证明目标目前可列出、可解析并被嵌入；E3 证明 build 依赖目标 YAML/schema。现有 positive count/name assertions 代表旧行为。进入实现前先把 S2 的 unknown contract test 写成 Red；在删除 entry 前它必须因为目标仍可解析而失败。

#### 5. 输入与输出

- 输入：manifest embedded name、目标 YAML/schema、Rust `PRESETS` entry、index entry、zsh value、overlay。
- 输出：9 项 public catalog；旧 name 的现有 unknown/missing error；剩余 preset embedded contents 正常。
- 错误：旧 name 非零 unknown；build 若任一剩余 manifest/PRESET/schema 不一致必须失败。
- 状态/副作用：不生成目标 `$OUT_DIR` preset；不创建 alias；不改 `.ralph/`。
- 不变量：`merge-loop` hidden 规则不变；`parallel-forge` entry、YAML、schema merge 和 `supervisor-db` 不变。

#### 6. 修改位置

| 位置 | 当前职责 | 预计修改边界 | 明确不修改 |
|---|---|---|---|
| `presets/manifest.yml` | build embedded allow-list | 删除目标项及只描述该项的注释 | 其他 embedded/hidden 条目 |
| `presets/en/ce-executor-supervisor.yml` | 目标 canonical preset | 删除文件 | 其他 `presets/en/**` |
| `presets/schemas/ce-executor-supervisor.yml` | 目标 schema SSOT | 删除文件 | 其他 schema |
| `crates/ralph-cli/src/presets.rs` | embedded registry、列表和 parity tests | 删除 `EmbeddedPreset`；更新 public count/name/zsh set；增加/调整 old-name unknown contract | generic registry/parity、parallel-forge tests |
| `presets/index.json` | public catalog | 删除目标对象 | 其他 public descriptions |
| `scripts/ralph-zsh-plugin.zsh` | builtin completion values/descriptions | 删除目标 value/对应说明 | completion style、其他 values |
| `ralph.supervisor.yml` | 目标 preset 项目 overlay | 删除整个明确绑定旧 preset 的文件 | 通用 runtime/config types |
| `crates/ralph-core/src/capability_inventory.rs` | compile-time capability evidence | 将目标 schema source 替换为 `presets/en/parallel-forge.yml` | capability IDs/coverage 算法 |

#### 7. 可依赖能力

- 当前 `PRESETS`/manifest/index/zsh parity tests。
- `get_preset` 对不存在名称的现有返回路径。
- build.rs 的 manifest/schema merge 和 embedded parity。
- `parallel-forge` 的现有 builtin 文件与 registry entry。

#### 8. 禁止依赖的未来能力

- 不提前删除/修改 `crates/ralph-core/src/supervisor/**` 或 wave dispatcher。
- 不把旧名称 alias 到 `parallel-forge`。
- 不在本 Unit 清理 active skill、历史文档或 generic BDD 文件名；这些属于 Unit 2/3。
- 不顺便更新与本删除无关的 builtin 描述或 count 历史。

#### 9. 验收测试

- **S2-AT1：** 在删除 entry 前，将现有 registry contract 改为对旧名称断言 `get_preset` 返回 None；动作是运行该单测；当前预期 Red 是“目标仍存在/返回 Some”。删除 entry 后 Green；副作用断言是 `parallel-forge` 仍可 `get_preset`。
- **S1-AT1：** 删除后运行 `ralph preset builtin list --format json`；断言 JSON 无旧名称、含 `parallel-forge`、长度 9。
- **S3-AT1：** 运行现有 manifest/index/zsh parity tests；断言无缺项、无多余 completion、hidden `merge-loop` 仍不 public。
- **Build-AT1：** 运行 `cargo build -p ralph-cli`；断言 build 不因缺失 target include/schema 而失败。

#### 10. Acceptance Red

1. 先只加入/调整 S2 unknown contract test，不删除生产 registry；运行 targeted nextest。
2. 有效 Red 必须是测试实际拿到 `Some(EmbeddedPreset)`，说明旧名称仍被 registry 解析。
3. `编译失败、测试路径拼错、fixture 缺失、无测试执行` 都不是有效 Red，必须先修复测试前置。

#### 11. 单元测试拆分

- `get_preset` old name：输入 `ce-executor-supervisor`，期望 `None`；不 Mock registry。
- `get_preset` surviving name：输入 `parallel-forge`，期望非空且 content 非空；这是删除误伤的 guard。
- `list_presets`：期望 public length 9，不包含旧 name。
- `preset_names`：期望 length 9，保留所有现有 surviving names。
- manifest/index/zsh parity：使用真实文件读取和现有结构比较；不使用字符串全文等价测试。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：旧 name unknown contract，因 target entry 仍存在而失败。
2. 最小实现：删除 manifest/PRESET/index/zsh/目标文件/overlay，并替换 capability evidence source。
3. Test 1 Green：旧 name 返回 None；parallel-forge 仍返回 Some。
4. Test 2 Red：更新 10→9 的 list/names/parity 断言后，若仍有旧集合/数量会失败。
5. 最小实现：只同步受影响的 count、集合和 zsh values。
6. Test 2 Green：registry/parity 全绿。
7. Refactor：删除目标专属注释，保留 manifest/build 的通用说明；不得做无关格式化。

#### 13. 最小实现范围

- 必须删除目标 canonical YAML/schema、manifest/Rust/index/zsh/overlay surface。
- 必须使 old name 走现有 unknown 解析路径。
- 必须更换 capability inventory 的悬空 evidence source。
- 必须保持 hidden preset、remaining registry、build schema merge 不变。
- 不实现别名、迁移提示新机制、runtime 删除或新依赖。

#### 14. 集成验证

- 联合 `build.rs`、manifest、Rust `PRESETS`、index、zsh parity 和 capability inventory；这些边界必须真实读取文件。
- 不 Fake build embedding；可以使用现有 unit harness 验证 `get_preset`。
- 命令：`cargo nextest run -p ralph-cli --bin ralph -- preset`、`cargo nextest run -p ralph-core -- capability_inventory`（若 substring 无匹配，则运行对应 package 的已确认测试目标，不得把“无匹配”当成功）和 `cargo build -p ralph-cli`。
- 预期：所有剩余嵌入 preset parse；无目标 include/schema 路径错误；失败不得进入 Unit 2。

#### 15. 风险驱动测试

- **Characterization：** S2 先固定旧 name 当前“可解析”行为，再转为删除后的 unknown，证明 Red 不是空测试。
- **Contract：** registry/index/zsh parity 是删除最容易遗漏的结构契约。
- 不增加 Fuzz/Property/Concurrency：输入是静态 catalog，不存在这些风险。

#### 16. 回归范围

- 直接：`crates/ralph-cli/src/presets.rs` preset registry tests、build、preset command tests。
- 相邻：`crates/ralph-core/src/capability_inventory.rs` compile-time includes、所有 embedded preset parse/strict lint。
- 公开消费者：CLI list/show/check/run、zsh completion。
- 配置/数据：manifest/index/schema source；不改旧 runtime data。
- default/feature：`cargo build -p ralph-cli` 默认 feature；Unit 2 再验证 supervisor-db。
- Lint/typecheck：Unit close 必须执行 `cargo fmt --check`、`cargo clippy -p ralph-cli --all-targets --all-features` 和 build。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/ce-executor-supervisor.yml` | 删除现有生产文件 | 删除 canonical target preset | E3 |
| `presets/schemas/ce-executor-supervisor.yml` | 删除配置文件 | 删除 target schema SSOT | E3 |
| `ralph.supervisor.yml` | 删除配置文件 | 删除 target-only overlay | E3 |
| `presets/manifest.yml` | 修改配置 | 删除 embedded entry | E1 |
| `crates/ralph-cli/src/presets.rs` | 修改现有生产/测试文件 | registry、count、unknown/parity | E1/E2 |
| `presets/index.json` | 修改配置 | 删除 public entry | E2 |
| `scripts/ralph-zsh-plugin.zsh` | 修改 operator 文件 | 删除 completion value | E2 |
| `crates/ralph-core/src/capability_inventory.rs` | 修改现有生产文件 | 替换悬空 evidence source | E13 |

#### 18. 完成标准

- S1/S2/S3、Unit 1 unit/integration tests、build、lint、typecheck 通过。
- 无新增 skip/only、无削弱断言、无 alias/fallback。
- `parallel-forge` registry entry 未被删除。
- Evidence/Decision 记录在执行报告或 commit notes 中更新，置信度不下降到 0.85 以下。
- Unit 1 可独立提交；工作树计划外变更未被覆盖或清理。

#### 19. 停止条件

停止于：manifest 与 build 行为冲突；`get_preset` 不存在或 unknown 语义不是现有路径；parallel-forge entry 被意外删除；Red 不是目标缺失导致；capability inventory 无可用 surviving source；发现新公开调用方；需要新依赖；或回归范围超出 catalog deletion。

停止后执行：记录新 Evidence → 更新 D1/D2/D3/D6 → 重算置信度 → 修订 Unit 1/后续 Unit；不得继续删除。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| build include 悬空 | 只删 manifest 或只删 YAML | `cargo build -p ralph-cli` | 同步删除 manifest/PRESET/schema 并先跑 Red | 低 |
| 误删 surviving builtin | index/zsh 集合机械重写 | registry parity + parallel-forge test | 只删除目标集合项，保留 surviving assertions | 低 |
| capability evidence 悬空 | 删除 schema 未更新 inventory | core compile/test | 使用 `parallel-forge.yml` | 低 |
| 用户仍运行旧 name | alias 或未知脚本 | CLI smoke + active rg | 明确 unknown，不静默 fallback | 仓库外调用方未知 |

### Unit 2：移除目标专属测试面并保留通用 supervisor 行为

#### 1. Unit 目标

删除/重命名所有活动测试中对旧 preset topology、YAML 或 schema 的直接依赖，同时让 generic supervisor BDD 场景和 `parallel-forge` 相关 runtime 覆盖继续有效。一个可观察行为是：删除旧 preset 后，通用 supervisor/parallel-forge 测试仍能真实执行，而不会因为旧文件缺失而失去 coverage。

#### 2. 对应需求与 Scenario

- Requirements：R3、R4。
- Scenarios：S4、S5。
- Decisions：D2、D4、D7。
- Evidence：E4、E5、E6、E8。

#### 3. 外部可观察结果

- 不再编译/运行 `integration_supervisor_primary.rs` 的旧 preset primary path、旧 preset lint topology pin、旧 preset worker lease pin 或旧 handoff include test。
- generic supervisor minimal/fanout/review-origin BDD 场景仍注册、执行真实 EventLoop/coordinator，并保持原有 event assertions。
- `parallel-forge` structured tests、BDD scenarios、wave supervisor tests、worktree/resume tests仍通过。
- `integration_supervisor_runtime_p0.rs` 不再注入已删除的 `RALPH_HATS_SOURCE` 值；它仍验证 SupervisorStore/bridge/worker env 的通用边界。

#### 4. 当前行为基线

E5 证明多个测试直接依赖目标文件；E6 证明 supervisor 场景中 minimal 真实驱动 coordinator，不能全部删除；E4/E8 证明有 surviving coverage。进入实现前，先将 generic场景注册/目标专属测试的预期调整拆成真实 Red：在删除目标 YAML 后，旧 include/路径测试应因文件不存在而失败；这是删除专属测试而非修生产代码的有效 Red。

#### 5. 输入与输出

- 输入：目标专属 integration/unit tests、旧名称场景文件/注册、runtime P0 env fixture、generic flow comments。
- 输出：删除目标 primary/lint/lease/handoff 测试；generic 场景改为 `supervisor_*` 名称并保持行为；P0 fixture 使用 surviving generic source。
- 错误：若 generic supervisor/parallel-forge 测试因清理而缺失或行为改变，Unit 失败。
- 副作用：测试文件/fixture rename 需同步 `scenarios.rs` 和注释；不改 supervisor 生产算法。
- 不变量：system-injected coordination 仍由 runtime 产生；agent 不可伪造 `*.wave.complete`；parallel-forge flow 不变。

#### 6. 修改位置

| 位置 | 当前职责 | 预计修改边界 | 明确不修改 |
|---|---|---|---|
| `crates/ralph-cli/tests/integration_supervisor_primary.rs` | 旧 preset primary fake-backend integration | 删除整个 target-only test file | `integration_supervisor_runtime_p0.rs`、`wave_supervisor/**` |
| `crates/ralph-core/src/preset_lint/supervisor_preset_test.rs` | 旧 preset YAML/schema topology pins | 删除整个 target-only test module文件，并从 `preset_lint/mod.rs` 删除 `mod supervisor_preset_test` | `preset_lint/supervisor.rs` 和 generic lint tests |
| `crates/ralph-cli/src/loop_runner/tests/wave.rs` | wave runner tests | 删除只读取目标 YAML 的 KTD7 lease test block | 其他 wave tests |
| `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs` | handoff/flow tests | 删除只 include 旧 YAML、只断言旧 `task-planner` topology 的 test | generic handoff tests |
| `crates/ralph-core/tests/scenarios.rs` | BDD registration | 删除旧 primary test registration；将 generic supervisor fixture/test function/file references改为新 generic names | `parallel_forge_*` tests |
| `crates/ralph-core/tests/scenarios/supervisor/ce_executor_supervisor_minimal.yml` | real coordinator minimal fixture | rename to generic `supervisor_minimal.yml`，内容/断言语义保持 | event payload contract |
| `.../ce_executor_supervisor_exec_wave_fanout.yml`、`...review_batch.yml` | fixture-neutral supervisor/origin scenarios | rename to generic names并去除注释中的 target name | scenario topology/expected events |
| `crates/ralph-core/src/supervisor/bridge.rs` | BDD bridge docs | 更新 scenario name/通用描述 | bridge implementation |
| `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs` | generic store/worker runtime fixture | 将硬编码 `RALPH_HATS_SOURCE` 改为 `builtin:parallel-forge` 或 fixture 参数；不改变 env contract assertions | store/bridge logic |
| `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage/tests.rs`、`fail_close_flow_authority.rs`、`progress_steward.rs` | generic runtime tests/comments | 删除旧 preset name，改为 generic supervisor-enabled fixture wording | test fixtures/behavior |

#### 7. 可依赖能力

- Unit 1 已验证的 manifest/registry 不再包含目标。
- 现有 `run_workflow_guard_scenario` 真实 EventLoop runner、`InMemoryCoordinatorBridge`、generic scenario loader。
- 现有 `parallel-forge` tests 和 `wave_supervisor` unit tests。
- 允许使用现有 InMemory/Rusqlite store，不新增 mock 绕过真实 coordinator。

#### 8. 禁止依赖的未来能力

- 不把旧 `integration_supervisor_primary` 改造成 parallel-forge fake-backend E2E；两者 topology 不同。
- 不删除 generic `integration_supervisor_runtime_p0`，不删除 supervisor store/bridge。
- 不用“改名”掩盖旧 preset 专属 topology 断言；无法泛化的 test 必须删除并由 surviving coverage 证明保留能力。

#### 9. 验收测试

- **S5-AT1：** 运行 generic renamed minimal/fanout/review scenarios；断言真实 `exec.wave.complete`、event counts、forged coordination event absent 等原有行为。
- **S4-AT1：** 运行 `parallel-forge` preset structured tests、BDD branch/failure scenarios 和 `wave_supervisor` tests；断言 supervisor enabled、wave success/failure、worktree/fan-in contract 仍通过。
- **T2-AT1：** 运行 target-specific test selectors，确认已删除 test module 不再被 Cargo 注册；不能以“测试不存在”作为唯一成功证据，必须同时通过 generic/surviving coverage。
- **T2-AT2：** 运行 `integration_supervisor_runtime_p0`；断言 captured env、store snapshot、slot binding、idempotency/terminal state assertions 仍通过，且 source 不含旧 name。

#### 10. Acceptance Red

1. 先删除 Unit 1 的 target YAML/schema 后运行受影响 selectors，旧 include-based tests 必须因文件不存在/target module 依赖而失败。
2. 对 generic scenario rename，先只改注册路径不改 fixture内容；旧路径不存在应产生 fixture load failure，证明测试确实执行到目标 fixture。
3. 有效 Red 不能是 cargo test 命令错误、未执行测试、或删除测试后没有任何 surviving assertion；若出现，停止并修正测试编排。

#### 11. 单元测试拆分

- generic scenario loader/path：新路径可读，旧 target path 不再注册。
- real coordinator fan-in：`exec.unit.done` 后仍产生 `exec.wave.complete`；不得 Mock system-injected event。
- origin guard：agent forged `review.wave.complete` 仍被拒绝，合法 `review.complete` 保留。
- P0 env fixture：source 使用 surviving preset/generic source；其余 `RALPH_*` channel/worktree fields 不变。
- parallel-forge structured contract：使用现有 tests，不复制旧 preset topology 文本，不把目标 preset schema当 fixture。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：删除目标输入后运行旧 include-based test selectors，记录真实缺失文件/module failure。
2. 最小实现：删除 `integration_supervisor_primary.rs`、target-only `supervisor_preset_test` module、old lease/handoff test block。
3. Test 1 Green：旧专属 selector不再进入编译，core/cli测试目标可构建。
4. Test 2 Red：将 generic fixture 文件名/registration改为新名称但暂不同步 loader/bridge comment，scenario loader failure/引用 failure出现。
5. 最小实现：同步 scenario YAML rename、`scenarios.rs` registration/test names、bridge comments。
6. Test 2 Green：generic scenarios真实运行并保持 events/absent assertions。
7. Test 3 Red：将 P0 source 改为 surviving source后运行 P0 selector，若断言/fixture仍绑定旧值则失败。
8. 最小实现：只更新 source fixture 或参数化 source，保留 env/store behavior。
9. Test 3 Green：P0 tests通过。
10. Refactor：清理 `progress_steward`、`fail_close_flow_authority`、flow-scope、dispatcher comments中的旧名称；不改测试逻辑。

#### 13. 最小实现范围

- 必须删除 target-only tests/module and old target path references。
- 必须 generic rename supervisor BDD fixtures/registration and preserve real runner.
- 必须 keep parallel-forge/supervisor runtime coverage green。
- 必须 update P0 source provenance without weakening assertions。
- 不新增替代 preset E2E，不改变生产 dispatch/fan-in/schema rules，不引入依赖。

#### 14. 集成验证

- Rust core：`cargo nextest run -p ralph-core --test scenarios -- supervisor` 与 `cargo nextest run -p ralph-core -- supervisor`；若 selector 不覆盖目标，使用现有 test target 的精确名称。
- Rust CLI：`cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0`、`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`、`cargo nextest run -p ralph-cli --bin ralph -- parallel_forge`。
- 真实联合边界：scenario 使用真实 EventLoop/coordinator；P0 使用真实 store APIs；只能 Fake 外部 worker shell，不得 Mock supervisor decision。
- 预期：generic supervisor 和 parallel-forge 绿；target-only tests 不再编译；失败不得进入 Unit 3。

#### 15. 风险驱动测试

- **Characterization：** generic minimal 场景和 P0 fixture 固定旧 runtime behavior，防止“删除 preset”误删 runtime。
- **State-machine：** supervisor terminal/fan-in behavior 已由现有 store/wave tests覆盖，本 Unit 必须保留其执行路径。
- **Idempotency：** P0 existing terminal/idempotent tests必须继续通过，因为旧 target removal 不应改变 store。
- **Contract：** origin guard BDD 保证 coordination topics仍由系统产生。

#### 16. 回归范围

- 直接：`scenarios` supervisor selectors、`ralph-cli` P0、wave_supervisor、preset lint module compile。
- 相邻：`crates/ralph-core/src/supervisor/**`、`crates/ralph-cli/src/loop_runner/wave/**`、parallel-forge preset tests。
- 公开消费者：parallel-forge run/preflight/preset check；不测试旧 run 成功。
- feature：默认 core tests 和 `supervisor-db` 真实 branch；不使用 bare cargo test。
- Build/lint/typecheck：core/cli packages；完整 workspace 在第 10 节。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/tests/integration_supervisor_primary.rs` | 删除测试 | target-only primary E2E | E5 |
| `crates/ralph-core/src/preset_lint/supervisor_preset_test.rs` | 删除测试 | target YAML/schema topology pins | E5 |
| `crates/ralph-core/src/preset_lint/mod.rs` | 修改测试注册 | 删除 target test module declaration | E5 |
| `crates/ralph-cli/src/loop_runner/tests/wave.rs` | 修改测试 | 删除 target lease block | E5 |
| `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs` | 修改测试 | 删除 target include test | E5 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试 | generic rename/registration | E6 |
| `crates/ralph-core/tests/scenarios/supervisor/*.yml` | 重命名/修改 fixture | 去除 target-only active names | E6 |
| `crates/ralph-core/src/supervisor/bridge.rs` | 修改文档注释 | 更新 fixture name | E6 |
| `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs` | 修改测试 fixture | 去除 deleted source string | E8 |
| `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage/tests.rs` | 修改测试注释 | generic wording | E5/E8 |
| `crates/ralph-core/src/event_loop/tests/fail_close_flow_authority.rs` | 修改测试注释 | generic wording | E8 |
| `crates/ralph-core/src/event_loop/tests/progress_steward.rs` | 修改测试注释 | generic wording | E8 |

#### 18. 完成标准

- S4/S5、Unit 2 unit/integration/BDD tests、parallel-forge regression、build/lint/typecheck通过。
- generic scenario 不再使用 target name/path；target-only test 不再编译。
- supervisor runtime/store/bridge/wave behavior 未被删除或放宽。
- 无新增 skip/only、无弱断言、无伪造 system event。
- Unit 2 可独立提交；Unit 1 已验证能力可复用。

#### 19. 停止条件

停止于：generic scenario 无法改名而需要旧 preset；parallel-forge 需要旧 target schema；删除 target test 造成通用 coverage 真空；P0 source 改动影响实际 ACL/event behavior；真实调用链出现新公开 consumer；Red 非预期；或 runtime behavior 需要生产修改。

停止后：记录新 Evidence → 重新比较“删除/泛化/迁移”方案 → 更新 D2/D4/D7 → 重算置信度 → 修订 Unit 2/3。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 误删通用 supervisor coverage | 删除整个 supervisor scenario 目录或 P0 | parallel-forge + generic scenario + P0 selectors | 只删 target-only primary/lint/lease，generic fixture 改名保留 | 低 |
| 旧 topology 与 parallel-forge 混淆 | 把旧 integration test直接改 preset name | compare hats/flow and existing parallel-forge tests | 不迁移旧 topology，使用现有 survivor coverage | 低 |
| BDD 变成 stub | rename时改用 `run_scenario` | inspect `scenarios.rs` runner call | 强制 `run_workflow_guard_scenario` | 低 |
| test source provenance drift | P0 hardcode remains | active `rg` and P0 test | use parallel-forge/generic source | 低 |

### Unit 3：清理活动 skill、operator 文档与公开入口

#### 1. Unit 目标

让当前活动的 skill、operator 文档、项目规则和补全不再提供已删除 preset 的可执行示例或旧拓扑说明。一个可观察行为是：新用户从活动入口无法得到旧 preset 的启动命令，但仍能得到有效的 `parallel-forge`/current catalog 入口。

#### 2. 对应需求与 Scenario

- Requirements：R5、R6。
- Scenarios：S6、S7。
- Decisions：D5、D8。
- Evidence：E7、E9、E10。

#### 3. 外部可观察结果

- `skills/tests` 的生成 stem、plan-touch fixture、static gate contract 不再要求旧 preset 文件名。
- README、guides、AGENTS/CLAUDE、`.cursor` 和 zsh completion 不再把旧 name作为当前可运行入口；历史目录仍可出现。
- `scripts/check-cli-doc-drift.sh` 和 skill test suite 通过。

#### 4. 当前行为基线

E7/E10 证明活动 skill 硬编码旧 name、旧 generated filenames 和旧 gate exception；E9 证明活动 docs/rules直接推荐旧 preset。Unit 3 进入前先将 gate contract tests 改为目标行为：旧 special approved finding 测试在删除 special branch前必须 Red（测试 helper无法再满足 target-only approval contract）；普通 gate tests必须保持 Green。

#### 5. 输入与输出

- 输入：skills Python tests、preset author/review references、活动 docs/rules。
- 输出：generic/parallel-forge skill examples；无 target-specific gate override；同步 operator catalog。
- 错误：未获通用批准的 strict finding仍 blocked；旧 preset运行文档不再出现。
- 副作用：只修改活动文档/skill；不改历史 archive/report/solution。
- 不变量：skill 的 forced `run_pipeline`、dual-plan、freshness、static-only、sandbox guardrails保留；zsh completion 仍使用当前 `compadd` style。

#### 6. 修改位置

| 位置 | 当前职责 | 预计修改边界 | 明确不修改 |
|---|---|---|---|
| `skills/tests/test_plan_resolve.py`、`test_bootstrap_pipeline.py` | plan/preset-gap tests | 用 surviving preset和路径 fixture替换旧 target literal | plan resolution behavior |
| `skills/tests/test_execution_model_contract.py`、`test_project_bootstrap_contract.py` | capability/name hygiene tests | 移除对已删除名称的 forbidden literal；保留 capability-triggered assertions | capability model |
| `skills/ralph-preset-author/references/{patterns,finding-rubric,agent-native-model}.md` | active author guidance | 将 target-specific example改为 capability/supervisor-enabled generic wording | lint finding IDs和通用流程 |
| `skills/ralph-preset-review/references/{patterns,finding-rubric,agent-native-model}.md` | active review guidance | 同步 author view，不点名已删除 builtin | review contract |
| `skills/ralph-preset-review/fixtures/README.md` | active fixture guidance | 删除旧 builtin 名称示例，保持 capability-triggered rule | fixture semantics |
| `README.md`、`docs/guide/presets.md`、`docs/guide/project-usage.md` | user/operator docs | 删除旧 row/recommendation；改用 current catalog/parallel-forge说明 | unrelated doc modernization |
| `AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc` | always-on project instructions | 同步 builtin list/topology，保留 parallel-forge supervisor描述；编辑 `AGENTS.md` 后按仓库约定用 `cp AGENTS.md CLAUDE.md` 同步 | hard rules unrelated to preset |
| `scripts/ralph-zsh-plugin.zsh` | completion | Unit 1已删 value；本 Unit只验证安装/加载和剩余描述 | completion implementation style |

#### 7. 可依赖能力

- Unit 1 已删除 public registry/old YAML；Unit 2 已删除 target-only test active references。
- `validate_pipeline` 普通 gate、现有 e2e contract runner、skills `.venv`。
- 活动文档的 current builtin list和 `parallel-forge`说明。

#### 8. 禁止依赖的未来能力

- 不为旧名称增加迁移提示、自动重写或 alias。
- 不把旧 gate exception迁移给 `parallel-forge`，除非新增直接证据证明其 strict gate确实需要同一具体例外；当前证据不支持。
- 不改历史文档目录以追求仓库全局零字符串。
- 不趁机重写完整 preset author/review skill；只删除受影响 target-specific assumptions并保持目录同步。

#### 9. 验收测试

- **S6-AT1：** `gate.run_static_gate` 使用普通 preset/runner，未批准 finding 返回 blocked；旧 supervisor-specific approved finding helper/测试被删除而不再存在名称放行路径。
- **S6-AT2：** `skills/.venv/bin/python -m pytest skills/tests -q`；断言 suite generation、plan resolution、freshness、argv shape、capability-triggered contracts均通过。
- **S7-AT1：** active allowlist audit：除明确保留的历史目录外，`rg` 不返回旧 name；`parallel-forge` 和 current catalog 入口存在。
- **S7-AT2：** 运行 `scripts/check-cli-doc-drift.sh --strict`；断言 CLI docs无新增 drift。
- **S7-AT3：** 按项目硬规则安装 `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`，启动新 zsh 读取 plugin，确认 completion function加载且 builtin values无旧 name。该动作由 Executor 在用户环境执行；若 home 不可写，记录为环境阻塞，不得改成静默跳过。

#### 10. Acceptance Red

1. 先删除/改写旧 gate special branch对应的 expected acceptance test，运行旧 special gate tests；它们必须因 `_SUPERVISOR_PRESET` 分支不存在/旧 approved output不再被接受而失败。
2. 正确 Green 是删除 helper/tests并让普通 gate suite通过；不能通过把所有非零结果改成 success获得 Green。
3. 对文档/skill清理，先运行 active `rg` audit，预期 Red 是列出仍在活动目录中的旧 name；历史目录命中必须按 allowlist排除，而不是盲删。

#### 11. 单元测试拆分

- Gate normal fail-closed：输入 unapproved finding，期望 `ok=False`，不 Mock away validation。
- Gate normal pass：输入 all-ok stage，期望 `ok=True`，argv shape保持。
- Bootstrap generate suite：输入 `builtin:parallel-forge`，期望 generated stem/argv正常且不修改 presets subtree。
- Plan resolution/preset-gap：输入 surviving preset + change plan touches `presets/`，期望仍要求 preset confirmation。
- Name hygiene：测试只检查 capability-triggered fixtures不按 builtin name分支；删除目标 forbidden literal，不削弱 capability assertions。
- 文档/skill静态 audit：只验证活动文件白名单，不将历史档案当作当前入口。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：运行旧 supervisor approved-finding tests，因名称特例目标已删除而失败。
2. 最小实现：删除 `gate.py` 特例、helper、旧 special tests；保留普通 `validate_pipeline`。
3. Test 1 Green：普通 gate contract suite通过，unapproved findings仍 blocked。
4. Test 2 Red：将 generic suite fixtures/stems改为 parallel-forge后，尚未同步 expected filenames/argv时失败。
6. Test 2 Green：Python contract suite通过。
7. Test 3 Red：active `rg` audit列出 AGENTS/CLAUDE/docs/rubric/patterns 等残留旧入口。
8. 最小实现：逐个清理活动 docs/rules/skill references，并保持 AGENTS/CLAUDE同步。
9. Test 3 Green：active allowlist、CLI doc drift 和 zsh load通过。
10. Refactor：统一用 capability/supervisor-enabled generic wording，删除重复旧历史注释；不改 skill 结构和非相关指南。

#### 13. 最小实现范围

- 必须删除旧 gate 名称分支/approved findings与对应测试。
- 必须把活动 skill 测试/示例迁移到 surviving/generic source。
- 必须同步活动用户文档、项目 rules、preset author/review references、zsh安装验证。
- 必须保持普通 fail-closed gate、argv shape、sandbox guardrails、capability-triggered audit。
- 不实现新 gate policy、不删历史 docs、不修改 runtime。

#### 14. 集成验证

- Python：运行 execution-model contract 相关的 targeted tests，再运行完整 `skills/.venv/bin/python -m pytest skills/tests -q`。
- CLI docs：运行 `scripts/check-cli-doc-drift.sh --strict`。
- Catalog/operator：运行 `./scripts/validate-builtin-presets.sh --strict`、CLI list/show/check smoke、zsh plugin load。
- 真实边界：`gate.py` 真实调用 `validate_pipeline`；可注入 fake subprocess 只用于既有 contract tests，不能绕过 finding classification。
- 预期：旧 special exception完全消失；普通 gate和 surviving preset checks通过。

#### 15. 风险驱动测试

- **Contract：** gate findings/argv 和 bootstrap generated file names是活动 skill 的稳定契约。
- **Characterization：** 普通 gate pass/block、plan-gap confirmation、sandbox no-mutation 保留旧行为。
- **Security/fail-close：** unapproved strict finding必须继续 blocked，防止删除特例时误放宽 gate。
- 不做 E2E live run：skill自身规定 static-only，且本次不需要外部服务。

#### 16. 回归范围

- 公开接口：CLI preset list/check and zsh builtin completion。
- 文档：README/guides/AGENTS/CLAUDE/.cursor，不要求历史归档零命中。
- Build/lint/typecheck：Python suite、CLI drift、Rust full gate；失败不得进入 final quality gate。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `skills/tests/test_plan_resolve.py`、`test_bootstrap_pipeline.py` | 修改测试 | 替换 old preset fixture | E10 |
| `skills/tests/test_execution_model_contract.py`、`test_project_bootstrap_contract.py` | 修改测试 | 删除 dead literal，保留 capability assertions | E10 |
| `skills/ralph-preset-author/references/**`、`skills/ralph-preset-review/references/**`、`skills/ralph-preset-review/fixtures/README.md` | 修改文档 | 清理 active target-specific wording | E10 |
| `README.md`、`docs/guide/presets.md`、`docs/guide/project-usage.md` | 修改文档 | 删除失效入口/推荐 | E9 |
| `AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc` | 修改项目文档/rules | builtin topology/catalog同步 | E9 |
| `scripts/ralph-zsh-plugin.zsh` | 修改后验证 | completion surface同步，若残留目标说明则一并删除 | E2/E9 |

#### 18. 完成标准

- S6/S7和 Unit 3 Python/CLI/doc/zsh tests通过。
- 活动入口不再提供旧名称；历史资料不被无理由改写。
- gate保持 fail-closed；没有将旧例外迁移给 surviving preset。
- AGENTS 与 CLAUDE 的 builtin 描述同步；zsh installed copy由 Executor验证。
- 无新增 skip/only、无弱化 finding/argv/assertion。
- Unit 3 可独立提交；Unit 1/2 已验证 registry/runtime边界。

#### 19. 停止条件

停止于：活动 skill 需要旧 preset 才能完成通用流程；删除特例导致普通 gate行为改变；`parallel-forge`被错误要求旧 exception；docs/rules与源码发生无法解释的冲突；zsh plugin安装会覆盖用户未授权内容；发现仓库外运行契约；或置信度低于 0.85。

停止后：记录新 Evidence → 明确是活动入口还是历史记录 → 重评 D5/D8 → 修订文件边界和回归命令；不能通过删除测试或放宽 gate继续。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| gate 被误放宽 | 删除 name branch时把所有 strict finding当 pass | targeted fail-closed tests + full skill suite | 只保留普通 validator，删除 approved override | 低 |
| active docs 与 code drift | 只改 README未改 rules/skill | active rg + CLI doc drift | 使用本 Unit 文件表逐项完成 | 低 |
| 历史文档被误删 | 用全仓库零命中作为标准 | path allowlist review/git diff | 明确历史目录 non-goal | 低 |
| zsh用户仍加载旧插件 | 只改 repo script未安装/验证 | copy + new zsh load | 按 hard rule安装并记录结果 | 中（用户环境差异） |

---

## Verification Contract

### 8. Unit 串行依赖图

```text
Unit 1：删除 builtin 注册面
  ↓ 完成 registry/build/unknown/parity 回归
Unit 2：删除 target-only 测试面，保留 generic supervisor/parallel-forge
  ↓ 完成真实 EventLoop/coordinator/store/wave 回归
Unit 3：清理活动 skill、文档、rules、zsh operator surface
```

- Unit 2 使用 Unit 1 已验证的“旧 preset 不再是 embedded input”能力；不能交换，因为 Unit 2 的 Red 必须由目标文件删除产生，而不是先修改测试掩盖依赖。
- Unit 3 使用 Unit 1 的“旧 name unknown/catalog 删除”和 Unit 2 的“活动测试不再依赖旧文件”能力；不能提前，因为 skill/doc 的最终 active `rg` 结果依赖前两 Unit 的文件名/registry边界。
- Unit 1 不实现 Unit 2 的 generic scenario rename，不实现 Unit 3 的 gate/doc cleanup。
- Unit 2 不实现 Unit 3 的名称迁移提示或文档，不修改 Unit 1 的 registry策略。
- Unit 3 不回头修改 runtime；发现需要 runtime 行为变化时必须停止并新建/修订计划。

### 9. 执行命令清单

所有 Rust 命令均遵守仓库 hard rule，使用 `cargo nextest run` 系列；禁止裸 `cargo test -p ralph-cli`。

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败处理 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-cli --bin ralph -- preset` | Unit 1 Red/Green | registry、list/name、unknown、parity相关测试 | 先按计划出现有效 Red，删除后全绿 | 非预期失败停止 Unit 1 |
| `cargo build -p ralph-cli` | Unit 1 Green | build.rs manifest/schema/embed | build成功，无悬空 include/schema | 不得进入 Unit 2 |
| `cargo nextest run -p ralph-core -- capability_inventory` | Unit 1 Green | compile-time capability inventory/source | capability inventory测试通过 | 检查 E13 source，不猜 |
| `cargo nextest run -p ralph-core --test scenarios -- supervisor` | Unit 2 Green | generic supervisor BDD真实 EventLoop | generic scenario events/absent events通过 | 检查是否 selector 命中真实 test |
| `cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0` | Unit 2 Green | generic P0 store/bridge/worker env | InMemory及启用 feature路径行为通过 | 非预期失败停止 Unit 2 |
| `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` | Unit 2 Green | wave dispatcher/store/worker contract | 既有 wave supervisor tests通过 | 不删除失败测试 |
| `cargo nextest run -p ralph-cli --bin ralph -- parallel_forge` | Unit 2/3 | parallel-forge structured contracts | parallel-forge所有命中测试通过 | 任何 supervisor regression阻塞后续 |
| `cargo nextest run -p ralph-cli --test integration_worktree_isolation` | Unit 2/最终 | surviving supervisor/parallel-forge worktree behavior | integration通过 | 检查受影响调用方 |
| `cargo nextest run -p ralph-cli --test integration_resume` | Unit 2/最终 | surviving resume/loop behavior | integration通过 | 检查 runtime path，不改 old alias |
| `skills/.venv/bin/python -m pytest skills/tests -q` | Unit 3 final | 全 skill contract regression | 三层 skill tests全绿 | 不允许跳过/切换系统 Python |
| `./scripts/validate-builtin-presets.sh --strict` | Unit 1/3 final | 所有 public builtin strict lint | 9 public preset全部通过 | 修复 parity/lint，不放宽 strict |
| `scripts/check-cli-doc-drift.sh --strict` | Unit 3 final | CLI injected docs drift | 无新增 drift | 只更新受影响文档 |
| `cp AGENTS.md CLAUDE.md` | Unit 3 after AGENTS edit | 同步 always-on project instructions | 两文件的 builtin catalog/rules一致 | 若文件不是同步源，先检查差异并停止，不覆盖用户独立变更 |
| `cargo fmt --check` | 每 Unit close | formatting | exit 0 | 必须修复格式再继续 |
| `cargo clippy -p ralph-cli --all-targets --all-features` | 每 Unit close/最终 | lint/type correctness | exit 0，无新增 allow | 不关闭 lint |
| `cargo build` | 每 Unit close/最终 | workspace production build | exit 0 | 停止并调查 |
| `./scripts/run-tests.sh` | Unit 3 close/最终 | 两阶段 nextest + doctest 全量回归 | workspace gate全绿 | 先查真失败；只允许按硬规则 serial fallback |
| `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅全量出现竞态/时序 flake | hard-rule serial fallback | 若仍失败即真失败 | 不是默认命令，不能掩盖失败 |
| `cargo run -p ralph-cli -- preset builtin list --format json` | Unit 1/最终 smoke | 真实 CLI public catalog | 9 names，无旧 name，含 parallel-forge | 非零停止 |
| `cargo run -p ralph-cli -- preset builtin show ce-executor-supervisor` | Unit 1/最终 smoke | 真实 old-name unknown | 非零，现有 unknown/missing语义，无 fallback | 记录实际 stderr，不擅自改契约 |
| `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` | Unit 3 close | 安装当前用户 zsh plugin | copy成功 | home不可写则标记环境阻塞并报告 |
| `zsh -ic 'typeset -f _ralph 2>/dev/null; print -r -- "${_RALPH_BUILTIN_HAT_VALUES:-}"'` | Unit 3 close | completion script加载 | function/value数组加载且无旧 name | 检查 plugin path/用户环境 |

命令参数若与当前 `--help` 不一致，Executor 必须先记录 Evidence 并停止；不能把命令改成未经验证的替代语法后继续。

---

## Definition of Done

### 10. 最终质量门禁

- S1–S7 全部有真实可执行测试且通过。
- R1–R6 每一项均能追踪到 Scenario、Unit、Evidence。
- `ce-executor-supervisor` 不再出现在活动 catalog/registry/build input/CLI completion/活动 skill/operator docs；允许出现在明确的历史资料 allowlist。
- `parallel-forge` 仍是 public builtin，supervisor enabled/isolated、wave dispatch/fan-in、failure recovery、worktree/resume和strict lint通过。
- generic supervisor BDD 仍由真实 EventLoop/coordinator/store 路径执行；不得用 `run_scenario` stub替代。
- Characterization/contract tests通过；适用的 idempotency/state-machine checks通过。
- Build、Clippy、fmt、Python pytest、CLI doc drift和`./scripts/run-tests.sh`通过。
- 需要 feature 的 supervisor branch已验证；不因删除旧 preset而删除 `supervisor-db`。
- 没有新增 skipped/ignored/only test，没有削弱 assertion，没有无解释 snapshot/golden变化，没有引入新依赖。
- 没有 alias/fallback、没有旧 overlay残留、没有手工修改 `.ralph/` 状态。
- 每个 Unit 均按顺序形成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close，并可独立提交。
- Evidence ledger 和 Decision confidence在执行结果中更新；没有未处理的 BLOCKED decision；所有执行关键决策仍 ≥0.85。
- 变更 diff 不包含本计划之外的无关清理或工作树重置。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 6 个稳定需求、7 个 Scenario、8 个 Decision、3 个串行 Unit、真实文件/测试/命令和完成条件均已列出。 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D8 已比较候选方案并固定删除边界、保留边界、unknown 语义、测试处理和 gate 处理。 |
| 所有文件和接口是否有代码库证据 | 是 | 文件均来自 `rg`/`sed`/构建配置；历史目录与活动目录边界已明确。 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D8 置信度为 0.92–0.99，均有 E1–E14 支持。 |
| 是否存在未处理的低置信度假设 | 否 | 仓库外调用方被明确标记为执行时停止条件，不作为实施决策。 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1=删除/unknown catalog，U2=移除专属测试但保留 runtime coverage，U3=清理活动入口并恢复通用 gate。 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit均有 Acceptance Red、targeted commands、integration、regression和close标准。 |
| 每个 Unit 是否有真实 Red | 是 | U1旧 name仍 Some；U2旧 include/fixture path失败；U3旧 special gate/active rg残留失败。 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit第16项列出直接、相邻、公开消费者、feature/build/lint范围。 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图仅允许 U1→U2→U3；每个 Unit明确禁止提前实现后续行为。 |
| 是否存在泛化任务描述 | 否 | 每项均绑定真实路径、符号/测试、输入输出、Red原因、命令和完成标准。 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第5节验收策略、第6节矩阵、Unit对应关系覆盖 S1–S7。 |
| 所有关键决策是否有 Evidence | 是 | D1–D8均引用E1–E14。 |
| 计划是否可以严格串行执行 | 是 | 第8节依赖图和每个 Unit的 Red→Green→Refactor顺序已固定。 |

本计划没有 BLOCKED 决策，可以直接交给 Coding Agent 按 Unit 1 → Unit 2 → Unit 3 执行；若执行过程中触发停止条件，必须先回到 Evidence/Decision 更新，不得把猜测写成实现。
