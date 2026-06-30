# Security API Reference

## Overview

Ralph's security-related utilities are distributed across crates. Common safeguards:

- **Safe CLI execution** via `ralph_adapters::CliExecutor` (no shell invocation)
- **Secret masking** via `ralph_adapters::redact_token` (when integrating custom adapters that accept tokens)
- **Output escaping** for hook payloads via the standard library's `html_escape` crate (operator-supplied Slack / webhook integrations)

> **Note:** Earlier versions of this page documented secret-masking and HTML-escape
> helpers that were tied to the human-in-the-loop channel. That channel is retired;
> the recovery story now lives in `docs/guide/runtime-diagnosis.md` and the
> `task.resume` event topic. (`human.guidance` was removed by plan
> 2026-06-28-005; the recovery story is now `task.resume` plus
> `TerminationReason::RecoveryExhausted`.)

## Safe CLI Execution

`CliExecutor` uses `tokio::process::Command` with explicit argument vectors, which
avoids shell interpolation and reduces injection risk for prompt content.

```rust
use ralph_adapters::{CliBackend, CliExecutor};
use ralph_core::CliConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure a backend explicitly (no shell commands involved).
    let config = CliConfig {
        backend: "codex".to_string(),
        ..Default::default()
    };

    let backend = CliBackend::from_config(&config)?;
    let executor = CliExecutor::new(backend);

    let result = executor.execute_capture("Summarize this task.").await?;
    println!("success={} exit_code={:?}", result.success, result.exit_code);

    Ok(())
}
```

## Mask Secrets in Adapter Configuration

When wiring a custom adapter (for example a private backend or a webhook sink) that
accepts a bearer token, prefer explicit token redaction in diagnostic output rather than
embedding the token in `tracing` fields.

```rust
fn redact_token(token: &str) -> String {
    if token.len() <= 8 {
        return "<redacted>".to_string();
    }
    let prefix = &token[..4];
    let suffix = &token[token.len() - 4..];
    format!("{prefix}…{suffix}")
}

fn main() {
    let token = "1234567890:abcdefg_hijklmnop";
    println!("token={}", redact_token(token));
}
```

## Escape HTML for Hook Payloads

Hook payloads that are routed to HTML-aware sinks (for example a custom Slack or
webhook notifier) need to escape `&`, `<`, `>`, and quotes. The same pattern applies
whenever operator-supplied strings cross an HTML boundary.

```rust
fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn main() {
    let raw = "<task> & details";
    let safe = escape_html(raw);
    assert_eq!(safe, "&lt;task&gt; &amp; details");
}
```
