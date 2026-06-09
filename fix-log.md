current_fix_round: 3

## Round 1
### Applied
- #1 crates/ralph-core/src/agent_doc_sync/mod.rs:135-151 — block_results 归因不准确 (binary failed>0 check) → 让 FileSyncResult 携带 per-block outcomes，sync_all 直接用它构建 block_results
- #1 crates/ralph-core/src/agent_doc_sync/writer.rs — FileSyncResult 缺少 per-block 跟踪 → 新增 FileBlockOutcome 结构，在 sync_file 每个分支 push 对应 outcome

### Failed
无

### Verification
- Tests passed: agent_doc_sync 33/33, ralph-core full suite 无失败
- Build: cargo test -p ralph-core 通过
- 新增测试: block_result_skipped_when_up_to_date 验证 Skipped 归因正确

## Round 2
### Applied
- #2 crates/ralph-core/src/agent_doc_sync/writer.rs:290-298 — Strict 模式不传播错误 → 新增 SyncError 枚举，sync_file 返回 Result<FileSyncResult, SyncError>，OnError::Strict 下锁失败/读写错误/验证失败均返回 Err
- #3 crates/ralph-core/src/agent_doc_sync/block.rs:114-128 + writer.rs compute_replace — 孤立 begin marker 导致重复追加 → parse_marker_state 对 orphan begin 返回 Mismatched（非 Missing），compute_replace 处理 end_line=None 时从 begin 替换到 EOF

### Failed
无

### Verification
- Tests passed: agent_doc_sync 37/37（含新增 sync_strict_mode_returns_err_on_lock_failure、sync_strict_mode_propagates_lock_error、sync_replaces_orphan_begin_marker_without_duplication、sync_replaces_orphan_begin_marker_no_duplication、parse_marker_state_orphan_begin_is_mismatched、parse_marker_state_mismatched_when_only_begin）
- Build: cargo test -p ralph-core 通过
- Clippy: agent_doc_sync 无新 warning

## Round 3
### Applied
- #3 crates/ralph-core/src/agent_doc_sync/block.rs:152 — orphan begin marker with matching hash 被错误升级为 UpToDate → parse_marker_state_with_version 新增 end.is_some() 检查，orphan begin 即使 hash 匹配也保持 Mismatched
- #4 crates/ralph-core/src/agent_doc_sync/writer.rs:253-288 — orphan begin marker 替换后丢失用户内容 → 移除 orphan_replaced 标志，改为直接 fall-through 保留 orphan 之后的所有行（用户内容）

### Failed
无

### Verification
- Tests passed: agent_doc_sync 41/41（含新增 parse_marker_state_with_version_orphan_begin_with_matching_hash_is_mismatched、sync_replaces_orphan_begin_preserves_user_content_after、sync_replaces_orphan_begin_with_matching_hash、sync_replaces_orphan_begin_with_matching_hash_and_preserves_user_content）
- Build: cargo test -p ralph-core 通过
- Clippy: agent_doc_sync 无新 warning
