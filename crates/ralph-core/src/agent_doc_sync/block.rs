//! Block specification and marker parsing for managed agent doc blocks.
//!
//! Each managed block is identified by a string ID and carries content that
//! is embedded into agent-facing markdown files (CLAUDE.md / AGENTS.md)
//! delimited by HTML-comment markers:
//!
//! ```text
//! <!-- ralph:begin <id> v=sha256:<64hex> -->
//! <content>
//! <!-- ralph:end <id> -->
//! ```

use sha2::{Digest, Sha256};

/// A managed block to be injected into agent doc files.
///
/// `content_sha256` is always a 64-character lowercase hex string representing
/// the SHA-256 digest of `content`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BlockSpec {
    /// Unique identifier for this block (e.g. `"hang-prevention"`).
    pub id: String,
    /// Markdown content to inject between markers.
    pub content: String,
    /// SHA-256 hex digest of `content` (64 lowercase hex chars).
    pub content_sha256: String,
}

impl BlockSpec {
    /// Creates a new `BlockSpec`, computing the SHA-256 hash of `content`.
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        let content_sha256 = compute_sha256_hex(&content);
        Self {
            id: id.into(),
            content,
            content_sha256,
        }
    }
}

/// State of a block's markers in a file, as determined by [`parse_marker_state`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockState {
    /// Block markers are completely absent from the file.
    Missing,
    /// Begin/end markers are present but the version hash differs.
    Mismatched {
        /// The version hash found in the begin marker.
        found_hash: String,
    },
    /// Begin/end markers are present and version hash matches.
    UpToDate,
}

/// Computes a 64-character lowercase hex SHA-256 digest.
pub fn compute_sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Regex pattern for the begin marker: `<!-- ralph:begin <id> v=sha256:<64hex> -->`
///
/// Uses `\s*$` at the end to tolerate trailing whitespace (e.g. from
/// `format!` line continuation in tests).
fn begin_marker_re(id: &str) -> regex::Regex {
    let escaped = regex::escape(id);
    regex::Regex::new(&format!(
        r"^<!--\s*ralph:begin\s+{escaped}\s+v=sha256:([0-9a-f]{{64}})\s*-->\s*$"
    ))
    .expect("invalid begin marker regex")
}

/// Regex pattern for the end marker: `<!-- ralph:end <id> -->`
fn end_marker_re(id: &str) -> regex::Regex {
    let escaped = regex::escape(id);
    regex::Regex::new(&format!(r"^<!--\s*ralph:end\s+{escaped}\s*-->\s*$"))
        .expect("invalid end marker regex")
}

/// Parses the file content and determines the state of markers for `block_id`.
///
/// Returns `(state, begin_line_idx, end_line_idx)` where the line indices
/// point to the begin and end marker lines (0-based).
///
/// - `Missing` when no begin marker is found.
/// - `Mismatched` when a begin marker is found but the end marker is missing
///   (orphan begin) or the hash differs. `end_line` is `None` for orphan begin.
/// - `UpToDate` when both markers exist and the hash matches (via
///   [`parse_marker_state_with_version`]).
pub(crate) fn parse_marker_state(
    content: &str,
    block_id: &str,
) -> (BlockState, Option<usize>, Option<usize>) {
    let begin_re = begin_marker_re(block_id);
    let end_re = end_marker_re(block_id);

    let mut found_begin = false;
    let mut begin_line = None;
    let mut found_hash = String::new();

    for (idx, line) in content.lines().enumerate() {
        if let Some(caps) = begin_re.captures(line) {
            found_begin = true;
            begin_line = Some(idx);
            found_hash = caps[1].to_string();
            continue;
        }

        if found_begin && end_re.is_match(line) {
            let end_line = Some(idx);
            return (BlockState::Mismatched { found_hash }, begin_line, end_line);
        }
    }

    // Orphan begin marker: begin found but no matching end.
    // Return Mismatched so the caller triggers Replace (not Append),
    // preventing duplicate blocks from being appended.
    if found_begin {
        return (BlockState::Mismatched { found_hash }, begin_line, None);
    }

    (BlockState::Missing, None, None)
}

/// Like [`parse_marker_state`] but returns `UpToDate` when the found hash
/// matches `expected_hash`.
pub(crate) fn parse_marker_state_with_version(
    content: &str,
    block_id: &str,
    expected_hash: &str,
) -> (BlockState, Option<usize>, Option<usize>) {
    let (state, begin, end) = parse_marker_state(content, block_id);
    match state {
        // Only upgrade to UpToDate when hash matches AND end marker exists.
        // An orphan begin (end is None) must remain Mismatched so the caller
        // triggers Replace and appends the missing end marker.
        BlockState::Mismatched { ref found_hash }
            if found_hash == expected_hash && end.is_some() =>
        {
            (BlockState::UpToDate, begin, end)
        }
        other => (other, begin, end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONTENT: &str = "# My Project\n\nSome description.\n";

    /// Builds a content string from parts, joining with `\n`.
    /// Trailing newline is added only if `trailing_newline` is true.
    fn build_content(parts: &[&str], trailing_newline: bool) -> String {
        let mut result = parts.join("\n");
        if trailing_newline {
            result.push('\n');
        }
        result
    }

    #[test]
    fn compute_sha256_is_stable() {
        let hash1 = compute_sha256_hex("hello world");
        let hash2 = compute_sha256_hex("hello world");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn compute_sha256_matches_known_value() {
        let hash = compute_sha256_hex("hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn block_spec_new_computes_hash() {
        let block = BlockSpec::new("test", "content");
        assert_eq!(block.id, "test");
        assert_eq!(block.content, "content");
        assert_eq!(block.content_sha256, compute_sha256_hex("content"));
    }

    #[test]
    fn parse_marker_state_missing_when_no_markers() {
        let (state, begin, end) = parse_marker_state(SAMPLE_CONTENT, "hang-prevention");
        assert_eq!(state, BlockState::Missing);
        assert!(begin.is_none());
        assert!(end.is_none());
    }

    #[test]
    fn parse_marker_state_mismatched_when_hash_differs() {
        let old_hash = "a".repeat(64);
        let content = build_content(
            &[
                "# My Project",
                "",
                "Some description.",
                "",
                "## Ralph Managed Blocks",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{old_hash} -->"),
                "old content",
                &format!("<!-- ralph:end hang-prevention -->"),
            ],
            true,
        );
        let (state, begin, end) = parse_marker_state(&content, "hang-prevention");
        assert!(matches!(state, BlockState::Mismatched { .. }));
        // begin at line 6, end at line 8
        assert_eq!(begin, Some(6));
        assert_eq!(end, Some(8));
    }

    #[test]
    fn parse_marker_state_with_version_up_to_date() {
        let hash = compute_sha256_hex("correct content");
        let content = build_content(
            &[
                "# My Project",
                "",
                "Some description.",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{hash} -->"),
                "correct content",
                &format!("<!-- ralph:end hang-prevention -->"),
            ],
            true,
        );
        let (state, begin, end) =
            parse_marker_state_with_version(&content, "hang-prevention", &hash);
        assert_eq!(state, BlockState::UpToDate);
        assert!(begin.is_some());
        assert!(end.is_some());
    }

    #[test]
    fn parse_marker_state_with_version_still_mismatched() {
        let old_hash = "a".repeat(64);
        let expected_hash = "b".repeat(64);
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{old_hash} -->"),
                "old",
                &format!("<!-- ralph:end hang-prevention -->"),
            ],
            true,
        );
        let (state, _, _) =
            parse_marker_state_with_version(&content, "hang-prevention", &expected_hash);
        assert!(matches!(state, BlockState::Mismatched { .. }));
    }

    #[test]
    fn parse_marker_state_mismatched_when_only_begin() {
        let hash = "a".repeat(64);
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{hash} -->"),
            ],
            true,
        );
        let (state, begin, end) = parse_marker_state(&content, "hang-prevention");
        assert!(
            matches!(state, BlockState::Mismatched { .. }),
            "orphan begin should be Mismatched, got: {state:?}"
        );
        assert!(begin.is_some());
        assert!(end.is_none());
    }

    #[test]
    fn parse_marker_state_missing_when_only_end() {
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:end hang-prevention -->"),
            ],
            true,
        );
        let (state, begin, end) = parse_marker_state(&content, "hang-prevention");
        assert_eq!(state, BlockState::Missing);
        assert!(begin.is_none());
        assert!(end.is_none());
    }

    #[test]
    fn parse_marker_state_different_block_id_ignored() {
        let hash = "a".repeat(64);
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:begin other-block v=sha256:{hash} -->"),
                "other content",
                &format!("<!-- ralph:end other-block -->"),
            ],
            true,
        );
        let (state, _, _) = parse_marker_state(&content, "hang-prevention");
        assert_eq!(state, BlockState::Missing);
    }

    #[test]
    fn parse_marker_state_orphan_begin_is_mismatched() {
        let hash = "a".repeat(64);
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{hash} -->"),
            ],
            true,
        );
        let (state, begin, end) = parse_marker_state(&content, "hang-prevention");
        assert!(
            matches!(state, BlockState::Mismatched { .. }),
            "orphan begin should return Mismatched, got: {state:?}"
        );
        assert!(begin.is_some());
        assert!(end.is_none());
    }

    #[test]
    fn parse_marker_state_with_version_orphan_begin_with_matching_hash_is_mismatched() {
        let hash = compute_sha256_hex("some content");
        let content = build_content(
            &[
                "# My Project",
                "",
                &format!("<!-- ralph:begin hang-prevention v=sha256:{hash} -->"),
                "some content",
            ],
            true,
        );
        // Hash matches but end marker is missing → must stay Mismatched
        let (state, begin, end) =
            parse_marker_state_with_version(&content, "hang-prevention", &hash);
        assert!(
            matches!(state, BlockState::Mismatched { .. }),
            "orphan begin with matching hash should stay Mismatched, got: {state:?}"
        );
        assert!(begin.is_some());
        assert!(end.is_none());
    }
}
