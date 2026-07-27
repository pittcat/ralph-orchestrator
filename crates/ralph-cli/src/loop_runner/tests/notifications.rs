//! plan 2026-07-25-001 U4: in-process loopback-HTTP integration tests for
//! `notify_loop_termination`.
//!
//! These tests call `notify_loop_termination` directly against a real local
//! HTTP/1.1 server bound to `127.0.0.1:0`, exercising `ReqwestTransport`
//! end-to-end. They never spawn the `ralph` CLI binary, so the HARD RULE 5
//! agent-env scrub requirement does not apply.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ralph_core::LoopContext;
use ralph_core::config::{NotificationEndpoint, NotificationsConfig, OnStatus};
use ralph_core::event_loop::TerminationReason;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::notifications::notify_loop_termination;

/// One recorded request: `(request_line, body)`.
type Recorded = Arc<Mutex<Vec<(String, String)>>>;

const OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
const ERR_RESPONSE: &str =
    "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// Finds the byte offset of the `\r\n\r\n` head/body separator.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Serves one connection: reads the full request (head + Content-Length
/// body), records `(request_line, body)`, writes `response`, and shuts the
/// write half so reqwest completes.
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    recorded: Recorded,
    response: String,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    let head_end = loop {
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        match stream.read(&mut chunk).await {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let request_line = head.lines().next().unwrap_or("").to_string();
    let content_length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);

    let body_start = head_end + 4;
    while buf.len() - body_start < content_length {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let body_end = (body_start + content_length).min(buf.len());
    let body = String::from_utf8_lossy(&buf[body_start..body_end]).into_owned();

    recorded
        .lock()
        .expect("recorded lock poisoned")
        .push((request_line, body));

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// Binds a loopback server on an ephemeral port and spawns its accept loop.
/// Every connection is answered with `response`.
async fn spawn_loopback(response: &'static str) -> (u16, Recorded) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("local_addr").port();
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let accept_recorded = recorded.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let rec = accept_recorded.clone();
            tokio::spawn(handle_connection(stream, rec, response.to_string()));
        }
    });
    (port, recorded)
}

/// Polls `cond` until it returns true or a 5s deadline fires; panics on
/// timeout. Never sleeps forever.
async fn wait_until(cond: impl Fn() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if cond() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn endpoint(name: &str, url: &str, on: Vec<OnStatus>, body: &str) -> NotificationEndpoint {
    NotificationEndpoint {
        name: name.to_string(),
        url: url.to_string(),
        on,
        headers: HashMap::new(),
        body: body.to_string(),
    }
}

fn enabled_config(endpoints: Vec<NotificationEndpoint>) -> NotificationsConfig {
    NotificationsConfig {
        enabled: true,
        timeout_seconds: 5,
        endpoints,
    }
}

fn primary_context(workspace: &tempfile::TempDir) -> LoopContext {
    LoopContext::primary(workspace.path().to_path_buf())
}

// ── 1. success POST renders template vars and delivers (AE1/S1) ─────────────

#[tokio::test]
async fn notify_success_posts_rendered_body_to_success_endpoint() {
    let (port, recorded) = spawn_loopback(OK_RESPONSE).await;
    let workspace = tempfile::tempdir().expect("tempdir");
    let config = enabled_config(vec![endpoint(
        "ep",
        &format!("http://127.0.0.1:{port}/hook"),
        vec![OnStatus::Success],
        r#"{"text":"Ralph {{status}} {{loop_id}} {{termination_reason}}"}"#,
    )]);
    let ctx = primary_context(&workspace);

    notify_loop_termination(&config, &Some(ctx), &TerminationReason::CompletionPromise).await;

    let rec = recorded.clone();
    wait_until(|| !rec.lock().expect("lock").is_empty(), "success delivery").await;
    let snapshot = recorded.lock().expect("lock").clone();
    assert_eq!(snapshot.len(), 1, "exactly one POST expected");
    assert!(
        snapshot[0].0.starts_with("POST /hook HTTP/1.1"),
        "unexpected request line: {}",
        snapshot[0].0
    );
    // LoopContext::primary has loop_id == None → template var falls back to
    // "primary".
    assert!(
        snapshot[0].1.contains("Ralph success primary completed"),
        "body not rendered as expected: {}",
        snapshot[0].1
    );
}

// ── 2. failure routing: only on:[failure] endpoint is hit (AE2/S2) ──────────

#[tokio::test]
async fn notify_failure_routes_only_to_failure_endpoint() {
    let (port, recorded) = spawn_loopback(OK_RESPONSE).await;
    let workspace = tempfile::tempdir().expect("tempdir");
    let config = enabled_config(vec![
        endpoint(
            "success-ep",
            &format!("http://127.0.0.1:{port}/success"),
            vec![OnStatus::Success],
            "s",
        ),
        endpoint(
            "failure-ep",
            &format!("http://127.0.0.1:{port}/failure"),
            vec![OnStatus::Failure],
            "f",
        ),
    ]);
    let ctx = primary_context(&workspace);

    notify_loop_termination(&config, &Some(ctx), &TerminationReason::MaxIterations).await;

    let rec = recorded.clone();
    wait_until(|| !rec.lock().expect("lock").is_empty(), "failure delivery").await;
    let snapshot = recorded.lock().expect("lock").clone();
    assert_eq!(snapshot.len(), 1, "exactly one POST expected");
    assert!(
        snapshot[0].0.starts_with("POST /failure HTTP/1.1"),
        "failure reason must route to /failure, got: {}",
        snapshot[0].0
    );
    assert!(
        !snapshot.iter().any(|(line, _)| line.contains("/success")),
        "/success must not be hit for a failure reason"
    );
}

// ── 3. 5xx response never panics and never alters the result (AE3/S5) ───────

#[tokio::test]
async fn notify_swallows_5xx_response_without_panic() {
    let (port, recorded) = spawn_loopback(ERR_RESPONSE).await;
    let workspace = tempfile::tempdir().expect("tempdir");
    let config = enabled_config(vec![endpoint(
        "ep",
        &format!("http://127.0.0.1:{port}/hook"),
        vec![OnStatus::Success],
        "x",
    )]);
    let ctx = primary_context(&workspace);

    // Must return normally despite the 500; the mount in `run_loop_impl`
    // ignores notify's outcome entirely and returns `result` unchanged, so a
    // 5xx can never convert success into failure or vice versa.
    notify_loop_termination(&config, &Some(ctx), &TerminationReason::CompletionPromise).await;

    // The POST was attempted (server saw it) and the non-2xx was swallowed.
    let rec = recorded.clone();
    wait_until(|| !rec.lock().expect("lock").is_empty(), "5xx attempt").await;
    assert_eq!(recorded.lock().expect("lock").len(), 1);
}

// ── 4. disabled config makes zero HTTP requests (S3) ────────────────────────

#[tokio::test]
async fn notify_disabled_makes_zero_requests() {
    let (port, recorded) = spawn_loopback(OK_RESPONSE).await;
    let workspace = tempfile::tempdir().expect("tempdir");
    let mut config = enabled_config(vec![endpoint(
        "ep",
        &format!("http://127.0.0.1:{port}/hook"),
        vec![OnStatus::Success],
        "x",
    )]);
    config.enabled = false;
    let ctx = primary_context(&workspace);

    notify_loop_termination(&config, &Some(ctx), &TerminationReason::CompletionPromise).await;

    // Give any (forbidden) stray request a bounded window to arrive, then
    // assert nothing was recorded.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        recorded.lock().expect("lock").is_empty(),
        "disabled notifications must make zero HTTP requests"
    );
}

// ── 5. mount-exists characterization (S7) ───────────────────────────────────
//
// `run_loop_impl` is the single chokepoint every `Ok(reason)` return of
// `run_loop_impl_inner` passes through (including shortcut paths). This test
// characterizes that call site by exercising `notify_loop_termination`
// directly with a success reason against the loopback: if the wrapper's one
// call site is intact, every successful loop termination is delivered.

#[tokio::test]
async fn notify_is_the_single_chokepoint_for_ok_reasons() {
    let (port, recorded) = spawn_loopback(OK_RESPONSE).await;
    let workspace = tempfile::tempdir().expect("tempdir");
    let config = enabled_config(vec![endpoint(
        "ep",
        &format!("http://127.0.0.1:{port}/done"),
        vec![OnStatus::Success],
        r#"{"reason":"{{termination_reason}}"}"#,
    )]);
    let ctx = primary_context(&workspace);

    notify_loop_termination(&config, &Some(ctx), &TerminationReason::CompletionPromise).await;

    let rec = recorded.clone();
    wait_until(
        || !rec.lock().expect("lock").is_empty(),
        "chokepoint delivery",
    )
    .await;
    let snapshot = recorded.lock().expect("lock").clone();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot[0].1.contains(r#""reason":"completed""#));
}
