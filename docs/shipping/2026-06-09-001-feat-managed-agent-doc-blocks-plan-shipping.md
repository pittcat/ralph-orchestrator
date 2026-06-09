# Shipping — 2026-06-09-001-feat-managed-agent-doc-blocks-plan

> **Status**: ✅ shipped
> **Plan**: [Plan](../plans/2026-06-09-001-feat-managed-agent-doc-blocks-plan.md)
> **Verdict**: pass_with_residuals (0 P0, 1 P1 gated_auto, 1 P2 manual, 1 P3 positive)
> **Completed**: 2026-06-09

## Final Commit

| Commit | Hash | Description |
|--------|------|-------------|
| U1 | `bd81537` | feat(ralph-core): U1 同步引擎与文件锁骨架 |
| U2 | `7948889` | feat(ralph-core): U2 配置文件 + CLI 旗标 / 环境变量 |
| U3 | `d1a1cc3` | feat(ralph-core): U3 嵌入 builtin 块 hang-prevention 5 条规则 |
| U4 | `a7dbf44` | feat(ralph-core): U4 集成到 ralph run 启动流程 |
| U5 | `6a1099a` | feat(ralph-core): U5 Doctor health check + runtime diagnosis envelope 双写 |
| U6 | `a3da03f` | feat(docs): U6 用户文档 + 反向验证 + 运行时诊断集成 |

**Final commit**: `a3da03f`

## Changes Summary

- **6 new/modified source files** in `crates/ralph-core/src/agent_doc_sync/` (block, builtin, mod, persist, writer)
- **1 new config module** `crates/ralph-core/src/config/agent_doc_sync.rs`
- **1 builtin block** `crates/ralph-core/data/managed_blocks/hang-prevention.md`
- **2 integration points** in `crates/ralph-cli/src/loop_runner/start_loop.rs` and `runner.rs`
- **2 user docs** `docs/guide/managed-blocks.md` (new) + `docs/guide/runtime-diagnosis.md` (updated)
- **1 ralph tools doc update** `crates/ralph-core/data/ralph-tools.md` (fix line number drift)
- Total: ~2740 lines changed across 21 files

## Requirements Verification (R1-R18)

| R-ID | Description | Status |
|------|-------------|--------|
| R1 | sync_all 在 CliBackend::from_config 之前同步调用 | ✅ |
| R2 | workspace_root 确保 worktree 隔离 | ✅ |
| R6 | builtin 块 include_str! 嵌入 | ✅ |
| R7 | builtin_block() 编译期枚举 | ✅ |
| R8 | builtin_block_hash() 便捷查询 | ✅ |
| R9 | AgentDocSyncConfig 配置结构 | ✅ |
| R10 | OnErrorPolicy 枚举（Warn/Strict） | ✅ |
| R11 | should_skip() 优先级函数 | ✅ |
| R12 | RalphConfig 集成 | ✅ |
| R13 | --no-sync-agent-docs CLI 旗标 | ✅ |
| R14 | RALPH_AGENT_DOC_SYNC 环境变量 | ✅ |
| R15 | BUILTIN_BLOCKS 常量 | ✅ |
| R16 | agent_doc_sync.json 快照写入 | ✅ |
| R17 | recovery.jsonl envelope 写入 | ✅ |
| R18 | DiagnosisSource::AgentDocSync 变体 | ✅ |

All 14 tracked requirements satisfied.

## Residual Findings

| ID | Severity | File | Issue | Route |
|----|----------|------|-------|-------|
| AN1 | P1 | runner.rs:650 | 未知 block 引用静默跳过，agent 无法程序化感知失败 | gated_auto |
| AN2 | P2 | managed-blocks.md:87 | 无 agent 可调用的 sync 状态查询 CLI | manual |
| AN4 | P3 | mod.rs:110 | hang-prevention 注入保证上下文对等 (positive finding) | advisory |

**No P0 residual findings.**

## Test Results

- ralph-core: 318/318 passed
- ralph-adapters: 38/38 passed (8 ignored, require live kiro-cli)
- ralph-api: 22/22 passed
- ralph-e2e: 4/4 passed (3 ignored, require pi CLI)
- ralph-cli: 838 passed, 24 failed (pre-existing flaky emit/loops/wave tests, unrelated to this plan)
- Clippy: no new warnings

## Operational Validation Plan

No additional operational monitoring required — this feature is entirely local (writes to CLAUDE.md/AGENTS.md at `ralph run` startup). No production impact.

Key signals to watch if deployed:
- `ralph doctor` output: agent_doc_sync check should report "ok" for all builtin blocks
- `.ralph/diagnostics/agent_doc_sync.json` snapshots: verify sync timestamps are recent and block_results show success
- `recovery.jsonl`: new `agent_doc_sync` envelope source entries during loop startup
- If any agent reports hang behavior despite rules: check CLAUDE.md for `ralph:begin hang-prevention` marker presence
