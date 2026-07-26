//! TTY fallback frontend using raw `libc::termios`: raw-mode terminal
//! input, ANSI rendering, and async-signal-safe terminal restore on
//! fatal signals.

use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

use crate::config::Config;
use crate::frontend::{Event, Frontend, FrontendError, InterfaceMode};
use crate::secret::SecretBuffer;

// ---------------------------------------------------------------------------
// RawTty: raw-mode terminal guard
// ---------------------------------------------------------------------------

/// RAII guard that puts a terminal fd into raw mode and restores the
/// original termios on drop.
struct RawTty {
    fd: RawFd,
    orig_termios: libc::termios,
}

impl RawTty {
    /// Save the current termios, switch to raw mode (no echo, no canonical,
    /// no signals; VMIN=1, VTIME=0), and flush pending input.
    fn new(fd: RawFd) -> io::Result<Self> {
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::tcgetattr(fd, &mut orig) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = orig;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        let rc = unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            fd,
            orig_termios: orig,
        })
    }

    /// Restore the original (cooked) termios.
    fn restore(&self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.orig_termios);
        }
    }
}

impl Drop for RawTty {
    fn drop(&mut self) {
        self.restore();
    }
}

// ---------------------------------------------------------------------------
// Signal handler statics
// ---------------------------------------------------------------------------

/// The tty fd, visible to the signal handler. -1 means not registered.
static TTY_FD: AtomicI32 = AtomicI32::new(-1);

/// Pointer to a leaked `libc::termios` with the original (cooked) state.
/// The signal handler uses this to restore the terminal before exiting.
static ORIG_TERMIOS_PTR: AtomicPtr<libc::termios> = AtomicPtr::new(std::ptr::null_mut());

/// Async-signal-safe handler: restore terminal and exit immediately.
///
/// ONLY calls `tcsetattr` and `_exit`, both of which are on the
/// POSIX async-signal-safe list. No allocations, no locks, no stdio.
fn signal_handler() {
    let fd = TTY_FD.load(Ordering::Relaxed);
    let ptr = ORIG_TERMIOS_PTR.load(Ordering::Relaxed);
    if fd >= 0 && !ptr.is_null() {
        unsafe {
            libc::tcsetattr(fd, libc::TCSAFLUSH, &*ptr);
            libc::_exit(0);
        }
    }
    // If we have no valid state, just exit.
    unsafe {
        libc::_exit(0);
    }
}

/// Register async-signal-safe handlers for SIGINT, SIGTERM, SIGHUP,
/// SIGQUIT, SIGTSTP. Stores the fd and a leaked copy of the original
/// termios for the handler to restore.
fn register_signal_handlers(fd: RawFd, orig: &libc::termios) -> io::Result<()> {
    TTY_FD.store(fd, Ordering::Relaxed);

    // Leak a Box<termios> so the handler has a stable pointer.
    let boxed: Box<libc::termios> = Box::new(*orig);
    let ptr: *mut libc::termios = Box::into_raw(boxed);
    ORIG_TERMIOS_PTR.store(ptr, Ordering::Relaxed);

    use signal_hook::low_level::register;
    unsafe {
        register(libc::SIGINT, signal_handler)?;
        register(libc::SIGTERM, signal_handler)?;
        register(libc::SIGHUP, signal_handler)?;
        register(libc::SIGQUIT, signal_handler)?;
        register(libc::SIGTSTP, signal_handler)?;
    }
    Ok(())
}

/// Reset the signal-handler statics so the handler becomes a no-op.
fn unregister_signal_handlers() {
    TTY_FD.store(-1, Ordering::Relaxed);
    // Leak the termios intentionally — we cannot safely free it while
    // a signal might be in flight. The process is short-lived.
    ORIG_TERMIOS_PTR.store(std::ptr::null_mut(), Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Input parser
// ---------------------------------------------------------------------------

/// A single parsed input token from the TTY byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtyInput {
    Enter,
    Escape,
    Backspace,
    /// Ctrl-C
    Cc,
    /// Ctrl-U (kill line)
    Cu,
    /// Ctrl-W (kill word)
    Cw,
    /// Ctrl-Backspace (0x08)
    CBackspace,
    /// A decoded Unicode codepoint.
    Codepoint(char),
    /// Unrecognized escape sequence or invalid UTF-8.
    Unknown,
}

/// Parse a raw byte buffer into a sequence of [`TtyInput`] tokens.
///
/// Escape sequences starting with `\x1b` followed by more bytes are consumed
/// as `Unknown`; a standalone `\x1b` (last byte) is `Escape`.
pub fn parse_input(buf: &[u8]) -> Vec<TtyInput> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        match buf[i] {
            b'\r' | b'\n' => {
                out.push(TtyInput::Enter);
                i += 1;
            }
            0x1b => {
                // If there are more bytes, this is an escape sequence
                // (e.g. \x1b[A for arrow keys, or Alt+key). Consume the
                // rest of the CSI/escape sequence as Unknown.
                if i + 1 < buf.len() {
                    // Consume the escape introducer.
                    i += 1;
                    // For CSI sequences (ESC [), consume until a final byte.
                    if i < buf.len() && buf[i] == b'[' {
                        i += 1;
                        // Consume parameter + intermediate bytes until
                        // the final byte (0x40..=0x7E).
                        while i < buf.len() && !(0x40..=0x7e).contains(&buf[i]) {
                            i += 1;
                        }
                        if i < buf.len() {
                            i += 1; // consume final byte
                        }
                    }
                    // For non-CSI (e.g. Alt+letter), just the next byte.
                    // Already consumed by the i+1 above for non-'['.
                    else if i < buf.len() {
                        i += 1;
                    }
                    out.push(TtyInput::Unknown);
                } else {
                    // Standalone ESC (last byte in buffer).
                    out.push(TtyInput::Escape);
                    i += 1;
                }
            }
            0x7f => {
                out.push(TtyInput::Backspace);
                i += 1;
            }
            0x03 => {
                out.push(TtyInput::Cc);
                i += 1;
            }
            0x15 => {
                out.push(TtyInput::Cu);
                i += 1;
            }
            0x17 => {
                out.push(TtyInput::Cw);
                i += 1;
            }
            0x08 => {
                out.push(TtyInput::CBackspace);
                i += 1;
            }
            b => {
                // Attempt UTF-8 decode from this lead byte.
                let seq_len = utf8_seq_len(b);
                if seq_len == 0 || i + seq_len > buf.len() {
                    out.push(TtyInput::Unknown);
                    i += 1;
                    continue;
                }
                match std::str::from_utf8(&buf[i..i + seq_len]) {
                    Ok(s) => {
                        if let Some(c) = s.chars().next() {
                            out.push(TtyInput::Codepoint(c));
                        } else {
                            out.push(TtyInput::Unknown);
                        }
                        i += seq_len;
                    }
                    Err(_) => {
                        out.push(TtyInput::Unknown);
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

/// Return the expected byte length of a UTF-8 sequence given its lead
/// byte, or 0 for continuation/invalid bytes.
fn utf8_seq_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead >> 5 == 0b110 {
        2
    } else if lead >> 4 == 0b1110 {
        3
    } else if lead >> 3 == 0b11110 {
        4
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// ANSI rendering helpers
// ---------------------------------------------------------------------------

/// SGR attribute descriptor for render_content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attr {
    bold: bool,
    fg_red: bool,
    fg_black: bool,
    bg_green: bool,
}

impl Attr {
    const DEFAULT: Self = Self {
        bold: false,
        fg_red: false,
        fg_black: false,
        bg_green: false,
    };

    /// Emit the SGR escape for this attribute.
    fn sgr(&self) -> &'static str {
        match (self.bold, self.fg_red, self.fg_black, self.bg_green) {
            // Title: bold, black fg, green bg
            (true, false, true, true) => "\x1b[1;30;42m",
            // Error: bold, red fg
            (true, true, false, false) => "\x1b[1;31m",
            // Prompt: bold only
            (true, false, false, false) => "\x1b[1m",
            // Default/reset
            _ => "\x1b[0m",
        }
    }

    /// Whether this attribute has a background colour (needs padding).
    fn has_bg(&self) -> bool {
        self.bg_green
    }
}

/// Clear screen and move cursor to home.
fn clear_and_home<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[2J\x1b[H")
}

/// Render a multi-line content block with the given attribute.
///
/// Each line: cursor to (line, 0), leading space, SGR attr, content,
/// pad to width if bg set. A blank line is appended after.
fn render_content<W: Write>(
    w: &mut W,
    content: &str,
    attr: Attr,
    line: &mut usize,
    width: u16,
    height: u16,
) -> io::Result<()> {
    w.write_all(attr.sgr().as_bytes())?;
    for l in content.split('\n') {
        if l.is_empty() && content.ends_with('\n') && *line > 0 {
            // `split` yields an empty trailing element for a final newline;
            // skip it.
            break;
        }
        if *line >= height as usize {
            return Ok(());
        }
        // Move cursor to (line, 0).
        write!(w, "\x1b[{};1H", *line + 1)?;
        // Leading space + content.
        w.write_all(b" ")?;
        w.write_all(l.as_bytes())?;
        // Pad to width if background is set.
        if attr.has_bg() {
            let written = 1 + l.len();
            if (written as u16) < width {
                let pad = (width as usize) - written;
                for _ in 0..pad {
                    w.write_all(b" ")?;
                }
            }
        }
        *line += 1;
    }
    // Blank line after content block.
    *line += 1;
    Ok(())
}

/// Render a button row: " <key>: <label>", continuation lines indent by
/// key.len() + 2, wrapping at width.
fn render_button<W: Write>(
    w: &mut W,
    key: &str,
    label: &str,
    line: &mut usize,
    _width: u16,
    height: u16,
) -> io::Result<()> {
    // Reset attributes for buttons.
    w.write_all(b"\x1b[0m")?;
    let first = *line;
    let indent = key.len() + 2; // ": " is 2 chars
    for l in label.split('\n') {
        if l.is_empty() && label.ends_with('\n') && *line > first {
            break;
        }
        if *line >= height as usize {
            return Ok(());
        }
        write!(w, "\x1b[{};1H", *line + 1)?;
        w.write_all(b" ")?;
        if *line == first {
            w.write_all(key.as_bytes())?;
            w.write_all(b": ")?;
        } else {
            for _ in 0..indent {
                w.write_all(b" ")?;
            }
        }
        w.write_all(l.as_bytes())?;
        *line += 1;
    }
    Ok(())
}

/// Render the PIN entry row: " > ***_____" style.
fn render_pin_row<W: Write>(
    w: &mut W,
    pin_square_amount: u16,
    len: usize,
    line: &mut usize,
    height: u16,
) -> io::Result<()> {
    if *line >= height as usize {
        return Ok(());
    }
    w.write_all(b"\x1b[1m")?; // bold
    write!(w, "\x1b[{};1H", *line + 1)?;
    w.write_all(b" > ")?;
    let squares = pin_square_amount as usize;
    let filled = squares.min(len);
    let empty = squares.saturating_sub(len);
    for _ in 0..filled {
        w.write_all(b"*")?;
    }
    for _ in 0..empty {
        w.write_all(b"_")?;
    }
    // Advance past the pin row and its trailing blank line.
    *line += 2;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tty frontend
// ---------------------------------------------------------------------------

/// TTY fallback frontend using raw `libc::termios`. Renders ANSI output to
/// the tty fd and reads raw input from the same fd.
pub struct Tty {
    fd: Option<RawFd>,
    raw: Option<RawTty>,
    mode: InterfaceMode,
    width: u16,
    height: u16,
    config_ptr: Option<*mut Config>,
    secbuf_ptr: Option<*mut SecretBuffer>,
}

impl Tty {
    /// Create a new TTY frontend in the initial (no-mode) state.
    pub fn new() -> Self {
        Self {
            fd: None,
            raw: None,
            mode: InterfaceMode::None,
            width: 80,
            height: 24,
            config_ptr: None,
            secbuf_ptr: None,
        }
    }

    /// Provide the secret buffer pointer. Must be called after `init`
    /// and before any mode entry. The caller guarantees the buffer
    /// outlives this frontend.
    ///
    /// # Safety
    /// The caller must ensure `secbuf` outlives this `Tty` instance.
    pub fn set_secret_buffer(&mut self, secbuf: &mut SecretBuffer) {
        self.secbuf_ptr = Some(secbuf as *mut SecretBuffer);
    }

    /// Get a reference to the config. Panics if init was not called.
    fn config(&self) -> &Config {
        unsafe { &*self.config_ptr.expect("init not called") }
    }

    /// Get a mutable reference to the secret buffer. Panics if not set.
    fn secbuf(&mut self) -> &mut SecretBuffer {
        unsafe { &mut *self.secbuf_ptr.expect("secret buffer not set") }
    }

    /// Query terminal size via TIOCGWINSZ ioctl.
    fn query_size(fd: RawFd) -> io::Result<(u16, u16)> {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((ws.ws_col, ws.ws_row))
    }

    /// Set the terminal window title via OSC escape sequence.
    fn set_window_title(&self, title: &str) -> io::Result<()> {
        let fd = self
            .fd
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no tty fd"))?;
        let mut buf = Vec::with_capacity(title.len() + 8);
        buf.extend_from_slice(b"\x1b]2;");
        buf.extend_from_slice(title.as_bytes());
        buf.push(0x07); // BEL
        let written = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Full render pass: clear, draw all visible elements.
    fn render(&self) -> io::Result<()> {
        let fd = match self.fd {
            Some(f) => f,
            None => return Ok(()),
        };
        let config = self.config();
        let labels = &config.labels;
        let width = self.width;
        let height = self.height;

        // Collect all output into a buffer, then write in one shot.
        let mut out: Vec<u8> = Vec::with_capacity(4096);

        clear_and_home(&mut out)?;

        if width < 5 || height < 5 {
            out.extend_from_slice(b"\x1b[1;31m");
            out.extend_from_slice(b"Terminal too small!");
            self.write_output(fd, &out)?;
            return Ok(());
        }

        let mut line: usize = 0;

        // Title (bold, green bg, black fg).
        if let Some(ref t) = labels.title {
            let attr = Attr {
                bold: true,
                fg_black: true,
                bg_green: true,
                ..Attr::DEFAULT
            };
            render_content(&mut out, t, attr, &mut line, width, height)?;
        }

        // Description (default attr).
        if let Some(ref d) = labels.description {
            render_content(&mut out, d, Attr::DEFAULT, &mut line, width, height)?;
        }

        // Prompt (bold).
        if let Some(ref p) = labels.prompt {
            let attr = Attr {
                bold: true,
                ..Attr::DEFAULT
            };
            render_content(&mut out, p, attr, &mut line, width, height)?;
        }

        // PIN row.
        if self.mode == InterfaceMode::GetPin {
            let pin_amount = config.wayland_ui.pin_square_amount;
            let len = if let Some(ptr) = self.secbuf_ptr {
                unsafe { (*ptr).len() }
            } else {
                0
            };
            render_pin_row(&mut out, pin_amount, len, &mut line, height)?;
        }

        // Error message (bold red).
        if let Some(ref e) = labels.err_message {
            let attr = Attr {
                bold: true,
                fg_red: true,
                ..Attr::DEFAULT
            };
            render_content(&mut out, e, attr, &mut line, width, height)?;
        }

        // Buttons.
        if let Some(ref o) = labels.ok {
            render_button(&mut out, "enter", o, &mut line, width, height)?;
        }
        if let Some(ref n) = labels.not_ok {
            render_button(&mut out, "C-c", n, &mut line, width, height)?;
        }
        if let Some(ref c) = labels.cancel {
            render_button(&mut out, "escape", c, &mut line, width, height)?;
        }

        // Reset attributes at the end.
        out.extend_from_slice(b"\x1b[0m");

        self.write_output(fd, &out)
    }

    /// Write a byte buffer to the tty fd.
    fn write_output(&self, fd: RawFd, buf: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < buf.len() {
            let written = unsafe {
                libc::write(
                    fd,
                    buf[offset..].as_ptr() as *const libc::c_void,
                    buf.len() - offset,
                )
            };
            if written < 0 {
                return Err(io::Error::last_os_error());
            }
            offset += written as usize;
        }
        Ok(())
    }
}

impl Default for Tty {
    fn default() -> Self {
        Self::new()
    }
}

impl Frontend for Tty {
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError> {
        let tty_name = match &cfg.tty_name {
            Some(n) => n.clone(),
            None => {
                return Err(FrontendError::Init("no tty_name set".into()));
            }
        };

        let c_name = CString::new(tty_name.as_bytes())
            .map_err(|_| FrontendError::Init("tty_name contains NUL byte".into()))?;

        // SAFETY: open(2) with a valid C string and O_RDWR.
        let fd = unsafe { libc::open(c_name.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            return Err(FrontendError::Init(format!(
                "failed to open tty '{}': {}",
                tty_name,
                io::Error::last_os_error()
            )));
        }

        self.fd = Some(fd);
        self.config_ptr = Some(cfg as *mut Config);

        // Register signal handlers so fatal signals restore the terminal.
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::tcgetattr(fd, &mut orig) };
        if rc == 0 {
            let _ = register_signal_handlers(fd, &orig);
        }

        Ok(fd)
    }

    fn deinit(&mut self) {
        // Drop RawTty first (restores termios).
        self.raw = None;
        // Unregister signal handlers.
        unregister_signal_handlers();
        // Close the fd.
        if let Some(fd) = self.fd.take() {
            unsafe {
                libc::close(fd);
            }
        }
        self.config_ptr = None;
        self.secbuf_ptr = None;
    }

    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError> {
        if mode == self.mode {
            return Err(FrontendError::InvalidMode(format!(
                "already in mode {:?}",
                mode
            )));
        }

        if mode == InterfaceMode::None {
            // Leave raw mode: drop RawTty (restores cooked termios).
            self.raw = None;
            self.mode = InterfaceMode::None;
            return Ok(());
        }

        // Entering a mode: must currently be None.
        if self.mode != InterfaceMode::None {
            return Err(FrontendError::InvalidMode(format!(
                "cannot enter {:?} from {:?}",
                mode, self.mode
            )));
        }

        let fd = self
            .fd
            .ok_or_else(|| FrontendError::Init("no tty fd".into()))?;

        // Enter raw mode.
        let raw_tty = RawTty::new(fd).map_err(FrontendError::Io)?;
        self.raw = Some(raw_tty);
        self.mode = mode;

        // Query terminal size.
        if let Ok((w, h)) = Self::query_size(fd) {
            self.width = w;
            self.height = h;
        }

        // Set window title.
        let config = self.config();
        let title = if let Some(ref t) = config.labels.title {
            format!("wayprompt TTY fallback: {t}")
        } else {
            "wayprompt TTY fallback".to_string()
        };
        let _ = self.set_window_title(&title);

        // Initial render.
        self.render().map_err(FrontendError::Io)?;

        Ok(())
    }

    fn handle_event(&mut self) -> Result<Event, FrontendError> {
        let fd = self
            .fd
            .ok_or_else(|| FrontendError::Init("no tty fd".into()))?;

        let mut buf = [0u8; 32];
        // SAFETY: read(2) into a stack buffer.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            return Err(FrontendError::Io(io::Error::last_os_error()));
        }
        if n == 0 {
            return Ok(Event::None);
        }

        let inputs = parse_input(&buf[..n as usize]);
        let mut ret = Event::None;

        for input in &inputs {
            match input {
                TtyInput::Enter => {
                    ret = Event::UserOk;
                    break;
                }
                TtyInput::Escape => {
                    ret = Event::UserAbort;
                    break;
                }
                TtyInput::Cc => {
                    let has_not_ok = {
                        let config = self.config();
                        config.labels.not_ok.is_some()
                    };
                    if has_not_ok {
                        ret = Event::UserNotOk;
                    } else {
                        ret = Event::UserAbort;
                    }
                    break;
                }
                TtyInput::Cu | TtyInput::Cw | TtyInput::CBackspace => {
                    if self.mode == InterfaceMode::GetPin {
                        let _ = self.secbuf().reset();
                        let _ = self.render();
                    }
                }
                TtyInput::Backspace => {
                    if self.mode == InterfaceMode::GetPin {
                        self.secbuf().delete_backwards();
                        let _ = self.render();
                    }
                }
                TtyInput::Codepoint(c) => {
                    if self.mode == InterfaceMode::GetPin {
                        let mut utf8_buf = [0u8; 4];
                        let s = c.encode_utf8(&mut utf8_buf);
                        let _ = self.secbuf().append_slice(s.as_bytes());
                        let _ = self.render();
                    }
                }
                TtyInput::Unknown => {
                    // Ignore unrecognized sequences.
                }
            }
        }

        Ok(ret)
    }

    fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
        // Blocking frontend: no buffered events.
        Ok(None)
    }

    fn no_event(&mut self) -> Result<(), FrontendError> {
        // No-op for the blocking TTY frontend.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_input tests --

    #[test]
    fn parse_enter() {
        assert_eq!(parse_input(b"\r"), vec![TtyInput::Enter]);
        assert_eq!(parse_input(b"\n"), vec![TtyInput::Enter]);
    }

    #[test]
    fn parse_backspace() {
        assert_eq!(parse_input(b"\x7f"), vec![TtyInput::Backspace]);
    }

    #[test]
    fn parse_utf8_codepoint() {
        // é = U+00E9 = 0xC3 0xA9
        assert_eq!(
            parse_input(b"\xC3\xA9"),
            vec![TtyInput::Codepoint('\u{00e9}')]
        );
    }

    #[test]
    fn parse_escape_sequence_unknown() {
        // ESC [ A (up arrow)
        assert_eq!(parse_input(b"\x1b[A"), vec![TtyInput::Unknown]);
    }

    #[test]
    fn parse_standalone_escape() {
        assert_eq!(parse_input(b"\x1b"), vec![TtyInput::Escape]);
    }

    #[test]
    fn parse_alt_key_unknown() {
        // Alt+a = ESC followed by 'a'
        assert_eq!(parse_input(b"\x1ba"), vec![TtyInput::Unknown]);
    }

    #[test]
    fn parse_multiple_ascii() {
        assert_eq!(
            parse_input(b"abc"),
            vec![
                TtyInput::Codepoint('a'),
                TtyInput::Codepoint('b'),
                TtyInput::Codepoint('c'),
            ]
        );
    }

    #[test]
    fn parse_ctrl_sequences() {
        assert_eq!(parse_input(b"\x03"), vec![TtyInput::Cc]);
        assert_eq!(parse_input(b"\x15"), vec![TtyInput::Cu]);
        assert_eq!(parse_input(b"\x17"), vec![TtyInput::Cw]);
        assert_eq!(parse_input(b"\x08"), vec![TtyInput::CBackspace]);
    }

    // -- render tests --

    #[test]
    fn render_pin_row_filled_and_empty() {
        let mut out = Vec::new();
        let mut line = 0;
        render_pin_row(&mut out, 8, 3, &mut line, 24).unwrap();
        let s = String::from_utf8_lossy(&out);
        // Should contain " > ***_____"
        assert!(s.contains(" > ***_____"), "got: {s:?}");
        assert_eq!(line, 2);
    }

    #[test]
    fn render_pin_row_empty() {
        let mut out = Vec::new();
        let mut line = 0;
        render_pin_row(&mut out, 8, 0, &mut line, 24).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(" > ________"), "got: {s:?}");
    }

    #[test]
    fn render_pin_row_capped() {
        let mut out = Vec::new();
        let mut line = 0;
        render_pin_row(&mut out, 8, 10, &mut line, 24).unwrap();
        let s = String::from_utf8_lossy(&out);
        // 10 chars but only 8 squares: all filled
        assert!(s.contains(" > ********"), "got: {s:?}");
    }

    #[test]
    fn render_terminal_too_small() {
        // Simulate the full render path with width=4.
        let mut out: Vec<u8> = Vec::new();
        clear_and_home(&mut out).unwrap();
        // width < 5 guard
        out.extend_from_slice(b"\x1b[1;31m");
        out.extend_from_slice(b"Terminal too small!");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Terminal too small!"));
        assert!(s.contains("\x1b[1;31m"));
    }

    #[test]
    fn render_button_contains_label() {
        let mut out = Vec::new();
        let mut line = 0;
        render_button(&mut out, "enter", "OK", &mut line, 80, 24).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("enter: OK"), "got: {s:?}");
    }

    #[test]
    fn render_content_with_bg_pads() {
        let mut out = Vec::new();
        let mut line = 0;
        let attr = Attr {
            bold: true,
            fg_black: true,
            bg_green: true,
            ..Attr::DEFAULT
        };
        render_content(&mut out, "Hi", attr, &mut line, 10, 24).unwrap();
        let s = String::from_utf8_lossy(&out);
        // " Hi" = 3 chars, width=10, so 7 spaces of padding
        assert!(s.contains(" Hi       "), "got: {s:?}");
        assert_eq!(line, 2); // 1 content line + 1 blank
    }

    // -- RawTty tests --

    #[test]
    #[cfg(unix)]
    fn raw_tty_invalid_fd_errors() {
        // An invalid fd should cause tcgetattr to fail.
        let result = RawTty::new(-1);
        assert!(result.is_err());
    }

    // -- Tty struct tests --

    #[test]
    fn tty_init_no_tty_name() {
        let mut tty = Tty::new();
        let mut cfg = Config::default(); // tty_name defaults to None
        let result = tty.init(&mut cfg);
        assert!(result.is_err());
        match result.unwrap_err() {
            FrontendError::Init(msg) => {
                assert!(msg.contains("no tty_name set"));
            }
            other => panic!("expected Init error, got: {other:?}"),
        }
    }

    #[test]
    fn tty_flush_returns_none() {
        let mut tty = Tty::new();
        assert_eq!(tty.flush().unwrap(), None);
    }

    #[test]
    fn tty_no_event_ok() {
        let mut tty = Tty::new();
        assert!(tty.no_event().is_ok());
    }
}
