use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::bot::TelegramBot;
use crate::error::{TelegramError, TelegramResult};
use crate::handler::MessageHandler;
use crate::state::StateManager;

/// Maximum number of retry attempts for sending messages.
pub const MAX_SEND_RETRIES: u32 = 3;

/// Base delay for exponential backoff (1 second).
pub const BASE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Execute a fallible send operation with exponential backoff retry.
///
/// Retries up to [`MAX_SEND_RETRIES`] times with delays of 1s, 2s, 4s.
/// Returns the result on success, or `TelegramError::Send` after all
/// retries are exhausted.
///
/// The `sleep_fn` parameter allows tests to substitute a no-op sleep.
pub fn retry_with_backoff<F, S>(mut send_fn: F, mut sleep_fn: S) -> TelegramResult<i32>
where
    F: FnMut(u32) -> TelegramResult<i32>,
    S: FnMut(Duration),
{
    let mut last_error = String::new();

    for attempt in 1..=MAX_SEND_RETRIES {
        match send_fn(attempt) {
            Ok(msg_id) => return Ok(msg_id),
            Err(e) => {
                last_error = e.to_string();
                warn!(
                    attempt = attempt,
                    max_retries = MAX_SEND_RETRIES,
                    error = %last_error,
                    "Telegram send failed, {}",
                    if attempt < MAX_SEND_RETRIES {
                        "retrying with backoff"
                    } else {
                        "all retries exhausted"
                    }
                );
                if attempt < MAX_SEND_RETRIES {
                    let delay = BASE_RETRY_DELAY * 2u32.pow(attempt - 1);
                    sleep_fn(delay);
                }
            }
        }
    }

    Err(TelegramError::Send {
        attempts: MAX_SEND_RETRIES,
        reason: last_error,
    })
}

/// Additional context for enhanced check-in messages.
///
/// Provides richer information than the basic iteration + elapsed time,
/// including current hat, task progress, and cost tracking.
#[derive(Debug, Default)]
pub struct CheckinContext {
    /// The currently active hat name (e.g., "executor", "reviewer").
    pub current_hat: Option<String>,
    /// Number of open (non-terminal) tasks.
    pub open_tasks: usize,
    /// Number of closed tasks.
    pub closed_tasks: usize,
    /// Cumulative cost in USD.
    pub cumulative_cost: f64,
}

/// Coordinates the Telegram bot lifecycle with the Ralph event loop.
///
/// Manages startup, shutdown, message sending, and response waiting.
/// Uses the host tokio runtime (from `#[tokio::main]`) for async operations.
pub struct TelegramService {
    workspace_root: PathBuf,
    bot_token: String,
    api_url: Option<String>,
    timeout_secs: u64,
    loop_id: String,
    state_manager: StateManager,
    handler: MessageHandler,
    bot: TelegramBot,
    shutdown: Arc<AtomicBool>,
    /// Trusted response channel: when the message handler receives a
    /// `human.response` from Telegram, it forwards the response text through
    /// this sender. `wait_for_response` installs a matching receiver and
    /// prefers it over the JSONL poll loop. This is the production source of
    /// trust: agent-written JSONL `human.response` events cannot satisfy
    /// this path because they never touch the channel.
    response_channel: Arc<Mutex<Option<UnboundedSender<String>>>>,
    /// Process-private nonce stamped on `human.response` events written by
    /// the handler. `wait_for_response` rotates the nonce per call; the
    /// handler reads it and embeds it in the event so the JSONL fallback
    /// path can verify ownership even when the channel is unavailable.
    response_nonce: Arc<Mutex<Option<String>>>,
    /// When `true`, `wait_for_response` uses the degraded JSONL polling
    /// path (forgeable) instead of the trusted in-process channel. Tests
    /// toggle this via `set_mock_mode`; the `RALPH_TELEGRAM_MOCK` env var
    /// is the production opt-in.
    mock_mode: Arc<Mutex<bool>>,
}

impl TelegramService {
    /// Create a new TelegramService.
    ///
    /// Resolves the bot token from config or `RALPH_TELEGRAM_BOT_TOKEN` env var.
    /// When `api_url` is provided, all Telegram API requests target that URL
    /// instead of the default `https://api.telegram.org`.
    pub fn new(
        workspace_root: PathBuf,
        bot_token: Option<String>,
        api_url: Option<String>,
        timeout_secs: u64,
        loop_id: String,
    ) -> TelegramResult<Self> {
        let resolved_token = bot_token
            .or_else(|| std::env::var("RALPH_TELEGRAM_BOT_TOKEN").ok())
            .ok_or(TelegramError::MissingBotToken)?;

        let state_path = workspace_root.join(".ralph/telegram-state.json");
        let state_manager = StateManager::new(&state_path);
        let handler_state_manager = StateManager::new(&state_path);
        let response_channel = Arc::new(Mutex::new(None::<UnboundedSender<String>>));
        let response_nonce = Arc::new(Mutex::new(None::<String>));
        let mock_mode = Arc::new(Mutex::new(
            std::env::var("RALPH_TELEGRAM_MOCK").is_ok(),
        ));
        let handler = MessageHandler::new(
            handler_state_manager,
            &workspace_root,
            Arc::clone(&response_channel),
            Arc::clone(&response_nonce),
        );
        let bot = TelegramBot::new(&resolved_token, api_url.as_deref());
        let shutdown = Arc::new(AtomicBool::new(false));

        Ok(Self {
            workspace_root,
            bot_token: resolved_token,
            api_url,
            timeout_secs,
            loop_id,
            state_manager,
            handler,
            bot,
            shutdown,
            response_channel,
            response_nonce,
            mock_mode,
        })
    }

    /// Get a reference to the workspace root.
    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    /// Get the configured timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Get a reference to the bot token (masked for logging).
    pub fn bot_token_masked(&self) -> String {
        if self.bot_token.len() > 8 {
            format!(
                "{}...{}",
                &self.bot_token[..4],
                &self.bot_token[self.bot_token.len() - 4..]
            )
        } else {
            "****".to_string()
        }
    }

    /// Get a reference to the state manager.
    pub fn state_manager(&self) -> &StateManager {
        &self.state_manager
    }

    /// Get a mutable reference to the message handler.
    pub fn handler(&mut self) -> &mut MessageHandler {
        &mut self.handler
    }

    /// Get the loop ID this service is associated with.
    pub fn loop_id(&self) -> &str {
        &self.loop_id
    }

    /// Returns a clone of the shutdown flag.
    ///
    /// Signal handlers can set this flag to interrupt `wait_for_response()`
    /// without waiting for the full timeout.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Toggle the mock mode that selects between the trusted in-process
    /// channel and the degraded JSONL polling path. Tests use this to
    /// simulate an external writer without the trusted channel running.
    /// Production callers should not invoke this; set the
    /// `RALPH_TELEGRAM_MOCK` env var instead.
    pub fn set_mock_mode(&self, mock: bool) {
        if let Ok(mut guard) = self.mock_mode.lock() {
            *guard = mock;
        }
    }

    /// Start the Telegram service.
    ///
    /// Spawns a background polling task on the host tokio runtime to receive
    /// incoming messages. Must be called from within a tokio runtime context.
    pub fn start(&self) -> TelegramResult<()> {
        info!(
            bot_token = %self.bot_token_masked(),
            workspace = %self.workspace_root.display(),
            timeout_secs = self.timeout_secs,
            "Telegram service starting"
        );

        // Spawn the polling task on the host tokio runtime
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            TelegramError::Startup("no tokio runtime available for polling".to_string())
        })?;

        let raw_bot =
            crate::apply_api_url(teloxide::Bot::new(&self.bot_token), self.api_url.as_deref());
        let workspace_root = self.workspace_root.clone();
        let state_path = self.workspace_root.join(".ralph/telegram-state.json");
        let shutdown = self.shutdown.clone();
        let loop_id = self.loop_id.clone();
        let response_channel = Arc::clone(&self.response_channel);
        let response_nonce = Arc::clone(&self.response_nonce);

        handle.spawn(async move {
            Self::poll_updates(
                raw_bot,
                workspace_root,
                state_path,
                shutdown,
                loop_id,
                response_channel,
                response_nonce,
            )
            .await;
        });

        // Send greeting if we already know the chat ID
        if let Ok(state) = self.state_manager.load_or_default()
            && let Some(chat_id) = state.chat_id
        {
            let greeting = crate::bot::TelegramBot::format_greeting(&self.loop_id);
            match self.send_with_retry(chat_id, &greeting) {
                Ok(_) => info!("Sent greeting to chat {}", chat_id),
                Err(e) => warn!(error = %e, "Failed to send greeting"),
            }
        }

        info!("Telegram service started — polling for incoming messages");
        Ok(())
    }

    /// Background polling task that receives incoming Telegram messages.
    ///
    /// Uses long polling (`getUpdates`) to receive messages, then routes them
    /// through `MessageHandler` to write events to the correct loop's JSONL.
    async fn poll_updates(
        bot: teloxide::Bot,
        workspace_root: PathBuf,
        state_path: PathBuf,
        shutdown: Arc<AtomicBool>,
        loop_id: String,
        response_channel: Arc<Mutex<Option<UnboundedSender<String>>>>,
        response_nonce: Arc<Mutex<Option<String>>>,
    ) {
        use teloxide::payloads::{GetUpdatesSetters, SetMessageReactionSetters};
        use teloxide::requests::Requester;

        let state_manager = StateManager::new(&state_path);
        let handler_state_manager = StateManager::new(&state_path);
        let handler = MessageHandler::new(
            handler_state_manager,
            &workspace_root,
            response_channel,
            response_nonce,
        );
        let mut offset: i32 = 0;

        if let Ok(state) = state_manager.load_or_default()
            && let Some(last_update_id) = state.last_update_id
        {
            offset = last_update_id + 1;
        }

        // Register bot commands with Telegram API
        Self::register_commands(&bot).await;

        info!(loop_id = %loop_id, "Telegram polling task started");

        while !shutdown.load(Ordering::Relaxed) {
            let request = bot.get_updates().offset(offset).timeout(10);
            match request.await {
                Ok(updates) => {
                    for update in updates {
                        // Next offset = current update ID + 1
                        #[allow(clippy::cast_possible_wrap)]
                        {
                            offset = update.id.0 as i32 + 1;
                        }

                        // Extract message from update kind
                        let msg = match update.kind {
                            teloxide::types::UpdateKind::Message(msg) => msg,
                            _ => continue,
                        };

                        let text = match msg.text() {
                            Some(t) => t,
                            None => continue,
                        };

                        let chat_id = msg.chat.id.0;
                        let reply_to: Option<i32> = msg.reply_to_message().map(|r| r.id.0);

                        info!(
                            chat_id = chat_id,
                            text = %text,
                            "Received Telegram message"
                        );

                        // Handle bot commands before routing to handler.
                        // Unknown slash-commands are rejected here (not treated as guidance).
                        if crate::commands::is_command(text) {
                            let response = crate::commands::handle_command(text, &workspace_root)
                                .unwrap_or_else(|| {
                                    "Unknown command. Use /help for the supported commands."
                                        .to_string()
                                });

                            use teloxide::payloads::SendMessageSetters;
                            let send_result = bot
                                .send_message(teloxide::types::ChatId(chat_id), &response)
                                .parse_mode(teloxide::types::ParseMode::Html)
                                .await;
                            if let Err(e) = send_result {
                                warn!(error = %e, "Failed to send command response");
                            }
                            continue;
                        }

                        let mut state = match state_manager.load_or_default() {
                            Ok(s) => s,
                            Err(e) => {
                                warn!(error = %e, "Failed to load Telegram state");
                                continue;
                            }
                        };

                        match handler.handle_message(&mut state, text, chat_id, reply_to) {
                            Ok(topic) => {
                                let emoji = if topic == "human.response" {
                                    "👍"
                                } else {
                                    "👀"
                                };
                                let react_result = bot
                                    .set_message_reaction(teloxide::types::ChatId(chat_id), msg.id)
                                    .reaction(vec![teloxide::types::ReactionType::Emoji {
                                        emoji: emoji.to_string(),
                                    }])
                                    .await;
                                if let Err(e) = react_result {
                                    warn!(error = %e, "Failed to react to message");
                                }

                                // For guidance, also send a short text reply
                                if topic == "human.guidance" {
                                    let _ = bot
                                        .send_message(
                                            teloxide::types::ChatId(chat_id),
                                            "📝 <b>Guidance received</b> — will apply next iteration.",
                                        )
                                        .await;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    text = %text,
                                    "Failed to handle incoming Telegram message"
                                );
                            }
                        }

                        state.last_seen = Some(Utc::now());
                        state.last_update_id = Some(offset.saturating_sub(1));
                        if let Err(e) = state_manager.save(&state) {
                            warn!(error = %e, "Failed to persist Telegram state");
                        }
                    }
                }
                Err(e) => {
                    if !shutdown.load(Ordering::Relaxed) {
                        warn!(error = %e, "Telegram polling error — retrying in 5s");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }

        info!(loop_id = %loop_id, "Telegram polling task stopped");
    }

    /// Register bot commands with the Telegram API so they appear in the menu.
    async fn register_commands(bot: &teloxide::Bot) {
        use teloxide::requests::Requester;
        use teloxide::types::BotCommand;

        let commands = vec![
            BotCommand::new("status", "Current loop status"),
            BotCommand::new("tasks", "Open tasks"),
            BotCommand::new("memories", "Recent memories"),
            BotCommand::new("tail", "Last 20 events"),
            BotCommand::new("model", "Show current backend/model"),
            BotCommand::new("models", "Show configured model options"),
            BotCommand::new("restart", "Restart the loop"),
            BotCommand::new("stop", "Stop the loop"),
            BotCommand::new("help", "List available commands"),
        ];

        match bot.set_my_commands(commands).await {
            Ok(_) => info!("Registered bot commands with Telegram API"),
            Err(e) => warn!(error = %e, "Failed to register bot commands"),
        }
    }

    /// Stop the Telegram service gracefully.
    ///
    /// Signals the background polling task to shut down.
    pub fn stop(self) {
        // Send farewell if we know the chat ID
        if let Ok(state) = self.state_manager.load_or_default()
            && let Some(chat_id) = state.chat_id
        {
            let farewell = crate::bot::TelegramBot::format_farewell(&self.loop_id);
            match self.send_with_retry(chat_id, &farewell) {
                Ok(_) => info!("Sent farewell to chat {}", chat_id),
                Err(e) => warn!(error = %e, "Failed to send farewell"),
            }
        }

        self.shutdown.store(true, Ordering::Relaxed);
        info!(
            workspace = %self.workspace_root.display(),
            "Telegram service stopped"
        );
    }

    /// Send a question to the human via Telegram and store it as a pending question.
    ///
    /// The question payload is extracted from the `human.interact` event. A pending
    /// question is stored in the state manager so that incoming replies can be
    /// routed back to the correct loop.
    ///
    /// On send failure, retries up to 3 times with exponential backoff (1s, 2s, 4s).
    /// Returns the message ID of the sent Telegram message, or 0 if no chat ID
    /// is configured (question is logged but not sent).
    pub fn send_question(&self, payload: &str) -> TelegramResult<i32> {
        let mut state = self.state_manager.load_or_default()?;

        let message_id = if let Some(chat_id) = state.chat_id {
            self.send_with_retry(chat_id, payload)?
        } else {
            warn!(
                loop_id = %self.loop_id,
                "No chat ID configured — human.interact question logged but not sent: {}",
                payload
            );
            0
        };

        self.state_manager
            .add_pending_question(&mut state, &self.loop_id, message_id)?;

        debug!(
            loop_id = %self.loop_id,
            message_id = message_id,
            "Stored pending question"
        );

        Ok(message_id)
    }

    /// Send a periodic check-in message via Telegram.
    ///
    /// Loads the chat ID from state and sends a short status update so the
    /// human knows the loop is still running. Skips silently if no chat ID
    /// is configured. Returns `Ok(0)` when skipped, or the message ID on
    /// success.
    ///
    /// When a [`CheckinContext`] is provided, the message includes richer
    /// details: current hat, task progress, and cumulative cost.
    pub fn send_checkin(
        &self,
        iteration: u32,
        elapsed: Duration,
        context: Option<&CheckinContext>,
    ) -> TelegramResult<i32> {
        let state = self.state_manager.load_or_default()?;
        let Some(chat_id) = state.chat_id else {
            debug!(
                loop_id = %self.loop_id,
                "No chat ID configured — skipping check-in"
            );
            return Ok(0);
        };

        let elapsed_secs = elapsed.as_secs();
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        let elapsed_str = if minutes > 0 {
            format!("{}m {}s", minutes, seconds)
        } else {
            format!("{}s", seconds)
        };

        let msg = match context {
            Some(ctx) => {
                let mut lines = vec![format!(
                    "Still working — iteration <b>{}</b>, <code>{}</code> elapsed.",
                    iteration, elapsed_str
                )];

                if let Some(hat) = &ctx.current_hat {
                    lines.push(format!(
                        "Hat: <code>{}</code>",
                        crate::bot::escape_html(hat)
                    ));
                }

                if ctx.open_tasks > 0 || ctx.closed_tasks > 0 {
                    lines.push(format!(
                        "Tasks: <b>{}</b> open, {} closed",
                        ctx.open_tasks, ctx.closed_tasks
                    ));
                }

                if ctx.cumulative_cost > 0.0 {
                    lines.push(format!("Cost: <code>${:.4}</code>", ctx.cumulative_cost));
                }

                lines.join("\n")
            }
            None => format!(
                "Still working — iteration <b>{}</b>, <code>{}</code> elapsed.",
                iteration, elapsed_str
            ),
        };
        self.send_with_retry(chat_id, &msg)
    }

    /// Send a document (file) to the human via Telegram.
    ///
    /// Loads the chat ID from state and sends the file at `file_path` with an
    /// optional caption. Returns `Ok(0)` if no chat ID is configured.
    pub fn send_document(&self, file_path: &Path, caption: Option<&str>) -> TelegramResult<i32> {
        let state = self.state_manager.load_or_default()?;
        let Some(chat_id) = state.chat_id else {
            warn!(
                loop_id = %self.loop_id,
                file = %file_path.display(),
                "No chat ID configured — document not sent"
            );
            return Ok(0);
        };

        self.send_document_with_retry(chat_id, file_path, caption)
    }

    /// Send a photo to the human via Telegram.
    ///
    /// Loads the chat ID from state and sends the image at `file_path` with an
    /// optional caption. Returns `Ok(0)` if no chat ID is configured.
    pub fn send_photo(&self, file_path: &Path, caption: Option<&str>) -> TelegramResult<i32> {
        let state = self.state_manager.load_or_default()?;
        let Some(chat_id) = state.chat_id else {
            warn!(
                loop_id = %self.loop_id,
                file = %file_path.display(),
                "No chat ID configured — photo not sent"
            );
            return Ok(0);
        };

        self.send_photo_with_retry(chat_id, file_path, caption)
    }

    /// Attempt to send a message with exponential backoff retries.
    ///
    /// Uses the host tokio runtime via `block_in_place` + `Handle::block_on`
    /// to bridge the sync event loop to the async BotApi.
    fn send_with_retry(&self, chat_id: i64, payload: &str) -> TelegramResult<i32> {
        use crate::bot::BotApi;

        let handle = tokio::runtime::Handle::try_current().map_err(|_| TelegramError::Send {
            attempts: 0,
            reason: "no tokio runtime available for sending".to_string(),
        })?;

        retry_with_backoff(
            |_attempt| {
                tokio::task::block_in_place(|| {
                    handle.block_on(self.bot.send_message(chat_id, payload))
                })
            },
            |delay| std::thread::sleep(delay),
        )
    }

    /// Attempt to send a document with exponential backoff retries.
    fn send_document_with_retry(
        &self,
        chat_id: i64,
        file_path: &Path,
        caption: Option<&str>,
    ) -> TelegramResult<i32> {
        use crate::bot::BotApi;

        let handle = tokio::runtime::Handle::try_current().map_err(|_| TelegramError::Send {
            attempts: 0,
            reason: "no tokio runtime available for sending".to_string(),
        })?;

        retry_with_backoff(
            |_attempt| {
                tokio::task::block_in_place(|| {
                    handle.block_on(self.bot.send_document(chat_id, file_path, caption))
                })
            },
            |delay| std::thread::sleep(delay),
        )
    }

    /// Attempt to send a photo with exponential backoff retries.
    fn send_photo_with_retry(
        &self,
        chat_id: i64,
        file_path: &Path,
        caption: Option<&str>,
    ) -> TelegramResult<i32> {
        use crate::bot::BotApi;

        let handle = tokio::runtime::Handle::try_current().map_err(|_| TelegramError::Send {
            attempts: 0,
            reason: "no tokio runtime available for sending".to_string(),
        })?;

        retry_with_backoff(
            |_attempt| {
                tokio::task::block_in_place(|| {
                    handle.block_on(self.bot.send_photo(chat_id, file_path, caption))
                })
            },
            |delay| std::thread::sleep(delay),
        )
    }

    /// Poll the events file for a `human.response` event, blocking until one
    /// arrives or the configured timeout expires.
    ///
    /// Production path: install a trusted channel receiver and prefer it over
    /// the JSONL poll loop. The handler in `poll_updates` forwards real
    /// Telegram messages through the channel; agent-written JSONL
    /// `human.response` events never touch the channel and are ignored.
    ///
    /// Degraded path: if the channel is unavailable (e.g. test/mock mode that
    /// bypasses `start()`), fall back to JSONL polling. A warning is logged
    /// because this mode is forgeable by an agent that can write to the
    /// events file.
    pub fn wait_for_response(&self, events_path: &Path) -> TelegramResult<Option<String>> {
        let timeout = Duration::from_secs(self.timeout_secs);
        let deadline = Instant::now() + timeout;
        let nonce = generate_response_nonce();

        // Install the trusted receiver and publish the current nonce. When
        // the handler stamps the matching nonce on a `human.response` event
        // written to JSONL, the degraded fallback path can verify ownership.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        if let Ok(mut guard) = self.response_channel.lock() {
            *guard = Some(tx);
        }
        if let Ok(mut guard) = self.response_nonce.lock() {
            *guard = Some(nonce.clone());
        }

        // Track file position to only read new lines
        let initial_pos = if events_path.exists() {
            std::fs::metadata(events_path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let mut file_pos = initial_pos;
        // Production: use the trusted channel. Mock mode (tests or
        // RALPH_TELEGRAM_MOCK) opts into the degraded JSONL path.
        let use_channel = !*self.mock_mode.lock().unwrap_or_else(|p| p.into_inner());

        info!(
            loop_id = %self.loop_id,
            timeout_secs = self.timeout_secs,
            events_path = %events_path.display(),
            mode = if use_channel { "trusted-channel" } else { "degraded-jsonl" },
            "Waiting for human.response"
        );

        if !use_channel {
            warn!(
                loop_id = %self.loop_id,
                "Degraded JSONL polling enabled; responses on this path are forgeable by agents that can write to the events file"
            );
        }

        if use_channel {
            // Trusted channel path: prefer the in-process delivery. If the
            // channel sender is dropped (handler shut down) before the
            // deadline, fall through to the JSONL fallback so any
            // already-recorded response can still resolve the wait.
            let mut channel_closed = false;
            while !channel_closed {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    self.finish_wait();
                    self.clear_trusted_paths();
                    return Ok(None);
                }
                if self.shutdown.load(Ordering::Relaxed) {
                    self.finish_wait();
                    self.clear_trusted_paths();
                    return Ok(None);
                }
                match self.try_recv_channel(&mut rx, remaining.min(Duration::from_millis(50))) {
                    Ok(Some(text)) => {
                        self.finish_wait();
                        self.clear_trusted_paths();
                        return Ok(Some(text));
                    }
                    Ok(None) => continue,
                    Err(()) => channel_closed = true,
                }
            }
            warn!(
                loop_id = %self.loop_id,
                "Trusted response channel closed before timeout; falling back to degraded JSONL polling"
            );
        }

        // Degraded JSONL path (mock mode or channel-closed fallback):
        // require the trusted source marker and matching nonce.
        while Instant::now() < deadline {
            if self.shutdown.load(Ordering::Relaxed) {
                self.finish_wait();
                self.clear_trusted_paths();
                return Ok(None);
            }
            if let Some(response) =
                Self::check_for_response_with_nonce(events_path, &mut file_pos, &nonce)?
            {
                self.finish_wait();
                self.clear_trusted_paths();
                return Ok(Some(response));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        self.finish_wait();
        self.clear_trusted_paths();
        Ok(None)
    }

    fn finish_wait(&self) {
        if let Ok(mut state) = self.state_manager.load_or_default() {
            let _ = self
                .state_manager
                .remove_pending_question(&mut state, &self.loop_id);
        }
    }

    fn clear_trusted_paths(&self) {
        if let Ok(mut guard) = self.response_channel.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.response_nonce.lock() {
            *guard = None;
        }
    }

    /// Try to receive a response from the trusted channel within `timeout`.
    /// Returns `Ok(Some(text))` on a delivered message, `Ok(None)` on
    /// timeout, and `Err(())` if the sender was dropped.
    fn try_recv_channel(
        &self,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
        timeout: Duration,
    ) -> Result<Option<String>, ()> {
        match rx.try_recv() {
            Ok(text) => Ok(Some(text)),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(timeout);
                match rx.try_recv() {
                    Ok(text) => Ok(Some(text)),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(None),
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => Err(()),
                }
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Check the events file for a `human.response` event with an expected
    /// nonce. When `expected_nonce` is non-empty, only events whose `source`
    /// marker matches `TRUSTED_HUMAN_RESPONSE_SOURCE` and whose `nonce` field
    /// equals `expected_nonce` are accepted. When the expected nonce is
    /// empty, the legacy behavior is used (any `human.response` is accepted).
    fn check_for_response_with_nonce(
        events_path: &Path,
        file_pos: &mut u64,
        expected_nonce: &str,
    ) -> TelegramResult<Option<String>> {
        use ralph_core::TRUSTED_HUMAN_RESPONSE_SOURCE;
        use std::io::{BufRead, BufReader, Seek, SeekFrom};

        if !events_path.exists() {
            return Ok(None);
        }

        let mut file = std::fs::File::open(events_path)?;
        file.seek(SeekFrom::Start(*file_pos))?;

        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            let line_bytes = line.len() as u64 + 1; // +1 for newline
            *file_pos += line_bytes;

            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as JSON event
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line)
                && event.get("topic").and_then(|t| t.as_str()) == Some("human.response")
            {
                if !expected_nonce.is_empty() {
                    let source = event
                        .get("source")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let event_nonce = event.get("nonce").and_then(|s| s.as_str()).unwrap_or("");
                    if source != TRUSTED_HUMAN_RESPONSE_SOURCE || event_nonce != expected_nonce {
                        debug!(
                            expected = %expected_nonce,
                            event_nonce = %event_nonce,
                            source = %source,
                            "Skipping human.response without matching trusted source/nonce"
                        );
                        continue;
                    }
                }
                let message = event
                    .get("payload")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(Some(message));
            }

            // Also check pipe-separated format (written by MessageHandler)
            if line.contains("EVENT: human.response") {
                // Pipe format is only used by legacy untrusted callers; when
                // the trusted nonce is required, ignore these.
                if !expected_nonce.is_empty() {
                    continue;
                }
                // Extract message from pipe-separated format:
                // EVENT: human.response | message: "..." | timestamp: "..."
                let message = line
                    .split('|')
                    .find(|part| part.trim().starts_with("message:"))
                    .and_then(|part| {
                        let value = part.trim().strip_prefix("message:")?;
                        let trimmed = value.trim().trim_matches('"');
                        Some(trimmed.to_string())
                    })
                    .unwrap_or_default();
                return Ok(Some(message));
            }
        }

        Ok(None)
    }
}

/// Generate a fresh, process-private nonce for a `wait_for_response` call.
/// The nonce is random per call so agent-written JSONL events cannot satisfy
/// the waiter even if they guess a single value.
fn generate_response_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("n{}-{}", pid, nanos)
}

impl ralph_proto::RobotService for TelegramService {
    fn send_question(&self, payload: &str) -> anyhow::Result<i32> {
        Ok(TelegramService::send_question(self, payload)?)
    }

    fn wait_for_response(&self, events_path: &Path) -> anyhow::Result<Option<String>> {
        Ok(TelegramService::wait_for_response(self, events_path)?)
    }

    fn send_checkin(
        &self,
        iteration: u32,
        elapsed: Duration,
        context: Option<&ralph_proto::CheckinContext>,
    ) -> anyhow::Result<i32> {
        // Convert ralph_proto::CheckinContext to the local CheckinContext
        let local_context = context.map(|ctx| CheckinContext {
            current_hat: ctx.current_hat.clone(),
            open_tasks: ctx.open_tasks,
            closed_tasks: ctx.closed_tasks,
            cumulative_cost: ctx.cumulative_cost,
        });
        Ok(TelegramService::send_checkin(
            self,
            iteration,
            elapsed,
            local_context.as_ref(),
        )?)
    }

    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    fn stop(self: Box<Self>) {
        TelegramService::stop(*self);
    }
}

impl fmt::Debug for TelegramService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramService")
            .field("workspace_root", &self.workspace_root)
            .field("bot_token", &self.bot_token_masked())
            .field("timeout_secs", &self.timeout_secs)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn test_service(dir: &TempDir) -> TelegramService {
        TelegramService::new(
            dir.path().to_path_buf(),
            Some("test-token-12345".to_string()),
            None,
            300,
            "main".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn new_with_explicit_token() {
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("test-token-12345".to_string()),
            None,
            300,
            "main".to_string(),
        );
        assert!(service.is_ok());
    }

    #[test]
    fn new_without_token_fails() {
        // Only run this test when the env var is not set,
        // to avoid needing unsafe remove_var
        if std::env::var("RALPH_TELEGRAM_BOT_TOKEN").is_ok() {
            return;
        }

        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            None,
            None,
            300,
            "main".to_string(),
        );
        assert!(service.is_err());
        assert!(matches!(
            service.unwrap_err(),
            TelegramError::MissingBotToken
        ));
    }

    #[test]
    fn bot_token_masked_works() {
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("abcd1234efgh5678".to_string()),
            None,
            300,
            "main".to_string(),
        )
        .unwrap();
        let masked = service.bot_token_masked();
        assert_eq!(masked, "abcd...5678");
    }

    #[test]
    fn loop_id_accessor() {
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            60,
            "feature-auth".to_string(),
        )
        .unwrap();
        assert_eq!(service.loop_id(), "feature-auth");
    }

    #[test]
    fn send_question_stores_pending_question() {
        let dir = TempDir::new().unwrap();
        let service = test_service(&dir);

        service.send_question("Which DB to use?").unwrap();

        // Verify pending question is stored
        let state = service.state_manager().load_or_default().unwrap();
        assert!(
            state.pending_questions.contains_key("main"),
            "pending question should be stored for loop_id 'main'"
        );
    }

    #[test]
    fn send_question_returns_message_id() {
        let dir = TempDir::new().unwrap();
        let service = test_service(&dir);

        let msg_id = service.send_question("async or sync?").unwrap();
        // Without a chat_id in state, message_id is 0
        assert_eq!(msg_id, 0);
    }

    #[test]
    fn check_for_response_json_format() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("events.jsonl");

        // Write a non-response event first
        let mut file = std::fs::File::create(&events_path).unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","payload":"tests: pass, lint: pass, typecheck: pass, audit: pass, coverage: pass","ts":"2026-01-30T00:00:00Z"}}"#
        )
        .unwrap();
        // Write a human.response event
        writeln!(
            file,
            r#"{{"topic":"human.response","payload":"Use async","ts":"2026-01-30T00:01:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut pos = 0;
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, Some("Use async".to_string()));
    }

    #[test]
    fn check_for_response_pipe_format() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("events.jsonl");

        let mut file = std::fs::File::create(&events_path).unwrap();
        writeln!(
            file,
            r#"EVENT: human.response | message: "Use sync" | timestamp: "2026-01-30T00:01:00Z""#
        )
        .unwrap();
        file.flush().unwrap();

        let mut pos = 0;
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, Some("Use sync".to_string()));
    }

    #[test]
    fn check_for_response_skips_non_response_events() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("events.jsonl");

        let mut file = std::fs::File::create(&events_path).unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","payload":"done","ts":"2026-01-30T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"human.guidance","payload":"check errors","ts":"2026-01-30T00:01:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut pos = 0;
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn check_for_response_missing_file() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("does-not-exist.jsonl");

        let mut pos = 0;
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn check_for_response_tracks_position() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("events.jsonl");

        // Write one event
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&events_path)
            .unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","payload":"done","ts":"2026-01-30T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut pos = 0;
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, None);
        assert!(pos > 0, "position should advance after reading");

        let pos_after_first = pos;

        // Append a human.response
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(
            file,
            r#"{{"topic":"human.response","payload":"yes","ts":"2026-01-30T00:02:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        // Should find the response starting from where we left off
        let result = TelegramService::check_for_response_with_nonce(&events_path, &mut pos, "").unwrap();
        assert_eq!(result, Some("yes".to_string()));
        assert!(pos > pos_after_first, "position should advance further");
    }

    #[test]
    fn wait_for_response_returns_on_response() {
        // Opt into the degraded JSONL polling path: this test simulates
        // an external writer (not the trusted channel).
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            5, // enough time for the writer thread
            "main".to_string(),
        )
        .unwrap();
        service.set_mock_mode(true);

        let events_path = dir.path().join("events.jsonl");
        // Create an empty events file so wait_for_response records position 0
        std::fs::File::create(&events_path).unwrap();

        // Store a pending question first
        service.send_question("Which plan?").unwrap();

        let service = std::sync::Arc::new(service);
        let writer_service = std::sync::Arc::clone(&service);

        // Spawn a thread to call wait_for_response; the writer thread will
        // append a response with the current nonce once it appears.
        let path_for_wait = events_path.clone();
        let wait_service = std::sync::Arc::clone(&service);
        let waiter = std::thread::spawn(move || {
            wait_service.wait_for_response(&path_for_wait).unwrap()
        });

        // Wait until wait_for_response has installed its nonce.
        let nonce = {
            let mut attempts = 0;
            loop {
                if let Some(n) = writer_service
                    .response_nonce
                    .lock()
                    .unwrap()
                    .clone()
                {
                    break n;
                }
                attempts += 1;
                if attempts > 200 {
                    panic!("wait_for_response did not install a nonce in time");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        // Write the response with the matching trusted source + nonce.
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&events_path)
                .unwrap();
            writeln!(
                file,
                r#"{{"topic":"human.response","payload":"Go with plan A","ts":"2026-01-30T00:00:00Z","source":"robot-trusted","nonce":"{nonce}"}}"#
            )
            .unwrap();
            file.flush().unwrap();
        }

        let result = waiter.join().unwrap();
        service.set_mock_mode(false);

        assert_eq!(result, Some("Go with plan A".to_string()));

        // Pending question should be removed
        let state = service.state_manager().load_or_default().unwrap();
        assert!(
            !state.pending_questions.contains_key("main"),
            "pending question should be removed after response"
        );
    }

    #[test]
    fn wait_for_response_returns_none_on_timeout() {
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            1, // 1 second timeout
            "main".to_string(),
        )
        .unwrap();

        let events_path = dir.path().join("events.jsonl");
        // Create an empty events file with no human.response
        std::fs::File::create(&events_path).unwrap();

        // Store a pending question
        service.send_question("Will this timeout?").unwrap();

        let result = service.wait_for_response(&events_path).unwrap();
        assert_eq!(result, None, "should return None on timeout");

        // Pending question should be removed even on timeout
        let state = service.state_manager().load_or_default().unwrap();
        assert!(
            !state.pending_questions.contains_key("main"),
            "pending question should be removed on timeout"
        );
    }

    #[test]
    fn retry_with_backoff_succeeds_on_first_attempt() {
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts_clone = attempts.clone();

        let result = retry_with_backoff(
            |attempt| {
                attempts_clone.lock().unwrap().push(attempt);
                Ok(42)
            },
            |_delay| {},
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(*attempts.lock().unwrap(), vec![1]);
    }

    #[test]
    fn retry_with_backoff_succeeds_on_second_attempt() {
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts_clone = attempts.clone();
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();

        let result = retry_with_backoff(
            |attempt| {
                attempts_clone.lock().unwrap().push(attempt);
                if attempt < 2 {
                    Err(TelegramError::Send {
                        attempts: attempt,
                        reason: "transient failure".to_string(),
                    })
                } else {
                    Ok(99)
                }
            },
            |delay| {
                delays_clone.lock().unwrap().push(delay);
            },
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 99);
        assert_eq!(*attempts.lock().unwrap(), vec![1, 2]);
        // First retry delay: 1s * 2^0 = 1s
        assert_eq!(*delays.lock().unwrap(), vec![Duration::from_secs(1)]);
    }

    #[test]
    fn retry_with_backoff_succeeds_on_third_attempt() {
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts_clone = attempts.clone();
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();

        let result = retry_with_backoff(
            |attempt| {
                attempts_clone.lock().unwrap().push(attempt);
                if attempt < 3 {
                    Err(TelegramError::Send {
                        attempts: attempt,
                        reason: "transient failure".to_string(),
                    })
                } else {
                    Ok(7)
                }
            },
            |delay| {
                delays_clone.lock().unwrap().push(delay);
            },
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 7);
        assert_eq!(*attempts.lock().unwrap(), vec![1, 2, 3]);
        // Delays: 1s * 2^0 = 1s, 1s * 2^1 = 2s
        assert_eq!(
            *delays.lock().unwrap(),
            vec![Duration::from_secs(1), Duration::from_secs(2)]
        );
    }

    #[test]
    fn retry_with_backoff_fails_after_all_retries() {
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts_clone = attempts.clone();
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();

        let result = retry_with_backoff(
            |attempt| {
                attempts_clone.lock().unwrap().push(attempt);
                Err(TelegramError::Send {
                    attempts: attempt,
                    reason: format!("failure on attempt {}", attempt),
                })
            },
            |delay| {
                delays_clone.lock().unwrap().push(delay);
            },
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            TelegramError::Send {
                attempts: 3,
                reason: _
            }
        ));
        // Should report the last error message
        if let TelegramError::Send { reason, .. } = &err {
            assert!(reason.contains("failure on attempt 3"));
        }
        assert_eq!(*attempts.lock().unwrap(), vec![1, 2, 3]);
        // Delays: 1s, 2s (no delay after final attempt)
        assert_eq!(
            *delays.lock().unwrap(),
            vec![Duration::from_secs(1), Duration::from_secs(2)]
        );
    }

    #[test]
    fn retry_with_backoff_exponential_delays_are_correct() {
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();

        let _ = retry_with_backoff(
            |_attempt| {
                Err(TelegramError::Send {
                    attempts: 1,
                    reason: "always fail".to_string(),
                })
            },
            |delay| {
                delays_clone.lock().unwrap().push(delay);
            },
        );

        let recorded = delays.lock().unwrap().clone();
        // Backoff: 1s * 2^0 = 1s, 1s * 2^1 = 2s (no sleep after 3rd attempt)
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0], Duration::from_secs(1));
        assert_eq!(recorded[1], Duration::from_secs(2));
    }

    #[test]
    fn checkin_context_default() {
        let ctx = CheckinContext::default();
        assert!(ctx.current_hat.is_none());
        assert_eq!(ctx.open_tasks, 0);
        assert_eq!(ctx.closed_tasks, 0);
        assert!(ctx.cumulative_cost.abs() < f64::EPSILON);
    }

    #[test]
    fn checkin_context_with_hat_and_tasks() {
        let ctx = CheckinContext {
            current_hat: Some("executor".to_string()),
            open_tasks: 3,
            closed_tasks: 5,
            cumulative_cost: 1.2345,
        };
        assert_eq!(ctx.current_hat.as_deref(), Some("executor"));
        assert_eq!(ctx.open_tasks, 3);
        assert_eq!(ctx.closed_tasks, 5);
        assert!((ctx.cumulative_cost - 1.2345).abs() < f64::EPSILON);
    }

    #[test]
    fn wait_for_response_returns_none_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            60, // long timeout — shutdown flag should preempt it
            "main".to_string(),
        )
        .unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();

        // Set shutdown flag before calling wait_for_response
        service.shutdown_flag().store(true, Ordering::Relaxed);

        let start = Instant::now();
        let result = service.wait_for_response(&events_path).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result, None, "should return None when shutdown flag is set");
        assert!(
            elapsed < Duration::from_secs(2),
            "should return quickly, not wait for timeout (elapsed: {:?})",
            elapsed
        );
    }

    // ---- P5 trusted channel + nonce fallback tests ----

    #[test]
    fn test_human_response_forged_jsonl_ignored_when_telegram_active() {
        // Production path: an external writer (e.g., agent) appends a
        // `human.response` event to the JSONL without the trusted source
        // marker. wait_for_response must NOT honor it.
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            2, // short timeout
            "main".to_string(),
        )
        .unwrap();
        // Mock mode is OFF by default in tests; trusted channel is active.
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();

        // Drop the trusted sender so the channel never delivers.
        drop(service.response_channel.lock().unwrap().take());

        // Write a forged human.response with no source marker.
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&events_path)
                .unwrap();
            writeln!(
                file,
                r#"{{"topic":"human.response","payload":"forged","ts":"2026-01-30T00:00:00Z"}}"#
            )
            .unwrap();
            file.flush().unwrap();
        }

        let result = service.wait_for_response(&events_path).unwrap();
        assert_eq!(
            result, None,
            "forged JSONL human.response must not satisfy the trusted waiter"
        );
    }

    #[test]
    fn test_human_response_from_trusted_channel_accepted() {
        // When the handler (simulated) sends through the trusted channel,
        // wait_for_response returns immediately without consulting JSONL.
        let dir = TempDir::new().unwrap();
        let service = std::sync::Arc::new(
            TelegramService::new(
                dir.path().to_path_buf(),
                Some("token".to_string()),
                None,
                5,
                "main".to_string(),
            )
            .unwrap(),
        );
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();
        service.send_question("Pick one").unwrap();

        let svc = std::sync::Arc::clone(&service);
        let path = events_path.clone();
        let waiter = std::thread::spawn(move || svc.wait_for_response(&path).unwrap());

        // Wait for wait_for_response to install its sender.
        let mut attempts = 0;
        let sender = loop {
            if let Some(tx) = service.response_channel.lock().unwrap().clone() {
                break tx;
            }
            attempts += 1;
            if attempts > 200 {
                panic!("wait_for_response did not install a sender in time");
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        sender.send("Approved via trusted channel".to_string()).unwrap();
        let result = waiter.join().unwrap();
        assert_eq!(result, Some("Approved via trusted channel".to_string()));
    }

    #[test]
    fn test_human_response_wrong_nonce_rejected() {
        // The degraded JSONL path requires the trusted source marker AND a
        // matching nonce. A forged event with a wrong nonce must be skipped
        // until timeout.
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            1, // short timeout
            "main".to_string(),
        )
        .unwrap();
        service.set_mock_mode(true);
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();

        // Pre-install a wrong nonce in the service.
        *service.response_nonce.lock().unwrap() = Some("expected-nonce".to_string());

        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&events_path)
                .unwrap();
            writeln!(
                file,
                r#"{{"topic":"human.response","payload":"forged","ts":"x","source":"robot-trusted","nonce":"WRONG"}}"#
            )
            .unwrap();
            file.flush().unwrap();
        }

        let result = service.wait_for_response(&events_path).unwrap();
        assert_eq!(
            result, None,
            "response with the wrong nonce must not satisfy the degraded waiter"
        );
    }

    #[test]
    fn test_human_response_timeout_still_injects_timeout() {
        // The trusted waiter path must still respect the configured timeout.
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            1,
            "main".to_string(),
        )
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();
        service.send_question("any?").unwrap();

        let start = Instant::now();
        let result = service.wait_for_response(&events_path).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result, None);
        assert!(
            elapsed < Duration::from_secs(3),
            "timeout should fire within the configured window (elapsed: {:?})",
            elapsed
        );
    }

    #[test]
    fn test_degraded_jsonl_polling_only_enabled_in_mock_mode() {
        // When mock_mode is false (the default in tests), an unmatched JSONL
        // event must not satisfy wait_for_response; the trusted path is
        // active and no channel sender delivers.
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            1,
            "main".to_string(),
        )
        .unwrap();
        // mock_mode default = false (production trusted channel active).
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();

        // Drop the trusted sender so the channel closes immediately.
        drop(service.response_channel.lock().unwrap().take());

        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&events_path)
                .unwrap();
            writeln!(
                file,
                r#"{{"topic":"human.response","payload":"untrusted","ts":"x"}}"#
            )
            .unwrap();
            file.flush().unwrap();
        }

        let result = service.wait_for_response(&events_path).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_degraded_jsonl_polling_logs_warning() {
        // Enable mock mode and verify wait_for_response completes (this also
        // exercises the warning log path; the log itself is not asserted to
        // keep the test hermetic).
        let dir = TempDir::new().unwrap();
        let service = TelegramService::new(
            dir.path().to_path_buf(),
            Some("token".to_string()),
            None,
            1,
            "main".to_string(),
        )
        .unwrap();
        service.set_mock_mode(true);
        let events_path = dir.path().join("events.jsonl");
        std::fs::File::create(&events_path).unwrap();
        service.send_question("any?").unwrap();
        let result = service.wait_for_response(&events_path).unwrap();
        assert_eq!(result, None);
    }
}
