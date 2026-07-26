//! Test-only Wayland geometry utility binary.
//!
//! Drives the Wayland frontend directly for the Wayland differential parity
//! nixosTest gate, which compares the in-tree build against the pinned
//! `pkgs.wayprompt` oracle. Parses label CLI args,
//! enters GetPin mode, polls stdin + the Wayland fd for events, and reports
//! the *real* configured surface geometry + hotspots (read back from the
//! frontend once the first configure event renders the surface).
//!
//! Stderr log format (consumed by `wayland-driver.py`):
//! ```text
//! configured: <W>x<H> scale=<S>
//! hotspots: [(Ok, x, y, w, h), (Cancel, x, y, w, h)]
//! ready
//! event: UserOk
//! event: UserAbort
//! ```

use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd};

use nowayprompt::config::{Config, Labels};
use nowayprompt::frontend::wayland::render::HotSpot;
use nowayprompt::frontend::{Event, Frontend, InterfaceMode, Wayland};
use nowayprompt::secret::{set_rlimit_core_zero, SecretBuffer};

fn main() {
    set_rlimit_core_zero().ok();

    let mut title = None;
    let mut description = None;
    let mut prompt = None;
    let mut ok = None;
    let mut cancel = None;
    let mut not_ok = None;

    // `--width`/`--height` are accepted (the driver passes them) but
    // ignored: the surface computes its own size from the label text.
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" | "--height" => {
                args.next(); // consume and discard the value
            }
            "--title" => title = args.next(),
            "--description" => description = args.next(),
            "--prompt" => prompt = args.next(),
            "--ok" => ok = args.next(),
            "--cancel" => cancel = args.next(),
            "--not-ok" => not_ok = args.next(),
            _ => {}
        }
    }

    let mut config = Config {
        labels: Labels {
            title,
            description,
            prompt,
            err_message: None,
            not_ok,
            ok,
            cancel,
        },
        ..Default::default()
    };

    let mut secbuf = SecretBuffer::new().expect("secbuf");
    let mut frontend = Wayland::new();
    frontend.set_secret_buffer(&mut secbuf);

    let fd = match frontend.init(&mut config) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("init: {e}");
            std::process::exit(1);
        }
    };

    frontend
        .enter_mode(InterfaceMode::GetPin)
        .expect("enter_mode");

    let stdin = std::io::stdin();
    let stdin_fd = stdin.as_raw_fd();
    let mut reader = BufReader::new(unsafe { std::fs::File::from_raw_fd(stdin_fd) });

    let mut fds: [libc::pollfd; 2] = [
        libc::pollfd {
            fd: stdin_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];

    let mut in_buffer = String::new();
    // Report the real geometry once, after the first configure renders.
    let mut reported = false;

    'outer: loop {
        // Non-blocking flush at loop top (parity with main.rs pattern).
        match frontend.flush() {
            Ok(Some(ev)) => {
                if is_terminal(ev) {
                    break 'outer;
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("flush error: {e}");
                break 'outer;
            }
        }

        // 100ms timeout so the loop keeps pumping until the surface
        // configures (the configure reply makes the Wayland fd readable).
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, 100) };
        if ret < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("poll failed");
            std::process::exit(1);
        }

        // Wayland fd readable.
        if fds[1].revents & libc::POLLIN != 0 {
            let ev = match frontend.handle_event() {
                Ok(ev) => ev,
                Err(e) => {
                    eprintln!("handle_event error: {e}");
                    break 'outer;
                }
            };
            if is_terminal(ev) {
                break 'outer;
            }
        } else {
            frontend.no_event().expect("no_event");
        }

        // Report real geometry once the surface has configured + rendered.
        if !reported {
            if let Some((w, h, scale, hotspots)) = frontend.surface_info() {
                eprintln!("configured: {w}x{h} scale={scale}");
                eprintln!("hotspots: [{}]", format_hotspots(&hotspots));
                eprintln!("ready");
                reported = true;
            }
        }

        // Stdin readable (the driver may feed `ok`/`abort` commands;
        // primary input is wtype over Wayland). On EOF, stop polling
        // stdin but keep running for Wayland keyboard events.
        if fds[0].revents & libc::POLLIN != 0 {
            in_buffer.clear();
            let n = reader.read_line(&mut in_buffer).expect("read_line");
            if n == 0 {
                fds[0].fd = -1; // EOF: stop polling stdin.
                fds[0].events = 0;
            } else {
                let line = in_buffer.trim_end_matches('\n');
                let terminal = match line {
                    "ok" => Some(Event::UserOk),
                    "abort" => Some(Event::UserAbort),
                    "notok" => Some(Event::UserNotOk),
                    _ => {
                        secbuf.append_slice(line.as_bytes()).expect("append_slice");
                        None
                    }
                };
                if let Some(ev) = terminal {
                    if is_terminal(ev) {
                        break 'outer;
                    }
                }
            }
        }
    }

    frontend.deinit();
}

/// Format hotspots as `(Effect, x, y, w, h)` tuples (driver greps for the
/// `Ok`/`Cancel` effect names).
fn format_hotspots(hotspots: &[HotSpot]) -> String {
    let parts: Vec<String> = hotspots
        .iter()
        .map(|h| {
            format!(
                "({:?}, {}, {}, {}, {})",
                h.effect, h.x, h.y, h.width, h.height
            )
        })
        .collect();
    parts.join(", ")
}

/// Log an event to stderr; return `true` if it terminates the session.
fn is_terminal(ev: Event) -> bool {
    match ev {
        Event::UserOk => {
            eprintln!("event: UserOk");
            true
        }
        Event::UserAbort => {
            eprintln!("event: UserAbort");
            true
        }
        Event::UserNotOk => {
            eprintln!("event: UserNotOk");
            true
        }
        Event::None => false,
    }
}
