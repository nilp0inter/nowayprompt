## Context

Stage 2 (`2026-07-25-stage-2-protocol-tty`, archived) delivered the Assuan IPC handler, the TTY fallback frontend, and froze the poll-based `Frontend` trait contract (`init`/`deinit`/`enter_mode`/`handle_event`/`flush`/`no_event`) in `src/frontend/mod.rs`. The `main.rs` dispatch loop polls `[stdin, frontend_fd]` and is frontend-agnostic — Stage 3 is additive.

The parity target is `reference/legacy/src/Wayland.zig` (1761 LOC), the legacy Zig Wayland frontend. It is in-tree (no external fetch). Stage 3 implements the Rust equivalent as a library implementing `Frontend`. It does NOT wire into `main.rs` (deferred to Stage 4); the entrypoint stays TTY-only.

The pure-Rust dependency stack is mandated by `RUST_REWRITE.md` §3: no `libwayland-client.so`, no `libfcft.so`/`libpixman-1.so`/`libcairo.so`. The one exception is `xkbcommon` (C-dlopen of `libxkbcommon.so`) — no pure-Rust XKB implementation exists, and XKB is protocol-critical.

Reference API manuals (`reference/{wayland,graphics,xkb_input,critic_wayland_graphics}.md`) confirm the crate mappings; the critic doc surfaced the load-bearing risks (pixel format swap, subpixel AA, font startup latency, SIGBUS, fractional scaling).

## Goals / Non-Goals

**Goals:**
- Implement `src/frontend/wayland/{mod,shm,render,input}.rs` implementing the frozen `Frontend` trait, with behavioral parity to `legacy/src/Wayland.zig`.
- Render parity is **behavioral only**: correct surface geometry, layout box positions, event flow, hotspot hit-testing. NOT pixel-identical.
- Add a geometry-only `nixosTests.stage-3-wayland` under headless `cage` + `wtype`, exercising a test-only `[[bin]]` target that instantiates `Wayland::init()` directly.
- Keep the phase additive: no `main.rs` edit, no end-to-end pinentry binary, no Stage 4 packaging.

**Non-Goals:**
- Wire Wayland-vs-TTY frontend selection into `main.rs` (Stage 4).
- Pixel-identical rendering to legacy `fcft`+`pixman` (impossible with `cosmic-text`+`tiny-skia`; explicitly out of scope).
- `grim`-based pixel-parity nixosTest (Stage 4's contract; Stage 4's gate must be reframed as tolerance/perceptual).
- Stage 4 packaging: `pinentry-nowayprompt` symlink, `wayprompt-ssh-askpass` wrapper, manpages, `packages.default` alias.
- Pointer/touch click hotspot hit-testing in the nixosTest (deferred to Stage 4; `wtype` does keyboard only).

## Decisions

### D1: Behavioral render parity, not pixel parity

**Choice**: Stage 3's render contract is behavioral — surface geometry, layout box positions, event flow, hotspot hit-testing. Pixel-identical output to legacy `fcft`+`pixman` is explicitly out of scope.

**Rationale**: `cosmic-text` (via `swash`/`rustybuzz`) and `tiny-skia` are different rasterizers/shapers than `fcft`+`pixman`. Glyph metrics, subpixel AA, and path AA differ. Pixel-identical output is impossible without the legacy C stack, which the pure-Rust invariant forbids. Stage 2's D10 already conceded "layout parity, not pixel parity" for TTY; Stage 3 extends the same concession to Wayland. Stage 4's `grim`-based nixosTest gate must be reframed as tolerance/perceptual, not byte-identical — this is recorded here so Stage 4 inherits the relaxation.

**Alternatives**:
- Require pixel parity: impossible under the pure-Rust invariant. Rejected.
- Fallback to C `fcft`+`pixman` bindings: violates §3 invariant. Rejected.

### D2: Keymap fd via `memmap2` `MAP_PRIVATE` read-only, no SIGBUS guard (match legacy)

**Choice**: Map the `wl_keyboard.keymap` fd with `memmap2::MmapOptions::new().len(size).map(&file)` (defaults to `MAP_PRIVATE` read-only on Unix), compile via `xkbcommon::xkb::Keymap::new_from_string`, then drop the mmap. No `SIGBUS` handler.

**Rationale**: Legacy `Wayland.zig:445-474` does `posix.mmap(fd, ev.size, PROT.READ, MAP.PRIVATE)` with no guard. The compositor is local and trusted in the legacy threat model. Adding a `SIGBUS` handler that `zeroize`s the `SecretBuffer` before exit is a defensible hardening, but diverges from parity and is scope creep for Stage 3.

**Risk (recorded, deferred)**: If the compositor truncates the keymap fd mid-read, `SIGBUS` kills the process and `SecretBuffer::Drop` may not run → plaintext password leak. This is a real security concern. Mitigation (a `SIGBUS` handler that `zeroize`s the secret) is deferred to a future hardening change, not Stage 3.

**Alternatives**:
- `read()` into a heap `Vec<u8>` then `Keymap::new_from_string` (critic §3.1): eliminates SIGBUS risk, diverges from legacy mmap. Rejected for parity; revisit as hardening.
- `memmap2` + `SIGBUS` handler: defensible but scope creep. Deferred.

### D3: `xkbcommon` C-dlopen is an acceptable pure-Rust invariant exception

**Choice**: Add `xkbcommon` (v0.8+, the `xkbcommon-dl` variant that dlopens `libxkbcommon.so` at runtime) as the sole C-linkage dependency.

**Rationale**: No pure-Rust XKB implementation exists. XKB is protocol-critical (keymap compilation, keysym lookup, modifier state) — there is no substitute. The `RUST_REWRITE.md` §3 invariant forbids `libwayland-client.so`, `libfcft.so`, `libpixman-1.so`, `libcairo.so` — it does not forbid `libxkbcommon.so`, and legacy itself links `xkbcommon`. `libxkbcommon.so` is universally present on Linux Wayland systems. This is the single, explicit, recorded exception.

**Alternatives**:
- Pure-Rust XKB: does not exist. Rejected.
- Hand-roll keysym tables: infeasible for arbitrary keymaps. Rejected.

### D4: `wayland-client` `rs` socket backend (pure-Rust, poll-able fd)

**Choice**: Use `wayland-client` v0.31+ with the default `rs` socket backend (pure-Rust, no `libwayland-client.so` dlopen). `Connection::connect_to_env()` → `new_event_queue()`; the queue's fd is returned from `Frontend::init` for `main.rs` to poll.

**Rationale**: The `rs` backend is pure-Rust and default. The `EventQueue` fd is poll-able via `poll(2)`, satisfying the Stage 2 `Frontend::init → RawFd` contract. The `Dispatch<I, U>` trait routes events to a central `State` struct (vs legacy's per-object `setListener` callbacks); the registry/sync listener pattern is reproduced via `Dispatch<WlRegistry, ()>` + a `WlCallback` sync round-trip that finalizes global binding and flushes `delayed_mode` (parity with `Wayland.zig:1542-1546`).

**Alternatives**:
- SCTK (Smithay Client Toolkit): rejected by §3 invariant (wraps `wayland-client` but adds opinionated abstractions that obscure parity).
- Raw socket protocol hand-rolling: reinvents `wayland-client`. Rejected.

### D5: SHM buffer pool — `Vec<Buffer>` arena + `Dispatch` user-data index

**Choice**: Represent the buffer pool as `Vec<Buffer>` (arena). Each `wl_buffer` carries its index in the `Dispatch<WlBuffer, usize>` user-data slot; the `.release` event flips `state.buffers[idx].busy = false`. Triple-buffer `max_buffer_multiplicity=3` + `cullBuffers` (parity with `Wayland.zig:1256-1351`).

**Rationale**: Legacy uses a `TailQueue` (linked list) with stable pointers for `wl_buffer.release` listener callbacks. `wayland-client`'s `Dispatch` model routes events to central `State` with a per-proxy user-data slot `U` — stable pointers are not the Rust idiom. An arena with indices is the faithful equivalent: the index is the buffer identity, carried through the user-data slot. No `unsafe` pointer juggling.

**Alternatives**:
- `Rc<RefCell<Buffer>>` shared between pool and listener: borrow-checker friction across the Dispatch boundary; rejected.
- Linked list with raw pointers: `unsafe`, unidiomatic; rejected.

### D6: Pixel format — `tiny-skia` premultiplied RGBA → in-place R/B swap → Wayland `Argb8888`

**Choice**: Render in `tiny-skia`'s native premultiplied RGBA8888 (`[R,G,B,A]`). Before `wl_surface.commit`, swap R and B channels in-place over the SHM pixel buffer (`chunk.swap(0, 2)` scalar, or SIMD `u32` bitwise rotation: `a|r|g|b` — auto-vectorizes on x86_64), converting to little-endian `Argb8888` (`[B,G,R,A]`). Use `wl_shm::Format::Argb8888` to match legacy (`Wayland.zig:1390`).

**Rationale**: `tiny-skia` outputs premultiplied RGBA; Wayland `wl_shm` on little-endian expects `Argb8888`/`Xrgb8888` = `[B,G,R,A]` byte order. The swap is a known, bounded cost (critic §2.1 flags the scalar version as a perf bottleneck at 4K; the SIMD `u32` bitwise form auto-vectorizes to <1ms). Legacy uses `.argb8888`; the reference skeleton's `Xrgb8888` is a discrepancy — we match legacy.

**Alternatives**:
- Render directly in BGRA: `tiny-skia` does not support BGRA natively. Rejected.
- Use `Xrgb8888`: diverges from legacy. Rejected.

### D7: Grayscale AA, bundled fallback font, cached pin mask glyph

**Choice**: (a) Disable subpixel AA — force grayscale (avoids chromatic aberration on transparent backgrounds, critic §2.3). (b) Bundle a fallback font via `include_bytes!` (DejaVu Sans / Fira Mono) and load via `fontdb::load_font_data`; do NOT call `fontdb::load_system_fonts()` (200ms–2s startup, critic §2.2). (c) Shape the single pin mask glyph (`•`/`*`) once at startup, cache its rasterized pixels, and blit iteratively per keystroke — do not re-run the cosmic-text shaper per keypress (critic §2.4).

**Rationale**: A pinentry must start in <20ms. System font scanning and per-keystroke shaping violate that. Grayscale AA is layout-agnostic and computationally cheaper than subpixel. The legacy `fcft` fallback chain (`[user_font, "sans:size=14", "mono:size=14"]`) is reproduced via `fontdb` query with the bundled font as the final fallback.

**Alternatives**:
- System font scan + subpixel AA: matches "default" cosmic-text usage but violates startup latency and AA constraints. Rejected.

### D8: Fractional-scale binding present, scale pinned to 1 (legacy parity)

**Choice**: Bind `wp_fractional_scale_manager_v1` (legacy binds it, `Wayland.zig:1699-1735`) and create a `wp_fractional_scale_v1` for the surface, but **pin `scale = 1`** — ignore `preferred_scale` events. The SHM buffer is allocated and rendered at logical size with `set_buffer_scale(1)`.

**Rationale**: Legacy binds the fractional-scale manager but pins `scale = 1` (`Wayland.zig:749`; fractional-scale support is an unimplemented TODO there). Honoring `preferred_scale` requires a *physical-size* buffer (`width*scale × height*scale`) plus scaled text/vector drawing; rendering a logical-size buffer with `set_buffer_scale(scale>1)` makes the compositor downscale it — the surface and fonts shrink (the bug caught in manual testing). Pinning `scale = 1` matches the legacy exactly and keeps buffer/drawing consistent. The binding is retained for protocol parity; crisp HiDPI rendering is a deferred enhancement.

**Alternatives**:
- Honor `preferred_scale` with a physical-size buffer + scaled drawing: crisp on HiDPI, but deviates from the legacy's `scale = 1` and is a larger change. Deferred.
- Don't bind fractional-scale at all: diverges from the legacy's global binding. Rejected.

### D9: Test-only `[[bin]]` target, not `main.rs` wiring

**Choice**: Add a second `[[bin]]` target (`name = "nowayprompt-wayland-test"`, `path = "src/bin/wayland-test.rs"`) that instantiates `Wayland::new()` + `init(cfg)` and drives the frontend directly. `main.rs` stays TTY-only. The nixosTest builds and runs this test binary, not the pinentry.

**Rationale**: Stage 3 is additive-only — no `main.rs` edit. But the nixosTest needs a real binary driving `Wayland::init()` under `cage`. A second `[[bin]]` is nixosTest-friendly (derivable as a separate package output) and doesn't pollute the main pinentry build. Stage 4 replaces this with the real `nowayprompt` binary once `main.rs` wires frontend selection.

**Alternatives**:
- `examples/wayland.rs`: less nixosTest-friendly to derive as a package output. Rejected.
- `#[test]` integration in the VM: awkward to drive a Wayland event loop from a test harness. Rejected.

### D10: Geometry-only nixosTest under headless `sway` + `wtype`

**Choice**: `nixosTests.stage-3-wayland` runs the test binary under `sway` (started via getty autologin, giving it a real logind session/seat) with `WLR_BACKENDS=drm`, `WLR_RENDERER=pixman` (software, no EGL) and a virtio-gpu device. `wtype` injects synthetic keyboard events (Return, Escape). A wrapper script loops the test binary, logging to a file the test script greps and recording the sway socket path (sway names its socket `wayland-N`). The test asserts configure dimensions, non-zero hotspot geometry, and keyboard `Event` emission. No `grim` pixel capture.

**Rationale**: The render path (`cosmic-text`+`tiny-skia`→SHM→`wl_buffer.commit`) is the highest-risk divergence from legacy; a real compositor exercises wlroots quirks a mock cannot. `cage` was the original plan but does **not** implement `zwlr_layer_shell_v1` (single-fullscreen kiosk), so `sway` (the reference wlroots compositor, which supports layer-shell) is used instead. `WLR_BACKENDS=drm` (plural — the singular `WLR_BACKEND` is ignored by current wlroots) avoids wlroots' Wayland-backend auto-detection that `WAYLAND_DISPLAY` would otherwise trigger. Pixel parity is Stage 4's contract (reframed as tolerance per D1). Pointer/touch click hit-testing is deferred (`wtype` is keyboard-only — Stage 4).

**Alternatives**:
- `cage`: lacks `zwlr_layer_shell_v1`. Rejected.
- `cargo test` with a mock compositor only: misses wlroots quirks. Rejected as the sole gate.
- Full `grim` pixel nixosTest now: Stage 4's contract; duplicates infra. Rejected for Stage 3.

### D11: Single-threaded read model (collapse legacy's prepare/read/cancel)

**Choice**: The poll loop drives `flush` (flush outbound), then `poll(stdin, wayland_fd)`; when the wayland fd is readable, `handle_event` does `prepare_read().read()` + `dispatch_pending`; otherwise `no_event` is a no-op. This collapses the legacy's three-step `prepare_read`/`read_events`/`cancel_read` dance.

**Rationale**: Legacy splits prepare/read/cancel to coordinate *multiple threads* reading the socket. A pinentry has exactly one event-loop thread, so the dance is inert. The collapsed model is behaviorally identical for a single-threaded client (events dispatched, outbound flushed, `exit_reason` surfaced) and avoids storing a self-referential `ReadEventsGuard` (which borrows the `Connection`) across the `flush`→`handle_event` poll window. Documented in `mod.rs`.

**Alternatives**:
- Reproduce the legacy prepare/read/cancel exactly: requires persisting a `ReadEventsGuard` across `flush`→`handle_event`, i.e. self-referential storage. Rejected as needless complexity for a single-threaded client.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|---|---|---|
| `SIGBUS` on keymap fd truncation voids `SecretBuffer::Drop` → plaintext leak | Security: password leak on compositor bug | D2: match legacy (no guard); record as deferred hardening. Future change adds a `SIGBUS` handler that `zeroize`s the secret. |
| `xkbcommon` C-dlopen breaks pure-Rust invariant | Build/runtime dep on `libxkbcommon.so` | D3: recorded as the single explicit exception; universally present on Wayland systems. |
| `cosmic-text` layout metrics differ from `fcft` | Layout box positions not pixel-identical to legacy | D1: behavioral parity only; Stage 4 gate reframed as tolerance. |
| Headless compositor in the NixOS VM | nixosTest flaky/unreliable | D10: `sway` (not `cage`, which lacks layer-shell) with `WLR_BACKENDS=drm`+`WLR_RENDERER=pixman`+virtio-gpu via getty autologin; `wtype` injects keys. |
| `wtype` keyboard-only; pointer/touch untested | Hotspot click hit-testing unverified in Stage 3 | D10: defer pointer/touch to Stage 4; keyboard `Event` emission is the Stage 3 gate. |
| `wayland-client` `Dispatch` model divergence from legacy `setListener` | Subtle event-ordering differences | D4: central `State` + `delegate_dispatch!` reproduces the listener semantics; verify via the nixosTest. |
| Fractional scaling adds `wayland-protocols` core dep | Build complexity | D8: binding retained for parity; `scale` pinned to 1 (legacy `Wayland.zig:749` TODO); honoring `preferred_scale` deferred. |