## ADDED Requirements

### Requirement: Deferred concrete frontend selection

The frontend module MUST provide a concrete owner that delegates the existing
`Frontend` trait to either `Wayland` or `Tty`. CLI mode MUST create this owner
after parsing valid arguments. Pinentry mode MUST create it immediately before
the first `GETPIN`, `CONFIRM`, or non-empty `MESSAGE` request that needs a
frontend, after preceding Assuan setup options have updated `Config`.

The selector MUST attempt Wayland first. It MAY select TTY only when the
Wayland display is absent or the initial connection fails and
`Config.allow_tty_fallback` is true. It MUST propagate missing-global,
protocol, input, rendering, and post-connection I/O failures without falling
back. `deinit` MUST delegate to the selected frontend exactly once.

#### Scenario: Assuan options configure frontend selection
- **WHEN** an Assuan client sends `OPTION putenv=WAYLAND_DISPLAY=<display>`
  and `OPTION ttyname=<tty>` before `GETPIN`
- **THEN** the subsequent frontend initialization uses those values

#### Scenario: unavailable Wayland display falls back
- **WHEN** no display is available, `allow_tty_fallback` is true, and a valid
  TTY name is configured
- **THEN** the selector initializes `Tty`

#### Scenario: Wayland protocol error does not fall back
- **WHEN** a Wayland connection succeeds but a required global is absent
- **THEN** prompt initialization fails and does not initialize `Tty`
