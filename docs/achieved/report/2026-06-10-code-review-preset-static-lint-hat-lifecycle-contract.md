---
title: "代码审查报告：preset-static-lint + hat-lifecycle-contract 两计划合入审核"
date: 2026-06-10
type: code-review
branch: pittcat-dev
base: main (0f407c6c)
head: b179e42
status: not-ready-to-merge
plans:
  - docs/plans/2026-06-08-003-feat-preset-static-lint-plan.md
  - docs/plans/2026-06-08-004-feat-hat-lifecycle-contract-plan.md
reviewers:
  - ce-correctness-reviewer
  - ce-adversarial-reviewer
  - ce-maintainability-reviewer
  - ce-project-standards-reviewer
  - ce-reliability-reviewer
  - ce-testing-reviewer
total_findings: 35
p0_count: 2
p1_count: 13
p2_count: 9
p3_count: 11
artifact_root: /tmp/compound-engineering/ce-code-review/20260609-cr-001/
---

# Ralph Orchestrator 代码审查报告（CE Code Review）

## 元信息

| 项 | 值 |
|---|---|
| 审查日期 | 2026-06-10 |
| 审查分支 | `pittcat-dev` |
| HEAD | `b179e42` |
| 相对 base | `main`（merge-base `0f407c6c`）|
| Scope | working tree 改动（10 文件）+ `main..HEAD` 上与两份计划相关的全部提交 |
| 评审方法 | CE Code Review skill（7 个并行 reviewer 子代理 + orchestrator 直接验证关键 P0/P1 finding）|
| **总体判定** | **🔴 Not Ready to Merge** |

## Plan 来源

- `docs/plans/2026-06-08-003-feat-preset-static-lint-plan.md`（preset static lint，U1–U6 + R1–R12）
- `docs/plans/2026-06-08-004-feat-hat-lifecycle-contract-plan.md`（hat lifecycle contract，U1–U6 + R1–R14）

## Reviewer 团队（并行 ce 子代理）

| Reviewer | 状态 | 原始 finding 数 |
|---|---|---|
| ce-correctness-reviewer (hat-lifecycle 域) | 完成 | 6 |
| ce-correctness-reviewer (preset-lint 域) | 完成 | 9 |
| ce-adversarial-reviewer | 完成 | 20 |
| ce-maintainability-reviewer | 完成 | 19 |
| ce-project-standards-reviewer | 完成 | 12 |
| ce-reliability-reviewer | 完成 | 11 |
| ce-testing-reviewer | 完成（重跑一次）| 12 |

去重 + cross-reviewer 提升 + 置信度阈值（≥75）后保留下来的 **actionable 主线 finding 共 35 条**（P0: 2、P1: 13、P2: 9、P3: 11）。

---

## 总结结论（先看）

**Verdict: Not Ready**（不建议直接合入 main）。

两份计划在文档层面写得很扎实，单元测试也覆盖到位，但 **关键集成路径上存在一个 P0 级正确性 bug**——`hat_lifecycle` tracker 的 `trigger_identity` 在 `activate` 与 `complete` 两端用了语义反转的查找逻辑（`can_publish` 而非 `can_subscribe`），导致 tracker 在生产路径上几乎永远无法关闭任何 activation，HashMap 单调增长，`diagnose --session latest` 显示的 `## Active Hat Activations` 会大量混入早已结束的 hat，让 R14 的"调试可观测性"目标实质失效。

下面四件 P1 配套问题同等需要修：

1. `merge-loop` preset 是靠 `exempt_findings` 白名单"通过"了 strict lint，并非真实满足 R10 全量迁移目标；
2. `active-activations.json` 仅在 loop **终止后**一次性落盘，与 plan U4 "卡住时实时可观测"的承诺直接冲突；
3. `runner.rs` 的 `std::process::exit(2)` 在 RAII Drop 链顺序错误的位置触发，未来加任何资源都会泄漏；
4. `bug.md` / `fix-log.md` 两个 agent 临时工作笔记被 commit 进了仓库，违反 `AGENTS.md` "MUST not commit ephemeral files"。

亮点（正向反馈见末尾）：plan 文档质量、tracker 单元测试完备性、CLI gate 的 fail-fast 顺序、`build_allowed_topics` 注释自反修正、Telegram bot/diagnostics 集成保留干净边界——这些都做得很好。

---

## P0 — Critical（必须在合入前修复）

| # | 文件 | 问题 | Reviewer | 置信度 |
|---|------|------|----------|------------|
| 1 | `crates/ralph-core/src/event_loop/mod.rs:2140-2162` & `:5009-5040` | `trigger_identity` 用 `can_publish` 反向计算，activate 与 complete key 永不匹配，tracker 永久 leak | correctness-hat, adversarial, correctness-preset, testing | 100 |
| 2 | `crates/ralph-core/src/event_loop/tests/hat_lifecycle_integration.rs` & `crates/ralph-core/tests/scenarios/preset_static_lint.yml` | 关键集成测试（T-U3-1..U3-7、AE2）全部手工调 tracker API，零测试真正跑 `process_events_from_jsonl`，bug #1 因此对回归无防护 | testing, correctness-hat | 100 |

### #1 详细说明 — trigger_identity 反向计算

- **现状**：`determine_active_hat_ids` 用 `registry.get_for_topic` 通过 **trigger subscription** 关系找到 hat。但 `mod.rs:2143` 的 activate 端用 `can_publish(hat_id, e.topic.as_str())` 找 trigger event，这是查 "hat 的 publishes 是否包含 e.topic"——而 trigger 是 hat 的**输入**事件，必然不在 publishes 里，所以 99% 的情况下 fallback 到字符串 `"unknown"`。
- **complete 端同样的错**：`mod.rs:5022` 也用 `can_publish`，但 fallback 到 `topic_str`（terminal event topic，如 `work.done`）。两端 key 不同 → `complete` 命中 `None` 分支只 `warn!` → tracker 中该 activation 保持 Active 永远存在。
- **复现路径**（以 ce-executor preset 为例）：
  - `executor` hat: `triggers: ["work.start"]`, `publishes: ["work.done"]`
  - event `work.start` 到来：`determine_active_hat_ids` → `[executor]`
  - activate 阶段：找 events 中能被 executor publish 的事件 → 没有（因为 publishes 只有 `work.done`）→ fallback `"unknown"`
  - tracker 存 `key.trigger_identity = "unknown"`
  - executor emit `work.done`：complete 阶段找 last_activation_events 中能被 executor publish 的事件 → 没有 → fallback `topic_str = "work.done"`
  - tracker 查 `key.trigger_identity = "work.done"`
  - **两端 key 完全不同** → complete 走 None 分支只 warn → tracker 永久 leak
- **修复方案**：在 `activate` 时把 `trigger_topic` 直接写到 `loop_state.last_activation_triggers: HashMap<HatId, String>`，complete 端按 `hat_id` 直接查表；或者更彻底——把 `ActivationKey` 改成只用 `(loop_id, iteration, hat_id)` 三元组，让 `trigger_identity` 退化为 snapshot 上的展示字段而非 hashmap key。

### #2 详细说明 — 测试假自信

- `hat_lifecycle_integration.rs` 中 7 个 `T-U3-*` 测试 100% 直接构造 `ActivationKey` 调 `tracker.activate(...) / complete(...)`，**从来没走 `event_loop::process_events_from_jsonl`**。
- `T-U3-7 complete_with_wrong_trigger_identity_does_not_close` 甚至 **无意中描述了生产 bug**：它把 `wrong_key.trigger_identity = "work.done"` 当反例断言"不会关闭"——但这正是生产代码 fallback 后实际生成的 key！
- `preset_static_lint.yml` 的 AE2 scenario 注释宣称 "no events.jsonl was created"，实际断言只检查 `error_count==1`，没访问 filesystem。
- **修复**：增加一个真正驱动 `process_events_from_jsonl` 的 e2e 测试，喂入 JSONL 事件，断言 `tracker.active_count() == 0` after terminal event。

---

## P1 — High（应该在合入前修复）

| # | 文件 | 问题 | Reviewer | 置信度 |
|---|------|------|----------|------------|
| 3 | `crates/ralph-cli/src/loop_runner/runner.rs:160-163` & `crates/ralph-core/src/diagnostics/mod.rs:554` | `active-activations.json` 仅在 loop 终止时落盘；卡住的 loop 永不生成此文件，违背 R14 "卡住时实时可观测" 价值主张 | correctness-hat, reliability | 90 |
| 4 | `crates/ralph-cli/src/presets.rs:4228-4264` & `presets/en/merge-loop.yml:293-297` | `merge-loop` preset 用 `exempt_findings` 白名单 3 类 finding 来"通过"strict lint，本质上承认 R10 "全部 9 个嵌入 preset 通过 strict" 未完成 | correctness-hat, correctness-preset, adversarial, maintainability | 90 |
| 5 | `crates/ralph-cli/src/loop_runner/runner.rs:256` | `std::process::exit(2)` 跳过 RAII Drop chain；tracing flush、`.ralph/` 局部状态、worktree lock 都会泄漏 | reliability, adversarial | 95 |
| 6 | `crates/ralph-core/src/hat_lifecycle.rs:286` | `complete()` 只把 state 改成 `Completed`，HashMap 条目永不 remove；长跑 loop 内存单调增长 + `active_count() / active_activations()` 遍历 O(N) | reliability | 95 |
| 7 | `bug.md`、`fix-log.md`（已 commit 入 HEAD） | 两份 agent 内部工作笔记被 commit 进仓库，违反 `AGENTS.md` line 435 "MUST not commit ephemeral files" | project-standards | 75 |
| 8 | `crates/ralph-core/src/event_policy.rs:204` & `event_loop/mod.rs:710` | 非法 topic 在 policy 阶段被 `Block(InvalidTopicFormat)` 后实际是 silently drop，与 plan R10 "write recovery signal" 字面承诺不一致 | reliability | 75 |
| 9 | `crates/ralph-core/src/preset_lint.rs`（全文 1757 行） | 单文件未按 U1/U2/U3 + tests 拆分，4 类规则 + 2 枚举 + 2 适配器 + 47 套测试堆在底部，未来演进互相阻塞 | maintainability | 75 |
| 10 | `crates/ralph-core/src/event_loop/mod.rs`（5400+ 行） | god module；hat lifecycle 集成逻辑内联在 line 2140-2162 与 5009-5040 两个相隔 3000 行的位置，`trigger_identity` 计算逻辑无法单测 | maintainability | 75 |
| 11 | `crates/ralph-core/src/hat_lifecycle.rs:191-197` | `ActivationState::Completed { completed_at, terminal_topic }` 整条 variant 全部为死代码（`#[allow(dead_code)]` 注释"Reserved for future"实际 reporter 没读） | maintainability | 75 |
| 12 | `crates/ralph-core/src/diagnosis/reporter.rs:1843-1849` | `looks_like_session_timestamp_edge_cases` 测试用例覆盖严重不足；未覆盖 18/20 字符、带毫秒、leap second、hex 字符、首字符非数字 | testing | 90 |
| 13 | `crates/ralph-core/src/hat_lifecycle.rs:tests` | FakeClock 测试缺关键边界：duration=0、advance 后回退、clock regress 产生负数 | testing | 95 |
| 14 | `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs:97-189` | R8 "artifact 写入失败不得掩盖 lint 失败"路径零测试；未模拟 `.ralph/diagnostics/` 不可写场景 | testing | 90 |
| 15 | `crates/ralph-cli/tests/integration_run.rs:211/319/389/463/539` | 5 处测试都被迫加 `topic_format_whitelist: [LOOP_COMPLETE]` + `tasks.enabled: false`，暗示现有用户配置无法不迁移就跑通新 gate，但**没有 migration 文档** | correctness-preset | 75 |

### 详细说明

- **#3 (active-activations 落盘时机)**：`runner.rs:160` 在 `write_termination_diagnostics()` **之后**才调 `event_loop.hat_lifecycle_tracker().active_activations()` 然后 `write_active_activations(&activations)`。`reporter.rs:454 read_active_activations` 读这个文件来渲染 `## Active Hat Activations`。结果是：用户跑 `ralph diagnose --session latest` 时如果 loop 还在运行（这恰好是 plan U4 写在文档里的核心场景"loop 跑着跑着卡住时"），文件根本不存在或来自上次 loop，section 要么空、要么显示陈旧数据。修复：要么把 tracker 序列化做成 SIGUSR/heartbeat 周期写盘，要么改成 `diagnose` 命令直接走 RPC/IPC 查 live loop。

- **#4 (merge-loop exempt)**：`merge-loop.yml:293-297` 的 `failure_handler` hat：`publishes: []`、无 `terminal_events`。同时 `MERGE_COMPLETE`（loop completion promise）没有任何 hat publish；`cleanup.done` 在 `cleaner` hat 发布但全集 preset 中没有订阅者。`presets.rs:4228-4240` 加了 3 个豁免对应这三个缺陷。这与 plan R10 字面承诺直接冲突。补救：要么真正迁移 `failure_handler`（给一个 `merge.handled` terminal event 并把它列在 completion_promise 候选里），要么在 plan 003 的 deferred section 写明"merge-loop 的 failure_handler 因业务语义豁免"并把 exempt 改成显式 plan-back-reference。

- **#5 (process::exit)**：`process::exit(2)` 在 `runner.rs:256` 直接结束进程。当前还没有需要 Drop 的资源（这是侥幸），但 plan U4 已经在 `runner.rs:160` 写了"在 termination diagnostics 之后落盘 active-activations"——只要 gate 失败发生在那一行**之前**，落盘就被跳过。更严重：未来在 `run_loop_impl` 顶部加任何 `tempdir::TempDir` / `LockGuard` / `tracing::subscriber::with_default` 都会泄漏。修复：让 `enforce_preset_lint_gate` 返回 `Err(...)`，让 `main.rs` 在最外层根据错误类型决定 exit code。

- **#6 (HashMap 单调增长)**：`hat_lifecycle.rs:286` 的 `complete` 把 `Active` 改写为 `Completed { ... }` 但 HashMap entry 仍在。`total_count()` 单调递增；`active_activations()` 每次都得 filter 跑全表。短跑 loop 影响不大，长跑（数千 iteration）会出现真实内存增长 + 每次 reporter 拉取变慢。修复：`complete` 直接 `self.activations.remove(key)`，并维护一个独立 `completed_count: usize` 字段供调试。

- **#7 (ephemeral 文件)**：`git ls-tree HEAD` 确认 `bug.md` (e8b81864) 与 `fix-log.md` (ff617fab) 都在 HEAD 中。`bug.md` 内容是 "工作已完成（commit 14db274）..."，明显是 ce-executor 中间状态；`fix-log.md` 是 fixer agent 的 round-tracking。`commit af7d1d2` 历史上已删除过 `bug.md`，`commit 6924f2b` 通过 auto-commit hook 又把它纳回来——形成对冲。修复：`git rm bug.md fix-log.md && git commit -m "chore: drop ephemeral agent notes"`，并把这两个名字加进 `.gitignore`。同时建议在 auto-commit hook 加 ephemeral 文件名 deny-list。

- **#8 (silently drop)**：`event_policy.rs:204 check_topic_format` 返回 `Some(EventDecision::Block { violation_type: InvalidTopicFormat })`。`event_loop/mod.rs:758` 的 `policy_finding_for` 对 `InvalidTopicFormat` 直接 `return None`，意味着不产生 recovery envelope 也不写 journal。plan R10 写的是"非法 topic 不自动 retry，**只写 recovery signal**"——目前是 silently drop，连 signal 都没有。修复：在 mod.rs:710 附近 `apply_event_policy_validation` 的 Block 分支为 `InvalidTopicFormat` 单独构造一个 `RecoveryDiagnosisEnvelope` 写入 `recovery.jsonl`。

- **#9 / #10 / #11 (结构债)**：三条结构债。`preset_lint.rs` 拆成 `preset_lint/{mod.rs, topic_format.rs, ownership.rs, coordinator.rs, finding_id.rs, tests/}` 是机械物理切分，本次合入前完成成本最低。`event_loop/mod.rs` 5400 行可以先把 hat lifecycle 集成抽到 `event_loop/hat_lifecycle_integration.rs` 子模块，至少让 `trigger_identity` 计算可以单测覆盖。`ActivationState::Completed.completed_at / terminal_topic` 既然 reporter 不读，直接删除该 variant，complete 就改为 `HashMap::remove`（同时修了 #6）。

- **#12 / #13 / #14 (测试 gap)**：三条测试 gap。`looks_like_session_timestamp` 用 `19 && bytes[10]==b'T' && bytes[13]==b'-' && bytes[16]==b'-'` 之类的浅判断，但只有 5 个断言；需补"恶意输入"集合（包含 unicode、tz、leap second）。FakeClock 测试缺"advance 后回退"——这本来就是 line 336 `unwrap_or(Duration::ZERO)` fallback 存在的理由，必须有反向测试守住 fallback 真触发。`write_preset_lint_artifact` 4 处错误分支都没单测；建议用 `tempfile + chmod 0o000` 模拟不可写。

- **#15 (migration 文档缺失)**：`integration_run.rs` 加 5 处 `topic_format_whitelist: - LOOP_COMPLETE` + `tasks: enabled: false`，说明现有用户配置（默认有 `LOOP_COMPLETE` 协议字、默认 tasks.enabled=true）一旦升级到新 gate 就 fail。`docs/guide/preset-authoring.md` 与 `docs/guide/runtime-contracts.md` 当前未给迁移路径。补救：在 `docs/guide/preset-authoring.md` 加 "Migration: upgrading to topic_format gate" 段，给出 3 步 fix（加 whitelist / 加 coordinator / 加 terminal_events）。

---

## P2 — Moderate（建议在下个迭代修）

| # | 文件 | 问题 | Reviewer | 置信度 |
|---|------|------|----------|------------|
| 16 | `crates/ralph-cli/src/presets.rs:4234-4239` | `exempt_findings` 用 `id.starts_with(prefix)` 前缀匹配，未来引入 sibling id（如 `config.empty_terminal_events_v2`）会被静默吞掉；改成完全匹配 | correctness-preset, adversarial | 80 |
| 17 | `crates/ralph-core/src/hat_lifecycle.rs:320-353 active_activations()` | 排序 by duration 在两条 activation 同时刻 activate 时不稳定；reporter 输出可能 flaky；用 `(duration, hat_id)` 二级 key 排序 | adversarial | 75 |
| 18 | `crates/ralph-core/src/event_loop/tests/hat_lifecycle_integration.rs:126-148` | `T-U3-4 decision_path_does_not_read_tracker` 是空壳——注释自承"this is enforced by code review"，仅断言 read API 存在；改成 instrumented mock tracker，记录调用栈 | testing, correctness-hat | 95 |
| 19 | `crates/ralph-core/src/hat_lifecycle.rs:286-313` | `complete` 函数第二次 `self.activations.get_mut(key).unwrap()` 依赖 NLL 隐式 drop 第一个 borrow；建议 take + insert 或显式 `*entry = Completed{...}` | correctness-hat | 75 |
| 20 | `crates/ralph-core/src/event_policy.rs:240-278 build_allowed_topics` | 注释自承"原注释错误"但无回归测试守住 `is_system_topic` 在 `check_topic_format` **之前**的调用顺序不变；加 `#[test] system_topic_short_circuit_runs_before_format_check` | correctness-hat | 70 |
| 21 | `crates/ralph-core/src/preset_lint.rs:494-515` | R4 检查的新加 `if publishers.is_empty() && !owners.is_empty()` 与 R2 检查可能对同一 topic 触发 1+N 条 finding（一条"无 publisher"+ N 条"owner 不在 publishes"），冗余 | correctness-preset | 50 |
| 22 | `crates/ralph-cli/src/presets.rs:4234-4239` | 例外列表内嵌在测试中、注释用英文且无 ticket/issue 引用、`TOPOLOGY_EXEMPT_PRESETS` 常量被 doc 引用但实际不存在 | maintainability, project-standards | 75 |
| 23 | `crates/ralph-core/src/hat_lifecycle.rs:151-168` & U2 plan | `ActivationSnapshot.linked_task_id: Option<String>` 用 String，plan U2 承诺 `Option<TaskId>`；且生产路径 `mod.rs:2160` 永远传 `None`——R14 表格中 Task 列永远空 | maintainability | 75 |
| 24 | `crates/ralph-core/src/diagnosis/reporter.rs:200` & `runner.rs:160` | plan 004 "tracker 唯一消费方是 diagnose reporter" 边界已被 `runner.rs:160` 落盘 + `reporter.rs:200` 序列化暗中拓宽到"落盘文件"；plan 与代码语义偏差，需在 plan 或代码中明示 | maintainability | 50 |

### 详细说明（精选）

- **#16 修复示例**：
  ```rust
  // before
  let is_id_exempt = exempt_findings.iter().any(|(name, id_prefix)|
      *name == preset.name && f.id.starts_with(id_prefix));
  // after
  const EXEMPT_FINDINGS: &[(&str, &str)] = &[
      ("merge-loop", "config.empty_terminal_events"),
      // ...
  ];
  let is_id_exempt = EXEMPT_FINDINGS.iter().any(|(name, id)|
      *name == preset.name && *id == f.id.as_str());
  ```

- **#17 修复示例**：
  ```rust
  // before
  snapshots.sort_by_key(|s| std::cmp::Reverse(s.duration));
  // after
  snapshots.sort_by(|a, b|
      b.duration.cmp(&a.duration).then(a.hat_id.cmp(&b.hat_id)));
  ```

- **#19 修复示例**：
  ```rust
  match self.activations.get_mut(key) {
      Some(state @ ActivationState::Active { .. }) => {
          let now = self.clock.now();
          *state = ActivationState::Completed {
              completed_at: now,
              terminal_topic: terminal_topic.to_string(),
          };
      }
      // ...
  }
  ```

- **#20 测试模板**：
  ```rust
  #[test]
  fn system_topics_bypass_format_check_via_is_system_topic() {
      let allowed = build_allowed_topics(&HashMap::new(), "loop.complete");
      // 大写 event.* 应该被 is_system_topic 短路接受
      assert!(!check_topic_format("event.foo.BAR", &allowed)
          .is_some_and(|d| matches!(d, EventDecision::Block { .. })));
      assert!(is_system_topic("event.foo.BAR"));
  }
  ```

---

## P3 — Low（taste / cleanup）

| # | 文件 | 问题 | Reviewer |
|---|------|------|----------|
| 25 | `crates/ralph-core/src/preset_lint.rs:494-515` | `!owners.is_empty()` 守卫位于 `for (topic, owners) in &config.topic_owners` 循环内，是死分支（HashMap value 不会是空 vec 除非显式插入） | correctness-preset |
| 26 | `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs:103` | artifact 文件名仅使用毫秒级时间戳，多进程并发 lint 可能撞名；加 PID 或随机后缀 | reliability, adversarial |
| 27 | `crates/ralph-core/src/hat_lifecycle.rs:61-95 FakeClock` | `Rc<Cell<>>` 仅单线程；如果未来 tracker 跨 tokio task 用，FakeClock 不能复用；至少在文档里写明 "FakeClock is single-threaded only" | reliability, maintainability |
| 28 | `crates/ralph-core/src/hat_lifecycle.rs:74` | `FakeClock::fixed()` 注释从 "2026-01-01" 改为 "2025-01-01"，但常量 1_735_689_600 对应的是 **2024-12-31 16:00:00 UTC**（差 8 小时）；如果是想表达"2025-01-01 UTC"则应改为 1_735_689_600 + 28800？建议精确算一遍并加 const_assert | adversarial, maintainability |
| 29 | `presets/en/hatless-baseline.yml` | 文件存在但 `manifest.yml` 注释掉，三态语义不明；文档未说明此状态 | project-standards |
| 30 | `crates/ralph-core/src/diagnosis/reporter.rs` | 段标题（`## Run summary`, `## Active Hat Activations`...）全英文；与 reporter 内 `_无 recovery journal。_` 等中文提示语言不一致 | project-standards |
| 31 | `crates/ralph-core/src/preset_lint.rs:494` | R4 finding message 全英文 `"topic ... is declared in topic_owners ..."`；与 reporter 中文提示不一致 | project-standards |
| 32 | `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` 等 | frontmatter 缺 `tags` 字段，与 `AGENTS.md` line 167 要求不符 | project-standards |
| 33 | `scripts/ralph-zsh-plugin.zsh:53-61` | `_RALPH_BUILTIN_HAT_VALUES` 列出 `builtin:merge-loop`，但 `presets.rs` 中 merge-loop 是 `public: false`；用户能 TAB 补全到一个 `ralph preset list` 看不到的 preset | project-standards |
| 34 | `crates/ralph-core/src/event_loop/tests/hat_lifecycle_integration.rs` 与 `hat_lifecycle.rs:tests` | 命名 `T-U2-N` / `T-U3-N` / `Additional:` / 无前缀混杂，分层不清 | maintainability |
| 35 | `crates/ralph-core/src/hat_lifecycle.rs` 测试占 43% (line 382-674) | 与 `event_loop/tests/hat_lifecycle_integration.rs` 重叠覆盖 ~10 处断言；建议把内部 unit 留下、把"消费 EventLoop tracker"的迁出 | maintainability, testing |

---

## Requirements Completeness（Plan 对照）

> 来源：plan 是 explicit（caller 显式指定），所以未达成项按 P1 处理。

### Plan 003 – Preset Static Lint

| R-ID | 描述 | 状态 | 关联 finding |
|---|---|---|---|
| R1 | RalphConfig 支持 topic_owners + topic_format_whitelist | ✅ 已实现 | — |
| R2 | owner 引用必须指向已声明 hat、且在 publishes 中 | ✅ 已实现 | — |
| R3 | 非 owner 发布 owner topic → `cross_hat_unauthorized_publish` | ✅ 已实现 | — |
| R4 | 所有 authoring topic 使用统一 lowercase dot-case 校验 | ⚠️ working tree 补 R4 新检查但与 R2 重复 | #21 |
| R5 | tasks.enabled=true 必须存在 coordinator_hats | ✅ 已实现 | — |
| R6 | 三入口（`preset check` / `hats validate` / `run`）共享同一 report | ⚠️ `hats validate` 默认未调 `run_preset_lint`（line 230-231 注释承认 backward compat），与字面表述偏差 | — |
| R7 | `ralph run` 在 backend / TUI / events 文件前 fail-fast、退出码 2 | ✅ 已实现（runner.rs:248-257 在 setup_process_group:261 与 backend spawn:642 之前）| — |
| R8 | lint 失败同时输出人类格式 + JSON artifact；artifact 写入失败不掩盖 lint 失败 | ⚠️ 实现合规，但 artifact 失败路径零测试 | #14 |
| R9 | lint 只读，不修改 preset 或自动迁移 | ✅ | — |
| R10 | manifest 9 个嵌入 preset 均通过 strict lint | ❌ 未真正达成，靠 exempt_findings 绕过 | #4 |
| R11 | PRESETS 名称集合 + canonical 文件 + 嵌入内容一致性测试 | ✅ | — |
| R12 | 修改 `ralph hats validate` 语法后同步 `ralph-tools.md` 与源码引用 | ⚠️ ralph-tools.md 引用 `hats.rs:170` 行号需反向验证 | — |

### Plan 004 – Hat Lifecycle Contract

| R-ID | 描述 | 状态 | 关联 finding |
|---|---|---|---|
| R1 | HatConfig 支持非空 terminal_events 集合，单字符串 alias | ✅ | — |
| R2 | 每个 terminal topic 必须在该 hat 的 publishes 中 | ✅ | — |
| R3 | lifecycle tracking 以 activation 为单位，暴露 `active_activations()` query API | ⚠️ API 实现 OK，但 trigger_identity bug 让生产路径几乎永远 leak active 项 | #1 |
| R9 | 所有 JSONL agent event 在 payload policy 前执行 topic 格式检查 | ✅ | — |
| R10 | topic 格式拒绝不自动 retry，只写 recovery signal | ❌ 当前是 silently drop，**没写 signal** | #8 |
| R11 | runtime owner check 仅在 ownership 配置可用时启用 | ✅ | — |
| R12 | manifest 全部嵌入 preset 补齐 terminal sets | ⚠️ merge-loop failure_handler 缺 terminal_events，靠 exempt 通过 | #4 |
| R13 | topic 格式拒绝复用 RecoveryDiagnosisEnvelope | ⚠️ 由于 R10 没真正实现，R13 复用也未生效 | — |
| R14 | `ralph diagnose --session latest` 输出 `## Active Hat Activations`，列出 active hat、时长、最后事件、task | ⚠️ section 渲染实现 OK，但数据源是终止时落盘文件，"卡住时实时可观测"目标失效 | #3 |

**未达成 / 部分达成汇总**：

- plan 003：R6（轻度偏差）、R10（重要未达成）、R12（待验证）
- plan 004：R3（实质失效）、R10（重要未达成）、R12（绕过通过）、R13（依赖失效）、R14（实时性失效）

按 explicit-plan 路由全部按 P1 提报，已包含在 finding 列表中。

---

## Actionable Findings 汇总（合并 routing）

| # | 严重度 | 文件:行 | 标题 | Owner | 含 suggested_fix |
|---|---|---|---|---|---|
| 1 | P0 | `event_loop/mod.rs:2143,5022` | trigger_identity 反向计算导致 tracker 永久 leak | downstream-resolver | 是 |
| 2 | P0 | `hat_lifecycle_integration.rs:*` + `preset_static_lint.yml` AE2 | 测试假自信，未真跑 event loop | downstream-resolver | 是 |
| 3 | P1 | `runner.rs:160`, `diagnostics/mod.rs:554` | active-activations 仅终止时落盘 | downstream-resolver | 是 |
| 4 | P1 | `presets.rs:4228-4264`, `merge-loop.yml:293-297` | merge-loop 用 exempt 绕过 strict | downstream-resolver | 是 |
| 5 | P1 | `runner.rs:256` | std::process::exit(2) 跳过 RAII | downstream-resolver | 是 |
| 6 | P1 | `hat_lifecycle.rs:286-313` | HashMap 单调增长无 GC | downstream-resolver | 是 |
| 7 | P1 | `bug.md`, `fix-log.md` | ephemeral 文件被 commit | downstream-resolver | 是 |
| 8 | P1 | `event_policy.rs:204`, `event_loop/mod.rs:710` | InvalidTopicFormat silently drop | downstream-resolver | 是 |
| 9 | P1 | `preset_lint.rs` 全文 | 1757 行单文件未拆 | human | 是 |
| 10 | P1 | `event_loop/mod.rs` 全文 | 5400+ 行 god module | human | 是 |
| 11 | P1 | `hat_lifecycle.rs:191-197` | ActivationState::Completed 死代码 | downstream-resolver | 是 |
| 12 | P1 | `diagnosis/reporter.rs:1843` | timestamp 边界测试不足 | downstream-resolver | 是 |
| 13 | P1 | `hat_lifecycle.rs:tests` | FakeClock 边界缺失 | downstream-resolver | 是 |
| 14 | P1 | `preset_lint_gate.rs:97-189` | artifact 写入失败路径零测试 | downstream-resolver | 是 |
| 15 | P1 | `integration_run.rs:211-539`, `docs/guide/` | 迁移文档缺失 | downstream-resolver / human | 是 |

P2/P3 暂留 backlog，可与 P1 同 PR 顺手修。

---

## Coverage（流程层面）

- **Reviewer team**：7 个并行 ce 子代理（4 always-on + 3 conditional：security 因 diff 不涉及 auth/支付被略过，adversarial 与 reliability 因 100+ 行可执行代码 + 后台行为命中触发，previous-comments 因无 PR comments 不触发）。
- **Validator 直接 orchestrator 验证**：本次对 P0/P1 关键 finding（#1, #2, #3, #4, #5, #6, #7, #8）通过原文件直接读取交叉验证，全部 confirmed。P2/P3 采用 reviewer agent 自身 evidence 字段判断。
- **Suppressed**：confidence < 75 的 P3 advisory 4 条（adversarial 中 wave.*/task.* 误拒、session_player 空 tracker、artifact 撞名 P3 子集），已合并到 residual_risks。
- **Mode-aware demotion**：testing/maintainability-only 的 P3 advisory 2 条已降到 `testing_gaps` / `residual_risks`。
- **失败 reviewer**：`ce-testing-reviewer` 第一轮因 socket connection 断开失败，已重跑一次，无后续 retry。

### Residual Risks（未升级到 finding）

- "tracker 唯一消费方是 diagnose reporter" 边界被 `runner.rs:160` + `reporter.rs:200` 暗中扩展，与 plan 004 显式承诺漂移。
- `hat_lifecycle.rs` 单测 + `hat_lifecycle_integration.rs` 集成测试共 25 个，重复断言 ~10 处；维护成本但不阻塞。
- replay fixture 测试 tracker 状态总是空，与 historical "active" diagnose 期望错位（design vs bug 模糊）。

### Testing Gaps（未升级）

- 端到端 event loop lifecycle 测试完全缺失（被 finding #2 覆盖）。
- R6 三入口一致性无测试。
- preset migration 无 negative test（"故意让 ce-executor.yml 缺 terminal_events"）。
- `LintStrictness::Default vs Strict` 切换未对每条规则覆盖。
- `ActivationSnapshot` JSON round-trip 单测缺失。

---

## 亮点（正向反馈）

代码审查里值得留住的好东西，按 reviewer 一致认可整理：

1. **Plan 文档质量**：两份 plan 写得非常扎实。`hat-lifecycle-contract-plan.md` 把"两条路径不相连"（写 vs 读）用 mermaid 画清楚，并显式列出 "out of scope: tracker 在 event loop 决策路径上的任何读访问"，这是阻止未来反馈环的强约束。`preset-static-lint-plan.md` 明确处理了需求文档中的语义冲突（独占 publisher vs 多 publisher），并选定 strict 模式独占 + 非 strict warning 作为迁移路径——decision rationale 清晰。
2. **Tracker 状态机单元测试完备性**：`hat_lifecycle.rs:tests` 18 个 unit test 覆盖 T-U2-1..U2-8 全部场景 + idempotent + duplicate activate + parallel activations + clock-injected duration——是项目里少有的"测试驱动"的模块。FakeClock 用 `Rc<Cell<>>` 让 clone 共享时间状态，这是测试设计上的小巧思（虽然单线程限制需文档化）。
3. **CLI gate fail-fast 顺序正确**：`runner.rs:248-257 enforce_preset_lint_gate` 调用位置在 `setup_process_group(261)` 与 backend spawn(642) **之前**——R7 字面合规。`preset_lint_gate.rs:142-171` 用 `tempfile::Builder + persist` 实现 artifact 原子写，符合 R8 "防止 partial reads" 要求。
4. **`build_allowed_topics` 注释自反修正**：working tree 主动把"event.*/human.* 存为 prefix"改为"通过 is_system_topic 单独处理"，并加注释解释原文档错误。诚实的文档修复比"装作一切都对"重要。
5. **CLAUDE.md / AGENTS.md 字节一致**：`diff CLAUDE.md AGENTS.md` 返回空，两份文档在本分支保持完全一致——同步规则被严格遵守。
6. **Preflight 集成测试加上 CwdGuard**：`b179e42 test(summary-writer)` 用 CwdGuard 隔离测试 cwd，防止 `.ralph/events.jsonl` 污染——working tree 上层 commit 已经在主动收紧 test 隔离。
7. **reporter 渲染 sort 逻辑独立**：`reporter.rs:211-212` 重新 sort 一次 `active_activations.sort_by_key(|a| std::cmp::Reverse(a.duration))`，不依赖 tracker 排序顺序——双重保险，对后续 tracker 输出顺序的不确定性有冗余防护。

---

## Verdict

**🔴 Not ready to merge.**

合入前必须修复的固定项（按 fix 顺序）：

1. **#1 trigger_identity 反向计算**（P0）—— 修复 hat_lifecycle 的核心可观测性目标，否则 plan 004 R3/R14 全部失效。
2. **#2 测试假自信**（P0）—— 加入一个真正驱动 `process_events_from_jsonl` 的 e2e 测试，用真实 JSONL fixture 验证 tracker 在 terminal event 后 `active_count() == 0`。这条 test 会同时充当 #1 的回归 gate。
3. **#7 ephemeral 文件**（P1）—— 一行 `git rm` 的事，不解决就是 AGENTS.md 强制约束的直接违规。
4. **#4 merge-loop exempt 真实债务**（P1）—— 要么真正迁移 failure_handler，要么在 plan 003 加 deferred section 写明业务豁免并把 exempt 改成 plan-back-reference。
5. **#5 process::exit + #6 HashMap 不 GC + #3 落盘语义 + #8 silently drop**（P1）—— 这四条形成"运行时可靠性"集群，建议同 PR 一起处理。
6. **#9/#10/#11 结构债**（P1）—— 不严格阻塞合入，但本次 PR 是最便宜的窗口，否则下次改 preset_lint 或 event_loop 就更难拆。
7. **#12/#13/#14 测试覆盖**（P1）—— 至少 #14 R8 path 必须补，#12/#13 可拖到下一个迭代。
8. **#15 迁移文档**（P1）—— 写 ~10 行 migration guide 加到 `docs/guide/preset-authoring.md`，让现有用户升级时知道要做什么。

P2/P3 视精力随手修；如果要分两轮 PR，第一轮聚焦 #1-#8，第二轮做结构 + 测试 + cleanup。

---

## Run Artifacts

完整 reviewer 输出（机器可读 JSON）已落盘到：

```
/tmp/compound-engineering/ce-code-review/20260609-cr-001/
├── correctness-hat-lifecycle.json     # 6 findings
├── correctness-preset-lint.json       # 9 findings
├── adversarial.json                   # 20 findings
├── maintainability.json               # 19 findings
├── project-standards.json             # 12 findings
├── reliability.json                   # 11 findings
├── testing.json                       # 12 findings
├── full.diff                          # 完整 diff snapshot
└── metadata.json                      # HEAD/branch/verdict
```

如需针对某一条 finding 给出具体 patch、或者继续验证 P2/P3 中的细项，告诉我编号即可。

---

## 附录 A：关键代码位置速查

| 关注点 | 位置 |
|---|---|
| trigger_identity activate 端 | `crates/ralph-core/src/event_loop/mod.rs:2140-2162` |
| trigger_identity complete 端 | `crates/ralph-core/src/event_loop/mod.rs:5009-5040` |
| tracker complete 状态机 | `crates/ralph-core/src/hat_lifecycle.rs:286-313` |
| active activations 落盘 | `crates/ralph-cli/src/loop_runner/runner.rs:160-163` |
| active activations 写入实现 | `crates/ralph-core/src/diagnostics/mod.rs:554-582` |
| 启动 lint gate | `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs:57-87` |
| process::exit 调用点 | `crates/ralph-cli/src/loop_runner/runner.rs:256` |
| artifact 写入 | `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs:97-189` |
| merge-loop exempt 例外 | `crates/ralph-cli/src/presets.rs:4228-4264` |
| merge-loop failure_handler | `presets/en/merge-loop.yml:293-297` |
| topic format silently drop | `crates/ralph-core/src/event_policy.rs:204` + `event_loop/mod.rs:758,710` |
| ActivationState dead variant | `crates/ralph-core/src/hat_lifecycle.rs:191-197` |
| FakeClock 实现 | `crates/ralph-core/src/hat_lifecycle.rs:61-95` |
| ephemeral 违规文件 | repo 根 `bug.md`、`fix-log.md` |

## 附录 B：Plan 与 Commit 对应

| Commit | Plan / Unit |
|---|---|
| `f372342 feat(preset-lint): U1` | plan 003 U1 配置模型与 topic 枚举 |
| `4d71db8 feat(preset-lint): U2` | plan 003 U2 ownership 与 coordinator 规则 |
| `bb70b71 feat(preset-lint): U3` | plan 003 U3 topic 格式规则 |
| `3eaf652 feat(preset-lint): U4` | plan 003 U4 CLI 输出 + exit code 2 |
| `f876241 feat(preset-lint): U5` | plan 003 U5 内置 preset strict 迁移 |
| `1049739 feat(preset-lint): U6` | plan 003 U6 BDD 与回放门禁覆盖 |
| `e8ecdca feat(hat-config): U1` | plan 004 U1 lifecycle terminal_events 配置 |
| `55f2940 feat(hat-lifecycle): U2` | plan 004 U2 activation tracker |
| `8368c42 fix(hat-lifecycle): U2 review` | plan 004 U2 review findings 修复 |
| `20dbf3c feat(hat-lifecycle): U3` | plan 004 U3 event loop 集成 |
| `def2855 feat(hat-lifecycle): U4` | plan 004 U4 diagnose 报告 |
| `0f87a... feat(hat-lifecycle): U5` | plan 004 U5 runtime topic 格式拒绝 |
| `fc9dec4 feat(hat-lifecycle): U6` | plan 004 U6 preset 迁移 |
| `3b4dd4e docs(plan)` | plan 004 标记 completed |
| `6924f2b chore: auto-commit before merge` | working tree 集成测试 + 一些 fix（**含 ephemeral 文件回吐**）|
| `b179e42 test(summary-writer)` | CwdGuard 隔离 |
