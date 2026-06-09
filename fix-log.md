current_fix_round: 1

## Round 1
### Applied
- #1 crates/ralph-core/src/hat_lifecycle.rs:193 — completed_at dead_code allow 无 TODO 标注 → 添加 `// TODO(U4): remove after diagnose reporter integration`
- #2 crates/ralph-core/src/hat_lifecycle.rs:334 — unwrap_or_default() 时钟回退静默 0 无说明 → 改为 `unwrap_or(Duration::ZERO)` 并添加注释说明 fallback 原因
### Failed
(无)
### Verification
- Tests passed: hat_lifecycle (17/17), ralph-core full (1575/1575)
- Build: pass
- Clippy: pre-existing error in ralph-proto (collapsible_if), not related to changes
