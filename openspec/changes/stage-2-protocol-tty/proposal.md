## Why

Stage 1 delivered the security foundation (`src/secret.rs`) and the INI configuration engine (`src/config.rs`). Stage 2 delivers the two modules that turn those primitives into a usable pinentry: the GPG Assuan IPC REPL (`src/protocol/assuan.rs`) that `gpg-agent` speaks to, and the TTY console fallback frontend (`src/frontend/tty.rs`) that collects a passphrase when no Wayland compositor is available. Together they make `pinentry-nowayprompt` functional in headless/TTY environments and establish the `Frontend` trait that Stage 3's Wayland frontend will implement — without requiring any Stage 3 re-architecture, because the dispatch model is poll-based from the start (legacy parity).

## What Changes

- **NEW**: `src/protocol/assuan.rs` — synchronous stdin/stdout Assuan IPC REPL with percent-decoding, hotkey-underscore stripping, full legacy command coverage (implemented commands + silently-accepted set + not-implemented set), zero-allocation `D <secret>` streaming directly from `SecretBuffer` bytes to stdout.
- **NEW**: `src/frontend/mod.rs` — `Frontend` trait declaring `init`, `deinit`, `enter_mode`, `handle_event`, plus an `Event` enum (`UserOk`, `UserAbort`, `UserNotOk`, `None`) and `InterfaceMode` enum (`None`, `GetPin`, `Confirm`, `Message`). Mirrors `legacy/src/Frontend.zig`.
- **NEW**: `src/frontend/tty.rs` — raw `libc::termios` TTY frontend: `tcgetattr`/`tcsetattr` raw mode (clear `ECHO | ICANON | ISIG`, `VMIN=1/VTIME=0`), hand-rolled ANSI renderer (clear/home/wrapping/button layout), hand-rolled byte-level input parser (enter/escape/C-c/C-u/C-w/C-backspace/backspace/UTF-8 codepoint), `signal-hook`-based restoration of termios on `SIGINT`/`SIGTERM`/`SIGHUP`/`SIGQUIT`/`SIGTSTP` before exit.
- **NEW**: `src/main.rs` — poll-based dispatch loop over stdin (Assuan) and the active frontend's fd, calling `assuan::handle_line` and `frontend::handle_event` concurrently (legacy parity). CLI/Pinentry/Askpass arg-multiplexing deferred to Stage 4; Stage 2 wires the pinentry path only.
- **NEW**: `nixosTests.stage-1-cli-config`, `nixosTests.stage-2-assuan`, `nixosTests.stage-3-tty` flake outputs — NixOS-VM differential parity tests running the Rust target against the pinned `pkgs.wayprompt` (nixos-26.05, v0.1.2) baseline, per `RUST_REWRITE.md` §5 "1:1 staggered testing strategy". Stage 1 test backfills the gap left by the archived Stage 0-1 change.
- **MODIFIED**: `Cargo.toml` — add `signal-hook` dependency.
- **MODIFIED**: `src/config.rs` — expose `Config::reset()` (clears all `Labels` fields to `None`) and ensure `tty_name`, `wayland_display`, `allow_tty_fallback`, `secbuf` are runtime-settable (already present per Stage 1; this change formalizes the reset contract the Assuan `RESET` command requires).
- **MODIFIED**: `flake.nix` — add `nixpkgs-26_05` input pinning the legacy oracle revision; add minimal `packages.<system>.nowayprompt` via `buildRustPackage` (Stage 4 packaging slice pulled forward so `nixosTest` can install the target); add `nixosTests` outputs.

## Capabilities

### New Capabilities

- `assuan-ipc`: Synchronous line-framed Assuan pinentry protocol handler with percent-decoding, hotkey stripping, full legacy command matrix (implemented / silently-accepted / not-implemented), and zero-copy `D <secret>` streaming from `SecretBuffer`. 100% behavioral parity with `legacy/src/wayprompt-pinentry.zig`.
- `frontend-trait`: The `Frontend` trait, `Event`, and `InterfaceMode` contracts that both the TTY fallback (Stage 2) and the Wayland frontend (Stage 3) implement. Defines the poll-based concurrent dispatch surface.
- `tty-fallback`: Raw `libc::termios` TTY console frontend with ANSI rendering, hand-rolled input parsing, and `signal-hook`-driven termios restoration. 100% behavioral parity with `legacy/src/TTY.zig` (minus `SIGWINCH`, which legacy also left unimplemented).
- `nixos-parity-testing`: NixOS-VM differential parity harness running the Rust target against the pinned `pkgs.wayprompt` (nixos-26.05, v0.1.2) baseline, with staggered `nixosTest` outputs per implementation stage and a byte-tolerance contract for known-allowed divergences (pid, version string).

### Modified Capabilities

<!-- None. No existing specs are changed at the requirement level. `config-parser` gains a `reset()` method but that is an additive implementation detail, not a spec-level behavior change; it will be noted in the design. -->