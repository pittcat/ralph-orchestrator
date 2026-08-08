# nowledge-mem-ralph Plugin Design

> **状态**：0.2.0 起,本插件从「永远只读、无 hooks」升级为「拥有 bounded
> save-memory lifecycle,但不保存 raw transcript」的服务于 headless Ralph
> hat 的 Claude Code 插件。本文件由 plan `2026-08-08-001` 的 U01
> bootstrap(plan §11 + inspector F1),U06 负责合并最终态。
>
> **版本**:0.2.0(U01 bootstrap)。后续 Unit 在本文件追加章节,不重写
> 已锁定章节。

## 目标

- 在首个 Ralph Claude session 触发一次有界 recall,把历史证据以
  bounded additionalContext 注入,但不复制 raw transcript。
- 任意 hat、retry、supervisor wave worker 在同一 loop 内复用同一份
  loop cache,避免反复调用 `nmem search`。
- 任意 hat 可在 activation 内通过统一的 `save-memory` 命令/技能
  提交 Memory 候选;插件校验固定 schema、来源、质量指标后再决定
  是否写入 nmem。
- 插件拥有生命周期(SessionStart + Stop,SubagentStop 由 U05 添加);
  Ralph 不持有任何 Memory 业务逻辑,也不引入新的 Memory bridge /
  finalize API。

## 非目标

- 不修改社区仓库、不修改通用 `nowledge-mem@nowledge-community` 插件。
- 不让插件读取或保存完整 Claude transcript(Stop/SubagentStop 只审计)。
- 不把 nmem client、Memory policy 或 evaluator 搬入 Rust runtime。
- 不让 Ralph 判断是否应保存 Memory;不引入 finalize-loop 或 bridge API。
- 不为非 Claude backend 提供等价生命周期。
- 不自动读取 Working Memory;recall 只查询 bounded Memory search。

## 边界

- **触发**:Claude Code lifecycle 钩子(SessionStart,Stop,后续 U05 增加
  SubagentStop)。所有触发都必须经过 `hooks/hooks.json` → `scripts/
  hook_runtime.py` 单一入口。
- **状态**:loop 状态、recall cache、accepted/rejected/unknown 记录写
  入 `CLAUDE_PLUGIN_DATA/<loop_id>/` 下,绝不修改 Ralph 事件文件、
  transcript、`~/.claude` 任何条目。
- **失败语义**:hook 必须 fail-open(exit 0 + 空 stdout),nmem 失败
  必须 fail-open(见 §5.3 状态机),evaluator 失败 = 本条 REJECTED,
  agent 继续(F4 统一语义)。
- **时间预算**:hook 端到端 ≤ 5s(`hooks/hooks.json` 的 `timeout`,
  `subprocess.run([...], timeout=5)` 双层保险)。

## 组件

| 路径 | 职责 | 引入单元 |
|---|---|---|
| `plugins/nowledge-mem-ralph/.claude-plugin/plugin.json` | 插件 manifest(版本、hooks 入口) | U01 |
| `plugins/nowledge-mem-ralph/hooks/hooks.json` | 生命周期钩子注册表 | U01(+U05 SubagentStop) |
| `plugins/nowledge-mem-ralph/scripts/__init__.py` | 包标记,锁定目录作为脚本命名空间 | U01 |
| `plugins/nowledge-mem-ralph/scripts/hook_runtime.py` | env gate + `resolve_nowledge_env` + SessionStart/Stop 入口 | U01 |
| `plugins/nowledge-mem-ralph/scripts/recall.py` | bounded search + loop cache + cache lease | U02 |
| `plugins/nowledge-mem-ralph/scripts/memory.py` | `save-memory` 入口,串联 schema/policy/writer | U03 |
| `plugins/nowledge-mem-ralph/scripts/memory_schema.py` | 固定 Memory schema + 字段约束 | U03 |
| `plugins/nowledge-mem-ralph/scripts/memory_policy.py` | 硬门槛 + 质量指标阈值 + evaluator 编排 | U03 |
| `plugins/nowledge-mem-ralph/scripts/memory_dedupe.py` | `memory_digest` 规范化(SSOT,U03/U04 共享) | U03 |
| `plugins/nowledge-mem-ralph/scripts/memory_writer.py` | 幂等 nmem 写 + argv-safe subprocess | U04 |
| `plugins/nowledge-mem-ralph/scripts/nmem_client.py` | 集中 nmem argv + 超时 + JSON 解析 | U04 |
| `plugins/nowledge-mem-ralph/scripts/memory_result.py` | 统一结果码(ACCEPTED / REJECTED / NEEDS_REWRITE / ALREADY_SAVED / FAILED_OPEN / UNKNOWN / OBSERVATION) | U04 |
| `plugins/nowledge-mem-ralph/scripts/audit_hook.py` | Stop / SubagentStop 审计 | U05 |
| `plugins/nowledge-mem-ralph/agents/memory-evaluator.md` | 结构化 JSON verdict subagent | U05 |
| `plugins/nowledge-mem-ralph/commands/save-memory.md` | agent-facing command | U03 |
| `plugins/nowledge-mem-ralph/skills/save-memory/SKILL.md` | agent-facing skill | U03 |

## 合同

- 插件 manifest 版本与本设计文档顶部版本必须一致;升级版本必须先
  升级本节。
- `hooks/hooks.json` 必须在每个 entry 上携带 `timeout: 5`,且命令
  路径通过 `${CLAUDE_PLUGIN_ROOT}/scripts/...` 解析;不允许相对路径
  或 shell 拼接。
- `scripts/hook_runtime.py` 是唯一 hook 入口;不得在 commands/skills
  中重新发起 `nmem` 写操作。
- 所有 `nmem` 调用必须通过 argv 列表(`subprocess.run([...])`),绝不
  `shell=True`。
- 所有 `nmem` 写入只能出现在 `save-memory` 的 ACCEPTED 分支;不得
  出现在 search / SessionStart / Stop / SubagentStop 任何路径。
- hook 返回 stdout 结构必须符合 Claude Code hook 协议:SessionStart
  的 `additionalContext` 是 JSON 对象、Stop 不写 stdout。

## 信任

- Memory 是历史证据,不是系统指令;recall block 必须用明确标签和
  XML escape 注入,避免被 agent 当作指令执行。
- agent 提交的 claim、evidence、verification 必须按不可信文本处理
  并限制长度;插件不接受任意路径读取 repo 外文件。
- hook stdin 的 `transcript_path`、`last_assistant_message` 只用于
  识别 session 与诊断;不读取 transcript 文件,不把最后回复当作
  Thread 内容。
- `nmem` 命令通过 argv 传递,不经过 shell 拼接;写入调用只能出现在
  `save-memory` 的 accepted 分支。
- Hooks 在 `RALPH_CURRENT_LOOP_ID` 缺失时必须 noop(exit 0,空
  stdout,无文件写入),以保证人工事 Claude session 不会被插件捕获。

## scope

- **scope = project**:Claude Code 的 `--setting-sources project,local`
  让 Ralph child 进程加载本插件,人工事 session 通过 user scope 加载
  通用插件 `nowledge-mem@nowledge-community`。两者互不干扰。
- **installer**:`scripts/setup_nowledge_ralph.py`(010 plan)负责把
  target project 从通用插件的 project scope 迁移到本专用插件;幂等、
  scope-aware,迁移前后用 `claude plugin list --json` 终检。
- **数据保留**:`--keep-data` 卸载;nmem 中的知识数据本身独立于
  插件,卸载不删除。

## installer

- 人工 CLI:`claude plugin marketplace add --scope project <repo-root>` →
  `claude plugin install nowledge-mem-ralph@ralph-orchestrator --scope
  project`。
- 推荐路径:`scripts/setup_nowledge_ralph.py <target project>`,
  先装并权威验证专用插件 → 再卸通用插件(`--keep-data`) → 用
  `claude plugin list --json` 终检。
- 失败处理:专用安装失败 → 通用插件保留;通用卸载失败 → 报告并存
  状态并给出恢复命令。

## 隐私

- 插件只发起本地/已配置的 nmem JSON 查询,且有数量上限;不上传任何
  内容。
- 不抓取 raw Claude 会话(通用插件的自动捕获路径在本插件中完全不
  存在)。
- 查询结果中的历史内容按 Nowledge Mem 自身的访问控制处理。
- hooks 不读取 transcript 文件,不读 `last_assistant_message`,不写
  Claude transcript。

## 版本

- **0.1.0**:read-only baseline。manifest 无 hooks,plugin 仅暴露
  `search` / `status` 命令与 `search-memory` skill。
- **0.2.0(U01)**:升级到带 hooks 的 lifecycle 插件骨架。SessionStart +
  Stop 已注册;`scripts/hook_runtime.py` 提供 env gate 与
  `resolve_nowledge_env`;010 契约测试按 F3 同步更新;本设计文档
  bootstrap 完成。
- **0.3.x(U02-U05)**:U02 接入 recall.py + loop cache;U03 接入
  memory schema/policy;U04 接入 writer;U05 接入 evaluator + SubagentStop
  + audit hook。每升级一档必须在本节追加版本段落,不得修改已锁定
  章节。
- **0.4.x(U06)**:e2e fixtures + 文档收尾 + 011 superseded 验证。

## 追踪

- 状态字段:`{"hook": "SessionStart|Stop|SubagentStop", "loop_id", "hat",
  "session_id", "source", "audit_only"}`。
- 文件位置:`CLAUDE_PLUGIN_DATA/<loop_id>/state.json` 与
  `CLAUDE_PLUGIN_DATA/<loop_id>/audit.jsonl`(后者 U05 引入)。
- 日志格式:`stderr` 一行 JSON,字段 `event`、`plugin`、`loop_id`
  等;不得把 transcript 内容写入 stderr。

---

## 5.3 Lifecycle 状态机

```
        ┌────────────────────────────────────────────────────────────┐
        │ Ralph 启动 headless hat / worker(activation 开始)             │
        └─────────────────┬──────────────────────────────────────────┘
                          │ SessionStart 触发(hook_runtime.py)
                          ▼
                  ┌──────────────────┐
                  │ RALPH_ENV_PRESENT? │
                  └────┬─────────┬────┘
                  否   │         │ 是
                       ▼         ▼
              ┌──────────────┐  ┌────────────────────┐
              │ NOOP         │  │ SESSION_START       │
              │ exit 0       │  │ resolve_nowledge_env │
              │ 空 stdout    │  │ recall cache 读/写   │
              │ 无文件       │  │ additionalContext 注 │
              └──────────────┘  └─────┬──────────────┘
                                       │ cache 命中 → 直接返回
                                       │ cache miss → bounded search
                                       ▼
                              ┌──────────────────────┐
                              │ RECALL_OUTCOME       │
                              │ ├─ hit   → INJECTED  │
                              │ ├─ miss  → NO_RESULT │
                              │ └─ err   → FAIL_OPEN │
                              └────────┬─────────────┘
                                       ▼
                              ┌──────────────────────┐
                              │ HAT_ACTIVATION       │
                              │ 任意 hat 可调 save-memory│
                              └─────┬────────────────┘
                                    │
                                    ▼
                       ┌─────────────────────────┐
                       │ SAVE_OUTCOME             │
                       │ ├─ ACCEPTED       → nmem│
                       │ ├─ ALREADY_SAVED  → nmem│
                       │ ├─ REJECTED       → noop│
                       │ ├─ NEEDS_REWRITE  → noop│
                       │ ├─ FAILED_OPEN    → noop│
                       │ ├─ UNKNOWN        → noop│
                       │ └─ OBSERVATION    → noop│
                       └─────────────────────────┘
                                    │
                                    ▼
                              ┌──────────────────────┐
                              │ Stop / SubagentStop    │
                              │ (U01:Stop;U05:SubagentStop) │
                              └────────┬─────────────┘
                                       │
                                       ▼
                              ┌──────────────────────┐
                              │ STOP_AUDIT            │
                              │ ├─ no save attempt →   │
                              │ │   AUDIT_NOOP         │
                              │ ├─ accepted/rejected → │
                              │ │   AUDIT_RECORDED     │
                              │ └─ agent stopped mid-  │
                              │     write →            │
                              │     UNKNOWN_RETAINED   │
                              └──────────────────────┘
```

### 状态转移矩阵(KTD 矩阵)

| From | Event | To | 副作用 | 触发者 | 失败语义 |
|---|---|---|---|---|---|
| (start) | SessionStart + no env | NOOP | 无 | hook | exit 0 + 空 stdout |
| (start) | SessionStart + env | SESSION_START | 写 state.json | hook | env gate 失败 → NOOP |
| SESSION_START | cache hit | INJECTED | 读 cache | hook | cache 读失败 → FAIL_OPEN |
| SESSION_START | cache miss + bounded search OK | INJECTED | 写 cache | hook + nmem | nmem 错 → FAIL_OPEN |
| SESSION_START | cache miss + search fail | FAIL_OPEN | stderr 警告 | hook | 不伪造 context |
| INJECTED | hat calls save-memory | SAVE_OUTCOME | 校验 + 可能写 nmem | U03-U04 | evaluator 失败 = REJECTED,agent 继续 |
| SAVE_OUTCOME | any code | STOP_AUDIT | 收尾 | agent / loop | 不抛错 |
| STOP_AUDIT | accept/reject | AUDIT_RECORDED | 追加 audit | hook | 不写 transcript |
| STOP_AUDIT | mid-write crash | UNKNOWN_RETAINED | 标记 + 诊断 | hook | U04 digest 兜底 |
| (any) | (any) | (any) | — | — | hook 永不退出非 0 给 Claude |

### 状态字段 SSOT

- `state.json` 必含字段:`{"hook": str, "loop_id": str, "hat": str,
  "session_id": str, "source": str}`。`Stop` 额外含 `"audit_only":
  true`;`SessionStart` 额外含 `"cache_status": "hit|miss|err"`(U02
  接入后)。
- `audit.jsonl`(U05 引入)每行一个 audit record:`{"timestamp":
  iso8601, "loop_id": str, "hat": str, "outcome": "AUDIT_NOOP|
  AUDIT_RECORDED|UNKNOWN_RETAINED", "memory_digest?": str,
  "verdict?": str}`。

### 状态机不变式

1. 任何状态下,hook 都不写 Claude transcript、Working Memory、nmem
   raw command。
2. 任何状态下,`nmem` 写入只能源自 `SAVE_OUTCOME.ACCEPTED`。
3. `STOP_AUDIT` 永不重新发起 `save-memory`;只能 append audit record。
4. 任何状态转移都必须在 stderr 留一行结构化日志(`event` 字段)。
5. hook 的可恢复故障不阻塞 Claude;runtime 只输出空/合法 JSON,不读取
  或修改 Ralph 终态。

## 5.4 Save-memory writer contract

- `memory.py` 的纯函数 `save()` 只负责 schema、policy 和候选 record;
  CLI boundary 才调用 `memory_writer.py`。
- writer 拒绝缺少 `ACCEPTED`、digest、scope、source 或 policy version 的
  record。nmem argv 由 `nmem_client.py` 集中构造,禁止 `shell=True`。
- `SAVED` 之后才原子更新 `memory-ledger.json`;已存在的
  `scope:digest` 返回 `ALREADY_SAVED`。非零、缺命令、超时或非法 JSON
  返回 `FAILED_OPEN`/`UNKNOWN`,不写 ledger。
- Stop/SubagentStop 只追加不含 transcript 的 `audit.jsonl`,不能触发
  writer 或 evaluator。
- 候选显式设置 `semantic_review=true` 时,`memory_evaluator.py` 通过无
  shell 的 argv 调用结构化 evaluator;缺失、超时、非法 JSON 或带副作用
  字段的返回一律拒绝本条 Memory。
