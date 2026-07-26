## Purpose

Defines the terminal fallback frontend, raw-mode lifecycle, and ANSI rendering.

## Requirements

### Requirement: Raw termios mode with RAII restoration

The TTY frontend MUST enter raw mode on `enter_mode` (when transitioning from `None` to an active mode) by calling `libc::tcgetattr(fd, &orig)`, clearing `ECHO | ICANON | ISIG` from `c_lflag`, setting `c_cc[VMIN] = 1` and `c_cc[VTIME] = 0`, and calling `libc::tcsetattr(fd, TCSAFLUSH, &raw)`. The original `termios` MUST be saved and restored on `deinit` (or `enter_mode(None)`) via `tcsetattr(fd, TCSAFLUSH, &orig)`. The fd comes from `config.tty_name` (opened `O_RDWR`); if `tty_name` is `None`, `init` returns `FrontendError::Init`.

#### Scenario: Raw mode clears ECHO, ICANON, ISIG
- **WHEN** `enter_mode(GetPin)` is called
- **THEN** `tcgetattr` saves the original termios, `tcsetattr` applies a copy with `ECHO`, `ICANON`, and `ISIG` cleared and `VMIN=1`, `VTIME=0`

#### Scenario: Termios restored on deinit
- **WHEN** `deinit` is called after raw mode was entered
- **THEN** `tcsetattr(fd, TCSAFLUSH, &orig)` restores the original termios

#### Scenario: No tty_name is an error
- **WHEN** `init` is called and `config.tty_name` is `None`
- **THEN** the result is `Err(FrontendError::Init(_))`

### Requirement: Signal-based termios restoration on termination signals

The module MUST register signal handlers (via `signal-hook`) for `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`, `SIGTSTP` that restore the saved `termios` via `tcsetattr(fd, TCSAFLUSH, &orig)` and call `libc::_exit(0)`. The handler MUST be async-signal-safe: no heap allocation, no locks, no std I/O. The fd and a pointer to the saved `termios` MUST be stored in `static` atomics accessible to the handler. This prevents a raw-mode terminal from being left in a corrupted state if the process is killed mid-prompt.

#### Scenario: SIGINT restores termios
- **WHEN** the process receives `SIGINT` while in raw mode
- **THEN** the signal handler restores the original termios and calls `_exit(0)`, leaving the terminal usable

#### Scenario: SIGTERM during prompt
- **WHEN** the process receives `SIGTERM` while in raw mode
- **THEN** the signal handler restores the original termios before exit

### Requirement: Terminal size query via ioctl

On `enter_mode` (transitioning into an active mode), the frontend MUST query the terminal size via `libc::ioctl(fd, TIOCGWINSZ, &mut winsize)` and store `width` and `height`. If the width is less than 5 or the height is less than 5, the frontend MUST render only the message `Terminal too small!` (bold, red) and not render the prompt UI.

#### Scenario: Terminal too small
- **WHEN** `enter_mode` queries the size and width < 5 or height < 5
- **THEN** the render output is only `Terminal too small!` (bold red text)

#### Scenario: Normal terminal size
- **WHEN** `enter_mode` queries the size and width >= 5 and height >= 5
- **THEN** the full prompt UI is rendered

### Requirement: ANSI rendering layout

The `render` function MUST write to stdout in order: (1) clear screen + cursor home (`\x1b[2J\x1b[H`); (2) if `config.labels.title` is `Some`, render it with bold + green background + black foreground, space-padded to width; (3) if `config.labels.description` is `Some`, render it with default attributes, space-padded; (4) if `config.labels.prompt` is `Some`, render it bold, space-padded; (5) if mode is `GetPin`, render a line ` > ` followed by `*` repeated `min(pin_square_amount, len)` times and `_` repeated `pin_square_amount - len` times (where `len` is `SecretBuffer::len()` and `pin_square_amount` is `config.wayland_ui.pin_square_amount`); (6) if `config.labels.err_message` is `Some`, render it bold + red foreground; (7) if `config.labels.ok` is `Some`, render a button line `enter: <ok>`; (8) if `config.labels.not_ok` is `Some`, render `C-c: <not_ok>`; (9) if `config.labels.cancel` is `Some`, render `escape: <cancel>`. Multi-line label strings MUST wrap at the terminal width. Each label section is followed by a blank line.

#### Scenario: GetPin render with secret
- **WHEN** mode is `GetPin`, `len` is 3, `pin_square_amount` is 8, and labels are set
- **THEN** the pin row renders ` > ***_____` (3 stars, 5 underscores)

#### Scenario: Title rendered with green background
- **WHEN** `config.labels.title` is `Some("Login")` and the terminal is 20 cols wide
- **THEN** the title line is bold, green background, black foreground, space-padded to 20 columns

#### Scenario: Buttons rendered with key prefixes
- **WHEN** `ok`, `not_ok`, and `cancel` labels are all `Some`
- **THEN** three button lines render: `enter: <ok>`, `C-c: <not_ok>`, `escape: <cancel>`

### Requirement: Hand-rolled input parser

The frontend MUST parse raw bytes from `libc::read(fd, buf, n)` into `TtyInput` events without a third-party input library. Recognized inputs: `\r` or `\n` → `Enter`; `\x1b` (standalone, not part of a longer escape sequence within the read buffer) → `Escape`; `\x7f` → `Backspace`; `\x03` → `C-c`; `\x15` → `C-u`; `\x17` → `C-w`; `\x08` → `C-backspace`. UTF-8 codepoint bytes (lead byte `0xxxxxxx`, `110xxxxx`, `1110xxxx`, or `11110xxx` followed by the correct number of `10xxxxxx` continuation bytes) → `Codepoint(char)` decoded via `std::str::from_utf8`. Modified codepoints (Alt/Ctrl/Super) MUST be ignored (dropped). Unrecognized escape sequences (`\x1b[A`, `\x1b[B`, etc.) MUST be consumed and dropped as `Unknown`, NOT appended to the secret buffer (dropping prevents terminal control bytes from contaminating the secret).

#### Scenario: Enter key
- **WHEN** the read buffer is `b"\r"`
- **THEN** the parser yields `TtyInput::Enter`

#### Scenario: Backspace key
- **WHEN** the read buffer is `b"\x7f"`
- **THEN** the parser yields `TtyInput::Backspace`

#### Scenario: UTF-8 codepoint
- **WHEN** the read buffer is `b"\xC3\xA9"` (é)
- **THEN** the parser yields `TtyInput::Codepoint('é')`

#### Scenario: Arrow key dropped
- **WHEN** the read buffer is `b"\x1b[A"` (arrow up)
- **THEN** the parser yields `TtyInput::Unknown` (dropped, not appended to secret)

#### Scenario: Modified codepoint ignored
- **WHEN** the read buffer is `b"\x1ba"` (Alt+a)
- **THEN** the parser yields `TtyInput::Unknown` (dropped)

### Requirement: Input event handling per mode

On `handle_event`, the frontend MUST read raw bytes and run the input parser. For each parsed input: `Enter` → return `Event::UserOk`; `Escape` → return `Event::UserAbort`; `C-c` → if `config.labels.not_ok` is `Some`, return `Event::UserNotOk`, else return `Event::UserAbort`; `C-u` / `C-w` / `C-backspace` → if mode is `GetPin`, call `SecretBuffer::reset()` and re-render; `Backspace` → if mode is `GetPin`, call `SecretBuffer::delete_backwards()` and re-render; `Codepoint(c)` → if mode is `GetPin`, encode `c` as UTF-8 and call `SecretBuffer::append_slice(bytes)`, then re-render. Inputs in `Message` mode other than `Enter`/`Escape`/`C-c` MUST be ignored. After a terminal event (`UserOk`/`UserAbort`/`UserNotOk`), the frontend returns the event and the dispatch loop calls `enter_mode(None)` / `deinit` as appropriate.

#### Scenario: Enter returns UserOk
- **WHEN** mode is `GetPin` and the user presses Enter
- **THEN** `handle_event` returns `Ok(Event::UserOk)`

#### Scenario: C-u clears the secret
- **WHEN** mode is `GetPin`, the secret buffer holds `"abc"`, and the user presses Ctrl+U
- **THEN** `SecretBuffer::reset()` is called, the buffer is empty, and the UI re-renders

#### Scenario: Codepoint appended in GetPin
- **WHEN** mode is `GetPin` and the user types `a`
- **THEN** `SecretBuffer::append_slice(b"a")` is called and the UI re-renders with the updated pin row

#### Scenario: C-c with not_ok label
- **WHEN** mode is active, `config.labels.not_ok` is `Some`, and the user presses Ctrl+C
- **THEN** `handle_event` returns `Ok(Event::UserNotOk)`

#### Scenario: C-c without not_ok label
- **WHEN** mode is active, `config.labels.not_ok` is `None`, and the user presses Ctrl+C
- **THEN** `handle_event` returns `Ok(Event::UserAbort)`

### Requirement: No SIGWINCH handling

The frontend MUST NOT handle `SIGWINCH`; resize tracking during an active prompt is intentionally not implemented. Terminal resize during a prompt leaves the layout stale until the next input event triggers a re-render. The frontend re-queries size on each `enter_mode` call, so a new prompt after resize gets the correct dimensions.

#### Scenario: Resize during prompt does not re-render
- **WHEN** the terminal is resized while a `GetPin` prompt is active and the user has not pressed a key
- **THEN** the UI remains at the pre-resize layout until the next keypress triggers a re-render