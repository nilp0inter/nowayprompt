## Purpose

Defines the Stage 3 parity-testing surface: the test-only `nowayprompt-wayland-test` binary for driving the Wayland frontend directly, and records that the automated headless-compositor `nixosTest` gate is deferred to Stage 4 (design D10).

## Requirements

### Requirement: Test-only binary target

The `Cargo.toml` MUST define a second `[[bin]]` target `nowayprompt-wayland-test` (path `src/bin/wayland-test.rs`) that instantiates `Wayland::new()` + `init(cfg)` and drives the frontend directly, without going through `main.rs`. The primary `nowayprompt` pinentry binary MUST remain TTY-only (Stage 4 wires frontend selection). The binary MUST log surface configure dimensions, scale, hotspot geometry, and emitted `Event`s to stderr for manual/test verification.

#### Scenario: test binary exists and builds
- **WHEN** `cargo build --bin nowayprompt-wayland-test` is run
- **THEN** the binary builds successfully and is distinct from the main `nowayprompt` binary

#### Scenario: main.rs unchanged
- **WHEN** Stage 3 is complete
- **THEN** `src/main.rs` still hardcodes `Tty::new()` and does not reference `Wayland`

#### Scenario: binary logs geometry and events
- **WHEN** the binary runs under a compositor and enters `GetPin` mode
- **THEN** it logs `configured: WxH scale=S`, the hotspot geometry, and `event: UserOk`/`UserAbort`/`UserNotOk` on terminal input

### Requirement: Automated compositor test — deferred to Stage 4

The automated headless-compositor `nixosTest` (`nixosTests.stage-3-wayland`) is **deferred to Stage 4** (see design D10). A headless compositor test for a layer-shell client proved unreliable in the NixOS VM: `cage` does not implement `zwlr_layer_shell_v1`, and `sway` headless keyboard delivery to a layer-shell surface is racy (`wtype`'s one-shot virtual keyboard is dropped before delivery; `machine.send_key` does not reach layer-shell surfaces). Stage 3 render/geometry/keyboard parity is instead validated by a manual test against a real compositor, the unit tests, and the parity review. Stage 4 MUST add a deterministic compositor gate (a `grim`-based tolerance test or a persistent-virtual-keyboard harness).

#### Scenario: deferred gate
- **WHEN** Stage 3 is complete
- **THEN** no `nixosTests.stage-3-wayland` derivation is shipped; the deferral and its rationale are recorded in design D10, and the deterministic compositor gate is a Stage 4 deliverable
