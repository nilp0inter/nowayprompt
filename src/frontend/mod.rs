//! Frontend abstraction: trait + shared types for poll-based dispatch.
//!
//! The TTY fallback and Wayland frontend both implement this trait.

use std::io;
use std::os::fd::RawFd;

use crate::config::Config;

/// Frontend event resulting from user interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// No event yet (inner sentinel; the dispatch loop treats `None`-as-event
    /// as a no-op).
    None,
    /// User confirmed (pressed Enter / clicked OK).
    UserOk,
    /// User aborted (pressed Escape).
    UserAbort,
    /// User pressed the not-OK button (Ctrl-C when `not_ok` is set).
    UserNotOk,
}

/// Which interface mode the frontend is currently rendering.
///
/// The Assuan-level `AssuanMode` distinguishes `Confirm` from `Message`, but
/// both map to the frontend `Message` mode (`confirm()` and `message()` both
/// enter `Message`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceMode {
    /// No prompt displayed (cooked terminal).
    None,
    /// PIN entry prompt.
    GetPin,
    /// Message or confirm dialog.
    Message,
}

/// Error returned by frontend operations.
#[derive(Debug)]
pub enum FrontendError {
    /// No Wayland display was configured or the display socket was unreachable.
    /// This is the only Wayland failure that may select the TTY fallback.
    Unavailable(String),
    /// `init` failed after a frontend was selected.
    Init(String),
    /// I/O error from a frontend read/write.
    Io(io::Error),
    /// `enter_mode` called with an invalid mode transition.
    InvalidMode(String),
}

impl std::fmt::Display for FrontendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(msg) => write!(f, "frontend unavailable: {msg}"),
            Self::Init(msg) => write!(f, "frontend init error: {msg}"),
            Self::Io(e) => write!(f, "frontend I/O error: {e}"),
            Self::InvalidMode(msg) => write!(f, "frontend invalid mode: {msg}"),
        }
    }
}

impl std::error::Error for FrontendError {}

impl From<io::Error> for FrontendError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Poll-based frontend contract.
///
/// The dispatch loop in `main.rs` calls `flush` at the top of each iteration,
/// polls stdin + the fd returned by `init`, then calls `handle_event` or
/// `no_event` depending on whether the frontend fd is readable.
pub trait Frontend {
    /// Initialize the frontend, returning the fd to poll alongside stdin.
    /// Stores runtime config state (e.g. `tty_name`) from `cfg`.
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>;

    /// Restore the terminal to a cooked state and release resources.
    fn deinit(&mut self);

    /// Enter a prompt mode (`GetPin`/`Message`) or leave it (`None`).
    /// Entering a mode asserts the current mode is `None`; leaving asserts
    /// the current mode is non-`None`.
    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>;

    /// Block until a frontend event is available, then return it.
    /// Returns `Event::None` if no terminal event occurred (e.g. only
    /// intermediate keypresses were processed).
    fn handle_event(&mut self) -> Result<Event, FrontendError>;

    /// Non-blocking check for pending events. Returns `Ok(None)` if no event
    /// is ready. The blocking TTY frontend always returns `Ok(None)`.
    fn flush(&mut self) -> Result<Option<Event>, FrontendError>;

    /// Called when the frontend fd was NOT readable in the last poll.
    /// No-op for the blocking TTY frontend.
    fn no_event(&mut self) -> Result<(), FrontendError>;
}

pub mod tty;
pub mod wayland;

use crate::secret::SecretBuffer;

/// Concrete production frontend selected for one prompt session.
///
/// Wayland state is heap-allocated once to keep the enum compact.
pub enum FrontendOwner {
    Wayland(Box<Wayland>),
    Tty(Tty),
}

impl FrontendOwner {
    /// Select Wayland first, falling back only for an unavailable display.
    pub fn select(
        cfg: &mut Config,
        secbuf: &mut SecretBuffer,
    ) -> Result<(Self, RawFd), FrontendError> {
        let mut wayland = Wayland::new();
        wayland.set_secret_buffer(secbuf);

        match wayland.init(cfg) {
            Ok(fd) => Ok((Self::Wayland(Box::new(wayland)), fd)),
            Err(error) if should_fallback(cfg.allow_tty_fallback, &error) => {
                let mut tty = Tty::new();
                tty.set_secret_buffer(secbuf);
                let fd = tty.init(cfg).map_err(|error| match error {
                    FrontendError::Unavailable(message) => FrontendError::Init(message),
                    other => other,
                })?;
                Ok((Self::Tty(tty), fd))
            }
            Err(error) => Err(error),
        }
    }
}

fn should_fallback(allow_tty_fallback: bool, error: &FrontendError) -> bool {
    allow_tty_fallback && matches!(error, FrontendError::Unavailable(_))
}

#[cfg(test)]
mod tests {
    use super::{should_fallback, FrontendError};

    #[test]
    fn only_unavailable_wayland_may_fall_back() {
        assert!(should_fallback(
            true,
            &FrontendError::Unavailable("no display".into())
        ));
        assert!(!should_fallback(
            false,
            &FrontendError::Unavailable("no display".into())
        ));
        assert!(!should_fallback(
            true,
            &FrontendError::Init("missing global".into())
        ));
    }
}

impl Frontend for FrontendOwner {
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError> {
        match self {
            Self::Wayland(frontend) => frontend.init(cfg),
            Self::Tty(frontend) => frontend.init(cfg),
        }
    }

    fn deinit(&mut self) {
        match self {
            Self::Wayland(frontend) => frontend.deinit(),
            Self::Tty(frontend) => frontend.deinit(),
        }
    }

    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError> {
        match self {
            Self::Wayland(frontend) => frontend.enter_mode(mode),
            Self::Tty(frontend) => frontend.enter_mode(mode),
        }
    }

    fn handle_event(&mut self) -> Result<Event, FrontendError> {
        match self {
            Self::Wayland(frontend) => frontend.handle_event(),
            Self::Tty(frontend) => frontend.handle_event(),
        }
    }

    fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
        match self {
            Self::Wayland(frontend) => frontend.flush(),
            Self::Tty(frontend) => frontend.flush(),
        }
    }

    fn no_event(&mut self) -> Result<(), FrontendError> {
        match self {
            Self::Wayland(frontend) => frontend.no_event(),
            Self::Tty(frontend) => frontend.no_event(),
        }
    }
}

/// Re-export the TTY frontend for convenient use.
pub use tty::Tty;

/// Re-export the Wayland frontend.
pub use wayland::Wayland;
