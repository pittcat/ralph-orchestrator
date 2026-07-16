//! Wave worker prompt builder.
//!
//! Constructs focused prompts for individual wave worker instances,
//! providing task context and constraints to keep workers on track.

use crate::config::HatConfig;
use crate::event_reader::Event;

/// Context for a wave worker instance.
#[derive(Debug)]
pub struct WaveWorkerContext {
    /// Wave correlation ID (e.g., "w-1a2b3c4d").
    pub wave_id: String,
    /// 0-based index of this worker within the wave.
    pub wave_index: u32,
    /// Total number of workers in this wave.
    pub wave_total: u32,
    /// Topics this worker should publish results to.
    pub result_topics: Vec<String>,
    /// Dimension this worker is hard-bound to (parsed from the
    /// `review.wave.ready` payload's `dimension` field). When
    /// `Some`, the worker MUST emit `review.dimension.done` with
    /// exactly this dimension; mismatch is rejected by the CLI
    /// precheck (R3) and dropped at merge (R4). `None` for waves
    /// that do not carry a dimension assignment.
    pub assigned_dimension: Option<String>,
}

/// Builds a focused prompt for a wave worker instance.
///
/// The prompt contains:
/// 1. Hat instructions (what the worker does)
/// 2. Wave context (worker identity within the wave)
/// 3. Task payload (the specific work item)
/// 4. Publishing guide (how to emit results)
/// 5. Constraints (nested wave prohibition, focus directive)
pub fn build_wave_worker_prompt(hat: &HatConfig, event: &Event, ctx: &WaveWorkerContext) -> String {
    let mut prompt = String::new();

    // 1. Instructions
    if !hat.instructions.trim().is_empty() {
        prompt.push_str("# Instructions\n\n");
        prompt.push_str(&hat.instructions);
        if !hat.instructions.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    // 2. Wave context
    prompt.push_str("# Wave Context\n\n");
    prompt.push_str(&format!(
        "You are worker **{}/{}** in wave `{}`.\n\
         Each worker in this wave processes one task independently and in parallel.\n\
         Focus exclusively on your assigned task below.\n\n",
        ctx.wave_index + 1,
        ctx.wave_total,
        ctx.wave_id,
    ));

    // 2b. Assigned dimension block (R2).
    // Surfaced for workers spawned from a `review.wave.ready` wave.
    // The HARD RULE in the preset (U6) tells the agent the CLI
    // precheck enforces this value; we still surface it here so
    // the prompt is self-describing.
    if let Some(ref dim) = ctx.assigned_dimension {
        prompt.push_str(&format!(
            "## ASSIGNED DIMENSION: {dim}\n\n\
             You MUST emit `review.dimension.done` with `dimension` exactly equal to `{dim}`.\n\
             Any other value will be rejected by the CLI precheck and dropped at merge.\n\n"
        ));
    }

    // 3. Task payload
    prompt.push_str("# Your Task\n\n");
    match event.payload.as_ref().map(|p| p.trim()) {
        Some(payload) if !payload.is_empty() => {
            prompt.push_str(payload);
        }
        _ => {
            prompt.push_str(
                "⚠️ **WARNING: No specific task payload provided.**\n\n\
                 This is an error condition — the wave was created without the required\n\
                 task data (e.g., dimension, focus, files to review).\n\n\
                 Do NOT attempt to guess or proceed with an unspecified task.\n\
                 Instead, publish a single diagnostic event indicating the wave\n\
                 worker received an empty task payload. Do NOT produce code reviews,\n\
                 findings, or any substantive work.\n",
            );
        }
    }
    prompt.push('\n');

    // 4. Publishing results
    if !ctx.result_topics.is_empty() {
        prompt.push_str("# Publishing Results\n\n");
        prompt.push_str("When your work is complete, publish your results using `ralph emit`:\n\n");
        for topic in &ctx.result_topics {
            prompt.push_str(&format!(
                "```bash\nralph emit {} \"<your result payload>\"\n```\n\n",
                topic
            ));
        }
    }

    // 5. Constraints
    prompt.push_str("# Constraints\n\n");
    prompt.push_str(
        "- **DO NOT** use `ralph wave emit` — nested wave dispatch is prohibited.\n\
         - Focus exclusively on your assigned task. Do not attempt work assigned to other workers.\n\
         - Publish exactly one result event when complete.\n",
    );

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hat_config() -> HatConfig {
        let yaml = r#"
            name: "Reviewer"
            triggers: ["review.file"]
            publishes: ["review.done"]
            instructions: "Review the file for bugs and style issues."
        "#;
        serde_yaml::from_str(yaml).unwrap()
    }

    fn make_event(payload: &str) -> Event {
        Event {
            topic: "review.file".to_string(),
            payload: Some(payload.to_string()),
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some("w-test1234".to_string()),
            wave_index: Some(0),
            wave_total: Some(3),
            system_injected: None,
        }
    }

    #[test]
    fn test_build_wave_worker_prompt_contains_all_sections() {
        let hat = make_hat_config();
        let event = make_event("src/main.rs");
        let ctx = WaveWorkerContext {
            wave_id: "w-test1234".to_string(),
            wave_index: 0,
            wave_total: 3,
            result_topics: vec!["review.done".to_string()],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);

        assert!(prompt.contains("# Instructions"));
        assert!(prompt.contains("Review the file for bugs"));
        assert!(prompt.contains("# Wave Context"));
        assert!(prompt.contains("worker **1/3**"));
        assert!(prompt.contains("w-test1234"));
        assert!(prompt.contains("# Your Task"));
        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("# Publishing Results"));
        assert!(prompt.contains("ralph emit review.done"));
        assert!(prompt.contains("# Constraints"));
        assert!(prompt.contains("DO NOT"));
    }

    #[test]
    fn test_worker_index_is_1_based_in_display() {
        let hat = make_hat_config();
        let event = make_event("file.rs");
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 2,
            wave_total: 5,
            result_topics: vec![],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);
        assert!(prompt.contains("worker **3/5**"));
    }

    #[test]
    fn test_empty_instructions_omitted() {
        let yaml = r#"
            name: "Reviewer"
            triggers: ["review.file"]
            publishes: ["review.done"]
            instructions: ""
        "#;
        let hat: HatConfig = serde_yaml::from_str(yaml).unwrap();
        let event = make_event("payload");
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec![],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);
        assert!(!prompt.contains("# Instructions"));
    }

    #[test]
    fn test_no_result_topics_skips_publishing_section() {
        let hat = make_hat_config();
        let event = make_event("payload");
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec![],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);
        assert!(!prompt.contains("# Publishing Results"));
    }

    #[test]
    fn test_empty_payload_shows_warning() {
        let hat = make_hat_config();
        let event = make_event(""); // empty payload
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec!["review.done".to_string()],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);

        assert!(prompt.contains("No specific task payload provided"));
        assert!(prompt.contains("WARNING"));
        assert!(prompt.contains("Do NOT attempt to guess"));
    }

    #[test]
    fn test_whitespace_only_payload_shows_warning() {
        let hat = make_hat_config();
        let event = make_event("   \n  \t  "); // whitespace-only payload
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec!["review.done".to_string()],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);

        assert!(prompt.contains("No specific task payload provided"));
        assert!(prompt.contains("WARNING"));
    }

    #[test]
    fn test_missing_payload_shows_warning() {
        let hat = make_hat_config();
        let event = Event {
            topic: "review.file".to_string(),
            payload: None, // no payload at all
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some("w-abc".to_string()),
            wave_index: Some(0),
            wave_total: Some(1),
            system_injected: None,
        };
        let ctx = WaveWorkerContext {
            wave_id: "w-abc".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec!["review.done".to_string()],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);

        assert!(prompt.contains("No specific task payload provided"));
        assert!(prompt.contains("WARNING"));
    }

    /// U1/R1 — when `assigned_dimension` is set, the prompt MUST
    /// contain a `## ASSIGNED DIMENSION: <dim>` block naming it.
    /// The agent uses this to know which dimension's review.dimension.done
    /// value is valid (R2/R8).
    #[test]
    fn test_assigned_dimension_renders_in_prompt() {
        let hat = make_hat_config();
        let event = make_event("src/main.rs");
        let ctx = WaveWorkerContext {
            wave_id: "w-test1234".to_string(),
            wave_index: 0,
            wave_total: 3,
            result_topics: vec!["review.dimension.done".to_string()],
            assigned_dimension: Some("testing".to_string()),
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);
        assert!(
            prompt.contains("## ASSIGNED DIMENSION: testing"),
            "prompt must contain the assigned dimension block; got: {prompt}"
        );
    }

    /// U1/R1 — when `assigned_dimension` is None, the prompt MUST
    /// NOT contain the assignment block (legacy waves).
    #[test]
    fn test_no_assigned_dimension_omits_block() {
        let hat = make_hat_config();
        let event = make_event("src/main.rs");
        let ctx = WaveWorkerContext {
            wave_id: "w-test1234".to_string(),
            wave_index: 0,
            wave_total: 1,
            result_topics: vec!["review.done".to_string()],
            assigned_dimension: None,
        };

        let prompt = build_wave_worker_prompt(&hat, &event, &ctx);
        assert!(!prompt.contains("## ASSIGNED DIMENSION:"));
    }
}
