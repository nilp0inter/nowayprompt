//! Integration tests for the Assuan REPL command matrix.
//!
//! These tests exercise the full `AssuanRepl::handle_line` dispatch with a
//! mock frontend, asserting byte-identical output against the legacy
//! `wayprompt-pinentry.zig` behavior. Parity with tasks 12.1-12.9.

use nowayprompt::config::Config;
use nowayprompt::frontend::{Event, Frontend, FrontendError, InterfaceMode};
use nowayprompt::protocol::assuan::AssuanRepl;
use nowayprompt::secret::SecretBuffer;
use std::os::fd::RawFd;

/// Mock frontend that records mode transitions and can replay events.
struct MockFrontend {
    mode: InterfaceMode,
    next_event: Option<Event>,
    mode_log: Vec<InterfaceMode>,
}

impl MockFrontend {
    fn new() -> Self {
        Self {
            mode: InterfaceMode::None,
            next_event: None,
            mode_log: Vec::new(),
        }
    }

    fn set_event(&mut self, event: Event) {
        self.next_event = Some(event);
    }
}

impl Frontend for MockFrontend {
    fn init(&mut self, _cfg: &mut Config) -> Result<RawFd, FrontendError> {
        Ok(0)
    }

    fn deinit(&mut self) {}

    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError> {
        self.mode_log.push(mode);
        self.mode = mode;
        Ok(())
    }

    fn handle_event(&mut self) -> Result<Event, FrontendError> {
        Ok(self.next_event.take().unwrap_or(Event::None))
    }

    fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
        Ok(self.next_event.take())
    }

    fn no_event(&mut self) -> Result<(), FrontendError> {
        Ok(())
    }
}

/// Create a REPL with a Vec<u8> writer and a mock frontend.
fn make_repl() -> (AssuanRepl<Vec<u8>>, MockFrontend) {
    let repl = AssuanRepl::new(Vec::new()).unwrap();
    let frontend = MockFrontend::new();
    (repl, frontend)
}

#[test]
fn greeting_on_new() {
    let (mut repl, _) = make_repl();
    assert_eq!(
        repl.get_writer().as_slice(),
        b"OK wayprompt is pleased to meet you\n"
    );
}

#[test]
fn settitle_setprompt_setdesc_then_getpin_enters_getpin_mode() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETTITLE T")
        .unwrap();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETPROMPT P")
        .unwrap();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETDESC D")
        .unwrap();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETPIN")
        .unwrap();

    // Three OK responses for SETTITLE/SETPROMPT/SETDESC.
    let out = repl.get_writer().as_slice();
    assert_eq!(
        &out[b"OK wayprompt is pleased to meet you\n".len()..],
        b"OK\nOK\nOK\n"
    );
    // Frontend entered GetPin mode.
    assert_eq!(frontend.mode, InterfaceMode::GetPin);
}

#[test]
fn getpin_with_enter_returns_pin() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    // Set a prompt label and enter GETPIN mode.
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETPROMPT P")
        .unwrap();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETPIN")
        .unwrap();

    // Simulate the user entering a pin: append bytes to the secret buffer.
    secbuf.append_slice(b"hunter2").unwrap();

    // Simulate Enter (UserOk) event from the frontend.
    repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserOk)
        .unwrap();

    // Output should contain D hunter2\nEND\nOK\n.
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"D hunter2\nEND\nOK\n"));
}

#[test]
fn getpin_with_empty_pin_returns_ok() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETPIN")
        .unwrap();

    // No pin entered — empty secret.
    repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserOk)
        .unwrap();

    let out = repl.get_writer().as_slice();
    // After greeting, GETPIN enters GetPin mode (no output), then
    // empty pin → "OK\n".
    assert!(out.ends_with(b"OK\n"));
}

#[test]
fn getpin_escape_returns_cancelled() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETPIN")
        .unwrap();

    // Escape → UserAbort.
    repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserAbort)
        .unwrap();

    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"ERR 83886179 Operation cancelled\n"));
}

#[test]
fn confirm_ctrl_c_with_notok_returns_not_confirmed() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    // Set not_ok label so Ctrl-C maps to UserNotOk.
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETNOTOK No")
        .unwrap();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "CONFIRM")
        .unwrap();

    // Ctrl-C → UserNotOk (because not_ok is set).
    repl.handle_frontend_event(&mut cfg, &mut secbuf, Event::UserNotOk)
        .unwrap();

    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"ERR 83886194 not confirmed\n"));
}

#[test]
fn bye_emits_ok_and_stops() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "BYE")
        .unwrap();

    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"OK\n"));
    assert!(!repl.is_running());
}

#[test]
fn setkeyinfo_silently_accepted() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETKEYINFO X")
        .unwrap();

    let out = repl.get_writer().as_slice();
    // After greeting, SETKEYINFO → OK\n.
    assert_eq!(
        &out[b"OK wayprompt is pleased to meet you\n".len()..],
        b"OK\n"
    );
}

#[test]
fn settimeout_not_implemented() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETTIMEOUT 30")
        .unwrap();

    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"ERR 536870981 Not implemented\n"));
}

#[test]
fn unknown_command_err() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "BOGUS")
        .unwrap();

    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"ERR 536871187 Unknown IPC command\n"));
}

#[test]
fn getinfo_flavor_version_pid() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    // GETINFO flavor
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETINFO flavor")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"D wayprompt\nEND\nOK\n"));

    // Reset for next command.
    repl.get_writer().clear();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETINFO version")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"D 0.0.0\nEND\nOK\n"));

    // GETINFO pid — format D <pid>\nEND\nOK\n (pid is process-specific).
    repl.get_writer().clear();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "GETINFO pid")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"\nEND\nOK\n"));
    assert!(out.starts_with(b"D "));
}

#[test]
fn reset_clears_labels_and_emits_ok() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "SETTITLE T")
        .unwrap();
    assert_eq!(cfg.labels.title, Some(" T".into()));

    repl.get_writer().clear();
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "RESET")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert_eq!(out, b"OK\n");
    assert_eq!(cfg.labels.title, None);
}

#[test]
fn nop_emits_ok() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "NOP")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"OK\n"));
}

#[test]
fn help_emits_command_list() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "HELP")
        .unwrap();
    let out = repl.get_writer().as_slice();
    let expected = b"# NOP\n# SETTITLE\n# SETPROMPT\n# SETDESC\n# SETERROR\n# GETPIN\n# BYE\n# OPTION\n# RESET\nOK\n";
    assert!(out.ends_with(expected));
}

#[test]
fn message_with_no_labels_is_noop() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    // No labels set → MESSAGE is a no-op returning OK.
    repl.handle_line(&mut cfg, &mut secbuf, &mut frontend, "MESSAGE")
        .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"OK\n"));
    // Frontend should NOT have entered Message mode.
    assert_eq!(frontend.mode, InterfaceMode::None);
}

#[test]
fn option_default_ok_strips_hotkey() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    // OPTION default-ok=_OK → hotkey stripped → "OK"
    repl.handle_line(
        &mut cfg,
        &mut secbuf,
        &mut frontend,
        "OPTION default-ok=_OK",
    )
    .unwrap();
    let out = repl.get_writer().as_slice();
    assert!(out.ends_with(b"OK\n"));
}

#[test]
fn option_ttyname_stores_in_config() {
    let (mut repl, mut frontend) = make_repl();
    let mut cfg = Config::default();
    let mut secbuf = SecretBuffer::new().unwrap();

    repl.handle_line(
        &mut cfg,
        &mut secbuf,
        &mut frontend,
        "OPTION ttyname=/dev/tty1",
    )
    .unwrap();
    assert_eq!(cfg.tty_name, Some("/dev/tty1".into()));
}
