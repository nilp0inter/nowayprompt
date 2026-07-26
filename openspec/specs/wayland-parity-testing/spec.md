## Purpose

Defines the Stage 3 parity-testing surface: the test-only `nowayprompt-wayland-test` binary for driving the Wayland frontend directly, and the deterministic automated compositor gate that compares the target against the pinned legacy oracle on a layer-shell compositor.

## Requirements

### Requirement: Test-only binary target

The `Cargo.toml` MUST define a second `[[bin]]` target `nowayprompt-wayland-test` (path `src/bin/wayland-test.rs`) that instantiates `Wayland::new()` + `init(cfg)` and drives the frontend directly, without routing through the public entrypoint multiplexer. The primary `nowayprompt` binary MUST select a production frontend through the frontend selector. The test binary MUST log surface configure dimensions, scale, hotspot geometry, and emitted `Event`s to stderr for test verification.

#### Scenario: test binary exists and builds
- **WHEN** `cargo build --bin nowayprompt-wayland-test` is run
- **THEN** the binary builds successfully and is distinct from the main `nowayprompt` binary

#### Scenario: main binary selects frontend
- **WHEN** the public `nowayprompt` or `pinentry-nowayprompt` binary starts a prompt
- **THEN** it selects a production frontend rather than constructing `Tty` unconditionally

#### Scenario: binary logs geometry and events
- **WHEN** the test binary runs under a compositor and enters `GetPin` mode
- **THEN** it logs `configured: WxH scale=S`, the hotspot geometry, and `event: UserOk`/`UserAbort`/`UserNotOk` on terminal input

### Requirement: Automated compositor test

The repository MUST provide a deterministic automated compositor gate for the reachable layer-shell prompt. The gate MUST use a compositor that implements `zwlr_layer_shell_v1`, retain the virtual keyboard through delivery of the input assertion, and wait on explicit client readiness rather than fixed sleeps. It MUST compare the target and pinned legacy oracle for successful secret input, cancellation, configured geometry, and observable configured surface behavior. `cage` MUST NOT be used because it lacks `zwlr_layer_shell_v1`; a one-shot `wtype` client under headless Sway MUST NOT be the sole input mechanism because its device lifetime is racy.

#### Scenario: deterministic Wayland gate
- **WHEN** the registered Wayland parity derivation runs
- **THEN** both target and oracle complete the same prompt scenarios without input-delivery races and the driver reports parity results
