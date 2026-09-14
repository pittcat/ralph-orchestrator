---
name: ralph-tools-workstate
description: Use when reading or writing loop-scoped workstate during a Ralph orchestration run
metadata:
  internal: true
---

# Ralph Tools — Workstate

工作态（workstate）是**当前 loop 内**的 key-value 中间态存储：同一个 loop 里的所有 hat 都能读写，新 loop 启动时从空开始。它与记忆（memories）分层：memories 是跨 loop 保留的长期知识库；workstate 是本次执行的草稿与中间结论，生命周期跟随当前 loop。

## Workstate Commands

```bash
ralph tools workstate set <key> <value>   # 写入；同 key 覆盖（读取时永远得到最新一次写入）
ralph tools workstate get <key>           # 读取单个 key，stdout 输出值原文
ralph tools workstate list                # 列出当前 loop 的全部 key（每行 `key<TAB>更新时间戳`）
ralph tools workstate delete <key>        # 删除当前 loop 的一个 key
```

- `--root` 是 `ralph tools` 命名空间共享的工作目录选项，一般无需显式指定。
- key 不能为空，且不能包含空白或控制字符；value 最长 10 000 字符。违反时命令非零退出并在 stderr 说明原因，且不写入任何内容。
- **loop 作用域**：loop 内（runner 已注入 `RALPH_CURRENT_LOOP_ID`）的操作只影响当前 loop，读不到其它 loop 的 key；loop 外人工执行操作的是无 loop 归属的独立作用域，与任何 loop 互不可见。
- `get` 一个不存在的 key 会非零退出；不确定有哪些 key 时先 `list`。

## 何时写 workstate，何时写 memories

- 写 workstate：本轮 loop 的中间结论、待验证假设、需要跨 hat 传递的草稿状态（例如 `方案A待验证`）。
- 写 memories：跨 loop 仍然成立的长期知识（代码库惯例、架构决策、可复用的修复经验）。
- 不要把一次性中间态写进 memories（污染长期知识库）；也不要把需要跨 loop 保留的知识只写进 workstate（loop 结束后不再可见）。

## `## WORKSTATE` 注入块

- **触发条件**：当前 loop 至少有一条工作态，且配置 `workstate.enabled: true`、`workstate.inject: auto`（均为默认值）。当前 loop 没有任何工作态时该块不出现。
- **agent 动作**：把该块当作当前 loop 已登记的中间态清单来读，每行是 `- key: value`。需要更新某条时用 `ralph tools workstate set` 覆盖同 key，不要推断条目是何时由谁写入的。
- **截断**：内容超出注入预算时，块在条目边界截断并附 `<!-- truncated: ... -->` 标记；被截掉的条目用 `ralph tools workstate list` / `ralph tools workstate get` 读取全文。
- **失败停止条件**：块内容与 `ralph tools workstate list` 的输出不一致时，以 CLI 输出为准；仍无法确认当前中间态时，停止本轮写入并报告阻塞原因。
