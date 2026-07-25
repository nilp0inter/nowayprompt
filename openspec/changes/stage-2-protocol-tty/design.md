## Context

Stage 0 + Stage 1 are archived and delivered: the Nix flake dev shell, `src/secret.rs` (page-locked, zeroized secret buffer), and `src/config.rs` (`wayprompt.5` INI parser with premultiplied colour conversion). `src/main.rs` is a stub (`fn main() {}`).

Stage 2 builds the next layer: the GPG Assuan pinentry IPC handler and the TTY console fallback frontend. The parity targets are `legacy/src/wayprompt-pinentry.zig` (491 LOC) and `legacy/src/TTY.zig` (212 LOC), plus the `legacy/src/Frontend.zig` (84 LOC) interface contract. A reference sketch exists in `reference/security_tty_ipc.md` but is simplified and regresses the dispatch model; the legacy source is the authoritative behavioral contract.

Constraints (from `RUST_REWRITE.md` §3):
- No async runtimes (`tokio`, `async-std`, `futures`). IPC and event loops MUST be synchronous and poll-based.
- No external C graphics deps (not relevant to Stage 2).
- No heavy desktop frameworks (SCTK, winit, calloop) — not relevant to Stage 2.
- No general heap secret allocations: `D <secret>` MUST stream raw bytes from `SecretBuffer` to the output fd, not through `String`/`Vec`.
- 100% behavioral parity with legacy Zig modules.

Stakeholders: single-user NixOS deployment; `pinentry-nowayprompt` invoked by `gpg-agent`; `nowayprompt-ssh-askpass` (Stage 4); CLI prompt (Stage 4).

## Goals / Non-Goals

**Goals:**
- `src/protocol/assuan.rs`: Assuan IPC REPL with full legacy command matrix, percent-decoding, hotkey-underscore stripping, zero-copy secret streaming.
- `src/frontend/mod.rs`: `Frontend` trait + `Event` + `InterfaceMode` enums matching `legacy/src/Frontend.zig`.
- `src/frontend/tty.rs`: Raw `libc::termios` TTY frontend with ANSI rendering, hand-rolled input parser, `signal-hook` termios restoration.
- `src/main.rs`: poll-based dispatch loop over stdin + frontend fd (legacy parity), wiring the pinentry path only.
- Behavioral parity with `legacy/src/wayprompt-pinentry.zig` and `legacy/src/TTY.zig`, including the silently-accepted and not-implemented command sets required for `gpg-agent` interop.
- Fix legacy's partial-line stdin TODO via `BufReader` over stdin (decided: fix partial-lines only, skip `SIGWINCH`).

**Non-Goals:**
- Wayland frontend (Stage 3): `frontend/wayland/` is not created. The `Frontend` trait is shaped to accept it later without rework.
- CLI / Askpass entrypoint multiplexing (Stage 4): `main.rs` wires the pinentry path; `arg[0]` dispatch deferred.
- Nix package output `packages.default` (Stage 4).
- `SIGWINCH` resize handling: legacy has an open `TODO listen to SIGWINCH` it never implemented; parity = skip (decided).
- `SETREPEAT` / `SETQUALITYBAR` / `SETGENPIN` / `SETTIMEOUT` actual implementation: parity = return `ERR 536870981 Not implemented` (legacy behavior).
- `CONFIRM --one-button`: legacy has an open TODO; parity = ignore the flag (treat `CONFIRM` uniformly).
- Quality-bar `INQUIRE QUALITY` protocol: not implemented (parity).
- Multi-page secret buffer growth: single fixed page (Stage 1 invariant).

## Decisions

### D1: Poll-based concurrent dispatch over stdin + frontend fd (legacy parity)

**Choice**: `main.rs` runs a `libc::poll(2)` loop over two fds: stdin (Assuan commands) and the frontend's fd (returned by `Frontend::init`). On `POLLIN` on stdin, read and dispatch Assuan lines; on `POLLIN` on the frontend fd, call `Frontend::handle_event` and dispatch the resulting `Event` to the Assuan response writer. The frontend's `flush()` / `no_event()` asymmetry from legacy is preserved: `flush()` returns a pending `Event` (Wayland-only, returns `None` for TTY); `no_event()` is a no-op for TTY.

**Rationale**: Legacy `wayprompt-pinentry.zig` lines 100–152 does exactly this. The alternative — a synchronous `AssuanRepl::handle_line` that blocks on `GETPIN` — works for TTY (blocking read) but deadlocks under Wayland (Stage 3), forcing a re-architecture. Building the poll loop now means Stage 3 only adds a `Wayland` frontend implementation; no `main.rs` rewrite. The project invariant is "100% behavioral parity with legacy"; the legacy dispatch model IS the parity target.

**Alternatives**:
- Sync `handle_line` (blueprint `security_tty_ipc.md` §4): simpler Stage 2, known Stage 3 rework. Rejected — violates parity and defers the unavoidable.
- `mio` / `calloop` event loop: rejected by Negative Constraint #7 (no async runtimes) and #2 (no heavy frameworks).

### D2: Full legacy Assuan command matrix (interop-critical)

**Choice**: Implement the complete legacy command set, partitioned into three tiers:

| Tier | Commands | Response | Rationale |
|------|----------|----------|-----------|
| Implemented | `SETTITLE`, `SETPROMPT`, `SETDESC`, `SETERROR`, `SETOK`, `SETNOTOK`, `SETCANCEL`, `GETPIN`, `CONFIRM`, `MESSAGE`, `GETINFO`, `BYE`, `OPTION`, `RESET`, `NOP`, `HELP` | Per-command | Core pinentry functionality |
| Silently accepted | `SETKEYINFO` | `OK\n` | `gpg-agent` aborts if rejected (legacy comment lines 372–380) |
| Not implemented | `CANCEL`, `SETGENPIN`, `SETGENPIN_TT`, `SETTIMEOUT`, `END`, `QUIT`, `AUTH`, `CLEARPASSPHRASE`, `SETREPEAT`, `SETREPEATERROR`, `SETQUALITYBAR`, `SETQUALITYBAR_TT` | `ERR 536870981 Not implemented\n` | Legacy parity; reserved/undocumented commands |
| Unknown | anything else | `ERR 536871187 Unknown IPC command\n` | Legacy fallback |

`GETINFO` sub-commands: `flavor` → `D wayprompt\nEND\nOK\n`; `version` → `D 0.0.0\nEND\nOK\n`; `pid` → `D <pid>\nEND\nOK\n` via `std::process::id()`; unknown subcommand → just `OK\n` (legacy emits `OK` after the `if` block unconditionally). `HELP` emits the legacy comment block listing `NOP/SETTITLE/SETPROMPT/SETDESC/SETERROR/GETPIN/BYE/OPTION/RESET` then `OK\n`.

**Rationale**: The silently-accepted set is a hard interop requirement — legacy explicitly documents that `gpg-agent` aborts on `SETKEYINFO` rejection. The not-implemented set matches legacy exactly, including the duplicated `CANCEL` entry (legacy lines 381, 388). The blueprint's subset (`BYE, SETTITLE, SETDESC, SETPROMPT, OPTION, GETPIN, RESET, NOP`) would break real-world `gpg-agent` sessions.

**Alternatives**:
- Blueprint subset only: rejected — breaks `gpg-agent` interop.
- Reject silently-accepted commands with `ERR 536870981`: rejected — legacy comment lines 376–380 states this causes `gpg-agent` abort.

### D3: Percent-decoding + hotkey stripping (`pinentryDupe` parity)

**Choice**: A single `assuan_decode(input: &str, strip_hotkey: bool) -> Result<String, AssuanError>` function mirroring legacy `pinentryDupe` (lines 461–491). Pass 1 computes output length (subtract 2 per `%`, subtract 1 per `_` when `button=true`). Pass 2 decodes: `%XX` → byte via `u8::from_str_radix`, `_` skipped when `strip_hotkey`, else verbatim. Malformed `%` (fewer than 2 trailing bytes) or invalid hex → error.

`strip_hotkey=true` is applied ONLY to `default-ok`, `default-cancel`, `default-yes`, `default-no` `OPTION` values. All `SET*` commands use `strip_hotkey=false`.

**Rationale**: Legacy `pinentryDupe` pre-computes length to allocate exactly once. In Rust, `String::with_capacity(input.len())` achieves the same single-allocation property. The two-pass structure is preserved for parity (the output is always ≤ input length, so `with_capacity(input.len())` is a safe upper bound; no reallocation occurs). The hotkey-strip scope matches legacy `getOption` calls: `default-ok=` and `default-cancel=` pass `button=true` (line 343, 349); `default-yes=` and `default-no=` pass `button=true` (line 355, 361); all `setString` calls pass `button=false`.

**Alternatives**:
- `percent-encoding` crate: does not handle hotkey stripping; adds a dep for half the job.
- Single-pass `Vec::push`: matches the blueprint sketch but loses the length-precomputation parity (functionally equivalent; rejected only for parity fidelity, not correctness).

### D4: Zero-copy `D <secret>` streaming from `SecretBuffer`

**Choice**: On `GETPIN` + `user_ok` event, the Assuan handler writes `D ` to stdout, then writes the raw `&[u8]` from `SecretBuffer::slice()` directly to the stdout fd via `write_all`, then `\nEND\nOK\n`. No intermediate `String` or `format!`. Empty pin (`slice()` is `None`) → just `OK\n` (legacy lines 163–176).

**Rationale**: Negative Constraint #3 forbids general heap secret allocations. `format!("D {}\nEND\nOK\n", s)` would allocate a `String` containing the secret on the heap, violating the invariant and creating a non-zeroized copy. Legacy uses `writer.print("D {s}\nEND\nOK\n", .{s})` which streams directly from the `SecretBuffer` slice into the buffered writer without duplication. The Rust port replicates this with explicit `write_all` calls.

**Alternatives**:
- `format!("D {}\nEND\nOK\n", secret_str)`: heap-allocates the secret copy. Rejected (Constraint #3).
- `writeln!` into a `BufWriter<std::io::Stdout>`: acceptable; `BufWriter` is stack-allocated and flushed per-iteration (legacy uses `io.bufferedWriter`). This is the chosen path.

### D5: `Frontend` trait shape (legacy `Frontend.zig` parity)

**Choice**: A `Frontend` trait mirroring `legacy/src/Frontend.zig`:

```rust
pub trait Frontend {
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>;
    fn deinit(&mut self);
    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>;
    fn handle_event(&mut self) -> Result<Event, FrontendError>;
    fn flush(&mut self) -> Result<Option<Event>, FrontendError>;  // Wayland-only; TTY returns Ok(None)
    fn no_event(&mut self) -> Result<(), FrontendError>;           // Wayland-only; TTY no-op
}
```

`Event` enum: `None`, `UserOk`, `UserAbort`, `UserNotOk`. `InterfaceMode` enum: `None`, `GetPin`, `Confirm`, `Message`. (Note: legacy `confirm()` sets `mode = .message` internally — lines 245–246 — so `Confirm` and `Message` share the same frontend mode. The Assuan-level `Mode` distinguishes them; the frontend `InterfaceMode` collapses them. This parity detail is preserved.)

**Rationale**: Legacy `Frontend.zig` is a struct with function pointers (Zig duck-typing); Rust expresses the same contract as a trait. The `flush`/`no_event` asymmetry is awkward but faithful — Wayland needs `flush()` to drain pending buffer events without blocking and `no_event()` to render when no event arrived; TTY's `handle_event` blocks on `read`, so `flush` returns `None` and `no_event` is a no-op. Keeping this asymmetry now avoids a trait reshape in Stage 3.

**Alternatives**:
- Unified poll-based trait where `handle_event` always blocks: cleaner, but loses the Wayland non-blocking drain path that legacy relies on. Rejected.
- Split into `BlockingFrontend` / `EventFrontend` traits: over-engineering for a 2-implementation system. Rejected.

### D6: Raw `libc::termios` TTY (no `spoon`, no `curses`)

**Choice**: `src/frontend/tty.rs` uses `libc::tcgetattr`/`tcsetattr` for raw mode (clear `ECHO | ICANON | ISIG`, set `VMIN=1`, `VTIME=0`), writes raw ANSI escape sequences (`\x1b[2J`, `\x1b[H`, `\x1b[<row>;<col>H`, `\x1b[<n>C` for cursor, SGR for bold/red/green) to stdout, and reads raw bytes via `libc::read(fd, buf, 1)`. No `curses`/`ncurses`/`termion`/`crossterm` dependency.

**Rationale**: Legacy `TTY.zig` uses the `spoon` Zig library (a termios+rendering wrapper). No Rust crate fits the "no heavy deps" invariant: `crossterm` pulls in event-reading abstractions and Windows support we don't need; `termion` is Linux-only but still heavier than raw termios. The TTY rendering in legacy is ~80 LOC of `moveCursorTo` + `writeAllWrapping` + `setAttribute`; the Rust equivalent is direct ANSI writes, ~120 LOC. The input parsing (legacy `spoon.inputParser`) is replaced by a hand-rolled byte-level state machine (see D7).

**Alternatives**:
- `crossterm`: 30+ transitive deps, Windows abstraction. Rejected.
- `termion`: smaller but still wraps termios; redundant when we need raw `libc::termios` for `mlock`-adjacent code anyway. Rejected.

### D7: Hand-rolled TTY input parser (byte-level state machine)

**Choice**: A `TtyInputParser` that consumes raw bytes from `libc::read` and emits `TtyInput` events: `Enter` (`\r` or `\n`), `Escape` (`\x1b`), `Backspace` (`\x7f`), `C-c` (`\x03`), `C-u` (`\x15`), `C-w` (`\x17`), `C-backspace` (`\x08`), `Codepoint(char)` (UTF-8 decoded), `Unknown`. Modifier keys (Alt/Ctrl/Super) are ignored (legacy line 113: `if in.mod_alt or in.mod_ctrl or in.mod_super continue`). Escape sequences (`\x1b[A`, etc.) collapse to `Escape` when standalone.

**Rationale**: Legacy `spoon.inputParser` is a Zig-only library with no Rust equivalent. The key set legacy actually handles is small and fixed (lines 89–126): `enter`, `escape`, `C-c`, `C-u`, `C-w`, `C-backspace`, `backspace`, and UTF-8 codepoints (excluding modified codepoints). A 60-LOC byte-level decoder covers this exactly. Full xterm escape-sequence parsing (arrow keys, function keys, Home/End) is unnecessary — legacy ignores those inputs (they fall through to `codepoint` handling and get appended to the secret buffer, which is a bug, but it's legacy's bug; we match parity by treating unrecognized escapes as `Unknown` and dropping them).

**Alternatives**:
- `crossterm::event::read`: pulls in the whole crossterm stack. Rejected (D6).
- Full VT100 state machine: over-engineering; legacy handles ~7 keys. Rejected.

### D8: `signal-hook` for termios restoration on exit signals

**Choice**: Add `signal-hook = "0.3"` to `Cargo.toml`. Register handlers for `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`, `SIGTSTP` that restore the saved `termios` via `tcsetattr(fd, TCSAFLUSH, &orig)` and call `libc::_exit(0)`. The `RawTty` RAII guard stores the fd and `orig_termios` in `static` atomics accessible to the signal handler (parity with `critic_security.md` lines 79–80).

**Rationale**: `RUST_REWRITE.md` line 30 and `critic_security.md` line 77 prescribe `signal-hook`. Raw `sigaction` is doable but error-prone (signal-handler safety: async-signal-safe functions only, no allocations, no locks). `signal-hook` provides a safe abstraction using `sigaction` underneath with a self-pipe/pipe-to-thread model for complex handlers, but its `signal_hook::low_level::register` allows a raw async-signal-safe C function for the restore-and-exit case. This keeps the unsafe signal-handler code minimal and reviewed.

**Alternatives**:
- Raw `libc::sigaction` with a hand-written handler: zero deps, maximum control, maximum unsafe surface. Rejected per user decision (the crate is prescribed by project docs).
- `signal-hook-registry` (lower-level): `signal-hook` re-exports it; no advantage to going lower.

### D9: Fix partial-line stdin reads; skip `SIGWINCH` (parity selective)

**Choice**: Wrap stdin in `std::io::BufReader<std::io::Stdin>` and split on `\n`, accumulating partial lines across `read` calls. `SIGWINCH` (terminal resize) is NOT handled — legacy has `TODO listen to SIGWINCH` (TTY.zig line 131) that was never implemented; parity = skip. The TTY renders once on `enter_mode` and on each input event; without `SIGWINCH`, a resize during a prompt leaves the layout stale until the next keypress triggers a re-render. This matches legacy.

**Rationale**: Legacy's partial-line TODO (pinentry.zig line 118) is a real correctness bug: a command split across two `read` calls would be parsed as two malformed lines. `BufReader` fixes this cheaply and correctly. `SIGWINCH` is a feature gap, not a bug — legacy shipped without it. The user decision was "fix partial-lines only", carrying the `SIGWINCH` gap as parity.

**Alternatives**:
- Match legacy on both (skip partial-line fix): rejected — the fix is cheap and strictly correct.
- Fix both (exceeds parity): rejected — `SIGWINCH` adds a signal handler + re-render path legacy lacks; scope creep.
- Manual line accumulation without `BufReader`: reinvents `BufReader`. Rejected.

### D10: ANSI rendering parity (layout, not pixel parity)

**Choice**: The TTY renderer writes, in order: clear screen + home (`\x1b[2J\x1b[H`); title (bold, green bg, black fg, space-padded); description (default attr, space-padded); prompt (bold, space-padded); if `GetPin` mode: ` > ` + `*` × `min(pin_square_amount, len)` + `_` × `pin_square_amount - len`; error message (bold, red fg); OK button (`enter: <label>`); Not-OK button (`C-c: <label>`); Cancel button (`escape: <label>`). Wrapping via a manual word-wrap at terminal width (parity with `spoon`'s `restrictedPaddingWriter`).

**Rationale**: Legacy `TTY.zig` `render()` (lines 132–169) and helpers `renderContent` (171–189), `renderButton` (191–212) define this exact layout. The `pin_square_amount` comes from `Config::wayland_ui.pin_square_amount` (Stage 1). Terminal dimensions come from `libc::ioctl(fd, TIOCGWINSZ, &winsize)` on `enter_mode` (legacy `term.fetchSize()`). "Terminal too small" guard at width<5 or height<5 (legacy lines 137–141).

**Alternatives**:
- `std::io::Write` with a `crossterm` cursor API: rejected (D6).
- Skip the "terminal too small" guard: rejected (parity).

### D11: NixOS-VM differential parity testing (plan §5 reconciliation)

**Choice**: Add `nixosTests` flake outputs that run the Rust target against the pinned legacy baseline `pkgs.wayprompt` (nixos-26.05, v0.1.2) inside a NixOS VM, asserting byte-identical wire-protocol and TTY behavior. Each implementation stage gets a corresponding `nixosTest` before the next stage begins. The baseline oracle is pinned via a dedicated flake input `nixpkgs-26_05.url = "github:nixos/nixpkgs/nixos-26.05"` so the oracle revision is reproducible and does not drift with `nixos-unstable`.

**Rationale**: `RUST_REWRITE.md` §5 mandates a "`nixosTest` 1:1 staggered testing strategy" with baseline `pkgs.wayprompt` and target `pkgs.nowayprompt`. The archived Stage 0-1 change and the initial Stage 2 artifacts omitted this entirely — zero `nixosTest` derivations, no flake output, no differential oracle. This decision closes that gap by formalizing the strategy as a spec (`nixos-parity-testing`) and a tasks group. "Staggered" means a stage is not done until its `nixosTest` passes against the pinned baseline; this prevents drift accumulating across stages.

**Alternatives**:
- Defer all `nixosTest` to Stage 4: contradicts "staggered" and loses per-stage parity signal. Rejected.
- Use `reference/legacy/` source build as the oracle instead of `pkgs.wayprompt`: the repo's vendored legacy source may not match the pinned nixpkg v0.1.2 byte-for-byte (uncommitted local edits, different build flags). Using the nixpkg as oracle tests real-world interop parity; using the vendored source tests source-parity. Decision: use `pkgs.wayprompt` as the oracle (plan §5 says "against `pinentry-wayprompt`" meaning the shipped binary), AND add a build task (D12) to optionally cross-check the vendored source against the nixpkg so we know if they diverge.
- `cargo` integration tests only (no VM): cannot test `tty1` termios, `SIGINT`/`SIGTSTP` on a real controlling tty, or `/proc/<pid>/maps` leak scans. Rejected as the sole strategy; kept as a fast pre-NixOS gate.

### D12: Minimal target package for nixosTest (Stage 4 slice pulled forward)

**Choice**: Add a minimal `packages.<system>.nowayprompt` via `pkgs.rustPlatform.buildRustPackage` (or `crane`) to `flake.nix` now, producing only the raw `nowayprompt` binary. The full Stage 4 packaging (manpages, `pinentry-nowayprompt` symlink, `wayprompt-ssh-askpass` wrapper, `packages.default` alias) remains a Stage 4 deliverable. The `nixosTest` installs this minimal package, not Stage 4 artifacts.

**Rationale**: `nixosTest` needs a derivable target binary. The plan defers `packages.default` to Stage 4, but "staggered" testing requires the target before Stage 4. Pulling forward a thin `buildRustPackage` slice (no symlinks, no docs) is the minimal reconciliation — it does not deliver Stage 4's user-facing packaging, only the buildable binary the VM test needs. The Stage 4 task then layers symlinks/docs/askpass on top of this existing derivation rather than creating it from scratch.

**Alternatives**:
- Build the Cargo workspace inline inside each `nixosTest` via `crane` ad-hoc: duplicates the build logic across tests. Rejected.
- Wait for Stage 4 for all `nixosTest`: contradicts "staggered". Rejected.
- Make the `nixosTest` `crane`-build the target: same as inline-build; rejected for the same reason.

### D13: Byte-tolerance contract for differential comparison

**Choice**: Differential comparison is byte-identical by default, with an explicit allowed-divergence list: greeting wording (assert `OK ` prefix + non-empty line, not exact suffix — though the target intentionally emits the identical string `OK wayprompt is pleased to meet you`); `GETINFO version` (assert `\d+\.\d+\.\d+` format, not exact digits); `GETINFO pid` (excluded entirely, process-specific); `GETINFO flavor` (byte-identical `D wayprompt\nEND\nOK\n`, target preserves legacy string). Error codes (`536870981`, `83886179`, `83886194`, `536871187`) and their messages are byte-identical. All other output is byte-identical.

**Rationale**: A naive byte-diff would fail on any intentional divergence (version string, pid). An explicit allowlist makes the comparison rigorous where it matters (error codes, protocol framing, `GETINFO flavor`) and tolerant where divergence is intrinsic. The target is designed to emit the legacy-identical greeting and flavor string, so those comparisons are effectively byte-identical in practice; the tolerance is a safety net, not a license to diverge.

**Alternatives**:
- Strict byte-identical everywhere: fails on pid (unavoidable) and version (if the project versions independently). Rejected.
- Fuzzy/string-based comparison: loses rigor on error codes and framing. Rejected.

### D14: TTY byte-capture method in the VM

**Choice**: The Stage 3 `nixosTest` captures TTY output by running the pinentry with its stdout redirected to a pipe/`script`-captured `pts` while its stdin reads from `tty1` (or a `pts` pair), so the raw ANSI bytes are captured for byte comparison. The terminal geometry is forced to a fixed 80x24 via `stty cols 80 rows 24` on the tty before launching, so both baseline and target render at the same geometry. `SIGINT`/`SIGTSTP` are delivered via `kill -INT <pid>` / `kill -TSTP <pid>` from the test driver; termios is read via `stty -a -F /dev/tty1` (or a `tcgetattr`-based helper) before, during, and after.

**Rationale**: Capturing ANSI bytes requires either a `pts` pair (the pinentry writes to a pseudo-terminal, the test reads the master end) or `script(1)`. Forcing the geometry eliminates wrap/resize nondeterminism. `tty1` is used for the termios/signal tests because `pts` pairs do not exercise the real VT driver path the plan names ("virtual console `tty1`"); the byte-capture test can use a `pts` for capture convenience, but the termios/signal tests MUST use `tty1`. This split is explicit in the spec.

**Alternatives**:
- `tmux` capture: adds a tmux dependency to the VM and a layer of escape-processing. Rejected.
- `grim`/framebuffer capture: pixel-level, not byte-level; the plan's Stage 3 is byte/termios-level (Stage 4 is pixel-level with `grim`). Rejected for Stage 3.
- `pts`-only (no `tty1`): violates plan §5 Stage 3 ("on virtual console `tty1`"). Rejected for the termios/signal assertions.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| `signal-hook` handler calls non-async-signal-safe code | Undefined behavior in signal context | Use `signal_hook::low_level::register` with a raw C-style handler that only calls `tcsetattr` + `_exit` (both async-signal-safe). No allocations, no locks in the handler. |
| Hand-rolled input parser misses an escape sequence legacy handles | Key not recognized; user input lost | Legacy handles a fixed small set; unit tests cover each key. Unrecognized escapes → `Unknown` (dropped), matching legacy's silent fallthrough. |
| `poll(2)` with `-1` timeout blocks forever if no fd is ready | Process hangs if both stdin and frontend stall | Legacy uses `-1` (infinite); parity. `gpg-agent` closes stdin on `BYE`, breaking the loop via `POLLHUP` + `stdin_closed`. |
| `BufReader` over stdin changes read granularity vs legacy | Behavioral divergence in multi-line reads | `BufReader` only changes partial-line handling; full-line semantics unchanged. Tests feed multi-line and partial-line streams. |
| `SETKEYINFO` silently accepted but key-caching never implemented | gpg-agent expects caching; user gets none | Parity with legacy (which also never implemented it). Documented in `HELP` output (legacy lists only implemented commands). |
| `CONFIRM --one-button` ignored | `--one-button` confirm acts as two-button | Legacy TODO (line 301); parity = ignore the flag. |
| `GETINFO pid` on non-Linux | Legacy guards `builtin.os.tag == .linux`; non-Linux emits only `OK` | `std::process::id()` is cross-platform; emit pid on all platforms. Minor divergence from legacy (stricter parity would cfg-gate, but `std::process::id` is always safe). Decision: emit on all platforms (strict superset of legacy behavior; no regression). |
| TTY `ioctl(TIOCGWINSZ)` fails on non-TTY fds | `enter_mode` error | `Frontend::init` must be given a real TTY fd (from `cfg.tty_name`); error propagates as `FrontendError`. Legacy same (returns `error.NoTTYNameSet` if no tty_name). |
| `pkgs.wayprompt` v0.1.2 diverges from `reference/legacy/` source | Differential test proves nixpkg parity, not source parity | Add a build task (D12) cross-checking the vendored legacy source build against the nixpkg binary; if they diverge, document which is authoritative. |
| `nixosTest` VM boot is slow (~30–60s) | Slows the dev feedback loop | `cargo test` runs first as the fast gate; `nixosTest` runs as the parity gate, not on every save. CI runs `nixosTest` on PRs. |
| `pts`-captured ANSI bytes differ between terminals | False parity failures | Force fixed 80x24 geometry via `stty cols/rows` before launch; both baseline and target run under the same `pts` driver. |
| `tty1` termios read requires root/permissions in VM | Test cannot read `/dev/tty1` termios | `nixosTest` VM runs as root; `stty -a -F /dev/tty1` is available. The pinentry itself runs as the test user but the driver reads as root. |