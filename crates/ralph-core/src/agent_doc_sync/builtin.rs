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

/// Returns the sorted list of builtin block IDs currently registered.
///
/// Used for fail-closed error messages (D4): when an unknown `block_ref`
/// is encountered, the operator can see what builtins are available.
#[must_use]
pub fn known_builtin_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = BUILTIN_IDS.to_vec();
    ids.sort_unstable();
    ids
}

const BUILTIN_IDS: &[&str] = &["hang-prevention"];

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
    fn known_builtin_ids_includes_hang_prevention() {
        let ids = known_builtin_ids();
        assert!(ids.contains(&"hang-prevention"), "ids = {ids:?}");
        // Sorted.
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "ids should be sorted");
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
}
