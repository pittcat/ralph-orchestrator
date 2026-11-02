//! Text utilities for the Ralph Orchestrator.
//!
//! This module provides common text manipulation functions used throughout
//! the codebase, including UTF-8 safe string truncation.

/// Finds the largest byte index <= `index` that is a valid UTF-8 character boundary.
///
/// This is needed because Rust strings cannot be sliced at arbitrary byte positions -
/// only at valid character boundaries. Multi-byte characters (emojis, etc.) would cause
/// a panic if sliced in the middle.
///
/// This is a stable Rust implementation of `str::floor_char_boundary` (nightly-only).
///
/// # Examples
///
/// ```
/// use ralph_core::floor_char_boundary;
///
/// let s = "Hello 🦀 World";  // 🦀 is at bytes 6-9
/// assert_eq!(floor_char_boundary(s, 6), 6);   // At start of emoji - valid boundary
/// assert_eq!(floor_char_boundary(s, 7), 6);   // Inside emoji - returns start
/// assert_eq!(floor_char_boundary(s, 8), 6);   // Inside emoji - returns start
/// assert_eq!(floor_char_boundary(s, 10), 10); // After emoji - valid boundary
/// ```
#[must_use]
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    // Walk backwards from index until we find a valid char boundary
    let mut boundary = index;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Truncates a string to a maximum number of characters, including "..." if truncated.
///
/// This function is UTF-8 safe: it uses character boundaries, not byte boundaries,
/// so it will never split a multi-byte character (emoji, non-ASCII, etc.).
///
/// If truncated, the resulting string length will be exactly `max_chars`.
/// If `max_chars` is less than 3, no ellipsis is added and it just takes `max_chars`.
///
/// # Arguments
///
/// * `s` - The string to truncate
/// * `max_chars` - Maximum number of characters (not bytes) for the final string
///
/// # Returns
///
/// - The original string if its character count is <= `max_chars`
/// - A truncated string of length `max_chars` with "..." suffix if longer
///
/// # Examples
///
/// ```
/// use ralph_core::truncate_with_ellipsis;
///
/// // Short strings pass through unchanged
/// assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
///
/// // Long strings are truncated so the total length is max_chars
/// assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");
///
/// // UTF-8 safe: emojis are not split
/// assert_eq!(truncate_with_ellipsis("🎉🎊🎁🎄", 3), "...");
/// ```
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars < 3 {
        // Not enough room for ellipsis, just take characters
        s.chars().take(max_chars).collect()
    } else {
        // Take max_chars - 3 characters and add ellipsis
        let keep = max_chars - 3;
        let byte_idx = s
            .char_indices()
            .nth(keep)
            .map(|(idx, _)| idx)
            .unwrap_or(s.len());
        format!("{}...", &s[..byte_idx])
    }
}

/// Decode file bytes to a `String`, tolerating UTF-8 BOM and UTF-16
/// LE/BE BOMs. PowerShell 7 `>` / `Out-File` writes UTF-8 by default, but
/// `powershell.exe` 5.1 and some tools emit UTF-16LE with BOM — agent
/// written events/payload files on Windows hit this path.
///
/// Pure function over bytes; identical behaviour on all platforms
/// (UTF-8 input without BOM is returned unchanged, zero cost).
pub fn decode_file_text(bytes: &[u8]) -> String {
    const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
    const UTF16_BE_BOM: &[u8] = b"\xfe\xff";
    const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

    if bytes.starts_with(UTF16_LE_BOM) {
        let units: Vec<u16> = bytes[UTF16_LE_BOM.len()..]
            .chunks(2)
            .map_while(|c| <[u8; 2]>::try_from(c).ok().map(u16::from_le_bytes))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(UTF16_BE_BOM) {
        let units: Vec<u16> = bytes[UTF16_BE_BOM.len()..]
            .chunks(2)
            .map_while(|c| <[u8; 2]>::try_from(c).ok().map(u16::from_be_bytes))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let body = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    String::from_utf8_lossy(body).into_owned()
}

/// `fs::read` + [`decode_file_text`]. Use this for files an agent or an
/// external tool may have written (events, payloads) where the encoding
/// is not runtime-controlled.
pub fn read_file_text(path: &std::path::Path) -> std::io::Result<String> {
    std::fs::read(path).map(|bytes| decode_file_text(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_file_text() {
        assert_eq!(decode_file_text(b"hello"), "hello");
        // UTF-8 BOM stripped
        assert_eq!(decode_file_text(b"\xef\xbb\xbfhello"), "hello");
        // UTF-16LE BOM decoded
        let mut le = vec![0xffu8, 0xfe];
        le.extend_from_slice(&[b'h', 0, b'i', 0]);
        assert_eq!(decode_file_text(&le), "hi");
        // UTF-16BE BOM decoded
        let be = vec![0xfeu8, 0xff, 0, b'h', 0, b'i'];
        assert_eq!(decode_file_text(&be), "hi");
    }

    #[test]
    fn test_floor_char_boundary_ascii() {
        let s = "hello";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 10), 5); // Beyond string length
    }

    #[test]
    fn test_floor_char_boundary_emoji() {
        // 🦀 is 4 bytes (U+1F980)
        let s = "hi🦀ok"; // h=0, i=1, 🦀=2-5, o=6, k=7
        assert_eq!(floor_char_boundary(s, 2), 2); // Start of emoji
        assert_eq!(floor_char_boundary(s, 3), 2); // Inside emoji
        assert_eq!(floor_char_boundary(s, 4), 2); // Inside emoji
        assert_eq!(floor_char_boundary(s, 5), 2); // Inside emoji
        assert_eq!(floor_char_boundary(s, 6), 6); // After emoji
    }

    #[test]
    fn test_floor_char_boundary_checkmark() {
        // ✅ is 3 bytes (U+2705)
        let s = "a✅b"; // a=0, ✅=1-3, b=4
        assert_eq!(floor_char_boundary(s, 1), 1); // Start of checkmark
        assert_eq!(floor_char_boundary(s, 2), 1); // Inside checkmark
        assert_eq!(floor_char_boundary(s, 3), 1); // Inside checkmark
        assert_eq!(floor_char_boundary(s, 4), 4); // At 'b'
    }

    #[test]
    fn test_floor_char_boundary_empty() {
        assert_eq!(floor_char_boundary("", 0), 0);
        assert_eq!(floor_char_boundary("", 5), 0);
    }

    #[test]
    fn test_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("short", 10), "short");
        assert_eq!(truncate_with_ellipsis("", 5), "");
        assert_eq!(truncate_with_ellipsis("exact", 5), "exact");
    }

    #[test]
    fn test_long_string_truncated() {
        assert_eq!(
            truncate_with_ellipsis("this is a long string", 10),
            "this is..."
        );
        assert_eq!(truncate_with_ellipsis("abcdef", 3), "...");
    }

    #[test]
    fn test_utf8_boundaries_arrows() {
        // Arrow characters are 3 bytes each in UTF-8
        let arrows = "→→→→→→→→";
        assert_eq!(truncate_with_ellipsis(arrows, 5), "→→...");
    }

    #[test]
    fn test_utf8_boundaries_mixed() {
        let mixed = "a→b→c→d";
        assert_eq!(truncate_with_ellipsis(mixed, 5), "a→...");
    }

    #[test]
    fn test_utf8_boundaries_emoji() {
        // Emojis are 4 bytes each in UTF-8
        let emoji = "🎉🎊🎁🎄";
        assert_eq!(truncate_with_ellipsis(emoji, 3), "...");
    }

    #[test]
    fn test_utf8_complex_emoji() {
        // Rust crab emoji
        let s = "hi 🦀 there";
        // "hi 🦀" = 4 characters (h, i, space, 🦀)
        assert_eq!(truncate_with_ellipsis(s, 4), "h...");
    }

    #[test]
    fn test_zero_max_chars() {
        assert_eq!(truncate_with_ellipsis("hello", 0), "");
    }

    #[test]
    fn test_single_char_truncation() {
        assert_eq!(truncate_with_ellipsis("hello", 1), "h");
        assert_eq!(truncate_with_ellipsis("🎉hello", 1), "🎉");
    }
}
