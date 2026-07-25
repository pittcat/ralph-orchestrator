//! Webhook dispatch orchestration for loop-completion notifications
//! (plan KTD-5 template variables, KTD-7 URL redaction, best-effort
//! semantics).
//!
//! [`dispatch`] renders each subscribed endpoint's `body` template and POSTs
//! it through a [`WebhookTransport`]. It is strictly best-effort: it never
//! returns an error and never panics — render failures and transport failures
//! are logged (with the webhook URL redacted) and the next endpoint is
//! attempted.

use std::collections::HashMap;
use std::time::Duration;

use crate::config::{NotificationsConfig, OnStatus};
use crate::event_loop::TerminationReason;

use super::template::render;
use super::transport::{WebhookTransport, redact_transport_error, redact_url};

/// The eight template variable names exposed to webhook `body` templates
/// (plan KTD-5).
pub const TEMPLATE_VAR_NAMES: [&str; 8] = [
    "loop_id",
    "status",
    "termination_reason",
    "workspace",
    "repo_root",
    "iteration_current",
    "iteration_max",
    "active_hat",
];

/// Loop-termination facts made available to webhook body templates.
///
/// All fields are plain strings (iteration counts included) so they can be
/// rendered verbatim into `{{var}}` placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationContext {
    /// The loop identifier (e.g. `loop-2026-07-25-001`).
    pub loop_id: String,
    /// Overall outcome: `"success"` or `"failure"`.
    pub status: String,
    /// Stable termination reason string (`TerminationReason::as_str`).
    pub termination_reason: String,
    /// The loop workspace (worktree) path.
    pub workspace: String,
    /// The repository root path.
    pub repo_root: String,
    /// Iterations consumed (stringified).
    pub iteration_current: String,
    /// Configured iteration limit (stringified).
    pub iteration_max: String,
    /// The hat that was active at termination.
    pub active_hat: String,
}

impl TerminationContext {
    /// Builds a context from its eight string fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        loop_id: impl Into<String>,
        status: impl Into<String>,
        termination_reason: impl Into<String>,
        workspace: impl Into<String>,
        repo_root: impl Into<String>,
        iteration_current: impl Into<String>,
        iteration_max: impl Into<String>,
        active_hat: impl Into<String>,
    ) -> Self {
        Self {
            loop_id: loop_id.into(),
            status: status.into(),
            termination_reason: termination_reason.into(),
            workspace: workspace.into(),
            repo_root: repo_root.into(),
            iteration_current: iteration_current.into(),
            iteration_max: iteration_max.into(),
            active_hat: active_hat.into(),
        }
    }

    /// Maps the context onto the eight KTD-5 template variable names.
    pub fn to_template_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::with_capacity(TEMPLATE_VAR_NAMES.len());
        vars.insert("loop_id".to_string(), self.loop_id.clone());
        vars.insert("status".to_string(), self.status.clone());
        vars.insert(
            "termination_reason".to_string(),
            self.termination_reason.clone(),
        );
        vars.insert("workspace".to_string(), self.workspace.clone());
        vars.insert("repo_root".to_string(), self.repo_root.clone());
        vars.insert(
            "iteration_current".to_string(),
            self.iteration_current.clone(),
        );
        vars.insert("iteration_max".to_string(), self.iteration_max.clone());
        vars.insert("active_hat".to_string(), self.active_hat.clone());
        vars
    }
}

/// Maps a [`TerminationReason`] onto the [`OnStatus`] filter value used by
/// endpoint routing: `Success` only for `CompletionPromise`, `Failure` for
/// everything else.
pub fn status_for_reason(reason: &TerminationReason) -> OnStatus {
    if reason.is_success() {
        OnStatus::Success
    } else {
        OnStatus::Failure
    }
}

/// Dispatches loop-completion webhook notifications, best-effort.
///
/// Semantics:
/// - `config.enabled == false` → zero transport calls, returns immediately.
/// - Endpoints whose `on` filter does not include the status derived from
///   `reason` are skipped.
/// - A render error for one endpoint logs a warning (endpoint `name` only)
///   and continues with the next endpoint.
/// - A transport error logs a warning (endpoint `name` + error, URL
///   redacted) and continues with the next endpoint.
/// - This function never returns an error and never panics.
pub async fn dispatch<T: WebhookTransport>(
    config: &NotificationsConfig,
    ctx: &TerminationContext,
    reason: &TerminationReason,
    transport: &T,
) {
    if !config.enabled {
        return;
    }

    let status = status_for_reason(reason);
    let vars = ctx.to_template_vars();
    let timeout = Duration::from_secs(config.timeout_seconds);

    for endpoint in &config.endpoints {
        if !endpoint.on.contains(&status) {
            continue;
        }

        let rendered = match render(&endpoint.body, &vars) {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(
                    endpoint = %endpoint.name,
                    error = %err,
                    "notification body render failed; skipping endpoint"
                );
                continue;
            }
        };

        match transport
            .post(&endpoint.url, &endpoint.headers, &rendered, timeout)
            .await
        {
            Ok(outcome) => {
                tracing::debug!(
                    endpoint = %endpoint.name,
                    status = outcome.status,
                    "notification delivered"
                );
            }
            Err(err) => {
                tracing::warn!(
                    endpoint = %endpoint.name,
                    url = %redact_url(&endpoint.url),
                    error = %redact_transport_error(&err),
                    "notification delivery failed; continuing"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::transport::FakeTransport;
    use super::*;
    use crate::config::NotificationEndpoint;
    use crate::event_loop::TerminationReason;

    fn endpoint(name: &str, url: &str, on: Vec<OnStatus>, body: &str) -> NotificationEndpoint {
        NotificationEndpoint {
            name: name.to_string(),
            url: url.to_string(),
            on,
            headers: HashMap::new(),
            body: body.to_string(),
        }
    }

    fn config_with(endpoints: Vec<NotificationEndpoint>) -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints,
        }
    }

    fn ctx(status: &str) -> TerminationContext {
        TerminationContext::new(
            "loop-2026-07-25-001",
            status,
            "completed",
            "/tmp/ws",
            "/tmp/repo",
            "3",
            "10",
            "executor",
        )
    }

    // ── 1. success routing ────────────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_success_only_calls_success_endpoint() {
        let config = config_with(vec![
            endpoint(
                "success-ep",
                "https://example.com/ok",
                vec![OnStatus::Success],
                "ok",
            ),
            endpoint(
                "failure-ep",
                "https://example.com/fail",
                vec![OnStatus::Failure],
                "fail",
            ),
        ]);
        let fake = FakeTransport::new();
        dispatch(
            &config,
            &ctx("success"),
            &TerminationReason::CompletionPromise,
            &fake,
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/ok");
        assert!(!calls.iter().any(|c| c.url.contains("/fail")));
    }

    // ── 2. failure routing ────────────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_failure_only_calls_failure_endpoint() {
        let config = config_with(vec![
            endpoint(
                "success-ep",
                "https://example.com/ok",
                vec![OnStatus::Success],
                "ok",
            ),
            endpoint(
                "failure-ep",
                "https://example.com/fail",
                vec![OnStatus::Failure],
                "fail",
            ),
        ]);
        let fake = FakeTransport::new();
        dispatch(
            &config,
            &ctx("failure"),
            &TerminationReason::MaxIterations,
            &fake,
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/fail");
        assert!(!calls.iter().any(|c| c.url.contains("/ok")));
    }

    // ── 3. transport error is best-effort and continues ───────────────────────

    #[tokio::test]
    async fn dispatch_transport_error_continues_to_next_endpoint() {
        let config = config_with(vec![
            endpoint(
                "first",
                "https://example.com/bad",
                vec![OnStatus::Success],
                "a",
            ),
            endpoint(
                "second",
                "https://example.com/good",
                vec![OnStatus::Success],
                "b",
            ),
        ]);
        let fake = FakeTransport::new();
        fake.fail_urls_containing("/bad");
        // Must not panic and must return normally.
        dispatch(
            &config,
            &ctx("success"),
            &TerminationReason::CompletionPromise,
            &fake,
        )
        .await;

        // Both endpoints were attempted (failure still records the attempt).
        assert_eq!(fake.call_count(), 2);
    }

    // ── 4. render error skips endpoint and continues ──────────────────────────

    #[tokio::test]
    async fn dispatch_render_error_skips_endpoint_and_continues() {
        let config = config_with(vec![
            endpoint(
                "broken",
                "https://example.com/broken",
                vec![OnStatus::Success],
                "hi {{unknown_var}}",
            ),
            endpoint(
                "valid",
                "https://example.com/valid",
                vec![OnStatus::Success],
                "hi {{loop_id}}",
            ),
        ]);
        let fake = FakeTransport::new();
        dispatch(
            &config,
            &ctx("success"),
            &TerminationReason::CompletionPromise,
            &fake,
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/valid");
        assert!(calls[0].body.contains("loop-2026-07-25-001"));
    }

    // ── 5. disabled = zero calls ──────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_disabled_makes_zero_calls() {
        let mut config = config_with(vec![endpoint(
            "ep",
            "https://example.com/hook",
            vec![OnStatus::Success, OnStatus::Failure],
            "x",
        )]);
        config.enabled = false;
        let fake = FakeTransport::new();
        dispatch(
            &config,
            &ctx("success"),
            &TerminationReason::CompletionPromise,
            &fake,
        )
        .await;
        dispatch(
            &config,
            &ctx("failure"),
            &TerminationReason::MaxIterations,
            &fake,
        )
        .await;
        assert_eq!(fake.call_count(), 0);
    }

    // ── 6. on:[success, failure] matches both ────────────────────────────────

    #[tokio::test]
    async fn dispatch_endpoint_subscribed_to_both_statuses_called_for_both() {
        let config = config_with(vec![endpoint(
            "both",
            "https://example.com/both",
            vec![OnStatus::Success, OnStatus::Failure],
            "x",
        )]);
        let fake = FakeTransport::new();
        dispatch(
            &config,
            &ctx("success"),
            &TerminationReason::CompletionPromise,
            &fake,
        )
        .await;
        dispatch(
            &config,
            &ctx("failure"),
            &TerminationReason::MaxIterations,
            &fake,
        )
        .await;
        assert_eq!(fake.call_count(), 2);
        assert!(
            fake.calls()
                .iter()
                .all(|c| c.url == "https://example.com/both")
        );
    }

    // ── 7. status_for_reason unit ─────────────────────────────────────────────

    #[test]
    fn status_for_reason_maps_success_and_failure() {
        assert!(status_for_reason(&TerminationReason::CompletionPromise).is_success());
        assert!(status_for_reason(&TerminationReason::MaxIterations).is_failure());
        // Non-completion reasons are all failure, including clean cancel.
        assert!(status_for_reason(&TerminationReason::Cancelled).is_failure());
    }

    // ── 8. to_template_vars + redact_url units ────────────────────────────────

    #[test]
    fn to_template_vars_contains_all_eight_keys() {
        let vars = ctx("success").to_template_vars();
        for name in TEMPLATE_VAR_NAMES {
            assert!(vars.contains_key(name), "missing template var `{name}`");
        }
        assert_eq!(vars.len(), TEMPLATE_VAR_NAMES.len());
        assert_eq!(vars["status"], "success");
        assert_eq!(vars["iteration_current"], "3");
        assert_eq!(vars["active_hat"], "executor");
    }

    #[test]
    fn redact_url_strips_query_string() {
        let redacted = redact_url("https://h/x?a=secret&token=abc");
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("token=abc"));
        // U1: scheme+authority preserved, path+query redacted.
        assert_eq!(redacted, "https://h/<redacted>");
        // No query string → path also redacted.
        assert_eq!(redact_url("https://h/x"), "https://h/<redacted>");
    }
}
