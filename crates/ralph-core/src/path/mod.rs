//! Path containment helpers shared by the DAG runtime.

pub mod workspace;

pub use workspace::path_within_workspace;

/// Map a logical name (idempotency key, archive name, …) to an on-disk
/// file stem legal on the current platform.
///
/// Windows percent-encodes the NTFS-illegal set (`< > : " / \ | ? *` and
/// control chars; `%` escaped first so the mapping stays injective).
/// It additionally handles NTFS rules that raw encoding cannot fix:
/// reserved device names (`CON`, `NUL`, `COM1`…, `LPT1`…), a trailing
/// `.` or ` `, empty input, and the 255-byte component limit (deterministic
/// hash-truncation, injective in practice).
/// Unix keeps the name verbatim — `:` is legal in POSIX filenames and
/// existing `.ralph` dirs must stay untouched.
#[cfg(windows)]
pub fn filename_stem(name: &str) -> String {
    /// Names Windows forbids as *any* extension stem (case-insensitive).
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "CLOCK$", "CONFIG$", "KEYBD$", "SCREEN$", "COM1", "COM2",
        "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
    ];
    /// NTFS component limit is 255 *bytes*; leave head-room for callers
    /// appending extensions like `.jsonl`.
    const MAX_STEM: usize = 200;
    /// Truncated stems keep this much verbatim prefix before the hash.
    const TRUNC_PREFIX: usize = MAX_STEM - 20;

    let mut out = String::with_capacity(name.len() + 8);
    for ch in name.chars() {
        match ch {
            '%' => out.push_str("%25"),
            ':' => out.push_str("%3A"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '"' => out.push_str("%22"),
            '/' => out.push_str("%2F"),
            '\\' => out.push_str("%5C"),
            '|' => out.push_str("%7C"),
            '?' => out.push_str("%3F"),
            '*' => out.push_str("%2A"),
            c if (c as u32) < 0x20 => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }

    // Trailing '.' or ' ' is illegal in a bare component: encode the
    // final char so the stem stays legal even with no extension.
    for trailing in ['.', ' '] {
        if out.ends_with(trailing) {
            let enc = format!("%{:02X}", trailing as u32);
            out.truncate(out.len() - trailing.len_utf8());
            out.push_str(&enc);
        }
    }

    // Reserved device names: escape the first character. Injective
    // because '%' itself is already escaped.
    if RESERVED.contains(&out.to_uppercase().as_str()) {
        let first = out.chars().next().unwrap();
        out = format!("%{:02X}{}", first as u32, &out[first.len_utf8()..]);
    }

    if out.is_empty() {
        out.push_str("%00"); // no un-encoded input can produce "%00"
    }

    // Component-length cap with a deterministic content hash so distinct
    // long names keep distinct stems.
    if out.len() > MAX_STEM {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = FNV_OFFSET;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        // Never slice mid-UTF-8.
        let mut prefix_len = TRUNC_PREFIX.min(out.len());
        while !out.is_char_boundary(prefix_len) {
            prefix_len -= 1;
        }
        out = format!("{}%{:016x}", &out[..prefix_len], hash);
    }

    out
}

/// Unix: the name is used verbatim as the file stem (historical
/// behaviour; `:` is legal in POSIX filenames).
#[cfg(not(windows))]
pub fn filename_stem(name: &str) -> String {
    name.to_string()
}
