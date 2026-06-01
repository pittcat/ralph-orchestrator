use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use crate::error::TelegramResult;
use crate::state::{StateManager, TelegramState};

/// Processes incoming Telegram messages and writes events to the correct loop's events.jsonl.
pub struct MessageHandler {
    state_manager: StateManager,
    workspace_root: PathBuf,
    /// Trusted response channel: when a `human.response` arrives, the text is
    /// also forwarded through this sender. `TelegramService::wait_for_response`
    /// owns the matching receiver and prefers this in-process path over the
    /// JSONL poll loop. This is the production source of trust.
    response_channel: Arc<Mutex<Option<UnboundedSender<String>>>>,
    /// Process-private nonce stamped on `human.response` events so the
    /// JSONL fallback path can verify ownership. Rotated per
    /// `wait_for_response` call by the service.
    response_nonce: Arc<Mutex<Option<String>>>,
}

impl MessageHandler {
    /// Create a new message handler rooted at the given workspace.
    pub fn new(
        state_manager: StateManager,
        workspace_root: impl Into<PathBuf>,
        response_channel: Arc<Mutex<Option<UnboundedSender<String>>>>,
        response_nonce: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            state_manager,
            workspace_root: workspace_root.into(),
            response_channel,
            response_nonce,
        }
    }

    /// Handle an incoming message from Telegram.
    ///
    /// Determines target loop, classifies as response or guidance, and appends
    /// the appropriate event to the loop's events.jsonl.
    ///
    /// Returns the event topic that was written (`"human.response"` or `"human.guidance"`).
    pub fn handle_message(
        &self,
        state: &mut TelegramState,
        text: &str,
        chat_id: i64,
        reply_to_message_id: Option<i32>,
    ) -> TelegramResult<String> {
        // Auto-detect chat ID from first message
        if state.chat_id.is_none() {
            state.chat_id = Some(chat_id);
            self.state_manager.save(state)?;
            tracing::info!(chat_id, "auto-detected chat ID from first message");
        }

        let target_loop = self.determine_target_loop(state, text, reply_to_message_id);
        let events_path = self.get_events_path(&target_loop);
        let is_response = state.pending_questions.contains_key(&target_loop);

        let topic = if is_response {
            "human.response"
        } else {
            "human.guidance"
        };

        let timestamp = Utc::now().to_rfc3339();

        // For `human.response`, stamp the trusted source marker and the
        // current nonce so the JSONL fallback path can verify that this event
        // originated from the active Telegram session, not from a forged
        // JSONL write by an agent.
        let mut event_json = serde_json::json!({
            "topic": topic,
            "payload": text,
            "ts": timestamp,
        });
        if is_response {
            let nonce = self
                .response_nonce
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_default();
            event_json["source"] = serde_json::Value::String(ralph_core::TRUSTED_HUMAN_RESPONSE_SOURCE.to_string());
            event_json["nonce"] = serde_json::Value::String(nonce);
        }
        let event_line = serde_json::to_string(&event_json)?;

        self.append_event(&events_path, &event_line)?;

        if is_response {
            self.state_manager
                .remove_pending_question(state, &target_loop)?;
            // Forward through the trusted channel so the blocking waiter wakes
            // immediately, without re-reading the events file.
            if let Ok(guard) = self.response_channel.lock()
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.send(text.to_string());
            }
        }

        tracing::info!(
            topic,
            target_loop,
            "wrote {} event for loop {}",
            topic,
            target_loop
        );

        Ok(topic.to_string())
    }

    /// Determine which loop a message is targeted at.
    ///
    /// Priority:
    /// 1. Reply to a pending question message → that loop
    /// 2. `@loop-id` prefix → extracted loop ID
    /// 3. Default → "main"
    fn determine_target_loop(
        &self,
        state: &TelegramState,
        text: &str,
        reply_to_message_id: Option<i32>,
    ) -> String {
        // Check reply-to routing
        if let Some(reply_id) = reply_to_message_id
            && let Some(loop_id) = self.state_manager.get_loop_for_reply(state, reply_id)
        {
            return loop_id;
        }

        // Check @loop-id prefix
        if let Some(loop_id) = text.strip_prefix('@')
            && let Some(id) = loop_id.split_whitespace().next()
            && !id.is_empty()
        {
            return id.to_string();
        }

        "main".to_string()
    }

    /// Get the active events file path for a given loop.
    ///
    /// Reads the `current-events` marker to find the timestamped events file.
    /// Falls back to the default `events.jsonl` if the marker doesn't exist.
    fn get_events_path(&self, loop_id: &str) -> PathBuf {
        let ralph_dir = if loop_id == "main" {
            self.workspace_root.join(".ralph")
        } else {
            self.workspace_root
                .join(".worktrees")
                .join(loop_id)
                .join(".ralph")
        };

        let marker_path = ralph_dir.join("current-events");
        if let Ok(contents) = std::fs::read_to_string(&marker_path) {
            let relative = contents.trim();
            if !relative.is_empty() {
                // Marker contains a path relative to workspace root
                // (e.g., ".ralph/events-20260201-210033.jsonl")
                if loop_id == "main" {
                    return self.workspace_root.join(relative);
                } else {
                    return self
                        .workspace_root
                        .join(".worktrees")
                        .join(loop_id)
                        .join(relative);
                }
            }
        }

        ralph_dir.join("events.jsonl")
    }

    /// Append an event line to the given file atomically.
    fn append_event(&self, path: &Path, event_line: &str) -> TelegramResult<()> {
        use std::fs::OpenOptions;
        use std::io::Write;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::TelegramError::EventWrite(format!(
                    "failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| {
                crate::error::TelegramError::EventWrite(format!(
                    "failed to open {}: {}",
                    path.display(),
                    e
                ))
            })?;

        writeln!(file, "{}", event_line).map_err(|e| {
            crate::error::TelegramError::EventWrite(format!(
                "failed to write to {}: {}",
                path.display(),
                e
            ))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TelegramState;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn setup() -> (MessageHandler, TempDir, TelegramState) {
        let dir = TempDir::new().unwrap();
        let state_path = dir.path().join(".ralph/telegram-state.json");
        let state_manager = StateManager::new(state_path);
        let response_channel = Arc::new(std::sync::Mutex::new(None::<UnboundedSender<String>>));
        let response_nonce = Arc::new(std::sync::Mutex::new(None::<String>));
        let handler = MessageHandler::new(
            state_manager,
            dir.path(),
            response_channel,
            response_nonce,
        );
        let state = TelegramState {
            chat_id: None,
            last_seen: None,
            last_update_id: None,
            pending_questions: HashMap::new(),
        };
        (handler, dir, state)
    }

    #[test]
    fn writes_guidance_event_to_main() {
        let (handler, dir, mut state) = setup();
        handler
            .handle_message(&mut state, "don't forget logging", 123, None)
            .unwrap();

        let events_path = dir.path().join(".ralph/events.jsonl");
        let contents = std::fs::read_to_string(events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(event["topic"], "human.guidance");
        assert_eq!(event["payload"], "don't forget logging");
    }

    #[test]
    fn writes_response_event_when_pending_question() {
        let (handler, dir, mut state) = setup();

        // Simulate a pending question for main loop
        state.pending_questions.insert(
            "main".to_string(),
            crate::state::PendingQuestion {
                asked_at: chrono::Utc::now(),
                message_id: 42,
            },
        );

        handler
            .handle_message(&mut state, "use async", 123, Some(42))
            .unwrap();

        let events_path = dir.path().join(".ralph/events.jsonl");
        let contents = std::fs::read_to_string(events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(event["topic"], "human.response");
        assert_eq!(event["payload"], "use async");

        // Pending question should be removed
        assert!(!state.pending_questions.contains_key("main"));
    }

    #[test]
    fn routes_at_prefix_to_correct_loop() {
        let (handler, dir, mut state) = setup();
        handler
            .handle_message(&mut state, "@feature-auth check edge cases", 123, None)
            .unwrap();

        let events_path = dir
            .path()
            .join(".worktrees/feature-auth/.ralph/events.jsonl");
        let contents = std::fs::read_to_string(events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(event["topic"], "human.guidance");
    }

    #[test]
    fn auto_detects_chat_id() {
        let (handler, _dir, mut state) = setup();
        assert!(state.chat_id.is_none());

        handler
            .handle_message(&mut state, "hello", 999, None)
            .unwrap();

        assert_eq!(state.chat_id, Some(999));
    }

    #[test]
    fn writes_to_timestamped_events_file_when_marker_exists() {
        let (handler, dir, mut state) = setup();

        // Create the .ralph dir and a current-events marker pointing to a timestamped file
        let ralph_dir = dir.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        std::fs::write(
            ralph_dir.join("current-events"),
            ".ralph/events-20260201-210033.jsonl",
        )
        .unwrap();

        handler
            .handle_message(&mut state, "progress update", 123, None)
            .unwrap();

        // Event should be written to the timestamped file, not events.jsonl
        let timestamped_path = dir.path().join(".ralph/events-20260201-210033.jsonl");
        assert!(
            timestamped_path.exists(),
            "event should be written to timestamped events file"
        );

        let contents = std::fs::read_to_string(&timestamped_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(event["topic"], "human.guidance");
        assert_eq!(event["payload"], "progress update");

        // The old default events.jsonl should NOT exist
        let default_path = dir.path().join(".ralph/events.jsonl");
        assert!(
            !default_path.exists(),
            "event should NOT be written to default events.jsonl when marker exists"
        );
    }
}
