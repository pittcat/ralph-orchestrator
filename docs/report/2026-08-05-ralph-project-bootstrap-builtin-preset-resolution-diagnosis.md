---
title: ralph-project-bootstrap 解析 builtin preset 失败根因诊断报告
date: 2026-08-05
type: diagnosis
scope: ralph-project-bootstrap skill + ralph CLI builtin preset introspection
status: 根因已确认；推荐修复边界已确定（CLI + Skill 协同修复，而非 Skill-only 改写）
diagnostics_mode: LOGS_ONLY
skill_under_diagnosis: ralph-project-bootstrap
related_skill: compound-engineering:ce-debug
---

# ralph-project-bootstrap 解析 builtin preset 失败根因诊断报告

> **生成时间**: 2026-08-05
> **诊断对象**: `ralph-project-bootstrap` skill（用户传入 `builtin:parallel-forge` 后 `preset_resolution` 阶段 `builtin_source_missing`）
> **对照现象**: 上一轮 `ce-debug` 给出"编译出来的 ralph 二进制没把它打进去 — 需要重装"的结论
> **本次结论**: 该结论无证据支持。binary 实际已包含 `parallel-forge`，故障在 skill 用错了 CLI 接口。
> **本报告范围**: 仅做根因诊断与修复边界建议；未修改任何实现代码。

---

## 0. 摘要

| 维度 | 上一轮诊断结论 | 本次诊断结论 |
|---|---|---|
| 故障位置 | 用户的 `~/.cargo/bin/ralph` binary 是旧版本，未把 `parallel-forge` 嵌入 | `ralph-project-bootstrap` skill 使用 `ralph preset list/show`（模板接口）解析 builtin preset |
| 故障表象 | `cargo install --path crates/ralph-cli --locked --force` 即可恢复 | `parallel-forge` 不是 `TemplateCatalog` 中的模板，模板接口找不到 |
| 故障真实码 | `builtin_source_missing`（skill 返回） | `builtin_source_missing`（同）；`error: template 'builtin:parallel-forge' not found`（CLI 反馈） |
| 修复路径 | 重装 ralph | 增加 CLI 只读 builtin introspection 接口（推荐），或短期在 skill 内解析 `ralph init --list-presets` + `--dry-run`（不推荐） |
| 证据 | "target/ 里那些构建产物是旧版本编译的" | `ralph run -H builtin:parallel-forge --dry-run` 输出 14 个 hat；`ralph init --list-presets` 含 `parallel-forge`；`~/.cargo/bin/ralph` 与 `target/release/ralph` 文件大小、SHA-256 完全一致；`Cargo.toml:19` 即 `0.1.0`，与 `ralph --version` 报告一致 |

**根因（一句话）**: `ralph-project-bootstrap` 在 `preset_resolution` 阶段把"模板接口（`ralph preset list/show`）"误用为"运行时 builtin preset 接口"，导致 `parallel-forge` 这类运行时 builtin 永远查不到 `source == "builtin:parallel-forge"` 的模板 manifest，从而抛出 `builtin_source_missing`。二进制本身无问题。

---

## 1. 现象复现

### 1.1 触发命令

```bash
python3 /Users/pittcat/.claude/skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py \
  --cwd /Users/pittcat/Dev/Rust/ralph-e2e \
  --preset builtin:parallel-forge \
  --plan docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md \
  --json
```

### 1.2 skill 返回

```json
{
  "level": "blocked",
  "stage": "preset_resolution",
  "code": "builtin_source_missing",
  "message": "no preset manifest carries source 'builtin:parallel-forge'"
}
```

### 1.3 上一轮解读（已确认不实）

- "模板确实在源码树里。但是编译出来的 ralph 二进制没把它打进去 — 可能是 target/ 里那些构建产物是旧版本编译的，或者装在 ~/.cargo/bin 那个二进制是从别的 commit 编的。"
- 建议重装：`cargo install --path crates/ralph-cli --locked --force`。

**结论**：这条解释在本环境中没有可验证的事实支撑。

---

## 2. 二进制实情（无版本问题）

### 2.1 已验证

| 检查 | 结果 |
|---|---|
| `which ralph` | `/Users/pittcat/.cargo/bin/ralph` |
| `ralph --version` | `ralph 0.1.0` |
| `cargo install --list` 中 `ralph-cli` | `v0.1.0` 装在 `crates/ralph-cli` |
| 仓库 `Cargo.toml:19` workspace version | `0.1.0`（与 `ralph --version` 一致） |
| `~/.cargo/bin/ralph` 与 `target/release/ralph` SHA-256 | **完全相同** `959bc703ed3aec38d92aa563a0cc0ae359e105360129642c1b8b8409482fc960` |
| 二进制文件大小 | 双方均 `35916352` 字节 |
| mtime | 双方均 `Aug  5 09:13:44 2026` |
| `cmp -s` 返回值 | `0`（一致） |

### 2.2 运行时识别 `parallel-forge` 的硬证据

```text
$ ralph run -H builtin:parallel-forge --dry-run \
    --plan docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md
Dry run mode - configuration:
  Hats: guardian, executor, reviewer, integrator, verifier, wave-fixer, tester,
        planner, worktree, forge-dispatcher, forge-failure-handler, auditor,
        inspector, reporter
  Prompt file: docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md
  Completion promise: LOOP_COMPLETE
  Max iterations: 100
  Max runtime: 14400s
  Scratchpad: .ralph/agent/scratchpad.md (enabled: true)
  Specs dir: .ralph/specs/
  Backend: claude
```

并行验证（同一 binary）：

```text
$ ralph init --list-presets
Available hat collections:
  ...
  parallel-forge            Parallel Forge: Spec-First planning, supervisor-driven
                            parallel Unit TDD in worktrees, serial integration with
                            linear commits, full regression, audit, and manager report
  ...
```

**结论**：`parallel-forge` 既能作为 `ralph run -H` 的运行时 builtin 被加载（解析到 14 个 hat 全部出现），也能被 `ralph init --list-presets` 列出。binary 不存在嵌入缺失。

---

## 3. 完整因果链

### 3.1 Ralph CLI 的两套 "preset" 概念

项目里刻意维护了两套不同的 "preset" 数据源：

| 系统 | 用途 | 实现位置 | 公开 CLI |
|---|---|---|---|
| **模板目录**（`TemplateCatalog`） | 生成本地 preset 的脚手架 | `crates/ralph-cli/src/preset_templates.rs:1051-1057` | `ralph preset list/show/new/...` |
| **运行时 builtin preset** | 实际可由 `ralph run -H builtin:<id>` 加载 | `crates/ralph-cli/src/presets.rs:23-159` | `ralph init --list-presets`、`ralph run -H builtin:<id>` |

证据 1（模板目录的固定列表）：

```text
crates/ralph-cli/src/preset_templates.rs:1055-1057
pub fn template_names() -> Vec<&'static str> {
    vec!["minimal-linear", "debug", "ce-executor-lite"]
}
```

证据 2（运行时 builtin 数组，含 `parallel-forge`）：

```text
crates/ralph-cli/src/presets.rs:23-118
const PRESETS: &[EmbeddedPreset] = &[
    EmbeddedPreset { name: "autoresearch", ... },
    EmbeddedPreset { name: "ce-executor-pipeline", ... },
    EmbeddedPreset { name: "ce-executor-pipeline-loop", ... },
    EmbeddedPreset { name: "ce-executor-supervisor", ... },
    EmbeddedPreset { name: "debug", ... },
    EmbeddedPreset { name: "merge-loop", ..., public: false },
    EmbeddedPreset { name: "merge-batch", ... },
    EmbeddedPreset { name: "post-merge-converge", ... },
    EmbeddedPreset { name: "parallel-forge", ... },
    EmbeddedPreset { name: "implementation-review", ... },
    EmbeddedPreset { name: "red-team-attack", ... },
];
```

证据 3（两个接口的 CLI 入口）：

```text
crates/ralph-cli/src/commands/preset.rs:188
Some(PresetCommands::List { format }) => list_templates(format, use_colors),
crates/ralph-cli/src/commands/preset.rs:189
Some(PresetCommands::Show { name, format }) => show_template(&name, format, use_colors),
crates/ralph-cli/src/init.rs:35-36
if args.list_presets {
    println!("{}", crate::init::format_preset_list());
}
```

### 3.2 模板接口的查询结果

```text
$ ralph preset list
Available workflow templates:
  minimal-linear
  debug
  ce-executor-lite
```

`parallel-forge` 不在模板接口中。模板接口中只有 `ce-executor-lite`（其 `source` 字段为 `builtin:ce-executor-pipeline`，但这只是一种模板→运行时 builtin 的元数据关联，不构成 `ralph preset list` 是"运行时 builtin 列表"的证据）。

### 3.3 Bootstrap 的解析契约

`SKILL.md:82-87` 规定：

> `builtin:<id>` is resolved via `ralph preset list --format json` (find the manifest whose `source` equals `builtin:<id>`) then `ralph preset show <template-name> --format yaml`. `preset show` addresses **template names**, which may differ from the builtin hats id; never assume stripping `builtin:` yields a template name.

实现位于 `scripts/bootstrap_pipeline.py:360-448`：

```text
def _resolve_builtin_preset(*, builtin_id, binary, runner) -> ResolvedPreset:
    """Resolve ``builtin:<id>`` via ``preset list`` → ``preset show``."""
    list_proc = runner([binary, "preset", "list", "--format", "json"], ...)
    ...
    template_name = None
    for entry in manifests:
        if entry.get("source") == builtin_id:
            candidate = entry.get("name")
            if isinstance(candidate, str) and candidate.strip():
                template_name = candidate
                break
    if template_name is None:
        raise ValueError(
            ("builtin_source_missing",
             f"no preset manifest carries source {builtin_id!r}"))
```

**故障就在这里**：当 `builtin_id = "builtin:parallel-forge"` 时，循环遍历 `ralph preset list --format json` 返回的三个模板（`minimal-linear`、`debug`、`ce-executor-lite`），没有一个 `source` 字段等于 `builtin:parallel-forge`，于是抛出 `builtin_source_missing`。

### 3.4 完整触发链

```
用户传入 preset = builtin:parallel-forge
  ↓
scripts/bootstrap_pipeline.py:_resolve_preset (l.451-468)
  ↓
scripts/bootstrap_pipeline.py:_resolve_builtin_preset (l.360-448)
  ↓
subprocess: ralph preset list --format json
  ↓ 返回 3 个模板 manifest (TemplateCatalog)
  ↓
循环查找 entry["source"] == "builtin:parallel-forge"
  ↓ 命中 0 个
  ↓
raise ValueError("builtin_source_missing", "no preset manifest carries source 'builtin:parallel-forge'")
  ↓
_run_pipeline (l.1112-1118) → _make_blocker(stage=preset_resolution, code=builtin_source_missing)
  ↓
PipelineResult{"level": "blocked", "stage": "preset_resolution", "code": "builtin_source_missing"}
  ↓
主流程退出码 2
```

### 3.5 上一轮为何误诊

`ce-debug` 收到 `builtin_source_missing` 之后，没有走自己的 `1.2 Verify environment sanity`（SKILL.md:78-87）：

- ❌ 没有 `which ralph` / `ralph --version` / `ls -la <binary_path>` 实际输出；
- ❌ 没有 `git -C <source> rev-parse HEAD` 对照二进制来源；
- ❌ 没有 `ralph run -H builtin:parallel-forge --dry-run` 直接验证 builtin 是否能加载；
- ❌ 把 skill 的 `code` 字符串作为"症状"，把"`parallel-forge` 不在 `ralph preset list` 里"作为"环境问题"，反向假设 binary 旧。

这是 `ce-debug` 的过程违规，不是 skill 设计缺陷。`ce-debug` 自身规则（Phase 1.2 环境核查 + Phase 2 因果链门禁）足以防止这类误诊，但执行时跳过了。

---

## 4. 当前 skill 行为与边界

### 4.1 skill 实际行为（审计后）

| 阶段 | 行为 | 失败模式 |
|---|---|---|
| audit | 校验目标 root、输入路径 | 输出 typed blocker（`root_ambiguous` 等） |
| preset_resolution | `_resolve_preset` → file 路径读 YAML / `builtin:<id>` 走模板接口 | **本报告根因** |
| generation + post-write verify | 写 `ralph.<stem>.yml` / `PROMPT.<stem>.md` / 文档块，原子写 + 重读校验 | 文档块冲突（marker blocker） |
| static validation | capability → `preset check --strict` → `preflight --strict` → `run --dry-run`（全四阶段） | 任一阶段非零返回 `blocked_cli/preset/backend/command` |
| smoke | 仅当 preset backend = `content_fixed_replay` + `--replay-transcript` 时 | 不授权则 `not_authorized` |
| handoff | 输出 typed level + 启动命令 + 报告 | 不向 backend spawn |

证据：

- `scripts/bootstrap_pipeline.py:1144-1230`：阶段顺序与生成后重读；
- `scripts/cli_probe.py:268-366`：`probe_capability` 只读，从不抛异常；
- `scripts/bootstrap_pipeline.py:1080-1130`：audit 失败即返回 blocker；preset_resolution 失败即返回 blocker；后续阶段跳过；
- `references/validation.md:42-49`：blocker 分类（`blocked_cli/preset/backend/command/unknown`）。

### 4.2 skill 设计边界（已确认合理）

- **不自动 install/upgrade**。`scripts/cli_probe.py:268-366` 注释明示 "The probe NEVER throws. ... version='missing' and every required flag in flags_missing"。`scripts/smoke_runner.py:562-577` 双重门禁（`backend.is_trusted` + `RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND=1`）。Skill 自身不替代环境准备。
- **不修改 `crates/ralph-cli/` 或 `presets/`**。SKILL.md:151-152 guardrails 明确禁止。修复预设或 CLI 属于 `ralph-preset-author` / `ralph-preset-review` 域。
- **不静默 spawn**。smoke 必须显式授权。
- **argv 必须 `-c ralph.<stem>.yml -H <preset>`**。`scripts/cli_probe.py:398-422` 强制，避免 `ralph.yml` / `RALPH_CONFIG` 抢占。

这些边界本身没有问题。**唯一的事实性错误**在 `scripts/bootstrap_pipeline.py:360-448` 的 builtin 解析路径。

---

## 5. 修复边界（推荐 vs 不推荐）

### 5.1 推荐：CLI + Skill 协同修复

#### 5.1.1 在 Ralph CLI 增加只读 builtin introspection

最小且与现有架构一致的设计：

```bash
ralph preset builtin list --format json
ralph preset builtin show <id> --format yaml
```

依据：
- 现有 `ralph init --list-presets` 已经能列出 builtin，但格式是人类文本，无法机器化；
- 现有 `ralph run -H builtin:<id> --dry-run` 可以验证 builtin 可加载，但不输出完整 YAML；
- 运行时 builtin 数组已在 `crates/ralph-cli/src/presets.rs:23-118` 与 `pub fn preset_names()` / `list_presets()`（l.140-159）；
- `crates/ralph-cli/src/init.rs:200-212` 已有 `format_preset_list`（人类输出），可平移到 `format_builtin_preset_list_json`。

**实现要点**：
- `list` 的 JSON 输出：每项 `{name, description, public, source: "builtin:<id>"}`；
- `show` 的 YAML 输出：直接 dump `EmbeddedPreset.content`（已经被 `include_str!` 嵌入）；

  - 注意：与现有 `ralph preset show` 的 `TemplateManifest` 区分；新接口用全新子命令 `preset builtin`，避免重名歧义。

- 受 `R-SW-1` / `R-SW-2` 风格 lint：命令名稳定、format 字段稳定、error code 稳定（`builtin_list_failed` / `builtin_show_failed` / `builtin_show_empty` / `builtin_id_unknown`）。

- 写测试：`crates/ralph-cli/src/commands/preset.rs` 新增 `tests/builtin_*` 段，覆盖 `parallel-forge` / `merge-loop`（`public=false`） / 不存在 id。

#### 5.1.2 修改 Bootstrap 使用新接口

`scripts/bootstrap_pipeline.py:360-448` 的 `_resolve_builtin_preset` 改为：

```text
list_proc = runner([binary, "preset", "builtin", "list", "--format", "json"], ...)
if returncode != 0 → builtin_list_failed
解析 → 找 entry["source"] == builtin_id
找不到 → builtin_source_missing
show_proc = runner([binary, "preset", "builtin", "show", template_name, "--format", "yaml"], ...)
```

#### 5.1.3 测试与 fixture

- `fixtures/cli/builtin-source-missing/01-builtin-list.json` / `02-builtin-list-stderr.txt`：模板仅含 `parallel-forge` / `merge-loop` 的输出，期望 `builtin_source_missing`；
- `fixtures/cli/builtin-show-empty/`：`parallel-forge` 返回空 body 期望 `builtin_show_empty`；
- `scripts/bootstrap_pipeline.py` 单测用 `runner` 注入，断言新契约；保留旧 `ralph preset list/show` 路径走 file preset 解析（不能动）。

#### 5.1.4 涉及的文件

| 文件 | 改动 |
|---|---|
| `crates/ralph-cli/src/commands/preset.rs` | 新增 `PresetCommands::BuiltinList/Show` + `format_builtin_preset_list_json` + `format_builtin_preset_show_yaml` + 单元测试 |
| `crates/ralph-cli/src/presets.rs` | 可能增加 `pub fn get_preset_yaml(name) -> Option<&'static str>` 复用现有 `EmbeddedPreset.content` |
| `crates/ralph-cli/src/preset_templates.rs` | 不动 |
| `presets/index.json` / `presets/manifest.yml` | 不动 |
| `crates/ralph-cli/build.rs` | 不动（`include_str!` 已就位） |
| `ralph-project-bootstrap/SKILL.md` | 更新 stage 2 描述与命令清单 |
| `ralph-project-bootstrap/scripts/bootstrap_pipeline.py` | `_resolve_builtin_preset` 切到新命令 |
| `ralph-project-bootstrap/fixtures/cli/*` | 增加 builtin 解析契约 fixture |
| `crates/ralph-core/data/ralph-tools.md` | 增加 `preset builtin list/show` 章节 |
| `scripts/ralph-zsh-plugin.zsh` | zsh 补全 `builtin:*` 同步更新 |

### 5.2 不推荐：仅改 skill（短期绕行）

可临时解析：

```bash
ralph init --list-presets
ralph run -H builtin:<id> --dry-run
```

**问题**：

1. `init --list-presets` 是人类文本，机器化解析脆弱；
2. `--dry-run` 不输出完整 preset YAML，无法获取 `cli.backend` / `event_loop.max_iterations` / `event_loop.max_runtime_seconds` / `event_loop.prompt`（bootstrap 还需要它们生成 suite，见 `scripts/pipeline_suite.py:731-794`）；
3. preset 内容变化检测依赖 `input_signature`（l.769-781），没有 YAML 文本就退化为无 provenance 的 hash；
4. 这种"按症状临时绕"很容易再次漂移。

### 5.3 `ce-debug` 是否要改

不需要。

- 上一轮误诊是执行时跳过了 `ce-debug` 自己的 `Phase 1.2` 环境核查与 `Phase 2` 因果链门禁；
- SKILL.md:78-87 与 SKILL.md:148-154 已有强约束，足以防止这类结论；
- 修改 `ce-debug` 规则无法补偿执行时跳过规则的行为；行为训练属于别处。

---

## 6. 现有对照与回归

### 6.1 当前 builtin → 模板 → 是否可工作

| builtin id | `TemplateCatalog` 中是否出现 | 模板接口能否解析 | 运行时可用 | 之前 bootstrap 行为 |
|---|---|---|---|---|
| `ce-executor-pipeline` | 通过 `ce-executor-lite` 模板（`source=builtin:ce-executor-pipeline`） | ✅ 命中 | ✅ | 表面成功，**实际拿到的是 `ce-executor-lite` 模板 YAML**，不是 `ce-executor-pipeline` 运行时 YAML |
| `parallel-forge` | ❌ | ❌ | ✅ | `builtin_source_missing`（本报告对象） |
| `ce-executor-supervisor` | ❌ | ❌ | ✅ | `builtin_source_missing` |
| `merge-batch` | ❌ | ❌ | ✅ | `builtin_source_missing` |
| `post-merge-converge` | ❌ | ❌ | ✅ | `builtin_source_missing` |
| `implementation-review` | ❌ | ❌ | ✅ | `builtin_source_missing` |
| `red-team-attack` | ❌ | ❌ | ✅ | `builtin_source_missing` |

**隐性风险**：`ce-executor-pipeline` 即使 `preset list` 能命中，bootstrap 拿到的 `preset show ce-executor-lite --format yaml` 是**模板脚手架**（带 `{{preset_name}}` / `{{description}}` / `{{generated_at}}` 占位符），经过 `pipeline_suite.extract_inline_preset_prompt`（`scripts/pipeline_suite.py:217-236`）必失败抛 `preset_prompt_missing`。所以"之前看似能工作"实际上一直被 blocker 命中，只是用户没注意。

### 6.2 回归测试增量

最小回归集合（建议落在 `scripts/` 单测 + `fixtures/` 双测）：

- `test_resolve_builtin_returns_known_preset`：fake runner 返回包含 `parallel-forge` 的 builtin list + 完整 YAML → 解析成功，YAML 透传；
- `test_resolve_builtin_missing_source`：fake runner builtin list 不含目标 → `builtin_source_missing`；
- `test_resolve_builtin_show_empty`：fake runner builtin list 命中、show 返回空 body → `builtin_show_empty`；
- `test_resolve_builtin_show_failed`：fake runner show 返回非零 → `builtin_show_failed`；
- `test_resolve_builtin_list_unparseable`：fake runner list stdout 不是 JSON → `builtin_list_unparseable`；
- `test_resolve_builtin_list_failed`：fake runner list 返回非零 → `builtin_list_failed`。

这些 fixture 与 `tests/contract/` 已有的 preset check / preflight / dry-run 模式一致。

---

## 7. 不变量与对外契约

修复后 bootstrap 应保持：

1. `presets/` 与 `crates/ralph-cli/` source 仍由 skill guardrail 保护，**不修改**（SKILL.md:151-152）；
2. argv 仍强制 `-c ralph.<stem>.yml -H <preset>`（`scripts/cli_probe.py:398-422`）；
3. dry-run 与 preflight 的四阶段顺序保持（`references/validation.md:13-30`）；
4. handoff 三级（`complete` / `incomplete_static_only` / `blocked`）不变；
5. smoke 仍只对 `content_fixed_replay` + `--replay-transcript` 授权。

修复应新增的不变量：

6. builtin 解析路径必须以**运行时 builtin 数据源**为单一事实源，不再借道 `TemplateCatalog`；
7. CLI 应提供稳定的 builtin list/show JSON+YAML 接口（参考第 5.1.1 节点），与现有 `ralph preset list/show` 不重名；
8. 任何对内置 builtin 的解析/变更检测必须基于完整 YAML 文本（保证 provenance 稳定）。

---

## 8. 推荐方案 vs 备选

| 方案 | 修复边界 | 风险 | 维护成本 | 推荐度 |
|---|---|---|---|---|
| A. CLI + Skill 协同（5.1） | 中 | 中 | 中 | **推荐** |
| B. 仅 Skill 改写（5.2） | 小 | 大（无 YAML 文本，provenance 失稳） | 高（长期漂移） | 不推荐 |
| C. 仅 CLI 改写（5.1.1） | 小 | bootstrap 仍坏 | 高 | 不推荐 |
| D. 不动 | 0 | 当前故障持续 | 0 | 不接受 |

**理由**：A 给两套数据源分别建立只读 introspection，避免 skill 重复踩"借错接口"的坑；同时给未来所有 builtin 解析需求（除 bootstrap 外可能的 operator、其它 skill）留下稳定 API；测试与 fixture 一次性补齐，回归有保障。

---

## 9. 结论

- ✅ `parallel-forge` 已经被正确嵌入当前 binary（`crates/ralph-cli/src/presets.rs:90-93` + `include_str!`），运行时加载与 dry-run 全部通过。
- ❌ `ralph-project-bootstrap` 在 `preset_resolution` 阶段使用 `ralph preset list/show` 解析 `builtin:<id>`，与运行时 builtin 解析无关，必然在 `parallel-forge` 等非模板 builtin 上失败。
- ❌ 上一轮 "binary 旧 / 需要 `cargo install --force`" 的诊断没有事实支撑。
- ✅ 推荐修复：在 CLI 增加 `ralph preset builtin list/show` 只读 introspection，将 skill 切到新接口，并补完整 fixture。
- ✅ `ce-debug` 自身规则足以防止这类误诊；不需要改。

> **附**：本报告严格只做诊断与修复边界建议；未触发任何修改、构建、安装或 git 操作。
