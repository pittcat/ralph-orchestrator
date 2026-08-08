# nowledge-mem-ralph — Ralph 项目专用 Nowledge Mem 插件(0.2.0 生命周期版)

本插件是 Ralph 项目环境专用的 Claude Code 插件,提供**有界的 loop-aware
recall** 与**有界的 save-memory lifecycle**:

- SessionStart 钩子在首个 Ralph session 触发一次 bounded memory search,
  后续 session/compact/retry/supervisor worker 复用同一份 loop cache。
- Stop/SubagentStop 钩子从 `last_assistant_message` 中只读取一个 bounded
  `<!-- nowledge-memory-finalize ... -->` 标记(必须显式带
  `finalize:true`),通过 policy/writer 后写入 nmem;无标记 / 非法标记 /
  非 `finalize:true` 时仍保持 audit-only,不发起保存、不读取 transcript。
- 任意 hat 可通过 `/nowledge-mem-ralph:save-memory <memory-json>`在
  activation 内提交 Memory 候选;插件校验固定 schema、质量指标和去重
  ledger 后,只有 `ACCEPTED` 分支才调用 argv-safe writer 写入 nmem。

**不**抓取 raw Claude 会话,**不**读取 Working Memory,**不**写入
Claude transcript。设计与边界详见
`.ralph/specs/nowledge-mem-ralph-plugin-design.md`(本仓库内)。

## 插件选型

| 场景 | 应使用的插件 | scope | 说明 |
|---|---|---|---|
| 人工交互 Claude Code 会话 | 通用插件 `nowledge-mem@nowledge-community` | `user` | 保留会话自动捕获等完整能力 |
| Ralph 启动的 Claude Code child(target project) | 本插件 `nowledge-mem-ralph@ralph-orchestrator` | `project` | lifecycle 钩子 + bounded recall/save-memory,无 transcript 写入 |

Ralph 的 Claude adapter 只加载 `project,local` setting sources,不加载 user
scope;因此 user 级通用插件不会被 Ralph child 看见,两者互不干扰。

## 前置条件

- 已安装 `claude` CLI（安装与 scope 操作必须）。
- 已安装 `nmem` CLI 且 Nowledge Mem 服务可达（仅执行 search/status 查询时需要；
  安装插件本身不需要）。

## 安装

### 推荐：迁移脚本（scope-aware，幂等）

本仓库提供 `scripts/setup_nowledge_ralph.py`，把 target project 从通用插件的
project scope 迁移到本专用插件（user scope 与其他项目绝不触碰）：

```bash
# 预演：只读取当前 plugin 状态并打印拟执行动作，不做任何变更
python3 <repo-root>/scripts/setup_nowledge_ralph.py <target project> --dry-run

# 实际迁移
python3 <repo-root>/scripts/setup_nowledge_ralph.py <target project>
```

脚本行为合同：

- 先安装并**权威验证** dedicated project 插件，成功后才卸载 project 通用插件
  （`--keep-data` 保留数据），最后用 `claude plugin list --json` 终检。
- 判定精确：完整插件 id + `scope=project` + canonical `projectPath` 等于
  target root；其他项目的同名条目不会被迁移。
- 幂等：已完成迁移后重跑只读取状态，不重复 install/uninstall，exit 0。
- `marketplace add` 非零（如已声明）仅作警告，由后续 install 与终检裁决。
- **local scope 检测（仅警告）**：Ralph 的 Claude child 加载 `project,local`
  两个 settings source。若 target 存在 `local` scope 的通用插件，其会话自动
  捕获在迁移后仍会对 Ralph child 生效。脚本发现时会打印警告和手动卸载命令
  （`claude plugin uninstall nowledge-mem@nowledge-community --scope local`），
  但**不会**自动移除——local 属项目内个人配置，超出迁移合同。

**安装失败处理**：

- dedicated 安装或安装后验证失败 → 脚本非零退出，project 通用插件**保留**；
  修复原因（如 marketplace 路径、网络）后直接重跑脚本。
- project 通用插件卸载失败 → 脚本非零退出并明确报告两个 project 插件并存；
  按输出中的恢复命令手动卸载，或直接重跑脚本收敛。
- 初始 plugin 状态无法解析（非法 JSON / 缺字段）→ 非零退出，不执行任何卸载。

### 手动安装（project scope，local marketplace）

在 **target project 目录**下执行（`<repo-root>` 为 ralph-orchestrator 仓库的
绝对路径）：

```bash
claude plugin marketplace add --scope project <repo-root>
claude plugin install nowledge-mem-ralph@ralph-orchestrator --scope project
```

说明：

- `marketplace add` 在该 project 已声明过本 marketplace 时可能返回非零，这是
  可恢复的警告，可继续执行 install。
- 安装只写入 target project 的 project scope；不会创建或改动任何 user scope
  条目，也不会触碰其他 project。

## 验证

在 target project 目录下：

```bash
claude plugin list --json
```

确认存在一条满足以下全部条件的条目：

- `id` 为 `nowledge-mem-ralph@ralph-orchestrator`
- `scope` 为 `project`
- `projectPath` 等于 target project 的绝对路径

## 使用

- `/nowledge-mem-ralph:search <query>` — 有界 memory search（最多 5 条，JSON）。
  空查询会显示用法并停止，不调用 nmem。
- `/nowledge-mem-ralph:status` — 只执行一次 `nmem --json status`；失败时原样
  报告错误并停止。
- `/nowledge-mem-ralph:save-memory <memory-json>` — 提交 Memory 候选。
  插件固定 schema + 硬门槛 + 七项指标后返回 verdict
  （`ACCEPTED`/`REJECTED`/`NEEDS_REWRITE`/`OBSERVATION`）。ACCEPTED
  进入 writer；成功返回 `SAVED`，相同 `scope + memory_digest` 返回
  `ALREADY_SAVED`，nmem 故障返回 `FAILED_OPEN`/`UNKNOWN`，不阻塞 Ralph。
- `search-memory` skill — 供 agent 在确有需要时主动做同样的有界只读查询。
- `save-memory` skill — 供 agent 在发现稳定、可复用的结论时主动
  提交 Memory 候选；固定 schema 与质量指标是 hard gate，agent 不可降级。
- 仅当确需追溯原对话且 memory 结果不足时，才允许有界的
  `nmem --json t search` / `nmem --json t show`（每次最多 8 条消息、每条最多
  1200 字符，按需翻页）。

## 无自动捕获保证

- hooks/hooks.json 当前注册 `SessionStart`、`Stop` 和 `SubagentStop`。
  三条钩子都不读取 raw transcript、不读
  `transcript_path`,Stop/SubagentStop 只读取 `last_assistant_message`
  中的单个 bounded `<!-- nowledge-memory-finalize ... -->` 标记;
  不抓取会话内容,无 marker 时 Stop 只追加 audit record。
- 通用插件的「整段会话自动捕获」路径在本插件中完全不存在;本插件的
  写操作只能由 agent 在 activation 内显式调用 `save-memory`,或由
  Stop/SubagentStop 在最终消息中显式嵌入 `finalize:true` 的 marker,
  且必须通过固定 schema + 硬门槛 + 质量指标。
- 会话内容的保存与蒸馏由 Ralph 自己的 curation 流程负责(见适配计划),
  不属于本插件。

## Lifecycle contract(0.2.0)

| 事件 | 触发 | 行为 | 失败 |
|---|---|---|---|
| SessionStart | 任何 Claude session 启动 | env gate → bounded recall → bounded additionalContext(`<knowledge-context historical-evidence="untrusted">`);loop cache miss 写 `recall.json`,hit 直接返回;source=`compact` 跳过 search | 缺 RALPH env = noop;nmem 错 = fail-open,空 additionalContext |
| Stop | session/worker 结束 | bounded auto-finalization:从 `last_assistant_message` 提取最多一个 `<!-- nowledge-memory-finalize ... -->` 标记(必须 `finalize:true`);通过现有 policy/writer 后写入 nmem;无 marker 或非 `finalize:true` 时保持 audit-only,不读 transcript | 永不抛错;非法 marker = REJECTED;nmem 失败 = FAILED_OPEN/UNKNOWN,hook exit 仍为 0 |
| SubagentStop | worker session 结束 | 与 Stop 同语义,走同一 bounded auto-finalization 链路 | 同上 |

`recall` 的细节(0.2.0):`scripts/recall.py` 在首个 SessionStart 拿
`flock` lease 后发起一次 `nmem --json m search <query> --limit 5`
(`query` 仅由 repo basename + preset + objective + plan 派生,绝不读
transcript / last_assistant_message),把渲染好的 XML(转义了 `<>&`、
控制字符剥除、按 UTF-8 字节边界截断到 4KB)原子落盘
`recall.json`;后续同 loop 的 SessionStart(普通 hat、compact、retry、
supervisor worker)直接命中 cache,`nmem` 计数=0。`source=compact`
无论 cache 是否命中都不调 search。

`save-memory` 入口(U03 引入)由 `scripts/memory.py` 提供,固定 schema
+ 硬门槛 + 七项质量指标 + dedupe signal:

- **固定 schema**(`MemorySchema.REQUIRED_FIELDS`):
  `memory_type` / `title` / `claim` / `why_it_matters` / `evidence` /
  `applies_when` / `scope` / `verification` / `critical_assumptions` /
  `critical_ambiguities` / `metrics`。字段缺失一律 `REJECTED`。
- **硬门槛**: `memory_type ∈ {progress, log, command, transcript}`
  一律 `REJECTED`(原始过程状态不进 Memory)。
- **七项质量指标**(`MemorySchema.REQUIRED_METRICS`):
  `confidence ≥ 80` / `evidence_coverage ≥ 70` / `reusability ≥ 50` /
  `verifiability ≥ 50` / `novelty ≥ 20` / `stability ≥ 60` /
  `scope_clarity ≥ 70`。
- **反幻觉**: `confidence ≥ 90` 且 `evidence_coverage < 70` 一律
  `REJECTED`,先于其他阈值执行。
- **关键假设/歧义**:`critical_assumptions` 或 `critical_ambiguities`
  非空 → `NEEDS_REWRITE`,不进入 writer。
- **dedupe signal**:`memory_digest`(SHA-256(title+claim+why+
  evidence+applies_when+scope+verification))命中已接受记录 →
  `OBSERVATION`,U04 不会重复 nmem write。
- **结果状态**: `ACCEPTED` / `REJECTED` / `NEEDS_REWRITE` / `OBSERVATION`,
  verdict 含 `memory_digest`、`policy_version`、`missing_fields`、
  `rewrite_suggestion`。
- writer 只接受带 `memory_digest`、`scope`、`source` 和 policy version 的
  `ACCEPTED` record;成功写入后才原子更新持久化 ledger,因此失败不会
  抢占重试幂等键。写入前会跨进程锁住 dedupe claim、nmem 调用和 ledger
  提交；远端已接受但本地提交失败时返回 `UNKNOWN`,禁止盲目重试。
  候选设置 `semantic_review=true` 时必须通过
  `RALPH_NOWLEDGE_EVALUATOR` 指定的结构化 evaluator; evaluator 失败 =
  本条 `REJECTED`,agent 继续。

## Auto-finalization marker (Stop / SubagentStop)

Claude 在最终消息中嵌入**一个** bounded marker 即可触发自动
save-memory 写入;不嵌入 marker / 非法 marker / 非 `finalize:true`
时 Stop 仍保持 audit-only。marker 格式固定,plugin 不接受任何
变体:

```text
<!-- nowledge-memory-finalize
{"finalize":true,"memory_type":"durable_decision","title":"...","claim":"...","why_it_matters":"...","evidence":"...","applies_when":"...","scope":"...","verification":"...","critical_assumptions":[],"critical_ambiguities":[],"metrics":{"confidence":95,"evidence_coverage":88,"reusability":90,"stability":92,"scope_clarity":96,"verifiability":90,"novelty":40}}
-->
```

规则(违反任何一条即跳过保存):

- 标记名必须精确为 `nowledge-memory-finalize`,且全文只能出现一次。
- 中间必须是合法 UTF-8 JSON object(单 marker ≤ 16 KiB)。
- 对象必须包含字段 `finalize: true`(JSON 布尔字面值,不是 truthy)。
- 其余字段必须满足上面的固定 schema + 七项质量指标 + 反幻觉规则,
  与 `/save-memory` 命令的契约完全一致。
- 整篇 assistant message 出现多个 marker 时拒绝解析,plugin 不会
  自行择一处理。
- plugin 永不读取 `transcript_path`,也不会因为 marker 缺失而把
  Thread / transcript / session 摘要当作 Memory。
- 自动保存的结果通过现有 writer 落地:`SAVED` / `ALREADY_SAVED`
  / `FAILED_OPEN` / `UNKNOWN`;nmem 失败时 Stop 仍 exit 0,不阻塞
  Claude session。

## nmem 排障

- `nmem: command not found` — 安装 nmem CLI（如 `uv tool install nmem-cli`），
  并确认 `~/.local/bin` 在 PATH 中。
- 服务不可达 / 认证失败 — status 会原样报告错误；先让 Nowledge Mem 服务恢复
  可用，再重试查询。插件不会用写命令或第二个子命令"自救"。
- JSON 无法解析 — 视为故障，原样报告并停止。

## 卸载

在 target project 目录下：

```bash
claude plugin uninstall nowledge-mem-ralph@ralph-orchestrator --scope project --keep-data
```

`--keep-data` 保留插件持久数据目录；nmem 中的知识数据本身独立于插件，任何
卸载都不会删除 nmem 数据。

重装：卸载后重新执行上面的安装步骤（脚本或手动均可）；迁移脚本会识别
dedicated 缺失并重新收敛到目标状态。

## 隐私

- 插件只发起本地/已配置的 nmem JSON 查询，且有数量上限；不上传任何内容。
- 不抓取 raw Claude 会话（通用插件的自动捕获路径在本插件中完全不存在）。
- 查询结果中的历史内容按 Nowledge Mem 自身的访问控制处理。

## 与 Ralph runtime 适配计划的关系

本插件与 Ralph runtime 的 Nowledge 适配计划（检索注入、Thread 保存、distill）
**相关但不依赖**：适配计划未实施时，本插件的安装、验证与全部查询能力照常可用；
本插件不读取、不要求任何适配计划产物。
