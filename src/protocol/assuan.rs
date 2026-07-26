//! Assuan pinentry IPC protocol handler.
//!
//! Implements the Assuan wire protocol as spoken between pinentry and
//! gpg-agent, with 100% behavioral parity with
//! `legacy/src/wayprompt-pinentry.zig`. Handles percent-decoding,
//! command dispatch, frontend mode transitions, and zero-copy secret
//! streaming.

use std::io::{self, Write};

use crate::config::Config;
use crate::frontend::{Event, Frontend, FrontendError, InterfaceMode};
use crate::secret::SecretBuffer;

/// Error returned by Assuan protocol operations.
#[derive(Debug)]
pub enum AssuanError {
    /// Malformed percent-escape or invalid UTF-8 in decoded output.
    DecodeError(&'static str),
    /// Underlying I/O error.
    Io(io::Error),
}

impl std::fmt::Display for AssuanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DecodeError(msg) => write!(f, "assuan decode error: {msg}"),
            Self::Io(e) => write!(f, "assuan I/O error: {e}"),
        }
    }
}

impl std::error::Error for AssuanError {}

impl From<io::Error> for AssuanError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<FrontendError> for AssuanError {
    fn from(e: FrontendError) -> Self {
        Self::Io(io::Error::other(e.to_string()))
    }
}

/// Decode an Assuan percent-encoded string, optionally stripping hotkey
/// underscores.
///
/// Two-pass algorithm mirroring legacy `pinentryDupe`
/// (wayprompt-pinentry.zig lines 461-491):
/// - Pass 1: compute output length (`%` subtracts 2, `_` with
///   `strip_hotkey` subtracts 1).
/// - Pass 2: decode bytes into the output buffer.
pub fn assuan_decode(input: &str, strip_hotkey: bool) -> Result<String, AssuanError> {
    let bytes = input.as_bytes();

    // Pass 1: compute output length.
    let mut len = bytes.len();
    for &b in bytes {
        if b == b'%' {
            len -= 2;
        }
        if b == b'_' && strip_hotkey {
            len -= 1;
        }
    }

    // Pass 2: decode.
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // Require at least 2 trailing bytes for the hex pair.
            if i + 3 > bytes.len() {
                return Err(AssuanError::DecodeError("malformed percent escape"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| AssuanError::DecodeError("malformed percent escape"))?;
            let val = u8::from_str_radix(hex, 16)
                .map_err(|_| AssuanError::DecodeError("malformed percent escape"))?;
            out.push(val);
            i += 3;
        } else if bytes[i] == b'_' && strip_hotkey {
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).map_err(|_| AssuanError::DecodeError("malformed percent escape"))
}

/// Assuan-level prompt mode. Distinguishes `Confirm` from `Message`
/// (both map to frontend `InterfaceMode::Message`).
///
/// Parity with legacy `Mode` enum in wayprompt-pinentry.zig line 39.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssuanMode {
    None,
    GetPin,
    Confirm,
    Message,
}

/// Assuan protocol REPL state machine.
///
/// Holds the output writer and protocol state. The dispatch loop owns
/// `Config`, `SecretBuffer`, and the `Frontend`; they are passed into
/// `handle_line` and `handle_frontend_event` per call.
pub struct AssuanRepl<W: Write> {
    writer: W,
    mode: AssuanMode,
    default_ok: Option<String>,
    default_cancel: Option<String>,
    default_yes: Option<String>,
    default_no: Option<String>,
    is_running: bool,
}

impl<W: Write> AssuanRepl<W> {
    /// Create a new REPL, emitting the Assuan greeting and flushing.
    pub fn new(mut writer: W) -> io::Result<Self> {
        writer.write_all(b"OK wayprompt is pleased to meet you\n")?;
        writer.flush()?;
        Ok(Self {
            writer,
            mode: AssuanMode::None,
            default_ok: None,
            default_cancel: None,
            default_yes: None,
            default_no: None,
            is_running: true,
        })
    }

    /// Whether the REPL loop should continue running.
    pub fn is_running(&self) -> bool {
        self.is_running
    }

    /// Borrow the writer for direct writes (e.g. error responses).
    pub fn get_writer(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Whether the Assuan mode is `None` (no prompt active).
    pub fn mode_is_none(&self) -> bool {
        self.mode == AssuanMode::None
    }

    /// Whether handling `line` requires creating a frontend first.
    ///
    /// Assuan setup commands intentionally remain frontend-free so `OPTION`
    /// values can select the connection used by the first prompt.
    pub fn requires_frontend(&self, cfg: &Config, line: &str) -> bool {
        if self.mode != AssuanMode::None {
            return false;
        }

        match line
            .split_whitespace()
            .next()
            .map(str::to_ascii_uppercase)
            .as_deref()
        {
            Some("GETPIN") | Some("CONFIRM") => true,
            Some("MESSAGE") => {
                cfg.labels.title.is_some()
                    || cfg.labels.description.is_some()
                    || cfg.labels.err_message.is_some()
            }
            _ => false,
        }
    }

    /// Handle a single Assuan input line.
    ///
    /// Parity with legacy `parseInput` (wayprompt-pinentry.zig
    /// lines 249-420). Commands received while a prompt is active
    /// (mode != None) are silently dropped (parity line 276).
    pub fn handle_line(
        &mut self,
        cfg: &mut Config,
        _secbuf: &mut SecretBuffer,
        frontend: &mut dyn Frontend,
        line: &str,
    ) -> Result<(), AssuanError> {
        // Drop commands during active prompt (parity line 276).
        if self.mode != AssuanMode::None {
            return Ok(());
        }

        // Tokenize: first whitespace-delimited token is the command.
        let cmd = match line.split_whitespace().next() {
            Some(c) => c,
            None => return Ok(()),
        };
        let cmd_upper = cmd.to_ascii_uppercase();

        match cmd_upper.as_str() {
            "SETTITLE" => {
                let args = &line[cmd.len()..];
                cfg.labels.title = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETPROMPT" => {
                let args = &line[cmd.len()..];
                cfg.labels.prompt = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETDESC" => {
                let args = &line[cmd.len()..];
                cfg.labels.description = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETERROR" => {
                let args = &line[cmd.len()..];
                cfg.labels.err_message = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETOK" => {
                // Legacy uses line["setok ".len..] — strip one
                // leading space from args.
                let args = line.get(cmd.len() + 1..).unwrap_or("");
                cfg.labels.ok = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETNOTOK" => {
                let args = line.get(cmd.len() + 1..).unwrap_or("");
                cfg.labels.not_ok = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "SETCANCEL" => {
                let args = line.get(cmd.len() + 1..).unwrap_or("");
                cfg.labels.cancel = Some(assuan_decode(args, false)?);
                self.writer.write_all(b"OK\n")?;
            }
            "GETPIN" => {
                // Apply default button labels (parity getpin(),
                // lines 195-212). Transfers ownership.
                if cfg.labels.ok.is_none() {
                    if let Some(ok) = self.default_ok.take() {
                        cfg.labels.ok = Some(ok);
                    }
                }
                if cfg.labels.cancel.is_none() {
                    if let Some(cancel) = self.default_cancel.take() {
                        cfg.labels.cancel = Some(cancel);
                    }
                }
                self.mode = AssuanMode::GetPin;
                frontend.enter_mode(InterfaceMode::GetPin)?;
            }
            "CONFIRM" => {
                // Apply default yes/no labels (parity confirm(),
                // lines 230-247). Transfers ownership.
                if cfg.labels.ok.is_none() {
                    if let Some(yes) = self.default_yes.take() {
                        cfg.labels.ok = Some(yes);
                    }
                }
                if cfg.labels.cancel.is_none() {
                    if let Some(no) = self.default_no.take() {
                        cfg.labels.cancel = Some(no);
                    }
                }
                self.mode = AssuanMode::Confirm;
                frontend.enter_mode(InterfaceMode::Message)?;
            }
            "MESSAGE" => {
                // If nothing to display, just acknowledge (parity
                // message(), lines 214-228).
                if cfg.labels.title.is_none()
                    && cfg.labels.description.is_none()
                    && cfg.labels.err_message.is_none()
                {
                    self.writer.write_all(b"OK\n")?;
                    return Ok(());
                }
                self.mode = AssuanMode::Message;
                frontend.enter_mode(InterfaceMode::Message)?;
            }
            "GETINFO" => {
                let sub = line.split_whitespace().nth(1).unwrap_or("");
                let sub_upper = sub.to_ascii_uppercase();
                match sub_upper.as_str() {
                    "FLAVOR" => {
                        self.writer.write_all(b"D wayprompt\nEND\n")?;
                    }
                    "VERSION" => {
                        self.writer.write_all(b"D 0.0.0\nEND\n")?;
                    }
                    "PID" => {
                        write!(self.writer, "D {}\nEND\n", std::process::id())?;
                    }
                    _ => {}
                }
                self.writer.write_all(b"OK\n")?;
            }
            "BYE" => {
                self.writer.write_all(b"OK\n")?;
                self.is_running = false;
            }
            "OPTION" => {
                self.handle_option(cfg, line)?;
                self.writer.write_all(b"OK\n")?;
            }
            "RESET" => {
                cfg.reset();
                self.writer.write_all(b"OK\n")?;
            }
            "NOP" => {
                self.writer.write_all(b"OK\n")?;
            }
            "HELP" => {
                self.writer.write_all(
                    b"# NOP\n# SETTITLE\n# SETPROMPT\n# SETDESC\n\
                      # SETERROR\n# GETPIN\n# BYE\n# OPTION\n\
                      # RESET\nOK\n",
                )?;
            }
            "SETKEYINFO" => {
                // Silently accepted (parity lines 372-380). gpg-agent
                // aborts on ERR for this command.
                self.writer.write_all(b"OK\n")?;
            }
            "CANCEL" | "SETGENPIN" | "SETGENPIN_TT" | "SETTIMEOUT" | "END" | "QUIT" | "AUTH"
            | "CLEARPASSPHRASE" | "SETREPEAT" | "SETREPEATERROR" | "SETQUALITYBAR"
            | "SETQUALITYBAR_TT" => {
                self.writer.write_all(b"ERR 536870981 Not implemented\n")?;
            }
            _ => {
                self.writer
                    .write_all(b"ERR 536871187 Unknown IPC command\n")?;
            }
        }
        Ok(())
    }

    /// Handle the OPTION command (parity lines 324-368).
    ///
    /// Prefix-matches on the option argument token (NOT split_once).
    /// Unknown options are silently accepted.
    fn handle_option(&mut self, cfg: &mut Config, line: &str) -> Result<(), AssuanError> {
        let option_token = line.split_whitespace().nth(1).unwrap_or("");

        if let Some(val) = get_option("putenv=WAYLAND_DISPLAY=", option_token, line) {
            cfg.wayland_display = Some(val.to_string());
        } else if let Some(val) = get_option("ttyname=", option_token, line) {
            cfg.tty_name = Some(val.to_string());
        } else if let Some(val) = get_option("default-ok=", option_token, line) {
            self.default_ok = Some(assuan_decode(val, true)?);
        } else if let Some(val) = get_option("default-cancel=", option_token, line) {
            self.default_cancel = Some(assuan_decode(val, true)?);
        } else if let Some(val) = get_option("default-yes=", option_token, line) {
            self.default_yes = Some(assuan_decode(val, true)?);
        } else if let Some(val) = get_option("default-no=", option_token, line) {
            self.default_no = Some(assuan_decode(val, true)?);
        }
        Ok(())
    }

    /// Handle a frontend event (parity handleFrontendEvent,
    /// lines 157-193).
    ///
    /// Translates user interaction events into Assuan wire responses
    /// and resets protocol state after GetPin/Confirm prompts.
    pub fn handle_frontend_event(
        &mut self,
        cfg: &mut Config,
        secbuf: &mut SecretBuffer,
        event: Event,
    ) -> Result<(), AssuanError> {
        match event {
            Event::None => return Ok(()),
            Event::UserAbort => {
                self.writer
                    .write_all(b"ERR 83886179 Operation cancelled\n")?;
            }
            Event::UserNotOk => {
                self.writer.write_all(b"ERR 83886194 not confirmed\n")?;
            }
            Event::UserOk => {
                if self.mode == AssuanMode::GetPin {
                    dump_pin(&mut self.writer, secbuf.slice())?;
                } else {
                    self.writer.write_all(b"OK\n")?;
                }
            }
        }

        // The error message must automatically reset after every
        // GETPIN or CONFIRM action (parity lines 181-187).
        if self.mode == AssuanMode::GetPin || self.mode == AssuanMode::Confirm {
            cfg.labels.err_message = None;
        }

        self.mode = AssuanMode::None;
        secbuf
            .reset()
            .map_err(|_| AssuanError::DecodeError("secret reset failed"))?;
        Ok(())
    }
}

/// Extract an OPTION value from the raw line using prefix matching.
///
/// Parity with legacy `getOption` (wayprompt-pinentry.zig lines
/// 433-438). Uses the fixed offset `"option ".len() + opt.len()`
/// into the original line to extract the value, preserving any
/// characters that the tokenizer might have split on.
fn get_option<'a>(opt: &str, arg: &str, line: &'a str) -> Option<&'a str> {
    if arg.starts_with(opt) {
        line.get("option ".len() + opt.len()..)
    } else {
        None
    }
}

/// Stream a secret to the Assuan writer without intermediate
/// allocation.
///
/// Parity with legacy `dumpPin` (wayprompt-pinentry.zig lines
/// 423-431). Writes `D <secret>\nEND\nOK\n` for Some, or just
/// `OK\n` for None. NO format!/String allocation holding the secret.
fn dump_pin<W: Write>(writer: &mut W, secret: Option<&[u8]>) -> io::Result<()> {
    match secret {
        Some(bytes) => {
            writer.write_all(b"D ")?;
            writer.write_all(bytes)?;
            writer.write_all(b"\nEND\nOK\n")?;
        }
        None => {
            writer.write_all(b"OK\n")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{FrontendError, InterfaceMode};
    use std::os::unix::io::RawFd;

    // --- assuan_decode tests ---

    #[test]
    fn decode_percent_space() {
        assert_eq!(assuan_decode("foo%20bar", false).unwrap(), "foo bar");
    }

    #[test]
    fn decode_strip_hotkey() {
        assert_eq!(assuan_decode("_Cancel", true).unwrap(), "Cancel");
    }

    #[test]
    fn decode_keep_underscore_without_strip() {
        assert_eq!(assuan_decode("_Cancel", false).unwrap(), "_Cancel");
    }

    #[test]
    fn decode_truncated_percent() {
        assert!(assuan_decode("foo%2", false).is_err());
    }

    #[test]
    fn decode_invalid_hex() {
        assert!(assuan_decode("foo%ZZbar", false).is_err());
    }

    #[test]
    fn decode_utf8_percent() {
        assert_eq!(assuan_decode("%C3%A9", false).unwrap(), "é");
    }

    #[test]
    fn decode_empty() {
        assert_eq!(assuan_decode("", false).unwrap(), "");
    }

    // --- dump_pin tests ---

    #[test]
    fn dump_pin_none() {
        let mut buf = Vec::new();
        dump_pin(&mut buf, None).unwrap();
        assert_eq!(buf, b"OK\n");
    }

    #[test]
    fn dump_pin_some() {
        let mut buf = Vec::new();
        dump_pin(&mut buf, Some(b"hunter2")).unwrap();
        assert_eq!(buf, b"D hunter2\nEND\nOK\n");
    }

    #[test]
    fn dump_pin_large_secret() {
        let secret = vec![b'A'; 1000];
        let mut buf = Vec::new();
        dump_pin(&mut buf, Some(&secret)).unwrap();
        assert_eq!(buf.len(), 2 + 1000 + 8); // "D " + secret + "\nEND\nOK\n"
        assert!(buf.starts_with(b"D "));
        assert!(buf.ends_with(b"\nEND\nOK\n"));
    }

    // --- Mock frontend for REPL tests ---

    struct MockFrontend {
        mode: InterfaceMode,
    }

    impl MockFrontend {
        fn new() -> Self {
            Self {
                mode: InterfaceMode::None,
            }
        }
    }

    impl Frontend for MockFrontend {
        fn init(&mut self, _cfg: &mut Config) -> Result<RawFd, FrontendError> {
            Ok(0)
        }

        fn deinit(&mut self) {}

        fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError> {
            self.mode = mode;
            Ok(())
        }

        fn handle_event(&mut self) -> Result<Event, FrontendError> {
            Ok(Event::None)
        }

        fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
            Ok(None)
        }

        fn no_event(&mut self) -> Result<(), FrontendError> {
            Ok(())
        }
    }

    // --- REPL test helpers ---

    impl AssuanRepl<Vec<u8>> {
        fn clear_output(&mut self) {
            self.writer.clear();
        }

        fn output(&self) -> &[u8] {
            &self.writer
        }
    }

    fn make_repl() -> AssuanRepl<Vec<u8>> {
        AssuanRepl::new(Vec::new()).unwrap()
    }

    fn handle(
        repl: &mut AssuanRepl<Vec<u8>>,
        cfg: &mut Config,
        secbuf: &mut SecretBuffer,
        fe: &mut MockFrontend,
        line: &str,
    ) {
        repl.clear_output();
        repl.handle_line(cfg, secbuf, fe, line).unwrap();
    }

    // --- REPL tests ---

    #[test]
    fn repl_greeting() {
        let repl = make_repl();
        assert_eq!(repl.output(), b"OK wayprompt is pleased to meet you\n");
    }

    #[test]
    fn repl_settitle() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "SETTITLE T");
        assert_eq!(repl.output(), b"OK\n");
        assert_eq!(cfg.labels.title, Some(" T".to_string()));
    }

    #[test]
    fn repl_getinfo_flavor() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "GETINFO flavor");
        assert_eq!(repl.output(), b"D wayprompt\nEND\nOK\n");
    }

    #[test]
    fn repl_bye() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "BYE");
        assert_eq!(repl.output(), b"OK\n");
        assert!(!repl.is_running());
    }

    #[test]
    fn repl_setkeyinfo() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "SETKEYINFO X");
        assert_eq!(repl.output(), b"OK\n");
    }

    #[test]
    fn repl_settimeout_not_implemented() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "SETTIMEOUT 30");
        assert_eq!(repl.output(), b"ERR 536870981 Not implemented\n");
    }

    #[test]
    fn repl_unknown_command() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "BOGUS");
        assert_eq!(repl.output(), b"ERR 536871187 Unknown IPC command\n");
    }

    #[test]
    fn repl_help() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "HELP");
        assert_eq!(
            repl.output(),
            b"# NOP\n# SETTITLE\n# SETPROMPT\n# SETDESC\n\
              # SETERROR\n# GETPIN\n# BYE\n# OPTION\n\
              # RESET\nOK\n"
        );
    }

    #[test]
    fn repl_option_default_ok_strips_hotkey() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(
            &mut repl,
            &mut cfg,
            &mut secbuf,
            &mut fe,
            "OPTION default-ok=_OK",
        );
        assert_eq!(repl.output(), b"OK\n");
        assert_eq!(repl.default_ok, Some("OK".to_string()));
    }

    #[test]
    fn repl_commands_dropped_during_prompt() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        // Enter GetPin mode.
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "GETPIN");
        assert_eq!(repl.mode, AssuanMode::GetPin);
        // Commands during active prompt are dropped.
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "NOP");
        assert_eq!(repl.output(), b"");
    }

    #[test]
    fn repl_getpin_applies_defaults() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        // Set defaults via OPTION.
        handle(
            &mut repl,
            &mut cfg,
            &mut secbuf,
            &mut fe,
            "OPTION default-ok=_Submit",
        );
        handle(
            &mut repl,
            &mut cfg,
            &mut secbuf,
            &mut fe,
            "OPTION default-cancel=_Abort",
        );
        // GETPIN applies defaults.
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "GETPIN");
        assert_eq!(cfg.labels.ok, Some("Submit".to_string()));
        assert_eq!(cfg.labels.cancel, Some("Abort".to_string()));
        assert_eq!(repl.mode, AssuanMode::GetPin);
        assert_eq!(fe.mode, InterfaceMode::GetPin);
    }

    #[test]
    fn repl_frontend_event_user_ok_getpin() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "GETPIN");
        secbuf.append_slice(b"hunter2").unwrap();
        repl.clear_output();
        repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserOk)
            .unwrap();
        assert_eq!(repl.output(), b"D hunter2\nEND\nOK\n");
        assert_eq!(repl.mode, AssuanMode::None);
    }

    #[test]
    fn repl_frontend_event_user_abort() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "GETPIN");
        repl.clear_output();
        repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserAbort)
            .unwrap();
        assert_eq!(repl.output(), b"ERR 83886179 Operation cancelled\n");
        assert_eq!(repl.mode, AssuanMode::None);
    }

    #[test]
    fn repl_message_empty_is_noop() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "MESSAGE");
        assert_eq!(repl.output(), b"OK\n");
        assert_eq!(repl.mode, AssuanMode::None);
    }

    #[test]
    fn repl_option_wayland_display() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(
            &mut repl,
            &mut cfg,
            &mut secbuf,
            &mut fe,
            "OPTION putenv=WAYLAND_DISPLAY=wayland-1",
        );
        assert_eq!(repl.output(), b"OK\n");
        assert_eq!(cfg.wayland_display, Some("wayland-1".to_string()));
    }

    #[test]
    fn repl_option_ttyname() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(
            &mut repl,
            &mut cfg,
            &mut secbuf,
            &mut fe,
            "OPTION ttyname=/dev/pts/0",
        );
        assert_eq!(repl.output(), b"OK\n");
        assert_eq!(cfg.tty_name, Some("/dev/pts/0".to_string()));
    }

    #[test]
    fn repl_case_insensitive() {
        let mut repl = make_repl();
        let mut cfg = Config::default();
        let mut secbuf = SecretBuffer::new().unwrap();
        let mut fe = MockFrontend::new();
        handle(&mut repl, &mut cfg, &mut secbuf, &mut fe, "nop");
        assert_eq!(repl.output(), b"OK\n");
    }
}
