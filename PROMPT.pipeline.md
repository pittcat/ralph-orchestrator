# ce-executor-pipeline 编排契约

固定 prompt：只说明**本轮执行哪份 plan** 与**全 hat 共用的仓库级约束**。
不写事件 topic、不写 hat 拓扑、不写上下游该 emit 什么——那些只在各 hat 自己的 `instructions:` 与注入的 `## HAT IDENTITY` 里。

## 执行目标

本轮开发计划路径见仓库根目录 **`execution.target`**（第一行非空、非 `#` 的路径，repo 相对；可为目录或 plan 文件）。
scope、任务拆分、验收标准 **以该 plan 正文为 SSOT**；本文件不重复 plan 内容。

## Scope（默认）

- **In scope** — `execution.target` 所指向 plan 要求改动的代码 / 测试 / 文档。
- **Out of scope（默认）** — 除非 plan 明确要求，不改 `presets/`、`presets/schemas/`、
  `presets/manifest.yml`、`presets/index.json`；不改其它 plan 文件；
  不改 `crates/ralph-core/data/*.md`（除非 plan 单列且按 CLAUDE.md 同步 skill）。

## 共用约束（不含事件名）

3. **Precheck** — 任何 emit 前先 `--policy-check`（`ralph-tools` §5），通过后再写盘。
4. **不分支** — 禁止 agent 侧 `git checkout -b` / `git switch -c` / `git worktree add`。
5. **不 push** — commit 落本地，push 由 operator 处理。
6. **不杀 ralph 进程** — 停 loop 由 operator 或当前 hat 按 preset 说明处理，禁止 kill/pkill ralph。
7. **证据先于收工** — 声称完成前须有可核验证据（测试输出、构建结果、产物文件等），禁止空占位。

## Notes

- 你该做什么、该 emit 什么 topic、payload 字段 — **只看本次 prompt 里的 `## HAT IDENTITY`、trigger 事件、以及 preset 里本 hat 的 `instructions:`**。
- 不要读 `.ralph/events.jsonl`；进度与上游交付物靠 **trigger payload** 与 **约定磁盘产物**。
- confidence ≤ 80 的决策写 `.ralph/agent/decisions.md`。

## 每轮必做的事(纪律)

> 这一节是给"你"看的 —— 你正在一次 activation 里跑。
> 你是哪个角色不重要(planner / executor / reviewer / fixer / ...),这一节对你都生效。

### 1. 你进来时,要立刻知道两件事

- **你是谁** —— 看本 prompt 顶上注入的 `## HAT IDENTITY` 段;那段会写你的角色名、能发什么事件、不能发什么事件。
- **你这轮要做什么** —— 看 trigger 事件(就是把你拉起来的那条)。payload 里通常有 `plan_name` / `plan_path` / `executor_head_sha` 等字段,那些字段就是你本轮的全部输入,不要向外部去找上下文。

### 2. 做你该做的事

按 `## HAT IDENTITY` 给你的角色去跑 —— 该写代码写代码,该读文件读文件,该产出 review 报告就产出。该并行就并行,该串行就串行,你自己判断。

### 3. 收尾:你必须发至少一条事件

> 这一步不能忘。

在你准备结束本轮**之前**,做一次自检,问自己"我做的活儿到底成没成":

- **成功了**(代码改了、测试也跑了、commit 也落了)→ 发**一条成功事件**(例子:`work.done` / `plan.ready` / `review.complete` / `report.done`,具体看你头顶那段 `## HAT IDENTITY` 写你能发哪些)。
- **失败**(build 起不来、测试红、改到一半发现计划有问题)→ 发**一条失败事件**(例子:`work.failed` / `plan.blocked`,也看 `## HAT IDENTITY`)。
- **没做完就跑**(外部依赖挂掉、token 用完、上下文截断)→ 也算失败,发失败事件,**不要装成功**。

### 4. 怎么发(完整语法见 ralph-tools-emit.md / ralph-tools.md)

两步:
1. 先 dry-run 一次:`ralph emit <topic> --policy-check -j '<payload>'`
2. 通过了再去掉 `--policy-check` 真正写盘

(完整的命令语法、字段约束、payload schema,见跟本 prompt 一起注入的 `ralph-tools-emit.md` / `ralph-tools.md`。两份应该都在你上下文里。)

### 5. 一次只发一条事件

**只发 1 条**。如果你想中间 progress 告诉别人,先**等下一轮**再发下一条;本轮只发 1 条。

### 6. 失败时怎么留痕

如果你走的失败路径,**想让下一轮 hat 知道为什么失败**:
- 把详细原因写进 `.ralph/agent/decisions.md`(这个文件就是给你留决策日志的)
- 在失败事件的 payload 里**只写一句话总结 + 关键 ID**,详细去看那个 markdown

### 7. 不知道该怎么发时

三选:
- 抬头看 `## HAT IDENTITY` —— 里面有你的角色 + `publishes`(你能发的 topic 列表)+ `exempt_topics`(豁免)
- 跑 `ralph tools task list --hat <你的角色>` 看任务上下文
- 实在发不出来,先把 payload 写好再跑 `ralph emit <topic> --policy-check` 看错误信息
