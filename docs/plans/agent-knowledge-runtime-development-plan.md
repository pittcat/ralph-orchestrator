---
title: "feat: Build agent-knowledge-runtime without Ralph source changes"
type: feat
status: active
date: 2026-04-29
created: 2026-04-28
updated: 2026-04-29
topic: agent-knowledge-runtime
origin:
  - docs/brainstorms/ralph-knowledge-curation-requirements.md
  - docs/guide/project-usage.md
---

# agent-knowledge-runtime 开发计划

## 1. 目标和边界

`agent-knowledge-runtime`，简称 AKR，是一个轻量 Python CLI。它挂在 Ralph loop 生命周期上，负责“长期运行之后的知识复用和知识沉淀”，但不接管 Ralph 的任务编排。

核心目标：

1. Ralph loop 开始前，AKR 检索 Nowledge Mem、Ralph 本地 memory、Obsidian 长期笔记，生成当前任务可用的 `.ralph/agent/knowledge-context.md/json`。
2. Ralph loop 结束后，AKR 收集 events、summary、handoff、tasks、review 输出等材料，生成知识审阅文件和 Obsidian draft。
3. 用户确认后，AKR 发布 Obsidian 正式笔记，并写入短 memory 作为下次检索索引。
4. AKR 作为外部工具独立演进；Ralph v1 只需要 hooks、项目配置和 prompt/guardrail 约定，不修改 Rust 源码。

明确不做：

- 不重新编排 preset。
- 不新增 Ralph hat。
- 不在 v1 自动发布正式 Obsidian 笔记。
- 不把 hook stdout 当作 prompt 修改通道。
- v1 不修改 Ralph 源码，不维护 Ralph fork，不实现自定义 prompt bridge。
- 暂时不融合 `ce:compound`。
- 不把 Nowledge thread/memory 当作最终长期阅读文档；它们只做证据和索引。

## 2. 总体架构

AKR 分成一个外部 CLI 和一组 Ralph 项目接入约定。核心原则是：先用 Ralph 已经存在的 hooks、`.ralph/agent/*.md` context files、memories auto-injection 和项目 guardrails 完成闭环；不要为了 v1 维护 Ralph 源码补丁。

| 工作包 | 所在 repo | 职责 |
|---|---|---|
| AKR Python CLI | 新 repo `agent-knowledge-runtime` | 检索、审阅、draft、publish、doctor |
| Ralph 零源码接入配置 | 当前 Ralph repo 或使用方项目 | 配置 hooks、保留 `.ralph/agent/knowledge-context.md`、通过 guardrail/PROMPT 要求 agent 读取相关 context file |

Ralph 继续主控执行：

```text
ralph run
  -> pre.loop.start hook
      -> akr prime
      -> 写 .ralph/agent/knowledge-context.md/json

  -> Ralph 构造 agent prompt
      -> 现有 prompt 会列出 .ralph/agent/*.md context files
      -> agent 看到 .ralph/agent/knowledge-context.md
      -> 项目 guardrail/PROMPT 要求 planning 前读取并应用它

  -> preset / hats 正常运行

  -> post.loop.complete 或 post.loop.error hook
      -> akr review
      -> 写 .ralph/agent/knowledge-review.md/json
      -> 写 Obsidian draft

  -> 用户手动确认
      -> akr publish
      -> 发布 Obsidian note
      -> 可选写入短 memory 索引
```

Ralph 已支持 hooks；`docs/guide/project-usage.md` 已补充用户级说明。当前缺口是：

1. AKR CLI repo 尚未创建。
2. 当前项目还没有一份可复用的 AKR hooks + guardrail 示例配置。
3. Ralph hook payload 对 AKR 来说还缺少更直接的 prompt/config hint，v1 可先通过 workspace 文件和 `PROMPT.md` 推断，后续再看是否推动上游增强 payload。
4. 零源码方案不是“强制全文注入”；它依赖现有 context file 列表和 agent 遵守 guardrail。若后续实测复用率不足，再把源码级 prompt bridge 作为上游增强，而不是 v1 前提。

## 3. 新 repo 设计

repo 名称：`agent-knowledge-runtime`

Python 版本：`>=3.11`

包管理：推荐 `uv`，同时保证普通 `.venv + pip` 可用。

CLI 命令名：`akr`

建议目录：

```text
agent-knowledge-runtime/
  pyproject.toml
  README.md
  docs/
    ralph-integration.md
    obsidian-vault-format.md
  examples/
    agent-knowledge-runtime.yml
    ralph-hooks.yml
    sample-hook-payload.json
  src/
    akr/
      __init__.py
      cli.py
      config.py
      models.py
      paths.py
      commands.py
      ralph.py
      nowledge.py
      obsidian.py
      search.py
      prime.py
      review.py
      publish.py
      doctor.py
      render.py
      text.py
  tests/
    fixtures/
      hook-payload-pre-loop-start.json
      hook-payload-post-loop-complete.json
      events-success.jsonl
      events-error.jsonl
      summary.md
      handoff.md
      obsidian-vault/
    test_config.py
    test_ralph_artifacts.py
    test_nowl_edge_adapter.py
    test_obsidian.py
    test_prime.py
    test_review.py
    test_publish.py
    test_cli.py
```

依赖选择：

| 依赖 | 用途 | 决策 |
|---|---|---|
| `typer` | CLI | v1 使用，命令可读性好 |
| `pydantic` | config/model validation | v1 使用，减少 schema 手写错误 |
| `pyyaml` | YAML config | v1 使用 |
| `rich` | 人类可读输出 | 可选；如果引入，CLI 输出更清楚 |
| `pytest` | 测试 | 必需 |

减少依赖的地方：

- Markdown frontmatter v1 可自实现，不强制 `python-frontmatter`。
- Obsidian v1 只写 Markdown 文件，不依赖 Obsidian 应用 CLI。
- Nowledge Mem v1 走 `nmem` CLI，不直接调用 REST API。

## 4. 配置文件

默认查找顺序：

1. `--config <path>`
2. 当前 workspace 的 `agent-knowledge-runtime.yml`
3. 当前 workspace 的 `.akr/config.yml`
4. 用户级 `~/.config/agent-knowledge-runtime/config.yml`

示例：

```yaml
version: 1

ralph:
  knowledge_context_md: ".ralph/agent/knowledge-context.md"
  knowledge_context_json: ".ralph/agent/knowledge-context.json"
  knowledge_review_md: ".ralph/agent/knowledge-review.md"
  knowledge_review_json: ".ralph/agent/knowledge-review.json"
  max_context_chars: 6000

obsidian:
  enabled: true
  vault_path: "~/Obsidian/Main"
  notes_dir: "Agent Knowledge"
  drafts_dir: "Agent Knowledge/_drafts"
  status_values: ["draft", "published", "archived"]

nowledge:
  enabled: true
  nmem_bin: "nmem"
  memory_limit: 8
  thread_limit: 5
  command_timeout_seconds: 20

retrieval:
  mode: "indexed_excerpt"
  max_memory_results: 8
  max_thread_results: 5
  max_obsidian_results: 8
  max_excerpt_chars_per_source: 1200
  min_score: 0.2

review:
  include_successful_runs: true
  include_failed_runs: true
  min_event_count: 3
  max_evidence_items: 20
  draft_title_strategy: "topic_first"

approval:
  mode: "manual"
```

配置行为：

- `obsidian.enabled: false` 时，`akr review` 仍生成 `.ralph/agent/knowledge-review.*`，但不写 vault。
- `nowledge.enabled: false` 时，只搜 Ralph memory 和 Obsidian。
- `nmem` 不可用时，AKR 降级，不让 Ralph 主 loop 失败。
- 路径统一按 hook payload 里的 `loop.workspace` 解析；用户级路径支持 `~`。

## 5. CLI 命令

### 5.1 `akr doctor`

用途：确认 AKR 能在当前项目里工作。

行为：

- 找配置文件。
- 检查 Python 包版本。
- 检查 `nmem status`，允许版本 mismatch 作为 warning。
- 检查 Obsidian vault、notes_dir、drafts_dir 是否存在或可创建。
- 检查 `.ralph/` 和 `.ralph/agent/` 是否存在。
- 检查 Ralph hook payload fixture 是否能解析。

输出：

```text
AKR doctor
config: agent-knowledge-runtime.yml
workspace: .
nmem: ok (warning: CLI/server version mismatch)
obsidian: ok
ralph files: ok
result: PASS
```

失败策略：

- 硬失败只用于配置不可解析、workspace 不可写、vault path 明确无效。
- `nmem` 不可用是 warning，因为 v1 可以靠 Obsidian 和本地文件降级。

### 5.2 `akr prime --hook-payload -`

用途：在 `pre.loop.start` 里运行，生成当前 loop 的知识上下文。

输入：

- stdin hook payload。
- 配置文件。
- 可选：`--prompt "..."` 或 `--prompt-file PROMPT.md`，用于未来手动调用。

输出文件：

```text
.ralph/agent/knowledge-context.md
.ralph/agent/knowledge-context.json
```

`knowledge-context.json` 结构：

```json
{
  "schema_version": 1,
  "loop_id": "20260428-120000",
  "workspace": ".",
  "generated_at": "2026-04-28T12:00:00Z",
  "query": {
    "text": "derived query text",
    "sources": ["hook_payload", "prompt_file", "repo_name"]
  },
  "results": [
    {
      "kind": "obsidian_note",
      "title": "Ralph hook knowledge workflow",
      "source": "Agent Knowledge/Ralph hook knowledge workflow.md",
      "reason": "Matches hook and knowledge-context terms",
      "excerpt": "..."
    }
  ]
}
```

`knowledge-context.md` 结构：

```markdown
---
schema_version: 1
loop_id: 20260428-120000
generated_at: 2026-04-28T12:00:00Z
---

# Knowledge Context

This context was selected for the current Ralph loop.

## Relevant Prior Knowledge

### Ralph hook knowledge workflow

Source: Obsidian note `Agent Knowledge/Ralph hook knowledge workflow.md`
Reason: Matches hook and knowledge-context terms.

Excerpt:
...
```

Prime 算法：

1. 解析 hook payload。
2. 确定 workspace、repo_root、loop_id。
3. 构造 query：
   - 优先读取配置传入的 prompt 文件。
   - 如果没有 prompt 文件，读取 `PROMPT.md` 前若干字符。
   - 加入 repo 名、config 文件名、preset/hook phase 线索。
4. 调用 `nmem wm read`、`nmem m search`、必要时 `nmem t search`。
5. 搜索 Obsidian notes：
   - v1 用文件名、frontmatter tags、正文关键词打分。
   - 不引入向量索引。
6. 合并和去重结果。
7. 限制上下文预算，生成 markdown 和 JSON。
8. 如果没有结果，写一个空但有效的 context 文件，避免 Ralph 读取旧文件。

### 5.3 `akr review --hook-payload -`

用途：在 `post.loop.complete` 或 `post.loop.error` 里运行，生成知识审阅和 Obsidian draft。

输入材料：

| 材料 | 路径/来源 | 用途 |
|---|---|---|
| hook payload | stdin | loop_id、workspace、termination_reason |
| current events marker | `.ralph/current-events` | 找本次 events JSONL |
| events JSONL | `.ralph/events-*.jsonl` | 识别关键事件、错误、成果 |
| summary | `.ralph/agent/summary.md` | 任务结论 |
| handoff | `.ralph/agent/handoff.md` | 继续执行上下文 |
| tasks | `.ralph/agent/tasks.jsonl` | 完成/失败任务 |
| memories | `.ralph/agent/memories.md` | 本地项目知识 |

输出文件：

```text
.ralph/agent/knowledge-review.md
.ralph/agent/knowledge-review.json
<obsidian-vault>/<drafts_dir>/<slug>.md
```

Review 判断：

只有满足至少一个条件才生成 Obsidian draft：

- loop 成功，并且出现可复用结论、稳定约束、修复路径或实验结论。
- loop 失败，但有实质排查材料、失败路径、环境约束或可复现教训。
- events 数达到 `review.min_event_count`。
- 用户显式通过配置或命令要求 review。

普通进度不生成长期 draft，只写 `knowledge-review.md` 说明“未发现值得长期沉淀的主题”。

Obsidian draft frontmatter：

```yaml
---
title: "Ralph hooks 接入长期知识运行层"
status: draft
created: 2026-04-28
updated: 2026-04-28
tags:
  - ralph
  - hooks
  - knowledge-runtime
source_runs:
  - loop_id: "20260428-120000"
    workspace: "ralph-orchestrator"
source_threads: []
source_memories: []
---
```

正文模板：

```markdown
# Ralph hooks 接入长期知识运行层

## Context

这次任务/实验/排错发生在什么背景下。

## What We Learned

这次真正值得复用的知识。

## Guidance

下次遇到类似任务时应该怎么做。

## Why It Matters

如果不遵守会造成什么问题。

## When To Reuse

- 适用条件 1
- 适用条件 2

## Evidence

- Source: `.ralph/events-...jsonl`
- Source: `.ralph/agent/summary.md`

## Related

- memory: ...
- thread: ...
```

### 5.4 `akr publish --review .ralph/agent/knowledge-review.json`

用途：人工确认后发布正式 Obsidian note。

行为：

1. 读取 review JSON。
2. 找到 draft。
3. 将 `status: draft` 改为 `published`。
4. 移动或复制到 `obsidian.notes_dir`。
5. 生成短 memory 候选：
   - 不是长文摘要。
   - 只包含触发条件、结论、Obsidian note 链接。
6. 如果 `nowledge.enabled: true`，调用 `nmem m search` 去重。
7. 如果未发现重复，调用 `nmem m add` 写入索引型 memory。

memory 文案示例：

```text
When working on Ralph lifecycle hooks or AKR context-file reuse, read the Obsidian note "Ralph hooks 接入长期知识运行层" first; it documents the hook/file/guardrail boundary and failure modes.
```

### 5.5 `akr inspect`

用途：调试输出。

子命令：

- `akr inspect context`：显示当前 `.ralph/agent/knowledge-context.json`。
- `akr inspect review`：显示当前 `.ralph/agent/knowledge-review.json`。
- `akr inspect vault-search "query"`：只搜 Obsidian，不写文件。

该命令不是 v1 必需，但建议作为开发期工具加入，因为它能降低调试成本。

## 6. Ralph 零源码接入计划

Ralph 已支持 hooks，并且现有 prompt 会列出 `.ralph/agent/` 下的 Markdown context files。v1 不新增 Ralph 配置结构，也不改 `crates/` 源码；AKR 通过“写文件 + 项目提示约定”接入。

现有依据：

- `crates/ralph-core/src/hatless_ralph.rs` 已在 prompt 中列出 `.ralph/agent/*.md` context files，并提示 agent “read if relevant”。
- `docs/concepts/memories-and-tasks.md` 说明 Ralph memories 支持自动注入；AKR 发布后的短 memory 可以作为 Obsidian note 的检索索引，而不是把整篇笔记塞进 memory。
- `docs/guide/configuration.md#hooks` 已把 hooks 定义为生命周期外部命令，适合 AKR 这种旁路自动化。

目标：

- `akr prime` 写 `.ralph/agent/knowledge-context.md/json`。
- Ralph 现有 context files 列表让 agent 能看到 `.ralph/agent/knowledge-context.md`。
- 项目级 `PROMPT.md` 或 `core.guardrails` 明确要求：planning 前如果该文件存在且与任务相关，先读取它。
- `akr prime` 每次都覆盖 context 文件；没有检索结果也写空 context，避免旧 run 内容残留。
- AKR 失败不阻塞 Ralph 主 loop，hook 默认 `on_error: warn`。

建议变更点：

| 文件 | 变更 |
|---|---|
| `docs/guide/project-usage.md` | 说明 AKR 零源码接入、context file 读取约定和限制 |
| 使用方 `ralph.yml` | 配置 `pre.loop.start` / `post.loop.complete` / `post.loop.error` hooks |
| 使用方 `ralph.yml` 或 `PROMPT.md` | 增加“读取 `.ralph/agent/knowledge-context.md`”的 guardrail |
| 新 repo `agent-knowledge-runtime` | 实现 `akr prime/review/publish/doctor` |

推荐 guardrail：

```yaml
core:
  guardrails:
    - "Before planning, if `.ralph/agent/knowledge-context.md` is listed under AVAILABLE CONTEXT FILES and is relevant to the task, read it and apply the selected prior knowledge."
```

如果项目已经使用 `PROMPT.md` 放长期指令，也可以放同一条规则。选择 `core.guardrails` 的好处是它跟 Ralph 配置放在一起；选择 `PROMPT.md` 的好处是更容易跨不同 agent CLI 复用。

限制和取舍：

- 这不是源码级 prompt mutation，不保证 context 全文一定进入每个 agent 的初始上下文。
- 它依赖 agent 看到 context file 列表后按 guardrail 读取文件。
- 通过强文件名、短 context、首屏摘要和 guardrail 可以提升执行稳定性。
- 如果实测 agent 经常漏读，再考虑向 Ralph 上游提交可选 prompt bridge；不要在 v1 维护私有补丁。

## 7. Nowledge Mem 集成

v1 只通过 `nmem` CLI。

需要封装：

```text
nmem status
nmem wm read
nmem m search
nmem m add
nmem t search
nmem t show
```

实现要求：

- 所有 `nmem` 调用都通过 `CommandRunner`。
- 每个命令有 timeout。
- stderr 和 exit code 进入 AKR diagnostic JSON。
- `nmem` 失败时，AKR 继续执行 Obsidian 和本地文件逻辑。
- `nmem` 版本 mismatch 只作为 warning。

不在 v1 做：

- 不直接调用 Nowledge REST API。
- 不自动 `nmem t distill`。
- 不自动大量保存 thread。

## 8. Obsidian 集成

v1 把 Obsidian 当作 Markdown vault 目录。

搜索策略：

1. 遍历 `obsidian.notes_dir` 下 `.md` 文件。
2. 读取 frontmatter 的 `title`、`tags`、`aliases`。
3. 读取正文前若干字符或按 heading 分段。
4. 用关键词匹配和简单评分排序。
5. 返回前 N 个摘录。

发布策略：

- draft 写入 `obsidian.drafts_dir`。
- publish 后正式笔记进入 `obsidian.notes_dir`。
- 同标题/slug 已存在时，默认不覆盖，生成 conflict review。
- 未来可以支持 merge，但 v1 只做保守行为。

文件命名：

```text
YYYY-MM-DD-topic-slug.md
```

如果同主题已存在，v1 生成：

```text
YYYY-MM-DD-topic-slug-2.md
```

并在 review JSON 里标记 `possible_duplicate_notes`。

## 9. 数据模型

核心模型：

| Model | 字段 |
|---|---|
| `HookPayload` | `schema_version`, `phase_event`, `loop`, `iteration`, `context`, `metadata` |
| `LoopIdentity` | `loop_id`, `workspace`, `repo_root`, `is_primary` |
| `KnowledgeSource` | `kind`, `title`, `source`, `score`, `reason`, `excerpt` |
| `KnowledgeContext` | `loop_id`, `generated_at`, `query`, `results`, `warnings` |
| `RunArtifactSet` | `events_path`, `summary_path`, `handoff_path`, `tasks_path`, `memories_path` |
| `KnowledgeReview` | `loop_id`, `termination_reason`, `should_create_note`, `draft_path`, `candidates`, `warnings` |
| `ObsidianDraft` | `title`, `slug`, `frontmatter`, `sections`, `source_runs` |

所有 JSON 文件必须带 `schema_version: 1`。

## 10. 实施顺序

### Phase 1：AKR repo skeleton

交付：

- `pyproject.toml`
- `src/akr/cli.py`
- `src/akr/config.py`
- `src/akr/models.py`
- `tests/test_cli.py`
- `tests/test_config.py`

验收：

- `akr --help` 可运行。
- `akr doctor` 在没有配置时给出清晰错误。
- `pytest` 通过。

### Phase 2：Ralph artifact reader

交付：

- `src/akr/ralph.py`
- `src/akr/paths.py`
- fixture：hook payload、events JSONL、summary、handoff。

验收：

- 能从 hook payload 解析 workspace 和 loop_id。
- 能从 `.ralph/current-events` 找到本次 events 文件。
- 文件缺失时返回 warning，不 panic。

### Phase 3：Obsidian reader/writer

交付：

- `src/akr/obsidian.py`
- `src/akr/search.py`
- `tests/test_obsidian.py`

验收：

- 能从 temp vault 搜索 note。
- 能生成 draft。
- frontmatter 格式稳定。
- 重名文件不覆盖。

### Phase 4：Nowledge adapter

交付：

- `src/akr/nowledge.py`
- `src/akr/commands.py`
- `tests/test_nowl_edge_adapter.py`

验收：

- mock `nmem m search` 结果能被解析。
- `nmem` 不存在时降级为 warning。
- command timeout 可测试。

### Phase 5：`akr prime`

交付：

- `src/akr/prime.py`
- `tests/test_prime.py`

验收：

- 从 hook payload + mock sources 生成 `knowledge-context.md/json`。
- 空结果也写入当前 loop_id 的空 context，避免旧文件污染。
- 输出不超过 `max_context_chars`。

### Phase 6：`akr review`

交付：

- `src/akr/review.py`
- `src/akr/render.py`
- `tests/test_review.py`

验收：

- 成功 run 可生成 review 和 draft。
- 失败 run 有证据时也生成 draft。
- 普通短 run 只生成 review，不生成长期 draft。
- draft 包含 Context、What We Learned、Guidance、Why It Matters、When To Reuse、Evidence、Related。

### Phase 7：`akr publish`

交付：

- `src/akr/publish.py`
- `tests/test_publish.py`

验收：

- draft 发布到 notes_dir。
- `status` 从 `draft` 更新为 `published`。
- 生成短 memory 候选。
- mock `nmem m search/add` 覆盖去重和写入。

### Phase 8：Ralph 零源码接入示例

交付：

- `examples/ralph-hooks.yml`：AKR hooks 示例。
- `examples/agent-knowledge-runtime.yml`：AKR 配置示例。
- `docs/ralph-integration.md`：解释 hooks、context files、guardrails、Obsidian draft 的关系。
- `docs/guide/project-usage.md`：保持用户级说明与零源码方案一致。

验收：

- `ralph hooks validate` 能理解示例 hooks 配置。
- `akr prime` 写出的 `.ralph/agent/knowledge-context.md` 能被 Ralph 现有 AVAILABLE CONTEXT FILES 列表发现。
- guardrail 明确要求 agent 在规划前读取相关 context file。
- 文档没有暗示需要修改 `crates/ralph-*` 源码。

### Phase 9：端到端 dogfood

交付：

- `examples/ralph-hooks.yml`
- `examples/agent-knowledge-runtime.yml`
- `docs/ralph-integration.md`

验收流程：

1. 在 temp workspace 中配置 hooks。
2. 手动运行 `akr prime --hook-payload tests/fixtures/hook-payload-pre-loop-start.json`。
3. 确认 `.ralph/agent/knowledge-context.md/json`。
4. 启动一次短 Ralph loop，确认 prompt 中出现 AVAILABLE CONTEXT FILES，并且 agent 能按 guardrail 读取 `knowledge-context.md`。
5. 手动运行 `akr review --hook-payload tests/fixtures/hook-payload-post-loop-complete.json`。
6. 确认 `.ralph/agent/knowledge-review.*` 和 Obsidian draft。
7. 手动运行 `akr publish`。
8. 确认正式 Obsidian note 和 memory 候选。

## 11. 测试计划

AKR repo：

```bash
uv run pytest
uv run ruff check .
uv run mypy src
```

如果不采用 `uv`：

```bash
python -m venv .venv
. .venv/bin/activate
pip install -e ".[dev]"
pytest
```

Ralph repo：

```bash
cargo run --bin ralph -- hooks validate -c examples/hooks/minimal/ralph.hooks.yml
```

由于 v1 不改 Ralph Rust 源码，Ralph 侧重点是 hooks 配置验证和一次短 loop dogfood，不需要为 prompt bridge 新增 `cargo test` 任务。

必须覆盖的测试场景：

| 场景 | 测试 |
|---|---|
| hook payload 正常 | `akr prime` 和 `akr review` 能解析 |
| hook payload 缺字段 | 给出诊断，不写错误 loop_id |
| `nmem` 不可用 | warning 降级 |
| Obsidian vault 不存在 | `doctor` fail，`review` 不写 draft |
| 无相关知识 | 生成空 context，防旧污染 |
| 长 note | 只摘录，不写大段全文 |
| 旧 context 文件 | `akr prime` 每次覆盖，空结果也写空 context |
| agent 漏读 context | guardrail 和 dogfood 记录是否需要上游 prompt bridge |
| 失败 run | 有证据则生成 draft |
| 普通短 run | 不生成长期 draft |
| publish 重名 | 不覆盖，生成冲突提示 |

## 12. 风险和处理

| 风险 | 处理 |
|---|---|
| agent 看到 context file 但没有读取 | guardrail/PROMPT 强化读取规则；dogfood 记录漏读频率；必要时后续推动上游 prompt bridge |
| 旧 context 污染新 run | `akr prime` 每次覆盖；空结果也写空 context；context 首部带 loop_id 和 generated_at |
| Obsidian 被流水账污染 | review 阈值 + manual publish |
| `nmem` 版本不一致 | warning，不阻断 |
| AKR 阻塞 Ralph loop | hooks 默认 `on_error: warn`，命令 timeout |
| 每轮执行太慢 | v1 只接 loop 前后，不接 iteration checkpoint |
| 文档生成质量不稳定 | 先生成 draft/review，不自动发布 |
| 跨 repo 路径混乱 | plan 和实现全部使用 workspace-relative paths |

## 13. 开发验收标准

v1 完成定义：

- 新 repo 能安装 `akr` CLI。
- `akr doctor` 能检查当前 workspace。
- `akr prime` 能在 Ralph hook payload 下生成 `knowledge-context.md/json`。
- Ralph 无源码改动时，现有 AVAILABLE CONTEXT FILES 能暴露 `knowledge-context.md`，项目 guardrail 能要求 agent 读取它。
- `akr review` 能生成 `knowledge-review.md/json` 和 Obsidian draft。
- `akr publish` 能人工发布 draft，并生成短 memory 候选。
- `nmem` 不可用不会影响 Ralph 主 loop。
- 文档说明用户如何在 `ralph.yml` 配 hooks。

v1 不要求：

- 自动 Obsidian 正式发布。
- 向量数据库。
- 后台服务。
- Web UI。
- `ce:compound` 集成。
- 每轮 checkpoint。

## 14. 后续增强

v1 稳定后再考虑：

- `nmem t save/import/distill` 深度集成。
- Obsidian note merge，而不是重名冲突。
- RObot/Telegram 审批发布。
- richer prompt hint：Ralph hook payload 带 `prompt_excerpt`、`config_path`、`preset_name`。
- 如果零源码方案复用率不够，向 Ralph 上游提交可选 prompt bridge，而不是维护私有源码补丁。
- 支持 Codex/Claude 非 Ralph 运行模式。
- 可选向量索引或 SQLite cache。
- `ce:compound` 作为人工 polish 的可选出口。


## 参考源码

/Users/pittcat/Dev/Rust/ralph-orchestrator


## 参考文档
/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/nowledge-mem-docs.md
