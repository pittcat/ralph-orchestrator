# Post-Merge Converge

把本文件复制到 `.ralph/post-merge.prompt.md`（该路径被 gitignore），然后运行：

```bash
ralph run -c ralph.post-merge.yml -H builtin:post-merge-converge
```

## 最终分支（可选）

若省略，使用当前检出分支。

```text
最终分支: main
```

## 开发计划列表（可选）

若提供，以本列表为准。若整段省略，agent 通过 `git log` / commit chronology / branch subjects 匹配 `docs/plans/` 自行发现；同一分支上顺序完成的多个计划也在范围内，不要求一定出现 merge commit。

```text
开发计划:
- docs/plans/<plan-a>.md
- docs/plans/<plan-b>.md
```

## 验证命令（可选覆盖）

默认使用仓库全量入口：

```bash
./scripts/run-tests.sh
```

- Baseline / Regression / Clean-env 的最终判定必须回到全量入口（或你在此明确覆盖的等价全量命令）。
- 子集只允许用于定位失败。
- 若全量出现竞态/时序 flake，可用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`；serial 仍失败视为真失败。
- **禁止**裸跑 `cargo test -p ralph-cli`。

## 产物与报告

- 中间产物与 Finding：`.ralph/post-merge/`
- 操作者报告：`.ralph/post-merge/REPORT.md`（complete 前必须存在；`postmerge.complete.report_path` 会指向它）

## 约束提醒

- 各开发计划已完成并已落在当前最终代码树里；这些计划可以是 merge 进来的，也可以是同一分支顺序提交完成的。不要重新执行各计划，不要重新 merge。
- 分析基于当前最终树；原 worktree / 开发分支可以已删除。
- 置信度 redo-first：P0/P1 低置信先加深/自环；仍低则 LOW_CONFIDENCE，不得当 VERIFIED，报告「待核实」。
