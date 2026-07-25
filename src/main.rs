//! nowayprompt — pinentry entrypoint and poll-based dispatch loop.
//!
//! 100% behavioral parity with `legacy/src/wayprompt-pinentry.zig` `main()`
//! (lines 42-155). A single `poll(2)` loop multiplexes stdin (Assuan) and the
//! frontend fd (TTY), dispatching lines to `AssuanRepl::handle_line` and
//! frontend events to `AssuanRepl::handle_frontend_event`.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::os::unix::io::AsRawFd;

use nowayprompt::config;
use nowayprompt::frontend::{Event, Frontend, InterfaceMode, Tty};
use nowayprompt::protocol::assuan::AssuanRepl;
use nowayprompt::secret::{set_rlimit_core_zero, SecretBuffer};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nowayprompt: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

/// Run the pinentry dispatch loop.
///
/// Parity with `wayprompt-pinentry.zig` `main()`.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Disable core dumps so secrets are never written to disk.
    set_rlimit_core_zero().ok();

    let mut secret = SecretBuffer::new()?;
    let mut config = config::Config {
        allow_tty_fallback: true,
        ..Default::default()
    };
    config.parse().ok(); // Non-fatal: defaults remain if parse fails.

    // Frontend init: TTY fallback only (Wayland is Stage 3).
    let mut frontend = Tty::new();
    frontend.set_secret_buffer(&mut secret);
    let frontend_fd = frontend.init(&mut config)?;

    // Assuan REPL: greeting is emitted in `new`.
    let stdout = std::io::stdout();
    let mut repl = AssuanRepl::new(stdout.lock())?;

    // Buffered stdin for partial-line handling (D9).
    let stdin = std::io::stdin();
    let stdin_fd = stdin.as_raw_fd();
    let mut reader = BufReader::new(unsafe { std::fs::File::from_raw_fd(stdin_fd) });

    let mut stdin_closed = false;

    // Poll fds: [0] = stdin, [1] = frontend.
    let mut fds: [libc::pollfd; 2] = [
        libc::pollfd {
            fd: stdin_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: frontend_fd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];

    let mut in_buffer = String::new();

    while repl.is_running() {
        // Non-blocking frontend flush at loop top (legacy line 101).
        match frontend.flush() {
            Ok(Some(event)) => {
                repl.handle_frontend_event(&mut config, &mut secret, event)?;
                if event != Event::None {
                    frontend.enter_mode(InterfaceMode::None)?;
                }
            }
            Ok(None) => {}
            Err(_) => {
                repl.get_writer()
                    .write_all(b"ERR 83886179 Operation cancelled\n")?;
            }
        }

        // Poll: skip stdin fd if closed (legacy line 109).
        let nfds = if stdin_closed { 1 } else { 2 };
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), nfds, -1) };
        if ret < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("poll failed".into());
        }

        // Stdin ready.
        if !stdin_closed && (fds[0].revents & libc::POLLIN != 0) {
            in_buffer.clear();
            let n = reader.read_line(&mut in_buffer)?;
            if n == 0 {
                // EOF on stdin.
                stdin_closed = true;
            } else {
                // Strip trailing newline; dispatch the line.
                let line = in_buffer.trim_end_matches('\n');
                if !line.is_empty() {
                    repl.handle_line(&mut config, &mut secret, &mut frontend, line)?;
                }
            }
        }

        // Detect stdin HUP (legacy line 128).
        if !stdin_closed && (fds[0].revents & libc::POLLHUP != 0) {
            stdin_closed = true;
        }

        // Exit if stdin closed and no prompt active (legacy line 133).
        if stdin_closed && repl.mode_is_none() {
            break;
        }

        // Frontend fd ready.
        if fds[1].revents & libc::POLLIN != 0 {
            match frontend.handle_event() {
                Ok(event) => {
                    repl.handle_frontend_event(&mut config, &mut secret, event)?;
                    if event != Event::None {
                        frontend.enter_mode(InterfaceMode::None)?;
                    }
                }
                Err(_) => {
                    repl.get_writer()
                        .write_all(b"ERR 83886179 Operation cancelled\n")?;
                }
            }
        } else {
            frontend.no_event()?;
        }

        // Flush output, handling BrokenPipe on BYE (legacy lines 146-151).
        if let Err(e) = repl.get_writer().flush() {
            if !repl.is_running() && e.kind() == std::io::ErrorKind::BrokenPipe {
                break;
            }
            return Err(e.into());
        }
    }

    frontend.deinit();
    Ok(())
}
