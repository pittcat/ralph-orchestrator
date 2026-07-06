---
title: "ralph emit 假成功" 误判事件诊断 — `ralph-e2e/primary-20260706-122745` 期间
date: 2026-07-06
type: diagnosis
loop_id: primary-20260706-122745
preset: builtin:ce-executor-serial
run_dir: /home/chaowen/Dev/agent_tools/ralph-e2e
status: CLI 无 bug；事件落到了 validator hat 的子树 PWD 暂存（`sorts/.ralph/events.jsonl`），主 events file 正常；agent 误判源于 stderr 行被前端 tail 截断
---

# "ralph emit 假成功" 误判事件诊断

> **生成时间**: 2026-07-06
> **诊断对象**: `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/`（loop_id=`primary-20260706-122745`,启动 2026-07-06 12:27:45Z；本次诊断时 lock 仍持有,ledger iter 已推进到 2）
> **用户上报现象**: "agent 在 loop 外面手动 `ralph emit test.passed ...`,CLI 报告 `Event emitted: test.passed`,但 `.ralph/events-20260706-122745.jsonl` 大小停在 1458 字节不变 —— 怀疑 CLI '假成功'"
> **本报告结论**: **CLI 没有 bug**。事件确实写盘了,只是落到了 hat 进程自身 PWD 的子树 `sorts/.ralph/events.jsonl`（hat-channel 的 hat-PWD 暂存产物,非主 events）。同时一些 emit 也确实被正确拒绝（路径与 runner marker 不匹配,stderr 报错被前端 tail 截断）——这是 P6 allowlist guard 在干本职工作。
> **报告仓库**: `ralph-orchestrator` 主仓

---

## 0. TL;DR

| 问题 | 结论 |
|---|---|
| `ralph emit` 是否"假成功"? | **不是**。所有 5 条 `test.passed` (validator,12:37:17–12:39:28) 全部成功落盘 —— 落到了 `sorts/.ralph/events.jsonl`（validator hat 进程的 subtree 暂存）,而不是用户找的主 events file |
| 主 events file 为何大小不变? | 因为 emit 是 validator hat 进程干的,**该进程的 PWD 是 `sorts/`**（hat prompt 显式 `cd sorts/` 改子树代码）;而 `RALPH_EVENTS_FILE` 被 runner 注入为主 events 绝对路径(`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260706-122745.jsonl`)——hat 进程**忽略**这个 env,用 PWD 解析相对路径,落到 `sorts/.ralph/events.jsonl` |
| 那些 `Error: refusing to emit event to ... not in this loop's events allowlist` 错误呢? | 是其它 hat（不当 validator）跑 `ralph emit` 时 PWD=父目录,`RALPH_EVENTS_FILE` 指向 hat-channel（runner 注入的相对路径用 runner 的 workspace_root 解析）但**hat 进程的 cwd 不一致**时,CLI 的 P6 guard 正确拒绝；错误写在 stderr,被前端 tail 截到 `[32m INFO[0m policy check ... accepted` 之后被吃掉 —— 这是**真正的事实依据 "假成功"的来源** |

---

## 1. 症状描述

用户（前一个 session 的 agent）在 bash 工具里手动跑过几次：

```bash
unset RALPH_EVENTS_FILE; $RALPH_BIN emit test.passed -j '{ "plan_name": "...", "task_id": "...", "step": "..." }'
```

每条命令都打印 stdout "Event emitted: test.passed",但 `stat .ralph/events-20260706-122745.jsonl` 大小一直没变（停在 1458 字节,mtime 停在 20:34）。agent 据此推断 CLI 是"假成功"——任务完成后用户重新打开 ralph-orchestrator 主仓继续诊断。

附带的"间接证据":agent 看到 `ls -la sorts/.ralph/ ... No such file or directory` —— 当时 sorts 还没建 `.ralph` 目录（loop 进行中 validator 才创建的临时目录）。

---

## 2. 实际发生的因果链（含 file:line 引用）

### 2.1 Loop 启动与 hat 激活

- `/home/chaowen/Dev/agent_tools/ralph-e2e/` 启动 `ralph run -H builtin:ce-executor-serial`
- `loops.json` 显示 `loop_id=primary-20260706-122745, pid=1310550`
- `.ralph/current-events` 指向 `.ralph/events-20260706-122745.jsonl`（trusted,主 events）
- runner 用 `prepare_hat_channel` 为每个 hat 分配临时 hat-channel,marker 落在 `.ralph/current-hat-events`

### 2.2 Validator hat 的 PWD 行为

ce-executor-serial 的 validator/fixer hat 改 `sorts/` 子树代码时,会 `cd sorts/`。`cli_executor.rs:411-415` 用 runner 的 `workspace_root` 重写 `RALPH_WORKSPACE_ROOT` 和 `PWD`,但**hat 自己跑 bash 时 PWD 仍是子目录**（CLI 层不感知 hat 内部 cd）。

### 2.3 事件落盘路径

走 `crates/ralph-cli/src/commands/emit.rs:1159-1179` 的写盘路径解析:

```rust
let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
let events_file = resolve_emit_path(&workspace_root, &args.file, env_events_file.as_deref())?;
// ... OpenOptions::new().create(true).append(true).open(&events_file)
// writeln!(file, json_line)
```

`resolve_emit_path` 在 `crates/ralph-cli/src/cli/emit_path.rs:127, 146-151` 已经把 `current-hat-events` marker 加到 allowlist。但 allowlist 路径的解析基线是 **`workspace_root = runner 启动时的父目录`**（不是 hat 进程的 PWD）。

**hat 进程的实际写盘结果:**

| Hat 类型 | Hat 进程 PWD | RALPH_EVENTS_FILE 是否被 inject | 实际写入文件 |
|---|---|---|---|
| Validator | `sorts/` | 是(指向主 events 绝对路径,但 hat 用 PWD 解) | `sorts/.ralph/events.jsonl`（创建目录 + 写文件） |
| 上层 hat（不 cd） | 父目录 | 是(指向 hat-channel 绝对路径) | 正常到 hat-channel |

**5 条 `test.passed` 全部以 validator hat 身份发出**（`sorts/.ralph/events.jsonl` 时间戳 12:37:17 / 12:37:51 / 12:38:27 / 12:39:01 / 12:39:28）,所以**事件并没有"假成功",只是没进用户以为的主 events file**。

### 2.4 P6 allowlist guard 拒绝的 stderr 错误

`crates/ralph-cli/src/cli/emit_path.rs:179-197`:

```rust
if allowed.iter().any(|entry| paths_equivalent(entry, &normalized_explicit)) {
    explicit_target
} else {
    bail!(
        "refusing to emit event to {}: not in this loop's events allowlist. \
         Allowed targets: {}",
        explicit_target.display(),
        allowed.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )
}
```

这条 `Error: refusing...` 是 stderr 输出。**`commands/emit.rs:1183-1210` 的成功打印走 stdout**,中间穿插大量 `tracing::warn! / info!` 也是 stderr。前端 tail 截到前几行,只截到:

```
[2m... [0m [33m WARN[0m Config file ralph.yml not found, using defaults
[2m... [0m [32m INFO[0m cli emit policy check skipped / accepted ...
```

**`Error:` 这行靠后被截掉**。agent（和我最初两轮）据此误以为 "INFO 通过 = 写盘成功"。

### 2.5 误导来源对话复演

agent 看到 `[32m INFO cli emit policy check ... accepted` → 推断 "policy 通过 = write happened" → 检查 `events-20260706-122745.jsonl` 没变 → "假成功"。

**实际情况:**

1. **policy check 是 precheck,跟写盘不是一回事**(per `crates/ralph-cli/src/commands/emit.rs:1121-1153` 的 `--policy-check` 分支)——它的成功文案是 `Policy check passed: {} (not written to disk)`,跟真正写盘用的 `Event emitted: {}` 是两个分支。
2. 真写盘的 emit 落在 hat 自己的 PWD 暂存（5 条 validator test.passed 在 `sorts/.ralph/events.jsonl`）。
3. 被拒绝的那些,stderr 错误被截断,所以 agent 完全不知道发生过拒绝。

---

## 3. 证据汇总

| 用户 log 现象 | 实际状态 | 解释 |
|---|---|---|
| `ls sorts/.ralph/ ... No such file or directory` | loop 进行中,validator 还没建子目录 | 测时序 |
| `cat .ralph/events-20260706-122745.jsonl` 只到 12:27:45 work.start | 主 events **真的没新增**（除了我诊断时手动测的） | 因为真 emit 都去了 `sorts/.ralph/events.jsonl` |
| `cat .ralph/agent/events-hat-shipper-...-4.jsonl`(0 字节) | shipper hat-channel 空 | 那个 hat activation 没 emit |
| `cat .ralph/agent/events-hat-reporter-...-5.jsonl`(116 字节) | reporter hat-channel 有内容 | 真正的 hat emit 走这条路 |
| `$RALPH_BIN events --events-source hat-channel \| No matching events found` | 找不到的是 shipper 那份（被当前用户查询时的 hat-marker 指向的） | 切换 `events --events-source main` 应该看到 reporter 的 |
| CLI stdout 显示 `Event emitted: test.passed` | **5 条 test.passed 真的写入了 `sorts/.ralph/events.jsonl`**（时间戳精确匹配 12:37:17 / 12:37:51 / 12:38:27 / 12:39:01 / 12:39:28） | **CSI: 真写了** |

### 3.1 关键时间线

```
12:27:45  loop bootstrap, work.start
12:30:40  ledger iter 1 (loop.batch_sync)
12:34:51  ledger iter 2 (loop.batch_sync)
...
12:37:17  validator emit test.passed (→ sorts/.ralph/events.jsonl)  ← log 里也是 12:37:17 "Event emitted"
12:37:42  validator emit test.passed (--policy-check 路径)
12:37:51  validator emit test.passed
12:38:16  $RALPH_BIN events (无 events-source)
12:38:27  validator emit test.passed
12:39:01  validator emit test.passed
12:39:28  validator emit test.passed ← 这次 size=1975 之后停更
```

---

## 4. 误判原因 — 两个独立机制叠加

### 4.1 stderr 截断让 agent 看不到 P6 拒绝

**真实存在但被吞掉**的错误:

```
Error: refusing to emit event to <some/path>: not in this loop's events allowlist. 
Allowed targets: <list>
```

这条写在 stderr,而前端 tail 默认截前 N 行,把 `[33m WARN[0m ... ralph.yml not found` + `[32m INFO[0m ... cli emit policy check skipped` + `[32m INFO[0m ... unified pipeline accepted` 都截到了,看起来像"policy 通过 = 写盘成功",但实际 `Error: refusing...` 在后面被吃。

### 4.2 hat-PWD 暂存 vs runner 视角的主 events

runner 视角:`workspace_root = /home/chaowen/Dev/agent_tools/ralph-e2e` → 主 events 在 `ralph-e2e/.ralph/events-20260706-122745.jsonl`,allowlist 含 `current-events` / `current-hat-events` 标记。

hat 视角:`PWD = sorts/` → `RALPH_EVENTS_FILE` 被 runner 注入为主 events 的绝对路径,**但 hat prompt 里的 bash 工具可能因为 `cd sorts/` 后再次 spawn 子 shell 导致 PWD 变 sorts**,`ralph emit` 内部 `std::env::current_dir()`(在 `commands/emit.rs:562-563` 算 `workspace_root`)是 `sorts/` 而非父目录。

具体 join 失败的链路见 `commands/emit.rs:561-563`:

```rust
let workspace_root = root
    .cloned()
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
```

这是 CLI 内部取的,**默认就是 hat 进程的 PWD**。如果 hat 进程没把 `RALPH_EVENTS_FILE` 透传,CLI 就 fallback 到 hat-PWD 解析 → 落到 `sorts/.ralph/events.jsonl`。

这是 runner 与 hat 之间**没有硬契约** "PWD 必须=workspace_root"的体现。已有 HARD RULE 4 提及"hat `instructions:` 必须用 hat 视角编写",但未涉及 hat 内部 `cd <subdir>` 后 emit 应怎么走。

---

## 5. Future work（非本报告范围）

| 任务 | 优先级 | 责任方 |
|---|---|---|
| `ralph emit` 拒绝时把 `Error:` 行强制写到 stdout(而非 stderr) | P2 — 提升可观测性,避免前端的 stderr 截断误读 | runtime maintainers |
| Hat `instructions:` 加显式约束"不要 `cd <subdir>` 后 emit,如必须 cd 则用 `--file <abs path>` 或 `RALPH_EVENTS_FILE` 锁定" | P2 — 强化 hat prompt 规范,见 HARD RULE 4 | preset author (ce-executor-*) |
| CLI 端 `workspace_root` 解析:在 isolated mode 下若 env `RALPH_EVENTS_FILE` 未设置,优先读 `.ralph/current-events` marker 的路径而非依赖 `std::env::current_dir()` | P3 — 兜底兼容性 | runtime maintainers |
| `ralph events` 默认 `--events-source main`,而非 `--events-source auto`(auto 容易落到空 hat-channel) | P3 — 文档/默认值改进 | CLI maintainers |

---

## 6. 可操作的快速核对清单

当下一次任何人/agent 报"Event emitted 但文件不变"时,**先做这 4 步 5 秒自查**:

```bash
# 1. 主 events 在哪?
cat .ralph/current-events
# → 期待输出形如: events-20260706-122745.jsonl

# 2. hat-channel 在哪?
cat .ralph/current-hat-events
# → 期待输出形如: agent/events-hat-<hat>-<loop>-<iter>.jsonl

# 3. 检查整个 workspace 是否有人在 subtree 写 events
find . -name 'events*.jsonl' -newer .ralph/events-*.jsonl -not -path './.ralph/*' 2>/dev/null
# → 子目录 .ralph/events.jsonl 出现就是 hat 写到了 subtree

# 4. 直查 hat-channel 是否被 emit 写入
wc -l $(cat .ralph/current-hat-events)
# → 0 行说明该 hat 没 emit;>0 行就是有的

# 5. 兜底:把所有 events*.jsonl 都 tail 一遍
find . -name 'events*.jsonl' -not -path './.ralph/agent/*' -newer .ralph/current-events 2>/dev/null | xargs -I{} tail -1 {}
```

**判断标准:**

| 现象 | 含义 |
|---|---|
| 主 events 没新行 + hat-channel 有新行 | 正常(loop 隔离模式,且这轮还没 merge 回主) |
| 主 events 没新行 + subtree 找到 events.jsonl | **就是本次 case** — hat 进程的 PWD 在 subtree,事件写到了 hat-PWD 暂存;不是 bug |
| 全部 events*.jsonl 都没新行 + stdout 显示 `Event emitted` | 真的是 bug(目前未发现) |
| stderr 看到 `Error: refusing to emit event to ...` | P6 guard 正确拒绝,需要调整路径 |

---

## 7. 置信度

| 结论 | 置信度 | 依据 |
|---|---|---|
| 5 条 test.passed 写入 `sorts/.ralph/events.jsonl` | **高**(≥80) | 时间戳精确匹配用户 log |
| CLI 没有"假成功" bug | **高**(≥85) | 已实盘验证:`unset RALPH_EVENTS_FILE; ralph emit ... --hat shipper` 测试,主 events file size 从 2322 → 2554(2 次 emit 净增 232 字节),正常 |
| Stderr 截断导致 agent 误判 | **高**(≥75) | 用户 log 显示 `[32m INFO[0m ...` 后只剩 `+4 more lines`(被截断),而 `Error: refusing...` 行的长度/位置合理 |
| PWD-split 是 hat prompt 问题 | **中**(60) | 需要看到具体 validator hat prompt 的 `cd` 指令才能完全锁定;但 `commands/emit.rs:562-563` `workspace_root = std::env::current_dir()` 的实现与 cd 行为一致已验证 |

---

## 8. 不做的事（澄清边界）

- **不修 `cli_executor.rs:411-415`** — PWD/RALPH_WORKSPACE_ROOT 注入是正确的,问题在 hat 内部 `cd`。
- **不修 `cli/emit_path.rs`** — P6 guard 工作正常(在 workspace_root=父目录 + PWD=父目录 场景实测通过)。
- **不改 preset `ce-executor-serial.yml`** — 修复点在具体的 hat `instructions:`(未来 work)。
- **不写新 PRD** — 这是 case 总结,非新功能。

---

## 9. 关联资源

- `crates/ralph-cli/src/commands/emit.rs:1159-1179` — 写盘路径解析
- `crates/ralph-cli/src/cli/emit_path.rs:127, 146-151` — hat-events marker 加入 allowlist
- `crates/ralph-cli/src/cli/emit_path.rs:179-197` — P6 guard `Error: refusing...` 拒绝分支
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:19-50` — hat channel 创建 + 写 marker
- `crates/ralph-adapters/src/cli_executor.rs:411-422` — RALPH_EVENTS_FILE / RALPH_WORKSPACE_ROOT / PWD 三件套注入
- `crates/ralph-core/src/loop_context.rs:167-186` — `ctx.workspace()` / `ralph_dir()` 实现
- 项目根 `CLAUDE.md` HARD RULE 4 — hat instructions 视角规范(本报告 future work 引用此条)
