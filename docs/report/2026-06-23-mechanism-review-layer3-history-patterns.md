# 机制层审查 - 第 3 层:历史模式对照

> **范围**: 历史诊断报告(2026-06-16 ~ 2026-06-22)+ 本次对抗性审查报告(2026-06-23)
> **方法**: ripgrep 关键词扫描,只列路径+行号

## 扫描关键词
- filename_mismatch / hat_handoff_filename
- typed 路由 / LintResumeHint / recovery 升级
- stall detector / 死信
- iter/seq 计数器耦合

## 历史命中文件清单(2026-06-16 ~ 2026-06-22,排除本次 2026-06-23)

| 关键词 | 报告文件路径 | 命中行号 |
|--------|--------------|----------|
| filename_mismatch | `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` | 50, 54, 332, 500 |
| filename_mismatch | `docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md`(引用源) | (ref) |
| filename_mismatch | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md`(引用源) | (ref) |
| typed 路由 / LintResumeHint | `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` | 17, 20, 112, 165, 168, 323, 401, 489, 557 |
| typed 路由 / LintResumeHint | `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` | 20, 197, 248 |
| typed 路由 / LintResumeHint | `docs/report/2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md` | 17 |
| typed 路由 / LintResumeHint | `docs/report/2026-06-18-003-base-stability-implementation-paths.md` | 294 |
| stall detector / 死信 | `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` | 377, 381, 382, 384, 386, 408, 425, 437, 448 |
| stall detector / 死信 | `docs/report/2026-06-17-ce-executor-wave-abstraction-issues-diagnosis.md` | 51 |
| stall detector / 死信 | `docs/report/2026-06-18-003-base-stability-optimization-report.md` | 74, 372 |
| stall detector / 死信 | `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` | 177, 301, 429, 443, 446 |
| iter/seq 耦合 | `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` | 108, 209, 251, 285 |
| iter/seq 耦合 | `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` | 179, 517, 519 |

## 重复次数统计

- **filename_mismatch**: 2 份直接报告 + 4 份引用(merry-lotus / noble-peacock / perky-maple / warm-tiger / primary-20260619) → **6 次复发**(本次为第 6 次)
- **typed 路由缺失**(LintResumeHint 字符串匹配): 4 份直接报告 + 多次 P0-3/P0-6 标记 → **5 次以上反复标记未闭环**
- **stall detector 漏报**("rejection noise 误识别为 event stall"): 4 份直接报告(keen-fern / wave-abstraction / perky-maple / warm-tiger) → **4 次复发**
- **iter/seq 耦合**(LoopState 计数器 vs handoff 文件名): 2 份直接报告 + 3 次同源补丁记录 → **30 天内第 6 次复发**

## 历史反模式归纳

### 反模式 1: hat_handoff filename_mismatch(iter/seq 漂移)
- **历史出现次数**: 6 次(merry-lotus / noble-peacock / perky-maple / warm-tiger / primary-20260619 / 本次)
- **首次出现**: `2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` §1 P0-A
- **本次是否修复**: 否(本次为第 6 次复发,plan 2026-06-21-001 U1 仍 active 未落地)
- **本次是否引入新变种**: 是 — 同一 root cause 在 `0-1-...md` / `1-1-...md` / `0-2-...md` 三种文件名组合上漂移,印证 `loop_state.rs:485-487` 的 `hat_handoff_seq` 0-init 边界设计错误

### 反模式 2: LintResumeHint 字符串匹配(typed 路由缺失)
- **历史出现次数**: 5 次以上(`keen-fern` / `perky-maple` / `warm-tiger` / 6-21 audit P0-6 / 本次 6-23 P0-2)
- **首次出现**: `2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` P2(明确指出 "把 `payload_contract_violation` 升级为 loop-fatal" 缺)
- **本次是否修复**: 部分(`gate.rs:509-531` 实现了 `filename_seq_mismatch_carries_typed_kind`,但 `LintResumeHint::from_typed_rejection` 实际未被 caller 链入,`event_loop/mod.rs:6074` 仍走字符串匹配)
- **本次是否引入新变种**: 是 — 暴露 docstring/代码语义漂移(`gates.rs` 写"升级链路就绪"实际 caller 没接),且 typed kind 字段加在 `recovery.jsonl` envelope 但消费者没读

### 反模式 3: stall detector 沉默(rejection noise 不被识别)
- **历史出现次数**: 4 次(keen-fern / wave-abstraction / perky-maple 6h+ / warm-tiger)
- **首次出现**: `2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` §377-386(明确指出 "stall detector 语义偏窄,只检测 3 轮无 business event,把 rejection noise 当 event")
- **本次是否修复**: 否(本次 8h+ 0 报警,等用户 TUI quit 才暴露,fix 文档未触及 stall detector)
- **本次是否引入新变种**: 是 — `progressive_failures` 在 7h47m 沉默期完全没触发,确认了 plan `2026-06-18-003 base-stability U1-U3`(stall detector + TTL + progress_steward)未覆盖"rejection stall"模式

### 反模式 4: task.resume 死信(ralph 越权发 + 0 消费者)
- **历史出现次数**: 5 次(merry-lotus / noble-peacock / perky-maple / warm-tiger / 本次)
- **首次出现**: `2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` P1-D
- **本次是否修复**: 部分(U2 已补 4 个 hat 的 task.resume triggers,但 ralph→coordinator 通道仍 0 消费者)
- **本次是否引入新变种**: 是 — 唯一 1 条 task.resume 记录 `agent/events-hat-ralph-primary-…:1` 直接变死信,确认 typed kind 路由缺导致消费者拿不到 reason_code

## 二轮修复建议(基于历史重复次数)

1. **必做 — 反模式 1 + 3 联合根治**: plan 2026-06-21-001 U1 扩展(iter/seq SSOT 化)必须与 U4 typed 路由同步落地,单独修 U1 不会打破 "filename_mismatch 第 6 次复发" 模式;stall detector 须新增 "rejection stall" 维度(参考 2026-06-18-003 U1-U3,扩出 U4 "rejection-count = accepted-count 持续 N 轮" 触发器)
2. **必做 — 反模式 2 caller 链入**: `event_loop/mod.rs:6074` 必须从 `LintResumeHint::from_reason` 字符串路径切到 `LintResumeHint::from_typed_rejection` typed 路径;补 cross-file 集成测试 `evaluate_event → Reject → LintResumeHint::from_typed_rejection → assert target == SourceHat`(本次 fix 漏的 P1-2)
3. **必做 — 反模式 4 消费者补全**: ralph→coordinator 的 task.resume 必须在 coordinator hat 注册 `reason_kinds` 订阅(覆盖 `HandoffFilenameMismatch` / `HandoffStructureInvalid` / `HandoffIllegalEmitTopic` 三类 typed kind),否则 typed 路由修了也是死信

## 本次未修复的历史重复问题数量

**4 / 4**(filename_mismatch / typed 路由 / stall detector / task.resume 死信 全部未闭环;P0-2 typed hint 只完成基础设施第一步,未完成修复闭环)
