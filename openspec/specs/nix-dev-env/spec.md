## Requirements

### Requirement: Nix flake development shell

The repository MUST contain a `flake.nix` providing `devShells.default` with a reproducible Rust development environment. The shell MUST include `rustc`, `cargo`, `rust-analyzer`, `clippy`, and `rustfmt` from nixpkgs (no overlay). The shell MUST include `pkg-config` and `libxkbcommon` development libraries (required by Stage 3, included now for environment completeness). The flake MUST declare `nixpkgs` as an input.

#### Scenario: Enter development shell
- **WHEN** a developer runs `nix develop` in the repository root
- **THEN** a shell is provided with `rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`, `pkg-config`, and `libxkbcommon` available on PATH and in include/link paths

#### Scenario: Reproducible toolchain
- **WHEN** the flake is evaluated on a different NixOS machine with the same nixpkgs revision
- **THEN** the same Rust toolchain version and native dependencies are provided

### Requirement: Cargo workspace initialization

The repository MUST contain a `Cargo.toml` at the workspace root declaring the `nowayprompt` binary crate and locked dependencies for Stage 1: `libc`, `zeroize`. `memmap2` MUST be declared (used by Stage 3 but locked now for reproducibility). The edition MUST be 2021 or later.

#### Scenario: Cargo build resolves
- **WHEN** `cargo build` is run in the workspace root
- **THEN** dependencies resolve from `Cargo.lock` without network access (locked)

#### Scenario: Clippy runs clean
- **WHEN** `cargo clippy -- -D warnings` is run
- **THEN** no warnings are emitted for Stage 1 code

### Requirement: No external C graphics dependencies in Stage 1

Stage 1 code (`src/secret.rs`, `src/config.rs`) MUST NOT link against or depend on `libfcft`, `libpixman-1`, `libcairo`, or `libwayland-client`. The only native dependency for Stage 1 is `libc` (via the `libc` crate, statically linked where possible).

#### Scenario: Stage 1 build without graphics libs
- **WHEN** `cargo build` is run in an environment without `libfcft`, `libpixman-1`, or `libwayland-client` installed
- **THEN** the build succeeds (only `libc` required)
