//! Webhook transports (plan U3): the abstraction over HTTP POST and a real
//! reqwest-backed implementation, plus an in-memory [`FakeTransport`] for
//! unit tests.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

/// The result of a successful HTTP POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportOutcome {
    /// The HTTP status code returned by the remote endpoint.
    pub status: u16,
}

impl TransportOutcome {
    /// Returns `true` for 2xx status codes.
    pub fn is_success(self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Errors produced by a [`WebhookTransport::post`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The request itself failed (connection error, TLS error, DNS, ...).
    Http(String),
    /// The request exceeded the configured timeout.
    Timeout,
    /// The remote endpoint responded with a non-2xx status code.
    Non2xx(u16),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Http(msg) => write!(f, "http error: {msg}"),
            TransportError::Timeout => write!(f, "request timed out"),
            TransportError::Non2xx(status) => write!(f, "non-2xx status code: {status}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// An async POST transport used by [`crate::notifications::dispatch`].
///
/// Implementations must never panic; network-level failures are reported as
/// [`TransportError`].
#[async_trait]
pub trait WebhookTransport: Send + Sync {
    /// POSTs `body` to `url` with the given `headers`, bounded by `timeout`.
    ///
    /// Returns [`TransportOutcome`] for any 2xx response. Non-2xx responses
    /// are reported as [`TransportError::Non2xx`].
    async fn post(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        body: &str,
        timeout: Duration,
    ) -> Result<TransportOutcome, TransportError>;
}

/// Real HTTP transport backed by reqwest (rustls, proxy disabled).
///
/// The client is built with `.no_proxy()` so `HTTP_PROXY` / `HTTPS_PROXY`
/// environment variables can never reroute webhook traffic (in particular
/// loopback targets used in tests) through a remote proxy. See
/// `docs/solutions/test-failures/reqwest-no-proxy-loopback-test-failures.md`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReqwestTransport;

#[async_trait]
impl WebhookTransport for ReqwestTransport {
    async fn post(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        body: &str,
        timeout: Duration,
    ) -> Result<TransportOutcome, TransportError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| TransportError::Http(e.to_string()))?;

        let mut request = client.post(url).timeout(timeout).body(body.to_string());
        for (key, value) in headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout
            } else {
                TransportError::Http(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            Ok(TransportOutcome { status })
        } else {
            Err(TransportError::Non2xx(status))
        }
    }
}

/// A recorded POST call made against a [`FakeTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    /// The URL the call was posted to.
    pub url: String,
    /// The rendered body of the call.
    pub body: String,
}

/// In-memory transport test double.
///
/// Records every `(url, body)` pair it receives and can be configured to fail
/// POSTs to URLs matching a configured substring, so tests can verify
/// best-effort continuation semantics without any network I/O.
#[derive(Debug, Default)]
pub struct FakeTransport {
    calls: Mutex<Vec<RecordedCall>>,
    fail_url_substrings: Mutex<Vec<String>>,
}

impl FakeTransport {
    /// Creates an empty transport that succeeds for every URL.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the transport to fail (with [`TransportError::Http`]) every
    /// POST whose URL contains `substring`.
    pub fn fail_urls_containing(&self, substring: &str) {
        self.fail_url_substrings
            .lock()
            .expect("fail_url_substrings lock poisoned")
            .push(substring.to_string());
    }

    /// Returns a snapshot of all recorded calls, in call order.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls lock poisoned").clone()
    }

    /// Returns the number of recorded calls.
    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("calls lock poisoned").len()
    }
}

#[async_trait]
impl WebhookTransport for FakeTransport {
    async fn post(
        &self,
        url: &str,
        _headers: &HashMap<String, String>,
        body: &str,
        _timeout: Duration,
    ) -> Result<TransportOutcome, TransportError> {
        let should_fail = self
            .fail_url_substrings
            .lock()
            .expect("fail_url_substrings lock poisoned")
            .iter()
            .any(|s| url.contains(s));
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(RecordedCall {
                url: url.to_string(),
                body: body.to_string(),
            });
        if should_fail {
            Err(TransportError::Http(format!(
                "fake transport failure for url {url}"
            )))
        } else {
            Ok(TransportOutcome { status: 200 })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> HashMap<String, String> {
        HashMap::new()
    }

    #[tokio::test]
    async fn fake_transport_records_calls() {
        let fake = FakeTransport::new();
        let outcome = fake
            .post(
                "https://example.com/hook",
                &headers(),
                "payload",
                Duration::from_secs(1),
            )
            .await
            .expect("post succeeds");
        assert!(outcome.is_success());
        assert_eq!(outcome.status, 200);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/hook");
        assert_eq!(calls[0].body, "payload");
        assert_eq!(fake.call_count(), 1);
    }

    #[tokio::test]
    async fn fake_transport_records_call_even_when_failing() {
        let fake = FakeTransport::new();
        fake.fail_urls_containing("bad");
        let err = fake
            .post(
                "https://example.com/bad",
                &headers(),
                "x",
                Duration::from_secs(1),
            )
            .await
            .expect_err("configured failure");
        assert!(matches!(err, TransportError::Http(_)));
        // The attempt is still recorded so tests can assert best-effort
        // continuation semantics.
        assert_eq!(fake.call_count(), 1);
    }

    #[test]
    fn transport_outcome_is_success_range() {
        assert!(TransportOutcome { status: 200 }.is_success());
        assert!(TransportOutcome { status: 299 }.is_success());
        assert!(!TransportOutcome { status: 199 }.is_success());
        assert!(!TransportOutcome { status: 300 }.is_success());
        assert!(!TransportOutcome { status: 500 }.is_success());
    }

    #[test]
    fn transport_error_display() {
        assert_eq!(
            TransportError::Http("boom".to_string()).to_string(),
            "http error: boom"
        );
        assert_eq!(TransportError::Timeout.to_string(), "request timed out");
        assert_eq!(
            TransportError::Non2xx(503).to_string(),
            "non-2xx status code: 503"
        );
    }
}
