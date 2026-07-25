## ADDED Requirements

### Requirement: Geometry-only nixosTest under headless cage

The `nixosTests.stage-3-wayland` derivation MUST run the `nowayprompt-wayland-test` `[[bin]]` target under a headless `cage` compositor with `WLR_BACKEND=headless`, `WLR_RENDERER=pixman` (software), and `WLR_LIBINPUT_NO_DEVICES=1`. The test MUST assert surface geometry (configure serial ack, width/height, scale), keyboard-driven `Event` emission, and `wl_buffer` dimensions. It MUST NOT perform `grim` pixel capture (Stage 4's contract).

#### Scenario: surface configures and acks
- **WHEN** the test binary connects to `cage` and enters `GetPin` mode
- **THEN** it receives a `configure` event, acks the serial, and logs the configured dimensions

#### Scenario: keyboard Return emits UserOk
- **WHEN** `wtype` sends a Return keypress to the `cage` surface
- **THEN** the test binary logs `Event::UserOk` and the driver asserts it

#### Scenario: keyboard Escape emits UserAbort
- **WHEN** `wtype` sends an Escape keypress
- **THEN** the test binary logs `Event::UserAbort` and the driver asserts it

### Requirement: Test-only binary target

The `Cargo.toml` MUST define a second `[[bin]]` target `nowayprompt-wayland-test` (path `src/bin/wayland-test.rs`) that instantiates `Wayland::new()` + `init(cfg)` and drives the frontend directly, without going through `main.rs`. The primary `nowayprompt` pinentry binary MUST remain TTY-only (Stage 4 wires frontend selection).

#### Scenario: test binary exists and builds
- **WHEN** `cargo build --bin nowayprompt-wayland-test` is run
- **THEN** the binary builds successfully and is distinct from the main `nowayprompt` binary

#### Scenario: main.rs unchanged
- **WHEN** Stage 3 is complete
- **THEN** `src/main.rs` still hardcodes `Tty::new()` and does not reference `Wayland`

### Requirement: Reusable cage harness for Stage 4

The `nixosTests.stage-3-wayland` harness (cage + wtype + headless VM env vars + Python driver pattern) MUST be structured so Stage 4 can extend it by adding `grim` frame capture and swapping the test binary for the real `nowayprompt` pinentry.

#### Scenario: harness reusable
- **WHEN** Stage 4 adds `grim` and the real pinentry binary
- **THEN** the Stage 3 cage/wtype/VM-config scaffolding is reused without rewrite

### Requirement: Geometry assertion via log-grepping

The test binary MUST log surface configure serial, dimensions, scale, and emitted `Event`s to stderr. The Python driver MUST assert by grepping these logs (no external wlroots query tool required).

#### Scenario: driver asserts dimensions from log
- **WHEN** the test binary logs `configured: WxH scale=S`
- **THEN** the Python driver greps the log and asserts W, H, and S match expected values