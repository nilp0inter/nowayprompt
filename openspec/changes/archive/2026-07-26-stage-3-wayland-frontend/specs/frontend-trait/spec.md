## MODIFIED Requirements

### Requirement: flush and no_event asymmetry

`flush` MUST return `Ok(None)` for the TTY frontend (blocking frontends have no pending non-blocking events to drain). `no_event` MUST be a no-op for the TTY frontend. The Wayland frontend (Stage 3) MUST override these to drain its event queue and render on idle: `flush` calls `prepare_read` + `display.flush()` and returns a pending `Event` or `Ok(None)`; `no_event` calls `cancel_read` to release the read lock when `poll` showed the frontend fd not readable. This asymmetry is faithful to legacy `Frontend.zig` where `flush`/`noEvent` are Wayland-only and TTY stubs them.

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

### Requirement: Frontend trait interface

The module MUST define a `Frontend` trait that both the TTY fallback (Stage 2) and the Wayland frontend (Stage 3) implement. The trait MUST expose: `init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>` (opens the frontend, returns the poll-able fd); `deinit(&mut self)` (restores terminal/closes resources); `enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>` (enters a UI mode); `handle_event(&mut self) -> Result<Event, FrontendError>` (blocks for or drains an event); `flush(&mut self) -> Result<Option<Event>, FrontendError>` (drains pending non-blocking events; returns `Ok(None)` if none); `no_event(&mut self) -> Result<(), FrontendError>` (called when `poll` returned no frontend event; no-op for blocking frontends). The trait shape is frozen from Stage 2; Stage 3 adds a second implementor (`Wayland`) without reshaping the trait.

#### Scenario: Frontend returns a pollable fd
- **WHEN** `Frontend::init` succeeds
- **THEN** it returns a `RawFd` that can be registered with `poll(2)` alongside stdin

#### Scenario: enter_mode transitions the UI
- **WHEN** `enter_mode(GetPin)` is called on an idle frontend
- **THEN** the frontend renders the pin-entry UI and is ready to accept input events

#### Scenario: Wayland frontend implements Frontend
- **WHEN** the Stage 3 `Wayland` struct is defined
- **THEN** it implements all six `Frontend` trait methods and compiles against the frozen trait