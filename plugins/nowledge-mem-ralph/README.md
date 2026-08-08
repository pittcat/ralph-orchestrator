# nowledge-mem-ralph — Ralph 项目专用 Nowledge Mem 插件(0.2.0 生命周期版)

本插件是 Ralph 项目环境专用的 Claude Code 插件,提供**有界的 loop-aware
recall** 与**有界的 save-memory lifecycle**:

- SessionStart 钩子在首个 Ralph session 触发一次 bounded memory search,
  后续 session/compact/retry/supervisor worker 复用同一份 loop cache。
- Stop 钩子只做审计,不发起保存、不读取 transcript。
- 任意 hat 可通过 `/nowledge-mem-ralph:save-memory`(本插件 0.2.0 暂
  未提供,U03 引入)在 activation 内提交 Memory 候选;插件校验固定
  schema 与质量指标后才写入 nmem。

**不**抓取 raw Claude 会话,**不**读取 Working Memory,**不**写入
Claude transcript。设计与边界详见
`.ralph/specs/nowledge-mem-ralph-plugin-design.md`(本仓库内)。

## 插件选型

| 场景 | 应使用的插件 | scope | 说明 |
|---|---|---|---|
| 人工交互 Claude Code 会话 | 通用插件 `nowledge-mem@nowledge-community` | `user` | 保留会话自动捕获等完整能力 |
| Ralph 启动的 Claude Code child(target project) | 本插件 `nowledge-mem-ralph@ralph-orchestrator` | `project` | lifecycle 钩子 + 只读查询,无 transcript 写入 |

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
- `search-memory` skill — 供 agent 在确有需要时主动做同样的有界只读查询。
- 仅当确需追溯原对话且 memory 结果不足时，才允许有界的
  `nmem --json t search` / `nmem --json t show`（每次最多 8 条消息、每条最多
  1200 字符，按需翻页）。

## 无自动捕获保证

- hooks/hooks.json 当前注册 `SessionStart` + `Stop`(U05 增加
  `SubagentStop`)。两条钩子都不读取 raw transcript、不读
  `last_assistant_message`、不抓取会话内容,Stop 只追加 audit record。
- 通用插件的「整段会话自动捕获」路径在本插件中完全不存在;本插件的
  写操作只能由 agent 在 activation 内显式调用 `save-memory`,且必须
  通过固定 schema + 硬门槛 + 质量指标。
- 会话内容的保存与蒸馏由 Ralph 自己的 curation 流程负责(见适配计划),
  不属于本插件。

## Lifecycle contract(0.2.0)

| 事件 | 触发 | 行为 | 失败 |
|---|---|---|---|
| SessionStart | 任何 Claude session 启动 | env gate → bounded recall → bounded additionalContext(`<knowledge-context historical-evidence="untrusted">`);loop cache miss 写 `recall.json`,hit 直接返回;source=`compact` 跳过 search | 缺 RALPH env = noop;nmem 错 = fail-open,空 additionalContext |
| Stop | session/worker 结束 | audit-only,append 状态标记,不读 transcript | 永不抛错;永不补保存 |
| SubagentStop | (U05 引入) | 与 Stop 同语义 | 同上 |

`recall` 的细节(0.2.0):`scripts/recall.py` 在首个 SessionStart 拿
`flock` lease 后发起一次 `nmem --json m search <query> --limit 5`
(`query` 仅由 repo basename + preset + workspace_root 派生,绝不读
transcript / last_assistant_message),把渲染好的 XML(转义了 `<>&`、
控制字符剥除、按 UTF-8 字节边界截断到 4KB)原子落盘
`recall.json`;后续同 loop 的 SessionStart(普通 hat、compact、retry、
supervisor worker)直接命中 cache,`nmem` 计数=0。`source=compact`
无论 cache 是否命中都不调 search。

`save-memory` 入口(U03 引入)统一由 `scripts/memory.py` 提供,插件内部
决定是否调用 nmem;失败 fail-open(Ralph/Claude 继续运行),evaluator 失败
= 本条 REJECTED(F4 统一语义),agent 继续。

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
