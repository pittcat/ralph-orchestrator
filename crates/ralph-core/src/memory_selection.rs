//! Budgeted, task-relevant memory selection (plan 2026-10-29-0104 U7).
//!
//! Single selection path shared by `ralph memory prime` and automatic
//! prompt injection so the two can never diverge (R13):
//!
//! - after the caller applies the visibility boundary, candidates are
//!   filtered for lifecycle (`invalidated` / `superseded_by` entries
//!   are never auto-selected, R14);
//! - each survivor is scored for task relevance (query terms on
//!   content+tags, tag/type filters, recency bonus);
//! - whole entries are packed under a finite budget measured in
//!   rendered characters — no mid-entry truncation;
//! - a [`SelectionReceipt`] records candidates, exclusions, scores,
//!   budget and reasons **without leaking memory content** (R15) —
//!   only ids, tags and numeric scores are captured.

use crate::memory::{Memory, MemoryType};
use serde::Serialize;

/// One candidate line in the receipt — id + score + verdict only,
/// never content (private memory text must not leak into receipts).
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptCandidate {
    pub id: String,
    pub score: i64,
    pub selected: bool,
    /// Why the entry was excluded (`""` when selected).
    pub exclusion_reason: String,
    pub tags: Vec<String>,
}

/// What the selector decided (R15). Safe to persist alongside the
/// diagnostics session or print to stderr — contains no content.
#[derive(Debug, Clone, Serialize)]
pub struct SelectionReceipt {
    pub schema_version: String,
    pub query: Option<String>,
    pub budget_chars: usize,
    pub used_chars: usize,
    pub candidates: Vec<ReceiptCandidate>,
}

/// Filter the caller already applied (visibility stays the caller's
/// job); lifecycle exclusions happen inside the selector.
#[derive(Debug, Clone, Default)]
pub struct SelectionFilters {
    pub query: Option<String>,
    pub types: Vec<MemoryType>,
    pub tags: Vec<String>,
    /// Only memories created within the last N days.
    pub recent_days: Option<u32>,
}

/// Estimated rendered size of one entry — content + id + tags +
/// fixed markdown scaffolding (### header, `> ` prefixes, metadata
/// comment ≈ 80 chars).
fn rendered_size(m: &Memory) -> usize {
    m.content.len() + m.id.len() + m.tags.join(", ").len() + 96
}

/// Deterministic relevance score. Higher is more relevant; a `0`
/// score entry is still eligible when nothing better exists — the
/// score only orders the pack, it never fabricates relevance (R8:
/// a memory reference is not proof of usefulness).
fn score(m: &Memory, query: Option<&str>, recent_days: Option<u32>) -> i64 {
    let mut s = 0i64;
    if let Some(q) = query {
        let content = m.content.to_lowercase();
        for term in q.to_lowercase().split_whitespace() {
            if term.len() < 2 {
                continue;
            }
            if content.contains(&term) {
                s += 10;
            }
            if m.tags.iter().any(|t| t.to_lowercase() == term) {
                s += 20;
            }
        }
    }
    if let Some(days) = recent_days {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
        if m.created >= cutoff.format("%Y-%m-%d").to_string() {
            s += 5;
        }
    }
    s
}

/// Select whole memory entries under `budget_chars`.
///
/// `memories` must already be visibility-filtered for the caller.
/// Returns `(selected, receipt)` — selected preserves score order
/// (highest first); the receipt covers every candidate including
/// exclusions with reasons.
pub fn select_memories(
    memories: &[Memory],
    filters: &SelectionFilters,
    budget_chars: usize,
) -> (Vec<Memory>, SelectionReceipt) {
    let mut scored: Vec<(i64, &Memory, String)> = Vec::new();
    for m in memories {
        let mut reason = String::new();
        if m.invalidated {
            reason = "invalidated".into();
        } else if m.superseded_by.is_some() {
            reason = "superseded".into();
        } else if !filters.types.is_empty() && !filters.types.contains(&m.memory_type) {
            reason = "type-filtered".into();
        } else if !filters.tags.is_empty() && !m.has_any_tag(&filters.tags) {
            reason = "tag-filtered".into();
        } else if let Some(days) = filters.recent_days {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
            if m.created < cutoff.format("%Y-%m-%d").to_string() {
                reason = "stale".into();
            }
        }
        scored.push((
            score(m, filters.query.as_deref(), filters.recent_days),
            m,
            reason,
        ));
    }
    // Order: eligible first by score desc (stable by id for ties),
    // excluded last.
    scored.sort_by(|a, b| {
        (a.2.is_empty())
            .cmp(&(b.2.is_empty()))
            .reverse()
            .then(b.0.cmp(&a.0))
            .then(a.1.id.cmp(&b.1.id))
    });

    let mut selected = Vec::new();
    let mut used = 0usize;
    let mut candidates = Vec::with_capacity(scored.len());
    for (sc, m, mut reason) in scored {
        let mut picked = false;
        if reason.is_empty() {
            let size = rendered_size(m);
            if used + size <= budget_chars {
                used += size;
                selected.push(m.clone());
                picked = true;
            } else {
                reason = "over-budget".into();
            }
        }
        candidates.push(ReceiptCandidate {
            id: m.id.clone(),
            score: sc,
            selected: picked,
            exclusion_reason: reason,
            tags: m.tags.clone(),
        });
    }
    (
        selected,
        SelectionReceipt {
            schema_version: "memory-selection-receipt/v1".into(),
            query: filters.query.clone(),
            budget_chars,
            used_chars: used,
            candidates,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryType, MemoryVisibility};

    fn mem(id: &str, content: &str, tags: &[&str]) -> Memory {
        Memory {
            id: id.into(),
            memory_type: MemoryType::Pattern,
            content: content.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            created: "2026-10-01".into(),
            owner_hat_id: None,
            visibility: MemoryVisibility::Shared,
            applicability: None,
            evidence_source: None,
            last_verified: None,
            invalidated: false,
            superseded_by: None,
        }
    }

    #[test]
    fn invalidated_and_superseded_are_never_selected() {
        let mut dead = mem("mem-1-a1b2", "use X", &[]);
        dead.invalidated = true;
        let mut old = mem("mem-2-b2c3", "use Y", &[]);
        old.superseded_by = Some("mem-3-d4e5".into());
        let live = mem("mem-3-d4e5", "use Z", &[]);
        let (sel, receipt) =
            select_memories(&[dead, old, live], &SelectionFilters::default(), 10_000);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].id, "mem-3-d4e5");
        assert!(
            receipt
                .candidates
                .iter()
                .any(|c| c.exclusion_reason == "invalidated")
        );
        assert!(
            receipt
                .candidates
                .iter()
                .any(|c| c.exclusion_reason == "superseded")
        );
    }

    #[test]
    fn budget_packs_whole_entries_only() {
        let big = mem("mem-1-a1b2", &"x".repeat(900), &[]);
        let small = mem("mem-2-b2c3", "tiny", &[]);
        let (sel, receipt) = select_memories(&[big, small], &SelectionFilters::default(), 200);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].content, "tiny");
        assert!(
            receipt
                .candidates
                .iter()
                .any(|c| c.exclusion_reason == "over-budget")
        );
    }

    #[test]
    fn query_scores_relevant_entries_first() {
        let a = mem("mem-1-a1b2", "database retry policy", &["db"]);
        let b = mem("mem-2-b2c3", "unrelated note", &[]);
        let (sel, receipt) = select_memories(
            &[b, a],
            &SelectionFilters {
                query: Some("database retry".into()),
                ..Default::default()
            },
            10_000,
        );
        assert_eq!(sel[0].content, "database retry policy");
        let a_score = receipt
            .candidates
            .iter()
            .find(|c| c.id == "mem-1-a1b2")
            .unwrap()
            .score;
        assert!(a_score > 0);
    }

    #[test]
    fn receipt_carries_no_content() {
        let secret = mem("mem-9-z9z9", "PRIVATE-SECRET-TEXT", &[]);
        let (_sel, receipt) = select_memories(&[secret], &SelectionFilters::default(), 10_000);
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(!json.contains("PRIVATE-SECRET-TEXT"));
    }
}
