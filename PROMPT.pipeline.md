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
