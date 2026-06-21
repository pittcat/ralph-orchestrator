// P2-6: SSOT multi-section merge table (KTD-1, plan 2026-06-20-001 U1).
//
// Each tuple maps a top-level `schemas/<name>.yml` key to the
// `event_loop.*` target path inside the merged embedded preset.
// Operators editing the SSOT use only the names on the left;
// `build.rs` and the runtime test (`merge_preset_with_schema_yaml`)
// consume the right side.
//
// This file is included by BOTH `crates/ralph-cli/build.rs` (as a
// build-time source) and `crates/ralph-cli/src/presets.rs` (as a
// pub const via include! trick). The build script cannot import
// from the library because they are separate compilation units;
// the `include!` macro is the only mechanism that lets them
// share the same source. Updating the array in one place
// updates both.
//
// Order matters: the merge walks the array in declaration order
// and a later entry overwrites an earlier one. Today the keys
// are disjoint under `event_loop.*`, so order is irrelevant;
// preserve the alphabetical-ish layout for readability.
pub const SSOT_SECTION_TARGETS: &[(&str, &[&str])] = &[
    ("execution_contracts", &["event_loop", "execution_contracts"]),
    ("verdict_gate", &["event_loop", "verdict_gate"]),
    ("workflow_contract", &["event_loop", "workflow_contract"]),
    ("state_projection", &["event_loop", "state_projection"]),
    ("hat_handoff", &["event_loop", "hat_handoff"]),
];
