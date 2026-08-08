---
title: "refactor: 将 9 个超 5000 行大文件拆分为模块（纯结构重构，零行为变更）"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
type: refactor
created: 2026-08-05
baseline_branch: pittcat-dev
baseline_commit: 552a5d41
---

# refactor: 将 9 个超 5000 行大文件拆分为模块（纯结构重构，零行为变更）

---

## 0. 计划状态

**READY** — 所有实施关键决策置信度均 ≥ 0.85。

| 项目 | 内容 |
|---|---|
| 代码基线 | branch `pittcat-dev` @ `552a5d41`（`chore: archive completed plans to docs/achieved/plan`） |
| 调查范围 | 9 个目标文件的项级结构大纲、`#[cfg(test)]` 分布、仓库既有目录模块先例、edition、文档行号引用、全量测试基线 |
| 已执行验证命令 | ① `wc -l` 全文件行数统计；② 每文件 `#[cfg(test)]` / 顶层项大纲提取；③ `./scripts/run-tests.sh` 全量基线 **通过**（Phase 1: 7514 pass / 0 fail / 34 skip，50.2s；Phase 2: 23 pass / 0 fail，5.5s；Doctest: 19 pass / 4 ignored，0.75s；总耗时 78s，exit 0）；④ `cargo nextest --version` = 0.9.140（与 mise.toml 钉死版本一致） |
| 尚未执行的验证 | 每个 Unit 执行前的当次 `./scripts/run-tests.sh` 前置绿确认（防止基线被其它提交移动）；`.cursor/rules/state-management.mdc` 行号引用的逐条 sed 复核（在对应 Unit 内执行） |
| 阻塞项 | 无 |

---

## 1. 功能目标

- **业务目标**：将仓库中 9 个超过 5000 行的 Rust 源文件拆分为更小的模块文件，降低单文件认知负荷与维护成本。
- **用户或调用方**：本仓库开发者（含 AI agent）；所有 crate 的下游消费者（ralph-cli / ralph-tui / ralph-api / ralph-e2e / ralph-bench 及外部调用方）。
- **当前行为**：9 个文件单体过大（5,042 ~ 17,513 行），其中 `event_loop/mod.rs` 内含单个 ~13,800 行 `impl EventLoop` 块与单个 ~3,777 行方法 `process_parse_result`。
- **目标行为**：每个目标文件拆为「根文件 + 多个子模块文件」的目录/兄弟模块结构，单文件行数显著下降。
- **行为差异（严格定义）**：**外部可观察行为零差异**。具体不变量：
  1. 全量测试（nextest Phase 1 + Phase 2 + doctest）逐项通过，计数与基线一致（7514 / 23 / 19 pass + 4 ignored）；
  2. 测试 ID 集合不变（例外见 D7：两个纯测试文件拆分允许模块路径中段变化，但测试名多重集与计数不变）；
  3. `cargo clippy --all-targets --all-features -- -D warnings` 零告警；
  4. `cargo fmt --all -- --check` 零 diff；
  5. 所有 crate 的公开 API 路径（`pub` 项的 `crate::module::Item` 路径）不变；
  6. CLI 行为、事件语义、preset 行为、产物格式无任何变化。
- **本次范围**：仅下列 9 个文件的**项级搬移**（整个 fn / struct / enum / impl / mod 块原样移动到新模块文件）+ 必要的机械性可见性/导入调整 + 受影响活跃规则文档的行号引用更新。
- **非目标**：
  - ❌ 不拆分任何函数体（包括 `process_parse_result` ~3,777 行、`run_loop_impl_inner` ~4,300 行、`validate_event_with_options` ~1,090 行、`build_prompt` ~740 行等巨型单方法——原样整体搬移）；
  - ❌ 不重命名任何项、不改签名、不改返回值、不改错误语义；
  - ❌ 不调整任何逻辑顺序、条件、短路行为；
  - ❌ 不删除/削弱/新增任何测试断言；
  - ❌ 不重写历史文档（`docs/reviews/`、`docs/superpowers/`、`docs/solutions/`、`ux-findings.md` 中的历史行号引用是时间快照，不改）；
  - ❌ 不做任何「顺便」清理（dead code、注释润色、格式重排）。
- **输入 / 输出 / 状态变化**：无运行时输入输出变化；磁盘状态、Git 记忆、事件 ledger 语义全部不变。
- **错误语义**：不变。编译期错误类型与消息允许因模块路径变化而不同（仅编译期，非运行时行为）。
- **兼容性要求**：公开 API 路径 100% 保持；`ralph` 二进制 CLI 接口不变；preset YAML / schema 不变。
- **性能要求**：无变化要求（编译时间允许波动）。
- **安全或权限要求**：无新增。
- **已知约束**（来自 CLAUDE.md HARD RULES）：
  - 测试入口必须 `cargo nextest run` 系列 / `./scripts/run-tests.sh`，禁止裸 `cargo test -p ralph-cli`；
  - 全量基线两阶段（Phase 1 并行 + Phase 2 串行隔离 3 个 partial_timeout 测试），不得手动 `cargo nextest run --workspace` 替代；
  - `.cursor/rules/*.mdc` 是活跃规则文档，行号引用漂移须同步；`crates/ralph-core/data/*.md` 已确认无行号引用（无需同步）；
  - 完成任何任务前必须跑 `./scripts/run-tests.sh`。
- **已确认假设**：无（全部关键事实有 E 编号证据）。
- **待验证假设**：无进入实施路径的假设。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

9 个目标文件（行数来自 E1）：

| # | 文件 | 行数 | 生产区 | 测试区 | 备注 |
|---|---|---|---|---|---|
| F1 | `crates/ralph-core/src/event_loop/mod.rs` | 17,513 | 1-15247（含单个 impl EventLoop 块 1095-~14936） | 尾部 6 个内联 cfg(test) mod（15248+）；`mod tests;`（行 82）已外挂 `event_loop/tests/` 目录 | 已是目录模块根，52 个子模块 |
| F2 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 12,452 | 1-6756（全部顶层项已逐项列全，见 E3b） | 单个 `mod tests`（6758-末尾，~5,694 行） | `wave/` 目录内兄弟文件 |
| F3 | `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 9,423 | 无（纯测试） | 108 个扁平 `#[test]` fn + helper struct（SpyBindingBridge 等） | 由 `tests/mod.rs` 行 26 `mod wave_supervisor;` 声明 |
| F4 | `crates/ralph-core/src/event_policy.rs` | 8,329 | 1-3099 | 单个 `mod tests`（3100-末尾，~5,200 行） | 独立文件 |
| F5 | `crates/ralph-cli/src/commands/emit.rs` | 6,635 | 1-792 + **856-2204**（maybe_derive_triggered_for_isolated / should_warn_on_missing_default_config / emit_command_with_root_and_hats，E3c 修正）+ 尾部 `pub mod schema_view`（6518，~117 行） | 6 个命名 cfg(test) mod（794/2206/5921/6010/6119/6308）；838 处 cfg(test) 挂在 `emit_command_with_root` 单个 fn 上（test-only 入口包装） | `commands/` 目录内 |
| F6 | `crates/ralph-cli/src/policy_check.rs` | 5,586 | 1-842 + **843-1245 的 unified 运行器族**（`#[cfg(test)]` 挂在 `run_policy_check_unified` fn 上，E3c） | 5 个命名 cfg(test) mod：read_main_ledger_topics_tests(1246,~101) / u6_unified_path_tests(1348,~1110) / u1_warn_parity_tests(2459,~152) / u2_structured_feedback_tests(2612,~695) / tests(3308,~2278) | 独立文件 |
| F7 | `crates/ralph-cli/src/loop_runner/runner.rs` | 5,457 | 1-5130（含单个 async fn `run_loop_impl_inner` 826-~5130；613/629/639/651 处为 run_loop_impl 函数体内的 **`cfg(!test)`/`cfg(test)` 条件 let 语句**，E3d） | 2 个 cfg(test) mod（sync_timeout_tests 5264 / u1_preset_name_aware_lint_gate_wiring 5328） | `loop_runner/` 目录内 |
| F8 | `crates/ralph-cli/src/loop_runner/tests/legacy.rs` | 5,316 | 无（纯测试） | 109 个扁平 `#[test]` fn | 由 `tests/mod.rs` 行 23 `mod legacy;` 声明 |
| F9 | `crates/ralph-cli/src/task_cli.rs` | 5,042 | 1-3090（clap Args 结构群 + load_coordinator_hats + execute 命令族 + verify 族） | 4 个命名 cfg(test) mod（3091/4512/4639/4818）；565/1197/1425/2086/2109 处为挂在**单个生产 fn** 上的 `#[cfg(test)]`（test-only 变体：read_current_loop_id / add_task_with_args / ensure_task_with_args / build_close_warning_payload / build_close_warning_payload_missing_marker），随宿主 fn 搬移 | 独立文件；生产区实际 ~3,090 行（E3a 修正） |

仓库既有的模块拆分先例（E4）：
- **兄弟目录模式**：`event_loop/emit_gate.rs` + `event_loop/emit_gate/`、`stage_pipeline.rs` + `stage_pipeline/`、`repair_flow.rs` + `repair_flow/` 等大量先例——文件保留为模块根，子模块放同名目录。
- **目录 mod.rs 模式**：`loop_runner/tests/mod.rs` 聚合兄弟测试模块。
- edition = 2024（E5），兄弟目录路径解析完全支持。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `wc -l` 对 9 个文件 | 行数如上表（5,042-17,513） | 确定目标清单与排序 | 高 |
| E2 | `./scripts/run-tests.sh`（2026-08-05，HEAD 552a5d41） | 全量绿：Phase 1 7514 pass/0 fail/34 skip；Phase 2 23 pass/0 fail；Doctest 19 pass/4 ignored；exit 0，78s | 基线计数成为每 Unit 的回归不变量 | 高 |
| E3 | `grep #[cfg(test)]` + 顶层项大纲（9 文件） | 每个文件的测试/生产区边界、内联测试 mod 名单、巨型单方法位置（见 2.1 表与 U9-U11 区域表） | 拆分方案与搬移清单直接由此导出 | 高 |
| E4 | `ls crates/ralph-core/src/event_loop/` 等 | 兄弟目录模式（`foo.rs` + `foo/`）与目录 mod 模式均有大量先例 | 选定 D1 拆分模式 | 高 |
| E5 | `Cargo.toml` `edition = "2024"` | workspace edition 2024 | 兄弟目录子模块路径解析可用 | 高 |
| E6 | `impl EventLoop` 块方法级大纲（awk 提取，覆盖 1095-14937 全部 ~200 方法起始行） | 单个 impl 块；方法按行号连续分布；`process_parse_result` 单方法约 9416-13192 | U11 区域表按行号区间机械划分，方法整体搬移 | 高 |
| E7 | `rg '\.rs:[0-9]'` 扫 `crates/ralph-core/data/`、`CLAUDE.md`、`AGENTS.md`、`.cursor/rules/` | data/*.md **零**行号引用；CLAUDE.md/AGENTS.md **零**引用；`.cursor/rules/state-management.mdc` 有 4 条引用指向 event_policy.rs（行 174、420+573）、policy_check.rs（行 1331-1390）、event_loop/mod.rs（行 1816-1835） | 仅 state-management.mdc 需在对应 Unit 内同步；data/*.md 无需动 | 高 |
| E8 | `rg` 扫 `docs/reviews/`、`docs/superpowers/`、`docs/solutions/`、`ux-findings.md` | 历史文档含大量指向大文件的行号引用 | 属历史快照，列为非目标不改写 | 高 |
| E9 | `loop_runner/mod.rs:152` `mod tests;` + `tests/mod.rs:18-26` | tests 目录模块声明链：`mod.rs → tests/mod.rs → {common, fake_path, hard_gate, ..., legacy, wave_supervisor}` | F3/F8 拆分沿用该声明链 | 高 |
| E10 | `cargo nextest --version` | 0.9.140，与 mise.toml SSOT 一致 | 回归命令可用 | 高 |
| E11 | `justfile` 行 13-40 | `just lint` = `cargo clippy --all-targets --all-features -- -D warnings`；`just fmt-check` = `cargo fmt --all -- --check`；`just test-parallel` = `./scripts/run-tests.sh` | 每 Unit 静态门禁命令 | 高 |
| E12 | `scripts/run-tests.sh` 头部注释 | 两阶段策略：Phase 1 `-E 'not test(/partial_timeout_events_visible/)'` 并行；Phase 2 `-E 'test(/partial_timeout_events_visible/)' -j 1`；脚本自带 agent-env scrub | 全量回归唯一合法入口 | 高 |
| E3a | task_cli.rs 565-3090 区间逐项大纲 | 生产区实际延伸至 3090：validation/filter 族（565-1022）、execute + add/ensure 族（1023-1641）、list/ready/start/close 族（1642-2162）、fail/show/confirm/reopen/verify 族（2163-3089）；5 处 cfg(test) 均挂在单个 fn 上 | U1 生产区五分组表与行数的依据 | 高 |
| E3b | dispatcher.rs 4820-6756 区间逐项补扫 | 未列全区间实为：aggregate_floor_for_attempts(4820)、attempt_aware_aggregate_timeout(4844)、parse_assigned_dimension(4875)、dispatch_wave_inner_with_release(4921-5726，单 fn ~806 行)、compute_slot_batch_fingerprint(5727)、ClassifiedReason/ClassifiedSlot(5761/5767)、classify_slot_result(5775)、classify_slot_attempt(5879)、reported_failure_detail(5925)、take_results(5936)、merge_round_into(5971)、outcome_for_completion(5987)、finalize_timeout(5995)、finalize_global_exceeded(6020)、inject_synthetic_failures(6033)、wait_for_progress_reporter(6080)、record_loop_max_runtime_envelope(6122)、record_wave_timeout_envelope(6185)、record_wave_spawn_failed_envelope(6242)、handle_wave_rejection(6293-6756) | U8 分组表闭合，消除「未列项」盲区 | 高 |
| E3c | policy_check.rs 729-1245 与 emit.rs 793-2210 逐项大纲 | policy_check：report_from_validation(766)、run_policy_check_unified(843 带 `#[cfg(test)]`)、run_policy_check_unified_with_config(874)、check_cli_flow_step_scope(1079)、recover_from_topics(1165)、recover_from_workspace_state(1179)、read_main_ledger_topics(1219)。emit：emit_command_with_root(838 带 `#[cfg(test)]`)、maybe_derive_triggered_for_isolated(856)、should_warn_on_missing_default_config(896)、emit_command_with_root_and_hats(903-2204，生产实现) | U2/U3 分组表与 cfg(test) 属性随行规则的依据 | 高 |
| E3d | runner.rs 609-652 原文 | 613/629/639/651 是 `run_loop_impl` 体内的 `#[cfg(!test)] let` / `#[cfg(test)] let` 条件语句；函数族：collect_idempotent_counts(68-244)、finalize_recovery_diagnosis(245-311)、finalize_session_pointer(312-347)、resolve_loop_id(362-392)、sentinel 三件套(393-528)、run_loop_impl(529-812)、resolve_supervisor_db_path(813-825) | U4 分组表与「条件语句随宿主 fn 搬移」规则的依据 | 高 |
| E13 | 三个大测试 mod 的 `fn test_` 前缀直方图（rg + sort + uniq） | event_policy::tests ≈5,229 行、dispatcher::tests ≈5,694 行、emit::tests ≈3,713 行；前缀族分布见 U2/U7/U8 分组表 | D7a 子分组拆分的机械依据；保证无「其他」大杂烩残留超 1,100 行 | 高 |

### 2.3 受影响范围（已经证据确认）

- **生产模块**：ralph-core 的 `event_loop`、`event_policy`；ralph-cli 的 `loop_runner::wave::dispatcher`、`loop_runner::runner`、`commands::emit`、`policy_check`、`task_cli`。
- **测试模块**：`loop_runner::tests::{wave_supervisor, legacy}` 及各文件内联 cfg(test) mod。
- **文档**：`.cursor/rules/state-management.mdc`（4 条行号引用，E7）。
- **不受影响（已确认）**：`crates/ralph-core/data/*.md`、`CLAUDE.md`/`AGENTS.md`、`presets/`、`presets/schemas/`、BDD scenarios、`.ralph/` 运行时状态、`scripts/ralph-zsh-plugin.zsh`（无文件名/行号耦合）。
- **调用方**：所有对 `ralph_core::event_policy::*`、`ralph_core::event_loop::*` 公开项的跨 crate 引用（ralph-cli/tui/api/e2e/bench）——由「re-export 保持路径」规则（D3）覆盖，编译器全量构建兜底。
- **构建目标**：workspace 全部 8 个包（nextest 全量覆盖）。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 目录结构模式 | A. 兄弟目录（`foo.rs` + `foo/`）；B. 传统 `foo/mod.rs` | **A. 兄弟目录** | E4（仓库大量先例）、E5（edition 2024） | B 需重命名原文件为 mod.rs，diff 噪音更大且违背仓库主流惯例 | 0.95 |
| D2 | 测试区拆分方式 | A. 整个 `#[cfg(test)] mod NAME {…}` 块原样搬入 `NAME.rs`，原位改路径式声明；B. 打散重组测试 | **A** | E3（各文件内联测试 mod 名单已确认） | B 触碰断言与测试体，违反零变更约束 | 0.95 |
| D3 | 生产项搬移后的路径保持 | A. 最小可见性放宽（`pub(super)`/`pub(crate)`）+ 必要时原位 `pub use` re-export；B. 全量 re-export；C. 调用方改 import | **A（以编译错误为机械判定：报错即修，二选一取最小）** | E4 先例 + Rust 模块语义 | B 产生不必要的公开面；C 大面积改动调用方，diff 失控且易漏 | 0.90 |
| D4 | 巨型单方法处理 | A. 原样整体搬移；B. 拆成 helper 函数 | **A** | E6（确认 process_parse_result/run_loop_impl_inner/validate_event_with_options 等为单方法） | B 是逻辑重组，违反「零逻辑改动」硬约束 | 0.98 |
| D5 | Unit 排序 | A. 先易后难（小文件→纯测试文件→大文件→event_loop 最后）；B. 按 crate 分组 | **A** | E1/E3（各文件结构复杂度已知） | 先在小文件上固化机制、暴露坑，再处理 17k 行核心文件，风险最低 | 0.90 |
| D6 | 每 Unit 的回归门禁 | A. 每 Unit 一次全量 `./scripts/run-tests.sh` + clippy + fmt-check；B. 仅 targeted | **A** | E2/E12（全量仅 78s，成本低；用户明确要求全量回归） | B 无法覆盖跨 crate 消费者与 Phase 2 串行隔离测试 | 0.98 |
| D7 | 测试 ID 不变量口径 | A. 全部文件测试 ID 逐字节不变；B. 纯测试文件（F3/F8）允许模块路径中段变化，测试名多重集+计数不变；其余文件 ID 逐字节不变 | **B** | E9（F3/F8 是扁平 fn 列表，拆子模块必然引入中间层） | A 对 F3/F8 物理不可能（扁平 fn 分组必改路径）；其余文件模块树不变，ID 可逐字节一致 | 0.92 |
| D7a | 单个测试 mod 外挂后仍 > 5,000 行怎么办 | A. 原样保留（制造新的 5k+ 文件）；B. 将该 `mod tests` 转为目录模块（`tests/mod.rs` + 按 fn 名前缀子分组），允许测试 ID 中段变化，测试名多重集与计数不变 | **B**（仅当外挂文件实测 > 5,000 行时触发；适用 event_policy::tests、dispatcher::tests；emit::tests ≈3,713 行不触发） | E13（前缀直方图证明可机械分组，最大残留杂项组 ~1,100 行） | A 违背本次重构的根本目的（消灭 5k+ 文件）；B 的代价仅是测试 ID 中段变化，由多重集核对兜底 | 0.88 |
| D8 | event_loop/mod.rs 的 impl 拆分粒度 | A. 按行号连续区间划 10 个区域，每区域一个子模块；B. 按语义精细归类 | **A** | E6（方法大纲按行连续） | B 需对 ~200 方法逐个做语义判断，引入执行期裁量且无行为收益 | 0.88 |
| D9 | 历史文档行号引用 | A. 不改（历史快照）；B. 全量重写 | **A** | E8 | B 篡改历史记录时点事实，且不在本次范围 | 0.95 |
| D10 | F3/F8 测试分组依据 | A. 按测试 fn 名前缀机械分组（最长前缀匹配，兜底 misc）；B. 按语义人工分组 | **A** | E3（F3/F8 测试 fn 名有强前缀规律，如 `build_supervisor_bridge_*`、`test_resolve_loop_id_*`） | B 引入执行期裁量；前缀规则可机械执行、可复核 | 0.86 |

全部决策 ≥ 0.85，无 BLOCKED 项。

---

## 4. BDD 行为规格

> 纯重构场景：行为规格描述的是**重构不变量**本身。正常流程 = 拆分后全绿；非法/边界 = 对不变量的任何违反都必须使 Unit 判败。

```gherkin
Feature: 大文件模块拆分保持行为完全不变

  Background:
    Given 仓库处于基线提交 552a5d41 之后的干净工作树
    And 全量测试基线为 Phase1=7514 pass、Phase2=23 pass、Doctest=19 pass + 4 ignored
    And cargo nextest 版本为 0.9.140

  Scenario: 任一拆分单元完成后全量回归通过
    Given 某个 Unit 只做了项级搬移、机械可见性调整与导入修复
    When 运行 ./scripts/run-tests.sh
    Then Phase 1 报告 7514 pass、0 fail
    And Phase 2 报告 23 pass、0 fail
    And Doctest 报告 19 pass、4 ignored
    And 退出码为 0

  Scenario: 测试清单不丢不重
    Given 拆分前记录了 cargo nextest list 的测试 ID 集合（或名多重集，见 D7）
    When Unit 完成后再次列出
    Then 非 F3/F8 单元：测试 ID 集合逐字节一致
    And F3/F8 单元：测试名多重集一致且总数不变

  Scenario: 公开 API 路径保持
    Given 某 pub 项原路径为 crate::module::Item
    When 该项被搬入子模块
    Then 原位存在 pub use re-export 使 crate::module::Item 仍可解析
    And 全 workspace cargo build 无 unresolved import 错误

  Scenario: 静态门禁不回退
    Given Unit 的搬移已完成
    When 运行 just lint 与 just fmt-check
    Then clippy 零告警（-D warnings）
    And fmt-check 零 diff

  Scenario: 禁止逻辑改动的边界
    Given 任何函数体内部
    When 审查该 Unit 的 diff
    Then 函数体逐字节不变（仅允许整个项在文件间移动）
    And 没有条件、顺序、字面量、签名的任何修改

  Scenario: 发现计划外耦合时停止
    Given 搬移引发的编译错误无法用 D3 的两种机械手段修复
    When 执行者遇到该情况
    Then 停止当前 Unit
    And 记录新证据并回到 Planner 重新决策，不得临场发明新方案
```

---

## 5. 验收与测试策略

**核心策略（适用于全部 Unit）**：本次为零行为变更重构，既有 7,560 个测试（7514+23+23 doctest）本身构成 Characterization + Differential 防线（E2）。不新增业务测试；验收 = 全量回归 + 不变量核对。

| 验收项 | 验收条件 | 测试入口 | 层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| AC-1 全量回归 | Phase1 7514 pass/0 fail、Phase2 23 pass/0 fail、doctest 19 pass/4 ignored、exit 0 | `./scripts/run-tests.sh` | 全量（含 BDD scenarios、smoke、集成） | 既有套件即 Characterization；新旧代码对同一输入行为一致由「函数体逐字节不变」+ 全绿共同保证（Differential 语义） | 由 `ralph-e2e --mock` 已含在 run-tests.sh 覆盖范围外 → 注：run-tests.sh 不含 e2e crate（`--exclude ralph-e2e` 语义见脚本），E2E 单独命令见 §9 |
| AC-2 测试清单不变 | D7 口径核对 | `cargo nextest list --workspace`（Unit 前后各一次，diff 比对） | 清单级 | — | 否 |
| AC-3 公开 API 保持 | 全 workspace 构建无 unresolved import；pub 路径可解析 | `cargo build --workspace` | 编译级 | — | 否 |
| AC-4 静态门禁 | clippy 零告警、fmt 零 diff | `just lint`、`just fmt-check` | 静态 | — | 否 |
| AC-5 diff 纯度 | `git diff --stat` 只含目标文件及本计划列出的新模块文件；函数体逐字节不变（抽查 + reviewer 检查） | `git diff` 人工/工具审查 | 审查级 | — | 否 |

选择理由：全量 nextest 只需 ~78s（E2），每 Unit 全量回归成本可忽略，且是唯一覆盖 Phase 2 竞态隔离测试与跨 crate 消费者的入口（E12）。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 回归范围 | Evidence |
|---|---|---|---|---|---|
| R1 | F9 task_cli.rs 拆分（< 5000 行→多模块） | §4 全部 Scenario | AC-1..AC-5（U1） | ralph-cli 全包 + workspace 全量 | E1,E3 |
| R2 | F6 policy_check.rs 拆分 | 同上 | AC-1..AC-5（U2） | ralph-cli + state-management.mdc 行号同步 | E1,E3,E7 |
| R3 | F5 commands/emit.rs 拆分 | 同上 | AC-1..AC-5（U3） | ralph-cli 全包 | E1,E3 |
| R4 | F7 loop_runner/runner.rs 拆分 | 同上 | AC-1..AC-5（U4） | ralph-cli 全包（run_loop 主链路） | E1,E3 |
| R5 | F8 tests/legacy.rs 拆分 | §4（D7 口径） | AC-1/AC-2/AC-4/AC-5（U5） | loop_runner 测试组 | E1,E3,E9 |
| R6 | F3 tests/wave_supervisor.rs 拆分 | §4（D7 口径） | AC-1/AC-2/AC-4/AC-5（U6） | loop_runner 测试组 | E1,E3,E9 |
| R7 | F4 event_policy.rs 拆分 | §4 全部 Scenario | AC-1..AC-5（U7） | workspace 全量（event_policy 被 core/cli 双消费）+ state-management.mdc | E1,E3,E7 |
| R8 | F2 wave/dispatcher.rs 拆分 | 同上 | AC-1..AC-5（U8） | workspace 全量（wave/supervisor 链路） | E1,E3 |
| R9 | F1 event_loop/mod.rs 拆分（测试区+头尾非 impl 区） | 同上 | AC-1..AC-5（U9/U10） | workspace 全量（核心中的核心） | E1,E3,E6 |
| R10 | F1 impl EventLoop 块按区域拆分 | 同上 | AC-1..AC-5（U11） | workspace 全量 | E6 |
| R11 | `.cursor/rules/state-management.mdc` 行号引用同步 | §4 Scenario「静态门禁」外延 | sed 复核 + 人工比对（U2/U7/U9 内） | 文档级 | E7 |

---

## 7. 严格串行开发单元

**全局机械规则（所有 Unit 共用，Unit 内不再重复）**：

- **搬移规则 M1**：以「整个项」为最小搬移单位（fn / async fn / struct / enum / impl 块 / const / static / type / 完整 `#[cfg(test)] mod NAME {…}` 块）。项内部逐字节不变。**项上的属性（`#[cfg(test)]` / `#[cfg(!test)]` / `#[test]` / `#[tokio::test]` / derive 等）必须随项整体搬移**——包括挂在单个 fn 上的 `#[cfg(test)]`（E3a/E3c）和函数体内的条件 let 语句（E3d：随宿主 fn 整体搬移，不得单独触碰）。
- **搬移规则 M2**：新子模块文件头部允许且仅允许添加 `use super::*;` 或精确 `use` 导入（由编译错误驱动，取能通过的最简形式）。
- **搬移规则 M3**：原位保留路径式声明 `mod NAME;`（生产）或 `#[cfg(test)] mod NAME;`（测试）；被搬项若被模块树外引用，原位补 `pub use NAME::Item;` 或 `pub(crate) use NAME::Item;`（与原可见性一致）。
- **搬移规则 M4**：私有项被跨模块引用时，只允许加最小可见性修饰（优先 `pub(super)`，不够再 `pub(crate)`）；**禁止**把非 pub 项升为 `pub`。
- **验证协议 V1（Unit 内快环）**：每完成一个搬移批次 → `cargo build --workspace` + 该文件所属 crate 的 targeted nextest 子集。
- **验证协议 V2（Unit 末全量门禁）**：`just lint` → `just fmt-check` → `./scripts/run-tests.sh` → AC-2 清单核对 → AC-5 diff 纯度审查。全部通过才可提交并进入下一 Unit。
- **停止条件 S1**（任一触发即停）：编译错误无法用 M2-M4 机械修复；diff 中出现函数体内部改动；回归计数与 E2 不符且无法归因于已知 skip；发现计划外跨 crate 耦合；需要新增依赖。**禁止**临场发明方案——记录证据，回 Planner。

---

### Unit 1：task_cli.rs 拆分（F9，5,042 行）

1. **Unit 目标**：将 `crates/ralph-cli/src/task_cli.rs` 的生产区与 4 个内联测试 mod 拆为兄弟目录模块，文件本体保留为模块根。
2. **对应需求与 Scenario**：R1；§4 全 Scenario；D1/D2/D3；E1/E3。
3. **外部可观察结果**：无行为变化；`task_cli.rs` 行数降至 ~1,200 以内，新增 `task_cli/` 子模块文件。
4. **当前行为基线**：E2 全量绿；E3 确认结构：生产区 1-564（CoordinatorHatsError、load_coordinator_hats(_from_path)、OutputFormat、TaskArgs/TaskCommands 及 AddArgs…VerifyArgs 等 clap 结构群）；测试 mod：`tests`（3091）、`load_coordinator_hats_tests`（4512）、`ensure_for_fix_unit_clap_tests`（4639）、`task_verify_gate_wiring_tests`（4818）；565/1197/1425/2086/2109 处为零散 `#[cfg(test)]` helper/use。
5. **输入与输出**：输入=现有文件；输出=拆分后模块树；错误=仅编译期；副作用=无运行时副作用；不变量=§1 全部。
6. **修改位置**（生产区实际为 1-3090，E3a；分组按行区间机械归属，项上的 cfg(test) 属性随项搬移）：
   - `crates/ralph-cli/src/task_cli.rs`：保留为模块根，只留 mod 声明 + `pub use` re-export + `execute` 分发入口。
   - 新建文件与内容（全部为 `crates/ralph-cli/src/task_cli/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `args.rs` | [1,564] CoordinatorHatsError、load_coordinator_hats(_from_path)、OutputFormat、TaskArgs、TaskCommands、AddArgs/EnsureArgs/ListArgs/ReadyArgs/StartArgs/CloseArgs/ConfirmArgs/FailArgs/ReopenArgs/ShowArgs/VerifyArgs、VerifyCommands 等全部 clap 结构 | ~560 |
| `validation.rs` | [565,1022] read_current_loop_id(cfg(test)随项)、operation_context_for、validate_task_id、authorize_lifecycle、confirmation_scope_conflict、enforce_command_policy、add_common_task_fields、validate_owner_hat_id、status_matches_filter、filter_tasks_for_list、filter_tasks_for_ready | ~460 |
| `cmd_add_ensure.rs` | [1023,1641] load_config_or_default、execute_add、add_task_with_args(cfg(test))、add_task_with_confirmation、execute_ensure、ensure_task_with_args(cfg(test))、ensure_task_with_confirmation、print_added_task、print_ensured_task（`execute` 本体留根） | ~620 |
| `cmd_list_close.rs` | [1642,2162] execute_list、execute_ready、execute_start、start_task_with_context、execute_close、close_task_with_context、close_task_with_context_and_config、emit_close_completion_warning、build_close_warning_payload(cfg(test))、build_close_warning_payload_missing_marker(cfg(test))、TAIL_SCAN_LINES、parse_topics_from_jsonl_tail | ~520 |
| `cmd_fail_verify.rs` | [2163,3090] execute_fail、fail_task_with_context、execute_show、execute_confirm、print_confirmed_task、execute_reopen、reopen_task_with_context、execute_verify、verify_add、verify_ensure、verify_lifecycle、gate_outcome、emit_bridge_deny、execute_verify_emit_bridge | ~930 |
| `tests.rs` | `mod tests`（3091-4511）整体 | ~1,420 |
| `load_coordinator_hats_tests.rs` | 同名 mod（4512-4637）整体 | ~130 |
| `ensure_for_fix_unit_clap_tests.rs` | 同名 mod（4638-4816）整体 | ~180 |
| `task_verify_gate_wiring_tests.rs` | 同名 mod（4817-5042）整体 | ~225 |

   **行数验收**：拆分后根文件 ≤ 250 行；新文件最大 ~1,420 行；全部 < 5,000 行。**不修改**：任何 fn 体、clap 属性字面量、错误消息字符串。
7. **可依赖能力**：既有全部代码；E4 兄弟目录先例。
8. **禁止依赖的未来能力**：不得触碰其余 8 个文件；不得预先建立任何共享测试工具。
9. **验收测试**：AC-1（`./scripts/run-tests.sh`）、AC-2（`cargo nextest list` 前后 diff = 空；本 Unit 测试 ID 必须逐字节不变）、AC-3/AC-4/AC-5。
10. **Acceptance Red**：重构类 Unit 无传统 Red。等价判据：搬移中途任何时刻若 `cargo build` 失败或 nextest 子集失败，即为「红」，必须当场修复至绿再继续；最终门禁 V2 任一项失败即 Unit 不成立。**无效红**：因外层 hat env 污染（HARD RULE 5）导致的失败不算——run-tests.sh 已内置 scrub，targeted 子集需自行确认无 `RALPH_CURRENT_HAT` 等残留。
11. **单元测试拆分**：不新增测试（零行为变更）。既有 4 个测试 mod 原样通过即证明。
12. **Red → Green → Refactor 顺序**：`nextest list 快照` → 搬移测试 mod（每批 build + targeted `cargo nextest run -p ralph-cli -- task_cli`）→ 全绿 → 必要时搬移生产结构群（同法）→ V2 全量门禁 → `nextest list` diff 核对 → close。
13. **最小实现范围**：仅 M1-M4 允许的改动；不实现任何新能力；不重命名。
14. **集成验证**：`cargo nextest run -p ralph-cli -- task_cli`（targeted）+ `./scripts/run-tests.sh`（全量，真实模块，无 Fake）。
15. **风险驱动测试**：既有套件即 Characterization/Differential（风险依据：拆模块可能漏搬 cfg(test) helper 导致编译失败或测试失踪——由 AC-2 清单核对捕获）。
16. **回归范围**：ralph-cli 全部单测与集成测（`integration_tasks` 等直接消费 task CLI）；workspace 全量（task_cli 被 ralph 主二进制引用）；`cargo build --workspace`；lint；fmt。理由：task_cli 是 CLI 一级命令面。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/task_cli.rs` | 修改（删除搬出项，保留 execute 分发 + mod 声明 + re-export；5,042 → ≤250 行） | 模块根化 | E1,E3,E3a |
| `crates/ralph-cli/src/task_cli/{args,validation,cmd_add_ensure,cmd_list_close,cmd_fail_verify,tests,load_coordinator_hats_tests,ensure_for_fix_unit_clap_tests,task_verify_gate_wiring_tests}.rs` | 新增 9 个文件（行数见 §6 表） | 承接搬出项 | E3a,E4 |

18. **完成标准**：V2 全绿 + AC-2 diff 为空 + AC-5 diff 纯度通过 + 根 ≤250 行、新文件全部 <5,000 行 + 可独立提交。
19. **停止条件**：S1 全集。
20. **风险与注意事项**：风险=零散 cfg(test) helper 归属判错导致重复定义；触发=编译报 duplicate definition；检测=`cargo build`；缓解=helper 随唯一使用者搬移，编译报错驱动；剩余风险=无（编译器兜底）。

---

### Unit 2：policy_check.rs 拆分（F6，5,586 行）

1. **Unit 目标**：拆分 `crates/ralph-cli/src/policy_check.rs`：测试区（5 个命名 mod + 843 处 helper）外挂；生产区（PolicyCheckMode/Flags/resolve*/load_workspace_config/build_policy_state/check_* 门）按项成组外挂。
2. **对应需求与 Scenario**：R2、R11（含 state-management.mdc 同步）；D1-D3；E1/E3/E7。
3. **外部可观察结果**：无行为变化；policy_check.rs 降至 ~1,500 行以内。
4. **当前行为基线**：E2/E3：生产区 1-842（PolicyCheckMode 43、PolicyCheckFlags 53、resolve_policy_check_mode(_with_ctx) 68/89、legacy_resolve 128、OnConfigError 142、load_workspace_config 161、load_policy_config_for_cli_emit 219、load_policy_config_from_hats_only 322、merge_workspace_hats_into 375、enabled_event_policy 413、PolicyCheckContext 421、build_policy_state 431、check_step_handoff_gate 457、mismatch_to_validation_error 509、check_wave_dimension_assignment(_with_env) 539/554、check_isolated_scope 618、PolicyCheckReport 686+729 impl）；测试 mod：`read_main_ledger_topics_tests`（1246）、`u6_unified_path_tests`（1348）、`u1_warn_parity_tests`（2459）、`u2_structured_feedback_tests`（2612）、`tests`（3308）。
5. **输入与输出**：同 U1 模式。
6. **修改位置**（E3/E3c；`#[cfg(test)]` 挂在 `run_policy_check_unified` 项上，必须原样随行）：
   - `crates/ralph-cli/src/policy_check.rs`：保留类型与解析器族（[1,430]：PolicyCheckMode、PolicyCheckFlags、resolve_policy_check_mode(_with_ctx)、legacy_resolve、OnConfigError、load_workspace_config、load_policy_config_for_cli_emit、load_policy_config_from_hats_only、merge_workspace_hats_into、enabled_event_policy、PolicyCheckContext）为根，加 mod 声明与 re-export。
   - 新建文件与内容（全部为 `crates/ralph-cli/src/policy_check/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `gates.rs` | [431,842] build_policy_state、check_step_handoff_gate、mismatch_to_validation_error、check_wave_dimension_assignment(_with_env)、check_isolated_scope、PolicyCheckReport、impl PolicyCheckReport、report_from_validation | ~410 |
| `unified.rs` | [843,1245] run_policy_check_unified（**`#[cfg(test)]` 随项**）、run_policy_check_unified_with_config、check_cli_flow_step_scope、recover_from_topics、recover_from_workspace_state、read_main_ledger_topics | ~400 |
| `read_main_ledger_topics_tests.rs` | 同名 mod（1246-1347）整体 | ~100 |
| `u6_unified_path_tests.rs` | 同名 mod（1348-2458）整体 | ~1,110 |
| `u1_warn_parity_tests.rs` | 同名 mod（2459-2611）整体 | ~150 |
| `u2_structured_feedback_tests.rs` | 同名 mod（2612-3307）整体 | ~695 |
| `tests.rs` | `mod tests`（3308-5586）整体（~2,278 < 5,000，D7a 不触发） | ~2,280 |

   **行数验收**：根 ≤ 450 行；新文件最大 ~2,280 行；全部 < 5,000 行。**不修改**任何检查逻辑。
   - `.cursor/rules/state-management.mdc` 行 27（`policy_check.rs:1331-1390`）：该范围落在 u6_unified_path_tests 内，拆分后用 `rg -n` 定位 envelope-layer 校验代码实际新位置（unified.rs 或 u6_unified_path_tests.rs）并更新引用（E7）。
7. **可依赖能力**：U1 已验证的 M1-M4/V1/V2 流程。
8. **禁止依赖的未来能力**：不动 event_policy.rs（U7 范围）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-cli -- policy_check`。
10. **Acceptance Red**：同 U1（编译/子集失败即红，当场修复）。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 搬测试 mod（build + targeted）→ 搬生产族（build + targeted）→ V2 → mdc 行号同步与复核 → close。
13. **最小实现范围**：M1-M4；mdc 引用更新仅改行号/路径，不改文字语义。
14. **集成验证**：`cargo nextest run -p ralph-cli -- policy`（含 policy_check 与相关 emit 预检测试）+ 全量。
15. **风险驱动测试**：既有套件（风险依据：policy_check 是 emit 预检同源路径，回归面广，全量门禁覆盖）。
16. **回归范围**：ralph-cli 全包；workspace 全量（ralph-core 的 event_policy 测试与 policy_check 有语义对称测试，全量兜底）；lint/fmt/build。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/policy_check.rs` | 修改（5,586 → ≤450 行） | 模块根化 | E1,E3,E3c |
| `crates/ralph-cli/src/policy_check/{gates,unified,read_main_ledger_topics_tests,u6_unified_path_tests,u1_warn_parity_tests,u2_structured_feedback_tests,tests}.rs` | 新增 7 个文件（行数见 §6 表） | 承接搬出项 | E3c,E4 |
| `.cursor/rules/state-management.mdc` | 修改（仅行 27 引用） | 行号漂移同步 | E7 |

18. **完成标准**：V2 全绿 + AC-2 diff 空 + 根 ≤450 行、新文件全部 <5,000 行 + mdc 引用复核通过 + 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=u1/u2/u6 系列测试名含历史 plan 编号，搬移时 mod 名必须逐字保留（测试 ID 依赖它）；检测=AC-2 diff；剩余风险=无。

---

### Unit 3：commands/emit.rs 拆分（F5，6,635 行）

1. **Unit 目标**：拆分 `crates/ralph-cli/src/commands/emit.rs`：6 个命名测试 mod 外挂；`pub mod schema_view` 保留或原样搬入子目录（路径不变）。
2. **对应需求与 Scenario**：R3；D1-D3；E1/E3。
3. **外部可观察结果**：无行为变化；emit.rs 根文件降至 ~1,500 行以内。
4. **当前行为基线**：E3：生产区 1-792（EmitArgs 31、format_fix_hint 103、resolve_provenance 148、record_cli_emit_rejection 173、should_policy_check_emit(_with_ctx) 207/218、looks_like_json 245、canonical_payload_for_token 256、compute_policy_check_token 274、U5GateState/U5Gate 324/341/345、write_cli_emit_recovery_envelope 629、emit_command 702、is_default_file_arg 723、paths_canonical_differ 738、bail_cwd_workspace_drift 747、print/format_emit_reject_summary 771/779）；测试 mod：`emit_reject_summary_tests`（794）、838 处 cfg(test) helper 所在 mod、`tests`（2206）、`emit_schema_emit_result_tests`（5921）、`emit_policy_check_reject_json_tests`（6010）、`emit_policy_check_accept_json_tests`（6119）、`emit_apply_recorded_json_tests`（6308）；`pub mod schema_view`（6518，生产 pub 子模块，体积以实测为准）。
5. **输入与输出**：同前。
6. **修改位置**（E3/E3c；生产区为 1-792 + 856-2204 两段）：
   - `crates/ralph-cli/src/commands/emit.rs`：保留 [1,792] 段（EmitArgs、format_fix_hint、resolve_provenance、record_cli_emit_rejection、should_policy_check_emit(_with_ctx)、looks_like_json、canonical_payload_for_token、compute_policy_check_token、U5GateState/U5Gate、write_cli_emit_recovery_envelope、emit_command、is_default_file_arg、paths_canonical_differ、bail_cwd_workspace_drift、print/format_emit_reject_summary）+ mod 声明 + re-export 为根。
   - 新建文件与内容（全部为 `crates/ralph-cli/src/commands/emit/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `command_impl.rs` | [856,2204] maybe_derive_triggered_for_isolated、should_warn_on_missing_default_config、emit_command_with_root_and_hats | ~1,350 |
| `test_support.rs` | emit_command_with_root（838-855，**`#[cfg(test)]` 随项**） | ~20 |
| `emit_reject_summary_tests.rs` | 同名 mod（794-837）整体 | ~45 |
| `tests.rs` | `mod tests`（2206-5919）整体（~3,713 < 5,000，D7a 不触发） | ~3,715 |
| `emit_schema_emit_result_tests.rs` | 同名 mod（5921-6008）整体 | ~90 |
| `emit_policy_check_reject_json_tests.rs` | 同名 mod（6010-6117）整体 | ~110 |
| `emit_policy_check_accept_json_tests.rs` | 同名 mod（6119-6306）整体 | ~190 |
| `emit_apply_recorded_json_tests.rs` | 同名 mod（6308-6517）整体 | ~210 |
| `schema_view.rs` | `pub mod schema_view` 体内内容（6518-6635）；原位改 `pub mod schema_view;` 路径式声明，外部路径 `commands::emit::schema_view::*` 不变 | ~120 |

   **行数验收**：根 ≤ 850 行；新文件最大 ~3,715 行；全部 < 5,000 行。**不修改** emit 命令任何逻辑与输出格式。
7. **可依赖能力**：U1-U2 流程。
8. **禁止依赖的未来能力**：不动 policy_check.rs（即使 emit 调用它——跨文件引用保持原 import）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-cli -- emit`。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 逐测试 mod 搬移（build + targeted）→ 视体积搬 schema_view → V2 → close。
13. **最小实现范围**：M1-M4。
14. **集成验证**：targeted emit 子集 + 全量（`integration_*` 覆盖 CLI emit 真实路径）。
15. **风险驱动测试**：既有套件（风险依据：emit 是 hat 事件写入唯一 CLI 面，OPAC/单事件预算语义敏感——但本次不碰逻辑，全量兜底）。
16. **回归范围**：ralph-cli 全包 + workspace 全量 + lint/fmt/build。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/commands/emit.rs` | 修改（6,635 → ≤850 行） | 模块根化 | E1,E3,E3c |
| `crates/ralph-cli/src/commands/emit/{command_impl,test_support,emit_reject_summary_tests,tests,emit_schema_emit_result_tests,emit_policy_check_reject_json_tests,emit_policy_check_accept_json_tests,emit_apply_recorded_json_tests,schema_view}.rs` | 新增 9 个文件（行数见 §6 表） | 承接搬出项 | E3c,E4 |

18. **完成标准**：V2 全绿 + AC-2 diff 空 + 根 ≤850 行、新文件全部 <5,000 行 + 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=838 处 cfg(test) helper 归属；检测=编译；缓解=M4 机械修复；剩余=无。

---

### Unit 4：loop_runner/runner.rs 拆分（F7，5,457 行）

1. **Unit 目标**：拆分 `crates/ralph-cli/src/loop_runner/runner.rs`：将巨型 `run_loop_impl_inner`（~4,300 行单 async fn）连同 `run_loop_impl` 与周边 helper 原样搬入子模块文件；2 个测试 mod 外挂。
2. **对应需求与 Scenario**：R4；D1-D4；E1/E3。
3. **外部可观察结果**：无行为变化；runner.rs 根文件 < 1,200 行。
4. **当前行为基线**：E3：顶层项 agent_wrote_any_valid_or_rejected 27、collect_idempotent_counts 68、finalize_recovery_diagnosis 245、finalize_session_pointer 312、RpcSharedState 348、resolve_loop_id 362、sentinel 三件套 393/402/411、run_loop_impl 529、resolve_supervisor_db_path 813、run_loop_impl_inner 826-~5130（**单个 async fn，禁止拆体**）、SyncRunError 5131、run_sync_with_timeout 5146、write_startup_timeout_envelope 5222、测试 mod sync_timeout_tests 5264、u1_preset_name_aware_lint_gate_wiring 5328；613-651 处为 run_loop_impl 函数体内的 `#[cfg(test)]` 片段（**随所在 fn 整体搬移，不得单独触碰**）。
5. **输入与输出**：同前。
6. **修改位置**（E3/E3d；613-651 的 `cfg(!test)`/`cfg(test)` 条件 let 语句在 run_loop_impl 函数体内，随宿主 fn 整体搬移，不得单独触碰）：
   - `crates/ralph-cli/src/loop_runner/runner.rs`：保留 agent_wrote_any_valid_or_rejected(27-67)、RpcSharedState(348-361)、resolve_loop_id(362-392) 与 mod 声明 + re-export 为根。
   - 新建文件与内容（全部为 `crates/ralph-cli/src/loop_runner/runner/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `entry.rs` | [68,528] collect_idempotent_counts、finalize_recovery_diagnosis、finalize_session_pointer、loop_termination_sentinel_path、remove_loop_termination_sentinel、write_loop_termination_sentinel | ~460 |
| `run_impl.rs` | [529,825] run_loop_impl（**含体内 613-651 条件 let，整体搬移**）、resolve_supervisor_db_path | ~300 |
| `inner.rs` | [826,5130] run_loop_impl_inner（**单个 async fn ~4,300 行，禁拆体，整体搬移**） | ~4,305 |
| `sync_timeout.rs` | [5131,5262] SyncRunError、run_sync_with_timeout、write_startup_timeout_envelope | ~130 |
| `sync_timeout_tests.rs` | sync_timeout_tests（5264-5326）+ u1_preset_name_aware_lint_gate_wiring（5328-5457）两个 cfg(test) mod 整体 | ~195 |

   **行数验收**：根 ≤ 150 行；最大单文件 inner.rs ≈ 4,305 行（其中 ~4,300 行为单个不可拆方法，D4）；全部 < 5,000 行。**不修改**：run_loop_impl_inner 内部任何语句。
7. **可依赖能力**：U1-U3 流程；loop_runner 目录既有兄弟模块模式（E4）。
8. **禁止依赖的未来能力**：不动 tests/ 目录（U5/U6）、不动 wave/（U8）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-cli -- runner` 与 `-- run_loop`。
10. **Acceptance Red**：同 U1；特别注意：run_loop_impl_inner 搬移后若出现生命周期/借用错误，说明搬移时误改了签名或 use——回滚该批重来，不得「顺手修逻辑」。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 搬 sync_timeout 族（build + targeted）→ 搬 run_impl 族（build + targeted）→ V2 → close。
13. **最小实现范围**：M1-M4；run_loop_impl_inner 仅允许「整体出现在新文件」。
14. **集成验证**：targeted runner 子集 + 全量（loop_runner 是 ralph run 主链路，Phase 2 的 3 个 partial_timeout 测试在此链路）。
15. **风险驱动测试**：既有套件；风险依据：该文件含时序敏感测试（HARD RULE 两阶段隔离的 3 个 partial_timeout 测试归属 ralph-cli），必须走 run-tests.sh 全量而非手动 nextest。
16. **回归范围**：ralph-cli 全包 + workspace 全量（run_loop_impl 被 resume/daemon 路径引用）+ lint/fmt/build。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/runner.rs` | 修改（5,457 → ≤150 行） | 模块根化 | E1,E3,E3d |
| `crates/ralph-cli/src/loop_runner/runner/{entry,run_impl,inner,sync_timeout,sync_timeout_tests}.rs` | 新增 5 个文件（行数见 §6 表） | 承接搬出项 | E3d,E4 |

18. **完成标准**：V2 全绿（含 Phase 2 三测试）+ AC-2 diff 空 + 根 ≤150 行、全部新文件 <5,000 行 + 独立提交。
19. **停止条件**：S1；另：若 run_loop_impl_inner 搬移触发非 import 类编译错误，立即停止（说明对其依赖结构判断有误）。
20. **风险与注意事项**：风险=巨型 fn 搬移时编辑器/工具意外改写空白或换行；检测=`git diff` 函数体逐字节抽查（对搬移前后做 `diff <(sed -n …)` 级核对）；缓解=搬移用整段剪切粘贴而非重写；剩余风险=低（fmt-check + 全量测试兜底）。

---

### Unit 5：tests/legacy.rs 拆分（F8，5,316 行，纯测试）

1. **Unit 目标**：将 `crates/ralph-cli/src/loop_runner/tests/legacy.rs` 的 109 个扁平测试 fn 按 fn 名前缀机械分组搬入子模块文件。
2. **对应需求与 Scenario**：R5；D1/D2/D7/D10；E1/E3/E9。
3. **外部可观察结果**：无行为变化；legacy.rs 变为薄声明文件（< 300 行）。
4. **当前行为基线**：E3/E9：文件由 `tests/mod.rs:23 mod legacy;` 声明；含 109 个 `#[test]` fn（如 test_resolve_loop_id_*、test_pty_*、test_prepare_tui_*、test_fail_if_blocking_*、test_wait_for_resume_*）与少量 helper。
5. **输入与输出**：同前。
6. **修改位置**：
   - `crates/ralph-cli/src/loop_runner/tests/legacy.rs`：改为薄声明文件（`mod <group>;` 声明 + 跨组共享 helper，helper 归属由编译错误驱动：仅单组使用则随组搬，多组使用则留根并最小可见性放宽）。
   - 新增 `crates/ralph-cli/src/loop_runner/tests/legacy/<group>.rs`。
   - **分组表（D10，已按实际 fn 清单 E13 预计算，执行时以 `rg -n '^fn ' legacy.rs` 复核，允许 ±2 fn 修正但不得改变组集合）**：

| 新文件 | 前缀规则 | fn 数 | 预期行数 |
|---|---|---|---|
| `loop_id.rs` | `test_resolve_loop_id_*` | 4 | ~70 |
| `interactive.rs` | `test_pty_*` / `test_user_interactive_*` / `test_prepare_tui_*` | 4 | ~60 |
| `termination_resume.rs` | `test_fail_if_blocking_*` / `test_wait_for_resume_*` / `test_suspend_*` | ~7 | ~250 |
| `events_ledger.rs` | `test_events_*` / `test_ledger_*` / `test_partial_timeout_*` | ~12 | ~700 |
| `tasks_lifecycle.rs` | `test_task_*` / `test_close_*` / `test_complete_*` | ~15 | ~800 |
| `hats_prompt.rs` | `test_hat_*` / `test_prompt_*` / `test_coordinator_*` | ~12 | ~700 |
| `misc.rs` | 不匹配上述任何前缀的剩余 fn | ~55 | ~3,400 |

   注：`misc.rs` ~3,400 行 < 5,000，D7a 不触发；执行时若复核发现 misc 中 ≥3 个 fn 共享同一新前缀，应再建组（规则不变，组集合可扩展不可收缩）。**分组只影响文件归属，不影响任何 fn 内容与测试名。**
   **行数验收**：根 ≤ 100 行；最大单文件 misc.rs ≈ 3,400 行；全部 < 5,000 行。
7. **可依赖能力**：U1-U4 流程。
8. **禁止依赖的未来能力**：不动 wave_supervisor.rs（U6）。
9. **验收测试**：AC-1、AC-2（D7 口径：测试名多重集与计数不变；路径中段允许变化）、AC-4、AC-5。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增（本文件即测试）。
12. **Red → Green → Refactor 顺序**：`nextest list` 快照 → 产出分组表 → 按组搬移（每组 build + targeted `cargo nextest run -p ralph-cli --test '*' -- <组内fn子串>`）→ V2 → 清单多重集比对 → close。
13. **最小实现范围**：仅 fn 项搬移 + mod 声明 + 必要 `use super::*;`/`use crate::…` 导入修复。
14. **集成验证**：全量（这些测试本身覆盖 loop_runner 集成行为）。
15. **风险驱动测试**：无新增；风险依据：扁平 fn 搬移的唯一真实风险是漏搬/重名，AC-2 计数+多重集核对即检测手段。
16. **回归范围**：ralph-cli 测试组 + workspace 全量 + lint/fmt。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/tests/legacy.rs` | 修改（5,316 → ≤100 行薄声明） | 分组根 | E3,E9 |
| `crates/ralph-cli/src/loop_runner/tests/legacy/{loop_id,interactive,termination_resume,events_ledger,tasks_lifecycle,hats_prompt,misc}.rs` | 新增 7 个文件（行数见 §6 表） | 承接分组测试 | E4,E13 |

18. **完成标准**：V2 全绿 + AC-2 多重集一致 + 根 ≤100 行、新文件全部 <5,000 行 + 分组复核结果入提交说明 + 独立提交。
19. **停止条件**：S1；另：若复核后 misc 组 > 5,000 行，停止并回报（触发 D7a 二级分组，需 Planner 确认）。
20. **风险与注意事项**：风险=共享 helper 跨组引用；检测=编译；缓解=helper 留在根或最小可见性放宽（M4）；剩余=无。

---

### Unit 6：tests/wave_supervisor.rs 拆分（F3，9,423 行，纯测试）

1. **Unit 目标**：同 U5 机制，将 108 个测试 fn（含 SpyBindingBridge 等 helper struct）按前缀分组搬入子模块。
2. **对应需求与 Scenario**：R6；D1/D2/D7/D10；E1/E3/E9。
3. **外部可观察结果**：无行为变化；wave_supervisor.rs 薄化（< 400 行）。
4. **当前行为基线**：E3/E9：由 `tests/mod.rs:26` 声明；helper struct SpyBindingBridge（52）；测试 fn 前缀族：`enabled_*`/`bridge_*`（绑定行为）、`build_supervisor_bridge_*`（db 路径解析）、`slot_binding_env_*`、`review_kind_*`、`recover_*`（启动恢复/投影）、`supervisor_capability_gate_*`/`supervisor_disabled_*`（能力门）。
5. **输入与输出**：同 U5。
6. **修改位置**：
   - `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`：薄声明 + 跨组共享 helper（SpyBindingBridge 等；归属由编译错误驱动，多组使用留根）。
   - 新增 `crates/ralph-cli/src/loop_runner/tests/wave_supervisor/<group>.rs`。
   - **分组表（D10，已按实际 fn 清单 E13 预计算，执行时 `rg -n '^fn ' wave_supervisor.rs` 复核，允许 ±2 fn 修正）**：

| 新文件 | 前缀规则 | fn 数 | 预期行数 |
|---|---|---|---|
| `binding.rs` | `enabled_*` / `bridge_*` / `slot_binding_env_*` / `review_kind_*` | ~6 | ~450 |
| `db_path.rs` | `build_supervisor_bridge_*` | ~6 | ~350 |
| `recovery.rs` | `recover_*` | ~6 | ~700 |
| `capability_gate.rs` | `supervisor_*` | ~9 | ~600 |
| `projection.rs` | `projection_*` / `pending_projection_*` / `close_stale_*` | ~8 | ~800 |
| `dispatch_env.rs` | `dispatch_*` / `worker_env_*` / `spawn_*` | ~8 | ~800 |
| `misc.rs` | 不匹配上述前缀的剩余 fn | ~65 | ~5,700 ⚠️ |

   ⚠️ **misc 预估超 5,000 行**：本表 misc 为保守兜底预估（108 个 fn 中仅 43 个能确定前缀归属）。执行第一步必须先产出全量 fn 清单并对 misc 候选做二次前缀聚类（≥3 fn 共享前缀即建组，规则同 U5）；二次聚类后任何单组 > 5,000 行即触发 D7a 报告义务（实际概率低：9,423/108 ≈ 87 行/fn，misc 需 >57 fn 才会超）。**分组只影响文件归属，不影响任何 fn 内容与测试名。**
   **行数验收**：根 ≤ 300 行（含共享 helper）；新文件全部 < 5,000 行（若二次聚类后仍超限，停止回报，不得静默产出 5k+ 文件）。
7. **可依赖能力**：U5 已验证的分组流程。
8. **禁止依赖的未来能力**：不动 wave/ 生产代码（U8）。
9. **验收测试**：AC-1、AC-2（D7 口径）、AC-4、AC-5。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 分组表 → 按组搬移（build + targeted `-- supervisor`）→ V2 → 多重集比对 → close。
13. **最小实现范围**：同 U5。
14. **集成验证**：全量（supervisor 特性门测试依赖 feature 组合，run-tests.sh `--all-features` 语义由脚本保证——以脚本实际参数为准，不得手动改）。
15. **风险驱动测试**：无新增；SpyBindingBridge 是 spy/mock 类 helper，搬移后若 feature-gated 测试编译失败，多为 cfg 属性漏带——搬移必须连同 item 上的全部属性（`#[cfg]`/`#[test]`/`#[tokio::test]`）逐字节移动。
16. **回归范围**：ralph-cli 测试组 + workspace 全量 + lint/fmt。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 修改（9,423 → ≤300 行薄声明 + 共享 helper） | 分组根 | E3,E9 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor/{binding,db_path,recovery,capability_gate,projection,dispatch_env,misc}.rs` | 新增 7+ 个文件（二次聚类可能增加组数；行数见 §6 表） | 承接分组测试 | E4,E13 |

18. **完成标准**：V2 全绿 + AC-2 多重集一致 + 根 ≤300 行 + 新文件全部 <5,000 行 + 独立提交。
19. **停止条件**：S1 + U5 同款规则 + misc 二次聚类后仍 >5,000 行。
20. **风险与注意事项**：风险=helper struct 字段被多组测试构造，可见性需放宽；检测=编译；缓解=M4；剩余=无。

---

### Unit 7：event_policy.rs 拆分（F4，8,329 行）

1. **Unit 目标**：拆分 `crates/ralph-core/src/event_policy.rs`：单个 `mod tests`（~5,200 行）外挂；生产区按项族外挂为 5 个子模块，全部 pub 路径 re-export。
2. **对应需求与 Scenario**：R7、R11；D1-D3；E1/E3/E7。
3. **外部可观察结果**：无行为变化；event_policy.rs 根 < 1,200 行（仅保留类型定义与 re-export）。
4. **当前行为基线**：E3：类型族 ViolationType 22、DuplicateWorkDoneHint 125/150、PolicyFinding 237、PolicyDecision 245、PolicyRejection 271、ReasonClass 306/333、is_recoverable_policy_finding 350、PolicyRuntimeState 370、precheck_proposed_dedup_key 493、review_start_dedup_key 512、PolicyRuntimeState impl 531-984；completion 族 check_completion_honored 985、check_completion_guard 997、apply_completion_after_terminal_action 1027；handoff 族 check_handoff_envelope 1066、handoff_envelope_validation_enabled 1098、EventLoopHandoffConfig 1112/1116、HandoffEnvelopeConfigAccess trait 1384、DefaultHandoffConfig 1389/1391；topic 族 check_topic_format 1130、build_allowed_topics 1170、is_system_topic 1216、is_system_control_topic 1234、NULL_PAYLOAD_REJECT_TOPICS 1255、is_null_payload_rejected_topic 1268、matches_topic_rule 1274、check_topic_deny_rules 1294；validation 族 validate_event 1355、validate_event_with_hat 1369、validate_event_with_options 1404-~2492（**单 fn ~1,090 行，禁止拆体**）、type_name 2493、obj_get 2509、validate_element_shape 2519、CandidateEmitPreview 2612、PolicyReasonEntry 2627；`mod tests` 3100 起。
5. **输入与输出**：同前。
6. **修改位置**：
   - `crates/ralph-core/src/event_policy.rs`：保留公共类型/枚举/trait 定义（[22,530]：ViolationType、DuplicateWorkDoneHint、PolicyFinding、PolicyDecision、PolicyRejection、ReasonClass、is_recoverable_policy_finding、PolicyRuntimeState 结构体、precheck_proposed_dedup_key、review_start_dedup_key）于根 + mod 声明 + re-export。
   - 新建文件与内容（全部为 `crates/ralph-core/src/event_policy/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `runtime_state.rs` | [531,984] impl PolicyRuntimeState 块 | ~455 |
| `completion.rs` | [985,1065] check_completion_honored、check_completion_guard、apply_completion_after_terminal_action | ~80 |
| `handoff_envelope.rs` | [1066,1129] + [1384,1403] check_handoff_envelope、handoff_envelope_validation_enabled、EventLoopHandoffConfig、HandoffEnvelopeConfigAccess trait、DefaultHandoffConfig | ~85 |
| `topic_checks.rs` | [1130,1354] check_topic_format、build_allowed_topics、is_system_topic、is_system_control_topic、NULL_PAYLOAD_REJECT_TOPICS、is_null_payload_rejected_topic、matches_topic_rule、check_topic_deny_rules | ~225 |
| `validation.rs` | [1355,1403] + [1404,3099] validate_event、validate_event_with_hat、validate_event_with_options（**单 fn ~1,089 行，禁拆体**）、type_name、obj_get、validate_element_shape、CandidateEmitPreview、PolicyReasonEntry 及其余至 3099 的项 | ~1,745 |
| `tests/mod.rs` | `mod tests` 转为目录模块（3100 起，~5,229 行 > 5,000，**D7a 触发**）：mod.rs 只含 use + `mod <group>;` 声明 | ≤150 |
| `tests/deny_rules.rs` | mod tests 内 `*deny*` / `*topic_format*` / `*system_control*` 前缀测试 fn | ~900 |
| `tests/handoff.rs` | `*handoff*` / `*envelope*` 前缀 | ~800 |
| `tests/completion.rs` | `*completion*` / `*terminal*` 前缀 | ~800 |
| `tests/dedup.rs` | `*dedup*` / `*duplicate*` / `*work_done*` / `*work_ready*` 前缀 | ~750 |
| `tests/misc.rs` | 其余 fn（E13 直方图保证 ≤ ~1,100 行） | ~1,100 |

   **D7a 执行细则**：先把 `mod tests { … }` 整体搬为 `event_policy/tests/mod.rs`（此时 V1 必须全绿，作为中间检查点）；再在 tests/mod.rs 内把扁平测试 fn 按前缀表分入子模块。测试 ID 由 `event_policy::tests::x` 变为 `event_policy::tests::<group>::x`——AC-2 按名多重集+计数核对。
   **行数验收**：根 ≤ 550 行；最大单文件 validation.rs ≈ 1,745 行；tests/ 下最大 misc.rs ≈ 1,100 行；全部 < 5,000 行。
   - `.cursor/rules/state-management.mdc` 行 24-25（`event_policy.rs:174`、`420+573`）：拆分后 `rg -n` 重新定位 `duplicate_work_done` 代码（留在根的 ViolationType 区）与 `work.ready` dedup 计数（runtime_state.rs）实际位置并更新（E7）。
7. **可依赖能力**：U1-U6 流程。
8. **禁止依赖的未来能力**：不动 event_loop/（U9-U11）；不动 policy_check.rs（U2 已完成则仅保持其 import）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-core -- event_policy` 与 `-- policy`。
10. **Acceptance Red**：同 U1；特别：event_policy 被 ralph-cli/tui/api 消费，任何 `use ralph_core::event_policy::X` 报 unresolved 时，一律用原位 `pub use` 修复（D3），不得改调用方。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 搬 tests（build + targeted）→ 逐族搬生产项（每族 build + targeted）→ V2 → mdc 同步复核 → close。
13. **最小实现范围**：M1-M4 + re-export；`validate_event_with_options` 函数体逐字节不动。
14. **集成验证**：ralph-core targeted + workspace 全量（BDD scenarios 与 preset_lint 大量消费 event_policy 语义）。
15. **风险驱动测试**：既有套件；风险依据：event_policy 是 OPAC 门禁核心，跨 crate 消费面最大——re-export 表 + 全 workspace build 即契约测试等价物。
16. **回归范围**：workspace 全量（ralph-core 被全部下游包依赖）+ lint/fmt/build + preset_lint 相关测试（含在 ralph-core/ralph-cli 包内，全量覆盖）。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_policy.rs` | 修改（8,329 → ≤550 行） | 模块根化 + re-export | E1,E3 |
| `crates/ralph-core/src/event_policy/{runtime_state,completion,handoff_envelope,topic_checks,validation}.rs` | 新增 5 个生产子模块（行数见 §6 表） | 承接项族 | E3,E4 |
| `crates/ralph-core/src/event_policy/tests/{mod,deny_rules,handoff,completion,dedup,misc}.rs` | 新增 6 个测试子模块（D7a） | 承接 ~5,229 行测试区 | E13 |
| `.cursor/rules/state-management.mdc` | 修改（行 24-25 引用） | 行号漂移同步 | E7 |

18. **完成标准**：V2 全绿 + AC-2 名多重集一致（tests 子分组按 D7a 口径；生产路径 ID 逐字节不变）+ 根 ≤550 行、新文件全部 <5,000 行 + 全部原 pub 路径可解析（workspace build 证明）+ 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=PolicyRuntimeState 字段在 impl 外被直接构造/访问，搬 impl 不影响，但若 struct 留根、impl 搬出，字段可见性需匹配；检测=编译；缓解=struct 与 impl 保持同可见域（struct 留根为 pub，impl 内 fn 按 M4）；剩余=无。

---

### Unit 8：wave/dispatcher.rs 拆分（F2，12,452 行）

1. **Unit 目标**：拆分 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`：单个 `mod tests`（~5,700 行）外挂；生产区按项族外挂为 4-5 个子模块。
2. **对应需求与 Scenario**：R8；D1-D3；E1/E3。
3. **外部可观察结果**：无行为变化；dispatcher.rs 根 < 2,000 行。
4. **当前行为基线**：E3：类型族 WaveOutputs 40、WaveDispatchLimits 66、WaveDispatchOutcome 81、HandleWaveOutcome 128、WorkerRequest Clone impl 204、silent_request 233、SupervisorSlotRelease 284/291；执行族 ProductionExecutor impl 307、DispatchContext impl 391、handle_wave_events 489-1224、execute_wave 1225、execute_wave_structured 1303-1646、execute_wave_via_supervisor 1647-2496；协调族 SupervisorFanInOutcome impl 2497、emit_injected_failed_coord 3141、commit_complete_coord_event 3268、commit_failed_coord_event 3314、coordination_summary_from_receipt 3357、CoordCommitOutcome 3373、build_wave_complete_payload 3442、ReviewDoneHints 3684、payload_object 3759；helper 族 COORD_SYSTEM_PRODUCER 4080、fingerprint_coord_payload 4210、unix_now_secs 4225、merge_completed_exec_fix_slots_to_main 4348、commit_salvage_batch 4414、build_empty_projection_receipt 4536、fingerprint_lines 4555、build_wave_failed_slots_json 4582、status_to_str 4608、write_wave_diagnostics_json 4627、open_default_supervisor_store 4720、PARTIAL_THRESHOLD_* 4762/4763、WAVE_WORK_BUDGET_SLACK_SECS 4767、wave_work_budget 4780、aggregate_timeout_for 4800、aggregate_floor_for_attempts 4820、其余至 6756；`mod tests` 6758 起。
5. **输入与输出**：同前。
6. **修改位置**（生产区全部顶层项已列全，E3b；分组按行区间机械归属）：
   - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`：保留顶层公共类型（[40,283]：WaveOutputs、WaveDispatchLimits、WaveDispatchOutcome、HandleWaveOutcome、WorkerRequest Clone impl、silent_request）+ mod 声明 + re-export 为根。
   - 新建文件与内容（全部为 `crates/ralph-cli/src/loop_runner/wave/dispatcher/` 下）：

| 新文件 | 承接内容（行区间 → 锚点项） | 预期行数 |
|---|---|---|
| `slot.rs` | [284,488] SupervisorSlotRelease、impl Drop、impl WaveWorkerExecutor for ProductionExecutor | ~205 |
| `execute.rs` | [489,2496] DispatchContext impl、handle_wave_events、execute_wave、execute_wave_structured、execute_wave_via_supervisor | ~2,010 |
| `inner.rs` | [2497,3140] SupervisorFanInOutcome impl | ~645 |
| `coordination.rs` | [3141,4079] emit_injected_failed_coord、commit_complete_coord_event、commit_failed_coord_event、coordination_summary_from_receipt、CoordCommitOutcome、build_wave_complete_payload、ReviewDoneHints、payload_object、COORD_SYSTEM_PRODUCER | ~940 |
| `helpers.rs` | [4080,4920] + [5727,6756] fingerprint_coord_payload、unix_now_secs、merge_completed_exec_fix_slots_to_main、commit_salvage_batch、build_empty_projection_receipt、fingerprint_lines、build_wave_failed_slots_json、status_to_str、write_wave_diagnostics_json、open_default_supervisor_store、PARTIAL_THRESHOLD_NUM/DEN、WAVE_WORK_BUDGET_SLACK_SECS、wave_work_budget、aggregate_timeout_for、aggregate_floor_for_attempts、attempt_aware_aggregate_timeout、parse_assigned_dimension、compute_slot_batch_fingerprint、ClassifiedReason、ClassifiedSlot、classify_slot_result、classify_slot_attempt、reported_failure_detail、take_results、merge_round_into、outcome_for_completion、finalize_timeout、finalize_global_exceeded、inject_synthetic_failures、wait_for_progress_reporter、record_loop_max_runtime_envelope、record_wave_timeout_envelope、record_wave_spawn_failed_envelope、handle_wave_rejection | ~2,080 |
| `dispatch_inner.rs` | [4921,5726] dispatch_wave_inner_with_release（单 fn ~806 行，整体搬移） | ~805 |
| `tests/mod.rs` | `mod tests` 转为目录模块（6758 起，~5,694 行 > 5,000，**D7a 触发**）：先整体搬为 tests/mod.rs 作中间绿检查点，再按前缀分子组 | ≤150 |
| `tests/slot_binding.rs` | `*slot*` / `*bind*` 前缀 | ~900 |
| `tests/timeouts.rs` | `*timeout*` / `*budget*` / `*aggregate*` 前缀 | ~900 |
| `tests/coordination.rs` | `*coord*` / `*complete_payload*` / `*failed_payload*` 前缀 | ~900 |
| `tests/salvage_merge.rs` | `*salvage*` / `*merge*` / `*projection*` 前缀 | ~800 |
| `tests/supervisor.rs` | `*supervisor*` / `*fan_in*` / `*spawn*` 前缀 | ~900 |
| `tests/misc.rs` | 其余 fn（E13 保证 ≤ ~1,100 行） | ~1,100 |

   **行数验收**：根 ≤ 300 行；最大生产文件 execute.rs ≈ 2,010 行；tests/ 下最大 misc.rs ≈ 1,100 行；全部 < 5,000 行。**不修改**任何调度/合并/预算逻辑。
7. **可依赖能力**：U1-U7 流程。
8. **禁止依赖的未来能力**：不动 wave/ 其他文件（channel_registry/heartbeat/io/supervisor_bridge/task_projection/worker）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-cli -- wave` 与 `-- dispatch`。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 搬 tests（build + targeted）→ 搬 execute/handle（build + targeted）→ 搬 coordination（同）→ 搬 helpers（同）→ V2 → close。
13. **最小实现范围**：M1-M4；跨模块引用的私有 helper 按 M4 放宽到 `pub(super)`/`pub(crate)`。
14. **集成验证**：targeted wave 子集 + 全量（wave/supervisor 链路含 worktree 隔离与 fan-in merge 真实行为，全部由既有套件覆盖）。
15. **风险驱动测试**：既有套件；风险依据：dispatcher 含并发槽位/超时聚合逻辑，但本次零逻辑改动，Phase 1/2 全量即充分。
16. **回归范围**：ralph-cli 全包 + workspace 全量 + lint/fmt/build。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改（12,452 → ≤300 行） | 模块根化 | E1,E3,E3b |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/{slot,execute,inner,coordination,helpers,dispatch_inner}.rs` | 新增 6 个生产子模块（行数见 §6 表） | 承接项族 | E3b,E4 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/tests/{mod,slot_binding,timeouts,coordination,salvage_merge,supervisor,misc}.rs` | 新增 7 个测试子模块（D7a） | 承接 ~5,694 行测试区 | E13 |

18. **完成标准**：V2 全绿 + AC-2 名多重集一致（tests 子分组按 D7a 口径）+ 根 ≤300 行、新文件全部 <5,000 行 + 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=区域间私有 helper 跨文件引用；检测=编译；缓解=M4 最小可见性放宽；剩余风险=低（helpers.rs 为明示清单闭合，E3b 已消除未列项盲区）。

---

### Unit 9：event_loop/mod.rs 第一步——内联测试区与尾部非 impl 区拆分（F1）

1. **Unit 目标**：将 `crates/ralph-core/src/event_loop/mod.rs` 的 6 个内联 `#[cfg(test)] mod`（15248+）与尾部自由项（14937+）搬入子模块文件。
2. **对应需求与 Scenario**：R9、R11；D1-D3；E1/E3/E6/E7。
3. **外部可观察结果**：无行为变化；mod.rs 从 17,513 行降至 ~15,000 行。
4. **当前行为基线**：E3/E6：内联测试 mod——`u7_rejection_stale_characterization`（15248）、`u4_current_plan_step_tests`（15670）、`p0_4_flow_authority_ledger_tests`（16289 附近）、`hat_only_pipeline_tests`（16478）、`flow_authority_pf_recovery_tests`（16499）、`flow_authority_pf_declared_14step_tests`（16838）；`pub mod wave_branch_tests;`（16819）已外挂不动；尾部自由项——EventLoopResumeDecision 14937、UserPrompt 14949、run_stall_detector_on_state 14988、is_rejection_stale 15204、self_is_state_idempotency_required 15346、extract_step_id 15426、initial_current_plan_step 15446、recover_current_plan_step 15587（pub）、load_flow_authority_current_step 15628（pub）。12 处 `#[cfg(test)]` 中若有嵌套于上述 mod 内部者，随宿主整体搬移。
5. **输入与输出**：同前。
6. **修改位置**：
   - `crates/ralph-core/src/event_loop/mod.rs`：6 个测试 mod 原位改路径式声明 `#[cfg(test)] mod NAME;`；尾部自由项搬出后按需 `pub use` re-export（recover_current_plan_step、load_flow_authority_current_step、UserPrompt、EventLoopResumeDecision 为 pub 或被跨模块引用）。
   - 新增 `crates/ralph-core/src/event_loop/`：`u7_rejection_stale_characterization.rs`、`u4_current_plan_step_tests.rs`、`p0_4_flow_authority_ledger_tests.rs`、`hat_only_pipeline_tests.rs`、`flow_authority_pf_recovery_tests.rs`、`flow_authority_pf_declared_14step_tests.rs`、`resume_types.rs`（EventLoopResumeDecision + UserPrompt）、`stall_recovery.rs`（run_stall_detector_on_state / is_rejection_stale / self_is_state_idempotency_required / extract_step_step→extract_step_id / initial_current_plan_step / recover_current_plan_step / load_flow_authority_current_step）。
   - `.cursor/rules/state-management.mdc` 行 28（`event_loop/mod.rs:1816-1835`）：拆分后 `rg -n 'literal-vs-glob'` 或按原文描述定位新位置并更新（E7）。注意该引用位于 impl 区（1816-1835 在 build_with_context 内），本 Unit 不动 impl 区，行号暂不变；**此条同步在 U11 执行**。
7. **可依赖能力**：U1-U8 流程；event_loop/tests/ 目录先例（E4）。
8. **禁止依赖的未来能力**：不动 impl EventLoop 块（U11）、不动头部 83-1094 区（U10）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-core -- event_loop`、`-- flow_authority`、`-- stall`。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 逐测试 mod 搬移（每 mod build + targeted）→ 搬尾部自由项（build + targeted）→ V2 → close。
13. **最小实现范围**：M1-M4。
14. **集成验证**：targeted + workspace 全量（BDD scenarios 依赖 event_loop 全部语义）。
15. **风险驱动测试**：既有套件即 Characterization（风险依据：event_loop 是运行时核心，任何搬移错漏都会被 scenarios/smoke/preset_lint 测试捕获）。
16. **回归范围**：workspace 全量 + lint/fmt/build。理由：ralph-core 被全部包消费。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 修改（17,513 → ~14,950 行） | 测试 mod 路径化 + 尾部项搬出 | E1,E3,E6 |
| `crates/ralph-core/src/event_loop/u7_rejection_stale_characterization.rs` | 新增（~420 行） | 承接同名测试 mod（15248-15669） | E3,E4 |
| `crates/ralph-core/src/event_loop/u4_current_plan_step_tests.rs` | 新增（~620 行） | 承接同名测试 mod（15670-16288） | E3,E4 |
| `crates/ralph-core/src/event_loop/p0_4_flow_authority_ledger_tests.rs` | 新增（~190 行） | 承接同名测试 mod（16289-16477） | E3,E4 |
| `crates/ralph-core/src/event_loop/hat_only_pipeline_tests.rs` | 新增（~20 行） | 承接同名测试 mod（16478-16498） | E3,E4 |
| `crates/ralph-core/src/event_loop/flow_authority_pf_recovery_tests.rs` | 新增（~320 行） | 承接同名测试 mod（16499-16818） | E3,E4 |
| `crates/ralph-core/src/event_loop/flow_authority_pf_declared_14step_tests.rs` | 新增（~675 行） | 承接同名测试 mod（16838-末尾） | E3,E4 |
| `crates/ralph-core/src/event_loop/resume_types.rs` | 新增（~50 行） | EventLoopResumeDecision + UserPrompt | E6 |
| `crates/ralph-core/src/event_loop/stall_recovery.rs` | 新增（~590 行） | 尾部自由 fn 族（14988-15628+ 至测试区前） | E6 |

   全部新文件 < 700 行。**行数验收**：mod.rs ≤ 15,000 行。

18. **完成标准**：V2 全绿 + AC-2 diff 空 + mod.rs ≤ 15,300 行 + 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=内联测试 mod 引用 mod.rs 内私有 fn（如 is_rejection_stale 同时被生产与测试引用）；检测=编译；缓解=先搬生产自由项再搬测试 mod，顺序固定；剩余=无。

---

### Unit 10：event_loop/mod.rs 第二步——头部非 impl 区拆分（F1）

1. **Unit 目标**：将 mod.rs 行 83-1094 的自由项（prompt 类型、SkillInjector、flow declaration/mechanism/phase authority/stage pipeline 构建器）搬入子模块。
2. **对应需求与 Scenario**：R9；D1-D3；E3/E6。
3. **外部可观察结果**：无行为变化；mod.rs 再降 ~1,000 行。
4. **当前行为基线**：E6：TerminationReason impl 272、RecoverableExhaustion 370、OverEmitRecovery 390、PromptPreview 442、PromptGates 496、SkillGateFlags 507、default_evidence_level 516、is_static_evidence_level 520、PromptSkillEntry 526、PromptSkillSource 540/546、SkillInjector 580/582、is_isolated_exempt_topic 710、strip_human_guidance_block 754、minimal_flow_declaration_yaml 778、load_opt_in_flow_declaration 866（pub）、effective_mechanism_config 880、build_phase_authority_arc 889、build_stage_pipeline_from_config 901、unknown_fix_step 963、build_invalid_step_target_resume_payload_for_jsonl 987、preview_prompt_for_config 1043（pub）。（83-271 区间为 use/常量/其他小项，随大纲实测整体归入下述两文件之一。）
5. **输入与输出**：同前。
6. **修改位置**：
   - `crates/ralph-core/src/event_loop/mod.rs`：搬出后按需 re-export（pub 项：load_opt_in_flow_declaration、preview_prompt_for_config；被 impl 区引用的私有项按 M4）。
   - 新增：`event_loop/prompt_types.rs`（272-777 区间：TerminationReason impl、RecoverableExhaustion、OverEmitRecovery、PromptPreview、PromptGates、SkillGateFlags、evidence level fn、PromptSkillEntry/Source、SkillInjector、is_isolated_exempt_topic、strip_human_guidance_block）；`event_loop/flow_wiring.rs`（778-1094 区间：minimal_flow_declaration_yaml、load_opt_in_flow_declaration、effective_mechanism_config、build_phase_authority_arc、build_stage_pipeline_from_config、unknown_fix_step、build_invalid_step_target_resume_payload_for_jsonl、preview_prompt_for_config）。**区间规则：以项定义起始行归属，边界项不跨界。**
7. **可依赖能力**：U9 完成态。
8. **禁止依赖的未来能力**：不动 impl 块（U11）。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-core -- event_loop`、`-- prompt`、`-- flow`。
10. **Acceptance Red**：同 U1。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 → 搬 prompt_types 区（build + targeted）→ 搬 flow_wiring 区（同）→ V2 → close。
13. **最小实现范围**：M1-M4。
14. **集成验证**：targeted + workspace 全量。
15. **风险驱动测试**：既有套件。
16. **回归范围**：workspace 全量 + lint/fmt/build。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 修改（~14,950 → ~13,850 行） | 头部项搬出 | E6 |
| `crates/ralph-core/src/event_loop/prompt_types.rs` | 新增（~505 行） | 承接 [272,777] 区间：TerminationReason impl、RecoverableExhaustion、OverEmitRecovery、PromptPreview、PromptGates、SkillGateFlags、default_evidence_level、is_static_evidence_level、PromptSkillEntry、PromptSkillSource、SkillInjector、is_isolated_exempt_topic、strip_human_guidance_block | E4,E6 |
| `crates/ralph-core/src/event_loop/flow_wiring.rs` | 新增（~315 行） | 承接 [778,1094] 区间：minimal_flow_declaration_yaml、load_opt_in_flow_declaration、effective_mechanism_config、build_phase_authority_arc、build_stage_pipeline_from_config、unknown_fix_step、build_invalid_step_target_resume_payload_for_jsonl、preview_prompt_for_config | E4,E6 |

   **行数验收**：两个新文件均 < 550 行；mod.rs ≤ 13,900 行。

18. **完成标准**：V2 全绿 + AC-2 diff 空 + mod.rs ≤ 14,300 行 + 独立提交。
19. **停止条件**：S1。
20. **风险与注意事项**：风险=SkillInjector 与 impl 区 build_prompt 双向引用；检测=编译；缓解=M4 最小可见性；剩余=无。

---

### Unit 11：event_loop/mod.rs 第三步——impl EventLoop 块按行区间拆分为 10 个子模块（F1）

1. **Unit 目标**：将单个 `impl EventLoop` 块（1095-~14936，~200 方法）按 D8 行区间表拆为 10 个 `impl EventLoop` 子模块文件；收尾 `.cursor/rules/state-management.mdc` 行号同步。
2. **对应需求与 Scenario**：R10、R11；D3/D4/D8；E6/E7。
3. **外部可观察结果**：无行为变化；mod.rs 根 ≤ 1,500 行（mod 声明 + re-export + 少量保留项）。
4. **当前行为基线**：E6 全方法大纲已提取（方法起始行全部在册）；impl 块为单一块，方法按行连续分布。
5. **输入与输出**：同前。
6. **修改位置**（区域表，方法按定义起始行归属，整体搬移；每个区域一个新文件，文件内为 `use super::*;` + `impl EventLoop { … }`）：

| 区域 | 新文件 | 行区间 | 预期行数 | 锚点方法（首→尾） |
|---|---|---|---|---|
| R1 | `event_loop/construction.rs` | [1095, 2000) | ~905 | mark_required_event_seen → loop_context |
| R2 | `event_loop/state_accessors.rs` | [2000, 2830) | ~830 | tasks_path → set_observer（含 isolated_publish_allowed / validate_resume_routing / enforce_wave_isolated_scope / publish_isolated_wave_violation） |
| R3 | `event_loop/termination.rs` | [2830, 3662) | ~830 | check_termination → completion_payload_mismatch |
| R4 | `event_loop/initialization.rs` | [3662, 3870) | ~210 | initialize → write_hold_artifact |
| R5 | `event_loop/scheduling.rs` | [3870, 4832) | ~960 | next_hat → inject_fallback_event |
| R6 | `event_loop/prompt_build.rs` | [4832, 8060) | ~3,230 | format_recovery_diagnosis_block → apply_engine_required_field_gate（含 build_prompt、build_prompt_body、全部 prepend_* / inject_* / robot guidance / recovery directives） |
| R7 | `event_loop/event_processing.rs` | [8060, 9416) | ~1,355 | parse_event_payload_value → check_workflow_guard_completion（含 process_output、audit_file_modifications、check_default_publishes、persist_system_injected_jsonl_event、bus） |
| R8 | `event_loop/parse_result.rs` | [9416, 13588) | ~4,170 | process_parse_result（**~3,777 行单方法，整体搬移，禁拆体**）→ discharge_obligations_for_accepted（含 drive_step_close_progress / drive_step_transition / drive_precheck_gate_obligation / dispatch_precheck_rejection） |
| R9 | `event_loop/wave_emit.rs` | [13588, 14881) | ~1,295 | process_events_from_jsonl_with_waves → loop_id_label（含 publish_event / publish_terminate_event / emit gate 族 / phase authority 族 / stall detector） |
| R10 | `event_loop/user_prompt.rs` | [14881, 14937) | ~55 | check_for_user_prompt → generate_prompt_id |

   合计 ~13,840 行 = impl 块总量，搬移后 mod.rs 内 impl 区清零。最大单文件 parse_result.rs ≈ 4,170 行（其中 ~3,777 行为单个不可拆方法）；prompt_build.rs ≈ 3,230 行——两者均 < 5,000，是本计划的预期终态（进一步缩小需方法体拆分，属另一个计划）。

   - 边界规则：方法定义起始行在区间内即归属该区间；跨区间调用由 M4 可见性解决（默认 `pub(super)` 起步）。
   - mod.rs 保留：impl 块之外的 struct EventLoop 定义（若在 mod.rs 中）、mod 声明、必要 re-export。
   - `.cursor/rules/state-management.mdc` 行 28（`mod.rs:1816-1835`）：按新位置更新（1816-1835 属 R1 区间 → construction.rs，`rg -n` 复核实际行号后更新）（E7）。
7. **可依赖能力**：U9/U10 完成态。
8. **禁止依赖的未来能力**：无后续 Unit。
9. **验收测试**：AC-1..AC-5；targeted：`cargo nextest run -p ralph-core -- event_loop`（每区域搬完即跑）。
10. **Acceptance Red**：同 U1；特别强调：impl 拆分最常见的错误是误删/重复方法——AC-2 之外，追加核对：拆分前后 `rg -c '^\s{4}(pub )?(async )?fn ' mod.rs 与新文件之和` 必须相等。
11. **单元测试拆分**：不新增。
12. **Red → Green → Refactor 顺序**：快照 + 方法计数基线 → R1 → R2 → … → R10（每区域：搬移 → build → targeted event_loop 子集 → 方法计数核对）→ V2 全量 → mdc 行号同步 → close。区域严格按 R1→R10 顺序，不得交错。
13. **最小实现范围**：M1-M4；任何方法体逐字节不变；`process_parse_result` 只允许整体出现在 parse_result.rs。
14. **集成验证**：workspace 全量（BDD scenarios、smoke、preset_lint、ralph-cli 集成测全部依赖 EventLoop 行为）。
15. **风险驱动测试**：既有套件即 Differential 防线；风险依据=impl 拆分是本计划最大风险点，全量 7,560 测试 + 方法计数核对 + fmt-check 三重兜底。
16. **回归范围**：workspace 全量 + lint/fmt/build + `cargo doc --no-deps`（确认文档链接无断裂）。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 修改（~13,850 → ≤1,500 行：struct EventLoop 定义 + 52+10 个 mod 声明 + re-export） | impl 块拆出 | E1,E6 |
| `crates/ralph-core/src/event_loop/{construction,state_accessors,termination,initialization,scheduling,prompt_build,event_processing,parse_result,wave_emit,user_prompt}.rs` | 新增 10 个文件（行数见 §6 区域表：55 ~ 4,170 行） | 承接 10 区域 | E4,E6 |
| `.cursor/rules/state-management.mdc` | 修改（行 28 引用） | 行号漂移同步 | E7 |

18. **完成标准**：V2 全绿 + AC-2 diff 空 + 方法计数前后一致 + mod.rs 根 ≤ 1,500 行 + `cargo doc --no-deps` 通过 + mdc 复核 + 独立提交。**至此 9 个目标文件全部完成。**
19. **停止条件**：S1；另：若任一区域搬移后 targeted 失败且错误指向方法体内部差异，立即 `git diff` 逐字节比对回滚，严禁「修正逻辑使其通过」。
20. **风险与注意事项**：风险①=区域间私有方法互调导致可见性放宽扩散；检测=编译；缓解=M4 从 `pub(super)` 起步；风险②=13,800 行搬移中的意外字节漂移；检测=函数体抽查 diff + fmt-check + 全量测试；风险③=方法计数不一致（漏搬/重复）；检测=每区域计数核对；剩余风险=低（三重兜底）。

---

## 8. Unit 串行依赖图

```text
U1 (task_cli)
  ↓ 固化 M1-M4/V1/V2 机制
U2 (policy_check)
  ↓ 验证生产族+测试 mod 双拆分
U3 (emit)
  ↓ 验证 commands/ 目录内拆分
U4 (runner)
  ↓ 验证巨型单 fn 整体搬移
U5 (tests/legacy)
  ↓ 验证纯测试前缀分组（D10）
U6 (tests/wave_supervisor)
  ↓ 复用 U5 分组机制
U7 (event_policy)
  ↓ 验证跨 crate re-export 保持
U8 (wave/dispatcher)
  ↓ 验证大测试区+生产族组合
U9 (event_loop 测试区+尾部)
  ↓
U10 (event_loop 头部)
  ↓
U11 (event_loop impl 十区域)
```

依赖说明：
- U1→U2→…：后续 Unit 复用前置 Unit 已验证的机械流程；顺序不可交换的原因是风险控制（小文件先暴露机制缺陷），而非代码依赖——即便无代码依赖，仍强制此顺序（用户要求严格串行）。
- U9→U10→U11 为同一文件的三步拆分，存在真实文件级依赖：U10/U11 的区域行号以 U9 完成后的文件状态为准（U9 只动 14937+ 区间，不改变 1095-14936 的行号，U11 区域表行号在 U9/U10 后仍有效；若实现中发现行号漂移，以**方法名**为准重新对齐，不视为逻辑变更）。
- 防提前实现：每个 Unit 的 diff 仅允许触及其「修改位置」表所列文件；review 时 `git diff --stat` 核对。

---

## 9. 执行命令清单

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败可否进入下一步 |
|---|---|---|---|---|
| `./scripts/run-tests.sh` | 每 Unit 开始前一次 + Unit 末（V2） | 全量回归唯一入口（两阶段+doctest，HARD RULE 1/2） | Phase1 7514 pass/0 fail；Phase2 23 pass/0 fail；Doctest 19 pass/4 ignored；exit 0 | 否 |
| `cargo nextest list --workspace > /tmp/list-before.txt`（Unit 首）与完成后 diff | 每 Unit 首尾 | AC-2 测试清单不变量（D7 口径） | 非 F3/F8 Unit：diff 空；F3/F8：名多重集一致 | 否 |
| `cargo build --workspace` | 每搬移批次后（V1） | 编译级即时反馈 | 零错误 | 否（当场修复） |
| `cargo nextest run -p <crate> -- <子串>`（如 `-p ralph-cli -- task_cli` / `-p ralph-core -- event_loop`） | 每搬移批次后（V1） | targeted 快环 | 全绿 | 否 |
| `just lint`（= `cargo clippy --all-targets --all-features -- -D warnings`，E11） | 每 Unit 末（V2） | 静态门禁 | 零告警 | 否 |
| `just fmt-check`（= `cargo fmt --all -- --check`，E11） | 每 Unit 末（V2） | 格式零 diff | 零 diff | 否 |
| `cargo doc --no-deps` | U11 末 | 文档链接完整性 | 无断裂 | 否 |
| `cargo run -p ralph-e2e -- --mock` | 全部 Unit 完成后一次（最终门禁补充；run-tests.sh 不含 e2e crate） | E2E 冒烟 | 通过 | 否 |
| `git diff --stat` / 函数体抽查 diff | 每 Unit 末（AC-5） | diff 纯度：仅计划内文件、函数体逐字节不变 | 符合 | 否 |

禁止项（HARD RULE 复述）：禁止裸 `cargo test -p ralph-cli`；禁止手动 `cargo nextest run --workspace` 替代 run-tests.sh（跳过 Phase 2 隔离）；targeted 子集运行时确认无 `RALPH_CURRENT_HAT` 等 agent env 残留（HARD RULE 5）。

---

## 10. 最终质量门禁

全部 Unit 完成后逐项验证：

- [ ] 9 个目标文件全部拆分，每个根文件行数达标（task_cli ≤250；policy_check ≤450；emit ≤850；runner ≤150；legacy ≤100；wave_supervisor ≤300；event_policy ≤550；dispatcher ≤300；event_loop/mod.rs ≤1,500）
- [ ] **所有新建文件 < 5,000 行**（全计划共新增 ~57 个文件，最大为 event_loop/parse_result.rs ≈4,170、runner/inner.rs ≈4,305、emit/tests.rs ≈3,715、legacy/misc.rs ≈3,400、prompt_build.rs ≈3,230；逐 Unit 清单见各 Unit §6/§17 表）
- [ ] `./scripts/run-tests.sh` 全绿且计数与 E2 基线完全一致（7514 / 23 / 19+4）
- [ ] 所有 Unit 的 AC-2 清单核对通过（D7 口径）
- [ ] `just lint`、`just fmt-check`、`cargo build --workspace`、`cargo doc --no-deps` 全过
- [ ] `cargo run -p ralph-e2e -- --mock` 通过
- [ ] `.cursor/rules/state-management.mdc` 4 条行号引用全部更新并 sed 复核（E7）
- [ ] 每个 Unit 独立提交，提交说明含分组表（U5/U6）与区域核对结果（U11）
- [ ] 无新增跳过测试、无 `.only`/ignore、无削弱断言、无 snapshot/golden 更新
- [ ] diff 纯度审查通过：无任何函数体内部改动
- [ ] 公开 API 路径全部保持（workspace build + re-export 证明）
- [ ] 未验证内容与剩余风险已明确（见下）

**未验证内容**：`docs/reviews/`、`docs/superpowers/`、`docs/solutions/`、`ux-findings.md` 中的历史行号引用不改写（D9，历史快照）；拆分后新模块路径与这些历史文档不再对应属预期。

**剩余风险**：① 巨型单方法（process_parse_result 等）拆分后仍是单体大方法，单文件行数改善有限处（parse_result.rs 预计仍 ~4,000 行）——方法体拆分属逻辑重构，需另立计划；② dispatcher.rs 4820-6756 区间大纲未逐项列全，由兜底规则（helpers.rs）覆盖，不影响正确性。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 给出文件级修改位置、行区间、锚点方法、命令与门禁 |
| Executor 是否仍需做关键设计决策 | 否 | 拆分模式（D1）、搬移规则（M1-M4）、区域表（U11）、分组规则（D10）均已决；编译错误修复手段限定为两种机械形式 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E12；全部路径经 ls/grep/awk 实测 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D10 全部 ≥ 0.86 |
| 是否存在未处理的低置信度假设 | 否 | 无假设进入实施路径 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 每 Unit = 一个文件的模块拆分 + 全量绿 |
| 每个 Unit 是否可以独立验证 | 是 | V1 快环 + V2 全量门禁 + AC-2 清单核对 |
| 每个 Unit 是否有真实 Red | 是（重构等价形式） | 编译/targeted/清单任一失败即红，判据在各 Unit §10 明示 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit §16 |
| 是否存在未来 Unit 依赖 | 否 | 仅文件级串行依赖（U9→U10→U11），已在 §8 声明且为顺序必需 |
| 是否存在泛化任务描述 | 否 | 全部 Unit 到文件/行区间/方法名级别 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §6 追踪矩阵 R1-R11 |
| 所有关键决策是否有 Evidence | 是 | §3 各决策挂 E 编号 |
| 计划是否可以严格串行执行 | 是 | §8 线性图 + 每 Unit 独立提交边界 |
