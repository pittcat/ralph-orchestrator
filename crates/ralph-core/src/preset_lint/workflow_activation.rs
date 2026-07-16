//! WAC-U1: Workflow Activation Contract (WAC) static rules.
//!
//! Re-emit trap (R2), activation egress (R3), handoff pairing (R4),
//! and trigger/publish asymmetry (R5). All four rules consume a
//! shared [`HandoffGraph`] built from [`RalphConfig::hats`] (using
//! `publishes` only — `default_publishes` is intentionally excluded
//! per KTD-5 / MH-U3 alignment).
//!
//! The WAC rule family lives next to the existing U1/U2/U3 lint
//! modules (`topic_format`, `ownership`, `coordinator`, `multi_hat`)
//! so it can be wired into [`run_preset_lint`](crate::preset_lint::run_preset_lint)
//! with no change to the orchestrator's public surface.
//!
//! Self-loop exemption: a hat that triggers AND publishes the same
//! topic T (`T ∈ H.triggers ∩ H.publishes`) is exempt from R2, per
//! KTD-4 / origin Outstanding Questions default.
//!
//! Wildcard `*` subscribers are NOT considered unique consumers for
//! the R4 pairing rule, per R9 / KTD-6.
//!
//! Plan Unit: WAC-U1 of `2026-06-12-002-feat-workflow-activation-contract-plan`.

use std::collections::{HashMap, HashSet};

use crate::config::RalphConfig;

// ──────────────────────────────────────────────────────────────────────────
// Handoff graph
// ──────────────────────────────────────────────────────────────────────────

/// Topic-triggered adjacency view of `RalphConfig.hats` for the WAC rule family.
///
/// Constructed once per preset via [`HandoffGraph::from_config`]; shared
/// by `check_re_emit_trap` / `check_activation_egress` /
/// `check_handoff_pairing` / `check_trigger_publish_asymmetry` so the
/// four rules cannot drift on what "publisher" / "subscriber" mean.
///
/// Per KTD-5: only `publishes` is consulted; `default_publishes` does
/// not influence WAC findings. The graph is intentionally distinct
/// from `preset_validator::TopologyGraph` (KTD-3) — WAC semantics
/// differ from completion / required_events BFS, and a shared graph
/// would conflate two purposes.
#[derive(Debug, Clone, Default)]
pub struct HandoffGraph {
    /// Topics → hats that explicitly publish them.
    pub topic_publishers: HashMap<String, Vec<String>>,
    /// Topics → hats whose `triggers` include them. `*` (wildcard)
    /// is recorded in `wildcard_subscribers` rather than per-topic.
    pub topic_subscribers: HashMap<String, Vec<String>>,
    /// Hats whose `triggers` contains the literal `*`.
    pub wildcard_subscribers: Vec<String>,
    /// Hat IDs in deterministic order (matches `RalphConfig.hats`).
    pub hat_order: Vec<String>,
    /// Side index: hat_id → topics it publishes. Used by the bounded
    /// BFS in `reaches_progress_endpoint`. Inverted from
    /// `topic_publishers` in [`HandoffGraph::from_config`].
    pub hat_publishes: HashMap<String, Vec<String>>,
}

impl HandoffGraph {
    /// Build a `HandoffGraph` from `RalphConfig::hats`.
    ///
    /// `triggers` are the explicit per-hat trigger list. The graph
    /// does not consult `phase_triggers`: the WAC rules evaluate the
    /// full surface of triggers a hat *could* receive. Hat names are
    /// read as the hashmap keys; phases are not part of the graph.
    ///
    /// 2026-07-03-001 plan U9/U13: when `event_loop.supervisor.enabled`
    /// is `true`, a virtual `supervisor` node is wired into the graph.
    /// It consumes the slot-level `*.unit.done` / `*.unit.failed` topics
    /// and emits the corresponding wave-level `*.wave.complete` /
    /// `*.wave.failed` topics. This lets the WAC rules see the fan-in
    /// handoff that the runtime implements via `system_injected`, so
    /// worker hats are not flagged as dead ends.
    pub fn from_config(config: &RalphConfig) -> Self {
        let mut topic_publishers: HashMap<String, Vec<String>> = HashMap::new();
        let mut topic_subscribers: HashMap<String, Vec<String>> = HashMap::new();
        let mut wildcard_subscribers: Vec<String> = Vec::new();
        let mut hat_publishes: HashMap<String, Vec<String>> = HashMap::new();
        let mut hat_order: Vec<String> = config.hats.keys().cloned().collect();

        for (hat_id, hat) in &config.hats {
            for topic in &hat.publishes {
                topic_publishers
                    .entry(topic.clone())
                    .or_default()
                    .push(hat_id.clone());
                hat_publishes
                    .entry(hat_id.clone())
                    .or_default()
                    .push(topic.clone());
            }
            for trigger in &hat.triggers {
                if trigger == "*" {
                    wildcard_subscribers.push(hat_id.clone());
                } else {
                    topic_subscribers
                        .entry(trigger.clone())
                        .or_default()
                        .push(hat_id.clone());
                }
            }
        }

        // U13 supervisor fan-in: add virtual supervisor edges so WAC
        // sees slot-to-wave handoffs as closed paths.
        if config.event_loop.supervisor.enabled {
            const SUPERVISOR_SLOT_TO_WAVE: &[(&str, &str)] = &[
                ("exec.unit.done", "exec.wave.complete"),
                ("exec.unit.failed", "exec.wave.failed"),
                ("fix.unit.done", "fix.wave.complete"),
                ("fix.unit.failed", "fix.wave.failed"),
                ("review.unit.done", "review.wave.complete"),
                ("review.unit.failed", "review.wave.failed"),
            ];
            let supervisor_id = "supervisor".to_string();
            hat_order.push(supervisor_id.clone());
            for (slot_topic, wave_topic) in SUPERVISOR_SLOT_TO_WAVE {
                topic_subscribers
                    .entry((*slot_topic).to_string())
                    .or_default()
                    .push(supervisor_id.clone());
                topic_publishers
                    .entry((*wave_topic).to_string())
                    .or_default()
                    .push(supervisor_id.clone());
                hat_publishes
                    .entry(supervisor_id.clone())
                    .or_default()
                    .push((*wave_topic).to_string());
            }
        }

        hat_order.sort();

        // Sort the value vectors for deterministic test output.
        for v in topic_publishers.values_mut() {
            v.sort();
            v.dedup();
        }
        for v in topic_subscribers.values_mut() {
            v.sort();
            v.dedup();
        }
        for v in hat_publishes.values_mut() {
            v.sort();
            v.dedup();
        }
        wildcard_subscribers.sort();
        wildcard_subscribers.dedup();

        Self {
            topic_publishers,
            topic_subscribers,
            wildcard_subscribers,
            hat_order,
            hat_publishes,
        }
    }

    /// Topics whose only consumers (non-wildcard) are a single hat.
    ///
    /// Per R9 / KTD-6: wildcard subscribers are treated as
    /// additional consumers, so a topic with (1 explicit + 1
    /// wildcard) consumer is **not** considered unique and will
    /// not enable handoff priority dispatch. A topic with no
    /// explicit subscribers at all (only wildcard) is also not
    /// unique — wildcard alone cannot provide a deterministic
    /// handoff target.
    pub fn unique_consumer_topics(&self) -> HashSet<String> {
        if !self.wildcard_subscribers.is_empty() {
            // Any wildcard subscriber invalidates the "exactly one
            // consumer" guarantee across the whole preset.
            return HashSet::new();
        }
        self.topic_subscribers
            .iter()
            .filter(|(_, hats)| hats.len() == 1)
            .map(|(topic, _)| topic.clone())
            .collect()
    }

    /// The single non-wildcard consumer of a topic, if any.
    pub fn unique_consumer_of(&self, topic: &str) -> Option<&str> {
        self.topic_subscribers.get(topic).and_then(|hats| {
            if hats.len() == 1 {
                Some(hats[0].as_str())
            } else {
                None
            }
        })
    }

    /// Publishers of a topic in deterministic order.
    pub fn publishers_of(&self, topic: &str) -> &[String] {
        self.topic_publishers
            .get(topic)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

// ──────────────────────────────────────────────────────────────────────────
// WAC rule: LintFinding output
// ──────────────────────────────────────────────────────────────────────────

/// A WAC rule's verdict for a single (hat, topic) or `(hat, hat, topic)` shape.
///
/// The four rules are surfaced as `LintFinding` entries so the existing
/// `lint_findings_to_contract_findings` adapter applies the canonical
/// `lint.` prefix and `FindingSource::Lint` envelope.
pub(crate) type WacFinding = crate::preset_lint::LintFinding;

/// WAC severity override. The WRC-U3 (2026-06-12-003) rule
/// applies KTD-7: builtin-embedded presets always produce
/// `Error`, regardless of `strict`. User-authored presets
/// follow the legacy mapping: `Warn` in default mode, `Error`
/// in strict mode. The `source_is_builtin_embedded` flag is
/// computed by the aggregator from the report's `source_label`
/// against `presets/manifest.yml`'s `embedded` list.
///
/// WAC rules never produce `Pass` — every firing is a real defect.
pub(crate) fn wac_severity(
    strict: bool,
    source_is_builtin_embedded: bool,
) -> crate::preset_lint::LintSeverity {
    if source_is_builtin_embedded {
        // KTD-7: builtin presets are part of the public contract
        // and must always surface WAC defects as blocking errors.
        return crate::preset_lint::LintSeverity::Error;
    }
    if strict {
        crate::preset_lint::LintSeverity::Error
    } else {
        crate::preset_lint::LintSeverity::Warn
    }
}

/// WRC-U3 / R-WRC-06: a `source_label` describes a builtin-embedded
/// preset if and only if it starts with the canonical
/// `builtin:<name>` prefix. The CLI is the source of truth for
/// which `<name>` values are valid (the embedded preset manifest
/// in `presets/manifest.yml` and the `crates/ralph-cli/src/presets.rs`
/// `PRESETS` array); the core aggregator only needs the prefix
/// heuristic, not the full list, because the WAC-severity upgrade
/// is the same `Error` for every builtin entry.
///
/// The CLI gate (`enforce_preset_lint_gate`) can also accept an
/// explicit `true` flag when the CLI already knows the preset came
/// from `-H builtin:foo`. The two paths are kept in lockstep by
/// the CLI tests (the gate path and the `preset check` path both
/// produce the same WAC severity for a builtin source).
pub fn source_label_is_builtin_embedded(source_label: &str) -> bool {
    source_label.starts_with("builtin:")
}

// ──────────────────────────────────────────────────────────────────────────
// R2: Re-emit trap
// ──────────────────────────────────────────────────────────────────────────

/// R2 (narrow semantics, WRC-U2 / R-WRC-03 / KTD-WRC-1, plan 003):
/// A hat H that triggers on topic T — published by another hat
/// P ≠ H — but does not declare T in its own `publishes` is a
/// re-emit hazard **only when the handoff is a dead end**:
/// H is the unique non-wildcard consumer of T, H's publishes do
/// not reach any downstream hat trigger or terminal topic, and
/// T is not in H's publishes. In that case the workflow
/// effectively drops T after H consumes it, and any expectation
/// that H would re-emit T (e.g. as a terminal signal) is unmet.
///
/// The plan-002 literal reading of R2 ("any hat that triggers a
/// topic it does not publish is a re-emit trap") would fire on
/// almost every normal hat in the system: the executor triggers
/// `work.ready` (published by plan-gate) and only publishes
/// `work.done`, `work.failed` — by the literal reading that is
/// always a trap. The narrow reading instead asks: does the
/// handoff close the workflow stage, or does it drop the topic
/// on the floor? Only the latter is a real re-emit trap. This
/// is the canonical R2 definition for 003; the 002 plan's
/// literal wording was correct as a starting point but the
/// implementation was always narrow (and the test suite already
/// encodes the narrow behaviour). The 002 plan is marked
/// `partial-complete` to record the wording drift.
///
/// R2 fires only when ALL of the following hold:
/// - H triggers T (and T is not `*`)
/// - T ∉ H.publishes (no self-loop)
/// - H is the unique non-wildcard consumer of T (a handoff)
/// - T has a publisher P ≠ H
/// - H's publishes have no closure path (per R3's BFS): none
///   of H's publishes reach a downstream hat trigger or
///   terminal topic within 2 hops
///
/// R2 and R4 are intentional complements: R4 names the
/// "handoff pairing broken" symptom; R2 names the "re-emit
/// trap" symptom. They both fire for the same dead-end handoff
/// and together tell the operator the same story from two
/// angles. When R2 fires on a hat with a healthy closure path
/// (a normal hat like the executor), the implementation
/// suppresses the finding — see `re_emit_trap_does_not_fire_on_healthy_handoff_chain`
/// for the canonical positive test.
///
/// Wildcard subscribers disqualify uniqueness per R9 / KTD-6.
pub fn check_re_emit_trap(
    config: &RalphConfig,
    graph: &HandoffGraph,
    strict: bool,
    source_is_builtin_embedded: bool,
) -> Vec<WacFinding> {
    let mut findings = Vec::new();
    let severity = wac_severity(strict, source_is_builtin_embedded);

    let unique_topics = graph.unique_consumer_topics();
    let terminals = collect_terminal_topics(config);

    for (hat_id, hat) in &config.hats {
        let publishes: HashSet<&str> = hat.publishes.iter().map(String::as_str).collect();
        for trigger in &hat.triggers {
            if trigger == "*" {
                continue;
            }
            // Self-loop exemption.
            if publishes.contains(trigger.as_str()) {
                continue;
            }
            // H must be the unique non-wildcard consumer of T.
            if !unique_topics.contains(trigger) {
                continue;
            }
            let Some(unique_consumer) = graph.unique_consumer_of(trigger) else {
                continue;
            };
            if unique_consumer != hat_id {
                continue;
            }
            // And T must be published by some other hat.
            let publishers = graph.publishers_of(trigger);
            let has_external_publisher = publishers.iter().any(|p| p != hat_id);
            if !has_external_publisher {
                continue;
            }
            // Closure-path check: H must not have any publish
            // that reaches a downstream hat trigger or terminal.
            // If H does have such a path, the handoff is a normal
            // consumption pattern and R2 does not apply.
            let has_closure = hat
                .publishes
                .iter()
                .any(|p| reaches_progress_endpoint(p, graph, &terminals, EGRESS_MAX_HOPS));
            if has_closure {
                continue;
            }
            findings.push(WacFinding {
                id: crate::preset_lint::finding_id::FINDING_RE_EMIT_TRAP,
                severity,
                message: format!(
                    "hat \"{hat_id}\" is the unique consumer of topic \"{trigger}\" \
                     (a handoff) but does not declare \"{trigger}\" in its publishes \
                     and none of its publishes reach a downstream hat trigger or \
                     terminal; this is a re-emit trap — if the workflow expects \
                     \"{hat_id}\" to re-emit \"{trigger}\" (e.g. as a terminal signal), \
                     it cannot"
                ),
                topic: Some(trigger.clone()),
                hat: Some(hat_id.clone()),
                owner: None,
                action_hint: Some(format!(
                    "Add a publish topic on hat \"{hat_id}\" that reaches a downstream \
                     hat trigger or terminal/completion topic; or, if \"{hat_id}\" \
                     should re-emit \"{trigger}\" itself, add \"{trigger}\" to \
                     \"{hat_id}\" publishes"
                )),
            });
        }
    }
    findings
}

// ──────────────────────────────────────────────────────────────────────────
// R3: Activation egress
// ──────────────────────────────────────────────────────────────────────────

/// R3: For each (hat H, trigger topic T) pair, at least one of H's
/// declared `publishes` topics must reach a "progressed" endpoint.
/// Endpoints are:
///
/// - another hat's `triggers` (a downstream workflow hat will pick up the work), or
/// - a known terminal/completion topic set.
///
/// Bound history: 4 → 8 (2026-06-24 10-hat refactor) → 9 → 10
/// (2026-07-02-003 `ce-executor-pipeline` 13-hat flat chain) → 12
/// (2026-07-08 `ce-executor-pipeline-loop` 15-hat chain).
///
/// The 12-hop bound accommodates the `ce-executor-pipeline-loop`
/// preset, which adds `review-reentry` and `review-gate` hats on top
/// of the `ce-executor-pipeline` flat serial chain. The `fix-planner`
/// hat in the loop preset is 11 hops from `report.done`:
/// `fix-planner → fixer → review-reentry → dim:goal-alignment →
/// dim:correctness → dim:testing → dim:maintainability →
/// dim:project-standards → dim:adversarial → review-synthesizer →
/// review-gate → (review.accepted) → alignment → reporter →
/// report.done`. The BFS budget must cover 11 hops so the loop
/// preset's chain terminates through the lint check.
///
/// 12 hops is still tight enough to catch genuine dead ends: a hat
/// publishing to a topic with no consumer at all fails at hop 1, and
/// a handoff chain that dead-ends mid-way fails well before 12. The
/// bound only limits how deep the BFS searches for a *valid* path to
/// a terminal; it does not affect detection of truly broken
/// topologies. The T-U1-03 test fixture uses a 1-hop chain and
/// continues to fire under the wider bound.
const EGRESS_MAX_HOPS: usize = 12;

pub fn check_activation_egress(
    config: &RalphConfig,
    graph: &HandoffGraph,
    strict: bool,
    source_is_builtin_embedded: bool,
) -> Vec<WacFinding> {
    let mut findings = Vec::new();
    let severity = wac_severity(strict, source_is_builtin_embedded);

    // Terminal / completion topics derived from event_loop config.
    // These represent "the workflow can end here" — egress reaching
    // any of them is sufficient (KTD-3 + R3 Endpoint (b)).
    let terminals = collect_terminal_topics(config);

    for (hat_id, hat) in &config.hats {
        if hat.triggers.is_empty() {
            // Hats with no triggers never get activated; R3 does not apply.
            continue;
        }
        let has_egress = hat
            .publishes
            .iter()
            .any(|p| reaches_progress_endpoint(p, graph, &terminals, EGRESS_MAX_HOPS));
        if !has_egress {
            findings.push(WacFinding {
                id: crate::preset_lint::finding_id::FINDING_ACTIVATION_EGRESS_MISSING,
                severity,
                message: format!(
                    "hat \"{hat_id}\" has no activation egress: none of its publishes reach \
                     a downstream hat trigger or a terminal/completion topic within {EGRESS_MAX_HOPS} hops"
                ),
                topic: None,
                hat: Some(hat_id.clone()),
                owner: None,
                action_hint: Some(format!(
                    "Add a publish topic on hat \"{hat_id}\" that (a) is consumed by \
                     another hat's triggers, or (b) reaches completion_promise/required_events"
                )),
            });
        }
    }
    findings
}

fn collect_terminal_topics(config: &RalphConfig) -> HashSet<String> {
    let mut terminals: HashSet<String> = HashSet::new();
    terminals.insert(config.event_loop.completion_promise.clone());
    if !config.event_loop.cancellation_promise.is_empty() {
        terminals.insert(config.event_loop.cancellation_promise.clone());
    }
    for req in &config.event_loop.required_events {
        terminals.insert(req.clone());
    }
    terminals
}

/// Bounded BFS: does `start_topic` reach a terminal/completion topic
/// or a downstream hat's trigger within `max_hops`?
fn reaches_progress_endpoint(
    start_topic: &str,
    graph: &HandoffGraph,
    terminals: &HashSet<String>,
    max_hops: usize,
) -> bool {
    if terminals.contains(start_topic) {
        return true;
    }
    if max_hops == 0 {
        return false;
    }
    // Hop 1: from start_topic, follow subscribers to the next hat frontier.
    let frontier: Vec<String> = graph
        .topic_subscribers
        .get(start_topic)
        .cloned()
        .unwrap_or_default();
    if frontier.is_empty() && graph.wildcard_subscribers.is_empty() {
        return false;
    }
    let next_hats: Vec<&str> = frontier
        .iter()
        .map(String::as_str)
        .chain(graph.wildcard_subscribers.iter().map(String::as_str))
        .collect();
    // Each frontier hat's publishes (excluding start_topic itself) is
    // the next hop frontier. We cap recursion at `max_hops - 1`.
    // Track visited topics to break cycles deterministically.
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start_topic.to_string());
    for hat in next_hats {
        if let Some(hat_pubs) = graph.hat_publishes.get(hat) {
            for publish in hat_pubs {
                if visited.contains(publish) {
                    continue;
                }
                if reaches_progress_endpoint(publish, graph, terminals, max_hops - 1) {
                    return true;
                }
                visited.insert(publish.clone());
            }
        }
    }
    false
}

// ──────────────────────────────────────────────────────────────────────────
// R4: Handoff pairing
// ──────────────────────────────────────────────────────────────────────────

/// R4: When a topic T is published by hat A and has exactly one
/// consumer hat B, B's activation egress (R3) must reach a
/// "next business stage" endpoint. In the current scope, the
/// business-stage endpoints are the same terminal / completion
/// set used by R3.
///
/// The rule produces a finding per `(A, B, T)` triple where B is
/// the unique consumer of T and B has no egress to a terminal.
pub fn check_handoff_pairing(
    config: &RalphConfig,
    graph: &HandoffGraph,
    strict: bool,
    source_is_builtin_embedded: bool,
) -> Vec<WacFinding> {
    let mut findings = Vec::new();
    let severity = wac_severity(strict, source_is_builtin_embedded);

    let terminals = collect_terminal_topics(config);

    for topic in graph.unique_consumer_topics() {
        let Some(consumer) = graph.unique_consumer_of(&topic) else {
            continue;
        };
        let Some(consumer_hat) = config.hats.get(consumer) else {
            continue;
        };
        let has_egress = consumer_hat
            .publishes
            .iter()
            .any(|p| reaches_progress_endpoint(p, graph, &terminals, EGRESS_MAX_HOPS));
        if !has_egress {
            findings.push(WacFinding {
                id: crate::preset_lint::finding_id::FINDING_HANDOFF_PAIRING_BROKEN,
                severity,
                message: format!(
                    "hat \"{consumer}\" is the unique consumer of topic \"{topic}\" \
                     (a handoff), but none of its publishes reach a downstream hat \
                     trigger or terminal/completion topic within 2 hops; the handoff \
                     leads to a dead end"
                ),
                topic: Some(topic.clone()),
                hat: Some(consumer.to_string()),
                owner: None,
                action_hint: Some(format!(
                    "Add a publish topic on hat \"{consumer}\" that connects the handoff \
                     \"{topic}\" to the next business stage (a downstream hat trigger or \
                     completion_promise)"
                )),
            });
        }
    }
    findings
}

// ──────────────────────────────────────────────────────────────────────────
// R5: Trigger / publish asymmetry
// ──────────────────────────────────────────────────────────────────────────

/// R5: For each hat H and each trigger T that H consumes, the
/// "asymmetry" rule fires when the workflow stage T represents
/// cannot close given H's `publishes` set AND the publishers of
/// T. Two specific archetypes trigger R5:
///
/// 1. T has no publisher at all (e.g. `work.retry` with nobody
///    emitting it). H would wait forever for an event that
///    nobody produces.
/// 2. T has a publisher, but H's publishes do not reach any
///    downstream hat trigger or terminal topic (H is a
///    one-shot sink with no continuation).
///
/// R3 owns the per-hat "no publishes reach a terminal" umbrella
/// case. R5 narrows to per-trigger diagnoses so the operator
/// can see exactly which trigger is the problem.
pub fn check_trigger_publish_asymmetry(
    config: &RalphConfig,
    graph: &HandoffGraph,
    strict: bool,
    source_is_builtin_embedded: bool,
) -> Vec<WacFinding> {
    let mut findings = Vec::new();
    let severity = wac_severity(strict, source_is_builtin_embedded);

    // The `starting_event` is injected by the loop runner (ralph
    // hat) at loop start, not by any user-defined hat. Triggers
    // matching `starting_event` are exempt from R5's "no
    // publisher" archetype because the runner owns the emit.
    let starting_event = config.event_loop.starting_event.as_deref();

    // WRC-U3 (2026-06-12-003) / KTD-WRC-3: the `cancellation_promise`
    // is also published by the loop runner (ralph hat), not by any
    // user-defined hat. Triggers on the cancellation_promise are
    // exempt from R5's "no publisher" archetype for the same
    // reason as starting_event: the loop runner injects the
    // publish when cancellation is requested. Without this
    // exemption, every preset that wires a `loop.cancel` trigger
    // (e.g. ce-executor-serial's `plan-gate`) would receive a
    // spurious R5 finding, blocking Tier-0.
    let cancellation_promise = if config.event_loop.cancellation_promise.is_empty() {
        None
    } else {
        Some(config.event_loop.cancellation_promise.as_str())
    };

    // 2026-06-16-001 U5: topics synthesised by the loop runner
    // (not by any user-defined hat) are exempt from R5's "no
    // publisher" archetype for the same reason as
    // cancellation_promise: the runner injects the publish. The
    // `progress-steward` hat is the canonical consumer.
    const RUNNER_INJECTED_TRIGGERS: &[&str] = &["loop.stalled", "task.resume"];

    // R5 is per-trigger and depends only on graph topology (no
    // bounded BFS over terminals), so the terminal set is unused.
    // The call is kept for symmetry with R3 / R4 and to give the
    // compiler a forward anchor if a future revision adds terminal
    // reachability to the asymmetry check.
    let _terminals = collect_terminal_topics(config);

    for (hat_id, hat) in &config.hats {
        if hat.triggers.is_empty() {
            continue;
        }
        for trigger in &hat.triggers {
            if trigger == "*" {
                continue;
            }
            // WRC-U3 / KTD-WRC-3: cancellation_promise is
            // runner-injected, same exemption as starting_event
            // above.
            if Some(trigger.as_str()) == cancellation_promise {
                continue;
            }
            // 2026-06-16-001 U5: runner-injected topics
            // (`loop.stalled`, `task.resume`) are exempt for the
            // same reason — the loop runner is the publisher,
            // not a hat. (2026-06-28-005: `human.guidance` was
            // removed from this list together with the topic.)
            if RUNNER_INJECTED_TRIGGERS.contains(&trigger.as_str()) {
                continue;
            }
            // starting_event exemption: ralph hat owns the emit.
            if Some(trigger.as_str()) == starting_event {
                continue;
            }
            let has_publisher = !graph.publishers_of(trigger).is_empty();
            let has_subscriber = graph
                .topic_subscribers
                .get(trigger)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
                || !graph.wildcard_subscribers.is_empty();

            if has_publisher && has_subscriber {
                // Both ends exist; the R5 narrow case does not
                // apply. R2 would catch re-emit trap; R3 would
                // catch the per-hat closure failure; R4 would
                // catch handoff pairing.
                continue;
            }

            // Archetype 1: no publisher (orphan trigger).
            // Archetype 2: no subscriber (dead end on the consumer
            // side, which is the asymmetry: someone publishes but
            // nobody can pick it up).
            let archetype = if has_publisher {
                "no subscriber"
            } else {
                "no publisher"
            };

            // R5 and R3 can both fire for the same hat — they
            // report different problems. R3 is the umbrella
            // "hat has no closure path" finding; R5 names the
            // specific trigger that is an orphan or dead-end.
            // Suppress R5 only when the trigger name is empty
            // (defensive: should never happen because we filtered
            // `*` above).
            if trigger.is_empty() {
                continue;
            }

            findings.push(WacFinding {
                id: crate::preset_lint::finding_id::FINDING_TRIGGER_PUBLISH_ASYMMETRY,
                severity,
                message: format!(
                    "hat \"{hat_id}\" triggers on topic \"{trigger}\" which has \
                     {archetype}; the workflow stage that \"{trigger}\" represents \
                     cannot close"
                ),
                topic: Some(trigger.clone()),
                hat: Some(hat_id.clone()),
                owner: None,
                action_hint: Some(format!(
                    "Add a publisher hat that emits \"{trigger}\" (if archetype is \
                     'no publisher'), or add a subscriber hat whose triggers include \
                     \"{trigger}\" (if archetype is 'no subscriber')"
                )),
            });
        }
    }
    findings
}

// ──────────────────────────────────────────────────────────────────────────
// Combined WAC entry point
// ──────────────────────────────────────────────────────────────────────────

/// Run all four WAC rules and return findings in deterministic order.
///
/// This is the entry point used by
/// [`run_preset_lint`](crate::preset_lint::run_preset_lint). Each
/// rule returns `Vec<LintFinding>` so the existing aggregator can
/// prefix them with `lint.` and feed them through the contract
/// reporter.
///
/// WRC-U3 / KTD-7: `source_is_builtin_embedded` upgrades every
/// WAC finding to `Error` regardless of `strict`. The aggregator
/// computes this flag from the report's `source_label` against
/// the `presets/manifest.yml` `embedded` list (and the
/// `crates/ralph-cli/src/presets.rs` `PRESETS` array, which the
/// build script keeps in lockstep). Direct callers (BDD scenarios,
/// diagnostic CLI) can pass `false` to opt out of the upgrade.
pub fn run_workflow_activation_contract(
    config: &RalphConfig,
    strict: bool,
    source_is_builtin_embedded: bool,
) -> Vec<WacFinding> {
    let graph = HandoffGraph::from_config(config);
    let mut findings = Vec::new();
    findings.extend(check_re_emit_trap(
        config,
        &graph,
        strict,
        source_is_builtin_embedded,
    ));
    findings.extend(check_activation_egress(
        config,
        &graph,
        strict,
        source_is_builtin_embedded,
    ));
    findings.extend(check_handoff_pairing(
        config,
        &graph,
        strict,
        source_is_builtin_embedded,
    ));
    findings.extend(check_trigger_publish_asymmetry(
        config,
        &graph,
        strict,
        source_is_builtin_embedded,
    ));

    // Deterministic order: (id, topic, hat).
    findings.sort_by(|a, b| {
        a.id.cmp(b.id)
            .then(a.topic.cmp(&b.topic))
            .then(a.hat.cmp(&b.hat))
    });
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset_lint::LintSeverity;

    // T-U1-01: re-emit trap fires on executor+queue.advance
    #[test]
    fn re_emit_trap_fires_when_other_hat_publishes_trigger() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["queue.advance"]
  executor:
    name: "Executor"
    triggers: ["queue.advance"]
    publishes: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings =
            check_re_emit_trap(&config, &HandoffGraph::from_config(&config), true, false);
        assert!(
            findings.iter().any(
                |f| f.id == crate::preset_lint::finding_id::FINDING_RE_EMIT_TRAP
                    && f.topic.as_deref() == Some("queue.advance")
                    && f.hat.as_deref() == Some("executor")
            ),
            "expected re_emit_trap finding on executor+queue.advance, got: {:?}",
            findings
        );
    }

    // T-U1-02: self-loop exemption
    #[test]
    fn re_emit_trap_self_loop_exempt() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  hat_a:
    name: "HatA"
    triggers: ["work.start", "work.retried"]
    publishes: ["work.retried", "work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings =
            check_re_emit_trap(&config, &HandoffGraph::from_config(&config), true, false);
        assert!(
            findings.is_empty(),
            "self-loop should be exempt: {:?}",
            findings
        );
    }

    // T-U1-03: hat with no egress
    #[test]
    fn activation_egress_missing_when_no_progress_path() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  lonely:
    name: "Lonely"
    triggers: ["work.start"]
    publishes: ["isolated.signal"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings =
            check_activation_egress(&config, &HandoffGraph::from_config(&config), true, false);
        assert!(
            findings.iter().any(|f| f.id
                == crate::preset_lint::finding_id::FINDING_ACTIVATION_EGRESS_MISSING
                && f.hat.as_deref() == Some("lonely")),
            "expected activation_egress_missing for lonely, got: {:?}",
            findings
        );
    }

    // T-U1-04: plan-gate → executor handoff where executor has no
    // path to a downstream endpoint.
    #[test]
    fn handoff_pairing_broken_when_consumer_has_no_egress() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["executor.dead_end"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings =
            check_handoff_pairing(&config, &HandoffGraph::from_config(&config), true, false);
        assert!(
            findings.iter().any(|f| f.id
                == crate::preset_lint::finding_id::FINDING_HANDOFF_PAIRING_BROKEN
                && f.topic.as_deref() == Some("work.ready")
                && f.hat.as_deref() == Some("executor")),
            "expected handoff_pairing_broken for executor+work.ready, got: {:?}",
            findings
        );
    }

    // T-U1-05: work.retry trigger with no publisher at all
    #[test]
    fn trigger_publish_asymmetry_when_trigger_has_no_publisher() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  executor:
    name: "Executor"
    triggers: ["work.retry"]
    publishes: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_trigger_publish_asymmetry(
            &config,
            &HandoffGraph::from_config(&config),
            true,
            false,
        );
        assert!(
            findings.iter().any(|f| f.id
                == crate::preset_lint::finding_id::FINDING_TRIGGER_PUBLISH_ASYMMETRY
                && f.topic.as_deref() == Some("work.retry")),
            "expected trigger_publish_asymmetry for work.retry, got: {:?}",
            findings
        );
    }

    // T-U1-06: wildcard subscriber is not a unique consumer
    #[test]
    fn wildcard_subscriber_excluded_from_unique_consumer() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  observer:
    name: "Observer"
    triggers: ["*"]
    publishes: ["observe.tick"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let graph = HandoffGraph::from_config(&config);
        // work.ready has 1 explicit (executor) + 1 wildcard (observer).
        // Should NOT be a unique consumer.
        assert!(!graph.unique_consumer_topics().contains("work.ready"));
        let findings = check_handoff_pairing(&config, &graph, true, false);
        assert!(
            !findings
                .iter()
                .any(|f| f.topic.as_deref() == Some("work.ready")),
            "work.ready should be excluded from handoff_pairing when wildcard subscriber exists: {:?}",
            findings
        );
    }

    // T-U1-07: deterministic output ordering
    #[test]
    fn run_workflow_activation_contract_is_deterministic() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["a.out"]
  b:
    name: "B"
    triggers: ["a.out"]
    publishes: ["b.out"]
  c:
    name: "C"
    triggers: ["a.out"]
    publishes: ["c.out"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let f1 = run_workflow_activation_contract(&config, true, false);
        let f2 = run_workflow_activation_contract(&config, true, false);
        assert_eq!(f1, f2, "two calls must produce identical ordered output");
    }

    // T-U1-08: severity propagation — Default → Warn, Strict → Error.
    #[test]
    fn default_mode_emits_warn_strict_emits_error() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["executor.dead_end"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let default_findings = run_workflow_activation_contract(&config, false, false);
        let strict_findings = run_workflow_activation_contract(&config, true, false);
        assert!(
            default_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Warn)
        );
        assert!(
            strict_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Error)
        );
    }

    // WRC-U3 / T-WRC-U3 (severity upgrade): the same fixture as
    // T-U1-08 must produce Error severity under
    // `source_is_builtin_embedded = true` even in the non-strict
    // (`strict = false`) path. This pins KTD-7.
    #[test]
    fn builtin_embedded_severity_is_always_error() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["executor.dead_end"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        // strict=false but source_is_builtin_embedded=true → every
        // WAC finding must still be Error.
        let findings = run_workflow_activation_contract(&config, false, true);
        assert!(
            !findings.is_empty(),
            "fixture must trigger at least one WAC finding, got none"
        );
        assert!(
            findings.iter().all(|f| f.severity == LintSeverity::Error),
            "builtin-embedded source must always produce Error, got: {:?}",
            findings
        );
    }

    // WRC-U2 / T-WRC-U2-03: a normal handoff (plan-gate → executor →
    // review chain) that closes via a downstream hat trigger must NOT
    // produce R2 (re_emit_trap). This is the canonical positive case
    // for the 003 plan's narrow R2 semantics: the rule fires only
    // when the consumer has *no* closure path, not whenever it
    // triggers a topic it does not itself re-emit.
    #[test]
    fn re_emit_trap_does_not_fire_on_healthy_handoff_chain() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.passed"]
  reporter:
    name: "Reporter"
    triggers: ["review.passed"]
    publishes: ["LOOP_COMPLETE"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = run_workflow_activation_contract(&config, true, false);
        let re_emit: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == crate::preset_lint::finding_id::FINDING_RE_EMIT_TRAP
                    && f.hat.as_deref() == Some("executor")
                    && f.topic.as_deref() == Some("work.ready")
            })
            .collect();
        assert!(
            re_emit.is_empty(),
            "executor→work.ready→work.done→review→reporter→LOOP_COMPLETE \
             is a healthy chain; R2 must not fire on the executor. \
             Got: {:?}",
            findings
        );
    }

    // WRC-U2 / T-WRC-U2-04: HandoffIndex default seeds (`queue.advance`,
    // `work.ready`, `fix.plan.ready`, `work.failed`) must not appear
    // as `HandoffEntry { consumer: None, handoff: false }` ghost
    // topics when the preset has a unique consumer. Concretely: the
    // `queue.advance` seed in a plan-gate self-loop preset should
    // resolve to the plan-gate (the unique non-wildcard consumer of
    // its own self-loop), and the priority pass should be enabled.
    #[test]
    fn handoff_index_default_seeds_carry_consumer_for_self_loop() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start", "queue.advance"]
    publishes: ["queue.advance", "work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done", "LOOP_COMPLETE"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let index = crate::workflow_contract::HandoffIndex::from_config(&config);
        // work.ready is unique-consumer (executor).
        let entry = index
            .entries
            .get("work.ready")
            .expect("work.ready must be in the index");
        assert_eq!(entry.consumer.as_deref(), Some("executor"));
        assert!(entry.is_priority_dispatchable());
        // queue.advance has plan_gate as its only non-wildcard
        // subscriber; priority pass should target it.
        let qa_entry = index
            .entries
            .get("queue.advance")
            .expect("queue.advance must be in the index");
        assert_eq!(qa_entry.consumer.as_deref(), Some("plan_gate"));
    }

    // WRC-U3 / T-WRC-U3-04 (Tier-0 mirror): a healthy WAC chain must
    // produce **zero** findings under `source_is_builtin_embedded =
    // true` AND `strict = true`. The fixture models a 3-hat
    // coordinator → executor → reviewer chain that closes on the
    // completion promise. This is the standalone unit-level
    // counterpart to the Tier-0 CI gate (`ralph preset check -H
    // builtin:ce-executor-serial --strict`) and pins the KTD-7
    // contract: builtin sources with a clean WAC graph pass
    // strict mode with no findings.
    #[test]
    fn tier0_strict_clean_wac_chain_passes_under_builtin_flag() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = run_workflow_activation_contract(&config, true, true);
        assert!(
            findings.is_empty(),
            "clean WAC chain under builtin+strict must produce zero findings, got: {:?}",
            findings
        );
    }

    // WRC-U3: source_label_is_builtin_embedded helper. The
    // aggregator's Step 2b and the CLI gate use this heuristic to
    // decide whether to escalate WAC findings to Error. The
    // heuristic must match the CLI parser's `HatsSource::Builtin`
    // variant exactly.
    #[test]
    fn source_label_builtin_embedded_helper() {
        assert!(source_label_is_builtin_embedded(
            "builtin:ce-executor-serial"
        ));
        assert!(source_label_is_builtin_embedded("builtin:foo"));
        assert!(!source_label_is_builtin_embedded(""));
        assert!(!source_label_is_builtin_embedded("/abs/path/to/preset.yml"));
        assert!(!source_label_is_builtin_embedded("current-config"));
    }

    // 2026-07-02-004 plan U8: synthesized `precheck-<X>` gate
    // hats are part of the desugared graph and must satisfy the
    // four WAC rules.  Specifically:
    // - the gate hat's `triggers=[<X>.proposed]` MUST have at
    //   least one publisher (the upstream hat that was rewritten
    //   to emit `<X>.proposed`), or R5 (trigger/publish
    //   asymmetry) fires;
    // - the gate hat's `publishes=[<X>, <X>.rejected]` MUST
    //   hand off to a downstream consumer of `<X>`, or R4
    //   (handoff pairing) fires;
    // - the rewritten producer's `<X>.proposed` MUST not be
    //   re-emitted by the same hat, or R2 (re-emit trap) fires.
    //
    // The fixture below wires `executor → precheck gate →
    // reviewer` with `executor` rewritten to emit
    // `review.complete.proposed`.  All four WAC rules must
    // pass on this graph.
    #[test]
    fn synthesized_gate_hat_satisfies_wac() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      review.complete:
        prompt: ["check findings are concrete"]
        on_fail:
          target: executor
          retry_budget: 3
          on_exhausted: "plan.blocked(reason=precheck_failed)"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["review.complete"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.complete"]
    publishes: ["LOOP_COMPLETE"]
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        config.normalize();
        use crate::event_loop::precheck_gate_enforcement as gate;
        assert!(
            config.hats.keys().any(|k| {
                gate::is_gate_hat(k) && gate::gate_topic(k) == Some("review.complete")
            }),
            "fixture must include a synthesized precheck-review.complete gate hat, got {:?}",
            config.hats.keys().collect::<Vec<_>>()
        );

        // Sanity: the producer was rewritten to `<X>.proposed`.
        let executor_publishes = &config.hats["executor"].publishes;
        assert!(
            executor_publishes
                .iter()
                .any(|p| p == "review.complete.proposed"),
            "executor must publish review.complete.proposed, got {executor_publishes:?}"
        );
        assert!(
            !executor_publishes.iter().any(|p| p == "review.complete"),
            "executor must NOT still publish the bare review.complete after desugar, got {executor_publishes:?}"
        );

        let graph = HandoffGraph::from_config(&config);

        // R5 (trigger/publish asymmetry): every trigger must
        // have at least one publisher.  `review.complete.proposed`
        // is published by `executor`; `review.complete` is
        // published by `precheck-review.complete`.  Both
        // resolve.
        let asym = check_trigger_publish_asymmetry(&config, &graph, true, false);
        assert!(
            asym.is_empty(),
            "trigger/publish asymmetry must be empty for a well-formed precheck graph, got: {asym:?}"
        );

        // R4 (handoff pairing): `review.complete.proposed` is
        // published by `executor` and consumed (only) by the
        // gate hat — exactly one consumer, OK.  `review.complete`
        // is published by the gate hat and consumed by
        // `reviewer` — one consumer, OK.
        let pairing = check_handoff_pairing(&config, &graph, true, false);
        assert!(
            pairing.is_empty(),
            "handoff pairing must be empty for a well-formed precheck graph, got: {pairing:?}"
        );

        // R3 (activation egress): every hat must publish at
        // least one terminal / progress-emitting topic.
        // executor publishes `review.complete.proposed`; the
        // gate publishes `<X>` and `<X>.rejected`; reviewer
        // publishes `LOOP_COMPLETE`.  All three clear.
        let egress = check_activation_egress(&config, &graph, true, false);
        assert!(
            egress.is_empty(),
            "activation egress must be empty, got: {egress:?}"
        );

        // R2 (re-emit trap): `review.complete.proposed` is
        // published by `executor` and triggers
        // `precheck-review.complete` (which does NOT publish
        // `review.complete.proposed`), so R2 must be clear.
        let re_emit = check_re_emit_trap(&config, &graph, true, false);
        assert!(
            re_emit.is_empty(),
            "re-emit trap must be empty, got: {re_emit:?}"
        );

        // Tidy: ensure the gate hat DOES NOT publish
        // `<X>.proposed` (which would loop the producer back
        // to the gate).  This is a deliberate invariant of
        // the desugar — the gate emits `<X>` or `<X>.rejected`,
        // never `<X>.proposed`.
        let gate = &config.hats["precheck-review.complete"];
        assert!(
            !gate
                .publishes
                .iter()
                .any(|p| p == "review.complete.proposed"),
            "gate hat must never re-publish <X>.proposed, got {gate_publishes:?}",
            gate_publishes = gate.publishes
        );
        let _ = config;
    }
}
