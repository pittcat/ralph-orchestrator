//! Plan 2026-07-07-006 Unit 4 Step 4.6: lock the finding_id surface so
//! future contributors cannot silently re-introduce a serial-only or
//! coordinator-loop finding into the lint catalogue. Pipeline is the
//! single-chain primary path; serial-derived findings belong in
//! history.

use crate::preset_lint::finding_id::ALL_FINDING_IDS;

/// Any finding id whose name embeds one of these substrings is treated
/// as a serial-only or coordinator-loop artifact and must not appear
/// in `ALL_FINDING_IDS`. The lock turns a future regression into a
/// failing test rather than a silent re-introduction.
const FORBIDDEN_SUBSTRINGS: &[&str] = &[
    "phase_authority",
    "strict_reason",
    "review_complete_misrouted",
    // `serial_` and `shipper_` are intentionally not matched here:
    // pipeline's `ce_executor_pipeline.yml` was derived from the
    // serial topology in 2026-07-02, so a future finding might
    // legitimately use one of those words without being serial-only.
    // The lock focuses on the three historically serial-only modules
    // Unit 4 deletes: phase_authority, strict_reason_routing,
    // review_complete_misrouted.
];

#[test]
fn test_no_serial_only_finding_id_exported() {
    let hits: Vec<&&str> = ALL_FINDING_IDS
        .iter()
        .filter(|id| FORBIDDEN_SUBSTRINGS.iter().any(|frag| id.contains(frag)))
        .collect();
    assert!(
        hits.is_empty(),
        "no serial-only finding_id may remain exported from `finding_id`; got {hits:?}"
    );
}
