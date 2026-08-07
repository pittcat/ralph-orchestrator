---
title: 跨 plan ralph run 期间靠 worktree mtime 误判"-c 劫持"根因(2026-08-07)
date: 2026-08-07
category: developer-experience
module: crates/ralph-cli/src/commands/run.rs
problem_type: developer_experience
component: cli_run
severity: medium
symptoms:
  - "同一个 git repo 同时跑多个 `ralph run --worktree`(例如多 plan 并行)，其中一个 plan X 的 worktree `.worktrees/<plan-X>/` 在另一个 plan Y 启动时间窗内被持续写盘"
  - "plan Y 的命令行里如果出现字符串 `<plan-X>`(比如 `-c ralph.pipeline.yml.worktrees/<plan-X>/`)，观察者容易把\"X 目录被写\"+\"Y 命令里含 X\"误判为 Y 劫持/复用了 X worktree"
  - "误判后会把时间浪费在 grep `find_reusable_worktree_by_name` / `resolve_exact_worktree_name` 上，怀疑 -c 路径影响了 worktree 派生——而实际上 worktree 派生只看 `args.worktree_name` 或 `args.plan.file_stem()`"
  - "真正在写 X 目录的进程是 plan X 自己的 ralph run(PID 在 `.ralph/loops.json` 里登记)，不是 plan Y"
root_cause: false_alarm
resolution_type: documentation_only
tags:
  - worktree
  - reuse-worktree
  - cli-run
  - multi-loop
  - mtime-correlation
  - false-alarm
---

# 跨 plan ralph run 期间靠 worktree mtime 误判"-c 劫持"根因

## 现象
在跑 `ralph run --worktree --reuse-worktree --plan <plan-Y> -c ralph.pipeline.yml.worktrees/<plan-X>/` 时，
观察者注意到 `.worktrees/<plan-X>/` 在 plan Y 启动时间窗内被持续写盘(events.jsonl / flow-authority.jsonl /
current-hat-events / accepted-transitions.jsonl 等 mtime 持续增长)，结合"-c 路径里含 plan-X 字符串"，
怀疑工具按 -c 路径 basename 派生 worktree，导致 plan Y 误用了 plan X 的 worktree。

## 真实根因(已被 ps / loops.json / git worktree list 三处互证推翻)
- `-c` 路径**完全不影响** worktree 派生。
  - `crates/ralph-cli/src/commands/run.rs:717-750` `worktree_file_name_prefix()` 只读 `args.plan.file_stem()`；
  - `crates/ralph-cli/src/commands/run.rs:752-764` `resolve_exact_worktree_name()` 只用 `args.worktree_name` 或 `derived_plan_name`；
  - `crates/ralph-cli/src/commands/run.rs:1081` `find_reusable_worktree_by_name(workspace_root, name)` 用 name 拼出 `.worktrees/<name>/`，**不读** config 来源。
- `.worktrees/<plan-X>/` 在 plan Y 启动时间窗被写盘的**真实来源**是 plan X 自己的 `ralph run` 进程。
  - `ps -ef | grep "ralph run"` 显示 PID 来自 `plan-X` 的命令行；
  - `.ralph/loops.json` 中 `{id: "plan-X", pid: <X>, worktree_path: ".../<plan-X>/"}` 登记在册；
  - 写盘就是该 PID 的 stall-detector / reporter / hat activation 写自己 worktree 的 runtime artifacts。
- `git worktree list` 不含 plan-X 是另一回事——`--reuse-worktree` 复用允许走 user-mode(直接复用目录、不通过 `git worktree add`)，不是异常。
- "plan Y 命令行里出现 `plan-X` 字符串"是巧合 / 命令拼写错的副产物，与 -c 派生逻辑无关。**真正可能存在的额外 bug**是 plan-X 的 -c 路径字面不存在(例如 `ralph.pipeline.yml.worktrees/<plan-X>/` 目录从未被创建过)，CLI 应直接报"config 文件不存在"。

## 因果链(从触发到症状)
1. 同一 git repo 同时跑多个 `ralph run --worktree`(本例 001/002/006 三 plan 并行)，每个 plan 有自己的 `.worktrees/<plan-id>/`。
2. 计划 plan-X 已合入主干但 loop 未退(stall 残留，详见 `docs/solutions/.../loop-stall-residue-after-merge.md` 待补)；stall-detector 周期性写 plan-X worktree 里的 runtime artifacts。
3. 启动 plan-Y 时，命令行被拼成 `-c ralph.pipeline.yml.worktrees/<plan-X>/` —— 这条路径**根本不存在**(`find -maxdepth 2 -name "ralph.pipeline.yml*"` 找不到)，CLI 应在 preflight 阶段拒绝；**真正在跑的** plan-Y 进程的 `-c` 是 `ralph.pipeline.yml` 根目录(由 `ps` 命令行可见)。
4. 观察者 `ls -la .worktrees/<plan-X>/` 看到 runtime artifacts mtime 持续在 plan-Y 启动时间窗内增长，把"X 被写"+"Y 命令含 X 字符串"误关联为因果。
5. 误判导致去 grep `find_reusable_worktree_by_name` / `-c` 解析逻辑，浪费 10-30 分钟。

## 区分真伪的快速诊断(下次见到先做这四步)
1. `ps -ef | grep "ralph run" | grep -v grep` —— 列出**当前在跑**的所有 ralph run 及其 `--plan` 参数。
2. `git worktree list` —— 列出 git 视角的 worktree。注意 user-mode 复用的 worktree 可能不在这里，但**目录仍在**。
3. `cat .ralph/loops.json` —— 权威 loop 注册表，里面有每个活跃 loop 的 `id` / `pid` / `worktree_path`。
4. `find -maxdepth 2 -name "ralph.pipeline.yml*"` —— 验证命令行里 `-c` 路径字面是否存在。**不存在就直接证伪"-c 劫持"假设**(CLI 应早报错)。
5. 把进程实际 `-c` 参数与 `ps` 命令行交叉对比——通常 ps 输出就是真相。

只有上述四步全部排除"plan-X 自己 loop 在写"+"plan-Y 命令行实际 `-c` ≠ 描述"两条假设后，才需要进代码层追 -c 派生。

## 处置(已验证)
- `kill <plan-X-PID>` 让 plan-X loop 退出(本例 9582)。
- `ralph loops prune` —— 移除 `.ralph/loops.json` 中已死 PID 的 entry，终止 stall-detector 持续重放。
- `rm -rf .worktrees/<plan-X>/` —— 清理遗留 worktree 目录(可选，001 report.md 里也建议)。
- 修正命令行：`-c ralph.pipeline.yml` 即可(与 002/001 已成功命令一致)。如需 worktree-scoped pipeline，先 `mkdir -p ralph.pipeline.yml.worktrees/<plan-Y>/ && cp ralph.pipeline.yml ...`。

## 关联
- HARD RULE 3 (CLAUDE.md)：worktree 复用键只由 `--worktree-name` 或 `--plan basename` 决定；本备忘是该规则的"反例防误读"补丁。
- 真正的 stall 残留根因(plan 合主干但 loop 未退导致 stall-detector 持续重写)另见 `docs/solutions/.../loop-stall-residue-after-merge.md`(TODO：若该条目被多人复现，再补一篇独立学习)。
- `worktree_file_name_prefix` (run.rs:717) 的注释明确写"do NOT scan prompt text for embedded plan paths any more — that behavior was fragile and has been removed"——即"旧版从 prompt 文本自动提取 plan 路径做模糊匹配"已废弃；本备忘是把"-c 路径"也一并排除出派生来源的同源事实。

## 复现 / 验证 / 防御建议
- **不写代码**。根因是观察者认知偏差，不是工具 bug。
- 可在未来给 `ralph loops list` 加一列"目录 mtime 最近 N 秒内被改写的进程 PID"，把"哪个 loop 在写哪个 worktree"显式化，避免误关联。优先级 P2。
