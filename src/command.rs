//! Public command entrypoints selected by executable basename.

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::config::{Config, Labels};
use crate::frontend::{Event, Frontend, FrontendOwner, InterfaceMode};
use crate::protocol::assuan::AssuanRepl;
use crate::secret::{set_rlimit_core_zero, SecretBuffer};

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_CANCEL: u8 = 10;
const EXIT_NOT_OK: u8 = 20;

/// Dispatch a public executable invocation by its basename.
pub fn run(args: Vec<OsString>) -> Result<u8, Box<dyn std::error::Error>> {
    let argv0 = args.first().ok_or("missing argv[0]")?;
    let basename = Path::new(argv0)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid executable basename")?;

    match basename {
        "nowayprompt" => run_cli(&args[1..]),
        "pinentry-nowayprompt" => run_pinentry(),
        "nowayprompt-ssh-askpass" => run_askpass(&args[1..]),
        _ => Err(format!("unknown executable basename: {basename}").into()),
    }
}

fn run_cli(args: &[OsString]) -> Result<u8, Box<dyn std::error::Error>> {
    let parsed = match parse_cli(args)? {
        ParseResult::Help => {
            print_help(&mut io::stdout().lock())?;
            return Ok(EXIT_OK);
        }
        ParseResult::Request(request) => request,
    };

    validate_cli(&parsed)?;
    let mut config = Config {
        labels: parsed.labels,
        wayland_display: parsed.wayland_display,
        ..Config::default()
    };
    config.parse()?;

    let mut secret = SecretBuffer::new()?;
    let event = prompt(
        &mut config,
        &mut secret,
        if parsed.get_pin {
            InterfaceMode::GetPin
        } else {
            InterfaceMode::Message
        },
    )?;
    let pin = (parsed.get_pin && event == Event::UserOk)
        .then(|| secret.slice())
        .flatten();
    write_cli_output(
        &mut io::stdout().lock(),
        event,
        pin,
        parsed.get_pin,
        parsed.json,
    )?;
    Ok(exit_status(event))
}

fn run_askpass(args: &[OsString]) -> Result<u8, Box<dyn std::error::Error>> {
    let title = if args.is_empty() {
        "SSH Password:".to_owned()
    } else {
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut config = Config {
        labels: Labels {
            title: Some(title),
            ok: Some("Ok".into()),
            cancel: Some("Abort".into()),
            ..Labels::default()
        },
        ..Config::default()
    };
    config.parse()?;
    let mut secret = SecretBuffer::new()?;
    let event = prompt(&mut config, &mut secret, InterfaceMode::GetPin)?;

    let mut stdout = io::stdout().lock();
    Ok(write_askpass_output(&mut stdout, event, secret.slice())?)
}

fn write_askpass_output(
    writer: &mut dyn Write,
    event: Event,
    pin: Option<&[u8]>,
) -> io::Result<u8> {
    let Some(pin) = pin.filter(|pin| !pin.is_empty()) else {
        return Ok(EXIT_ERROR);
    };
    if event != Event::UserOk {
        return Ok(EXIT_ERROR);
    }
    writer.write_all(pin)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(EXIT_OK)
}

fn run_pinentry() -> Result<u8, Box<dyn std::error::Error>> {
    set_rlimit_core_zero().ok();

    let mut secret = SecretBuffer::new()?;
    let mut config = Config {
        allow_tty_fallback: true,
        ..Config::default()
    };
    config.parse().ok();

    let stdout = io::stdout();
    let mut repl = AssuanRepl::new(stdout.lock())?;
    let stdin = io::stdin();
    let stdin_fd = stdin.as_raw_fd();
    // Assuan commands must be consumed directly from the pollable descriptor.
    // Buffered readers can hide later pipe lines after the first command.
    let mut input = Vec::new();
    let mut stdin_closed = false;
    let mut frontend: Option<(FrontendOwner, i32)> = None;

    while repl.is_running() {
        if let Some((frontend, _)) = frontend.as_mut() {
            if let Some(event) = frontend.flush()? {
                handle_assuan_event(&mut repl, &mut config, &mut secret, frontend, event)?;
            }
        }

        if stdin_closed && repl.mode_is_none() {
            break;
        }

        let mut fds = [libc::pollfd {
            fd: stdin_fd,
            events: libc::POLLIN,
            revents: 0,
        }; 2];
        let (nfds, frontend_index) = match (stdin_closed, frontend.as_ref()) {
            (false, Some((_, fd))) => {
                fds[1].fd = *fd;
                fds[1].events = libc::POLLIN;
                (2, Some(1))
            }
            (false, None) => (1, None),
            (true, Some((_, fd))) => {
                fds[0].fd = *fd;
                fds[0].events = libc::POLLIN;
                (1, Some(0))
            }
            (true, None) => break,
        };
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), nfds, -1) };
        if ret < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err("poll failed".into());
        }

        if !stdin_closed && fds[0].revents & libc::POLLIN != 0 {
            let mut byte = [0_u8; 1];
            let count = unsafe { libc::read(stdin_fd, byte.as_mut_ptr().cast(), 1) };
            if count < 0 {
                return Err(io::Error::last_os_error().into());
            }
            if count == 0 {
                stdin_closed = true;
            } else if byte[0] == b'\n' {
                let line = std::str::from_utf8(&input)?;
                if !line.is_empty() {
                    if frontend.is_none() && repl.requires_frontend(&config, line) {
                        frontend = Some(FrontendOwner::select(&mut config, &mut secret)?);
                    }
                    if let Some((frontend, _)) = frontend.as_mut() {
                        repl.handle_line(&mut config, &mut secret, frontend, line)?;
                    } else {
                        handle_setup_line(&mut repl, &mut config, &mut secret, line)?;
                    }
                }
                input.clear();
            } else {
                input.push(byte[0]);
            }
        }
        if !stdin_closed && fds[0].revents & libc::POLLHUP != 0 {
            stdin_closed = true;
        }

        if let Some(index) = frontend_index {
            if fds[index].revents & libc::POLLIN != 0 {
                let (frontend, _) = frontend.as_mut().expect("polled frontend exists");
                let event = frontend.handle_event()?;
                handle_assuan_event(&mut repl, &mut config, &mut secret, frontend, event)?;
            } else if let Some((frontend, _)) = frontend.as_mut() {
                frontend.no_event()?;
            }
        }
        repl.get_writer().flush()?;
    }

    if let Some((mut frontend, _)) = frontend {
        frontend.deinit();
    }
    Ok(EXIT_OK)
}

fn handle_setup_line(
    repl: &mut AssuanRepl<impl Write>,
    cfg: &mut Config,
    secbuf: &mut SecretBuffer,
    line: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut frontend = SetupFrontend;
    repl.handle_line(cfg, secbuf, &mut frontend, line)?;
    Ok(())
}

fn handle_assuan_event(
    repl: &mut AssuanRepl<impl Write>,
    cfg: &mut Config,
    secbuf: &mut SecretBuffer,
    frontend: &mut FrontendOwner,
    event: Event,
) -> Result<(), Box<dyn std::error::Error>> {
    if event == Event::None {
        return Ok(());
    }
    frontend.enter_mode(InterfaceMode::None)?;
    repl.handle_frontend_event(cfg, secbuf, event)?;
    Ok(())
}

/// Frontend passed only to setup-only Assuan commands. Prompt commands are
/// always intercepted and initialize a real owner first.
struct SetupFrontend;

impl Frontend for SetupFrontend {
    fn init(&mut self, _: &mut Config) -> Result<i32, crate::frontend::FrontendError> {
        unreachable!("setup frontend is never initialized")
    }

    fn deinit(&mut self) {}

    fn enter_mode(&mut self, _: InterfaceMode) -> Result<(), crate::frontend::FrontendError> {
        unreachable!("prompt commands initialize a real frontend")
    }

    fn handle_event(&mut self) -> Result<Event, crate::frontend::FrontendError> {
        unreachable!("setup frontend is never polled")
    }

    fn flush(&mut self) -> Result<Option<Event>, crate::frontend::FrontendError> {
        unreachable!("setup frontend is never polled")
    }

    fn no_event(&mut self) -> Result<(), crate::frontend::FrontendError> {
        unreachable!("setup frontend is never polled")
    }
}

fn prompt(
    cfg: &mut Config,
    secret: &mut SecretBuffer,
    mode: InterfaceMode,
) -> Result<Event, Box<dyn std::error::Error>> {
    set_rlimit_core_zero().ok();
    let (mut frontend, fd) = FrontendOwner::select(cfg, secret)?;
    let result = (|| {
        frontend.enter_mode(mode)?;
        loop {
            if let Some(event) = frontend.flush()? {
                if event != Event::None {
                    frontend.enter_mode(InterfaceMode::None)?;
                    return Ok(event);
                }
            }
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ret = unsafe { libc::poll(&mut pollfd, 1, -1) };
            if ret < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err::<Event, Box<dyn std::error::Error>>("poll failed".into());
            }
            let event = if pollfd.revents & libc::POLLIN != 0 {
                frontend.handle_event()?
            } else {
                frontend.no_event()?;
                Event::None
            };
            if event != Event::None {
                frontend.enter_mode(InterfaceMode::None)?;
                return Ok(event);
            }
        }
    })();
    frontend.deinit();
    result
}

#[derive(Debug, Default)]
struct CliRequest {
    labels: Labels,
    wayland_display: Option<String>,
    get_pin: bool,
    json: bool,
}

enum ParseResult {
    Help,
    Request(CliRequest),
}

fn parse_cli(args: &[OsString]) -> Result<ParseResult, Box<dyn std::error::Error>> {
    let mut request = CliRequest::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let mut value = |request: &mut Option<String>, name: &str| {
            if request.is_some() {
                return Err(format!("redundant '{name}' flag"));
            }
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("flag '{name}' needs an argument"))?;
            *request = Some(value.to_string_lossy().into_owned());
            Ok(())
        };
        match flag.as_ref() {
            "--title" => value(&mut request.labels.title, "--title")?,
            "--description" => value(&mut request.labels.description, "--description")?,
            "--prompt" => value(&mut request.labels.prompt, "--prompt")?,
            "--error" => value(&mut request.labels.err_message, "--error")?,
            "--button-ok" => value(&mut request.labels.ok, "--button-ok")?,
            "--button-not-ok" => value(&mut request.labels.not_ok, "--button-not-ok")?,
            "--button-cancel" => value(&mut request.labels.cancel, "--button-cancel")?,
            "--wayland-display" => value(&mut request.wayland_display, "--wayland-display")?,
            "--get-pin" if !request.get_pin => request.get_pin = true,
            "--json" if !request.json => request.json = true,
            "--help" | "-h" => return Ok(ParseResult::Help),
            "--get-pin" | "--json" => return Err(format!("redundant '{flag}' flag").into()),
            _ => return Err(format!("unknown flag: '{flag}'").into()),
        }
        index += 1;
    }
    Ok(ParseResult::Request(request))
}

fn validate_cli(request: &CliRequest) -> Result<(), Box<dyn std::error::Error>> {
    if !request.get_pin && request.labels.prompt.is_some() {
        return Err("--prompt requires --get-pin".into());
    }
    if !request.get_pin
        && request.labels.title.is_none()
        && request.labels.description.is_none()
        && request.labels.err_message.is_none()
    {
        return Err("a message requires --title, --description, or --error".into());
    }
    Ok(())
}

fn exit_status(event: Event) -> u8 {
    match event {
        Event::UserOk => EXIT_OK,
        Event::UserAbort => EXIT_CANCEL,
        Event::UserNotOk => EXIT_NOT_OK,
        Event::None => EXIT_ERROR,
    }
}

fn write_cli_output(
    writer: &mut dyn Write,
    event: Event,
    pin: Option<&[u8]>,
    get_pin: bool,
    json: bool,
) -> io::Result<()> {
    let action = match event {
        Event::UserOk => "ok",
        Event::UserAbort => "cancel",
        Event::UserNotOk => "not-ok",
        Event::None => return Ok(()),
    };
    if json {
        write!(writer, "{{\n    \"user-action\": \"{action}\"")?;
        if let Some(pin) = pin {
            writer.write_all(b",\n    \"pin\": \"")?;
            write_json_bytes(writer, pin)?;
            writer.write_all(b"\"\n")?;
        } else if get_pin {
            writer.write_all(b",\n    \"pin\": null\n")?;
        } else {
            writer.write_all(b"\n")?;
        }
        writer.write_all(b"}\n")?;
    } else {
        writeln!(writer, "user-action: {action}")?;
        if let Some(pin) = pin {
            writer.write_all(b"pin: ")?;
            writer.write_all(pin)?;
            writer.write_all(b"\n")?;
        } else if get_pin {
            writer.write_all(b"no pin\n")?;
        }
    }
    writer.flush()
}

fn write_json_bytes(writer: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
    for &byte in bytes {
        match byte {
            b'"' => writer.write_all(b"\\\"")?,
            b'\\' => writer.write_all(b"\\\\")?,
            b'\n' => writer.write_all(b"\\n")?,
            b'\r' => writer.write_all(b"\\r")?,
            b'\t' => writer.write_all(b"\\t")?,
            0x00..=0x1f => write!(writer, "\\u{byte:04x}")?,
            _ => writer.write_all(&[byte])?,
        }
    }
    Ok(())
}

fn print_help(writer: &mut dyn Write) -> io::Result<()> {
    writer.write_all(
        b"Usage: nowayprompt [options..]\n\
  --title           <string>   Set the window title\n\
  --description     <string>   Set the description text\n\
  --prompt          <string>   Set the password prompt text\n\
  --error           <string>   Set the error message\n\
  --button-ok       <string>   Set the OK button text\n\
  --button-not-ok   <string>   Set the not-OK button text\n\
  --button-cancel   <string>   Set the cancel button text\n\
  --wayland-display <string>   Select a Wayland display\n\
  --get-pin                    Query for a password\n\
  --json                       Format output as JSON\n\
  --help, -h                   Show this help text\n",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        exit_status, parse_cli, run, validate_cli, write_askpass_output, write_cli_output,
        ParseResult,
    };
    use crate::frontend::Event;
    use std::ffi::OsString;

    #[test]
    fn cli_rejects_prompt_without_secret_mode() {
        let request = match parse_cli(&["--prompt".into(), "PIN".into()]).unwrap() {
            ParseResult::Request(request) => request,
            ParseResult::Help => panic!("unexpected help"),
        };
        assert!(validate_cli(&request).is_err());
    }

    #[test]
    fn cli_rejects_repeated_value_flag() {
        assert!(parse_cli(&[
            OsString::from("--title"),
            OsString::from("one"),
            OsString::from("--title"),
            OsString::from("two"),
        ])
        .is_err());
    }

    #[test]
    fn cli_rejects_unknown_and_argumentless_options() {
        assert!(parse_cli(&[OsString::from("--unknown")]).is_err());
        assert!(parse_cli(&[OsString::from("--title")]).is_err());
    }

    #[test]
    fn cli_json_escapes_secret_without_copying_it() {
        let mut output = Vec::new();
        write_cli_output(&mut output, Event::UserOk, Some(b"a\\\"b"), true, true).unwrap();
        assert_eq!(
            output,
            b"{\n    \"user-action\": \"ok\",\n    \"pin\": \"a\\\\\\\"b\"\n}\n"
        );
    }

    #[test]
    fn cli_cancelled_secret_reports_no_pin() {
        let mut output = Vec::new();
        write_cli_output(&mut output, Event::UserAbort, None, true, false).unwrap();
        assert_eq!(output, b"user-action: cancel\nno pin\n");
    }

    #[test]
    fn cli_exit_statuses_match_contract() {
        assert_eq!(exit_status(Event::UserOk), 0);
        assert_eq!(exit_status(Event::UserAbort), 10);
        assert_eq!(exit_status(Event::UserNotOk), 20);
    }

    #[test]
    fn askpass_emits_only_a_non_empty_accepted_secret() {
        let mut output = Vec::new();
        assert_eq!(
            write_askpass_output(&mut output, Event::UserOk, Some(b"fixture")).unwrap(),
            0
        );
        assert_eq!(output, b"fixture\n");

        output.clear();
        assert_eq!(
            write_askpass_output(&mut output, Event::UserAbort, Some(b"fixture")).unwrap(),
            1
        );
        assert!(output.is_empty());
        assert_eq!(
            write_askpass_output(&mut output, Event::UserOk, Some(b"")).unwrap(),
            1
        );
        assert!(output.is_empty());
    }

    #[test]
    fn unknown_basename_is_rejected_before_frontend_initialization() {
        assert!(run(vec![OsString::from("unrecognized-nowayprompt")]).is_err());
    }
}
