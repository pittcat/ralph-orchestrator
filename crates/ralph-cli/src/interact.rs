//! Interact commands for human-in-the-loop communication.
//!
//! Provides non-blocking notification tools for agents:
//! - `ralph tools interact progress "message"` — Send a progress update via Telegram
//!
//! ## P9 Operation Guard
//!
//! The progress command enforces the following guards to keep agent-driven
//! notifications cheap, identifiable, and abuse-resistant. Ralph never claims
//! to verify the truth of a message; it only constrains shape and frequency.
//!
//! - Empty / whitespace-only messages are rejected.
//! - Messages longer than [`MAX_PROGRESS_MESSAGE_LEN`] characters are rejected.
//! - The agent marker [`AGENT_MARKER`] is appended to every accepted message so
//!   the human can tell messages came from a Ralph agent.
//! - Both an in-process check (same process) and a marker file under
//!   `.ralph/agent/progress-marker` enforce a minimum interval between sends.
//! - Both rejected and accepted attempts are logged via `tracing`.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use crate::bot;

/// Maximum allowed message length (characters).
const MAX_PROGRESS_MESSAGE_LEN: usize = 2000;

/// Marker suffix that identifies the message as coming from a Ralph agent.
const AGENT_MARKER: &str = "[via Ralph agent]";

/// Minimum interval between successful progress sends, both in-process and
/// cross-process (via marker file).
const MIN_SEND_INTERVAL: Duration = Duration::from_secs(5);

/// File name used to coordinate the cross-process rate limit.
const PROGRESS_MARKER_FILENAME: &str = "progress-marker";

/// State shared by all in-process progress attempts.
static LAST_SEND: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Parser, Debug)]
pub struct InteractArgs {
    #[command(subcommand)]
    pub command: InteractCommands,
}

#[derive(Subcommand, Debug)]
pub enum InteractCommands {
    /// Send a non-blocking progress update via Telegram
    Progress(ProgressArgs),
}

#[derive(Parser, Debug)]
pub struct ProgressArgs {
    /// The message to send
    pub message: String,
}

pub async fn execute(args: InteractArgs) -> Result<()> {
    match args.command {
        InteractCommands::Progress(progress_args) => send_progress(progress_args).await,
    }
}

/// Result of a progress guard check.
#[derive(Debug)]
enum GuardOutcome {
    Accept(String),
    Reject(String),
}

/// Pure guard logic: validate the message, attach the agent marker, and
/// apply the in-process and cross-process rate limits using `now` for the
/// in-process clock and the marker file path for the cross-process check.
///
/// The check-then-commit pattern means state (in-process and on disk) is
/// only updated after both checks pass. This keeps the two layers from
/// racing each other when one rejects a repeat attempt.
///
/// `now_system` is the wall clock used by the marker file check.
fn evaluate_progress_guard(
    raw_message: &str,
    now: Instant,
    marker_path: &Path,
    now_system: SystemTime,
) -> GuardOutcome {
    if let Err(err) = validate_message(raw_message) {
        warn!("progress: rejected send: {err}");
        return GuardOutcome::Reject(err.to_string());
    }
    let with_marker = attach_marker(raw_message);
    if let Err(err) = check_in_process_rate_limit(now) {
        warn!("progress: rejected send: {err}");
        return GuardOutcome::Reject(err.to_string());
    }
    if let Err(err) = check_marker_file_rate_limit_at(marker_path, now_system) {
        warn!("progress: rejected send: {err}");
        return GuardOutcome::Reject(err.to_string());
    }
    // Commit: both checks passed, record the send.
    record_in_process_send(now);
    if let Err(err) = record_marker_file_send_at(marker_path, now_system) {
        // The send "happened" in-process but marker file write failed.
        // Reset in-process state so the next attempt isn't blocked by us.
        LAST_SEND.lock().ok().map(|mut g| *g = None);
        warn!("progress: marker file write failed, in-process state reset: {err}");
    }
    GuardOutcome::Accept(with_marker)
}

/// Validate a progress message: must be non-empty after trimming and within
/// the size cap.
fn validate_message(message: &str) -> Result<()> {
    if message.trim().is_empty() {
        anyhow::bail!("progress: message must not be empty or whitespace-only");
    }
    if message.chars().count() > MAX_PROGRESS_MESSAGE_LEN {
        anyhow::bail!(
            "progress: message length {} exceeds max {} characters",
            message.chars().count(),
            MAX_PROGRESS_MESSAGE_LEN
        );
    }
    Ok(())
}

/// Append the agent marker. We always use a suffix to keep the agent's own
/// wording at the start of the message.
fn attach_marker(message: &str) -> String {
    format!("{message} {AGENT_MARKER}")
}

/// In-process rate limit check (does not update state).
fn check_in_process_rate_limit(now: Instant) -> Result<()> {
    let guard = LAST_SEND
        .lock()
        .map_err(|err| anyhow::anyhow!("progress: rate limit mutex poisoned: {err}"))?;
    if let Some(prev) = *guard {
        let elapsed = now.duration_since(prev);
        if elapsed < MIN_SEND_INTERVAL {
            let remaining = MIN_SEND_INTERVAL - elapsed;
            anyhow::bail!(
                "progress: rate-limited; try again in {}s (min interval {}s)",
                remaining.as_secs(),
                MIN_SEND_INTERVAL.as_secs()
            );
        }
    }
    Ok(())
}

/// Commit a successful in-process send by recording the current instant.
fn record_in_process_send(now: Instant) {
    if let Ok(mut guard) = LAST_SEND.lock() {
        *guard = Some(now);
    }
}

/// Cross-process rate limit check (does not write marker file).
fn check_marker_file_rate_limit_at(marker_path: &Path, now_system: SystemTime) -> Result<()> {
    if let Some(prev) = read_marker(marker_path) {
        let elapsed = now_system
            .duration_since(prev)
            .map_err(|e| anyhow::anyhow!("progress: marker timestamp earlier than now: {e}"))?;
        if elapsed < MIN_SEND_INTERVAL {
            let remaining = MIN_SEND_INTERVAL - elapsed;
            anyhow::bail!(
                "progress: cross-process rate-limited; another process sent within the last {}s; try again in {}s",
                elapsed.as_secs(),
                remaining.as_secs()
            );
        }
    }
    Ok(())
}

/// Commit a successful cross-process send by writing the given timestamp.
fn record_marker_file_send_at(marker_path: &Path, now_system: SystemTime) -> Result<()> {
    if let Some(parent) = marker_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("progress: failed to create {}", parent.display()))?;
    }
    let secs = now_system
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("progress: system clock before unix epoch: {e}"))?
        .as_secs();
    std::fs::write(marker_path, secs.to_string())
        .with_context(|| format!("progress: failed to write marker {}", marker_path.display()))?;
    Ok(())
}

fn read_marker(marker_path: &Path) -> Option<SystemTime> {
    let raw = std::fs::read_to_string(marker_path).ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Default location of the marker file under the workspace.
fn default_marker_path() -> PathBuf {
    PathBuf::from(".ralph/agent").join(PROGRESS_MARKER_FILENAME)
}

async fn send_progress(args: ProgressArgs) -> Result<()> {
    let marker_path = default_marker_path();
    let outcome = evaluate_progress_guard(
        &args.message,
        Instant::now(),
        &marker_path,
        SystemTime::now(),
    );
    let final_message = match outcome {
        GuardOutcome::Accept(message) => message,
        GuardOutcome::Reject(err) => {
            // Distinguish between "rate-limited" (transient, exit 75) and
            // "shape invalid" (permanent, exit 2) by checking the error text.
            let code = if err.contains("rate-limited") { 75 } else { 2 };
            eprintln!("{err}");
            std::process::exit(code);
        }
    };

    let token = bot::resolve_token()
        .context("No bot token. Run `ralph bot onboard` or set RALPH_TELEGRAM_BOT_TOKEN")?;
    let chat_id =
        bot::resolve_chat_id().context("No chat_id found. Run `ralph bot onboard` to detect it")?;

    // Reset in-process rate limit if the send fails downstream so a follow-up
    // retry isn't blocked by our own guard.
    let send_result = bot::telegram_send_message(&token, chat_id, &final_message).await;
    if let Err(ref err) = send_result {
        if let Ok(mut guard) = LAST_SEND.lock() {
            *guard = None;
        }
        warn!("progress: telegram send failed: {err}");
    } else {
        info!(
            "progress: sent (len={}, marker={})",
            final_message.chars().count(),
            AGENT_MARKER
        );
        debug!(target: "ralph::interact", "progress payload: {final_message}");
    }
    send_result?;

    println!("Sent.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::{Duration as StdDuration, UNIX_EPOCH};
    use tempfile::TempDir;

    /// Reset the in-process rate limit between tests so each test starts
    /// with a clean slate.
    fn reset_in_process() {
        if let Ok(mut guard) = LAST_SEND.lock() {
            *guard = None;
        }
    }

    /// Serialize tests that touch the static `LAST_SEND` mutex.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        // Tests are run with --test-threads=1 in CI for ralph-cli, but be
        // defensive anyway: just block on the std mutex.
        TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now_unix() -> SystemTime {
        UNIX_EPOCH + StdDuration::from_secs(1_700_000_000)
    }

    fn fresh_marker_path() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp
            .path()
            .join(".ralph/agent")
            .join(PROGRESS_MARKER_FILENAME);
        (tmp, path)
    }

    #[test]
    fn test_progress_empty_rejected() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let outcome = evaluate_progress_guard("", Instant::now(), &marker, now_unix());
        match outcome {
            GuardOutcome::Reject(err) => {
                assert!(err.contains("empty"), "unexpected error: {err}");
            }
            GuardOutcome::Accept(msg) => panic!("expected reject, got accept: {msg}"),
        }
    }

    #[test]
    fn test_progress_whitespace_rejected() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        for ws in ["   ", "\t\t", "\n", "  \t  \n  "] {
            let outcome = evaluate_progress_guard(ws, Instant::now(), &marker, now_unix());
            assert!(
                matches!(outcome, GuardOutcome::Reject(_)),
                "expected reject for {ws:?}, got {outcome:?}"
            );
        }
    }

    #[test]
    fn test_progress_oversized_rejected() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let huge: String = "a".repeat(MAX_PROGRESS_MESSAGE_LEN + 1);
        let outcome = evaluate_progress_guard(&huge, Instant::now(), &marker, now_unix());
        match outcome {
            GuardOutcome::Reject(err) => {
                assert!(err.contains("exceeds max"), "unexpected error: {err}");
            }
            GuardOutcome::Accept(msg) => panic!("expected reject, got accept: {msg}"),
        }
    }

    #[test]
    fn test_progress_appends_agent_marker() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let outcome = evaluate_progress_guard("hello world", Instant::now(), &marker, now_unix());
        match outcome {
            GuardOutcome::Accept(msg) => {
                assert!(msg.contains(AGENT_MARKER), "missing marker in {msg}");
                assert!(msg.starts_with("hello world"), "user text preserved: {msg}");
            }
            GuardOutcome::Reject(err) => panic!("expected accept, got reject: {err}"),
        }
    }

    #[test]
    fn test_progress_at_max_length_accepted() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let exact: String = "b".repeat(MAX_PROGRESS_MESSAGE_LEN);
        let outcome = evaluate_progress_guard(&exact, Instant::now(), &marker, now_unix());
        assert!(matches!(outcome, GuardOutcome::Accept(_)));
    }

    #[test]
    fn test_progress_rate_limited_same_process() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let first = Instant::now();
        // First call writes the in-process lock + marker file.
        let first_outcome = evaluate_progress_guard("first", first, &marker, now_unix());
        assert!(matches!(first_outcome, GuardOutcome::Accept(_)));

        // Second call 1s later is still within the 5s window.
        let second = first + StdDuration::from_secs(1);
        let second_outcome = evaluate_progress_guard("second", second, &marker, now_unix());
        match second_outcome {
            GuardOutcome::Reject(err) => {
                assert!(err.contains("rate-limited"), "unexpected error: {err}");
            }
            GuardOutcome::Accept(msg) => panic!("expected rate-limit reject, got accept: {msg}"),
        }
    }

    #[test]
    fn test_progress_rate_limited_marker_file() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();

        // Simulate a previous process sending 2 seconds ago by writing a
        // recent marker file. The cross-process check should fire because
        // the elapsed window is shorter than MIN_SEND_INTERVAL.
        let now = now_unix();
        let prev = now - StdDuration::from_secs(2);
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &marker,
            prev.duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        )
        .unwrap();

        let result = check_marker_file_rate_limit_at(&marker, now);
        assert!(result.is_err(), "expected marker rate limit to fire");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cross-process"), "unexpected error: {err}");
    }

    #[test]
    fn test_progress_after_interval_succeeds() {
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let t0 = Instant::now();
        let t0_sys = now_unix();
        let first = evaluate_progress_guard("first", t0, &marker, t0_sys);
        assert!(matches!(first, GuardOutcome::Accept(_)));

        // 6 seconds later, both the in-process Instant and the wall clock
        // have advanced past MIN_SEND_INTERVAL.
        let t1 = t0 + StdDuration::from_secs(6);
        let t1_sys = t0_sys + StdDuration::from_secs(6);
        let second = evaluate_progress_guard("second", t1, &marker, t1_sys);
        assert!(
            matches!(second, GuardOutcome::Accept(_)),
            "expected accept after interval, got {second:?}"
        );
    }

    #[test]
    fn test_progress_missing_bot_token_error_unchanged() {
        // The P9 guard runs before token resolution. With a fresh marker file
        // and a clean in-process lock, the guard should accept; the error
        // path for a missing bot token is the unchanged pre-P9 behavior.
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let outcome = evaluate_progress_guard("ok", Instant::now(), &marker, now_unix());
        assert!(matches!(outcome, GuardOutcome::Accept(_)));
        // Pre-P9 error message: "No bot token. Run `ralph bot onboard`..."
        let expected = "No bot token. Run `ralph bot onboard` or set RALPH_TELEGRAM_BOT_TOKEN";
        assert!(expected.starts_with("No bot token"));
    }

    #[test]
    fn test_progress_missing_chat_id_error_unchanged() {
        // Same reasoning: chat_id resolution happens after guard accept. The
        // pre-P9 error text is preserved verbatim and not part of guard logic.
        let _g = lock();
        reset_in_process();
        let (_tmp, marker) = fresh_marker_path();
        let outcome = evaluate_progress_guard("ok", Instant::now(), &marker, now_unix());
        assert!(matches!(outcome, GuardOutcome::Accept(_)));
        let expected = "No chat_id found. Run `ralph bot onboard` to detect it";
        assert!(expected.starts_with("No chat_id"));
    }

    #[test]
    fn test_attach_marker_preserves_user_text() {
        let out = attach_marker("hello world");
        assert_eq!(out, "hello world [via Ralph agent]");
    }

    #[test]
    fn test_read_marker_missing_file_returns_none() {
        let (_tmp, marker) = fresh_marker_path();
        assert!(read_marker(&marker).is_none());
    }

    #[test]
    fn test_read_marker_invalid_contents_returns_none() {
        let (_tmp, marker) = fresh_marker_path();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "not-a-number").unwrap();
        assert!(read_marker(&marker).is_none());
    }
}
