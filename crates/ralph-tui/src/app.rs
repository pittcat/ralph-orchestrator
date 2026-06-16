//! Main application loop for the TUI.
//!
//! This module provides a read-only observation dashboard that displays
//! formatted output from the Ralph orchestrator, with iteration navigation,
//! scroll, and search functionality.

use crate::input::{Action, map_key};
use crate::rpc_writer::RpcWriter;
use crate::state::TuiState;
use crate::update_check;
use crate::widgets::{content::ContentPane, footer, header, help};
use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
};
use scopeguard::defer;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWrite;
use tokio::sync::watch;
use tokio::time::{Duration, interval};
use tracing::info;

/// Notifies the backend subprocess (or in-process main loop) that the user
/// requested quit, before the TUI exits.
///
/// Symmetric with the Ctrl+C abort branch in `App::run` (app.rs:238-254).
/// The RPC writer path is preferred when both are available (subprocess mode);
/// the watch channel is the in-process fallback. If neither is set (e.g. a
/// test harness), the call is a no-op so the loop can still exit cleanly.
///
/// This function is fire-and-forget: the RPC variant spawns the abort send
/// as a tokio task; the channel variant is synchronous. Either way the
/// caller should `break` out of the event loop right after, and the
/// `defer!` terminal cleanup guard runs on the way out.
pub(crate) fn notify_backend_quit<W: AsyncWrite + Unpin + Send + 'static>(
    rpc_writer: Option<&RpcWriter<W>>,
    interrupt_tx: Option<&watch::Sender<bool>>,
) {
    info!(
        target: "ralph_tui::app",
        has_rpc_writer = rpc_writer.is_some(),
        has_interrupt_tx = interrupt_tx.is_some(),
        our_pid = std::process::id(),
        "notify_backend_quit() called"
    );
    if let Some(writer) = rpc_writer {
        let writer = writer.clone();
        tokio::spawn(async move {
            info!(
                target: "ralph_tui::app",
                our_pid = std::process::id(),
                "notify_backend_quit sending Abort via RPC"
            );
            let _ = writer.send_abort().await;
        });
    } else if let Some(tx) = interrupt_tx {
        info!(
            target: "ralph_tui::app",
            our_pid = std::process::id(),
            "notify_backend_quit sending interrupt via watch channel"
        );
        let _ = tx.send(true);
    }
}

/// Dispatches an action to the TuiState.
///
/// Returns `true` if the action signals to quit the application.
///
/// **Note:** `Action::Quit` is intentionally NOT handled here. The `App::run`
/// input loop intercepts Quit before calling this function so it can call
/// `notify_backend_quit` (sending the abort RPC / interrupt signal) before
/// breaking. If Quit were dispatched here, the loop would break silently and
/// the parent CLI would hang on `child_wait.await` waiting for the orphaned
/// backend subprocess. See the regression test
/// `dispatch_action_quit_is_no_longer_handled_by_dispatcher` and the bug
/// report referenced in the test module.
pub fn dispatch_action(action: Action, state: &mut TuiState, viewport_height: usize) -> bool {
    match action {
        // Quit is owned by App::run, not by the dispatcher. Do not match.
        Action::Quit => {}
        Action::ScrollDown => {
            if state.wave_view_active {
                if let Some(buffer) = state.current_wave_worker_buffer_mut() {
                    buffer.scroll_down(viewport_height);
                }
            } else if let Some(buffer) = state.current_iteration_mut() {
                buffer.scroll_down(viewport_height);
            }
        }
        Action::ScrollUp => {
            if state.wave_view_active {
                if let Some(buffer) = state.current_wave_worker_buffer_mut() {
                    buffer.scroll_up();
                }
            } else if let Some(buffer) = state.current_iteration_mut() {
                buffer.scroll_up();
            }
        }
        Action::ScrollTop => {
            if state.wave_view_active {
                if let Some(buffer) = state.current_wave_worker_buffer_mut() {
                    buffer.scroll_top();
                }
            } else if let Some(buffer) = state.current_iteration_mut() {
                buffer.scroll_top();
            }
        }
        Action::ScrollBottom => {
            if state.wave_view_active {
                if let Some(buffer) = state.current_wave_worker_buffer_mut() {
                    buffer.scroll_bottom(viewport_height);
                }
            } else if let Some(buffer) = state.current_iteration_mut() {
                buffer.scroll_bottom(viewport_height);
            }
        }
        Action::NextIteration => {
            if state.wave_view_active {
                state.wave_view_next();
            } else {
                state.navigate_next();
            }
        }
        Action::PrevIteration => {
            if state.wave_view_active {
                state.wave_view_prev();
            } else {
                state.navigate_prev();
            }
        }
        Action::ShowHelp => {
            state.show_help = true;
        }
        Action::DismissHelp => {
            if state.wave_view_active {
                state.exit_wave_view();
            } else {
                state.show_help = false;
                state.clear_search();
            }
        }
        Action::StartSearch => {
            state.search_state.search_mode = true;
        }
        Action::SearchNext => {
            state.next_match();
        }
        Action::SearchPrev => {
            state.prev_match();
        }
        Action::GuidanceNext => {
            state.start_guidance(crate::state::GuidanceMode::Next);
        }
        Action::GuidanceNow => {
            state.start_guidance(crate::state::GuidanceMode::Now);
        }
        Action::EnterWaveView => {
            state.enter_wave_view();
        }
        Action::ToggleMouseMode => {
            state.mouse_capture_enabled = !state.mouse_capture_enabled;
        }
        Action::None => {}
    }
    false
}

fn set_mouse_capture(enabled: bool) -> Result<()> {
    if enabled {
        execute!(io::stdout(), EnableMouseCapture)?;
    } else {
        execute!(io::stdout(), DisableMouseCapture)?;
    }
    Ok(())
}

/// Main TUI application for read-only observation.
pub struct App<W = tokio::process::ChildStdin> {
    state: Arc<Mutex<TuiState>>,
    /// Receives notification when the underlying process terminates.
    /// This is the ONLY exit path for the TUI event loop (besides Action::Quit).
    terminated_rx: watch::Receiver<bool>,
    /// Channel to signal main loop on Ctrl+C.
    /// In raw terminal mode, SIGINT is not generated, so TUI must signal
    /// the main orchestration loop through this channel.
    interrupt_tx: Option<watch::Sender<bool>>,
    /// RPC writer for subprocess mode (replaces interrupt_tx for abort).
    rpc_writer: Option<RpcWriter<W>>,
}

impl App<tokio::process::ChildStdin> {
    /// Creates a new App with shared state, termination signal, and optional interrupt channel.
    pub fn new(
        state: Arc<Mutex<TuiState>>,
        terminated_rx: watch::Receiver<bool>,
        interrupt_tx: Option<watch::Sender<bool>>,
    ) -> Self {
        Self {
            state,
            terminated_rx,
            interrupt_tx,
            rpc_writer: None,
        }
    }
}

impl<W: AsyncWrite + Unpin + Send + 'static> App<W> {
    /// Creates a new App for subprocess mode with an RPC writer.
    pub fn new_subprocess(
        state: Arc<Mutex<TuiState>>,
        terminated_rx: watch::Receiver<bool>,
        rpc_writer: RpcWriter<W>,
    ) -> Self {
        Self {
            state,
            terminated_rx,
            interrupt_tx: None,
            rpc_writer: Some(rpc_writer),
        }
    }

    /// Runs the TUI event loop.
    pub async fn run(mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        // CRITICAL: Ensure terminal cleanup on ANY exit path (normal, abort, or panic).
        // When cleanup_tui() calls handle.abort(), the task is cancelled immediately
        // at its current await point, skipping all code after the loop. This defer!
        // guard runs on Drop, which is guaranteed even during task cancellation.
        let cleanup_state = Arc::clone(&self.state);
        defer! {
            let _ = disable_raw_mode();
            let mouse_capture_enabled = cleanup_state
                .lock()
                .map(|state| state.mouse_capture_enabled)
                .unwrap_or(false);
            if mouse_capture_enabled {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        }

        let update_state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let status = update_check::fetch_update_status().await;
            if let Ok(mut state) = update_state.lock() {
                state.set_update_status(status);
            }
        });

        // Event-driven architecture: input polling is the primary driver
        // Render is throttled to ~60fps via interval tick
        let mut events = EventStream::new();
        let mut render_tick = interval(Duration::from_millis(16));

        // Track viewport height for scroll calculations
        let mut viewport_height: usize = 24; // Default, updated on render

        loop {
            // Use biased select to prioritize input over render ticks
            tokio::select! {
                biased;

                // Priority 1: Handle input events immediately for responsiveness
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            match event {
                                // Handle Ctrl+C: signal main loop and exit.
                                // In raw mode, SIGINT is not generated, so we must signal the
                                // main orchestration loop through interrupt_tx channel or RPC writer.
                                Event::Key(key) if key.kind == KeyEventKind::Press
                                    && key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    info!("Ctrl+C detected, signaling abort");
                                    if let Some(ref writer) = self.rpc_writer {
                                        // Subprocess mode: send abort via RPC
                                        let writer = writer.clone();
                                        tokio::spawn(async move {
                                            let _ = writer.send_abort().await;
                                        });
                                    } else if let Some(ref tx) = self.interrupt_tx {
                                        // In-process mode: signal via channel
                                        let _ = tx.send(true);
                                    }
                                    break;
                                }
                                Event::Mouse(mouse) => {
                                    match mouse.kind {
                                        MouseEventKind::ScrollUp => {
                                            let mut state = self.state.lock().unwrap();
                                            let buffer = if state.wave_view_active {
                                                state.current_wave_worker_buffer_mut()
                                            } else {
                                                state.current_iteration_mut()
                                            };
                                            if let Some(buffer) = buffer {
                                                for _ in 0..3 {
                                                    buffer.scroll_up();
                                                }
                                            }
                                        }
                                        MouseEventKind::ScrollDown => {
                                            let mut state = self.state.lock().unwrap();
                                            let buffer = if state.wave_view_active {
                                                state.current_wave_worker_buffer_mut()
                                            } else {
                                                state.current_iteration_mut()
                                            };
                                            if let Some(buffer) = buffer {
                                                for _ in 0..3 {
                                                    buffer.scroll_down(viewport_height);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                Event::Paste(text) => {
                                    let mut state = self.state.lock().unwrap();
                                    if state.is_guidance_active() {
                                        state.guidance_input.push_str(&text);
                                    }
                                }
                                Event::Key(key) if key.kind == KeyEventKind::Press => {
                                    // Guidance input mode: intercept all keys
                                    {
                                        let mut state = self.state.lock().unwrap();
                                        if state.is_guidance_active() {
                                            match key.code {
                                                KeyCode::Esc => {
                                                    state.cancel_guidance();
                                                }
                                                KeyCode::Enter => {
                                                    // In subprocess mode, send via RPC
                                                    if let Some(ref writer) = self.rpc_writer {
                                                        let message = state.guidance_input.trim().to_string();
                                                        let mode = state.guidance_mode;
                                                        state.cancel_guidance(); // Clear input
                                                        if !message.is_empty() {
                                                            let writer = writer.clone();
                                                            tokio::spawn(async move {
                                                                let _ = match mode {
                                                                    Some(crate::state::GuidanceMode::Now) => {
                                                                        writer.send_steer(&message).await
                                                                    }
                                                                    _ => {
                                                                        writer.send_guidance(&message).await
                                                                    }
                                                                };
                                                            });
                                                        }
                                                    } else {
                                                        // In-process mode: use existing state method
                                                        state.send_guidance();
                                                    }
                                                }
                                                KeyCode::Backspace => {
                                                    state.guidance_input.pop();
                                                }
                                                KeyCode::Char(c) => {
                                                    state.guidance_input.push(c);
                                                }
                                                _ => {}
                                            }
                                            continue;
                                        }
                                    }

                                    // Dismiss help on any key when help is showing
                                    {
                                        let mut state = self.state.lock().unwrap();
                                        if state.show_help {
                                            state.show_help = false;
                                            continue;
                                        }
                                    }

                                    // Map key to action and dispatch.
                                    //
                                    // Quit is intercepted BEFORE dispatch_action so
                                    // we can notify the backend subprocess via
                                    // notify_backend_quit before breaking the loop.
                                    // Without this, the parent CLI blocks on
                                    // child_wait.await and the user perceives the
                                    // exit as "stuck" while the backend keeps
                                    // running orphaned. Symmetric with the Ctrl+C
                                    // handler above (app.rs:238-254).
                                    let action = map_key(key);
                                    if action == Action::Quit {
                                        info!(
                                            target: "ralph_tui::app",
                                            our_pid = std::process::id(),
                                            has_rpc_writer = self.rpc_writer.is_some(),
                                            has_interrupt_tx = self.interrupt_tx.is_some(),
                                            "Action::Quit intercepted; notifying backend before breaking"
                                        );
                                        if let Some(ref writer) = self.rpc_writer {
                                            notify_backend_quit(
                                                Some(writer),
                                                None,
                                            );
                                        } else if let Some(ref tx) = self.interrupt_tx {
                                            notify_backend_quit::<
                                                tokio::process::ChildStdin,
                                            >(
                                                None,
                                                Some(tx),
                                            );
                                        }
                                        break;
                                    }
                                    let mut state = self.state.lock().unwrap();
                                    let mouse_capture_enabled_before = state.mouse_capture_enabled;
                                    if dispatch_action(action, &mut state, viewport_height) {
                                        break;
                                    }
                                    let mouse_capture_enabled_after = state.mouse_capture_enabled;
                                    drop(state);
                                    if mouse_capture_enabled_before != mouse_capture_enabled_after {
                                        set_mouse_capture(mouse_capture_enabled_after)?;
                                    }
                                }
                                // Ignore other events (FocusGained, FocusLost, Paste, Resize, key releases)
                                _ => {}
                            }
                        }
                        Some(Err(e)) => {
                            // Log error but continue - transient errors shouldn't crash TUI
                            tracing::warn!("Event stream error: {}", e);
                        }
                        None => {
                            // Stream ended unexpectedly
                            break;
                        }
                    }
                }

                // Priority 2: Render at throttled rate (~60fps)
                _ = render_tick.tick() => {
                    let frame_size = terminal.size()?;
                    let frame_area = ratatui::layout::Rect::new(0, 0, frame_size.width, frame_size.height);
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(2),  // Header: content + bottom border
                            Constraint::Min(0),     // Content: flexible
                            Constraint::Length(2),  // Footer: top border + content
                        ])
                        .split(frame_area);

                    let content_area = chunks[1];
                    viewport_height = content_area.height as usize;

                    let mut state = self.state.lock().unwrap();

                    // Clear expired flash messages (e.g., guidance send confirmation)
                    state.clear_expired_guidance_flash();

                    // Autoscroll: if user hasn't scrolled away, keep them at the bottom
                    // as new content arrives. This mimics standard terminal behavior.
                    if state.wave_view_active {
                        if let Some(buffer) = state.current_wave_worker_buffer_mut()
                            && buffer.following_bottom
                        {
                            let max_scroll = buffer.line_count().saturating_sub(viewport_height);
                            buffer.scroll_offset = max_scroll;
                        }
                    } else if let Some(buffer) = state.current_iteration_mut()
                        && buffer.following_bottom
                    {
                        let max_scroll = buffer.line_count().saturating_sub(viewport_height);
                        buffer.scroll_offset = max_scroll;
                    }

                    let state = state; // Rebind as immutable for rendering
                    terminal.draw(|f| {
                        // Render header
                        f.render_widget(header::render(&state, chunks[0].width), chunks[0]);

                        // Render content: wave worker buffer when in wave view, else iteration
                        let content_buffer = if state.wave_view_active {
                            state.current_wave_worker_buffer()
                        } else {
                            state.current_iteration()
                        };
                        if let Some(buffer) = content_buffer {
                            let mut content_widget = ContentPane::new(buffer);
                            if let Some(query) = &state.search_state.query {
                                content_widget = content_widget.with_search(query);
                            }
                            f.render_widget(content_widget, content_area);
                        }

                        // Render footer
                        f.render_widget(footer::render(&state), chunks[2]);

                        // Render help overlay if active
                        if state.show_help {
                            help::render(f, f.area());
                        }
                    })?;
                }

                // Priority 3: Handle termination signal
                _ = self.terminated_rx.changed() => {
                    if *self.terminated_rx.borrow() {
                        break;
                    }
                }
            }
        }

        // NOTE: Explicit cleanup removed - now handled by defer! guard above.
        // The guard ensures cleanup happens even on task abort or panic.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Action, map_key};
    use crate::state::TuiState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::text::Line;

    // =========================================================================
    // AC1: Events Reach State — TuiStreamHandler → IterationBuffer
    // =========================================================================

    #[test]
    fn dispatch_action_scroll_down_calls_scroll_down_on_current_buffer() {
        // Given TuiState with an iteration buffer containing content
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for i in 0..20 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }
        let initial_offset = state.current_iteration().unwrap().scroll_offset;
        assert_eq!(initial_offset, 0);

        // When dispatch_action with ScrollDown and viewport_height 10
        dispatch_action(Action::ScrollDown, &mut state, 10);

        // Then scroll_offset is incremented
        assert_eq!(
            state.current_iteration().unwrap().scroll_offset,
            1,
            "scroll_down should increment scroll_offset"
        );
    }

    // =========================================================================
    // AC2: Keyboard Triggers Actions — 'j' → scroll_down()
    // =========================================================================

    #[test]
    fn j_key_triggers_scroll_down_action() {
        // Given key press 'j'
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);

        // When map_key is called
        let action = map_key(key);

        // Then Action::ScrollDown is returned
        assert_eq!(action, Action::ScrollDown);
    }

    #[test]
    fn dispatch_action_scroll_up_calls_scroll_up_on_current_buffer() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for i in 0..20 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }
        // Set initial scroll offset to 5
        state.current_iteration_mut().unwrap().scroll_offset = 5;

        dispatch_action(Action::ScrollUp, &mut state, 10);

        assert_eq!(
            state.current_iteration().unwrap().scroll_offset,
            4,
            "scroll_up should decrement scroll_offset"
        );
    }

    #[test]
    fn dispatch_action_scroll_top_jumps_to_top() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for _ in 0..20 {
            buffer.append_line(Line::from("line"));
        }
        state.current_iteration_mut().unwrap().scroll_offset = 10;

        dispatch_action(Action::ScrollTop, &mut state, 10);

        assert_eq!(state.current_iteration().unwrap().scroll_offset, 0);
    }

    #[test]
    fn dispatch_action_scroll_bottom_jumps_to_bottom() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for _ in 0..20 {
            buffer.append_line(Line::from("line"));
        }

        dispatch_action(Action::ScrollBottom, &mut state, 10);

        // max_scroll = 20 - 10 = 10
        assert_eq!(state.current_iteration().unwrap().scroll_offset, 10);
    }

    #[test]
    fn dispatch_action_next_iteration_navigates_forward() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 0;
        state.following_latest = false;

        dispatch_action(Action::NextIteration, &mut state, 10);

        assert_eq!(state.current_view, 1);
    }

    #[test]
    fn dispatch_action_prev_iteration_navigates_backward() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 2;

        dispatch_action(Action::PrevIteration, &mut state, 10);

        assert_eq!(state.current_view, 1);
    }

    #[test]
    fn dispatch_action_show_help_sets_show_help() {
        let mut state = TuiState::new();
        assert!(!state.show_help);

        dispatch_action(Action::ShowHelp, &mut state, 10);

        assert!(state.show_help);
    }

    #[test]
    fn dispatch_action_dismiss_help_clears_show_help() {
        let mut state = TuiState::new();
        state.show_help = true;

        dispatch_action(Action::DismissHelp, &mut state, 10);

        assert!(!state.show_help);
    }

    #[test]
    fn dispatch_action_search_next_calls_next_match() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("find me"));
        buffer.append_line(Line::from("find me again"));
        state.search("find");
        assert_eq!(state.search_state.current_match, 0);

        dispatch_action(Action::SearchNext, &mut state, 10);

        assert_eq!(state.search_state.current_match, 1);
    }

    #[test]
    fn dispatch_action_search_prev_calls_prev_match() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("find me"));
        buffer.append_line(Line::from("find me again"));
        state.search("find");
        state.search_state.current_match = 1;

        dispatch_action(Action::SearchPrev, &mut state, 10);

        assert_eq!(state.search_state.current_match, 0);
    }

    // =========================================================================
    // AC5: Quit Signals Backend Abort (not just local exit)
    // =========================================================================
    //
    // Regression test for: pressing 'q' in the TUI exited the TUI render loop
    // but did NOT notify the backend subprocess. The parent CLI would then
    // block on child_wait.await forever (run.rs:1749), making the user
    // perceive the exit as "stuck" while the backend kept running orphaned.
    //
    // The fix moves Quit handling out of dispatch_action (which only has
    // state-level access) and into the App::run input loop, symmetric with
    // the Ctrl+C handler. The abort-sending logic is extracted into a small
    // helper `notify_backend_quit` so it can be tested in isolation.

    #[tokio::test]
    async fn notify_backend_quit_sends_abort_via_rpc_writer() {
        // Given an RPC writer bound to an in-memory duplex stream
        let (client, mut server) = tokio::io::duplex(64);
        let writer = RpcWriter::new(client);

        // When notify_backend_quit is called with the writer
        notify_backend_quit(Some(&writer), None);

        // Then the backend (server side) receives a well-formed Abort command.
        //
        // Allow the spawned send task to make progress.
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(writer); // close our end so server read can finish

        let mut buf = Vec::new();
        use tokio::io::AsyncReadExt;
        let _ =
            tokio::time::timeout(Duration::from_millis(200), server.read_to_end(&mut buf)).await;

        let line = std::str::from_utf8(&buf)
            .expect("rpc writer should send utf-8 JSON")
            .trim();
        assert!(
            !line.is_empty(),
            "rpc writer should have written an abort line, got empty buffer",
        );
        let cmd: serde_json::Value =
            serde_json::from_str(line).expect("rpc writer should send a JSON line");
        assert_eq!(
            cmd.get("type").and_then(|v| v.as_str()),
            Some("abort"),
            "Quit must signal the backend via the abort RPC command, not silently exit",
        );
    }

    #[tokio::test]
    async fn notify_backend_quit_signals_interrupt_channel_when_no_rpc() {
        // Given an interrupt watch channel (in-process mode)
        let (tx, mut rx) = watch::channel(false);

        // When notify_backend_quit is called with no rpc_writer
        notify_backend_quit::<tokio::io::DuplexStream>(None, Some(&tx));

        // Then the channel flips to true
        assert!(
            *rx.borrow_and_update(),
            "Quit must flip interrupt_tx to true so the main loop can abort",
        );
    }

    #[test]
    fn notify_backend_quit_noop_when_both_none() {
        // Should not panic when no backend channel exists (e.g. test harness).
        notify_backend_quit::<tokio::io::DuplexStream>(None, None);
    }

    #[test]
    fn dispatch_action_quit_is_no_longer_handled_by_dispatcher() {
        // Quit is intentionally NOT in dispatch_action: the App::run loop
        // intercepts Action::Quit before dispatch so it can notify the
        // backend via notify_backend_quit. If Quit were handled here, the
        // loop would break without sending abort and the parent would
        // hang on child_wait (the original bug).
        let mut state = TuiState::new();
        let should_quit = dispatch_action(Action::Quit, &mut state, 10);
        assert!(
            !should_quit,
            "dispatch_action must NOT exit on Quit; the loop owns Quit so it can call notify_backend_quit first",
        );
    }

    #[test]
    fn dispatch_action_non_quit_returns_false() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("line"));

        let should_quit = dispatch_action(Action::ScrollDown, &mut state, 10);
        assert!(!should_quit, "Non-quit actions should return false");
    }

    #[test]
    fn dispatch_action_toggle_mouse_mode_flips_state() {
        let mut state = TuiState::new();
        assert!(!state.mouse_capture_enabled);

        dispatch_action(Action::ToggleMouseMode, &mut state, 10);
        assert!(state.mouse_capture_enabled);

        dispatch_action(Action::ToggleMouseMode, &mut state, 10);
        assert!(!state.mouse_capture_enabled);
    }

    // =========================================================================
    // AC6: No PTY Code — Structural Test
    // =========================================================================

    #[test]
    fn no_pty_handle_in_app() {
        let source = include_str!("app.rs");
        let test_module_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production_code = &source[..test_module_start];

        // Check for PTY-related imports/code
        assert!(
            !production_code.contains("PtyHandle"),
            "app.rs should not contain PtyHandle after refactor"
        );
        assert!(
            !production_code.contains("tui_term"),
            "app.rs should not contain tui_term references after refactor"
        );
        assert!(
            !production_code.contains("TerminalWidget"),
            "app.rs should not contain TerminalWidget after refactor"
        );
    }

    /// Regression test: TUI must NOT have tokio::signal::ctrl_c() handler.
    ///
    /// Raw mode prevents SIGINT, so tokio's signal handler never fires.
    /// TUI must detect Ctrl+C directly via crossterm events.
    #[test]
    fn no_tokio_signal_handler_in_app() {
        let source = include_str!("app.rs");
        let pattern = ["tokio", "::", "signal", "::", "ctrl_c", "()"].concat();
        let test_module_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production_code = &source[..test_module_start];
        let occurrences: Vec<_> = production_code.match_indices(&pattern).collect();
        assert!(
            occurrences.is_empty(),
            "Found {} occurrence(s) of tokio::signal::ctrl_c() in production code. \
             This doesn't work in raw mode - use crossterm events instead.",
            occurrences.len()
        );
    }

    /// Verify Ctrl+C handling exists in production code.
    ///
    /// Since raw mode prevents SIGINT, we must handle Ctrl+C via crossterm events.
    /// TUI is observation-only, so Ctrl+C breaks out of the event loop.
    #[test]
    fn ctrl_c_handling_exists_in_app() {
        let source = include_str!("app.rs");
        let test_module_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production_code = &source[..test_module_start];

        assert!(
            production_code.contains("KeyCode::Char('c')")
                && production_code.contains("KeyModifiers::CONTROL"),
            "Production code must detect Ctrl+C via crossterm events"
        );
    }

    #[test]
    fn mouse_capture_starts_disabled_by_default() {
        assert!(
            !TuiState::new().mouse_capture_enabled,
            "Production TUI should start with mouse capture disabled so native text selection works by default"
        );
    }
}
