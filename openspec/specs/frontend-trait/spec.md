## Purpose

Defines the shared frontend abstraction, selection policy, and user events.

## Requirements

### Requirement: Frontend trait interface

The module MUST define a `Frontend` trait that both the TTY fallback and the Wayland frontend implement. The trait MUST expose: `init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>` (opens the frontend, returns the poll-able fd); `deinit(&mut self)` (restores terminal/closes resources); `enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>` (enters a UI mode); `handle_event(&mut self) -> Result<Event, FrontendError>` (blocks for or drains an event); `flush(&mut self) -> Result<Option<Event>, FrontendError>` (drains pending non-blocking events; returns `Ok(None)` if none); `no_event(&mut self) -> Result<(), FrontendError>` (called when `poll` returned no frontend event; no-op for blocking frontends). The trait shape is frozen: both implementors (`Tty` and `Wayland`) share exactly these six methods.

#### Scenario: Frontend returns a pollable fd
- **WHEN** `Frontend::init` succeeds
- **THEN** it returns a `RawFd` that can be registered with `poll(2)` alongside stdin

#### Scenario: enter_mode transitions the UI
- **WHEN** `enter_mode(GetPin)` is called on an idle frontend
- **THEN** the frontend renders the pin-entry UI and is ready to accept input events

#### Scenario: Wayland frontend implements Frontend
- **WHEN** the `Wayland` frontend struct is defined
- **THEN** it implements all six `Frontend` trait methods and compiles against the frozen trait

### Requirement: Event enum

The module MUST define an `Event` enum with variants `None`, `UserOk`, `UserAbort`, `UserNotOk`. `None` indicates no terminal user action (used by `flush` and as a no-op sentinel). `UserOk` indicates the user confirmed (pressed Enter). `UserAbort` indicates the user cancelled (pressed Escape). `UserNotOk` indicates the user pressed the "not OK" button (Ctrl+C when a `not_ok` label is set).

#### Scenario: UserOk from Enter key
- **WHEN** the user presses Enter during a `GetPin` prompt
- **THEN** `handle_event` returns `Ok(Event::UserOk)`

#### Scenario: UserAbort from Escape key
- **WHEN** the user presses Escape during a prompt
- **THEN** `handle_event` returns `Ok(Event::UserAbort)`

#### Scenario: UserNotOk from Ctrl+C with not_ok label
- **WHEN** the user presses Ctrl+C during a prompt and `config.labels.not_ok` is `Some`
- **THEN** `handle_event` returns `Ok(Event::UserNotOk)`

### Requirement: InterfaceMode enum

The module MUST define an `InterfaceMode` enum with variants `None`, `GetPin`, `Message`. `None` is the idle state. `GetPin` renders the pin-entry input row. `Message` renders the confirm/message display. The `confirm` and `message` Assuan modes collapse into the same frontend `Message` mode, matching the pinned `pkgs.wayprompt` oracle.

#### Scenario: GetPin mode renders pin input
- **WHEN** `enter_mode(GetPin)` is called
- **THEN** the frontend renders the ` > ` prompt with pin squares

#### Scenario: Message mode renders display-only
- **WHEN** `enter_mode(Message)` is called
- **THEN** the frontend renders title/description/error without a pin input row

### Requirement: flush and no_event asymmetry

`flush` MUST return `Ok(None)` for the TTY frontend (blocking frontends have no pending non-blocking events to drain). `no_event` MUST be a no-op for the TTY frontend. The Wayland frontend MUST override these to drain its event queue and render on idle: `flush` calls `prepare_read` + `display.flush()` and returns a pending `Event` or `Ok(None)`; `no_event` calls `cancel_read` to release the read lock when `poll` showed the frontend fd not readable. This asymmetry matches the pinned oracle contract: the flush/idle hooks are meaningful only on the event-driven Wayland frontend; the TTY frontend stubs them.

#### Scenario: TTY flush returns None
- **WHEN** `flush()` is called on the TTY frontend
- **THEN** the result is `Ok(None)` (no event drained)

#### Scenario: TTY no_event is a no-op
- **WHEN** `no_event()` is called on the TTY frontend
- **THEN** the method returns `Ok(())` with no side effects

#### Scenario: Wayland flush drains outbound and returns pending event
- **WHEN** `flush()` is called on the Wayland frontend and `exit_reason` is set
- **THEN** it calls `prepare_read` + `display.flush()` and returns `Ok(Some(Event))` corresponding to `exit_reason`

#### Scenario: Wayland no_event cancels the read
- **WHEN** `no_event()` is called on the Wayland frontend after a poll with no frontend fd activity
- **THEN** it calls `cancel_read` and returns `Ok(())`

### Requirement: FrontendError type

The module MUST define a `FrontendError` enum covering: `Init(String)` (frontend initialization failure, e.g. no TTY available), `Io(std::io::Error)` (I/O failure during render or read), `InvalidMode(String)` (invalid mode transition). The error MUST implement `std::error::Error` and `Display`.

#### Scenario: No TTY name set
- **WHEN** `init` is called and `config.tty_name` is `None`
- **THEN** the result is `Err(FrontendError::Init(_))`

### Requirement: Deferred concrete frontend selection

The frontend module MUST provide a concrete owner that delegates the existing `Frontend` trait to either `Wayland` or `Tty`. CLI mode MUST create this owner after parsing valid arguments. Pinentry mode MUST create it immediately before the first `GETPIN`, `CONFIRM`, or non-empty `MESSAGE` request that needs a frontend, after preceding Assuan setup options have updated `Config`.

The selector MUST attempt Wayland first. It MAY select TTY only when the Wayland display is absent or the initial connection fails and `Config.allow_tty_fallback` is true. It MUST propagate missing-global, protocol, input, rendering, and post-connection I/O failures without falling back. `deinit` MUST delegate to the selected frontend exactly once.

#### Scenario: Assuan options configure frontend selection
- **WHEN** an Assuan client sends `OPTION putenv=WAYLAND_DISPLAY=<display>` and `OPTION ttyname=<tty>` before `GETPIN`
- **THEN** the subsequent frontend initialization uses those values

#### Scenario: unavailable Wayland display falls back
- **WHEN** no display is available, `allow_tty_fallback` is true, and a valid TTY name is configured
- **THEN** the selector initializes `Tty`

#### Scenario: Wayland protocol error does not fall back
- **WHEN** a Wayland connection succeeds but a required global is absent
- **THEN** prompt initialization fails and does not initialize `Tty`