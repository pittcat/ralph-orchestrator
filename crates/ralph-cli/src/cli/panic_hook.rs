//! Terminal-state-restore panic hook for the TUI panic path.
//!
//! When a TUI application panics, the terminal can be left in a broken
//! state: raw mode enabled, alternate screen buffer active, cursor hidden.
//! This hook ensures the terminal is restored so the panic message is
//! visible and the user can scroll/interact normally.

/// Installs a panic hook that restores terminal state before printing panic info.
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Restore terminal state before printing panic info
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        // Call the default panic hook to print the panic message
        default_hook(panic_info);
    }));
}
