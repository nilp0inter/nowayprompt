//! Frontend abstraction: trait + shared types for poll-based dispatch.
//!
//! 100% behavioral parity with `legacy/src/Frontend.zig`. The TTY fallback
//! (Stage 2) and Wayland frontend (Stage 3) both implement this trait.

use std::io;
use std::os::fd::RawFd;

use crate::config::Config;

/// Frontend event resulting from user interaction.
///
/// Parity with `legacy/src/Frontend.zig` `Event` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// No event yet (inner sentinel; the dispatch loop treats `None`-as-event
    /// as a no-op, matching legacy `.none`).
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
/// Parity with `legacy/src/Frontend.zig` `InterfaceMode` enum. Note that the
/// Assuan-level `AssuanMode` distinguishes `Confirm` from `Message`, but both
/// map to the frontend `Message` mode (legacy `Frontend.enterMode(.message)`
/// for both `confirm()` and `message()`).
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
    /// `init` failed (e.g. no `tty_name` set, or open failed).
    Init(String),
    /// I/O error from a frontend read/write.
    Io(io::Error),
    /// `enter_mode` called with an invalid mode transition.
    InvalidMode(String),
}

impl std::fmt::Display for FrontendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
///
/// Parity with `legacy/src/Frontend.zig` method set.
pub trait Frontend {
    /// Initialize the frontend, returning the fd to poll alongside stdin.
    /// Stores runtime config state (e.g. `tty_name`) from `cfg`.
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>;

    /// Restore the terminal to a cooked state and release resources.
    fn deinit(&mut self);

    /// Enter a prompt mode (`GetPin`/`Message`) or leave it (`None`).
    /// Entering a mode asserts the current mode is `None`; leaving asserts
    /// the current mode is non-`None` (parity with legacy `debug.assert`).
    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>;

    /// Block until a frontend event is available, then return it.
    /// Returns `Event::None` if no terminal event occurred (e.g. only
    /// intermediate keypresses were processed).
    fn handle_event(&mut self) -> Result<Event, FrontendError>;

    /// Non-blocking check for pending events. Returns `Ok(None)` if no event
    /// is ready. The blocking TTY frontend always returns `Ok(None)`.
    fn flush(&mut self) -> Result<Option<Event>, FrontendError>;

    /// Called when the frontend fd was NOT readable in the last poll.
    /// No-op for the blocking TTY frontend (parity with legacy `noEvent`).
    fn no_event(&mut self) -> Result<(), FrontendError>;
}

pub mod tty;

/// Re-export the TTY frontend for convenient use.
pub use tty::Tty;
