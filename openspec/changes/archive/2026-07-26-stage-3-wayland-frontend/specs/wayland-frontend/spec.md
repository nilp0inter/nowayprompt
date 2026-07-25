## ADDED Requirements

### Requirement: Wayland frontend implements Frontend

The `src/frontend/wayland/mod.rs` module MUST define a `Wayland` struct that implements the `Frontend` trait. `init` MUST connect to the Wayland display (via `WAYLAND_DISPLAY` env or `config.wayland_display`), bind registry globals (`wl_compositor`, `wl_shm`, `wl_seat`, `zwlr_layer_shell_v1`, `wp_cursor_shape_manager_v1`, `wp_fractional_scale_manager_v1`), perform a sync round-trip, initialize the XKB context, and return the `EventQueue` fd for `poll(2)`. `deinit` MUST tear down all globals in legacy order. `enter_mode` MUST defer to `delayed_mode` if the sync callback has not fired (parity with `Wayland.zig:1542-1546`). `flush`/`handle_event`/`no_event` MUST implement the Wayland dispatch triad (D4).

#### Scenario: init connects and returns a pollable fd
- **WHEN** `Wayland::init` is called with a valid `WAYLAND_DISPLAY`
- **THEN** it binds the registry globals, performs a sync round-trip, and returns the `EventQueue` fd as a `RawFd`

#### Scenario: init fails without a display
- **WHEN** `Wayland::init` is called with no `WAYLAND_DISPLAY` and no `config.wayland_display`
- **THEN** it returns `FrontendError::Init` with a message indicating no Wayland display

#### Scenario: enter_mode delayed until sync
- **WHEN** `enter_mode(GetPin)` is called before the sync callback fires
- **THEN** the mode is stored as `delayed_mode` and applied when the sync listener fires

#### Scenario: flush drains outbound and returns pending event
- **WHEN** `flush` is called and the user has pressed Enter (setting `exit_reason`)
- **THEN** it calls `prepare_read` + `display.flush()` and returns `Ok(Some(Event::UserOk))`

#### Scenario: handle_event consumes inbound
- **WHEN** the frontend fd is readable (POLLIN) and `handle_event` is called
- **THEN** it calls `read_events` + `dispatch_pending` and returns the pending `Event` or `Event::None`

#### Scenario: no_event cancels the read
- **WHEN** `no_event` is called after a poll that did not show the frontend fd readable
- **THEN** it calls `cancel_read` to release the read lock and returns `Ok(())`

### Requirement: Registry global binding

The `Wayland` struct MUST bind registry globals in the `registryListener` pattern: `wl_compositor`, `wl_shm`, `wl_seat` (multi-seat, tracked as a list), `zwlr_layer_shell_v1`, `wp_cursor_shape_manager_v1`, and `wp_fractional_scale_manager_v1` (if advertised). A sync round-trip (`display.sync` + `WlCallback`) MUST finalize binding before the frontend enters the event loop. `addSeat` MUST append to a seat list (parity with `Wayland.zig:1737-1738`).

#### Scenario: all globals bound after sync
- **WHEN** the sync callback fires
- **THEN** `compositor`, `shm`, `layer_shell`, `cursor_shape_manager`, and at least one `Seat` are bound

#### Scenario: multiple seats tracked
- **WHEN** the registry advertises two `wl_seat` globals
- **THEN** the `Wayland` struct tracks both seats in its seat list

### Requirement: Layer-shell surface lifecycle

The `Surface` MUST be created via `zwlr_layer_shell_v1.get_layer_surface` with `Layer::Overlay`, anchored on all edges, with keyboard interactivity set to exclusive. It MUST acknowledge the `configure` event with the same serial before committing. `calculateSize` MUST compute width/height from the TextViews and UI config (parity with `Wayland.zig:788-849`). Multi-output Enter/Leave MUST be tracked. `set_buffer_scale` MUST be set per the fractional scale (D8).

#### Scenario: configure serial acknowledged
- **WHEN** the compositor sends a `configure` event with serial N and dimensions WxH
- **THEN** the surface acks serial N, sets its size to WxH, and marks `configured = true`

#### Scenario: initial buffer-less commit
- **WHEN** the surface is created before any buffer is attached
- **THEN** it commits an empty `wl_surface` to trigger the initial configure

### Requirement: exit_reason state machine

The `Wayland` struct MUST track an `exit_reason: Option<FrontendError-like>` set by `abort()` on user input (UserOk/UserAbort/UserNotOk) or error. `flush`/`handle_event` MUST convert a set `exit_reason` into the corresponding `Event` via `exitReasonToReturnVal`, clear it, and call `enter_mode(None)` (parity with `Wayland.zig:1673-1689`).

#### Scenario: UserOk converts to Event
- **WHEN** `exit_reason` is `UserOk` and `flush` or `handle_event` is called
- **THEN** the method returns `Ok(Event::UserOk)`, clears `exit_reason`, and enters `None` mode

#### Scenario: error propagates
- **WHEN** `exit_reason` is a non-user error and `flush` or `handle_event` is called
- **THEN** the method returns `Err(FrontendError)` with the error