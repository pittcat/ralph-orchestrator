---
title: "fix: 修复 ralph-tools agent reference 验证门禁"
type: fix
status: active
date: 2026-06-02
origin: docs/brainstorms/2026-06-01-ralph-cli-agent-reference-requirements.md
related_plan: docs/plans/2026-06-01-002-feat-ralph-cli-agent-reference-plan.md
---

# fix: 修复 ralph-tools agent reference 验证门禁

## Overview

当前 `ralph-tools` 分层拆分功能本身基本可用：入口文件已缩小，`ralph-tools-emit` / `ralph-tools-wave` / `ralph-tools-cmdref` 能作为 builtin skill 被列出并加载，`.claude/skills` symlink 也指向正确源文件。

问题集中在验收层：CI 的 BDD job 空跑、BDD 场景没有执行真实 CLI 路径、CLI 文档漂移检测默认用 baseline 吞掉 292 个 drift、schema 解析器还会漏掉合法的 variadic flag。这会让“文档与 `--help` 保持一致”和“agent 能按需加载并正确使用 CLI”的核心成功标准失去可信度。

本计划修复这些验证门禁，不重新设计已合入的 skill 分层结构。

## Problem Frame

原需求要求 CLI 参考“全面、权威、内嵌校验步骤”，并明确成功标准包括：

- Skill 内容与 `ralph --help` / `ralph <subcommand> --help` 保持一致。
- 每条高频命令都有可执行校验步骤。
- `ralph tools skill load ralph-tools` 和运行时特殊注入路径保持不变。

已合入实现达成了可加载性和 prompt 大小目标，但暴露出 4 个验证缺口：

- `.github/workflows/ci.yml` 的 BDD filter 使用连字符名称，实际 Rust test 使用下划线名称，CI 当前会跑 0 个测试。
- `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.yml` 只是 mock 文本；`run_scenario` 只断言迭代数和 backend 被调用，没有执行 `ralph tools skill load`、`ralph emit` 或 `ralph tools interact progress`。
- `scripts/check-cli-doc-drift.sh` 默认 baseline 模式把 292 个 drift 视为已知问题并返回 0，不能作为硬门禁。
- `scripts/extract-cli-schema.py` 不识别 clap 的 `<VALUE>...` 变长参数形态，导致 `ralph wave emit --payloads <PAYLOADS>...` 被误判为文档漂移。

## Requirements Trace

- R1. CI 必须真实运行 agent reference 的验收测试，不允许空跑或仅检查 mock 文本。
- R2. 文档漂移检测必须在正常状态下严格通过；任何新增、删除、重命名或文档残留的 flag drift 都应让门禁失败。
- R3. 漂移检测必须区分“命令专属 flag”和“全局/inherited flag”，避免通过 baseline 掩盖误报。
- R4. `ralph-tools-emit`、`ralph-tools-wave`、`ralph-tools-cmdref`、`ralph-tools-tasks`、`ralph-tools-memories` 的覆盖范围必须与实际 CLI `--help` 输出形成可验证契约。
- R5. `ralph tools skill list/load` 的现有行为、hat 过滤、无 hat agent fail-closed 行为不能回归。
- R6. prompt 大小保护、builtin skill 注册、symlink 解析继续作为门禁保留。

## Scope Boundaries

- 不重新拆分 `ralph-tools` 文件结构。
- 不修改 `ralph emit`、`ralph wave emit`、`ralph tools skill` 等 CLI 命令的业务行为，除非实现阶段发现文档正确性必须依赖一个已存在的 CLI bug 修复。
- 不引入自动文档生成系统；本次只修复静态文档与 `--help` 的一致性检测。
- 不把低频命令扩写成完整手册；低频命令只需要有明确范围规则，并被 drift checker 正确处理。

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/skill_registry.rs` 已注册 3 个新 builtin skill，现有集成测试覆盖 list/load。
- `crates/ralph-cli/src/skill_cli.rs` 已实现 agent 上下文缺少 `RALPH_CURRENT_HAT` 时 fail closed；`crates/ralph-cli/tests/integration_skill.rs` 有相关测试。
- `scripts/check-cli-doc-drift.sh` 已有 section-aware 结构，但默认 baseline 模式削弱了门禁。
- `scripts/extract-cli-schema.py` 是 drift checker 的唯一 `--help` 解析入口，应优先修正并补测试。
- `crates/ralph-core/tests/scenarios.rs` 的 `run_scenario` 目前不验证 YAML `expected.events`，也不执行 shell 命令；它不适合独立证明 CLI 可用性。
- `.github/workflows/ci.yml` 现在把 agent reference BDD 拆成单独 job，但 test filter 错误导致 job 空跑。

### Institutional Learnings

- `docs/solutions/ralph-zsh-completion-issue.md` 记录过“非交互式脚本无法完整模拟真实终端行为”的问题。这里的类比是：mock response 不能证明 CLI 命令真实可执行，验收必须落到真实运行路径。
- `docs/brainstorms/2026-05-31-agent-operation-guard-requirements.md` 明确要求 `ralph tools skill list/load` 在 agent 上下文中应用当前 hat 过滤，缺少 hat 时 fail closed。本计划的测试必须保留这条安全边界。

### External References

- 本次不需要外部研究。问题和修复路径都由本仓库脚本、测试和 CI 配置决定。

## Key Technical Decisions

- **先修 parser，再收紧门禁。** 当前 strict drift 有真实漂移也有误报；先让 schema 提取可信，再把 CI 改为 strict，避免把错误脚本升级成硬失败。
- **用真实 CLI integration 覆盖 agent reference，而不是继续依赖 mock-only BDD。** `ralph tools skill load`、`ralph emit`、`ralph tools interact progress` 都是 CLI surface，最可靠的验收位置是 `crates/ralph-cli/tests/`。
- **保留 BDD job 但修正其职责。** 如果继续保留 core scenario，它只能作为 event-loop 烟雾测试；真正的 agent reference 验收应由 CLI integration test 承担。CI job 名称和命令要反映这一点，避免“BDD 已覆盖端到端”的假信号。
- **删除或收敛 baseline。** `scripts/cli-doc-drift.baseline` 不能保存 292 条长期 drift 并让 CI 通过。若确有暂时无法覆盖的低频命令，必须以显式 allowlist 配置表达范围，而不是把全部输出固化为 baseline。

## Open Questions

### Resolved During Planning

- 是否继续使用现有 core YAML scenario 作为主要验收？不继续。它不会执行真实 CLI，也没有足够断言能力。
- 是否需要修改 CLI 业务行为？当前没有证据需要。修复范围应优先限制在脚本、文档、测试和 CI。
- 是否需要覆盖所有低频命令的每个 global flag？不一定。需要先定义 drift checker 的覆盖策略：高频命令严格逐 flag；低频命令允许只在共享“全局选项”契约中覆盖 inherited flags。

### Deferred to Implementation

- 292 个 drift 中哪些是真实文档错误、哪些是 parser/scope 误报：需要在修正 parser 和 scope 后重新跑 strict drift 决定。
- 是否删除 `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.yml`：实现阶段可在“改成有真实断言”与“移除误导性场景”之间选择，前提是 CLI integration 已覆盖真实行为。

## Implementation Units

- [ ] **Unit 1: 为 drift parser 和 checker 加特征化测试**

**Goal:** 先冻结当前失败模式，防止修复后再次出现“误报靠 baseline 掩盖”或“variadic flag 漏解析”。

**Requirements:** R2, R3, R4

**Dependencies:** 无

**Files:**
- Modify: `scripts/extract-cli-schema.py`
- Modify: `scripts/check-cli-doc-drift.sh`
- Create: `scripts/test-cli-doc-drift.sh`
- Test: `scripts/test-cli-doc-drift.sh`

**Approach:**
- 新增脚本级测试，使用固定 help 文本 fixture 或临时 shim 命令验证 parser 行为，而不是依赖完整 cargo build。
- 覆盖 clap 常见 flag 形态：`--flag`、`--flag <VALUE>`、`--flag <VALUE>...`、`-x, --flag`、多行描述。
- 覆盖 checker 的退出码语义：strict 模式遇到 drift 必须失败；无 drift 必须成功；默认模式不得在存在未说明 drift 时静默通过。

**Execution note:** 先写会失败的脚本测试，再修 parser/checker。

**Patterns to follow:**
- `scripts/guard-prompt-size.sh` 的轻量 shell gate 风格。
- `scripts/check-cli-doc-drift.sh` 现有 section-aware mapping，但不要沿用 baseline 作为默认通过机制。

**Test scenarios:**
- Happy path: help 文本包含 `--payloads <PAYLOADS>...`，parser 输出包含 `payloads` 且标记为 takes value。
- Happy path: help 文本包含 `-j, --json`，parser 输出 short flag 和 long flag。
- Edge case: 文档 section 没有 drift 时 checker strict 返回成功。
- Error path: 文档缺少 help 中的 `--json` 时 checker strict 返回失败并输出具体 command/doc/section。
- Error path: 文档提到不存在的 `--no-such-flag` 时 checker strict 返回失败。

**Verification:**
- parser 不再漏掉 `ralph wave emit --payloads`。
- checker 的测试能在不构建完整工作空间的情况下稳定运行。

- [ ] **Unit 2: 重新定义并收紧 CLI 文档漂移门禁**

**Goal:** 让正常仓库状态下 drift checker 严格通过，CI 不能再依赖 292 条 baseline 漂移。

**Requirements:** R2, R3, R4

**Dependencies:** Unit 1

**Files:**
- Modify: `scripts/check-cli-doc-drift.sh`
- Modify or remove: `scripts/cli-doc-drift.baseline`
- Modify: `.github/workflows/ci.yml`
- Modify as needed: `crates/ralph-core/data/ralph-tools-emit.md`
- Modify as needed: `crates/ralph-core/data/ralph-tools-wave.md`
- Modify as needed: `crates/ralph-core/data/ralph-tools-cmdref.md`
- Modify as needed: `crates/ralph-core/data/ralph-tools-tasks.md`
- Modify as needed: `crates/ralph-core/data/ralph-tools-memories.md`
- Test: `scripts/test-cli-doc-drift.sh`

**Approach:**
- 把 drift checker 的默认 CI 语义改为 strict，或让 CI 显式传 strict 模式。
- 删除大 baseline；如确实需要暂时排除低频命令，使用小而明确的 ignore list，并在脚本注释中说明每条排除的理由。
- 明确定义 global flags 策略：`--config`、`--hats`、`--verbose`、`--color`、`--help` 这类 inherited flags 要么在共享 section 中统一覆盖，要么在 checker 中作为 documented globals 处理，不能对每个子命令重复产生 drift。
- 重新跑严格 drift 后修正文档真实错误，例如 `ralph wave emit --payloads`、`ralph emit` 不存在 `--format` 等已知差异。

**Patterns to follow:**
- 入口文件当前“遇到不确定命令先 `--help`”的原则保留。
- 任务/记忆文档仍保持独立注入，不把大段内容塞回 `ralph-tools.md`。

**Test scenarios:**
- Happy path: 当前文档与当前 CLI help 对齐时，strict checker 返回成功。
- Error path: 在 `ralph-tools-emit.md` 临时加入不存在 flag，strict checker 返回失败。
- Error path: 从 `ralph-tools-wave.md` 临时删除 `--payloads`，strict checker 返回失败。
- Edge case: global flags 不会在每个子命令 section 中制造重复误报。
- Regression: `scripts/guard-prompt-size.sh` 仍通过，`ralph-tools.md` 不膨胀超过 200 行。

**Verification:**
- CI 中 CLI doc drift job 不再打印“known in baseline, 0 new”作为成功原因。
- `scripts/cli-doc-drift.baseline` 为空、删除，或只含少量有明确理由和到期条件的临时排除项。

- [ ] **Unit 3: 用真实 CLI integration 替换 mock-only agent reference 验收**

**Goal:** 证明 agent reference 相关命令真的能在临时 Ralph workspace 中执行，而不是只在 mock response 文本里出现。

**Requirements:** R1, R5

**Dependencies:** Unit 1 可并行；Unit 2 不阻塞

**Files:**
- Create: `crates/ralph-cli/tests/integration_agent_reference.rs`
- Modify: `crates/ralph-cli/tests/integration_skill.rs`
- Modify or remove: `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.yml`
- Modify: `crates/ralph-core/tests/scenarios.rs`

**Approach:**
- 在 `ralph-cli` integration test 中创建临时 workspace，写入最小 `.ralph` 状态和 `ralph.yml`，通过 `CARGO_BIN_EXE_ralph` 执行真实 CLI。
- 覆盖三个实际流程：
  - human/diagnostic context 能 `skill load ralph-tools-emit` 并看到错误恢复表。
  - agent context 设置 `RALPH_CURRENT_HAT` 后能 `skill load ralph-tools-cmdref`，随后 `interact progress` 的成功路径或明确的 no-op/test-mode 成功路径可验证。
  - 在有事件文件 marker 的 workspace 中执行 `ralph emit build.done '{"ok":true}' -j`，事件文件末尾包含 JSON object payload。
- 对 fail-closed 行为使用已存在 `integration_skill.rs` 模式：agent context 缺少 `RALPH_CURRENT_HAT` 时 `skill load` 非零退出，stderr 提到 `RALPH_CURRENT_HAT`。
- 对 core YAML scenario 做二选一处理：如果保留，必须让 `run_scenario` 至少断言 expected events；如果不保留，删除误导性场景和对应测试，避免“BDD 覆盖端到端”的错误描述。

**Execution note:** 以 characterization-first 方式先写能暴露当前 mock-only 缺口的测试，再替换 CI 入口。

**Patterns to follow:**
- `crates/ralph-cli/tests/integration_skill.rs` 中用 `TempDir` 和 `CARGO_BIN_EXE_ralph` 调用 CLI 的模式。
- `crates/ralph-cli/src/skill_cli.rs` 的 agent/human context 行为边界。

**Test scenarios:**
- Happy path: `ralph tools skill list --format quiet` 包含 `ralph-tools-emit`、`ralph-tools-wave`、`ralph-tools-cmdref`。
- Happy path: `ralph tools skill load ralph-tools-emit` 输出 `Invalid JSON payload` 和 `事件文件解析优先级`。
- Happy path: `ralph emit build.done '{"ok":true}' -j` 写入 active events file，payload 为 JSON object。
- Happy path: `ralph tools skill load ralph-tools-cmdref` 输出 `ralph tools interact progress` 的参考内容。
- Error path: agent context 缺少 `RALPH_CURRENT_HAT` 时 `skill load` fail closed。
- Regression: human CLI context 不设置 `RALPH_CURRENT_HAT` 仍可 list/load 全部 builtin skill。

**Verification:**
- 新 integration test 真实启动 `ralph` binary，不依赖 mock response 文本。
- 如果 core scenario 保留，它不再只验证“backend 被调用”。

- [ ] **Unit 4: 修正 CI wiring，禁止空跑通过**

**Goal:** CI 明确运行真实验收，且任何 test filter 拼写错误都不能悄悄通过。

**Requirements:** R1, R2, R6

**Dependencies:** Unit 2, Unit 3

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify as needed: `scripts/ci-rust-gate.sh`
- Modify as needed: `scripts/run-tests.sh`
- Test: `crates/ralph-cli/tests/integration_agent_reference.rs`
- Test: `scripts/test-cli-doc-drift.sh`

**Approach:**
- 把 `bdd-agent-reference` job 改为运行真实 CLI integration test，或至少用准确的 Rust test 名称过滤，并开启测试数量检查。
- 对需要 filter 的 cargo test 调用，确保 filter 不匹配任何测试时会被发现。可通过单独列出测试、使用精确测试名约定，或改为运行整个 test binary 避免空过滤。
- CLI doc drift job 改为 strict 模式，并在输出中不允许“known baseline”作为成功条件。
- 保留 prompt size guard job，但让它依赖真实 drift 和真实 integration test，而不是依赖空跑 BDD。

**Patterns to follow:**
- `.github/workflows/ci.yml` 现有独立 job 风格。
- `scripts/run-tests.sh` 的“本地与 CI 共享入口”思路。

**Test scenarios:**
- Happy path: CI 本地等价命令至少运行 1 个 agent reference integration test。
- Error path: 故意传入不存在 test filter 时，验证策略能发现 0-test 状态。
- Error path: drift checker strict 失败时 CI job 失败。
- Regression: `./scripts/run-tests.sh` 仍通过，不因新增 test 二进制破坏 nextest 串行策略。

**Verification:**
- CI 配置中不再使用 `feat-ralph-cli-agent-reference-split` 这种无法匹配 Rust test 名的 filter。
- 本地运行 CI 等价命令时，测试输出显示实际执行了 agent reference 测试。

- [ ] **Unit 5: 收尾文档与回归报告**

**Goal:** 让维护者知道新的验证边界，避免未来再次用 mock-only 或 baseline 方式绕过核心成功标准。

**Requirements:** R4, R6

**Dependencies:** Unit 1-4

**Files:**
- Modify: `docs/plans/2026-06-01-002-feat-ralph-cli-agent-reference-plan.md`
- Modify as needed: `docs/report/2026-06-02-ce-executor-2026-06-01-002-feat-ralph-cli-agent-reference-plan-report.md`
- Modify as needed: `AGENTS.md`
- Modify as needed: `CLAUDE.md`

**Approach:**
- 在原计划中追加一段 post-merge correction 记录，说明 BDD/filter/baseline 问题已通过本 fix 计划处理。
- 如果实现修改了测试命令或新增脚本入口，同步更新 `AGENTS.md` 与 `CLAUDE.md`，保持两者完全一致。
- 报告中区分“功能可用”与“验收门禁修复完成”，避免把先前 mock-only gate 描述为真实端到端通过。

**Test scenarios:**
- Test expectation: none -- 文档收尾本身不新增运行行为。

**Verification:**
- `AGENTS.md` 与 `CLAUDE.md` 如有修改，内容保持完全一致。
- 原计划和报告不再声称 mock-only BDD 已证明真实 CLI 端到端行为。

## System-Wide Impact

- **Interaction graph:** 主要影响 CI、脚本验证和 CLI integration tests；不改变 event loop 注入、skill registry 或 CLI 命令业务逻辑。
- **Error propagation:** drift checker 必须在真实 drift、parser 漏解析、命令 help 抽取失败时返回非零退出码；CI 不应吞掉这些错误。
- **State lifecycle risks:** integration tests 会创建临时 `.ralph` workspace 和事件文件 marker，必须保持测试隔离，避免污染仓库 `.ralph/`。
- **API surface parity:** `ralph tools skill list/load` 的 human vs agent context 行为必须继续一致；新增测试不能为了方便绕过 `RALPH_CURRENT_HAT` fail-closed 规则。
- **Integration coverage:** 单元测试覆盖 parser；CLI integration 覆盖 binary 行为；CI job 覆盖实际门禁 wiring。
- **Unchanged invariants:** `ralph-tools.md` 仍保持精简入口；3 个详细 builtin skill 仍按需加载；`ralph-tools-tasks.md` 和 `ralph-tools-memories.md` 仍独立注入。

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| strict drift 一次暴露大量真实文档债，导致修复范围扩大 | 先修 parser/scope，再分类剩余 drift；只允许小而明确的临时 ignore list，不允许大 baseline |
| CLI integration test 依赖真实环境导致不稳定 | 使用 `TempDir`、测试专用 `.ralph` marker、最小配置，避免网络和外部服务 |
| `interact progress` 在无 Telegram 配置时行为不稳定 | 测试只断言 CLI 可解释的本地成功/失败契约；如果命令设计需要 Telegram，改测文档加载和明确错误信息，不伪造成功 |
| 修正 BDD 后发现旧 scenario harness 不适合 CLI 命令 | 将真实验收迁移到 `ralph-cli` integration test，删除或降级误导性 core scenario |
| 更新 `AGENTS.md` 忘记同步 `CLAUDE.md` | 若任一文件修改，验收中加入两文件 diff 检查 |

## Documentation / Operational Notes

- 这个 fix 完成后，原 agent reference 功能才可以被视为“验收可信”。
- CI 输出应清楚区分三类门禁：Rust 测试、CLI 文档漂移、prompt size guard。
- 后续任何修改 `ralph tools` 子命令的 PR，都应同时更新对应 `crates/ralph-core/data/ralph-tools*.md` 文件，并由 strict drift gate 捕获遗漏。

## Sources & References

- Origin requirements: `docs/brainstorms/2026-06-01-ralph-cli-agent-reference-requirements.md`
- Related plan: `docs/plans/2026-06-01-002-feat-ralph-cli-agent-reference-plan.md`
- CI workflow: `.github/workflows/ci.yml`
- Drift checker: `scripts/check-cli-doc-drift.sh`
- Help parser: `scripts/extract-cli-schema.py`
- Prompt size guard: `scripts/guard-prompt-size.sh`
- Skill integration tests: `crates/ralph-cli/tests/integration_skill.rs`
- Scenario tests: `crates/ralph-core/tests/scenarios.rs`
- Agent reference scenario: `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.yml`
- Skill CLI behavior: `crates/ralph-cli/src/skill_cli.rs`
- Builtin skill registry: `crates/ralph-core/src/skill_registry.rs`
