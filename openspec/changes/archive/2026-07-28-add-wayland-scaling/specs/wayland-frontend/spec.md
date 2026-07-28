## MODIFIED Requirements

### Requirement: Wayland frontend implements Frontend

The `src/frontend/wayland/mod.rs` module MUST define a `Wayland` struct that
implements the `Frontend` trait. `init` MUST connect to the Wayland display
named by `config.wayland_display` when set, otherwise by `WAYLAND_DISPLAY`. The
selected name MUST be the input to the actual client connection, not merely a
validation check. `init` MUST bind registry globals (`wl_compositor`, `wl_shm`,
`wl_seat`, `wl_output`, `zwlr_layer_shell_v1`,
`wp_cursor_shape_manager_v1`, `wp_fractional_scale_manager_v1`, and
`wp_viewporter`), perform a sync round-trip, initialize the XKB context, and
return the `EventQueue` fd for `poll(2)`. Fractional-scale and viewporter
support MUST be treated as optional and MUST be used together. `deinit` MUST
tear down per-surface protocol objects, buffers, layer shell, cursor manager,
fractional-scale manager, viewporter, outputs, seats, registry, queue, and
connection without leaving live child objects. `enter_mode` MUST defer to
`delayed_mode` if the sync callback has not fired.
`flush`/`handle_event`/`no_event` MUST implement the Wayland dispatch triad.

#### Scenario: explicit configured display connects
- **WHEN** `Config.wayland_display` names an available Wayland socket distinct from `WAYLAND_DISPLAY`
- **THEN** `Wayland::init` connects to the configured socket

#### Scenario: environment display connects
- **WHEN** `Config.wayland_display` is unset and `WAYLAND_DISPLAY` names an available Wayland socket
- **THEN** `Wayland::init` connects to that environment-selected socket

#### Scenario: init connects and returns a pollable fd
- **WHEN** `Wayland::init` is called with a valid `WAYLAND_DISPLAY`
- **THEN** it binds the required globals and all advertised scaling globals, performs a sync round-trip, and returns the `EventQueue` fd as a `RawFd`

#### Scenario: no display is configured
- **WHEN** neither `Config.wayland_display` nor `WAYLAND_DISPLAY` is set
- **THEN** `Wayland::init` returns an initialization error before creating a client connection

#### Scenario: enter_mode delayed until sync
- **WHEN** `enter_mode(GetPin)` is called before the sync callback fires
- **THEN** the mode is stored as `delayed_mode` and applied when the sync listener fires

#### Scenario: flush drains outbound and returns pending event
- **WHEN** `flush` is called and the user has pressed Enter, setting `exit_reason`
- **THEN** it calls `prepare_read` plus `display.flush()` and returns `Ok(Some(Event::UserOk))`

#### Scenario: handle_event consumes inbound
- **WHEN** the frontend fd is readable and `handle_event` is called
- **THEN** it calls `read_events` plus `dispatch_pending` and returns the pending `Event` or `Event::None`

#### Scenario: no_event cancels the read
- **WHEN** `no_event` is called after a poll that did not show the frontend fd readable
- **THEN** it calls `cancel_read` to release the read lock and returns `Ok(())`

### Requirement: Registry global binding

The `Wayland` struct MUST bind required globals and every advertised
`wl_output` in its `Dispatch<WlRegistry, ()>` registry handler. It MUST bind
`wp_fractional_scale_manager_v1` and `wp_viewporter` when advertised, but MUST
only enable fractional scaling when both globals are available. A sync
round-trip (`display.sync` plus `WlCallback`) MUST finalize initial binding
before the frontend enters the event loop. The registry handler MUST append
each advertised `wl_seat`, track each output by registry identity, and remove
an output and its surface membership when the corresponding global is removed.

#### Scenario: required globals bound after sync
- **WHEN** the sync callback fires
- **THEN** `compositor`, `shm`, `layer_shell`, `cursor_shape_manager`, and at least one `Seat` are bound

#### Scenario: multiple seats tracked
- **WHEN** the registry advertises two `wl_seat` globals
- **THEN** the `Wayland` struct tracks both seats in its seat list

#### Scenario: scaling globals are optional
- **WHEN** either `wp_fractional_scale_manager_v1` or `wp_viewporter` is absent
- **THEN** initialization succeeds and the frontend uses integer output scaling

#### Scenario: output global removed
- **WHEN** the registry removes an entered `wl_output` global
- **THEN** the frontend removes that output, recomputes the effective scale, and rerenders if the scale changed

### Requirement: Layer-shell surface lifecycle

The `Surface` MUST be created via
`zwlr_layer_shell_v1.get_layer_surface` with `Layer::Overlay`, anchored on all
edges, with keyboard interactivity set to exclusive. It MUST acknowledge each
`configure` event with the same serial before committing. `calculate_size`
MUST compute logical width and height from the text views and UI configuration.
The surface MUST track `wl_surface.enter` and `wl_surface.leave` membership for
all outputs. Before a fractional preferred scale has been received, the
effective scale MUST be the highest positive integer scale among entered
outputs, or 1 when no entered output has reported a scale. Once a fractional
preferred scale is received for a surface with viewporter support, it MUST take
precedence. Any event that changes the effective scale MUST schedule a render
at the new scale without changing logical surface geometry.

#### Scenario: configure serial acknowledged
- **WHEN** the compositor sends a `configure` event with serial N and dimensions W by H
- **THEN** the surface acknowledges serial N, records its logical size, and marks `configured = true`

#### Scenario: initial buffer-less commit
- **WHEN** the surface is created before any buffer is attached
- **THEN** it commits an empty `wl_surface` to trigger initial configure and scaling events

#### Scenario: highest entered-output fallback selected
- **WHEN** a surface has entered outputs with integer scales 1 and 2 and no fractional preferred scale is active
- **THEN** its effective scale is 2

#### Scenario: output migration changes fallback scale
- **WHEN** a surface leaves its only 2× output and remains on a 1× output
- **THEN** its effective scale becomes 1 and a new 1× buffer is rendered

#### Scenario: fractional preferred scale takes precedence
- **WHEN** the surface has viewporter support and receives `preferred_scale(180)`
- **THEN** its effective scale becomes 180/120 regardless of entered-output integer scales and it rerenders at 1.5×

#### Scenario: scaling event precedes configure
- **WHEN** an output or preferred-scale event arrives before the first layer-surface configure
- **THEN** the scale is retained and applied to the first render after configure
