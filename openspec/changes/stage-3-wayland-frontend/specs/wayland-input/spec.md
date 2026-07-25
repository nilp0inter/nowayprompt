## ADDED Requirements

### Requirement: XKB keymap compilation from mmap'd fd

The `src/frontend/wayland/input.rs` module MUST handle the `wl_keyboard.keymap` event by memory-mapping the fd with `memmap2::MmapOptions::new().len(size).map(&file)` (`MAP_PRIVATE` read-only, matching legacy), compiling the keymap via `xkbcommon::xkb::Keymap::new_from_string` with `KEYMAP_FORMAT_TEXT_V1`, creating an `xkb::State`, and dropping the mmap (parity with `Wayland.zig:445-474`). No `SIGBUS` guard (D2).

#### Scenario: keymap compiled from fd
- **WHEN** a `wl_keyboard.keymap` event arrives with format `xkb_v1`, fd F, size S
- **THEN** the fd is mmap'd `MAP_PRIVATE` read-only, `Keymap::new_from_string` compiles it, and the mmap is dropped

#### Scenario: unsupported keymap format
- **WHEN** the keymap format is not `xkb_v1`
- **THEN** the frontend aborts with an unsupported-format error (parity with `Wayland.zig:447-450`)

### Requirement: Modifier state sync via wl_keyboard.modifiers

The `Seat` MUST update its `xkb::State` via `state.update_mask(depressed, latched, locked, 0, 0, group)` on each `wl_keyboard.modifiers` event (parity with `Wayland.zig:475-479`). The client MUST NOT derive modifiers from physical key events.

#### Scenario: modifier mask applied
- **WHEN** a `modifiers` event arrives with depressed Ctrl
- **THEN** `xkb::State::update_mask` is called and subsequent `key_get_one_sym` reflects the Ctrl modifier

### Requirement: Evdev keycode +8 offset and keysym lookup

On a `wl_keyboard.key` press event, the module MUST add 8 to the raw keycode, call `xkb::State::key_get_one_sym`, and dispatch on the keysym (parity with `Wayland.zig:480-528`). Ctrl+BackSpace/u/w clears the pin buffer; Return → `UserOk`; Escape → `UserAbort`; BackSpace deletes one char; Delete is a no-op; other keys append UTF-8 via `key_get_utf8` to the `SecretBuffer`.

#### Scenario: Return emits UserOk
- **WHEN** the user presses Return (keysym `Return`)
- **THEN** the frontend sets `exit_reason = UserOk`

#### Scenario: Escape emits UserAbort
- **WHEN** the user presses Escape (keysym `Escape`)
- **THEN** the frontend sets `exit_reason = UserAbort`

#### Scenario: BackSpace deletes one pin char
- **WHEN** the user presses BackSpace in `GetPin` mode
- **THEN** `SecretBuffer::delete_backwards` is called and the surface re-renders

#### Scenario: Ctrl+u clears the pin buffer
- **WHEN** the user presses Ctrl+u in `GetPin` mode
- **THEN** `SecretBuffer::reset` is called and the surface re-renders

#### Scenario: Unicode key appends to pin
- **WHEN** the user presses a Unicode key in `GetPin` mode
- **THEN** `key_get_utf8` output is appended to the `SecretBuffer` and the surface re-renders

### Requirement: Pointer and touch hotspot hit-testing

The `Seat` MUST bind `wl_pointer` and `wl_touch`, track motion/down/up events, and on a button press/touch-down consult the `Surface` hotspot list to determine the `Effect` (cancel/notok/ok), mapping it to `UserAbort`/`UserNotOk`/`UserOk` (parity with `Wayland.zig:285-635`). Cursor shape MUST be set via `wp_cursor_shape_manager_v1`.

#### Scenario: click on OK hotspot emits UserOk
- **WHEN** a pointer button press lands within the OK button's hotspot rectangle
- **THEN** the frontend sets `exit_reason = UserOk`

#### Scenario: touch on cancel hotspot emits UserAbort
- **WHEN** a touch-down event lands within the cancel button's hotspot rectangle
- **THEN** the frontend sets `exit_reason = UserAbort`

#### Scenario: cursor shape set on pointer enter
- **WHEN** the pointer enters the surface
- **THEN** the cursor shape is set to `Pointer` via `wp_cursor_shape_device_v1`

### Requirement: xkbcommon C-dlopen linkage

The `xkbcommon` crate (v0.8+) MUST be used via its `xkbcommon-dl` variant that dlopens `libxkbcommon.so` at runtime. This is the single recorded exception to the pure-Rust invariant (D3).

#### Scenario: libxkbcommon loaded at runtime
- **WHEN** the `Wayland` frontend initializes the XKB context
- **THEN** `libxkbcommon.so` is dlopen'd at runtime (not linked at build time)