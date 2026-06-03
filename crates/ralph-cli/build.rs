// build.rs for ralph-cli
//
// Strips the `crates/ralph-cli/presets/` mirror directory by copying
// canonical preset yml files (the single source of truth at
// `presets/` in the repo root) into Cargo's $OUT_DIR at compile time.
// The Rust code embeds them via:
//
//     include_str!(concat!(env!("OUT_DIR"), "/presets/<name>.yml"))
//
// Why: `include_str!` content is baked into the binary, so the file
// must exist on the build host's filesystem during compilation, but
// the source location is otherwise free. That lets us delete the
// mirror directory and rely on the canonical files alone.
//
// Behaviour:
//   * Copies only top-level `*.yml` files in `presets/`.
//   * Skips any file whose name ends in `-zh.yml` — those are
//     reference-only, never embedded (per the preset author policy
//     recorded in `presets/COLLECTION.md`).
//   * Does NOT copy the `schemas/` or `minimal/` subdirectories —
//     nothing in the codebase uses `include_str!` on them. `schemas/`
//     stays canonical and is read at runtime via
//     `event_policy.schema_file`; `minimal/` is only used by the
//     smoke-test fixture loader which reads from the canonical path.
//
// `cargo:rerun-if-changed` lines pin the build to the precise set
// of copied source files so a yml edit triggers a rebuild but an
// unrelated edit in `presets/COLLECTION.md` does not.

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

    let canonical = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets");
    let dest = PathBuf::from(&out_dir).join("presets");

    if !canonical.is_dir() {
        eprintln!(
            "build.rs: canonical preset dir not found at {}; skipping preset copy. \
             This is expected when building from a crates.io tarball — the embedded \
             binary falls back to runtime path resolution in that case.",
            canonical.display()
        );
        return;
    }

    fs::create_dir_all(&dest).expect("failed to create $OUT_DIR/presets");

    let mut copied = 0usize;
    let mut skipped_zh = 0usize;
    let mut total_seen = 0usize;

    let entries = match fs::read_dir(&canonical) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "build.rs: failed to read_dir({}): {}; skipping preset copy",
                canonical.display(),
                e
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.ends_with(".yml") {
            continue;
        }
        total_seen += 1;
        if name.ends_with("-zh.yml") {
            skipped_zh += 1;
            // Intentionally do NOT emit rerun-if-changed for -zh files:
            // they are reference material and never enter the binary.
            continue;
        }
        let dest_file = dest.join(name);
        if let Err(e) = fs::copy(&path, &dest_file) {
            panic!(
                "build.rs: failed to copy {} -> {}: {}",
                path.display(),
                dest_file.display(),
                e
            );
        }
        copied += 1;
        println!("cargo:rerun-if-changed={}", path.display());
    }

    eprintln!(
        "build.rs: copied {} preset yml (skipped {} -zh.yml, {} total yml seen)",
        copied, skipped_zh, total_seen
    );
}
