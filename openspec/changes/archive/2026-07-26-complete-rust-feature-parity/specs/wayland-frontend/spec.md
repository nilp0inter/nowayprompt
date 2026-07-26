## MODIFIED Requirements

### Requirement: Wayland frontend implements Frontend

The `src/frontend/wayland/mod.rs` module MUST define a `Wayland` struct that
implements the `Frontend` trait. `init` MUST connect to the Wayland display
named by `config.wayland_display` when set, otherwise by `WAYLAND_DISPLAY`.
The selected name MUST be the input to the actual client connection, not merely
a validation check. `init` MUST bind registry globals (`wl_compositor`,
`wl_shm`, `wl_seat`, `zwlr_layer_shell_v1`, `wp_cursor_shape_manager_v1`,
`wp_fractional_scale_manager_v1`), perform a sync round-trip, initialize the
XKB context, and return the `EventQueue` fd for `poll(2)`. `deinit` MUST tear
down all globals in legacy order (surfaces, buffers, layer shell, cursor
manager, fractional-scale manager, seats, registry, queue, connection).

#### Scenario: explicit configured display connects
- **WHEN** `Config.wayland_display` names an available Wayland socket distinct
  from `WAYLAND_DISPLAY`
- **THEN** `Wayland::init` connects to the configured socket

#### Scenario: environment display connects
- **WHEN** `Config.wayland_display` is unset and `WAYLAND_DISPLAY` names an
  available socket
- **THEN** `Wayland::init` connects to that environment-selected socket

#### Scenario: no display is configured
- **WHEN** neither `Config.wayland_display` nor `WAYLAND_DISPLAY` is set
- **THEN** `Wayland::init` returns an initialization error before creating a
  client connection
