//! Terminal ownership and cleanup for the interactive interface.
use std::{
    env,
    io::{self, IsTerminal, Stdout, Write},
    panic,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use hs::{Error, Result};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};

/// Call before opening a database, so a pipe or unsupported terminal has no
/// data side effects and never receives terminal-control escape sequences.
pub(super) fn validate() -> Result<()> {
    validate_environment(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        env::var("TERM").ok().as_deref(),
    )
}

fn validate_environment(input: bool, output: bool, term: Option<&str>) -> Result<()> {
    if !input || !output {
        return Err(Error::Input(
            "`hs tui` needs an interactive terminal for both input and output. Use `hs matrix` for plain output.".into(),
        ));
    }
    if term == Some("dumb") {
        return Err(Error::Input(
            "`hs tui` needs a terminal with cursor support; TERM=dumb is not supported. Use `hs matrix` for plain output.".into(),
        ));
    }
    Ok(())
}

pub(super) struct TerminalGuard {
    // Drop the renderer before returning from the alternate screen.
    terminal: Terminal<CrosstermBackend<Stdout>>,
    _screen: ScreenGuard,
    signals: TerminationSignals,
}

impl TerminalGuard {
    pub(super) fn enter() -> Result<Self> {
        validate()?;
        let signals = TerminationSignals::register()?;
        enable_raw_mode()?;
        // Establish cleanup immediately after changing terminal modes. Every
        // fallible operation below is protected, including initial drawing.
        let screen = ScreenGuard::new();
        let mut stdout = io::stdout();
        // Clear before constructing Ratatui's empty buffers. Terminal::clear()
        // preserves the cursor by querying it, which can time out in otherwise
        // usable PTYs; fullscreen initialization has no cursor to preserve.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        )?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            _screen: screen,
            signals,
        })
    }

    pub(super) fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> Result<()> {
        self.terminal.draw(render)?;
        Ok(())
    }

    /// Ctrl-C typed in raw mode is a key event. This flag also covers actual
    /// process signals, letting the event loop exit through ordinary cleanup.
    pub(super) fn interrupted(&self) -> bool {
        self.signals.pending.load(Ordering::Relaxed)
    }
}

type PanicHook = dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static;

struct ScreenGuard {
    active: Arc<AtomicBool>,
    previous_hook: Arc<PanicHook>,
}

impl ScreenGuard {
    fn new() -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let previous_hook: Arc<PanicHook> = Arc::from(panic::take_hook());
        let panic_active = active.clone();
        let panic_previous = previous_hook.clone();
        panic::set_hook(Box::new(move |info| {
            if panic_active.swap(false, Ordering::AcqRel) {
                restore();
            }
            panic_previous(info);
        }));
        Self {
            active,
            previous_hook,
        }
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        if self.active.swap(false, Ordering::AcqRel) {
            restore();
        }
        // Updating hooks while unwinding itself panics. The installed hook
        // remains harmless in that case because its active flag is now false.
        if !std::thread::panicking() {
            let previous = self.previous_hook.clone();
            panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

fn restore() {
    let mut stdout = io::stdout();
    // Attempt every mode reset even if the output device has disappeared.
    let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen, Show);
    let _ = stdout.flush();
    let _ = disable_raw_mode();
}

struct TerminationSignals {
    pending: Arc<AtomicBool>,
    #[cfg(unix)]
    registrations: Vec<signal_hook::SigId>,
}

impl TerminationSignals {
    fn register() -> Result<Self> {
        let result = Self {
            pending: Arc::new(AtomicBool::new(false)),
            #[cfg(unix)]
            registrations: Vec::new(),
        };
        #[cfg(unix)]
        {
            use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
            let mut result = result;
            for signal in [SIGINT, SIGTERM, SIGHUP, SIGQUIT] {
                result
                    .registrations
                    .push(signal_hook::flag::register(signal, result.pending.clone())?);
            }
            Ok(result)
        }
        #[cfg(not(unix))]
        Ok(result)
    }
}

#[cfg(unix)]
impl Drop for TerminationSignals {
    fn drop(&mut self) {
        for id in self.registrations.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_terminal_modes_fail_before_terminal_setup() {
        for (input, output, term) in [
            (false, true, Some("xterm")),
            (true, false, Some("xterm")),
            (false, false, None),
            (true, true, Some("dumb")),
        ] {
            assert!(validate_environment(input, output, term).is_err());
        }
        assert!(validate_environment(true, true, Some("xterm-256color")).is_ok());
        assert!(validate_environment(true, true, None).is_ok());
    }
}
