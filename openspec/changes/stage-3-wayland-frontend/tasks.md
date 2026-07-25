## 1. Dependencies and scaffold

- [ ] 1.1 Add `wayland-client` (v0.31+, default `rs` backend), `wayland-protocols-wlr` (v0.3+), `wayland-protocols` (core, for `wp_fractional_scale_manager_v1`), `tiny-skia`, `cosmic-text`, and `xkbcommon` (v0.8+, `xkbcommon-dl` variant) to `Cargo.toml`; verify `cargo build` is clean.
- [ ] 1.2 Create `src/frontend/wayland/{mod.rs, shm.rs, render.rs, input.rs}` module skeleton with `mod` declarations wired into `src/frontend/mod.rs` (`pub mod wayland;` + `pub use wayland::Wayland;`).
- [ ] 1.3 Bundle a fallback font (DejaVu Sans and Fira Mono TTFs) under `assets/` and add `include_bytes!` constants in `render.rs`.

## 2. Wayland connection and registry (mod.rs)

- [ ] 2.1 Implement `Wayland::init`: `Connection::connect_to_env()` (or `config.wayland_display`), `new_event_queue()`, return the queue fd as `RawFd`.
- [ ] 2.2 Implement the `Dispatch<WlRegistry, ()>` impl binding `wl_compositor`, `wl_shm`, `wl_seat` (multi-seat list), `zwlr_layer_shell_v1`, `wp_cursor_shape_manager_v1`, `wp_fractional_scale_manager_v1`.
- [ ] 2.3 Implement the sync round-trip (`display.sync` + `WlCallback`) that finalizes global binding and flushes `delayed_mode`.
- [ ] 2.4 Implement `Wayland::deinit` tearing down all globals in legacy order (`Wayland.zig:1502-1533`).
- [ ] 2.5 Implement `Wayland::enter_mode` with the `delayed_mode` deferral when `sync` is non-null (parity `Wayland.zig:1535-1564`).
- [ ] 2.6 Implement the `exit_reason` state machine: `abort()` sets it; `flush`/`handle_event` convert it to `Event` via `exitReasonToReturnVal`, clear it, and call `enter_mode(None)`.

## 3. Frontend trait dispatch triad (mod.rs)

- [ ] 3.1 Implement `flush`: `prepare_read` loop + `display.flush()` with EAGAIN/PIPE handling; return pending `Event` or `Ok(None)` (parity `Wayland.zig:1612-1643`).
- [ ] 3.2 Implement `handle_event`: `read_events` + `dispatch_pending`; return pending `Event` or `Event::None` (parity `Wayland.zig:1645-1667`).
- [ ] 3.3 Implement `no_event`: `cancel_read` (parity `Wayland.zig:1669-1671`).
- [ ] 3.4 Verify the `Wayland` struct compiles against the frozen `Frontend` trait without reshaping it.

## 4. SHM buffer pool (shm.rs)

- [ ] 4.1 Implement `Buffer::init`: `libc::memfd_create` + `MFD_CLOEXEC`, `ftruncate`, `memmap2::MmapMut` (`MAP_SHARED`, `PROT_READ|PROT_WRITE`), `wl_shm.create_pool`, `pool.create_buffer` with `Format::Argb8888` and stride `width*4`.
- [ ] 4.2 Implement the `Dispatch<WlBuffer, usize>` impl: the `.release` event flips `state.buffers[idx].busy = false` via the user-data index.
- [ ] 4.3 Implement `BufferPool::next_buffer`: reuse idle matching-size, else re-init idle mismatched-size, else allocate new.
- [ ] 4.4 Implement `BufferPool::cull_buffers`: destroy idle buffers exceeding `max_buffer_multiplicity=3`.
- [ ] 4.5 Implement `Buffer::deinit` and `BufferPool::deinit` (unref pixmap, destroy wl_buffer, drop MmapMut).

## 5. Render pipeline (render.rs)

- [ ] 5.1 Implement `Surface::render` skeleton: acquire buffer from pool, wrap `MmapMut` slice in `tiny_skia::PixmapMut`, skip if not `configured`.
- [ ] 5.2 Implement `draw_background`: `fill_rect` (background) + `stroke_path` (border) + rounded corners via `PathBuilder` when `corner_radius > 0`.
- [ ] 5.3 Implement `TextView` with `cosmic-text::Buffer` + `FontSystem` + `SwashCache`; expose `width`/`height` metrics; font fallback chain `[user_font, "sans:size=14", "mono:size=14"]` via `fontdb` query.
- [ ] 5.4 Implement the custom `Renderer` blending straight-alpha swash pixels onto the premultiplied `tiny-skia` pixmap; force grayscale AA (disable subpixel).
- [ ] 5.5 Implement `draw_pin_area`: shape the mask glyph once, cache it, blit iteratively per pin square (no per-keystroke shaping).
- [ ] 5.6 Implement button drawing (ok/notok/cancel) with `borderedRectangle` + TextView draw; populate hotspots on first render.
- [ ] 5.7 Implement the in-place R/B byte swap (SIMD `u32` bitwise form) converting premultiplied RGBA to `Argb8888` before commit.
- [ ] 5.8 Implement `Surface::calculate_size` computing width/height from TextViews and UI config (parity `Wayland.zig:788-849`).
- [ ] 5.9 Implement the layer-shell surface lifecycle: `get_layer_surface` (Layer::Overlay, all anchors, exclusive keyboard), configure serial ack, initial buffer-less commit, `set_buffer_scale` per fractional scale.
- [ ] 5.10 Implement fractional scaling: bind `wp_fractional_scale_manager_v1`, honor `preferred_scale`, scale layout + font metrics, `set_buffer_scale(1)`.

## 6. Seat input (input.rs)

- [ ] 6.1 Implement `Seat::init` + `Dispatch<WlSeat, ()>` binding keyboard/pointer/touch on capability flags.
- [ ] 6.2 Implement `Dispatch<WlKeyboard, ()>`: `.keymap` event → `memmap2` `MAP_PRIVATE` read-only mmap + `Keymap::new_from_string` + `State::new` (no SIGBUS guard, D2).
- [ ] 6.3 Implement the `.modifiers` event → `State::update_mask`.
- [ ] 6.4 Implement the `.key` event: keycode+8, `key_get_one_sym`, ctrl-mod detection (BackSpace/u/w clear), Return/Escape/Delete dispatch, `key_get_utf8` → `SecretBuffer::append`.
- [ ] 6.5 Implement `Dispatch<WlPointer, ()>`: motion, button, hotspot hit-testing via `Surface::hotspot_from_point`, `setCursor` via `wp_cursor_shape_device_v1`.
- [ ] 6.6 Implement `Dispatch<WlTouch, ()>`: down/up/motion, hotspot hit-testing, touchpoint tracking.
- [ ] 6.7 Implement `HotSpot` struct with `Effect` enum (cancel/notok/ok), `contains_point`, and `act` mapping to `exit_reason`.

## 7. Test-only binary

- [ ] 7.1 Add `[[bin]]` target `nowayprompt-wayland-test` (path `src/bin/wayland-test.rs`) to `Cargo.toml`.
- [ ] 7.2 Implement `wayland-test.rs`: instantiate `Wayland::new()` + `init(cfg)`, drive the frontend, log configure serial/dimensions/scale/hotspot rects/`Event`s to stderr.
- [ ] 7.3 Verify `cargo build --bin nowayprompt-wayland-test` is clean and the main `nowayprompt` binary is unchanged (TTY-only).

## 8. cargo tests

- [ ] 8.1 Add `cargo test` unit tests for `BufferPool::next_buffer` reuse/re-init/new/cull logic (no Wayland socket needed; mock the pool state).
- [ ] 8.2 Add `cargo test` unit tests for the R/B byte swap correctness (RGBA→BGRA on a known pixel pattern).
- [ ] 8.3 Add `cargo test` unit tests for `HotSpot::contains_point` and `Surface::hotspot_from_point`.
- [ ] 8.4 Add `cargo test` unit tests for the `exit_reason` state machine (UserOk/UserAbort/UserNotOk → Event conversion + clear + enter_mode(None)).
- [ ] 8.5 Verify `cargo test` passes with no regressions to Stage 2 tests.

## 9. nixosTest stage-3-wayland

- [ ] 9.1 Add `nixos-tests/stage-3-wayland.nix`: headless `cage` (`WLR_BACKEND=headless`, `WLR_RENDERER=pixman`, `WLR_LIBINPUT_NO_DEVICES=1`), `wtype`, the `nowayprompt-wayland-test` binary, Python driver.
- [ ] 9.2 Wire `nixosTests.stage-3-wayland` into `flake.nix` `nixosTests` output.
- [ ] 9.3 Implement the Python driver: launch `cage` + test binary, send `wtype` Return/Escape/BackSpace, grep stderr logs for configure/dimensions/scale/`Event` assertions.
- [ ] 9.4 Run `nix build .#nixosTests.stage-3-wayland --print-build-logs` and iterate until it exits 0.
- [ ] 9.5 Add the `nixosTests.stage-3-wayland` step to `.github/workflows/ci.yml` (with `continue-on-error: true` initially, matching the Stage 1-3 pattern; remove once green).

## 10. Parity verification

- [ ] 10.1 Diff `Wayland.zig` registry/sync/flush/handleEvent/noEvent against the Rust impl method-by-method; confirm behavioral parity.
- [ ] 10.2 Diff `Wayland.zig` keyboardListener key dispatch (Return/Escape/BackSpace/Delete/Ctrl+u/Ctrl+w) against the Rust impl.
- [ ] 10.3 Diff `Wayland.zig` Surface.render layout (title/description/prompt/errmessage/pin/buttons ordering and centering) against the Rust impl.
- [ ] 10.4 Confirm `src/main.rs` is unchanged (no Wayland wiring; Stage 4 deliverable).
- [ ] 10.5 Run `cargo fmt`, `cargo clippy`, `cargo test`, `cargo build` clean.