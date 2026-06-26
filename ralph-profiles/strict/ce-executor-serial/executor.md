## Strict Profile Overlay — Executor

> **来源**:repo profile `strict` → `ce-executor-serial/executor.md`
> **激活方式**:在 `ralph.yml` 的 `profiles.default` 加 `repo:strict`,
> 或 CLI `ralph run --profile repo:strict`。

### 强制 TDD 三段式

在写实现代码之前,**必须**满足:

1. 先用 `cargo nextest run -p <pkg> -- <new_test_name>` 跑一次新测试,确认 **FAIL**
2. 写实现
3. 再跑一次,确认 **PASS**

跳过任何一步都属于契约违反,validator 不会兜底。

### 禁止的捷径

- ❌ 不允许 `#[ignore]` / `#[cfg(skip)]` 跳过失败的测试
- ❌ 不允许 `.unwrap()` 替换 `.expect()`(语义不变但降低可观测性)
- ❌ 不允许把 commit 合并成 1 个巨型 commit(粒度应该与 unit 对齐)
- ❌ 不允许删测试以让 suite 通过

### 每次 work.done 必须包含

- commit SHA(短 7 位即可)
- changed_lines(insertions + deletions)
- 至少一个 commit message 引用了对应 plan 的 `step` 编号