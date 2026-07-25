# `nowayprompt` Rust Rewrite Plan & Architecture Specification

`nowayprompt` is a multi-purpose Wayland prompt utility written in Rust, replacing the Zig `wayprompt` implementation. It serves as a password prompt, a drop-in GPG Assuan Pinentry replacement (`pinentry-nowayprompt`), and an SSH askpass provider (`nowayprompt-ssh-askpass`), with a fallback TUI interface for TTY consoles.

---

## 1. Architectural Strategy & Hardened Design

### Pure-Rust Dependency Stack
- **Wayland Client & Layer-Shell**: `wayland-client` (v0.31+, pure-Rust `rs` socket backend) + `wayland-protocols-wlr` (v0.3+, `zwlr_layer_shell_v1`). Eliminates runtime/build dependency on `libwayland-client.so`.
- **Text & Graphics Engine**: `cosmic-text` (v0.19+) + `tiny-skia` (v0.12+) + `fontdb` (v0.23+) + `swash` (v0.2+). `fontdb` parses system `fonts.conf`; `cosmic-text` + `swash` handle text shaping, layout, and hinting; `tiny-skia` rasterizes primitives directly to linear software `wl_shm` buffers.
- **XKB Keyboard Mapping**: `xkbcommon` (v0.9+) + `memmap2` (v0.9+). Memory-maps `wl_keyboard.keymap` file descriptors with `SIGBUS` panic guards to construct `xkbcommon::xkb::Keymap` and `State` instances.
- **Protected Secret Memory**: OS-level `mmap(2)` / `munmap(2)` page allocation (`MAP_PRIVATE | MAP_ANONYMOUS`), `libc::mlock` page locking, `libc::MADV_DONTDUMP` coredump protection, `libc::MADV_WIPEONFORK` fork protection, `RLIMIT_CORE = 0`, and `zeroize::Zeroize` on `Drop`.
- **TTY Console Fallback**: Direct `libc` termios raw mode management (`tcgetattr`/`tcsetattr`), POSIX signal handlers (`SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`, `SIGTSTP`) restoring terminal state before process exit/stop, and non-allocating ANSI escape sequence formatting.
- **Config & IPC**: Custom streaming `std::io::BufRead` line parser for `wayprompt.5` INI configurations (with trailing semicolon stripping) and a synchronous stdin/stdout GPG Assuan Pinentry IPC REPL.

---

## 2. Module Tree Structure (`src/`)

```
src/
├── main.rs                   # Multiplexer & CLI / Pinentry / Askpass entrypoint
├── config.rs                 # INI parser matching wayprompt.5 (trailing semicolon stripping)
├── secret.rs                 # Direct mmap(2), mlock(2), MADV_DONTDUMP/WIPEONFORK buffer with Zeroize
├── protocol/
│   └── assuan.rs             # Synchronous stdin/stdout Assuan line REPL & %XX decoder
└── frontend/
    ├── mod.rs                # Frontend trait & interface mode dispatch (Wayland vs TTY)
    ├── tty.rs                # Raw termios, signal-hook restoration, ANSI renderer
    └── wayland/
        ├── mod.rs            # Connection, event loop iteration, State dispatch
        ├── shm.rs            # memfd_create, wl_shm_pool, busy-buffer tracking (wl_buffer.release)
        ├── render.rs         # cosmic-text, fontdb, swash, tiny-skia UI compositor, SIMD BGRA swap
        └── input.rs          # wl_seat, wl_keyboard, xkbcommon state, compositor modifier mask
```

---

## 3. Negative Constraints & Hardened Invariants

1. **No External C Graphics Dependencies**: MUST NOT link against `libfcft.so`, `libpixman-1.so`, or `libcairo.so`. Wayland IPC MUST use `wayland-client` pure-Rust `rs` socket backend.
2. **No Heavy Desktop Frameworks**: MUST NOT use `smithay-client-toolkit` (SCTK), `winit`, or `calloop`. Dispatches MUST use direct `wayland-client` traits.
3. **No General Heap Secret Allocations**: Secret password strings MUST NOT use standard library heap allocators (`std::alloc::alloc`, `String`, `Vec<u8>`). MUST use direct kernel `mmap(2)` page allocations with `mlock`, `MADV_DONTDUMP`, `MADV_WIPEONFORK`, and zeroization on drop. IPC responses MUST stream raw bytes directly from `secret.rs` pointers to output file descriptors.
4. **No Early Surface Buffer Commit**: MUST NOT attach a buffer to `wl_surface` on or before the initial `wl_surface.commit()`. The initial commit MUST be buffer-less to request layer surface layout dimensions, waiting for `zwlr_layer_surface_v1::Event::Configure` before buffer attachment and acknowledgment (`ack_configure`).
5. **No Blind Overwrite of Busy SHM Buffers**: MUST track `wl_buffer.release` events per SHM buffer. Re-rendering MUST NOT overwrite a buffer while marked `busy` by the compositor.
6. **No Physical Key Code Modifier Calculation**: MUST NOT update XKB modifier state via raw key events; MUST synchronize modifier state exclusively via `wl_keyboard.modifiers` events.
7. **No Async Runtimes**: MUST NOT use `tokio`, `async-std`, or `futures`. IPC and event loops MUST be synchronous and poll-based.
8. **No Legacy X11 Protocol Support**: X11 windowing is strictly non-goal. Target environment is Wayland (`zwlr_layer_shell_v1`) with TTY console fallback.

---

## 4. Phased Implementation Roadmap

* **Stage 0: Nix Flake & Development Environment**
  * `flake.nix`: `devShells.default` declaring Rust toolchain (`rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`), `pkg-config`, `libxkbcommon` development libraries, and `nixpkgs` inputs.

* **Stage 1: Core Security Foundation & Configuration**
  * `Cargo.toml` workspace initialization and dependency locking.
  * `src/secret.rs`: Direct `mmap(2)` OS allocator (`MAP_PRIVATE | MAP_ANONYMOUS`), `libc::mlock`, `libc::MADV_DONTDUMP`, `libc::MADV_WIPEONFORK`, `rlimit` core zeroing, `Zeroize` on `Drop`.
  * `src/config.rs`: INI line reader, `wayprompt.5` semicolon parser, RGBA color conversion.

* **Stage 2: Protocol Engine & Hardened TTY Fallback**
  * `src/protocol/assuan.rs`: Assuan IPC parser, percent-decoder, zero-allocation `D <secret>` output stream.
  * `src/frontend/tty.rs`: Raw mode `libc::tcgetattr`/`tcsetattr`, signal hooks (`SIGINT`, `SIGTERM`, `SIGTSTP`, `SIGCONT`) restoring termios, non-echoed ANSI renderer.

* **Stage 3: Wayland Layer-Shell Frontend & Graphics Engine**
  * `src/frontend/wayland/mod.rs`: Registry global binding (`wl_compositor`, `wl_shm`, `wl_seat`, `zwlr_layer_shell_v1`, `wp_fractional_scale_manager_v1`). Buffer-less initial `wl_surface.commit()`, `Configure` serial acknowledgment, multi-output Enter/Leave tracking.
  * `src/frontend/wayland/shm.rs`: `memfd_create` buffer pool allocation, `wl_buffer.release` busy state tracking, triple-buffer expansion.
  * `src/frontend/wayland/render.rs`: `cosmic-text` + `tiny-skia` UI compositor, SIMD ARGB->BGRA `u32` pixel byte order conversion, subpixel AA premultiplied alpha math.
  * `src/frontend/wayland/input.rs`: `memmap2` fd keymap compilation with `SIGBUS` guards, compositor `wl_keyboard.modifiers` sync, evdev keycode +8 offset handling.

* **Stage 4: CLI Entrypoints & Nix Packaging**
  * `src/main.rs`: `arg[0]` multiplexer for CLI, Pinentry, and SSH Askpass.
  * `flake.nix`: Final package output (`packages.default`) build definition and binary symlink installation.

---

## 5. NixOS 1:1 Staggered Testing Strategy (`nixosTest`)

Baseline package: `pkgs.wayprompt` (`github:nixos/nixpkgs/nixos-26.05#wayprompt`, v0.1.2)  
Target package: `pkgs.nowayprompt` (Rust rewrite)

1. **Stage 1: CLI & Config Parity**: Verify exit status codes, `--help`/`--version` output, and `wayprompt.5` trailing semicolon INI file parsing without a display server.
2. **Stage 2: Assuan IPC Protocol Parity**: Automated stdin/stdout test streams validating `OK`, `D <secret>`, percent decoding (`%XX`), and error codes against `pinentry-wayprompt`.
3. **Stage 3: Virtual TTY Console Fallback**: Verify raw termios flag clearing (`ECHO`, `ICANON`, `ISIG`) on virtual console `tty1`, zero password buffer leaks, signal restoration on `SIGINT`/`SIGTSTP`, and ANSI cursor control.
4. **Stage 4: Wayland Layer-Shell & Rendering Parity**: Run under headless `cage` compositor using `wtype` virtual keyboard driver and `grim` frame capture to assert initial buffer-less commit, exclusive keyboard grab, dynamic resizing, and 1:1 surface geometry.

---

## 6. Reference & Critique Index

- `reference/wayland.md`: `wayland-client` v0.31 & `zwlr_layer_shell_v1` API reference manual.
- `reference/graphics.md`: `cosmic-text`, `tiny-skia`, `fontdb`, and `swash` API reference manual.
- `reference/xkb_input.md`: `xkbcommon` & `memmap2` API reference manual.
- `reference/security_tty_ipc.md`: POSIX `mlock`, `termios`, INI parser, and Assuan IPC API reference manual.
- `reference/critic_security.md`: Adversarial security & POSIX systems critique.
- `reference/critic_wayland_graphics.md`: Adversarial Wayland protocol, graphics rendering, and XKB critique.
