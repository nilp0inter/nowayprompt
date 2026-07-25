## Why

Stage 2 delivered the Assuan IPC + TTY fallback frontend and froze the poll-based `Frontend` trait contract. The legacy `wayprompt` is primarily a Wayland layer-shell pinentry; the TTY fallback is secondary. Stage 3 implements the Wayland frontend as a library implementing `Frontend`, restoring the primary UI path. It is deliberately scoped as additive-only (no `main.rs` wiring, no end-to-end pinentry binary) so the phase stays small; Stage 4 wires frontend selection into the entrypoint and ships the full binary.

## What Changes

- Add `src/frontend/wayland/{mod,shm,render,input}.rs` implementing the frozen `Frontend` trait (`init`/`deinit`/`enter_mode`/`handle_event`/`flush`/`no_event`) against the pure-Rust Wayland/graphics stack.
- Add `wayland-client` (v0.31+, pure-Rust `rs` socket backend), `wayland-protocols-wlr` (v0.3+, `zwlr_layer_shell_v1`), `wayland-protocols` (core, `wp_fractional_scale_manager_v1`), `tiny-skia`, `cosmic-text`, and `xkbcommon` (C-dlopen, the sole acceptable exception to the pure-Rust invariant — no pure-Rust XKB exists) to `Cargo.toml`.
- Render parity is **behavioral only**: surface geometry, layout box positions, event flow, hotspot hit-testing. Pixel-identical output to legacy `fcft`+`pixman` is explicitly out of scope; `cosmic-text`+`tiny-skia` cannot match those rasterizers. Stage 4's `grim`-based nixosTest gate must be reframed as tolerance/perceptual, not byte-identical.
- Add a second `[[bin]]` target (`nowayprompt-wayland-test`) that instantiates `Wayland::init()` directly, used only by the Stage 3 nixosTest. Does NOT wire into `main.rs`; `main.rs` stays TTY-only until Stage 4.
- Add a geometry-only `nixosTests.stage-3-wayland` that runs the test binary under a headless `cage` compositor (`WLR_BACKEND=headless`, `WLR_RENDERER=pixman`) with `wtype` synthetic keyboard input, asserting layer-shell configure-ack, surface dimensions, scale, and keyboard-driven `Event` emission. No `grim` pixel capture (Stage 4's contract). The harness is reusable by Stage 4.
- Keymap fd: `memmap2` `MAP_PRIVATE` read-only mmap matching legacy; no `SIGBUS` guard (match legacy threat model; record the secret-leak risk as a deferred hardening in `design.md`).
- Bundle a fallback font via `include_bytes!` (DejaVu Sans / Fira Mono) and load via `fontdb::load_font_data`; do NOT call `fontdb::load_system_fonts()` (startup latency). Disable subpixel AA (force grayscale) to avoid chromatic aberration on transparent backgrounds.
- Pin mask glyph: shape the single `•`/`*` once, cache, blit iteratively per keystroke (avoid re-running the cosmic-text shaper per key).

## Capabilities

### New Capabilities
- `wayland-frontend`: The `Wayland` struct implementing the `Frontend` trait — Wayland connection, registry/sync global binding, layer-shell surface lifecycle, SHM buffer pool, software render pipeline, seat input (keyboard/pointer/touch), and the `flush`/`handle_event`/`no_event` dispatch triad parity with `legacy/src/Wayland.zig`.
- `wayland-shm-buffer`: `memfd_create` + `memmap2::MmapMut` SHM buffer pool with triple-buffer `max_buffer_multiplicity=3`, `wl_buffer.release` busy-state tracking via a `Vec<Buffer>` arena + `Dispatch` user-data index, and `Argb8888` format (matching legacy).
- `wayland-render`: `tiny-skia` + `cosmic-text` software render pipeline producing behavioral (not pixel) parity with legacy `pixman`+`fcft` — bordered/rounded rectangles, text layout, pin squares, button hotspots, premultiplied RGBA→BGRA byte swap before commit.
- `wayland-input`: `xkbcommon` keymap mmap + compile, modifier state sync via `wl_keyboard.modifiers`, evdev +8 keycode offset, keysym/utf8 lookup, pointer/touch hotspot hit-testing, `wp_cursor_shape` cursor management.
- `wayland-parity-testing`: Geometry-only `nixosTests.stage-3-wayland` under headless `cage` + `wtype`, exercising the test-only `[[bin]]` target. Asserts surface geometry, configure-ack, scale, and keyboard `Event` emission. Reusable by Stage 4 (adds `grim` + real pinentry).

### Modified Capabilities
- `frontend-trait`: No spec-level change to the trait (frozen in Stage 2). The delta records that a second implementor (`Wayland`) now exists alongside `Tty`, and that `flush`/`no_event` are no longer TTY-only no-ops — they carry real Wayland dispatch semantics. This is an additive requirement (a new implementor), not a trait reshape.