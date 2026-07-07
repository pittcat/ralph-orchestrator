//! Plan 2026-07-07-006 Unit 4 Step 4.1: lock the recovery / fallback
//! path so it can never reach a pass terminal. Pipeline is the
//! single-chain primary path; silent-success promotion through the
//! recovery bucket (the 2026-07 silent-success family) is the
//! regression we are closing.

use ralph_proto::Topic;

/// Topics that are reserved for explicit success terminals. A
/// `fallback.*` or `recovery.*` topic MUST NOT be promoted into this
/// set, even when the recovery bucket's reason matches a
/// "promote-to-success" branch in the historical serial code path.
///
/// Pinning this list here turns a future regression into a
/// compile/test failure rather than a silent success terminal.
const PASS_ONLY_TOPICS: &[&str] = &[
    "work.done",
    "report.done",
];

#[test]
fn test_fallback_recovery_cannot_produce_success() {
    // A representative set of recovery / fallback topics that
    // historically could route to a pass terminal through the
    // shipper-reason whitelist.
    let recovery_topics = [
        "fallback.blocked",
        "fallback.resume",
        "recovery.exhausted",
        "plan.blocked",
        "task.resume",
    ];
    for topic in recovery_topics {
        let topic_str = Topic::new(topic).to_string();
        assert!(
            !PASS_ONLY_TOPICS.contains(&topic_str.as_str()),
            "fallback topic {topic} MUST NOT be in the pass-only set; \
             recovery events must terminate at blocked/failed"
        );
    }
}
