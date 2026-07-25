## 1. Dependency & Scaffold

- [ ] 1.1 Add `signal-hook = "0.3"` to `Cargo.toml` `[dependencies]`; run `cargo build` to verify resolution and update `Cargo.lock`
- [ ] 1.2 Create `src/frontend/mod.rs` (trait + enums skeleton) and `src/protocol/mod.rs` (module root); wire `mod frontend; mod protocol;` in `src/main.rs`
- [ ] 1.3 Run `cargo build` and `cargo clippy -- -D warnings` to verify the scaffold compiles with zero warnings

## 2. Frontend Trait & Enums (`src/frontend/mod.rs`)

- [ ] 2.1 Implement `Event` enum (`None`, `UserOk`, `UserAbort`, `UserNotOk`) with `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`
- [ ] 2.2 Implement `InterfaceMode` enum (`None`, `GetPin`, `Message`) with `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`
- [ ] 2.3 Implement `FrontendError` enum (`Init(String)`, `Io(std::io::Error)`, `InvalidMode(String)`) implementing `std::error::Error` + `Display`
- [ ] 2.4 Define `Frontend` trait: `init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>`, `deinit(&mut self)`, `enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>`, `handle_event(&mut self) -> Result<Event, FrontendError>`, `flush(&mut self) -> Result<Option<Event>, FrontendError>`, `no_event(&mut self) -> Result<(), FrontendError>`

## 3. Assuan Percent-Decode & Hotkey Strip (`src/protocol/assuan.rs`)

- [ ] 3.1 Implement `AssuanError` enum (`DecodeError(&'static str)`, `Io(std::io::Error)`) implementing `std::error::Error` + `Display`
- [ ] 3.2 Implement `assuan_decode(input: &str, strip_hotkey: bool) -> Result<String, AssuanError>`: two-pass parity with legacy `pinentryDupe` — pass 1 computes output length (`-2` per `%`, `-1` per `_` when `strip_hotkey`), pass 2 decodes `%XX` via `u8::from_str_radix(_, 16)` and skips `_` when `strip_hotkey`; error on malformed `%` (<2 trailing bytes or non-hex)
- [ ] 3.3 Write unit tests for `assuan_decode`: `"foo%20bar"` → `"foo bar"`, `"_Cancel"` with `true` → `"Cancel"`, `"_Cancel"` with `false` → `"_Cancel"`, `"foo%2"` → Err, `"foo%ZZbar"` → Err, multi-byte `%C3%A9` → `"é"`, empty input → `""`

## 4. Assuan REPL State & Command Dispatch (`src/protocol/assuan.rs`)

- [ ] 4.1 Implement `AssuanMode` enum (`None`, `GetPin`, `Confirm`, `Message`) (Assuan-level mode, distinct from `InterfaceMode`)
- [ ] 4.2 Implement `AssuanRepl<W: Write>` struct holding: `writer: W`, `mode: AssuanMode`, `default_ok/cancel/yes/no: Option<String>`, `is_running: bool`, and a reference to shared `Config` + `SecretBuffer` (design how state is shared with the dispatch loop — likely `&mut Config` + `&mut SecretBuffer` passed per-`handle_line` call, or stored in the struct)
- [ ] 4.3 Implement `AssuanRepl::new(writer) -> io::Result<Self>` emitting `OK wayprompt is pleased to meet you\n` and flushing
- [ ] 4.4 Implement command dispatch: case-insensitive command token split from args; `if mode != None { return Ok(()) }` guard (drop commands during active prompt, parity line 276)
- [ ] 4.5 Implement `SETTITLE`/`SETPROMPT`/`SETDESC`/`SETERROR`/`SETOK`/`SETNOTOK`/`SETCANCEL`: decode with `assuan_decode(args, false)`, store in `config.labels.<field>` (replacing prior value), emit `OK\n`
- [ ] 4.6 Implement `GETPIN`: apply `default_ok`→`labels.ok` and `default_cancel`→`labels.cancel` defaults if `None`, set mode=`GetPin`, call `frontend.enter_mode(GetPin)` (frontend reference threaded through); the actual event handling happens in the dispatch loop, not `handle_line` — design how `GETPIN` returns control to the poll loop
- [ ] 4.7 Implement `CONFIRM`: apply `default_yes`→`labels.ok` and `default_no`→`labels.cancel` defaults, set mode=`Confirm` (frontend mode `Message`), call `frontend.enter_mode(Message)`
- [ ] 4.8 Implement `MESSAGE`: if `title`/`description`/`err_message` all `None`, emit `OK\n` and return; else set mode=`Message`, call `frontend.enter_mode(Message)`
- [ ] 4.9 Implement `GETINFO`: `flavor`→`D wayprompt\nEND\n`; `version`→`D 0.0.0\nEND\n`; `pid`→`D <std::process::id()>\nEND\n`; then `OK\n`; unknown/missing subcommand → `OK\n` only
- [ ] 4.10 Implement `BYE`: emit `OK\n`, set `is_running=false`; `RESET`: call `Config::reset()`, emit `OK\n`; `NOP`: emit `OK\n`; `HELP`: emit `# NOP\n# SETTITLE\n# SETPROMPT\n# SETDESC\n# SETERROR\n# GETPIN\n# BYE\n# OPTION\n# RESET\n` then `OK\n`
- [ ] 4.11 Implement `OPTION` prefix-matching (legacy `getOption` on the whole token, not `split_once('=')`): `putenv=WAYLAND_DISPLAY=` → `config.wayland_display`; `ttyname=` → `config.tty_name`; `default-ok=`/`default-cancel=`/`default-yes=`/`default-no=` → runtime `default_*` (decoded with `strip_hotkey=true`); unknown options silently accepted with `OK\n`
- [ ] 4.12 Implement silently-accepted set: `SETKEYINFO` → `OK\n` (no state change)
- [ ] 4.13 Implement not-implemented set: `CANCEL`, `SETGENPIN`, `SETGENPIN_TT`, `SETTIMEOUT`, `END`, `QUIT`, `AUTH`, `CLEARPASSPHRASE`, `SETREPEAT`, `SETREPEATERROR`, `SETQUALITYBAR`, `SETQUALITYBAR_TT` → `ERR 536870981 Not implemented\n`
- [ ] 4.14 Implement unknown command fallback: `ERR 536871187 Unknown IPC command\n`
- [ ] 4.15 Implement `handle_frontend_event(event: Event)`: map `UserOk`/`UserAbort`/`UserNotOk` to the correct response per current `mode` (getpin streams secret via `D`/`END`/`OK` or `OK` if empty; confirm/message emit `OK`/`ERR 83886179`/`ERR 83886194`); then clear `err_message`, reset `SecretBuffer`, set `mode=None`

## 5. Zero-Copy Secret Streaming (`src/protocol/assuan.rs`)

- [ ] 5.1 Implement `dump_pin(writer: &mut W, secret: Option<&[u8]>)`: if `Some(bytes)`, write `D ` then `writer.write_all(bytes)` then `\nEND\nOK\n`; if `None`, write `OK\n` only. No `format!`/`String` allocation holding the secret.
- [ ] 5.2 Write unit test with an in-memory `Vec<u8>` writer: empty secret → `b"OK\n"`; non-empty → `b"D hunter2\nEND\nOK\n"`; assert no intermediate `String` allocation (verify via the writer contents only — the test asserts the output bytes match)
- [ ] 5.3 Write unit test that `dump_pin` does not panic on a 1000-byte secret (parity with legacy large-pin handling)

## 6. TTY Frontend — termios & Signals (`src/frontend/tty.rs`)

- [ ] 6.1 Implement `RawTty` struct `{ fd: RawFd, orig_termios: libc::termios }` with `new(fd)` (tcgetattr, clear `ECHO|ICANON|ISIG`, set `VMIN=1/VTIME=0`, tcsetattr `TCSAFLUSH`) and `restore()` (tcsetattr orig, `TCSAFLUSH`); `Drop` calls `restore`
- [ ] 6.2 Implement `static` storage for signal-handler access: `static ORIG_TERMIOS: Mutex<Option<libc::termios>>` or `AtomicPtr`; `static TTY_FD: AtomicI32`
- [ ] 6.3 Register signal handlers via `signal_hook::low_level::register` for `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`, `SIGTSTP` calling a raw C-style handler that does `tcsetattr(TTY_FD, TCSAFLUSH, &ORIG_TERMIOS)` then `libc::_exit(0)` — verify async-signal-safety (no allocations/locks in handler)
- [ ] 6.4 Implement `Tty::init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>`: if `cfg.tty_name` is `None`, return `Err(Init)`; open the tty fd `O_RDWR`; store cfg reference; return fd
- [ ] 6.5 Implement `Tty::deinit`: restore termios (drop `RawTty`), close fd
- [ ] 6.6 Implement `Tty::enter_mode(mode)`: if `None`, restore termios (cook); else assert current mode is `None`, enter raw mode via `RawTty::new`, query size via `ioctl(TIOCGWINSZ)`, set window title via `\x1b]2;<title>\x07`, call `render`
- [ ] 6.7 Write unit test for `RawTty` raw-mode flag clearing (use a pseudo-tty `openpty` or skip on CI — guard with `#[cfg(unix)]` and `#[ignore]` if no tty available; test the flag math by mocking `termios`)

## 7. TTY Frontend — ANSI Renderer (`src/frontend/tty.rs`)

- [ ] 7.1 Implement `clear_and_home(writer)` writing `\x1b[2J\x1b[H`
- [ ] 7.2 Implement `render_content(writer, str, attr, line, width)`: split `str` on `\n` (legacy `LineIterator`), for each line write at `line`/col 0 with leading space, apply attr (bold/red/green-bg via SGR), pad to width if `bg != none`; increment `line`; append a blank line after
- [ ] 7.3 Implement `render_button(writer, key: &str, label: &str, line, width)`: first line writes ` <key>: <label>`; continuation lines indent by `key.len() + 2`; wrap at width
- [ ] 7.4 Implement `render(writer, tty)`: clear+home; if width<5 or height<5 write `Terminal too small!` (bold red) and return; render title (bold green-bg black-fg), description (default), prompt (bold); if mode=`GetPin` render ` > ` + `*`×`min(pin_square_amount, len)` + `_`×`pin_square_amount - len`; render err_message (bold red); render ok/not_ok/cancel buttons
- [ ] 7.5 Write unit test for `render` GetPin pin row: `len=3`, `pin_square_amount=8` → ` > ***_____` (3 stars, 5 underscores); `len=0` → ` > ________` (8 underscores); `len=10` → ` > ********` (8 stars, capped)
- [ ] 7.6 Write unit test for terminal-too-small guard: width=4 → renders only `Terminal too small!`
- [ ] 7.7 Write unit test for button rendering: `ok=Some("OK")` → line contains `enter: OK`

## 8. TTY Frontend — Input Parser (`src/frontend/tty.rs`)

- [ ] 8.1 Implement `TtyInput` enum (`Enter`, `Escape`, `Backspace`, `C_c`, `C_u`, `C_w`, `C_backspace`, `Codepoint(char)`, `Unknown`)
- [ ] 8.2 Implement `parse_input(buf: &[u8]) -> Vec<TtyInput>`: scan bytes; `\r`/`\n`→Enter; `\x1b`→ if next byte exists and is a letter (escape sequence like `[A`), consume the sequence and return `Unknown`; if standalone, `Escape`; `\x7f`→Backspace; `\x03`→C_c; `\x15`→C_u; `\x17`→C_w; `\x08`→C_backspace; else decode UTF-8 from the lead byte and return `Codepoint` (or `Unknown` if invalid/modified)
- [ ] 8.3 Write unit tests: `b"\r"`→`[Enter]`; `b"\x7f"`→`[Backspace]`; `b"\xC3\xA9"`→`[Codepoint('é')]`; `b"\x1b[A"`→`[Unknown]`; `b"\x1b"`→`[Escape]`; `b"\x1ba"`→`[Unknown]` (Alt+a dropped); `b"abc"`→`[Codepoint('a'),Codepoint('b'),Codepoint('c')]`
- [ ] 8.4 Implement `Tty::handle_event`: `libc::read(fd, buf, 32)`, parse via `parse_input`, for each input apply the per-mode rules (Enter→UserOk, Escape→UserAbort, C-c→UserNotOk if not_ok else UserAbort, C-u/C-w/C-backspace→reset+render in GetPin, Backspace→delete_backwards+render in GetPin, Codepoint→append_slice+render in GetPin); return first terminal `Event` or continue reading

## 9. TTY Frontend — flush/no_event stubs (`src/frontend/tty.rs`)

- [ ] 9.1 Implement `Tty::flush` returning `Ok(None)` (blocking frontend has no pending events)
- [ ] 9.2 Implement `Tty::no_event` as a no-op returning `Ok(())`
- [ ] 9.3 Verify the `Frontend` trait is implemented for `Tty` (compile-time check)

## 10. Poll-Based Dispatch Loop (`src/main.rs`)

- [ ] 10.1 Implement the pinentry entrypoint: set `RLIMIT_CORE=0` (call existing `secret::set_rlimit_core_zero()`), init `SecretBuffer`, init `Config` (parse + `allow_tty_fallback=true`), init frontend (`Tty`), init `AssuanRepl`
- [ ] 10.2 Implement the `poll(2)` loop over stdin (fd 0) + frontend fd; `POLLIN` on both; track `stdin_closed`; `if stdin_closed { poll only frontend }`; timeout `-1` (block)
- [ ] 10.3 On `POLLIN` on stdin: read via `BufReader`, split on `\n`, dispatch each line via `AssuanRepl::handle_line`; on `POLLHUP` set `stdin_closed=true`
- [ ] 10.4 On `POLLIN` on frontend fd: call `frontend.handle_event()`, pass result to `AssuanRepl::handle_frontend_event`, which emits the response; after terminal event call `frontend.enter_mode(None)`
- [ ] 10.5 Call `frontend.flush()` at loop top (returns `None` for TTY); if `Some(event)`, dispatch via `handle_frontend_event`; on error emit `ERR 83886179 Operation cancelled\n`
- [ ] 10.6 Call `frontend.no_event()` when `POLLIN` not set on frontend fd (no-op for TTY)
- [ ] 10.7 Handle `out_buffer.flush()` broken pipe on `BYE` (legacy lines 146–151): if `is_running==false` and `BrokenPipe`, break loop
- [ ] 10.8 Exit conditions: `is_running==false` after `BYE`, or `stdin_closed && mode==None`

## 11. Config Reset Contract (`src/config.rs`)

- [ ] 11.1 Implement `Config::reset(&mut self)`: set all `Labels` fields (`title`, `description`, `prompt`, `err_message`, `ok`, `not_ok`, `cancel`) to `None`; match legacy `config.reset(alloc)` semantics (the legacy also resets `wayland_ui` but that is config-file state, not Assuan-state; reset only labels for parity with the Assuan `RESET` command)
- [ ] 11.2 Write unit test: populate labels, call `reset()`, assert all are `None`

## 12. Integration & Parity Verification

- [ ] 12.1 Write integration test: feed `SETTITLE T\nSETPROMPT P\nSETDESC D\nGETPIN\n` + simulated Enter event to a TTY frontend backed by a pseudo-tty; assert stdout emits greeting + `OK\n`×3 + `D <pin>\nEND\nOK\n`
- [ ] 12.2 Write integration test: `GETPIN\n` + Escape event → `ERR 83886179 Operation cancelled\n`
- [ ] 12.3 Write integration test: `GETPIN\n` + Ctrl+C with `not_ok` set → `ERR 83886194 not confirmed\n`
- [ ] 12.4 Write integration test: `BYE\n` → `OK\n` and loop exits
- [ ] 12.5 Write integration test: `SETKEYINFO X\n` → `OK\n` (silently accepted)
- [ ] 12.6 Write integration test: `SETTIMEOUT 30\n` → `ERR 536870981 Not implemented\n`
- [ ] 12.7 Write integration test: `BOGUS\n` → `ERR 536871187 Unknown IPC command\n`
- [ ] 12.8 Write integration test: partial-line stdin (`SETT` then `ITLE X\n`) → single `OK\n` (partial-line fix)
- [ ] 12.9 Write integration test: `GETINFO flavor`/`version`/`pid` produce `D wayprompt\nEND\nOK\n`, `D 0.0.0\nEND\nOK\n`, `D <pid>\nEND\nOK\n`
- [ ] 12.10 Run `cargo test` (full suite) and verify all Stage 2 tests pass
- [ ] 12.11 Run `cargo clippy -- -D warnings` and verify zero warnings
- [ ] 12.12 Run `cargo fmt --check` and verify formatting compliance
- [ ] 12.13 Run `nix develop --command cargo test` to verify tests pass inside the Nix dev shell
- [ ] 12.14 Cross-reference legacy `wayprompt-pinentry.zig` command matrix and `TTY.zig` key handling; confirm behavioral parity on all test vectors
- [ ] 12.15 Verify no `tokio`/`async-std`/`futures`/`crossterm`/`termion` linkage in Stage 2 build (`Cargo.toml` deps only `libc`, `zeroize`, `memmap2`, `signal-hook`)

## 13. NixOS Parity Testing — Flake & Package

- [ ] 13.1 Add `nixpkgs-26_05.url = "github:nixos/nixpkgs/nixos-26.05"` input to `flake.nix` (pinned oracle revision); add `nixpkgs-26_05` to `outputs` args
- [ ] 13.2 Add minimal `packages.<system>.nowayprompt` via `pkgs.rustPlatform.buildRustPackage` (or `crane`): build `Cargo.toml` workspace, produce `nowayprompt` binary only (no symlinks, no manpages — Stage 4 layers those); verify `nix build .#nowayprompt` succeeds and the binary runs
- [ ] 13.3 Cross-check: build `reference/legacy/` Zig source via `pkgs.zig.buildPackage` (or `nix run nixpkgs#zig` + `zig build`) and compare the resulting `pinentry-wayprompt` binary against `pkgs.wayprompt` from `nixpkgs-26_05`; document any divergence (determines whether `reference/legacy/` == nixpkg v0.1.2)
- [ ] 13.4 Add `nixosTests` flake output skeleton (`flake.nix` `nixosTests` attrset) wired to `nixpkgs.lib.nixosTest` for each stage

## 14. NixOS Test — Stage 1 CLI & Config Parity (backfill)

- [ ] 14.1 Write `nixosTests.stage-1-cli-config`: minimal VM, install both `pkgs.wayprompt` (nixos-26_05) and `packages.x86_64-linux.nowayprompt`; assert `--version` exit 0 and non-empty stdout for both; assert `--help` exit 0 for both
- [ ] 14.2 In the VM, write a sample `wayprompt.5` config (trailing semicolons, `[colours]` hex, inline `#` comments) and load it via both binaries (use the CLI config path or `XDG_CONFIG_HOME`); assert both parse without error
- [ ] 14.3 In the VM, write a malformed config (unknown section, bad color) and assert both binaries emit an error and exit non-zero (or emit the same error line); compare error output for parity
- [ ] 14.4 Run `nix build .#nixosTests.stage-1-cli-config` and verify the VM test passes

## 15. NixOS Test — Stage 2 Assuan IPC Parity

- [ ] 15.1 Write `nixosTests.stage-2-assuan`: minimal VM (no display server), install both binaries; write a Python test driver that pipes an Assuan command stream to each binary's stdin and captures stdout
- [ ] 15.2 Define the shared command stream: greeting check, `SETTITLE`/`SETPROMPT`/`SETDESC`/`SETERROR`/`SETOK`/`SETNOTOK`/`SETCANCEL` (with `%XX` and `_hotkey` cases), `GETPIN` (empty + non-empty, driven by a scripted frontend that writes a fixed pin to the configured TTY and sends Enter), `CONFIRM`, `MESSAGE`, `GETINFO flavor`/`version`/`pid`, `OPTION ttyname`/`default-ok`/`default-cancel`/`default-yes`/`default-no`/`putenv`, `BYE`, `RESET`, `NOP`, `HELP`, `SETKEYINFO`, not-implemented set, unknown command, partial-line (`SETT`/`ITLE X\n`)
- [ ] 15.3 Implement the `GETPIN` scripted-frontend harness: configure `OPTION ttyname=/dev/<pts>`, write the pin bytes to the pts, send `\r` (Enter), capture the `D <pin>\nEND\nOK\n` (or `OK\n` for empty) from both binaries
- [ ] 15.4 Implement the byte-tolerance comparator: byte-identical by default; for `GETINFO pid` exclude the line; for `GETINFO version` assert `\d+\.\d+\.\d+` format; for greeting assert `OK ` prefix + non-empty; everything else byte-identical
- [ ] 15.5 Run `nix build .#nixosTests.stage-2-assuan` and verify the VM test passes (target matches baseline within tolerance)

## 16. NixOS Test — Stage 3 Virtual TTY Console Parity

- [ ] 16.1 Write `nixosTests.stage-3-tty`: NixOS VM with `services.kmscon.enable = false` + `services.getty.tty1` (or agetty on `tty1`); install both binaries; test driver runs as root
- [ ] 16.2 Termios-flag test: read `/dev/tty1` termios via `stty -a -F /dev/tty1` before launch; launch `nowayprompt` with `OPTION ttyname=/dev/tty1` + `GETPIN`; read termios during prompt; assert `ECHO`, `ICANON`, `ISIG` are cleared; compare against baseline `pinentry-wayprompt` on the same `tty1`
- [ ] 16.3 Signal-restoration test: launch pinentry on `tty1`, send `SIGINT` via `kill -INT <pid>` from the driver, read `tty1` termios after exit, assert it matches the pre-prompt cooked state; repeat with `SIGTSTP` (verify restore on stop; verify cooked state)
- [ ] 16.4 ANSI byte-capture test: redirect pinentry stdout to a `pts`/pipe while stdin reads from a fixed-geometry (80x24 via `stty cols 80 rows 24`) tty; drive identical keystream; capture stdout bytes from both binaries; assert byte-identical (`\x1b[2J`, `\x1b[H`, title/desc/prompt/pin-row/buttons)
- [ ] 16.5 Zero-leak test: launch `nowayprompt`, enter pin `"hunter2"` via `GETPIN`, after exit scan `/proc/<pid>/maps` (before reap) for an mlocked page containing `hunter2` — assert none; alternatively `strings /proc/<pid>/maps` (or a core if RLIMIT_CORE allowed) — assert `hunter2` absent; run the same against baseline and assert target is at least as leak-free (target uses `MADV_DONTDUMP` + zeroize; baseline may not)
- [ ] 16.6 Run `nix build .#nixosTests.stage-3-tty` and verify the VM test passes
- [ ] 16.7 Run all three `nixosTest`s sequentially: `nix build .#nixosTests.stage-1-cli-config .#nixosTests.stage-2-assuan .#nixosTests.stage-3-tty`; verify all pass