//! Built-in managed blocks embedded at compile time.
//!
//! Each builtin block's markdown content is embedded via `include_str!` and
//! its SHA-256 hash is computed at compile time. Runtime lookup is a simple
//! string match — no file I/O.

use super::block::BlockSpec;

/// Content of the `hang-prevention` block.
///
/// Embedded from `crates/ralph-core/data/managed_blocks/hang-prevention.md`
/// at compile time. The 5 Command Hang Prevention Rules are embedded verbatim.
const HANG_PREVENTION_CONTENT: &str =
    include_str!("../../data/managed_blocks/hang-prevention.md");

/// Returns the [`BlockSpec`] for a builtin block by ID, or `None` if the ID
/// is not a recognised builtin block.
///
/// # Examples
///
/// ```ignore
/// let block = builtin_block("hang-prevention").expect("known block");
/// assert_eq!(block.id, "hang-prevention");
/// ```
pub fn builtin_block(id: &str) -> Option<BlockSpec> {
    match id {
        "hang-prevention" => Some(BlockSpec::new(id, HANG_PREVENTION_CONTENT)),
        _ => None,
    }
}

/// Returns the compile-time SHA-256 hash for a builtin block.
///
/// This is a thin wrapper around [`builtin_block`] for callers that only
/// need the hash (e.g. for marker version checks without constructing a
/// full [`BlockSpec`]).
pub fn builtin_block_hash(id: &str) -> Option<String> {
    builtin_block(id).map(|b| b.content_sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::block::compute_sha256;

    #[test]
    fn hang_prevention_content_not_empty() {
        assert!(!HANG_PREVENTION_CONTENT.is_empty());
    }

    #[test]
    fn hang_prevention_sha256_is_stable() {
        let hash1 = compute_sha256(HANG_PREVENTION_CONTENT);
        let hash2 = compute_sha256(HANG_PREVENTION_CONTENT);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn builtin_block_returns_hang_prevention() {
        let block = builtin_block("hang-prevention");
        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.id, "hang-prevention");
        assert_eq!(block.content, HANG_PREVENTION_CONTENT);
        assert_eq!(block.content_sha256, compute_sha256(HANG_PREVENTION_CONTENT));
    }

    #[test]
    fn builtin_block_returns_none_for_unknown() {
        assert!(builtin_block("nope").is_none());
        assert!(builtin_block("").is_none());
        assert!(builtin_block("HANG-PREVENTION").is_none());
    }

    #[test]
    fn hang_prevention_contains_all_five_rules() {
        // Verify all 5 numbered rules are present
        assert!(HANG_PREVENTION_CONTENT.contains("1."));
        assert!(HANG_PREVENTION_CONTENT.contains("2."));
        assert!(HANG_PREVENTION_CONTENT.contains("3."));
        assert!(HANG_PREVENTION_CONTENT.contains("4."));
        assert!(HANG_PREVENTION_CONTENT.contains("5."));
    }

    #[test]
    fn hang_prevention_blocks_forbidden_examples() {
        // Verify forbidden command patterns are present
        assert!(HANG_PREVENTION_CONTENT.contains("tail -f"));
        assert!(HANG_PREVENTION_CONTENT.contains("tail -F"));
        assert!(HANG_PREVENTION_CONTENT.contains("journalctl -f"));
        assert!(HANG_PREVENTION_CONTENT.contains("adb logcat"));
        assert!(HANG_PREVENTION_CONTENT.contains("dmesg -w"));
        assert!(HANG_PREVENTION_CONTENT.contains("watch"));
        assert!(HANG_PREVENTION_CONTENT.contains("while true"));
    }

    #[test]
    fn builtin_block_hash_matches_block_spec() {
        let hash = builtin_block_hash("hang-prevention").unwrap();
        let block = builtin_block("hang-prevention").unwrap();
        assert_eq!(hash, block.content_sha256);
    }

    #[test]
    fn builtin_block_hash_returns_none_for_unknown() {
        assert!(builtin_block_hash("nope").is_none());
    }
}
