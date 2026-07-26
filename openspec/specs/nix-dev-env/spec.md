## Purpose

Defines the reproducible Nix development environment and Rust workspace baseline.

## Requirements

### Requirement: Nix flake development shell

The repository MUST contain a `flake.nix` providing `devShells.default` with a reproducible Rust development environment. The shell MUST include `rustc`, `cargo`, `rust-analyzer`, `clippy`, and `rustfmt` from nixpkgs (no overlay). The shell MUST include `pkg-config` and `libxkbcommon` development libraries (required by the Wayland frontend). The flake MUST declare `nixpkgs` as an input.

#### Scenario: Enter development shell
- **WHEN** a developer runs `nix develop` in the repository root
- **THEN** a shell is provided with `rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`, `pkg-config`, and `libxkbcommon` available on PATH and in include/link paths

#### Scenario: Reproducible toolchain
- **WHEN** the flake is evaluated on a different NixOS machine with the same nixpkgs revision
- **THEN** the same Rust toolchain version and native dependencies are provided

### Requirement: Cargo workspace initialization

The repository MUST contain a `Cargo.toml` at the workspace root declaring the `nowayprompt` binary crate with all dependencies locked in `Cargo.lock`; the locked set MUST include at least `libc`, `zeroize`, and `memmap2`. The edition MUST be 2021 or later.

#### Scenario: Cargo build resolves
- **WHEN** `cargo build` is run in the workspace root
- **THEN** dependencies resolve from `Cargo.lock` without network access (locked)

#### Scenario: Clippy runs clean
- **WHEN** `cargo clippy -- -D warnings` is run
- **THEN** no warnings are emitted for workspace code

### Requirement: No external C graphics dependencies in core modules

The core modules (`src/secret.rs`, `src/config.rs`) MUST NOT link against or depend on `libfcft`, `libpixman-1`, `libcairo`, or `libwayland-client`. Their only native dependency is `libc` (via the `libc` crate, statically linked where possible).

#### Scenario: Core module build without graphics libs
- **WHEN** `cargo build` is run in an environment without `libfcft`, `libpixman-1`, or `libwayland-client` installed
- **THEN** the core modules compile successfully; their only native dependency is `libc`
