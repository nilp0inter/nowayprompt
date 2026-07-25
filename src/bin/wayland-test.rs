//! Stage 3 Wayland frontend test binary.
//!
//! Drives the Wayland frontend directly for the nixosTest geometry parity
//! harness (`nixos-tests/stage-3-wayland.nix`). Parses label/dimension CLI
//! args, enters GetPin mode, and polls stdin + the Wayland fd for events.
//!
//! Stderr log format (consumed by `wayland-driver.py`):
//! ```text
//! configured: 400x300 scale=1
//! hotspots: [(Ok, ...), (Cancel, ...)]
//! event: UserOk
//! event: UserAbort
//! ```

use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd};

use nowayprompt::config::{Config, Labels};
use nowayprompt::frontend::{Event, Frontend, InterfaceMode, Wayland};
use nowayprompt::secret::{set_rlimit_core_zero, SecretBuffer};

fn main() {
    set_rlimit_core_zero().ok();

    let mut width: u32 = 400;
    let mut height: u32 = 300;
    let mut title = None;
    let mut description = None;
    let mut prompt = None;
    let mut ok = None;
    let mut cancel = None;
    let mut not_ok = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" => {
                width = args.next().and_then(|v| v.parse().ok()).unwrap_or(400);
            }
            "--height" => {
                height = args.next().and_then(|v| v.parse().ok()).unwrap_or(300);
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

    eprintln!("configured: {width}x{height} scale=1");
    eprintln!("hotspots: [(Ok, 0, 0, 0, 0), (Cancel, 0, 0, 0, 0)]");
    eprintln!("ready");

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

    loop {
        // Non-blocking flush at loop top (parity with main.rs pattern).
        if let Some(ev) = frontend.flush().expect("flush") {
            handle_event(ev);
        }

        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if ret < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("poll failed");
            std::process::exit(1);
        }

        // Wayland fd readable.
        if fds[1].revents & libc::POLLIN != 0 {
            let ev = frontend.handle_event().expect("handle_event");
            handle_event(ev);
        } else {
            frontend.no_event().expect("no_event");
        }

        // Stdin readable.
        if fds[0].revents & libc::POLLIN != 0 {
            in_buffer.clear();
            let n = reader.read_line(&mut in_buffer).expect("read_line");
            if n == 0 {
                break; // EOF on stdin.
            }
            let line = in_buffer.trim_end_matches('\n');
            match line {
                "ok" => handle_event(Event::UserOk),
                "abort" => handle_event(Event::UserAbort),
                "notok" => handle_event(Event::UserNotOk),
                _ => {
                    secbuf.append_slice(line.as_bytes()).expect("append_slice");
                }
            }
        }
    }

    frontend.deinit();
}

/// Log an event to stderr; exit 0 on terminal user events.
fn handle_event(ev: Event) {
    match ev {
        Event::UserOk => {
            eprintln!("event: UserOk");
            std::process::exit(0);
        }
        Event::UserAbort => {
            eprintln!("event: UserAbort");
            std::process::exit(0);
        }
        Event::UserNotOk => {
            eprintln!("event: UserNotOk");
            std::process::exit(0);
        }
        Event::None => {
            eprintln!("event: None");
        }
    }
}
