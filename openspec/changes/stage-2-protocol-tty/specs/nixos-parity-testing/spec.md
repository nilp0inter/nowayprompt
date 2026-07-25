## ADDED Requirements

### Requirement: NixOS VM-based differential parity harness

The repository MUST define `nixosTests` flake outputs that run the Rust `nowayprompt` target against the legacy `pkgs.wayprompt` (nixos-26.05, v0.1.2) baseline inside a NixOS VM, asserting byte-identical wire-protocol and TTY behavior. Each implementation stage (per `RUST_REWRITE.md` §4) MUST have a corresponding `nixosTest` (per `RUST_REWRITE.md` §5) that runs before the next implementation stage begins. Tests are "staggered": a stage is not considered done until its `nixosTest` passes against the pinned legacy baseline.

The baseline oracle MUST be `pkgs.wayprompt` from `github:nixos/nixpkgs/nixos-26.05` (v0.1.2), pinned via a flake input so the oracle revision is reproducible and does not drift with `nixos-unstable`. The target MUST be the `nowayprompt` binary built from this repository.

#### Scenario: Staggered test per implementation stage
- **WHEN** implementation Stage N is complete
- **THEN** `nix build .#nixosTests.stage-N-<name>` MUST succeed, running the target and baseline through the same scripted scenario and asserting 1:1 parity

#### Scenario: Reproducible oracle revision
- **WHEN** the `nixosTest` is evaluated on a different machine
- **THEN** `pkgs.wayprompt` resolves to the same nixos-26.05 revision via the pinned flake input, so the baseline behavior is fixed

### Requirement: Target package derivable for nixosTest

The flake MUST expose a buildable `nowayprompt` package (`packages.<system>.nowayprompt` or a `nixosTest`-internal `crane`/`buildRustPackage` derivation) so `nixosTest` can install the target binary without waiting for the full Stage 4 packaging (manpages, askpass symlink, `pinentry-` symlink). A minimal `buildRustPackage` derivation producing the `nowayprompt` binary is sufficient for Stage 2 tests; the full `packages.default` with symlinks and docs is a Stage 4 deliverable. The test MUST NOT depend on Stage 4 artifacts.

#### Scenario: nixosTest installs the target
- **WHEN** the `nixosTest` VM configuration references the target package
- **THEN** `environment.systemPackages` includes a derivable `nowayprompt` binary built from the repo `Cargo.toml`, independent of Stage 4 packaging

#### Scenario: No Stage 4 dependency
- **WHEN** Stage 4 artifacts (symlinks, manpages, askpass wrapper) are absent from the repo
- **THEN** the Stage 2 `nixosTest` still builds and runs, because it installs the raw `nowayprompt` binary, not the Stage 4 deliverables

### Requirement: Stage 2 test — Assuan IPC stream parity

The flake MUST define `nixosTests.stage-2-assuan` that boots a minimal NixOS VM (no display server) with both `pinentry-wayprompt` (baseline) and `nowayprompt` (target) installed, and drives each through an identical scripted Assuan stdin stream, asserting byte-identical stdout. The test stream MUST exercise: greeting line, `SETTITLE`/`SETPROMPT`/`SETDESC`/`SETERROR`/`SETOK`/`SETNOTOK`/`SETCANCEL` (with `%XX` and `_hotkey` cases), `GETPIN` (with empty and non-empty secret), `CONFIRM`, `MESSAGE`, `GETINFO flavor/version/pid` (pid excluded from byte-comparison), `OPTION ttyname`/`default-ok`/`default-cancel`/`default-yes`/`default-no`/`putenv`, `BYE`, `RESET`, `NOP`, `HELP`, `SETKEYINFO` (silent accept), the not-implemented set (`ERR 536870981`), and unknown commands (`ERR 536871187`). The test MUST run each command against both binaries with identical stdin and compare stdout (excluding the `pid` line, which is process-specific). `GETPIN` MUST be driven by a scripted frontend: the test feeds `GETPIN\n` then writes a fixed pin to the frontend's TTY fd and sends a simulated Enter, capturing the `D <pin>\nEND\nOK\n` (or `OK\n` for empty) from both binaries and comparing.

#### Scenario: Greeting parity
- **WHEN** both binaries start with empty stdin
- **THEN** both emit `OK Pleased to meet you...` (case-insensitive suffix match; the exact wording may differ but the `OK ` prefix and presence of a greeting MUST match — see D13 for the exact tolerance)

#### Scenario: SETDESC percent-decode parity
- **WHEN** both binaries receive `SETDESC Foo%20Bar\n` followed by `GETPIN\n` and a fixed pin
- **THEN** both emit identical `OK\n` then `D <pin>\nEND\nOK\n` sequences

#### Scenario: Not-implemented set parity
- **WHEN** both binaries receive `SETTIMEOUT 30\n`
- **THEN** both emit `ERR 536870981 Not implemented\n`

#### Scenario: Unknown command parity
- **WHEN** both binaries receive `BOGUS\n`
- **THEN** both emit `ERR 536871187 Unknown IPC command\n`

### Requirement: Stage 3 test — Virtual TTY console fallback parity

The flake MUST define `nixosTests.stage-3-tty` that boots a NixOS VM with a virtual console (`tty1` via `services.kmscon` or `agetty` on `tty1`), installs both `pinentry-wayprompt` and `nowayprompt`, and drives each on `tty1` with a scripted keystream. The test MUST assert:

1. **Raw termios flag clearing**: after the pinentry enters raw mode on `tty1`, the test reads `/proc/<pid>/fd/0` termios via a helper (or `stty -a -F /dev/tty1` parsed) and asserts `ECHO`, `ICANON`, and `ISIG` are cleared.
2. **Signal restoration on `SIGINT`/`SIGTSTP`**: the test sends `SIGINT` and `SIGTSTP` to the running pinentry via `kill`, then asserts `tty1` termios is restored to the pre-prompt cooked state (the test must read termios before launching the pinentry, store it, and compare after signal delivery / process exit).
3. **ANSI cursor control byte capture**: the test captures the bytes written to `tty1` (via `script` / `tmux` / a `pts` capture, or by redirecting the pinentry's stdout to a pipe while keeping stdin on the tty) and asserts the presence of `\x1b[2J` (clear) and `\x1b[H` (home) and the ` > ` pin-row prefix; the byte stream MUST be byte-identical between baseline and target for the same input sequence (same terminal geometry).
4. **Zero password buffer leak**: the test runs the pinentry with `RLIMIT_CORE=0` asserted, enters a pin, then after exit scans `/proc/<pid>/maps` (before the process is reaped) or uses a `gdb`-attached `info proc mappings` to assert no `mlock`ed page with the pin bytes remains; alternatively, asserts that the pin string does NOT appear in a `strings` dump of any `/proc/<pid>/*` readable file or core (no core exists due to `RLIMIT_CORE=0` + `MADV_DONTDUMP`). The baseline legacy uses `alloc`+`mlock` without `MADV_DONTDUMP`; the target MUST do at least as well (strict superset: target MUST NOT leak where baseline might).

#### Scenario: termios flags cleared on tty1
- **WHEN** `nowayprompt` enters raw mode on `tty1`
- **THEN** `stty -a -F /dev/tty1` (or equivalent termios read) shows `-echo -icanon -isig`

#### Scenario: SIGINT restores termios
- **WHEN** `nowayprompt` is in raw mode on `tty1` and receives `SIGINT`
- **THEN** `tty1` termios is restored to the pre-prompt cooked state before the process exits

#### Scenario: SIGTSTP restore on resume
- **WHEN** `nowayprompt` is in raw mode on `tty1` and receives `SIGTSTP`
- **THEN** `tty1` termios is restored; on `SIGCONT`/resume the process re-enters raw mode (or exits cleanly, matching legacy behavior)

#### Scenario: ANSI byte parity
- **WHEN** both binaries render a `GetPin` prompt with identical labels on `tty1` at 80x24 geometry
- **THEN** the captured stdout byte streams match byte-for-byte (clear, home, title, description, prompt, pin row, buttons)

#### Scenario: No secret in /proc maps or core
- **WHEN** `nowayprompt` exits after a `GETPIN` with pin `"hunter2"`
- **THEN** no `/proc/<pid>/maps` entry or post-exit memory scan contains the bytes `hunter2`; the `mlock`ed page is `munmap`ed on drop

### Requirement: Stage 1 backfill — CLI & Config parity

The flake MUST define `nixosTests.stage-1-cli-config` that boots a minimal VM (no display server), installs both binaries, and asserts: identical `--help`/`--version` exit codes and output (or documented tolerance for version-string differences), identical `wayprompt.5` INI parsing behavior (feed the same config file to both, exercise the config path, assert no parse errors on valid configs and identical error lines on invalid configs), and identical exit codes for the CLI subcommands exercised without a display. This test was omitted from the archived Stage 0-1 change and MUST be backfilled so the staggered strategy is complete from Stage 1 forward.

#### Scenario: --version exit code parity
- **WHEN** both binaries are invoked with `--version`
- **THEN** both exit 0 (version string content may differ; only exit code and non-empty stdout are asserted)

#### Scenario: INI parse parity
- **WHEN** both binaries load the same `wayprompt.5` config with trailing semicolons, inline `#` comments, and `[colours]` hex values
- **THEN** both parse without error (or both emit the same error line for a malformed config)

### Requirement: Byte-tolerance contract for differential comparison

Differential byte comparison MUST account for known-allowed divergences between baseline and target, documented explicitly in the test. Allowed divergences:
- **Greeting wording**: legacy emits `OK wayprompt is pleased to meet you`; target emits `OK wayprompt is pleased to meet you` (intentional identical string — if the target wording diverges, the test MUST assert the `OK ` prefix and that the line is non-empty, not the exact suffix).
- **`GETINFO version`**: legacy `0.0.0`; target may differ if the project versions independently — assert format `D X.Y.Z\nEND\nOK\n`, not the exact version.
- **`GETINFO pid`**: always excluded from byte comparison (process-specific).
- **`GETINFO flavor`**: MUST be byte-identical (`D wayprompt\nEND\nOK\n`) if the target preserves the legacy flavor string, OR the test MUST assert the target's flavor string matches its own identity (decision: target emits `D wayprompt\nEND\nOK\n` for legacy parity; the test asserts byte-identical).
- **Error message text**: `ERR <code> <message>` — the `<code>` MUST match byte-for-byte; the `<message>` text MUST match for the not-implemented and unknown-command cases (legacy uses fixed strings `Not implemented` / `Unknown IPC command`); the cancellation/not-confirmed messages MUST match byte-for-byte.

Anything not in the allowed-divergence list MUST be byte-identical between baseline and target.

#### Scenario: Greeting tolerance
- **WHEN** baseline emits `OK wayprompt is pleased to meet you\n` and target emits `OK wayprompt is pleased to meet you\n`
- **THEN** the comparison passes (identical); if the target wording diverges, the comparison asserts `OK ` prefix + non-empty suffix only

#### Scenario: Error code byte-identical
- **WHEN** both binaries receive an unknown command
- **THEN** both emit `ERR 536871187 Unknown IPC command\n` byte-for-byte (code and message identical)

#### Scenario: Version format-only
- **WHEN** both binaries receive `GETINFO version\n`
- **THEN** both emit `D <X.Y.Z>\nEND\nOK\n` where `<X.Y.Z>` matches the regex `\d+\.\d+\.\d+`; the exact digits need not match