// build.rs for ralph-cli
//
// Copies canonical preset yml files (the single source of truth under
// `presets/en/` in the repo root) into Cargo's $OUT_DIR at compile time,
// per the explicit allow-list in `presets/manifest.yml`. The Rust code
// embeds the copies via:
//
//     include_str!(concat!(env!("OUT_DIR"), "/presets/<name>.yml"))
//
// Why a manifest: `include_str!` content is baked into the binary, so
// the file must exist on the build host's filesystem during compilation,
// but the source location is otherwise free. That lets us delete the
// crate-internal mirror directory entirely. A manifest in addition
// keeps the embedded set auditable: adding a new preset requires an
// explicit opt-in in `presets/manifest.yml`, and a new entry in
// `crates/ralph-cli/src/presets.rs` (the two sides cross-check each
// other at compile time).
//
// Authoring rules live at the top of `presets/manifest.yml`.
//
// Directories that are NEVER read by this script:
//   * `presets/zh/`         — Chinese reference copies, not embedded.
//   * `presets/extras/`     — Orphan / demo files, not embedded.
//   * `presets/minimal/`    — Used by the smoke-test fixture loader,
//                              read from the canonical path at runtime.
//   * `presets/schemas/`    — Reference copies of payload schemas that
//                              USED to be loaded at runtime via
//                              `event_policy.schema_file`. That path is
//                              now broken (see audit fix 2026-06-03 in
//                              commit history) and the schemas are
//                              inlined into `presets/en/*.yml` instead.
//                              Kept here for diff-able reference only.

use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("build.rs: CARGO_MANIFEST_DIR not set; skipping preset copy");
            return;
        }
    };
    let out_dir = match std::env::var("OUT_DIR") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("build.rs: OUT_DIR not set; skipping preset copy");
            return;
        }
    };

    let manifest_path = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets")
        .join("manifest.yml");
    let en_dir = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets")
        .join("en");
    let dest = PathBuf::from(&out_dir).join("presets");

    println!("cargo:rerun-if-changed={}", manifest_path.display());

    let manifest_text = match fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "build.rs: failed to read manifest at {}: {}; skipping preset copy. \
                 This is expected when building from a crates.io tarball — the embedded \
                 binary falls back to runtime path resolution in that case.",
                manifest_path.display(),
                e
            );
            return;
        }
    };

    let names: Vec<String> = match parse_embedded_names(&manifest_text) {
        Ok(v) => v,
        Err(e) => panic!(
            "build.rs: failed to parse `presets/manifest.yml` ({}). \
             Expected a top-level `embedded:` key whose value is a YAML list of \
             preset basenames (no `.yml` extension).",
            e
        ),
    };

    if !en_dir.is_dir() {
        eprintln!(
            "build.rs: canonical preset dir not found at {}; skipping preset copy. \
             This is expected when building from a crates.io tarball — the embedded \
             binary falls back to runtime path resolution in that case.",
            en_dir.display()
        );
        return;
    }

    fs::create_dir_all(&dest).expect("failed to create $OUT_DIR/presets");

    let mut copied = 0usize;
    for name in &names {
        if name.is_empty() {
            panic!("build.rs: manifest contains an empty preset name");
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            panic!(
                "build.rs: manifest preset name `{}` is not a bare basename",
                name
            );
        }
        let src = en_dir.join(format!("{}.yml", name));
        let dest_file = dest.join(format!("{}.yml", name));
        if !src.is_file() {
            panic!(
                "build.rs: manifest lists `{}` but presets/en/{}.yml does not exist \
                 (looked at {}). Add the file or remove the name from manifest.yml.",
                name,
                name,
                src.display()
            );
        }
        if let Err(e) = fs::copy(&src, &dest_file) {
            panic!(
                "build.rs: failed to copy {} -> {}: {}",
                src.display(),
                dest_file.display(),
                e
            );
        }
        copied += 1;
        println!("cargo:rerun-if-changed={}", src.display());
    }

    eprintln!(
        "build.rs: copied {} preset yml from manifest ({} declared)",
        copied,
        names.len()
    );
}

/// Minimal `presets/manifest.yml` parser.
///
/// We avoid pulling in a YAML dependency in build.rs. The manifest format
/// is a small, stable subset:
///
/// ```yaml
/// # comments are allowed
/// embedded:
///   - name1
///   - name2
/// ```
///
/// Lines outside the `embedded:` block are ignored. Indentation must
/// match the example (two spaces under `embedded:`).
fn parse_embedded_names(text: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_embedded = false;
    let mut found_embedded_key = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim_end();
        if line.is_empty() {
            continue;
        }
        if !in_embedded {
            // Look for "embedded:" at the start of a line (allowing leading spaces).
            let stripped = line.trim_start();
            if stripped.starts_with("embedded:") {
                found_embedded_key = true;
                in_embedded = true;
                // Allow `embedded: [a, b, c]` flow-style too.
                let rest = stripped["embedded:".len()..].trim();
                if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    for n in inner.split(',') {
                        let n = n.trim().trim_matches('"').trim_matches('\'');
                        if !n.is_empty() {
                            names.push(n.to_string());
                        }
                    }
                    in_embedded = false;
                }
                continue;
            }
            // Any other top-level key terminates the search.
            if !line.starts_with(' ') && !line.starts_with('\t') && line.contains(':') {
                return Err(format!("line {}: unexpected key `{}`", idx + 1, line));
            }
        } else {
            let stripped = line.trim_start();
            if stripped.starts_with("- ") {
                let n = stripped[2..].trim().trim_matches('"').trim_matches('\'');
                if n.is_empty() {
                    return Err(format!("line {}: empty entry in `embedded:` list", idx + 1));
                }
                names.push(n.to_string());
            } else if !stripped.is_empty() {
                // Another top-level key — the embedded block has ended.
                in_embedded = false;
            }
        }
    }

    if !found_embedded_key {
        return Err("no `embedded:` key found".to_string());
    }
    Ok(names)
}
