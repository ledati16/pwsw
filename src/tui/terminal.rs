//! Terminal session: raw mode + alternate screen + bracketed paste, restored
//! exactly once.
//!
//! [`TerminalSession::enter`] installs a panic hook *before* touching the
//! terminal and creates the guard that undoes everything, so a failure half-way
//! through initialisation, a panic anywhere (including a spawned task or a
//! blocking thread) and a normal return all restore the terminal. The hook is
//! uninstalled again when the session ends.
//!
//! A panic off the main task restores the terminal from the hook while the event
//! loop is still running; [`panic_seen`] lets the loop notice and stop instead of
//! drawing over the restored screen.

use std::io::stdout;
use std::panic::PanicHookInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal};

type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static;

/// Whether the terminal is in TUI mode and still needs restoring.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set by the panic hook; the event loop exits when it sees it.
static PANIC_SEEN: AtomicBool = AtomicBool::new(false);

/// Leave raw mode, bracketed paste and the alternate screen if (and only if)
/// they are active.
fn restore_once() {
    if TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), DisableBracketedPaste);
        ratatui::restore();
        let _ = execute!(stdout(), Show);
    }
}

/// Whether any thread panicked while the session was active.
pub(crate) fn panic_seen() -> bool {
    PANIC_SEEN.load(Ordering::SeqCst)
}

/// Restores the terminal and the previous panic hook when dropped.
struct RestoreGuard {
    previous_hook: Arc<PanicHook>,
}

impl RestoreGuard {
    /// Chain a panic hook that restores the terminal before the previous hook
    /// prints its report.
    fn install() -> Self {
        let previous_hook: Arc<PanicHook> = Arc::from(std::panic::take_hook());
        let chained = Arc::clone(&previous_hook);
        std::panic::set_hook(Box::new(move |info| {
            PANIC_SEEN.store(true, Ordering::SeqCst);
            restore_once();
            chained(info);
        }));
        Self { previous_hook }
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore_once();
        // `set_hook` panics on a panicking thread; the process is unwinding
        // anyway then, and the chained hook stays harmless.
        if !std::thread::panicking() {
            let previous = Arc::clone(&self.previous_hook);
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

/// Owns the TUI terminal; dropping it restores the terminal and the previous
/// panic hook.
pub(crate) struct TerminalSession {
    // Field order matters: the terminal is dropped before the guard restores.
    pub(crate) terminal: DefaultTerminal,
    _guard: RestoreGuard,
}

impl TerminalSession {
    /// Enter raw mode and the alternate screen, and enable bracketed paste (a
    /// paste arrives as one `Event::Paste` instead of a stream of key presses).
    ///
    /// # Errors
    /// Returns an error if raw mode, the alternate screen or the terminal backend
    /// cannot be set up (anything already changed is restored).
    pub(crate) fn enter() -> Result<Self> {
        // The guard exists before the terminal is touched.
        let guard = RestoreGuard::install();
        TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        enable_raw_mode().context("Failed to enable raw mode")?;
        execute!(stdout(), EnterAlternateScreen).context("Failed to enter alternate screen")?;
        execute!(stdout(), EnableBracketedPaste).context("Failed to enable bracketed paste")?;
        let terminal =
            Terminal::new(CrosstermBackend::new(stdout())).context("Failed to create terminal")?;
        Ok(Self {
            terminal,
            _guard: guard,
        })
    }
}
