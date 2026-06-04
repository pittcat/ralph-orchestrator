---
title: feat: 通过 cargo-nextest 实现并行测试执行
type: feat
status: active
date: 2026-06-01
---

# feat: 通过 cargo-nextest 实现并行测试执行

## 摘要

引入 cargo-nextest 作为工作空间的主测试运行器，把当前 CI 中全局 `--test-threads=1` 的限制改为 nextest 的进程级并行调度。`ralph-cli` 包内测试通过 nextest `test-groups` 配置串行运行，其他包按 nextest 默认并行度运行；doctest 继续由 `cargo test --doc` 单独覆盖，因为 nextest 当前不运行 doctest。无需改动测试代码，变更集中在 nextest 配置、测试脚本、CI 和开发者文档。

---

## 问题描述

工作空间有 9 个 crate。按 `rg '#\[(tokio::test|test|rstest|test_case)' crates -g '*.rs'` 粗略统计，当前约有 3,011 个 Rust 测试属性，其中排除 `ralph-e2e` 后约 2,554 个。CI 测试门控目前以 `cargo test -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` 运行，原因是 ralph-cli 的 loop_runner 测试使用了四个全局 `Mutex` 锁（`MOCK_ACP_EXECUTIONS`、`MOCK_ACP_EXECUTION_SERIAL`、`FAKE_PATH_BACKEND_SERIAL`、`FAKE_PATH_BACKEND_BIN`），这些锁保护 fake-backend 和 ACP mock 执行相关的全局状态。

`--test-threads=1` 标志统一传给所有工作空间测试二进制，导致没有这类全局状态约束的包也失去并行度。当前粗略统计中，非 e2e 测试主要集中在 `ralph-core`（约 1,161 个）、`ralph-cli`（约 599 个）、`ralph-adapters`（约 312 个）和 `ralph-tui`（约 249 个）。全局串行化无谓地延长 CI 测试时间，也拖慢本地开发迭代。

现有 CI 门控（`scripts/ci-rust-gate.sh`）使用单线程 `cargo test`；justfile 的 `test`/`ci` 配方目前是 `cargo test --all` 路径，并不是 CI 完全等价的单线程路径。引入 cargo-nextest 后，`scripts/run-tests.sh` 将成为统一入口：nextest 可用时使用 nextest 并行运行非 doctest 测试，nextest 缺失时回退到现有单线程 `cargo test` 语义。

---

## 需求

- R1. `ralph-cli` 包内测试必须保持串行化；nextest 是每个测试一个进程，进程内全局 `Mutex` 不能跨测试进程共享，必须由 nextest `test-groups` 提供外部串行调度。
- R2. 所有非 cli crate 的非 doctest 测试必须自动并行运行（无 `--test-threads` 全局约束）。
- R3. 现有的 `cargo test` 路径必须保持可用，作为没有 nextest 的环境的后备方案。
- R4. CI（`.github/workflows/ci.yml`）必须在测试 job 中使用 nextest。
- R5. 本地开发的 justfile 必须在原有 `test` 配方之外提供基于 nextest 的配方。
- R6. 测试退出码处理和错误报告必须达到或超过当前 `ci-rust-gate.sh` 的行为水平。
- R7. Nextest 配置必须纳入版本控制，以便所有开发者获得一致的行为。
- R8. 现有的 `--skip acp_executor::tests::test_create_terminal_and_output` 排除规则必须保留。
- R9. 当前 `cargo test` 覆盖的 doctest 不能因为迁移到 nextest 而丢失，必须通过单独的 `cargo test --doc` 步骤继续执行。

---

## 范围边界

- 不改动任何 `#[cfg(test)]` 代码、全局锁或 loop_runner.rs 及其他 crate 中的测试逻辑。
- 不添加 `serial_test` crate 或任何 `#[serial]` 属性注解。
- 不改动工作空间的测试二进制布局结构。
- 不删除 `cargo test` 后备路径。
- 不将 ralph-e2e 测试迁移到 nextest（它们有独立的 mock 基础设施）。
- 不添加测试重试逻辑或慢超时配置（留待未来迭代需要时处理）。
- 不新增 CI 专用 nextest 覆盖文件；除非后续确有 CI-only 行为，否则只维护一个 `.config/nextest.toml`。

---

## 上下文与研究

### 相关代码和模式

- **当前 CI 测试调用：** `scripts/ci-rust-gate.sh` 第 168 行运行 `cargo test -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output`
- **CI 工作流：** `.github/workflows/ci.yml` 以 `--skip-embedded-files --skip-format` 标志调用 `ci-rust-gate.sh`
- **Justfile：** `test` 配方运行 `cargo test --all`；`ci` 配方链式执行 fmt-check → lint → embedded-check → test；这条本地配方当前不是 `scripts/ci-rust-gate.sh` 的完全等价路径
- **Nextest 本地状态：** 当前开发机未安装 `cargo-nextest`（`cargo nextest --version` 返回 `no such command: nextest`），因此必须保留可验证的回退路径
- **Doctest 现状：** 仓库存在文档示例代码（例如 `crates/ralph-core/src/utils.rs`），nextest 官方说明当前不支持 doctest，迁移后必须单独运行 `cargo test --doc`
- **全局测试锁（共 4 个）：**
  - `crates/ralph-cli/src/loop_runner.rs:6261` — `MOCK_ACP_EXECUTIONS`（LazyLock<Mutex<VecDeque>>）
  - `crates/ralph-cli/src/loop_runner.rs:6266` — `MOCK_ACP_EXECUTION_SERIAL`（LazyLock<Mutex<()>>）
  - `crates/ralph-cli/src/loop_runner.rs:6924` — `FAKE_PATH_BACKEND_SERIAL`（LazyLock<Mutex<()>>）
  - `crates/ralph-cli/src/loop_runner.rs:6928` — `FAKE_PATH_BACKEND_BIN`（LazyLock<Mutex<Option<PathBuf>>>）

### 外部参考

- [cargo-nextest 配置参考](https://nexte.st/docs/configuration/reference/) — `.config/nextest.toml`、`default-filter`、`test-threads`、`test-group`
- [cargo-nextest test-groups](https://nexte.st/docs/configuration/test-groups/) — `test-groups` 和 `[[profile.default.overrides]]` 的配置语法
- [cargo-nextest 选择测试](https://nexte.st/docs/selecting/) — `--skip` 到 filterset 的迁移方式
- [cargo-nextest 预构建二进制与 GitHub Actions](https://nexte.st/docs/installation/pre-built-binaries/) — 官方推荐 `taiki-e/install-action@nextest`
- [cargo-nextest 工作模型](https://nexte.st/docs/design/how-it-works/) — nextest 逐测试进程运行；doctest 当前不支持

---

## 关键技术决策

| 决策 | 理由 |
|---|---|
| 使用 nextest 的 `test-groups` 实现 `ralph-cli` 包级串行化 | nextest 逐测试进程执行，进程内 `Mutex` 不能跨测试进程保护全局资源；`max-threads = 1` 的测试组是在 nextest 调度层提供的真正互斥 |
| 保留 `cargo test` 作为后备 | CI 故障或开发者机器上 nextest 的问题不应阻塞所有测试——旧路径仍可通过 `cargo test` 使用 |
| 不添加 `serial_test` crate | nextest 的配置级串行化已足够；为 nextest 原生提供的功能添加 crate 依赖没有必要 |
| 在单个 nextest 调用中运行所有非 e2e 包 | nextest 自动处理测试级并行调度；`ralph-cli` 通过 override 分配到串行组，其他包留在默认全局组 |
| 让 ralph-e2e 继续使用 `cargo test` | e2e 测试有自己的 mock 基础设施和 CI 入口点（`cargo run -p ralph-e2e -- --mock`）；混入 nextest 增加复杂性但收益不大 |
| 用 `cargo test --workspace --exclude ralph-e2e --doc` 保留 doctest 覆盖 | nextest 当前不运行 doctest；直接替换为 nextest 会让 CI 覆盖面倒退 |

---

## 实施单元

### U1. 添加 nextest 配置和工作空间设置

**目标：** 创建 nextest 配置，使 ralph-cli 的测试二进制串行化，同时允许所有其他二进制并行运行。

**需求：** R1、R2、R7、R8

**依赖：** 无

**文件：**
- 新建：`.config/nextest.toml`

**不修改测试代码。**

**方案：**
- 在工作空间根目录创建 `.config/nextest.toml`。
- 定义一个 `cli-serial` 测试组，设置 `max-threads = 1`。
- 通过 `[[profile.default.overrides]]` 和 `filter = 'package(ralph-cli)'` 将 `ralph-cli` 包内所有测试分配到 `cli-serial`。这是有意的包级保守策略：虽然真正有冲突的测试集中在 loop_runner 相关路径，但包级串行化避免为了提速去依赖脆弱的测试名过滤。
- 其他包不设置 `test-group`，保留 nextest 默认全局并行度（默认 `test-threads = "num-cpus"`）。
- 通过 `profile.default.default-filter` 保留现有排除规则，使用精确测试名过滤：`not (package(ralph-adapters) and test(=acp_executor::tests::test_create_terminal_and_output))`。
- 确认 nextest 自动发现 `.config/nextest.toml`（nextest 默认从工作空间根目录读取）。
- 使用 `cargo nextest show-config test-groups --workspace --exclude ralph-e2e` 验证 `ralph-cli` 测试进入 `cli-serial`，非 cli 测试留在 `@global`。

**配置草图（实现时应以 nextest 解析结果为准）：**

```toml
[profile.default]
default-filter = 'not (package(ralph-adapters) and test(=acp_executor::tests::test_create_terminal_and_output))'

[test-groups]
cli-serial = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'package(ralph-cli)'
test-group = 'cli-serial'
```

**测试场景：**
- 快乐路径：`cargo nextest run --workspace --exclude ralph-e2e` 运行所有非 e2e、非 doctest 测试；`ralph-cli` 测试顺序执行；非 cli 测试并行执行。
- 错误路径：多个 ralph-cli loop_runner 测试不会被 nextest 同时调度，因此不会因 fake backend/ACP mock 全局状态冲突产生 Mutex 中毒或竞态失败。
- 回归：之前通过 `--test-threads=1` 的测试现在通过 `cargo nextest run` 仍然通过。
- 覆盖回归：被 skip 的 `acp_executor::tests::test_create_terminal_and_output` 不出现在默认 nextest run 的执行集合中。

**验证：**
- `cargo nextest show-config test-groups --workspace --exclude ralph-e2e` 显示 `ralph-cli` 测试在 `cli-serial` 组内，其他包在 `@global`。
- `cargo nextest list --workspace --exclude ralph-e2e -E 'package(ralph-adapters) and test(=acp_executor::tests::test_create_terminal_and_output)'` 配合默认过滤验证该测试被排除；如需检查全部集合，使用 `--ignore-default-filter` 对照。
- `cargo nextest run --workspace --exclude ralph-e2e` 干净通过。
- 同一提交上运行两次该命令产生相同的通过/失败结果（确定性）。
- `cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` 仍然可以作为后备方案正常工作。

---

### U2. 创建 `scripts/run-tests.sh` —— Nextest 测试运行器

**目标：** 提供一个使用 nextest 运行测试的单一入口点，镜像当前 `ci-rust-gate.sh` 测试部分的行为。

**需求：** R3、R4、R5、R6、R8、R9

**依赖：** U1

**文件：**
- 新建：`scripts/run-tests.sh`

**方案：**
- 创建一个 shell 脚本，实现以下功能：
  1. 检查 nextest 是否可用（`cargo nextest --version`）。如果缺失，回退到单线程 `cargo test`。
  2. nextest 可用时运行 `cargo nextest run --workspace --exclude ralph-e2e`（主非 doctest 测试套件）。
  3. nextest 成功后运行 `cargo test --workspace --exclude ralph-e2e --doc`，保留 doctest 覆盖。
  4. 捕获每个阶段的退出码；任一阻塞阶段失败即非零退出。
  5. 输出与 CI 日志预期兼容的摘要。
- 使用 `set -euo pipefail` 保证安全性，与 `ci-rust-gate.sh` 风格一致。
- 添加 `--help` 标志和简短文档注释。
- **不**包含 clippy、fmt、embedded-check 或 e2e 步骤——这些保留在 `ci-rust-gate.sh` 中。
- 支持 `RALPH_CARGO_TOOLCHAIN=stable` 环境变量；设置后通过 `rustup run "$RALPH_CARGO_TOOLCHAIN" cargo ...` 调用 cargo，使 `ci-rust-gate.sh` 中安装的 stable 工具链能被测试脚本复用。

**脚本结构草图（方向性，非实现规范）：**

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FALLBACK=1

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --skip-fallback) FALLBACK=0 ;;
    --help) usage; exit 0 ;;
    *) echo "未知参数: $1"; exit 1 ;;
  esac; shift
done

run_cargo() {
  if [[ -n "${RALPH_CARGO_TOOLCHAIN:-}" ]]; then
    rustup run "$RALPH_CARGO_TOOLCHAIN" cargo "$@"
  else
    cargo "$@"
  fi
}

if run_cargo nextest --version >/dev/null 2>&1; then
  run_cargo nextest run --workspace --exclude ralph-e2e
  run_cargo test --workspace --exclude ralph-e2e --doc
  exit 0
fi

if [[ "$FALLBACK" -eq 0 ]]; then
  echo "未找到 cargo-nextest，且已禁用回退。"
  exit 1
fi

echo "⚠️  未找到 cargo-nextest。回退到 cargo test（慢路径）..."
run_cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
```

**测试场景：**
- 快乐路径：nextest 已安装 → 使用 nextest 运行非 doctest 测试，再运行 doctest，两者都通过。
- 错误路径：nextest 已安装但 `cargo nextest run` 失败 → 脚本非零退出。
- 错误路径：nextest 已安装且 nextest 通过，但 `cargo test --doc` 失败 → 脚本非零退出。
- 后备路径：nextest 未安装 → 回退到 `cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output`，行为与当前 CI 测试段一致。
- 强制路径：`--skip-fallback` 且 nextest 不可用 → 脚本非零退出，便于本地验证 nextest 安装。

**验证：**
- `./scripts/run-tests.sh` 在 nextest 已安装时成功。
- `./scripts/run-tests.sh` 在 nextest 不在 PATH 中时回退到带 skip 的单线程 `cargo test`（通过 PATH 操作验证）。
- `RALPH_CARGO_TOOLCHAIN=stable ./scripts/run-tests.sh --skip-fallback` 能通过 `rustup run stable cargo nextest` 找到 nextest，或在缺失时按预期失败。

---

### U3. 更新 justfile，添加 Nextest 配方

**目标：** 为基于 nextest 的并行测试提供便捷的 just 配方。

**需求：** R5

**依赖：** U1、U2

**文件：**
- 修改：`justfile`

**方案：**
- 添加 `test-parallel` 配方：`./scripts/run-tests.sh`
- 保留 `test` 配方为 `cargo test --all`（向后兼容，手动使用）。
- 更新 `ci` 配方链为：fmt-check → lint → embedded-check → test-parallel
- 添加 `nextest-install` 配方：`cargo install cargo-nextest --locked`（方便新开发者）。
- 可选添加 `test-serial` 配方：`cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output`，作为显式慢路径；如果维护者不想增加配方，可仅依赖 `scripts/run-tests.sh` 的自动回退。
- 保持现有的 `coverage` 和 `coverage-summary` 配方不变——它们使用 `cargo llvm-cov`，与测试运行器无关。

**差异草图（方向性，非实现规范）：**

```diff
 # 运行测试
 test:
     cargo test --all

+# 通过 nextest 并行运行测试（需要 cargo-nextest）
+test-parallel:
+    ./scripts/run-tests.sh
+
+# 单线程慢路径，等价于测试脚本的 cargo test 回退
+test-serial:
+    cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output
+
+# 安装 cargo-nextest
+nextest-install:
+    cargo install cargo-nextest --locked
+
 # 同步嵌入式资源 ...
```

**测试场景：**
- 快乐路径：`just test-parallel` 运行 nextest 测试。
- 后备路径：`just test` 仍然正常工作，没有变化。
- CI 路径：`just ci` 运行 test-parallel 而非 test。
- 慢路径：如添加 `test-serial`，其命令与 `scripts/run-tests.sh` fallback 保持一致。

**验证：**
- `just test-parallel` 在测试通过时退出码为 0。
- `just test` 行为与变更前完全一致。

---

### U4. 更新 CI —— `scripts/ci-rust-gate.sh` 和 `.github/workflows/ci.yml`

**目标：** 在 CI 中使用 nextest，如果 nextest 安装失败则优雅回退到现有单线程 `cargo test` 路径。

**需求：** R4、R6、R8、R9

**依赖：** U1、U2

**文件：**
- 修改：`scripts/ci-rust-gate.sh`
- 修改：`.github/workflows/ci.yml`

**方案：**

**`scripts/ci-rust-gate.sh`：**
- 替换测试部分（第 164-169 行），调用 `./scripts/run-tests.sh` 而非 `cargo test -- --test-threads=1`。
- 保留说明为何使用 nextest 的注释（并行 + ralph-cli 串行组）。
- 不改动 clippy、fmt、embedded-check、hooks-BDD 或 mock-e2e 部分。
- 当 `TOOLCHAIN_MODE=rustup` 时，以 `RALPH_CARGO_TOOLCHAIN=stable ./scripts/run-tests.sh` 调用，避免前面的 stable toolchain 安装被绕过；非 rustup 模式下直接调用 `./scripts/run-tests.sh`。

**`.github/workflows/ci.yml`：**
- 在 `test` job 中添加安装 nextest 的步骤，在运行测试之前。
- 优先使用官方文档推荐的 `taiki-e/install-action@nextest` 安装预构建二进制，避免 `cargo install cargo-nextest --locked` 在 CI 中重新编译带来的额外耗时。
- nextest 安装步骤必须设置 `continue-on-error: true`，或者用 shell 步骤捕获安装失败；否则安装失败会直接中断 job，`scripts/run-tests.sh` 的回退路径根本没有机会执行。
- `ci-rust-gate.sh` 脚本已内置回退到 `cargo test` 的功能（来自 U2）。
- 不改动 `web-tests`、`package-check`、`fmt` 或 `check-embedded-files` job。

**变更后的 CI 测试 job 流程：**
1. 系统依赖
2. Rust 工具链（stable + clippy）
3. 缓存 cargo（Swatinem/rust-cache）
4. `taiki-e/install-action@nextest`（`continue-on-error: true`）
5. `./scripts/ci-rust-gate.sh --skip-embedded-files --skip-format`
6. 上传测试产物

**测试场景：**
- 快乐路径：CI 安装 nextest 并并行运行测试；所有测试通过。
- 后备路径：nextest 安装失败但 job 继续 → `ci-rust-gate.sh` 回退到带 skip 的单线程 `cargo test`。
- 回归：测试产物（`.e2e-tests/report.md`、`.artifacts/hooks-bdd/`）仍然上传。
- 回归：`acp_executor::tests::test_create_terminal_and_output` 排除规则仍然生效。
- 回归：doctest 仍由 `cargo test --workspace --exclude ralph-e2e --doc` 覆盖。

**验证：**
- CI 测试 job 完成，与当前 CI 产生相同的通过/失败结果。
- CI 测试 job 完成速度快于当前 CI（测量：在 PR 运行中比较持续时间）。

---

## 系统级影响

- **错误传播：** 如果 nextest 配置有语法错误，nextest 以非零退出码退出并给出清晰的错误信息。`run-tests.sh` 中的后备仅在 nextest 不可用时触发，而非配置损坏或测试失败时触发；因此配置错误会在 CI 中明显失败。
- **状态生命周期风险：** nextest 默认每个测试一个独立进程；`ralph-cli` 的进程内静态 `Mutex` 不能跨 nextest 测试进程共享。计划通过 `cli-serial` 测试组把 `ralph-cli` 包级串行化，避免 fake backend、ACP mock、PTY 相关全局状态被多个测试进程同时使用。其他包获得更强的测试隔离。
- **覆盖面变化：** nextest 不运行 doctest；`scripts/run-tests.sh` 必须在 nextest 成功后额外运行 `cargo test --workspace --exclude ralph-e2e --doc`，否则 CI 覆盖面会低于当前 `cargo test`。
- **API 表面一致性：** 无 API 变更。所有变更都是构建工具和 CI 配置。
- **集成覆盖：** 测试退出码、hooks BDD、mock e2e 和产物上传与当前 CI 保持一致。
- **不变的不变量：** `cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` 仍然可用。`ci-rust-gate.sh` CLI 标志（`--skip-*`）保持不变。`just test` 保持不变。

---

## 风险与依赖

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|----------|
| 开发者机器上 nextest 不可用 | 中 | 低 | `run-tests.sh` 中回退到 `cargo test`；`justfile` 保留两个路径 |
| nextest 配置错误导致令人困惑的测试失败 | 低 | 中 | 配置极少；用 `cargo nextest show-config test-groups` 和 `cargo nextest list` 在实现阶段验证 |
| 忘记单独运行 doctest 导致覆盖面下降 | 中 | 中 | `scripts/run-tests.sh` 在 nextest 成功后固定运行 `cargo test --workspace --exclude ralph-e2e --doc` |
| nextest 安装步骤失败时 CI 直接中断，回退路径无法执行 | 中 | 中 | GitHub Actions 安装步骤使用 `continue-on-error: true`，把是否回退交给 `scripts/run-tests.sh` |
| nextest 本身导致 Serde 或依赖冲突 | 极低 | 低 | nextest 是独立二进制文件，不是库依赖——不影响编译 |
| nextest 安装开销增加 CI job 时长 | 低 | 低 | 使用 `taiki-e/install-action@nextest` 安装预构建二进制，避免在 CI 编译 cargo-nextest |
| 新贡献者不熟悉 nextest | 低 | 低 | `just test` 后备和 `run-tests.sh` 自动回退意味着没有人被强制使用 nextest |

---

## 文档 / 运维说明

- 更新 `CLAUDE.md` 和 `AGENTS.md` 的"构建与测试"部分，两者必须保持完全一致，记录两个测试路径：
  - `cargo test -- --test-threads=1`（后备方案，不变）
  - `cargo nextest run --workspace --exclude ralph-e2e` + `cargo test --workspace --exclude ralph-e2e --doc`（推荐路径的两个阶段）
  - `just test-parallel`（run-tests.sh 的别名）
- 更新 `scripts/ci-rust-gate.sh` 中测试部分旁边的注释块，说明为何使用 nextest。
- 如果未来 nextest 版本的 `.config/nextest.toml` 语法发生变化，CI 会捕获到。

---

## 来源与参考

- **现有 CI 门控：** `scripts/ci-rust-gate.sh`
- **测试运行器配置位置：** `.config/nextest.toml`
- **Justfile：** `justfile`（工作空间根目录）
- **CI 工作流：** `.github/workflows/ci.yml`
- **全局测试锁位置：** `crates/ralph-cli/src/loop_runner.rs` 第 6261、6266、6924、6928 行
- **nextest 配置参考：** https://nexte.st/docs/configuration/reference/
- **nextest test-groups：** https://nexte.st/docs/configuration/test-groups/
- **nextest 选择测试：** https://nexte.st/docs/selecting/
- **nextest GitHub Actions 安装：** https://nexte.st/docs/installation/pre-built-binaries/
- **nextest 工作模型与 doctest 限制：** https://nexte.st/docs/design/how-it-works/
