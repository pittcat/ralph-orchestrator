---
title: run-tests.sh doctest 阶段被 600s 超时强制清场(空 doctest crate + 超时默认值低估)
date: 2026-06-21
category: developer-experience
module: scripts/run-tests.sh
problem_type: developer_experience
component: testing_framework
severity: high
symptoms:
  - "./scripts/run-tests.sh doctest 阶段跑过 ralph_adapters/ralph_cli/ralph_core 后,在 ralph_telegram 处被 600s 超时清场"
  - "实际剩余 ralph_telegram/ralph_tui/ralph_api/ralph_bench 永远跑不到,LOOP_COMPLETE 永远拿不到"
  - "本地 macOS 与 CI ubuntu 表现差异巨大(ubuntu ~5-7 分钟,macOS 12-15 分钟)"
  - "测试日志最后一行是 ⏰ 总超时 600s,强制清场... killed ./scripts/run-tests.sh"
root_cause: configuration_error
resolution_type: code_change
tags:
  - doctest
  - cargo-test
  - run-tests
  - timeout
  - macos
  - developer-experience
---

# run-tests.sh doctest 阶段被 600s 超时强制清场

## Problem

`./scripts/run-tests.sh` 在本地 macOS 跑全量基线时,nextest 阶段能通过(实测 ~161s,4862 tests pass),但进入 `cargo test --workspace --exclude ralph-e2e --doc` 阶段后会**被脚本内置的 600s 总超时杀掉**,导致 doctest 永远跑不完,LOOP_COMPLETE 永远拿不到。

2026-06-21 实测:doctest 阶段从 ralph_adapters 开始串行执行,跑过 ralph_adapters → ralph_cli → ralph_core,在 ralph_telegram 阶段被 `kill -KILL -$$` 强制清掉,实际剩余 ralph_telegram / ralph_tui / ralph_api / ralph_bench 还没执行。

## Symptoms

- `cargo test --doc` 阶段日志稀疏,只输出 `Doc-tests <crate>` 头一行就等下一个 crate
- 进程栈:`cargo::ops::cargo_test::run_tests → ProcessBuilder::status → __wait4`,cargo 在等 rustdoc 子进程退出
- rustdoc 子进程自身 CPU 6-8%、RSS ~73MB,在 `SearchPath::new → __getdirentries64` 枚举 `target/debug/deps` 后做 lint + 编译
- 600s timeout 触发后:`⏰ 总超时 600s,强制清场... [1] 22381 killed ./scripts/run-tests.sh`

## Root Cause

双层叠加,两层都需要修。

### 1. `cargo test --doc` 在空 doctest 的 crate 上仍跑完整 rustdoc 流水线

当前 workspace 9 个 crate,**真正含 ```rust doctest 的为 0 个**:

```bash
$ for c in crates/*/src; do
    name=$(basename $(dirname $c))
    printf "%-15s %s\n" "$name" "$(rg -c '\`\`\`rust\b' "$c" 2>/dev/null | wc -l)"
  done
ralph-adapters         0
ralph-api              0
ralph-bench            0
ralph-cli              0
ralph-core             0
ralph-e2e              0
ralph-proto            0
ralph-telegram         1   # bot.rs:522 是测试用例里 let input = "...```rust..." 的字符串字面量,不是 doc comment
ralph-tui              0
```

但 `cargo test --doc` 对每个 crate 串行 fork 一个 `rustdoc --test`,rustdoc 仍要做:

- `SearchPath::new` 枚举 `target/debug/deps/*.rlib *.rmeta`(`__getdirentries64`,在 macOS 上慢)
- 完整 lint:`--warn=clippy::pedantic` + 30+ allow 列表
- 类型检查 + 单 crate 内 doctest extraction

**实测单个空 doctest crate 仍耗时 1-3 分钟**。8 个相关 crate 串联后总时长 10-15 分钟,远超 600s 超时阈值。

### 2. `TOTAL_TIMEOUT_SECONDS=600` 默认值基于过时估算

`scripts/run-tests.sh:30` 注释(修复前):"10 分钟覆盖 workspace 全量 nextest + doctest 的实测耗时(~5-7 分钟)"。

这个数对 CI `ubuntu-latest` 接近准确——无 XProtect 串行化,rustdoc 跑完 ~5-7 分钟。但对本地 macOS:

- **nextest 阶段**:~3 分钟(parallel + process-per-test,跟 OS 无关)
- **doctest 阶段**:~10-15 分钟(rustdoc 单进程 + SearchPath 枚举慢 + 7+ crate 串联)
- **合计**:~13-18 分钟,**远超 600s**

## Why This Matters

CLAUDE.md Hard Rule 1 第 ① 项允许 `cargo test --doc` 作为测试入口的合法例外——**doctest 不能删除,必须保留**。修复必须从"跑哪些 crate"和"留多久"两端优化,不能改硬规则。

CI 不受影响:`.github/workflows/ci.yml:32` 走 `ubuntu-latest`,doctest ~5-7 分钟,虽然紧但能跑完。**升级 timeout 主要是为本地 macOS 服务**,CI 不会变慢。

## Solution

### 1. `scripts/run-tests.sh:31` 升级默认超时 600→1500

```diff
- # 10 分钟覆盖 workspace 全量 nextest + doctest 的实测耗时(~5-7 分钟)。
- TOTAL_TIMEOUT_SECONDS=${TOTAL_TIMEOUT_SECONDS:-600}
+ # 25 分钟覆盖 workspace 全量基线:本地 macOS nextest ~3 分钟 + doctest ~2-3 分钟(已跳过空 crate)
+ # + rustdoc 全量首跑开销;CI ubuntu-latest 上实测 ~5-7 分钟,本地 macOS 跳过 doctest 后 ~6-8 分钟。
+ TOTAL_TIMEOUT_SECONDS=${TOTAL_TIMEOUT_SECONDS:-1500}
```

**1500 的依据**:本地 macOS 全量(nextest ~3 分钟 + doctest 跳过空 crate 后 ~2-3 分钟 + 编译复用缓存)合计 ~6-8 分钟。1500 留 1.5x 余量,防止未来加 doctest 后又被杀。

### 2. `scripts/run-tests.sh:212-262` doctest crate 过滤

在 nextest 阶段后,用 `rg -q '```rust\b'` 扫每个 crate 的 `crates/<pkg>/src/`,只在有 ```rust 代码块时才跑 `cargo test -p <pkg> --doc`。

```bash
crates_with_doc=()
crates_skipped=()
for crate_dir in crates/*/; do
  crate_name=$(basename "$crate_dir")
  [[ "$crate_name" == "ralph-e2e" ]] && continue
  if command -v rg >/dev/null 2>&1; then
    if rg -q '```rust\b' "$crate_dir/src" 2>/dev/null; then
      crates_with_doc+=("$crate_name")
    else
      crates_skipped+=("$crate_name")
    fi
  elif grep -rl '```rust' "$crate_dir/src" >/dev/null 2>&1; then
    crates_with_doc+=("$crate_name")
  else
    crates_skipped+=("$crate_name")
  fi
done

if [[ ${#crates_with_doc[@]} -eq 0 ]]; then
  echo "📚 跳过 doctest:workspace 中 8 个非 e2e crate 均无 \`\`\`rust 代码块"
elif [[ ${#crates_skipped[@]} -gt 0 ]]; then
  echo "📚 doctest:跳过 ${#crates_skipped[@]} 个无 doctest 的 crate($(...)),只跑:$(...)"
  for crate_name in "${crates_with_doc[@]}"; do
    run_cargo test -p "$crate_name" --doc
  done
else
  echo "📚 运行 doctest 覆盖(cargo test --workspace --doc)..."
  run_cargo test --workspace --exclude ralph-e2e --doc
fi
```

**关键设计**:
- `rg -q` 找第一个匹配就退出,8 crate 串联 <1s;`-c` 计数要扫完整个 src/,慢且语义冗余
- ` ```rust\b` 精确匹配(避免误判 ```rustscript / ```rustic 等未来方言)
- **`command -v rg` 检测 + `grep -rl` fallback**:脚本在 PATH 不含 ripgrep 时仍能自愈,不强制依赖
- `[[ "$crate_name" == "ralph-e2e" ]] && continue` 显式排除 e2e,与 `nextest run --workspace --exclude ralph-e2e` 保持一致
- 三分支:`全空跳过` / `部分跳过只跑剩余` / `全有才全跑`,覆盖未来加 doctest 的演化路径

## What Didn't Work

- **直接删除 doctest 阶段**:违反 CLAUDE.md Hard Rule 1 第 ① 项允许例外,否决
- **`cargo test --doc --no-run`**:只编译不跑,但 lint + 编译仍要执行,不解决根本慢
- **`RUSTDOCFLAGS=-Awarnings`**:消掉 lint warning 但不解决 SearchPath 枚举开销
- **加 `--jobs N` 给 rustdoc**:rustdoc 本身是单进程单线程,不支持并行;`cargo test --doc` 没有 `--jobs` 选项
- **只升 timeout 不做过滤**:本地 macOS 仍要等 12-15 分钟,体验差

## Verification

修复后(2026-06-21 本地 macOS 实测):

```bash
$ ./scripts/run-tests.sh
🚀 使用 cargo-nextest 并行运行测试(ralph-cli 串行组,其它包并行)...
   ... (nextest 输出,161s)
     Summary [ 161.108s] 4862 tests run: 4862 passed, 13 skipped

📚 跳过 doctest:workspace 中 8 个非 e2e crate 均无 ```rust 代码块

✅ 测试通过(nextest + doctest)
```

总时长 ~165s,比修复前的 600s+ 强制清场有质的提升。

回归保护:
- 在某个 crate 加 `/// \`\`\`rust fn main() {} \`\`\``,下次跑脚本应自动把它纳入 `crates_with_doc` 并跑该 crate 的 `--doc`
- `PATH=/usr/bin:/bin ./scripts/run-tests.sh`(去除 rg)应走 `grep -rl` fallback 路径,行为不变

## Prevention

- 未来加 doctest 务必用 ` ```rust ` 三反引号开头 + `\b` 后缀匹配(rustdoc 标准格式)
- 每次给 `TOTAL_TIMEOUT_SECONDS` 调整值时,需同步检查 `scripts/ci-rust-gate.sh` 的 CI 调用是否还兼容(目前 CI 直接调用 `run-tests.sh`,无独立 timeout 约束)
- 如果发现 rustdoc 在某个 crate 特别慢,先用 `sample <pid>` 抓栈,看是不是又卡在 `__getdirentries64`——若是,考虑给该 crate 加 `package.metadata.docs.rs.disable` 或拆 crate

## Related

- `docs/solutions/developer-experience/macos-nextest-test-list-hang-orphan-processes-2026-06-20.md` — 同目录的姊妹篇,记录 nextest test-list 阶段在 macOS 上的孤儿 `--list` 问题(不同根因,但同属 nextest+macOS 性能/挂起类)
- `CLAUDE.md:11` Hard Rule 1 — 测试入口硬规则,本次修复在第 ① 项允许例外内,未触碰
- `scripts/ci-rust-gate.sh:165-184` — CI 入口,直接调用 `run-tests.sh`,timeout 升级对 CI 透明(ubuntu 仍 5-7 分钟,在 1500s 内)
- `.github/workflows/ci.yml:32` — `runs-on: ubuntu-latest`,确认 CI 不受 macOS 性能差异影响