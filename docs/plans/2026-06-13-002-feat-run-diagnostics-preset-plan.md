# feat: run-diagnostics preset — 把 ce-debug 的 ad-hoc 诊断报告自动化

**Date:** 2026-06-13
**Status:** active
**Origin:** `docs/brainstorms/2026-06-13-run-diagnostics-requirements.md`
**Pilot preset:** `debug`（4-hat isolated）

---

## 问题

跑完 `ralph run` 后用户拿到 `events.jsonl` / `tasks.jsonl` / `progress.md` / `findings.md` / `fix-log.md` / `report.md` / preset YAML 一堆散落产物，要肉眼拼链路 + 归因到 preset / 机制 / agent 哪个层，效率极低。

仓库**已有**这套诊断的报告传统——见 `docs/report/2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis.md`（5 维度归因、6 段结构、P0/P1/P2 + caual chain gate），由 `compound-engineering:ce-debug` skill ad-hoc 产出。但它**有 3 个硬约束**：

1. **ad-hoc 触发**：必须 agent 主动调 `ce-debug`，跑完的 run 不自动出报告
2. **依赖 agent 直觉**：causal chain gate 质量取决于 agent 当时的上下文
3. **报告不进入 preset 反压循环**：跑出 P0 后没法自动进「3 段式 plan → preset 改 → 复跑验证」

本 plan 把这套 ad-hoc 流程**产品化**为 1 个新的 builtin preset (`diagnose-run`)，让任何 run 跑完后自动出可分享、可 commit、可喂给 preset 作者的诊断报告。

## 方案

新增 1 个 `diagnose-run` builtin preset (4-hat isolated)：

```
reconstructor (lineage graph)
    ↓ audit
auditor (5 维度证据)
    ↓ attribution
attributor (P0/P1/P2 + 4 维归因)
    ↓ write
reporter (写 docs/diagnoses/<date>-<preset>-<loop_id>.md)
    ↓
DIAGNOSE_COMPLETE
```

**触发模型**：被诊断 preset 的末位 hat（pilot: `debug` 的 `verifier`）在自己 emit 完 terminal event 后，spawn `ralph run -H builtin:diagnose-run -p "run_dir=… preset=…"` 子进程。`PROMPT.md` 顶部一段 `## DIAGNOSTICS MODE` 块给用户填开关 + 路径。**不加 CLI subcommand、不加 skill**。

**报告位置**：`docs/diagnoses/<YYYY-MM-DD>-<preset>-<loop_id>.md`（用户显式指定，与 `docs/report/`（ad-hoc ce-debug 报告）并存；ad-hoc 报告留作历史档案，自动报告走新目录，避免污染已有 git 历史）。

**作用域**：pilot 阶段只把 `debug.yml` 接上诊断。其他 builtin preset（`ce-executor-isolated` / `autoresearch` / `ce-executor-wave` / `merge-loop`）走 follow-up plan。

### 关键决策

| 决策 | 选 | 不选 | 理由 |
|---|---|---|---|
| 拓扑 | 4-hat isolated | 3-hat coordinator | 4 维度需要各自独立 LLM 上下文；coordinator 共享 prompt 会污染分析（U6 / R1 / R11） |
| 触发 | spawn 子进程 | 跨 preset 事件总线 | spawn 完全解耦两个 loop 的 `events.jsonl` / `loop.lock`；诊断 crash 不污染被诊断 run |
| 读源 | 一次性 snapshot (`cp -a`) | live read | 防止被诊断 run 的清理 / 后续 loop 改写产物（snapshot 落诊断 loop 自己的 `.ralph/diagnostics/<diag-session>/run-snapshot/`） |
| 报告位置 | `docs/diagnoses/` | `docs/report/` 或 `.ralph/diagnoses/` | 用户显式指定。`docs/diagnoses/` 是 committed、可分享、manager 可见位置；与 `docs/report/`（ad-hoc 历史）并存 |
| 用户输入 | `PROMPT.md` 顶部 `## DIAGNOSTICS MODE` 块 | CLI args / 新 skill | 用户零学习成本；preset YAML + PROMPT.md 已经是 ralph 约定 |
| 报告结构 | 固定 6 段（TL;DR / lineage / 证据 / 归因 / 修复 / 三段式 plan） | ad-hoc ce-debug 自由发挥 | 6 段是已有 `docs/report/` diagnosis 报告的归纳；保留 §3 因果链 + §4 P0/P1/P2 表 + §6 三段式 |
| 自动修复 | **不做** | 改 preset / 改基座 | 反压循环必须保留「诊断 → plan → 人决策 → 改」的人审一环 |

### 与现有 ce-debug skill 的关系

不是替代 `compound-engineering:ce-debug`，是把它的高质量产物**沉淀**为自动化 preset：

| 维度 | ce-debug skill (现状) | diagnose-run preset (本 plan) |
|---|---|---|
| 触发 | agent 主动调 | run 跑完自动 spawn |
| 报告位置 | `docs/report/...-diagnosis.md` (ad-hoc) | `docs/diagnoses/...md` (自动) |
| 拓扑 | 单 agent ad-hoc | 4-hat isolated (4 维度并发) |
| 报告结构 | ce-debug 自由发挥 (6 段是惯例但非强约束) | 强约束 6 段 + 5 维度硬清单 |
| 跨 run 趋势 | 无 | 无 (本 plan 不做；future work) |
| 复跑 | ad-hoc | 同一 `run_dir` + `preset` 可 deterministic 复跑 |

**保留**：`docs/report/` 下的历史 ad-hoc 报告不被迁移、不被重写。它们是「agent 当时是怎么分析问题的」快照，新报告走 `docs/diagnoses/`。

---

## 实施单元

### U1. 新建 `diagnose-run.yml` preset

**Goal:** 4-hat isolated 诊断 preset，是整个能力的单点真源。

**Dependencies:** 无

**Files:**
- `presets/en/diagnose-run.yml` — 新建（参考 `presets/en/debug.yml` 的 4-hat isolated 风格）

**Approach:**

1. 4 个 hat 线性串联：`reconstructor` → `auditor` → `attributor` → `reporter`，每 hat `triggers` 只有一个上游 topic
2. `event_loop` 段关键字段：
   - `execution_mode: isolated`（U6 / R1 / R11 强制；4 hat 超 3-hat coordinator 上限）
   - `completion_promise: "DIAGNOSE_COMPLETE"`
   - `required_events: [reconstruct.done, audit.done, attribution.done, report.written]`（任一缺失阻 LOOP_COMPLETE）
   - `max_iterations: 8` + `max_runtime_seconds: 1800`（30 min 诊断预算上限）
   - `starting_event: "diagnose.start"`
3. 每个 hat 的 `terminal_events` 在 `publishes` 里显式声明（U3 isolated authority 强约束）
4. **`reporter` 不能设 `default_publishes`**——其 `publishes` 已包含 `report.written` 和 `DIAGNOSE_COMPLETE`，重复声明会被 isolated authority 拒
5. `core.guardrails` 加 3 条 ABSOLUTE PROHIBITION：
   - 禁止写 `<run_dir>/.ralph/events.jsonl` / `<run_dir>/.ralph/loop.lock` / `<run_dir>/.ralph/diagnostics/`
   - 禁止 kill / signal 任何 ralph 进程
   - 禁止自动改 preset YAML 或基座代码
6. **不要写 hat instructions 的具体 prompt 文本**——那是实现期 agent 的事；plan 只指明"该 hat 应处理 X 维度、产出 Y 事件、payload 含 Z 字段"

**Test scenarios:**
- **拓扑断言**：解析后 `event_loop.execution_mode == HatExecutionMode::Isolated`
- **4 hat 完整**：4 hat 都在 `hats` 字段中，triggers/publishes/terminal_events 都设置
- **completion_promise 闭合**：DIAGNOSE_COMPLETE 在 `reporter.publishes` 中
- **required_events 非空**：4 个事件都列出
- **YAML 合法**：`serde_yaml::from_str(preset.content).is_ok()`
- **自指安全**：guardrails 含 3 条 ABSOLUTE PROHIBITION

**Verification:**
- `cargo test -p ralph-cli` 中 preset 解析相关测试全 pass
- `cargo run -p ralph-cli -- preset check diagnose-run` exit 0
- `ralph preset list` 包含 `diagnose-run`

---

### U2. 注册到 4 处源真源

**Goal:** 让 `diagnose-run` 在 `ralph preset list` / `ralph run -H builtin:diagnose-run` / zsh 补全 / CLAUDE.md builtin 列表 4 处都可被发现。

**Dependencies:** U1

**Files:**
- `presets/manifest.yml` — `embedded:` 列表加 `- diagnose-run`
- `crates/ralph-cli/src/presets.rs` — `PRESETS` 数组追加 `EmbeddedPreset { name: "diagnose-run", ..., public: true }`；同步 `test_list_presets_returns_all` 计数 4 → 5，`test_preset_names_returns_all_names` 计数 4 → 5 并加 `"diagnose-run"` 断言
- `presets/index.json` — 加 `{ "name": "diagnose-run", "description": "Post-hoc run diagnostics: 5-dimension audit + P0/P1/P2 attribution + manager-facing report", "category": "diagnostics" }`
- `presets/COLLECTION.md` — 加一行到 architecture patterns table 描述 4-hat diagnostic 拓扑
- `scripts/ralph-zsh-plugin.zsh` — `_RALPH_BUILTIN_HAT_VALUES` 和 `_RALPH_BUILTIN_HAT_DESCRIPTIONS` 加 `diagnose-run`；保留 `compadd`-based style 不改
- `CLAUDE.md` 和 `AGENTS.md` — `## Presets & Hats System` 段 builtin preset 列表加 `diagnose-run`（CLAUDE.md / AGENTS.md 同步规则）

**Approach:**

1. 按现有 4-hat isolated preset 的模式（如 `debug.yml`）追加新条目
2. `build.rs` 自动从 `manifest.yml` 读取并复制文件到 `$OUT_DIR/presets/`——只需把 `diagnose-run` 加到 manifest
3. **测试更新**：`crates/ralph-cli/src/presets.rs` 中两个 public-count 测试必须同步（4 → 5），否则 `cargo test` 必失败；这是天然的"manifest / Rust / index 三处不一致"硬门
4. zsh 更新后必须 `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` 并 `compdef -d ralph && autoload -U compinit && compinit` 验证加载

**Test scenarios:**
- **manifest ↔ Rust 一致**：`test_presets_match_manifest` pass（这条测试是 hard gate，缺一就 panic）
- **公共列表完整**：`test_list_presets_returns_all` 和 `test_preset_names_returns_all_names` pass
- **YAML 合法**：`test_preset_content_is_valid_yaml` pass
- **completion_promise 闭合**：`assert_public_preset_has_completion_path(diagnose_run)` pass
- **required_events 非空**：`assert_public_preset_has_required_events(diagnose_run)` pass
- **zsh 补全含 diagnose-run**：手动 `ralph run -H builtin:<TAB>` 验证
- **CLAUDE.md / AGENTS.md builtin 列表一致**：`diff <(grep diagnose-run CLAUDE.md) <(grep diagnose-run AGENTS.md)` 输出一致

**Verification:**
- `cargo test -p ralph-cli` 全 pass
- `cargo build` 成功
- `ralph preset list` 包含 `diagnose-run`
- zsh 补全 reload 后 `ralph run -H builtin:diagnose-run <TAB>` 工作

---

### U3. `PROMPT.md` 顶部 `## DIAGNOSTICS MODE` 块 + `debug.yml` 末位 hat spawn 指令

**Goal:** 把诊断接到 `debug` preset 的末位 hat（`verifier`），用户通过 `PROMPT.md` 控制开关。

**Dependencies:** U1, U2

**Files:**
- `presets/en/debug.yml` — `verifier` hat 的 `instructions` 末尾追加"诊断收尾 SOP"段，触发条件是 `fix.verified` 或 `fix.failed`
- `presets/en/debug.yml` (作为模板参考) 或 `docs/diagnoses/README.md`（新）— 包含 `## DIAGNOSTICS MODE` 块模板供用户复制

**Approach:**

1. `verifier` 的 instructions 末尾加：
   - 读 `PROMPT.md` 顶部 `## DIAGNOSTICS MODE` 段，解析 `诊断目标` / `对照 preset` / `启用` 三字段
   - 若 `启用: 是`（默认），spawn `"$RALPH_BIN" run -H builtin:diagnose-run -p "run_dir=<解析值> preset=debug"` 作为后台子进程
   - 若 `启用: 否`，跳过 spawn，正常 emit `fix.verified` / `fix.failed`
2. `## DIAGNOSTICS MODE` 块模板（写进 `docs/diagnoses/README.md`，让用户复制）：
   ```markdown
   ## DIAGNOSTICS MODE
   诊断目标: <run_dir>            # 默认 .ralph/，可省
   对照 preset: <preset-name>     # 必填，对照哪个 preset 的预期事件流
   报告路径: docs/diagnoses/      # 默认即可，可省
   启用: 是                        # 默认是，可填"否"关闭
   ```
3. **不要把 `## DIAGNOSTICS MODE` 块写进 `debug.yml` 自己的 instructions**——用户 `PROMPT.md` 是约定位置（`prompt_file: "PROMPT.md"`），hat 通过 auto-injected context 看到

**Test scenarios:**
- **spawn 行存在**：`presets/en/debug.yml` 的 `verifier` `instructions` 包含 `builtin:diagnose-run` 字串
- **开关分支存在**：instructions 包含 `启用: 否` 跳过路径
- **PROMPT.md 块格式**：`docs/diagnoses/README.md` 包含完整 4 字段模板
- **不污染 hat 主流程**：`verifier` 的 `fix.verified` / `fix.failed` 终端 emit 仍然在 spawn 之前（spawn 是 spawn 子进程，不是 emit 业务 topic）

**Verification:**
- 用户跑 `ralph run -H builtin:debug -p "X"` 在 `fix.verified` 后，ps 能看到子 `ralph run -H builtin:diagnose-run`
- 用户设 `启用: 否`，跑同样 prompt，ps 看不到子进程
- 报告落在 `docs/diagnoses/2026-06-13-debug-<loop_id>.md`

---

### U4. `docs/diagnoses/` 目录 + README

**Goal:** 给新报告位置一个入口和约定说明。

**Dependencies:** 无（可与 U1 并行）

**Files:**
- `docs/diagnoses/.gitkeep` — 新建
- `docs/diagnoses/README.md` — 新建，< 100 行

**Approach:**

README 含：
1. 报告命名约定：`YYYY-MM-DD-<preset>-<loop_id>.md`
2. 6 段结构名 + 5 维度清单 + 三段式 plan 形状
3. `## DIAGNOSTICS MODE` 块模板（与 U3 同源）
4. 手动复跑诊断的 CLI：`"$RALPH_BIN" run -H builtin:diagnose-run -p "run_dir=<path> preset=<name>"`
5. 与 `docs/report/`（ad-hoc ce-debug 历史）的关系：并存不互迁

**Test scenarios:**
- 目录存在
- README 包含 5 个关键术语：6 段结构、5 维度、PROMPT.md 块、loop_id、复跑命令

**Verification:**
- `ls docs/diagnoses/` 显示 `.gitkeep` 和 `README.md`
- 一个从未看过本系统的用户，按 README 能在首次 try 产出报告

---

### U5. Replay-based smoke test

**Goal:** 验证诊断端到端能跑通。用 fixture（不连 live API）确保 CI 安全。

**Dependencies:** U1, U2, U3

**Files:**
- `crates/ralph-core/tests/fixtures/diagnose-run-smoke/` — 新建，录一段已知好的 4-hat debug 风格 run（含 `fix.verified` 终态）
- `crates/ralph-cli/src/presets.rs` 测试模块 — 加 `test_diagnose_run_smoke`

**Approach:**

1. Fixture 包含：`events.jsonl`（24-30 行，4 hat 走通）、`tasks.jsonl`、`progress.md`、`findings.md`、`fix-log.md`、`loops.json`、可选 `recovery.jsonl`
2. 测试用 fixture 启动 diagnose-run 子 loop（mock backend 走 fixture 输出）
3. 断言：
   - `docs/diagnoses/<date>-debug-<loop_id>.md` 落盘
   - 报告含 6 个 H2 段（`## 1.` ~ `## 6.`）
   - §1 ≤ 200 词
   - §6 三个 horizon 各自首句以动词开头
   - fixture 的 `events.jsonl` hash 不变（诊断未污染被诊断 run）

**Test scenarios:**
- **happy fixture**：跑通，6 段结构完整
- **§1 长度**：≤ 200 词（reporter hat 的硬 guardrail 必须生效）
- **§6 形状**：每 horizon 首句以动词开头（`Edit` / `Add` / `Refactor` 等）
- **snapshot 隔离**：fixture `events.jsonl` SHA256 跑前跑后一致
- **缺 findings.md 兜底**：auditor 把 dimension C 标 `unverified`（不是 P0）

**Verification:**
- `cargo test -p ralph-cli --test public_presets test_diagnose_run_smoke` pass
- 整个 `cargo test -p ralph-cli` workspace 不回归

---

## 验证序列（U1–U5 全部落地后）

1. **单测门**：`cargo test -p ralph-cli` 全绿（覆盖 manifest 同步 + 公共列表计数 + smoke）
2. **preset lint**：`cargo run -p ralph-cli -- preset check diagnose-run` exit 0
3. **zsh 补全**：reload 后 `ralph run -H builtin:diagnose-run <TAB>` 工作
4. **端到端手动**：跑一个真实 `ralph run -H builtin:debug -p "..."`，`fix.verified` 后看到 `docs/diagnoses/<date>-debug-<loop_id>.md` 含 6 段

## 范围外（显式不做）

- 把诊断接到 `ce-executor-isolated` / `autoresearch` / `ce-executor-wave` / `merge-loop`（follow-up plan）
- 跨 run 趋势分析（"这个月 debug preset 失败率上升"）
- 自动改 preset YAML / 自动发版基座修复
- LLM-as-judge 对报告本身打分
- 新 CLI subcommand 或新 skill

## 风险与回滚

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| `test_presets_match_manifest` panic（manifest / Rust / index 三处不同步） | 高（U2 一处漏就触发） | 高（CI red） | 跑 `cargo test -p ralph-cli` 必过再 commit；测试本身就是 hard gate |
| 诊断 4-hat 自身不收敛（无 DIAGNOSE_COMPLETE） | 中 | 中 | `completion_promise` + `required_events` + `max_iterations: 8` 兜底；30 min 后强制终止 |
| 报告 §1 写成工程腔 | 高 | 中（违反 §1 manager-facing 目标） | `reporter` instructions 强 guardrail：≤ 200 词、禁术语清单、§6 首句动词开头 |
| snapshot 时被诊断 run 被改写 | 低 | 高（数据 corruption） | `cp -a` 一次性原子快照，reconstructor 记录 snapshot mtime + file count |
| `verifier` hat 重命名时 spawn 指令断链 | 中 | 中 | smoke test `test_diagnose_run_smoke` 加 `presets/en/debug.yml` 必含 `builtin:diagnose-run` 字串的 grep 断言 |
| 报告文件被错误 commit 到 .ralph/ 之外的 runtime 目录 | 低 | 中 | U4 `docs/diagnoses/.gitkeep` + README 明示是 committed 位置；不在 .ralph/ 下 |

## 文档同步清单

U2 + U3 落地后必须同步：

- `presets/en/diagnose-run.yml`（U1）
- `presets/manifest.yml`（U2）
- `crates/ralph-cli/src/presets.rs`（U2，含 2 个测试断言）
- `presets/index.json`（U2）
- `presets/COLLECTION.md`（U2）
- `scripts/ralph-zsh-plugin.zsh`（U2，需 install 到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`）
- `CLAUDE.md` 和 `AGENTS.md` 的 `## Presets & Hats System` builtin preset 列表（U2）
- `presets/en/debug.yml` 的 `verifier` instructions 末尾（U3）
- `docs/diagnoses/.gitkeep` + `docs/diagnoses/README.md`（U4）
- `crates/ralph-core/tests/fixtures/diagnose-run-smoke/`（U5）
- `crates/ralph-cli/src/presets.rs::test_diagnose_run_smoke`（U5）
