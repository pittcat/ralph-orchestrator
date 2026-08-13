---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
type: feat
created: 2026-08-09
---

# 外置 Worktree 与终态 Report 路径可靠展示开发计划

## Goal Capsule

本计划只处理两个已经在当前代码库中复现并定位的问题：

1. `ralph run --worktree` 以及 supervisor slot 创建的 Git worktree 默认落在目标仓库内部的 `.worktrees/`，导致 agent 能在目标 checkout 的可发现路径中看到并误写其他 worktree；默认 worktree 必须迁移到目标仓库父目录下的共享根：`<target-parent>/worktree/<project-name>/<plan-name>/`。
2. reporter 已经通过 `report.done.report_path` 和 `LOOP_COMPLETE.report_path` 传递最终报告路径，且有一致性 gate，但终态 CLI 只打印通用终止信息，agent prompt 中的 `DELIVERABLE_PATH` 不是可靠的 runtime 最终展示；终态 runtime 必须从已接受的终态 payload 读取路径并展示。

外置路径是本计划处理 worktree 风险的边界；现有 precheck、event policy 和恢复语义保持不变。报告路径展示是“accepted terminal event → runtime display”的补齐，不新增第二套 report 存储。

## Product Contract

### 0. 计划状态

- 状态：**READY**。所有影响实现路径的关键决策置信度均不低于 0.85；当前没有 BLOCKED 决策。
- 当前基线：分支 `pittcat-dev`，HEAD `d98b9f45`；源码和测试均以该 checkout 的当前内容为准。
- 调查范围：
  - `crates/ralph-core/src/worktree.rs`、`crates/ralph-core/src/supervisor/worktree_bind.rs`；
  - `crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/src/loop_runner/inner.rs`、`crates/ralph-cli/src/display.rs`；
  - `crates/ralph-core/src/event_loop/event_processing.rs`、`crates/ralph-core/src/event_loop/wave_scope.rs`、`crates/ralph-core/src/event_loop/loop_state.rs`；
  - `crates/ralph-proto/src/json_rpc.rs`、`crates/ralph-tui/src/state.rs`、`crates/ralph-tui/src/rpc_source.rs`、`crates/ralph-tui/src/widgets/footer.rs`；
  - builtin preset/schema、agent-facing guide、worktree integration tests、相关 git 历史。
- 已执行的证据调查命令：
  - `rg --files`、`rg -n -C`、`sed -n`、`nl -ba`：确认目录、符号、调用链、测试位置和现有路径引用；
  - `git status --short`、`git log --oneline`、`git show --stat`、`git show a38b0218`：确认基线及 terminal deliverable 相关历史。
- 本计划阶段没有执行测试、build、clippy 或全量 gate；这是有意的，因为用户要求只写计划，不能把计划阶段的静态调查伪装成实现验证。
- 实现阶段必须执行的验证见第 9 节；任一 Red 不是预期目标行为缺失、任一关键调用链与本计划不符，必须按 Unit 停止条件暂停，不得猜测。

### 1. 功能目标

#### 业务目标与调用方

- 调用方一：`ralph run --worktree`、`--reuse-worktree`、supervisor 的 `DefaultWorktreeFactory`。它们需要创建、定位、复用和清理同一个确定的外置 worktree。
- 调用方二：运行 `ralph` 的人、TUI/RPC 消费者和后续自动化。它们需要看到“已经被 runtime 接受的最终报告路径”，而不是依赖 agent 是否遵守 prompt 最后一行。

#### 当前行为

- 默认 `WorktreeConfig` 的 `worktree_dir` 是 `.worktrees`，`create_worktree` 将 `repo_root/.worktrees/<loop_id>` 交给 `git worktree add`。
- `spawn_worktree_loop` 每次使用 `WorktreeConfig::default()`，并无条件调用 `ensure_gitignore(workspace_root, ".worktrees")`。
- `find_reusable_worktree_by_name` 直接拼接 `repo_root/.worktrees/<name>`，没有复用同一个配置解析入口。
- `DefaultWorktreeFactory` 也使用 `WorktreeConfig::default()`，因此 supervisor slot 同样落在默认根。
- reporter 的 schema 已要求 `report.done` 包含 `plan_name`、`report_path`、`verdict`，`LOOP_COMPLETE` 包含 `reason`、`report_path`；ce-executor-pipeline 已配置 `completion_payload_match` 比对 `report.done.report_path`。
- runtime 在终态 prompt 中注入 `DELIVERABLE_PATH`，但 `crates/ralph-cli/src/display.rs::print_termination` 只展示通用终止信息；终态 CLI 没有从已接受 payload 提取路径并打印的机制。

#### 目标行为与差异

- 默认创建路径变为：

  `repo_root.parent()/worktree/repo_root.file_name()/loop_id/`

  其中 `repo_root.file_name()` 是 project name，`loop_id` 是当前已有的 plan/worktree exact name；Git branch 继续使用当前 `ralph/<loop_id>` 规则。supervisor slot 的现有 branch/slot 字符串继续作为 leaf 相对路径，不引入第二种命名规则。
- 目标 checkout 不再因为默认 worktree 创建而新增 `.worktrees/` 目录或 `.gitignore` 条目；目标 repo 内现有 `.worktrees` 不会被默认路径读取、写入、复用或清理。
- create、exists、reuse、cleanup、supervisor factory 和 child 的 `--worktree-path` 必须指向同一个已解析的绝对路径；registry 仍保存绝对 workspace/worktree path。
- 只有在终态 `LOOP_COMPLETE` 已被 runtime 接受后，才从接受的 completion payload 读取顶层 `report_path` 或 `artifact_path`，并：
  - 在非 TUI、非 RPC 的终态 CLI 输出一条独立的 `DELIVERABLE_PATH: <path>`；
  - 在 `loop.terminate` observer payload 中携带同一个 path；
  - 在 RPC `LoopTerminated` 中携带可选的 `deliverable_path`，供 TUI/RPC 消费者显示。
- `report.done` 与 `LOOP_COMPLETE` 的字段一致性 gate 保持现有语义；路径不一致时不得进入终态展示，不得展示错误的第二个路径。

#### 输入、输出与状态变化

- Worktree 输入：目标 repo root、已有 `loop_id`、`WorktreeConfig`（默认外置或显式 custom directory）。输出：`Worktree { path, branch, head }`；副作用是 Git worktree/branch 创建或复用；错误继续使用现有 `WorktreeError`，路径占用必须 fail-closed。
- Report 输入：已接受终态事件的 payload，顶层 `report_path` 优先，其次 `artifact_path`；输入缺失时无 deliverable display，不能从 agent 普通文本、prompt、历史 report 或推测路径补全。
- Report 输出：标准 CLI 独立 marker、`loop.terminate` payload、RPC optional field；同一次终态只允许同一 path，不新增文件或数据库。

#### 错误、兼容、安全和性能语义

- 默认 worktree 路径已存在时继续返回 `AlreadyExists`/现有冲突错误；不能自动选择随机目录、覆盖目录或将其当作另一个 project 的 worktree。
- `--reuse-worktree` 只能复用 canonical external path；无 registry 但在 Git worktree list 中存在的既有“手工 worktree”继续按现有规则处理，前提是路径属于当前 canonical name。
- 显式 `WorktreeConfig::with_dir(...)` 的 custom absolute/relative 行为保留；本计划不增加配置字段和 feature flag。
- 不新增 OS sandbox、文件系统 watcher 或新的权限框架；默认 worktree 外置解决普通 agent 误入 target checkout 的主要风险，LoopContext contract 继续提供 agent-facing 约束。这不是对任意绝对路径写入的内核级阻断。
- 不改变 `report.done`/`LOOP_COMPLETE` schema、`completion_payload_match` 的比较规则、report 文件的生产方式或已有 reuse/task recovery 行为。
- 路径解析是本地同步计算；新增字符串提取只处理已接受的单个 JSON payload，不引入网络、数据库或新的依赖。

#### 本次范围、非目标与约束

范围：默认 worktree resolver、所有已确认的 create/reuse/supervisor 调用方、integration/characterization tests、终态 deliverable extraction/display、RPC/TUI 终态传递、已确认的文档和 agent-facing command guide。

非目标：

- 不实现 Writer Workspace Contract、OS sandbox、文件系统 watcher、git hook 或新的 agent permission framework；
- 不改变现有 precheck、event policy、rejected event 和恢复语义；
- 不改变 reporter 的 report 内容、verdict 计算、OPAC emit 顺序或 report 文件位置；
- 不迁移历史 `.ralph/review` 产物，不清理用户已有的旧 `.worktrees`，不自动搬运旧 worktree；
- 不新增 worktree 配置 CLI 参数；
- 不把普通 worktree 中“其他 agent 同时主动写绝对路径”的全部风险宣称为已消除。

已确认假设：

- `repo_root` 在生产 worktree 创建入口是绝对或可稳定取得 basename 的路径；当前 `LoopContext`、registry 和 `create_worktree` 都以绝对 worktree path 工作。
- plan basename/exact worktree name 已由现有 `worktree_file_name_prefix`/`resolve_exact_worktree_name` 生成；本计划不重写命名策略。
- `last_completion_payload` 保存的是已接受的 terminal payload，适合作为终态展示的唯一 runtime source。

待验证但不构成关键设计阻塞的假设：

- macOS/Linux 临时测试 repo 的父目录可创建 `worktree/<project>`；由 Unit 1 的 filesystem test 直接验证。
- TUI footer 和 RPC event 的现有渲染宽度/serde round-trip 可承载可选路径；由 Unit 3 的现有 ratatui/RPC tests 验证，失败时只调整展示布局，不改变 source-of-truth。

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口

**Worktree 入口与调用链**

`ralph run --worktree` → `crates/ralph-cli/src/commands/run.rs::spawn_worktree_loop` → `WorktreeConfig::default` → `worktree_exists`/`create_worktree` → `LoopContext::worktree` → child executor 使用 `--worktree-path`。显式 `--reuse-worktree` 从 `run_command` 进入 `find_reusable_worktree_by_name`。supervisor slot 通过 `crates/ralph-core/src/supervisor/worktree_bind.rs::DefaultWorktreeFactory::create` 进入同一个 core `create_worktree`。

`crates/ralph-core/src/worktree.rs::remove_worktree`、CLI `loops`/API cleanup 使用 registry 里记录的绝对 path；它们不依赖 `.worktrees` 字符串，因此是可复用的清理边界。`worktree_exists` 已经接受 `WorktreeConfig`，但 reuse helper 仍有硬编码路径。

**Report 入口与调用链**

reporter → `report.done`（required `report_path`）→ `LOOP_COMPLETE`（required `report_path`）→ event policy/required event/completion payload match → `EventLoop::check_completion_event` → `run_loop_impl_inner` 的终态 closure → `print_termination`、RPC `LoopTerminated`、TUI termination signal。现有 `event_processing.rs::inject_terminal_deliverable_contract` 只影响 terminal hat prompt；已有 `LoopState::last_completion_payload` 保存接受的 completion payload，可作为 display source。

**数据边界与外部依赖**

- Worktree 依赖 Git CLI；路径创建/验证由 core 同步函数负责。
- Report path 的一致性依赖 event policy schema 和 `completion_payload_match`；report 文件由 reporter 写入并自行 `test -f`/readability 检查；本计划不把 filesystem validation 新增到 runtime。
- TUI 通过 `loop.terminate` event 或 RPC `LoopTerminated` 更新状态；RPC 类型由 `ralph-proto` 定义。

**已有测试和验证入口**

- core worktree 单测集中在 `crates/ralph-core/src/worktree.rs`；
- CLI worktree 端到端隔离测试在 `crates/ralph-cli/tests/integration_worktree_isolation.rs`，其中多个 helper 直接写死 `.worktrees`；
- completion payload match 的行为测试在 `crates/ralph-core/src/event_loop/tests/termination.rs`；
- terminal prompt 注入测试在 `crates/ralph-core/src/event_loop/tests/build_prompt.rs`；
- preset 结构测试在 `crates/ralph-cli/src/presets.rs`；
- display、RPC、TUI footer/source 都有现有单测；全量入口是 `./scripts/run-tests.sh`，不得使用裸 `cargo test` 作为 `ralph-cli` 验证入口。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-core/src/worktree.rs::WorktreeConfig`、`worktree_path` | 默认 `worktree_dir` 是 `.worktrees`；已有 absolute path 分支；`with_dir` 是唯一 custom 入口。 | 外置默认路径应在现有 resolver 内完成，不新增第二套创建 API。 | 高 |
| E2 | `crates/ralph-core/src/worktree.rs::create_worktree` | 真实 Git 调用是 `git worktree add -b <branch> <worktree_path>`；path 由 config base + loop id 组成。 | 改变 base 即可改变创建位置，branch/同步/HEAD 逻辑应保持不变。 | 高 |
| E3 | `crates/ralph-cli/src/commands/run.rs::spawn_worktree_loop` | 每次默认创建都用 `WorktreeConfig::default()`，并调用 `ensure_gitignore(workspace_root, ".worktrees")`。 | 必须移除默认 in-repo ignore 副作用，并让创建、exists、child path 共用解析结果。 | 高 |
| E4 | `crates/ralph-core/src/worktree.rs::find_reusable_worktree_by_name` | reuse helper 直接拼接 `repo_root/.worktrees/<name>`，只在 helper 内做 Git/registry 交叉校验。 | reuse 必须改为接受同一 `WorktreeConfig`/canonical resolver，否则迁移后复用必然失效。 | 高 |
| E5 | `crates/ralph-core/src/worktree.rs::remove_worktree`、registry/API cleanup | cleanup 接收绝对 worktree path；registry 保存 absolute path。 | 不需要新增清理协议或迁移数据库；只需确保新 path 被正确注册。 | 高 |
| E6 | `crates/ralph-core/src/supervisor/worktree_bind.rs::DefaultWorktreeFactory` | supervisor slot 也使用 `WorktreeConfig::default()`；Review slot 是 SharedReadonly，不创建 worktree。 | executor/fix slot 必须跟随 default resolver；reviewer 不应被扩大范围。 | 高 |
| E7 | `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 多个真实 CLI 测试、fixture 和 helper 直接假设 `main_repo/.worktrees`。 | 迁移测试 fixture 到 canonical external root，并保留“目标 repo 无 `.worktrees`”断言。 | 高 |
| E8 | `crates/ralph-core/src/loop_context.rs::generate_context_file` 及其 tests | agent context 将 workspace 作为唯一写入范围，并注入 `RALPH_WORKSPACE_ROOT`；没有 OS sandbox。 | 保持现有 context、event policy 与 precheck 语义不变；本计划不虚构强制 filesystem boundary。 | 高 |
| E9 | `presets/schemas/ce-executor-pipeline.yml`、`presets/en/ce-executor-pipeline.yml` | `report.done` required `plan_name/report_path/verdict`；`LOOP_COMPLETE` required `reason/report_path`；fill rule 要求相同 path。 | report contract 已存在，不应重复新增 schema 字段；缺口是 runtime final display。 | 高 |
| E10 | `crates/ralph-cli/src/presets.rs`、`crates/ralph-core/src/event_loop/tests/build_prompt.rs` | builtin terminal schema 必须有 report/artifact path；runtime 只向 prompt 注入 `DELIVERABLE_PATH`。 | prompt contract 保持；新增 runtime display 必须取 accepted payload，不取 prompt 文本。 | 高 |
| E11 | `crates/ralph-core/src/event_loop/loop_state.rs::last_completion_payload`、`parse_and_emit.rs` | accepted completion event payload 会保存到 `last_completion_payload`；终态 gate 先校验再设置 honored。 | 这是 report display 的唯一可靠 source；可避免扫描 agent 普通输出。 | 高 |
| E12 | `crates/ralph-core/src/event_loop/wave_scope.rs::completion_payload_match` | 已有 gate 比较 predecessor 与 `LOOP_COMPLETE` payload；不一致会注入 correction 并拒绝完成；ce pipeline 已配置 `report.done/report_path`。 | 报告路径不一致行为无需重新发明；计划只补终态展示和回归断言。 | 高 |
| E13 | `crates/ralph-cli/src/display.rs::print_termination`、`loop_runner/inner.rs` 终态 closure | CLI 终态当前只传 `TerminationReason`、`LoopState`、loop id；没有 deliverable 参数或 marker。 | Unit 3 必须在这个终态边界加入 formatter/marker。 | 高 |
| E14 | `crates/ralph-proto/src/json_rpc.rs::RpcEvent::LoopTerminated`、`ralph-tui` RPC/footer tests | `LoopTerminated` 没有 deliverable field；TUI 只收到 completed 状态和 iteration。 | 若要保证 RPC/TUI 终态可见性，增加 optional field/state/footer 传递，并保持无 path 时兼容。 | 高 |
| E15 | `.cursor/rules/feature-flags.mdc`、`docs/advanced/parallel-loops.md`、`presets/en/merge-loop.yml`、`crates/ralph-core/data/ralph-tools-cmdref.md` | 多处 agent/operator-facing 文档仍写 `.worktrees/<loop-id>`；merge-loop 还给出删除命令。 | 行为改变后必须同步可执行路径说明；历史 plan/report 不改。 | 高 |
| E16 | `git show a38b0218`、`git show 1049cacb` | terminal path contract 曾从“prompt-only”加强到 schema/match；最近 worktree 修复集中在 cwd/parent-child duplication。 | 本计划延续现有机制边界：schema/match 保持，path resolver 和 runtime output 分别补缺口。 | 中高 |

#### 2.3 受影响范围

已由证据确认的生产模块：

- `crates/ralph-core/src/worktree.rs`；
- `crates/ralph-core/src/supervisor/worktree_bind.rs`；
- `crates/ralph-cli/src/commands/run.rs`；
- `crates/ralph-cli/src/loop_runner/inner.rs`；
- `crates/ralph-cli/src/display.rs`；
- `crates/ralph-core/src/event_loop/loop_state.rs`、`parse_and_emit.rs`、`completion_and_termination.rs`；
- `crates/ralph-proto/src/json_rpc.rs`；
- `crates/ralph-tui/src/state.rs`、`rpc_source.rs`、`widgets/footer.rs`。

已确认测试模块：

- `crates/ralph-core/src/worktree.rs` 内嵌 tests；
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`；
- `crates/ralph-core/src/event_loop/tests/termination.rs`、`build_prompt.rs`；
- `crates/ralph-cli/src/presets.rs`、`display.rs` tests；
- `crates/ralph-proto/src/json_rpc.rs` tests；
- `crates/ralph-tui/src/rpc_source.rs`、`widgets/footer.rs`、`state.rs` tests。

已确认配置/文档/数据边界：

- `presets/en/ce-executor-pipeline.yml` 与 `presets/schemas/ce-executor-pipeline.yml`：只做 builtin preset/schema parity 回归检查，不修改 terminal schema；
- `presets/en/merge-loop.yml`：更新硬编码 worktree 路径说明；因没有 event topology 变更，不改 schema 内容；
- `.cursor/rules/feature-flags.mdc`、`docs/advanced/parallel-loops.md`、`crates/ralph-core/data/ralph-tools-cmdref.md`：更新可执行路径说明；
- `.ralph/loops.json` 等运行时状态：不手工编辑；测试通过真实 CLI/runtime 生成。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 默认 worktree 应放在哪里？ | A. 保持 repo 内 `.worktrees`；B. repo parent 的 `worktree/<project>/<loop>`；C. 引入 OS sandbox/temp root | 选择 B：`repo_root.parent()/worktree/<repo basename>/<loop_id>`。（session-settled: user-directed — chosen over in-repo `.worktrees` and OS sandbox: 目标 repo 外置即可解决大部分误发现，且不引入新运行时机制。） | E1、E2、E3、E8、E16 | A 保留目标 checkout 内可发现路径；C 超出用户明确范围且需要新的进程/权限契约。 | 0.98 |
| D2 | 如何表达默认路径而不破坏 custom path？ | A. 所有调用点手拼；B. `WorktreeConfig` 内保留 custom override，并由同一 `worktree_path(repo_root)` 解析 default external；C. 新增另一套 factory | 选择 B。（session-settled: user-directed — chosen over scattered call-site paths: 创建、exists、reuse、supervisor 必须共享同一个解析入口。） | E1、E2、E4、E6、E7 | A 会重新制造 hardcode drift；C 会绕开已有 Git/cleanup/test seam。 | 0.96 |
| D3 | reuse/exists/supervisor 是否都迁移到同一 resolver？ | 只迁移普通 CLI；或迁移所有已确认使用 `WorktreeConfig::default` 的入口 | 迁移普通 CLI、reuse helper、supervisor factory；reviewer SharedReadonly 不新增 worktree。（session-settled: user-directed — executor 写盘边界是主要矛盾，reviewer 不扩大范围。） | E3、E4、E6 | 只迁移 CLI 会让 supervisor slot 仍写回旧根；reviewer 没有 worktree 创建调用。 | 0.97 |
| D4 | 是否增加 OS sandbox/Writer Workspace Contract？ | 增加强制 filesystem sandbox；或移动默认目录并保留现有 context/precheck | 选择仅移动默认目录，保持 context、event policy、precheck，不增加新机制。（session-settled: user-directed — chosen over sandbox/Task resume: 不改变当前恢复和权限控制面。） | E8、E16 | 本需求不是建立新的安全平台；sandbox 会扩大 API、平台和测试面。 | 0.95 |
| D5 | 最终 report path 应从哪里展示？ | A. 继续依赖 prompt 的 `DELIVERABLE_PATH`；B. 扫描 agent 输出；C. 从 runtime 已接受的 `last_completion_payload` 读取 | 选择 C；只从 accepted terminal payload 的顶层 `report_path`，其次 `artifact_path` 读取。（session-settled: user-directed — chosen over prompt-only hint: runtime 接受结果才是终态事实。） | E9、E10、E11、E12、E13 | A/B 都可能在 agent 遗漏或输出非结构化文本时静默缺失/错误。 | 0.97 |
| D6 | report schema/match gate 是否重做？ | 新增第二套 report guard；或复用当前 required fields + payload match | 选择复用当前 schema/match，只增加 display extraction 和回归测试。 | E9、E10、E12、E16 | 第二套 guard 会重复现有契约，且不是当前缺口。 | 0.96 |
| D7 | 哪些终态表面必须展示？ | 仅 prompt；仅 no-TUI；no-TUI + loop.terminate + RPC/TUI | 选择 no-TUI standalone marker、`loop.terminate` payload、RPC optional field/TUI footer 三个 runtime 表面；无 deliverable 的非 completion 终态不展示。 | E13、E14、E11 | 只改 prompt 已被当前问题证明不可靠；只改单一表面会让 TUI/RPC 仍丢失终态事实。 | 0.91 |
| D8 | 是否验证报告文件存在？ | runtime 新增 filesystem guard；继续由 reporter contract 验证，runtime 只展示 accepted path | 选择后者；不新增文件检查。 | E9 的 fill rule、E10、E12 | 用户本次要的是可靠展示，文件写入/可读性已有 reporter contract；新增 runtime I/O 会改变终态 gate 语义。 | 0.90 |

所有关键决策均达到 0.85；D7/D8 的剩余实现验证写入对应 Unit 的 Red/Integration，不把未验证细节留给 Executor 自行选择。

### 4. BDD 行为规格

#### Feature: 默认 worktree 在目标仓库外创建

  Background:
    Given 一个有初始 commit 的 Git repository `repo_root`
    And 当前 loop 的 exact name 为 `plan-a`

  Scenario: 默认 worktree 使用 project-scoped 外置路径
    Given 没有显式 custom `WorktreeConfig`
    When 调用 `ralph run --worktree` 的创建路径
    Then Git worktree 位于 `repo_root.parent()/worktree/<repo basename>/plan-a`
    And Git branch 仍为 `ralph/plan-a`
    And `repo_root/.worktrees` 不被创建

  Scenario: 默认创建不修改目标 repo 的 ignore 文件
    Given 目标 repo 没有 `.worktrees` ignore 条目
    When 默认 worktree 创建成功
    Then 目标 repo 的 `.gitignore` 不新增 `.worktrees/`
    And external worktree path 被 Git 注册

  Scenario: 相同 canonical name 已占用时 fail-closed
    Given `repo_root.parent()/worktree/<repo basename>/plan-a` 已存在
    When 再次执行新建而不是 reuse
    Then 返回现有 `AlreadyExists`/等价错误
    And 不覆盖目录、不新建随机 leaf、不修改该已有 worktree

  Scenario: `--reuse-worktree` 定位外置 worktree
    Given external canonical path 已是 Git worktree，且 registry entry 已结束
    When 以相同 plan basename 执行 `--reuse-worktree`
    Then 返回该 external path
    And 不创建第二个 Git worktree
    And 既有 archive/cleanup 行为保持不变

  Scenario: child 使用父进程传入的 path
    Given 父进程已创建 external worktree
    When child 以 `--worktree-path` 启动
    Then child cwd/workspace 是同一个 external path
    And child 不在目标 repo 或 external path 下再次创建 worktree

  Scenario: supervisor executor/fix slot 使用同一 external resolver
    Given supervisor 请求创建 executor 或 fixer slot
    When `DefaultWorktreeFactory` 创建 slot
    Then slot 位于同一个 project-scoped external root
    And SharedReadonly reviewer slot 仍不创建 worktree

#### Feature: 终态 report path 由 runtime 可靠展示

  Background:
    Given terminal schema 接受 `LOOP_COMPLETE`
    And `last_completion_payload` 是 runtime 保存的 accepted terminal JSON payload

  Scenario: accepted report path 出现在普通 CLI 最终输出
    Given accepted payload 为 `{"reason":"pass","report_path":".ralph/review/p/report.md"}`
    When loop 以 `CompletionPromise` 终止且不是 TUI/RPC 模式
    Then最终输出包含一条独立的 `DELIVERABLE_PATH: .ralph/review/p/report.md`
    And path 与 accepted payload 完全相同
    And不依赖 agent prompt 的最终可见文本

  Scenario: accepted artifact path 可作为通用 terminal deliverable
    Given accepted payload 没有 `report_path` 但有 `artifact_path: ".ralph/out/a.md"`
    When loop 以 `CompletionPromise` 终止
    Then runtime 使用 `.ralph/out/a.md` 作为 deliverable path
    And不得输出空 path 或猜测 report 文件名

  Scenario: 非 completion 终态不伪造 deliverable
    Given loop 因中断、超时或 safeguard 终止
    When没有被接受的 terminal payload
    Then普通 CLI、loop.terminate、RPC 都不产生 `DELIVERABLE_PATH`

  Scenario: report.done 与 LOOP_COMPLETE path 不一致时不展示错误路径
    Given accepted `report.done.report_path` 为 `p1.md`
    And `LOOP_COMPLETE.report_path` 为 `p2.md`
    When completion payload match gate 处理终态
    Then `LOOP_COMPLETE` 被拒绝并进入现有 correction/guard 语义
    And不打印 `p2.md`
    And直到一个匹配的 terminal payload 被接受前不展示 deliverable

  Scenario: RPC/TUI 获得与 CLI 相同的 path
    Given accepted terminal payload 含 `report_path: "p.md"`
    When loop 发布 loop.terminate/RPC LoopTerminated
    Then loop.terminate payload 和 RPC optional field 都是 `p.md`
    And TUI footer 显示同一 path
    And无 path 时现有 footer/RPC 行为不变

  Scenario: 重复处理终态不会产生第二个 path
    Given terminal payload 已被 accepted 且 completion 已 honored
    When termination observer 或 RPC consumer 重放同一个终态
    Then path 值保持相同
    And不生成第二个不同 path、不重写 report 文件

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| W-S1/W-S2 | 断言默认绝对 path 的三段层级、branch、目标 repo 无 `.worktrees`/ignore 变化。 | `crates/ralph-core/src/worktree.rs` tests；`integration_worktree_isolation.rs` | core unit + CLI integration | Characterization：旧 custom absolute path 继续工作。 | 需要，使用现有 `--worktree --no-tui` fixture，不调用真实 LLM。 |
| W-S3 | 目录占用返回 existing error，目录内容 hash/marker 不变。 | `worktree.rs` tests | unit | Fault injection：目标 parent 不可创建时错误可读且无部分注册。 | 不需要 |
| W-S4 | reuse 读取 external canonical path，不创建第二条 Git worktree；archive 副作用仍只发生在目标 worktree。 | `integration_worktree_isolation.rs` 现有 reuse scenarios | integration | Idempotency：重复 `--reuse-worktree` 不增加 worktree 数。 | 需要，现有真实 Git fixture。 |
| W-S5/W-S6 | child marker 写入 external workspace；supervisor slot path external；reviewer no-worktree 不变。 | `integration_worktree_isolation.rs`、`loop_runner/tests/wave_supervisor/slot_binding.rs` | integration/unit | Contract：registry `workspace == worktree_path`；不接受 `.worktrees` hardcode。 | W-S5 需要；W-S6 不需要真实 LLM。 |

每项测试都必须断言副作用和不变量：path 归属、Git worktree 数量、registry path、accepted/rejected 状态、输出 path 一致性；不能只断言“命令成功”。

### 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 默认 worktree 使用外置 project-scoped root | W-S1 | `default_worktree_path_is_external_and_project_scoped`（计划新增） | `worktree.rs` | `integration_worktree_isolation` path assertion | 是 | E1-E3 |
| R2 | 默认创建不改变目标 repo `.worktrees`/ignore | W-S2 | `default_creation_leaves_target_checkout_clean`（计划新增） | `worktree.rs` | CLI filesystem/gitignore assertion | 是 | E3、E7 |
| R3 | create/exists/reuse 使用同一 resolver | W-S3/W-S4 | existing name + reuse scenarios updated | `worktree.rs` resolver/reuse tests | existing integration reuse tests | 是 | E4-E5 |
| R4 | executor/fix slot external，reviewer 不创建 | W-S6 | slot binding contract test | `worktree_bind` tests | supervisor runtime tests | 否 | E6 |
| R5 | child cwd/write path 保持 external | W-S5 | existing headless marker test updated | adapter/LoopContext existing tests | `integration_worktree_isolation` | 是 | E8、E16 |
| R6 | accepted report path 在普通终态可靠展示 | R-S1/R-S2 | `termination_display_uses_accepted_payload`（计划新增） | `LoopState` extraction + display formatter | runner termination wiring test | 否 | E9-E13 |
| R7 | 非 completion 不伪造 path | R-S3 | `termination_without_deliverable_has_no_marker`（计划新增） | display/RPC optional field | termination state test | 否 | E11、E13 |
| R8 | mismatch path 不得展示 | R-S4 | existing mismatch test extended with no-display assertion | `completion_payload_match` existing tests | event loop termination | 否 | E12 |
| R9 | TUI/RPC 与 CLI 使用同一 path | R-S5 | RPC roundtrip + footer render tests | `RpcEvent`/TUI state tests | loop.terminate payload contract | 否 | E14 |
| R10 | 迁移后 agent-facing instructions 正确 | W-S1/W-S5 | guide/path smoke checks | 不适用 | `scripts/check-cli-doc-drift.sh` | 否 | E15 |

## Implementation Units

以下 Unit 严格串行；每个 Unit 必须完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close 后才能进入下一 Unit。

### Unit 1：建立唯一的外置 Worktree 路径解析行为

#### 1. Unit 目标

让默认 `WorktreeConfig` 对任意正常 Git repo 返回唯一 canonical base：`repo_root.parent()/worktree/<repo basename>`，并使 `create_worktree` 的最终 leaf 仍是 exact `loop_id`；custom `with_dir` 行为不改变。

#### 2. 对应需求与 Scenario

- Requirement：R1、R3；Scenario：W-S1、W-S3；Decision：D1、D2；Evidence：E1、E2、E4。

#### 3. 外部可观察结果

调用 core `create_worktree(repo_root, "plan-a", &WorktreeConfig::default())` 后，返回 path 精确位于 repo parent 的 `worktree/<project>/plan-a`，不会写 repo 内 `.worktrees`。

#### 4. 当前行为基线

当前默认 path 是 `repo_root/.worktrees/plan-a`，由 `WorktreeConfig::default` 与 `worktree_path` 直接决定；现有 `worktree.rs` tests 断言 default `.worktrees`。这些旧断言必须先作为 Characterization Red 运行，失败原因必须是目标默认路径变化，而不是 fixture 或编译问题。

#### 5. 输入与输出

- 输入：绝对 temp Git repo、project basename、`plan-a`、default/custom config。
- 输出：absolute `PathBuf` base、`Worktree` leaf path、现有 branch `ralph/plan-a`。
- 错误：repo 不存在/非 Git/目标 leaf 已存在，继续走现有 `WorktreeError`。
- 状态变化：default 创建只在 repo parent 下建立 external directory 和 Git worktree。
- 副作用：目标 repo 不新增 `.worktrees`；custom in-repo config 若被直接调用，保持其既有行为。
- 不变量：path 解析只由一个 resolver 决定；不接受 call-site 手拼路径。

#### 6. 修改位置

- `crates/ralph-core/src/worktree.rs::WorktreeConfig` / `worktree_path`：当前负责 default/custom directory 解析；修改 default external mode 和 project-name/parent 解析；不修改 Git command、branch 命名和 cleanup。
- `crates/ralph-core/src/worktree.rs` 内嵌 tests：当前覆盖 default/absolute path、create/remove；更新 default expectation，新增 parent/project/leaf/占用行为；不删除 custom absolute characterization。
- `crates/ralph-core/src/worktree.rs::sync_working_directory_to_worktree`：当前按 `worktree_dir` 排除同步输入；只修正其使用 resolved base 的判断以支持 external default；不改变同步哪些 tracked/unstaged 文件。

#### 7. 可依赖能力

- 现有 `tempfile`、Git test helper、`WorktreeError`、`create_worktree`、`get_head_commit`。
- 现有 `with_dir` custom override 和 `remove_worktree` absolute path。

#### 8. 禁止依赖的未来能力

- 不改 CLI `spawn_worktree_loop`、reuse call-site、supervisor factory；它们留给 Unit 2。
- 不改 report/display、RPC/TUI、docs。
- 不增加 sandbox、配置字段、随机 collision suffix 或 task resume。

#### 9. 验收测试

- 测试：`default_worktree_path_is_external_and_project_scoped`（计划新增）。前置：temp Git repo basename `demo`; 输入 `plan-a`; 动作：调用 `WorktreeConfig::default().worktree_path`/`create_worktree`; 断言：`<parent>/worktree/demo/plan-a`、branch `ralph/plan-a`、repo 内 `.worktrees` 不存在；副作用：Git list 含该 absolute path；不变量：custom `with_dir` test 仍绿。
- 测试：`default_worktree_rejects_occupied_leaf_without_overwrite`（计划新增）。断言已有 marker 保持。
- 命令：`cargo nextest run -p ralph-core -- worktree`。

#### 10. Acceptance Red

- 先运行现有 worktree default tests 和新增 path acceptance test。
- 预期 Red：旧 default expectation 看到 `.worktrees`，新增 test 看到实际 path 不是 parent/project root；这证明测试进入了真实 resolver。
- 无效 Red：`cargo` 参数错误、temp Git 初始化失败、测试未执行、permission/环境不可写、失败栈不经过 `WorktreeConfig`；出现这些必须修 fixture/调查，不得进入 Green。

#### 11. 单元测试拆分

1. default resolver：输入带有正常 basename 的 repo `/tmp/demo`，期望 `/tmp/worktree/demo`；不为 basename 缺失设计隐式 `project` fallback，也不得落回 `.worktrees`。若真实生产调用链允许无 parent/basename 的 repo root，按 Unit 19 停止并补证据后再决定错误语义。
2. leaf composition：输入 `plan-a`，期望 canonical base/leaf；含 supervisor branch slash 的 leaf 仍由现有 Git path 规则处理。
3. custom absolute：`with_dir("/tmp/custom")` 仍返回 `/tmp/custom`。
4. custom relative：现有 `repo.join(custom)` 行为仍保持。
5. collision：leaf 已存在时返回 existing error，Fake 只用于隔离 Git；不能 mock `worktree_path` 真实解析。
6. sync exclusion：external default 不会因为相对 `.worktrees` 字符串错误排除/复制 repo 文件；使用真实 temp filesystem。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：default path 仍为 `.worktrees` → 实现 default external resolver → Test 1 Green。
2. Test 2 Red：project/plan 层级缺失 → 最小补齐 project basename + leaf 组合 → Test 2 Green。
3. Test 3 Red：custom absolute/relative regression → 最小保留 override 分支 → Test 3 Green。
4. Test 4 Red：sync exclusion 使用旧字符串 → 改为基于 resolved base 的最小判断 → Test 4 Green。
5. Refactor：抽取单一私有 path helper，删除重复拼接；运行 Unit 全部测试。

#### 13. 最小实现范围

必须实现 default external resolver、正常 repo 的 project basename、exact leaf、custom override 保留、外置同步排除和现有错误传播。必须保持 branch、Git add、sync 内容、remove semantics。明确不实现 basename 缺失的隐式 fallback、collision suffix、旧 `.worktrees` migration、sandbox、CLI config。

#### 14. 集成验证

- 联合真实 `create_worktree`、`git worktree list`、`remove_worktree`、temp filesystem。
- Git CLI 必须真实运行；只可 Fake 不相关的 clock/registry fixture。
- 运行 `cargo nextest run -p ralph-core -- worktree`；预期所有 core worktree tests 通过。

#### 15. 风险驱动测试

- Characterization：custom absolute/relative path；原因是 default 解析重构可能误伤 public helper。
- Fault Injection：parent/worktree mkdir 失败；原因是默认 root 从 repo 内移出后权限边界改变。
- Idempotency：同 leaf 创建两次；原因是外部共享 project root 可能让 collision 更容易出现。

#### 16. 回归范围

- 直接：`crates/ralph-core/src/worktree.rs` 全部 tests。
- 相邻：`crates/ralph-api/tests/rpc_v1_loop_parity_regressions.rs` 的 `WorktreeConfig::default` 创建验证。
- 构建目标：`ralph-core`、`ralph-api`；fmt/clippy。
- 暂不跑 CLI integration，直到 Unit 2 接入 call-sites；不得以 Unit 2 的结果替代本 Unit close。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/worktree.rs` | 修改现有生产文件 | canonical external default/resolver 与 sync boundary | E1、E2 |
| `crates/ralph-core/src/worktree.rs` tests | 新增/修改测试 | path/边界/characterization | E1、E2 |

#### 18. 完成标准

Unit 1 的 path acceptance、unit tests、core worktree integration、相关回归、build/lint/typecheck 全通过；无 skip/断言削弱；没有修改 future Unit 行为；Evidence/Decision 仍 >=0.85；可独立提交。

#### 19. 停止条件

若 `repo_root` 在真实调用链不是预期 root、custom API 有未调查消费者、Git path 不能表达 project/plan 层级、Red 不是 resolver 缺失，立即停止；记录新 Evidence，重新比较 `D1/D2`，不得把 fallback 交给 Executor。

#### 20. 风险与注意事项

- 风险：同名 project 的不同 clone 共享 external project directory。触发：同一 parent 下两个 basename 相同的 repo 并发创建同名 plan。检测：Git worktree add/ownership test。缓解：保留现有 `AlreadyExists` fail-closed，不自动覆盖或随机改名。剩余风险：同名 clone 仍需用户使用不同 worktree name 或不同 parent。
- 风险：repo parent 不可写。检测：mkdir error。缓解：返回现有 IO error；不回退到 repo 内 `.worktrees`，避免静默破坏隔离目标。

### Unit 2：接通 CLI、reuse、supervisor 与 agent-facing 外置路径

#### 1. Unit 目标

让所有已确认的生产创建/检查/复用入口使用 Unit 1 的 canonical resolver，使一次 `--worktree` 运行只创建一个 external worktree，child、registry、cleanup 和 supervisor slot 都不回到目标 repo 内。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R3、R4、R5、R10；Scenario：W-S1～W-S6；Decision：D2、D3、D4；Evidence：E3-E8、E15。

#### 3. 外部可观察结果

真实 `ralph run --worktree --no-tui` 的 workspace/registry path 在 `<parent>/worktree/<project>/<plan>`；目标 repo 无 `.worktrees`；`--reuse-worktree` 和 supervisor executor/fix slot 仍命中同一根；agent-facing 文档不再给出目标 repo 内的错误路径。

#### 4. 当前行为基线

`spawn_worktree_loop` 默认创建并更新 `.gitignore`；reuse helper 不接 config；supervisor factory default 也走旧根；CLI integration helper 多处直接构造 `main_repo/.worktrees`。Acceptance Red 应由这些真实 `.worktrees` 断言和 fixture 路径失败产生。

#### 5. 输入与输出

- 输入：`--worktree`、`--plan`/`--worktree-name`、external canonical path、supervisor slot branch。
- 输出：`LoopContext::worktree`、registry absolute `workspace/worktree_path`、child cwd、reuse `ReusableWorktree`。
- 错误：existing/registry alive/not Git 的现有错误；不转为随机新路径。
- 副作用：默认不调用 `.gitignore` 写入；cleanup 删除 external path；目标 repo 只保留自身 `.ralph` runtime 状态。
- 不变量：父创建 child 不重复创建；registry path 与 context workspace 相等；review SharedReadonly 不创建。

#### 6. 修改位置

- `crates/ralph-cli/src/commands/run.rs::spawn_worktree_loop`：删除默认 `.worktrees` ignore side effect，传递 Unit 1 config 到 exists/create；不改 plan name 解析和 child `--worktree-path` 优先级。
- `crates/ralph-cli/src/commands/run.rs::run_command` reuse 分支：向 `find_reusable_worktree_by_name` 传入相同 config；不改 resume manifest/task semantics。
- `crates/ralph-core/src/worktree.rs::find_reusable_worktree_by_name`：增加 config/resolver 入参；保留 Git list/registry/alive 检查。
- `crates/ralph-core/src/supervisor/worktree_bind.rs::DefaultWorktreeFactory`：继续委托 core create，但确保 default 使用 Unit 1 external resolver；不改 slot env 或 Review branch。
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`：将测试 helper 从 `main_repo/.worktrees` 改成 canonical external helper；新增 target repo clean、external path、supervisor/child path assertion；不删除已有 parent/child duplicate coverage。
- `.cursor/rules/feature-flags.mdc`、`docs/advanced/parallel-loops.md`、`presets/en/merge-loop.yml`、`crates/ralph-core/data/ralph-tools-cmdref.md`：更新 `.worktrees/<id>` 为 resolver 语义或 `git worktree list` 查找方式；不修改 preset event topology，不添加 task resume。

#### 7. 可依赖能力

- Unit 1 已验证的 resolver；现有 `LoopContext::worktree`、registry、cleanup、child `--worktree-path`、supervisor slot binding。
- 现有 `crates/ralph-cli/tests/common/mod.rs` binary fixture、temp Git repo 和 nextest。

#### 8. 禁止依赖的未来能力

- 不实现 report display、RPC/TUI path。
- 不添加 OS sandbox、scope guard、new precheck 或 migration tool。
- 不修改 reuse manifest、task.resume、branch naming、auto-merge logic。

#### 9. 验收测试

- 更新 `test_worktree_creates_exactly_one_and_registry_correct`：断言 external path 层级与 registry。
- 更新 `test_worktree_no_duplicate_across_runs`、`test_reuse_worktree_*`、`headless_worktree_backend_writes_only_to_worktree`：所有 fixture 使用 resolver，断言目标 `.worktrees` 不存在且 marker 只在 external workspace。
- 新增 `test_default_worktree_does_not_modify_main_gitignore`：运行真实 CLI，比较前后 `.gitignore`。
- 更新 `slot_binding`/supervisor runtime contract：executor/fixer external，reviewer none。
- 命令：`cargo nextest run -p ralph-cli --test integration_worktree_isolation`；`cargo nextest run -p ralph-core -- worktree supervisor`。

#### 10. Acceptance Red

- 先运行现有 integration tests；预期 `.worktrees` hardcode helper 找不到 worktree，或 path assertion失败；这是 fixture/接线未迁移的真实 Red。
- `--reuse-worktree` Red 必须显示 lookup 没命中 external path，而不是 process crash。
- 无效 Red：backend 未启动导致测试没有执行 filesystem assertion、CLI 参数错误、临时 repo 初始化错误；不得把这些当作功能 Red。

#### 11. 单元测试拆分

1. `spawn_worktree_loop` 不写 `.gitignore`：Fake 只观察文件差异，Git creation 使用真实 helper。
2. `find_reusable_worktree_by_name` config path：输入 external path + live/dead registry，期望既有 alive/error semantics。
3. child path forwarding：使用已有 headless marker backend，禁止 mock cwd。
4. supervisor factory：输入 slot branch，期望 path 在 external root；review binding 仍 `worktree: None`。
5. docs command path：静态 drift/rg 只检查 active docs，不把历史报告当 active contract。

#### 12. Red → Green → Refactor 顺序

1. Integration path Red → 修改 CLI default create wiring → Green。
2. Reuse Red → 传入 canonical config 并更新 helper → Green。
3. Child headless Red → 保持已有 `--worktree-path` 传递，只修 path expectation → Green。
4. Supervisor Red → default factory 接入 canonical path → Green。
5. Gitignore Red → 删除默认 `.worktrees` 写入 → Green。
6. 文档/guide drift Red → 更新 active instructions → Green；最后 Refactor 测试 helper，运行完整 Unit 回归。

#### 13. 最小实现范围

必须修改 run/reuse/supervisor 入口和测试 fixture；必须保证 external path、registry、child cwd、cleanup 一致；必须更新 active docs。明确不改变 reuse recovery、merge queue、event policy、reviewer execution model。

#### 14. 集成验证

- 真实 Git + real ralph binary + custom headless backend；不调用外部 LLM。
- 真实 registry/LoopContext/cleanup；只 Fake 不相关 time/pid data。
- 命令：`cargo nextest run -p ralph-cli --test integration_worktree_isolation`、`cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0`、`scripts/check-cli-doc-drift.sh`。

#### 15. 风险驱动测试

- Contract：registry/workspace equality，原因是 path relocation 最容易出现 parent/child 使用不同 resolver。
- Idempotency：parent+child only one Git worktree，原因是最近提交 `1049cacb` 正在修复该边界。
- Fault Injection：reuse alive entry、non-Git path、occupied external path，原因是共享 project root 增加 collision。

#### 16. 回归范围

- 直接：`integration_worktree_isolation` 全部 tests、core worktree tests、supervisor slot binding。
- 相邻：`integration_subprocess_tui_lock`、`integration_run`、`integration_loops_merge`、API worktree parity。
- 文档：`scripts/check-cli-doc-drift.sh`；preset change 后执行 builtin preset lint/parity commands。
- 不修改/不回归历史 `.ralph/review` plan artifacts；不手工改 `.ralph` runtime state。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/commands/run.rs` | 修改现有生产文件 | create/reuse 使用 canonical config，取消默认 in-repo ignore | E3、E4 |
| `crates/ralph-core/src/worktree.rs` | 修改现有生产文件 | reuse helper 使用 canonical resolver | E4 |
| `crates/ralph-core/src/supervisor/worktree_bind.rs` | 修改现有生产文件 | supervisor executor/fix 复用 default external root | E6 |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 修改现有测试 | 外置路径/target clean/child cwd/reuse assertions | E7、E8 |
| `.cursor/rules/feature-flags.mdc` | 修改文档 | active worktree path contract | E15 |
| `docs/advanced/parallel-loops.md` | 修改文档 | operator path/cleanup instructions | E15 |
| `presets/en/merge-loop.yml` | 修改 builtin preset prompt | 不再硬编码 `.worktrees`，改为 Git list/branch lookup | E15 |
| `crates/ralph-core/data/ralph-tools-cmdref.md` | 修改 agent-facing guide | `--plan`/reuse path 说明与实际行为一致 | E15 |

#### 18. 完成标准

所有 W-S1～W-S6 通过；target repo 无默认 `.worktrees` 和 ignore mutation；custom path/reuse/cleanup regression 通过；active docs drift 通过；build/lint/typecheck 通过；没有新增恢复机制、skip 或断言削弱；可独立提交。

#### 19. 停止条件

若任一真实调用方仍自行拼 `.worktrees`、registry path 不等于 workspace、supervisor 创建落回旧根、文档改动触发 schema/topology drift、或需要新增配置字段，停止并更新 Evidence/D2/D3，不把发现留给后续 Unit。

#### 20. 风险与注意事项

- 风险：`presets/en/merge-loop.yml` 的命令上下文没有直接拿到 repo basename。检测：preset smoke/manual prompt inspection。缓解：使用 `git worktree list --porcelain` 按 branch `ralph/{loop_id}` 找绝对 path，不在 prompt 中手算目录；剩余风险是手工 merge agent 不遵守命令。
- 风险：已有用户 `.worktrees` 不会自动迁移。检测：旧目录仍存在但不被 default resolver 读取。缓解：文档明确新旧不迁移，用户显式 cleanup；剩余风险是旧目录占磁盘。

### Unit 3：从 accepted terminal payload 生成唯一 report deliverable

#### 1. Unit 目标

建立 core 的唯一 terminal deliverable 提取规则，并让普通终态 CLI 在 `CompletionPromise` 后输出准确的 `DELIVERABLE_PATH`；非 completion 或未提供 path 时不伪造输出。

#### 2. 对应需求与 Scenario

- Requirement：R6、R7、R8；Scenario：R-S1～R-S4；Decision：D5、D6、D8；Evidence：E9-E13。

#### 3. 外部可观察结果

agent 即使没有在 visible reply 打印 prompt 要求的 marker，runtime 仍会在 accepted terminal 后打印同一个结构化 path；不一致的 completion payload 不会把错误 path 送入 display。

#### 4. 当前行为基线

`event_processing.rs` 只向 final hat prompt 注入 marker；`LoopState::last_completion_payload` 已保存 accepted terminal JSON；`display::print_termination` 当前没有 deliverable 参数。Acceptance Red 应表现为 formatter 无 marker/提取函数不存在，而不是修改 schema 后的失败。

#### 5. 输入与输出

- 输入：`LoopState.last_completion_payload`、`TerminationReason`、`enable_tui/enable_rpc` 分流结果。
- 输出：普通 CLI exact standalone line；`loop.terminate` 的 payload 在 Unit 4 扩展，Unit 3 先提供共享 extraction/format contract。
- 错误：payload 缺失、JSON 非 object、path 非 string/空字符串 → `None`，不报成功 path。
- 状态：不写文件、不修改 completion state、不触发 recovery。
- 不变量：只在 `CompletionPromise` 且 accepted payload 存在时展示；report_path 优先 artifact_path；原始 path 不重写、不绝对化。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/loop_state.rs`：增加基于 `last_completion_payload` 的纯提取能力；不改 payload 保存和 completion gate。
- `crates/ralph-cli/src/display.rs`：增加可测试的终止文本/marker formatter，`print_termination` 使用它；不改变已有 status/resume 文案。
- `crates/ralph-cli/src/loop_runner/inner.rs`：终态 closure 从 state 取得 path 并传给 display；不在 agent output 文本中 grep path。
- `crates/ralph-core/src/event_loop/tests/termination.rs`、`crates/ralph-cli/src/display.rs` tests：覆盖 accepted/missing/malformed/non-completion/mismatch；不删现有 completion match tests。

#### 7. 可依赖能力

Unit 2 已完成的 worktree 不影响本 Unit；使用现有 `LoopState.last_completion_payload`、`TerminationReason`、display formatter 测试 seam。

#### 8. 禁止依赖的未来能力

- Unit 3 不修改 RPC/TUI 类型和 footer，留给 Unit 4。
- 不修改 report schema、reporter prompt、filesystem validation、completion retry/recovery。

#### 9. 验收测试

- `accepted_terminal_payload_provides_report_path`：payload `report_path` → exact string。
- `accepted_terminal_payload_falls_back_to_artifact_path`：无 report_path 有 artifact_path → exact artifact path。
- `invalid_or_missing_terminal_path_returns_none`：missing/null/number/empty → None。
- `completion_termination_prints_standalone_deliverable_marker`：formatter output 含 exactly one line；旧 termination status/resume unchanged。
- `non_completion_termination_does_not_print_marker`：MaxRuntime/Interrupted → no marker。
- 命令：`cargo nextest run -p ralph-core -- termination`、`cargo nextest run -p ralph-cli --bin ralph -- display`。

#### 10. Acceptance Red

- 先运行新增 extraction/formatter tests；当前代码应因缺少 method/参数或输出不含 marker 而失败。
- Red 必须在真实 `last_completion_payload`/formatter 逻辑上发生；若失败来自 serde fixture 语法、stdout 捕获工具或测试没有执行目标函数，则无效。

#### 11. 单元测试拆分

1. JSON object/report_path string；不允许从 nested field 或普通 text 猜路径。
2. artifact_path fallback；report_path 与 artifact_path 同时存在时 report_path wins。
3. malformed/non-string/whitespace-empty；期望 None。
4. completion reason + path formatter；精确 standalone marker。
5. non-completion reason；marker absent，既有 status/resume text unchanged。
6. duplicate invocation formatter；每次只产出一个 path line，不追加第二条。

#### 12. Red → Green → Refactor 顺序

1. extraction report_path Red → 最小 JSON field extraction → Green。
2. artifact fallback Red → 最小 fallback → Green。
3. malformed/empty Red → strict string/non-empty guard → Green。
4. CLI formatter Red → 增加 path argument/line → Green。
5. non-completion Red → completion-only conditional → Green。
6. Refactor：把 path extraction 保留在 core 单一 source，display 只负责格式化；运行 termination regression。

#### 13. 最小实现范围

只实现 accepted payload extraction、CLI final marker、严格 None behavior；保持 report schema/match/recovery、既有 termination labels、resume hints。不得验证文件存在、不得读取 agent prompt、不得写 report。

#### 14. 集成验证

- 联合真实 EventLoop accepted completion event、`check_completion_event`、runner termination closure、display formatter。
- 不 mock completion gate；可用 temp event JSONL fixture，不启动真实 backend。
- 运行 `cargo nextest run -p ralph-core -- termination` 和 `cargo nextest run -p ralph-cli --bin ralph -- display`。

#### 15. 风险驱动测试

- State-machine：mismatch rejected → no display；matching retry → display；原因是错误 path 泄漏是核心风险。
- Property-style table：非 object/非 string/空值；原因是不可信 agent payload 不能导致错误 marker。
- Idempotency：accepted terminal replay 读取同一 payload；原因是终态 observer 可能重复触发。

#### 16. 回归范围

- `crates/ralph-core/src/event_loop/tests/termination.rs` 全部 completion tests；
- `crates/ralph-core/src/event_loop/tests/build_prompt.rs` 保持 prompt contract tests；
- `crates/ralph-cli/src/display.rs` 全部 status/resume/footer output tests；
- `crates/ralph-cli/src/loop_runner/tests/legacy/termination.rs` 不得被新 marker 逻辑破坏；
- build/clippy/typecheck；不提前覆盖 Unit 4 RPC/TUI behavior。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/loop_state.rs` | 修改现有生产文件 | accepted terminal payload 的单一 path extraction | E11 |
| `crates/ralph-cli/src/display.rs` | 修改现有生产文件 | final standalone marker formatter | E13 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改现有生产文件 | accepted path 接入终态 display | E11、E13 |
| `crates/ralph-core/src/event_loop/tests/termination.rs` | 修改现有测试 | mismatch/no-display 与 extraction regression | E12 |
| `crates/ralph-cli/src/display.rs` tests | 新增/修改测试 | exact output contract | E13 |

#### 18. 完成标准

R-S1～R-S4 通过；accepted path exact display；missing/non-completion 无伪造；mismatch 不展示；prompt/schema/match tests 仍绿；build/lint/typecheck 通过；无文件写入/恢复语义变化；可独立提交。

#### 19. 停止条件

若 `last_completion_payload` 在真实 accepted path 未保存、终态 closure 在其他入口复制、display 只能依赖 stdout capture、或 completion mismatch 语义与 Evidence 不一致，停止并更新 D5/D6/D8；不得改用 agent text grep。

#### 20. 风险与注意事项

- 风险：某些自定义 preset 的 completion schema 使用 `artifact_path` 而非 `report_path`。检测：preset structural test/fixture。缓解：固定 report-first/artifact-second；剩余风险是自定义 payload 不含两者时无 path，这是正确 fail-closed 展示。
- 风险：path 字符串是 repo-relative 但 runtime worktree cwd 不同。检测：assert 不做 `join(current_dir)`，保持原始 relative value。缓解：display 原样输出；消费方按 workspace 解析。

### Unit 4：把同一 deliverable path 传到 loop.terminate、RPC 和 TUI

#### 1. Unit 目标

让非 no-TUI 表面也使用 Unit 3 的同一 accepted path：`loop.terminate` payload、RPC `LoopTerminated.deliverable_path` 和 TUI footer；无 path 的旧行为保持。

#### 2. 对应需求与 Scenario

- Requirement：R6、R7、R9；Scenario：R-S3、R-S5、R-S6；Decision：D5、D7；Evidence：E11、E13、E14。

#### 3. 外部可观察结果

RPC consumer 不再需要读取 prompt 或 event ledger 才能知道最终 report path；TUI 在完成状态下显示 `DELIVERABLE_PATH: <path>`；`loop.terminate` observer 得到同一 value。

#### 4. 当前行为基线

`RpcEvent::LoopTerminated` 当前只有 reason/iteration/duration/cost/timestamp；TUI `LoopTerminated` 只设置 completed；in-process TUI 的 `loop.terminate` handler 只冻结计时器。现有 roundtrip/footer/state tests 是基线。

#### 5. 输入与输出

- 输入：Unit 3 的 `LoopState` extraction；accepted completion or None。
- 输出：`loop.terminate` payload 追加可选 deliverable line；RPC optional `deliverable_path: Option<String>`；TUI state/footer path line。
- 错误：None 时不新增字段/不新增 footer line；serde old JSON 无字段仍成功解析。
- 状态变化：只更新 observer/UI state，不修改 core completion state。
- 副作用：不写 report、不重放事件、不触发 recovery；终态重复处理保持相同 path。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/completion_and_termination.rs::publish_terminate_event`：从 state 取 Unit 3 extraction 并把相同 path 放入 system event payload；不改 termination reason/status。
- `crates/ralph-proto/src/json_rpc.rs::RpcEvent::LoopTerminated`：增加 optional `deliverable_path`，serde 缺省兼容；不改现有字段命名。
- `crates/ralph-cli/src/loop_runner/inner.rs`：构造 RPC event 时传同一个 extracted path；不重新 parse/normalize。
- `crates/ralph-tui/src/state.rs`、`crates/ralph-tui/src/state_mutations.rs`：保存 path；`crates/ralph-tui/src/rpc_source.rs`/`rpc_bridge.rs`：消费 event/loop.terminate；`crates/ralph-tui/src/widgets/footer.rs`：完成状态显示 path。只新增 path 行，不重排既有 footer contract。
- `crates/ralph-proto/src/json_rpc.rs` tests、`ralph-tui` state/source/footer tests：新增 roundtrip/None/same-path assertions。

#### 7. 可依赖能力

Unit 3 已验证的 core extraction 和 CLI formatter；现有 RPC/TUI event routing、footer ratatui TestBackend、serde roundtrip tests。

#### 8. 禁止依赖的未来能力

- 不修改 report schema、reporter prompt、completion match、filesystem validation。
- 不新增数据库/ledger/report store，不改 TUI navigation/search/guidance。

#### 9. 验收测试

- `loop_terminate_payload_carries_accepted_deliverable`：accepted report path → system event payload exact line；None → no line。
- `loop_terminated_rpc_roundtrip_preserves_optional_deliverable`：Some path roundtrip；旧无字段 JSON parse 为 None。
- `tui_loop_terminate_and_rpc_show_same_deliverable`：两种输入途径更新同一 state；footer render exact marker。
- `duplicate_terminal_observation_is_idempotent`：同 event 两次不产生不同 path/重复 state mutation。
- 命令：`cargo nextest run -p ralph-proto -- json_rpc`、`cargo nextest run -p ralph-tui -- rpc_source footer state`、`cargo nextest run -p ralph-core -- termination`、`cargo nextest run -p ralph-cli -- loop_runner`。

#### 10. Acceptance Red

- 先运行新增 RPC/TUI tests；当前类型无 field、TUI 无 state、footer 无 marker，应产生编译/断言 Red；这是目标能力缺失的有效 Red。
- `loop.terminate` payload test 必须真实调用 `publish_terminate_event`；只拼字符串的 fixture failure 无效。
- 旧无-field RPC JSON parse 失败属于兼容性实现错误，不是可接受 Red。

#### 11. 单元测试拆分

1. `publish_terminate_event` report path exact line；保留 reason/status assertions。
2. no path event payload unchanged except no marker。
3. RPC Some/None serde roundtrip；不允许把 optional field 变为必填。
4. TUI in-process event path extraction；与 RPC source path extraction 得到同一 value。
5. footer width/path display；旧 ACTIVE/DONE/elapsed assertions仍通过。
6. duplicate event idempotency；不重复 append path line。

#### 12. Red → Green → Refactor 顺序

1. system event Red → 接入 core extraction → Green。
2. RPC type Red → optional field + producer wiring → Green。
3. TUI state/source Red → 保存/消费 optional path → Green。
4. footer Red → 完成态 path line + width layout → Green。
5. duplicate/None Red → conditional idempotent update → Green。
6. Refactor：所有 producer 只调用 Unit 3 extraction，不复制 JSON parsing；运行三 crate integration。

#### 13. 最小实现范围

必须实现三种终态表面的同源 path 传递、None compatibility、exact output and no duplicate。不得新增 event topic、不得改 RPC command、不得做 filesystem check、不得变更 report content。

#### 14. 集成验证

- 真实调用 core `publish_terminate_event`、CLI RPC producer、proto serde、TUI event consumer/footer TestBackend。
- 允许使用 in-memory TUI state/TestBackend；不 mock extraction。
- 执行 Unit 4 commands 和 `cargo nextest run -p ralph-cli --test integration_run`；预期 old completion/non-completion UI tests 全绿。

#### 15. 风险驱动测试

- Contract：RPC JSON old/new roundtrip，原因是 `ralph-proto` 是外部 consumer boundary。
- UI component/golden-like：footer exact text，原因是 final path 必须被人可靠看到但不能破坏布局。
- Idempotency：重复 terminal observer，原因是 loop terminate/RPC may be delivered through more than one path。

#### 16. 回归范围

- 直接：`ralph-proto` JSON RPC tests；`ralph-tui` state/source/footer tests；core termination tests；CLI runner tests。
- 相邻：`integration_run`、`integration_subprocess_tui_lock`、RPC/TUI bridge tests。
- 旧配置/数据：无 deliverable 的 default config、旧 RPC JSON、非 completion termination；必须保持 None 行为。
- 最终全量：`./scripts/run-tests.sh`、`./scripts/ci-rust-gate.sh`；不得以单 crate green 宣布完成。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/completion_and_termination.rs` | 修改现有生产文件 | system termination payload 同源展示 | E11、E14 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改现有生产文件 | RPC producer 传 accepted path | E13、E14 |
| `crates/ralph-proto/src/json_rpc.rs` | 修改现有生产文件 | optional machine-readable deliverable | E14 |
| `crates/ralph-tui/src/state.rs` | 修改现有生产文件 | 保存终态 path | E14 |
| `crates/ralph-tui/src/rpc_source.rs`、`rpc_bridge.rs` | 修改现有生产文件 | 两种 TUI input path 同源 | E14 |
| `crates/ralph-tui/src/widgets/footer.rs` | 修改现有生产文件 | final visible path | E14 |
| 上述各文件现有 tests | 新增/修改测试 | contract/roundtrip/UI/idempotency | E14 |

#### 18. 完成标准

R-S3/R-S5/R-S6 通过；CLI、loop.terminate、RPC、TUI path 完全一致；None/旧 JSON/非 completion regression 通过；build/lint/typecheck/全量测试通过；无新增 skip/弱断言；可独立提交。

#### 19. 停止条件

若 TUI 有未调查的第二条终态输入路径、RPC consumer 要求必填字段、system event payload 被其他 parser 依赖且不能兼容、或同一个 path 需要重复 parse，停止并更新 Evidence/D7，不让 Executor临时选择兼容方案。

#### 20. 风险与注意事项

- 风险：TUI footer 高度只有两行测试 backend，path 行可能挤压旧信息。检测：现有 `render_to_string_with_width` tests。缓解：在完成态优先显示 path，保持 ACTIVE/非完成布局不变；剩余风险是极窄终端需要截断策略，必须由已有 ratatui layout 的可验证行为决定，不静默丢 path。
- 风险：RPC optional field 改变 JSON snapshot。检测：proto roundtrip/serialized JSON tests。缓解：`skip_serializing_if` 仅在 None 时省略，Some 时精确输出；解释所有 snapshot 差异。

## Verification Contract

### 8. Unit 串行依赖图

```text
Unit 1
  ↓ canonical external resolver 已通过 core tests
Unit 2
  ↓ 所有创建/复用/child/supervisor 入口已通过 external integration
Unit 3
  ↓ accepted terminal payload extraction + no-TUI display 已通过 state/CLI tests
Unit 4
```

- Unit 2 不能早于 Unit 1：否则 CLI 和 supervisor 会各自猜测 default path。
- Unit 3 不能早于 Unit 2：先把 worktree runtime regression 收敛，避免终态测试受到未完成的 runner wiring 干扰；Unit 3 本身不依赖 report schema 改动。
- Unit 4 不能早于 Unit 3：RPC/TUI 必须消费已验证的 extraction，不得各自 parse payload。
- 每个 Unit 完成后才允许使用其能力；后续 Unit 不得提前修改未来表面。

### 9. 执行命令清单

以下命令是实现阶段命令，不代表本计划阶段已运行。命令失败时不得进入下一步，除非失败被明确证明与目标行为无关并记录新证据。

| 时机 | 命令 | 目的 | 预期结果 | 失败处理 |
|---|---|---|---|---|
| Unit 1 Red/Green | `cargo nextest run -p ralph-core -- worktree` | core resolver/create/remove/sync tests | 旧断言先 Red，最小实现后全绿 | 非目标 Red 停止调查 |
| Unit 1 相邻 | `cargo nextest run -p ralph-api -- rpc_v1_loop_parity_regressions` | WorktreeConfig public consumer | 既有 API parity 通过 | 不允许跳过 |
| Unit 2 Red/Green | `cargo nextest run -p ralph-cli --test integration_worktree_isolation` | 真实 CLI Git worktree/child/reuse path | external path、single create、target clean 全通过 | 不允许用 mock 替代真实 cwd/Git |
| Unit 2 supervisor | `cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0` | supervisor runtime path regression | executor/fix slot 不落旧根 | 失败阻止 Unit 3 |
| Unit 2 docs | `scripts/check-cli-doc-drift.sh` | CLI/agent guide 参数和 active path drift | exit 0 | 修文档后重跑 |
| Unit 2 preset structure | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`；`cargo nextest run -p ralph-core -- preset_lint`；`cargo nextest run -p ralph-cli --bin ralph -- presets` | builtin YAML/schema/manifest parity | 全部通过；仅 prompt 改动也要确认无 drift | 不允许只改 embedded 内容不验证 |
| Unit 3 Red/Green | `cargo nextest run -p ralph-core -- termination` | accepted payload extraction/mismatch/no-display state | exact path and no-path semantics | 失败阻止 Unit 4 |
| Unit 3 display | `cargo nextest run -p ralph-cli --bin ralph -- display` | formatter/status/resume regression | marker exact、旧文案保持 | 不能删/ignore visual tests |
| Unit 4 proto | `cargo nextest run -p ralph-proto -- json_rpc` | optional field JSON contract | new/old JSON roundtrip | 兼容失败必须修复 |
| Unit 4 TUI | `cargo nextest run -p ralph-tui -- rpc_source footer state` | final path visible in both TUI paths | path exact、None old behavior | 不允许扩大布局改动 |
| Unit 4 CLI adjacent | `cargo nextest run -p ralph-cli --bin ralph -- loop_runner`；`cargo nextest run -p ralph-cli --test integration_run`；`cargo nextest run -p ralph-cli --test integration_subprocess_tui_lock` | runner and subprocess TUI regression | completion/non-completion unaffected | 失败阻止全量 |
| 每 Unit close | `cargo fmt --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo build` | formatting/lint/build/typecheck-equivalent compile gate | exit 0 | 失败必须修复，不得降 lint |
| 最终 gate | `./scripts/run-tests.sh` | workspace nextest + required phases/doctest | 全量通过 | 允许按 AGENTS 规定 serial fallback 诊断，但 serial 失败仍是真失败 |
| 最终 CI gate | `./scripts/ci-rust-gate.sh` | fmt/clippy/full Rust gate | exit 0 | 不得宣称完成 |

注意：`ralph-cli` 测试禁止裸 `cargo test -p ralph-cli`；全量不得手动替代 `./scripts/run-tests.sh`。涉及 preset YAML 的 Unit 2 必须检查对应 schema；本计划没有 event topology/required field 变更，因此预期 schema 内容不变，但必须用上述命令验证。

### 10. 最终质量门禁

- W-S1～W-S6、R-S1～R-S6 全部有测试并通过；每个 Scenario 都能追踪到 Unit。
- core worktree、event termination、CLI runner/display、proto RPC、TUI state/source/footer 通过。
- accepted/mismatch/missing/non-completion、custom path、reuse、parent-child、supervisor/reviewer boundary、old RPC JSON 均验证。
- 没有新增 skip/only/ignore、没有删除或模糊化断言、没有无解释 snapshot/golden 更新。
- `cargo fmt --check`、clippy、build、`scripts/check-cli-doc-drift.sh`、builtin preset checks、`./scripts/run-tests.sh`、`./scripts/ci-rust-gate.sh` 全部通过。
- 没有修改 `.ralph` 运行时状态文件；没有新增恢复机制、新 sandbox、新依赖或 report filesystem guard。
- 最终实际变更不超出预期文件；四个 Unit 按 1→2→3→4 完成并各自满足 Close。
- 所有 Decision 置信度仍 >=0.85；剩余风险仅为同名 clone collision、已有旧 `.worktrees` 不迁移、以及 agent 主动使用任意绝对路径的边界。

## Definition of Done

实现者只能在以下全部满足后声明该计划完成：

1. 默认普通 worktree、reuse、supervisor executor/fix 均位于 `<repo-parent>/worktree/<project>/<plan>`；目标 repo 默认无 `.worktrees` 创建和 `.gitignore` mutation。
2. `LoopContext`、registry、child cwd、cleanup 对同一 external absolute path 达成一致；reviewer SharedReadonly 未被扩大为 worktree writer。
3. accepted terminal payload 的 report/artifact path 经单一 extraction 进入 CLI、loop.terminate、RPC/TUI；path mismatch/no path 不会伪造展示。
4. report schema、completion match、reporter 文件生成和现有恢复语义未被重复实现或改变；没有新增恢复机制。
5. 全部测试/验证命令通过，且每个 Unit 的 Red 是目标行为缺失、Green 是最小实现、Refactor/Integration/Regression/Close 均有证据。
6. active docs/guides 已更新为新路径，历史 plan/report artifacts 未被改写，所有 Evidence/Decision 记录与最终 diff 一致。

## Appendix

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指向已确认符号、Red、最小实现、命令和 Close。 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D8 已选择路径、source、错误和表面；未决只剩 Unit 内可执行验证。 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E16；计划新增测试均明确标为计划新增，未把猜测路径当既有事实。 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D8 为 0.90-0.98，并有直接代码证据。 |
| 是否存在未处理的低置信度假设 | 否 | 两个待验证假设有明确测试动作且不改变关键方向；无 BLOCKED 决策。 |
| 每个 Unit 是否只有一个可观察行为 | 是 | Unit 1 resolver、Unit 2 runtime wiring、Unit 3 CLI accepted display、Unit 4 RPC/TUI propagation。 |
| 每个 Unit 是否可以独立验证 | 是 | 每 Unit 有独立 Red/Green/Integration/Regression 命令和 Close。 |
| 每个 Unit 是否有真实 Red | 是 | Red 都绑定当前真实 hardcode/缺少 field/缺少 formatter 行为；无效 Red 已列出。 |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 节逐 Unit 列出直接、相邻、旧配置/数据和 build gate。 |
| 是否存在未来 Unit 依赖 | 否 | 只有线性已完成能力依赖；禁止依赖未来行为已写入第 8 节。 |
| 是否存在泛化任务描述 | 否 | 没有“完善逻辑/添加测试”孤立描述，均有对象、输入、断言和命令。 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节逐项关联，R1-R10 均有 Scenario/测试。 |
| 所有关键决策是否有 Evidence | 是 | D1-D8 均引用 E1-E16。 |
| 计划是否可以严格串行执行 | 是 | Unit 1 → 2 → 3 → 4，前一 Unit close 后才进入下一。 |

### 12. 计划边界说明

本计划不声称外置 worktree 可以阻止 agent 对任意绝对路径的恶意写入；它解决的是用户指出的“普通 worktree 被放在 target branch 内、agent 容易发现并改到其他 checkout”这一主要矛盾。现有 precheck/event policy 继续承担结构化 emit 和 scope 的既有职责。report 侧不再把 prompt 的最后一行当作最终交付机制，而是把已接受终态 payload 变成 runtime 的最终展示事实。
