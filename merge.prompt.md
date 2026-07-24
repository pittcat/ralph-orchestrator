# Merge Batch

目标分支: pittcat-dev

## 验证命令（Stabilizer 必须跑全量，不要降级成子集）

本仓库的**全量测试入口**是:

```bash
./scripts/run-tests.sh
```

- Stabilizer 在 attempt 1 必须把 `verification_command` **锁定为 `./scripts/run-tests.sh`**（nextest + doctest 全 workspace），并在后续每次 `merge.retest` 复用它。
- **禁止**用单包子集（如 `cargo nextest run -p <pkg> -- <substring>`）替代全量验证来判断 `passed: true`。子集只允许用于定位失败，最终判定必须回到全量入口。
- 若全量基线出现竞态/时序类 flake，改用单线程兜底 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`；serial 仍失败即为真失败，必须修复。
- **禁止**裸跑 `cargo test -p ralph-cli`（会触发 loop_runner process-global Mutex 中毒 flake）。

## 待评估并合并的分支（顺序）
ralph/2026-07-03-001-feat-supervisor-rusqlite-parallel-preset-plan-sunny-jay