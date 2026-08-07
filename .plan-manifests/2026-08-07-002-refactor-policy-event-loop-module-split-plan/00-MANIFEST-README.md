# U1 Pre-flight: 文件/符号/测试基线 manifest

`git rev-parse HEAD` = e49c018a9e9f30f03ae7f35bd1d5819be52cb93a

## 1. 文件清单（计划 §0.1 第 1 类）

| 文件 | 行数 | sha256 |
|---|---|---|
| crates/ralph-cli/src/policy_check.rs | 5890 | 40492cdff7c00abf210b4f10173e1f662097fe4434fda45022167e2d267f58d5 |
| crates/ralph-core/src/event_policy.rs | 8406 | 1895fda732dd8593bd89e9b003f4268561a62beb29702ddcbd755db4b37d60b9 |
| crates/ralph-core/src/event_loop/mod.rs | 17691 | 019670bb5c0678c826af84294c504a7cf9f4b8754d0949672faab96990be7399 |
| .cursor/rules/state-management.mdc | 31 | 9fb4c9941aa24a1a3b571c0e8794d3f85dd638b23dd53d14659a70115cb4b62a |

## 2. 符号清单（计划 §0.1 第 2 类）

- policy_check.rs 顶层 item 起始行号列表见 `02-items-baseline.txt` 和 `policy-check-top-items.txt`
- event_policy.rs / event_loop/mod.rs 顶层 item 起始行号见 `02-items-baseline.txt`
- `process_parse_result` 起始行 = 9413（按 reuse-guidance.md 与 grep 复核确认）

## 3. 测试清单（计划 §0.1 第 3 类）

- `cargo nextest list --workspace` 总数 = 8042
- policy_check family = 133 IDs
- event_policy = 180
- event_loop = 1219
- 完整列表见 `04-test-ids-baseline.txt`
- 多重集 baseline 用于 U1/U2/U3 完成后对比

## 4. 工具链 baseline

- cargo nextest 0.9.140 ✓
- cargo build --workspace ✓ (49.97s, 0 error)
- just fmt-check ✓
- just lint ✓ (clippy -D warnings clean)
- cargo nextest list --workspace ✓ (8042 IDs)
