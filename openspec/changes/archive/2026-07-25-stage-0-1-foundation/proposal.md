## Why

The Zig `wayprompt` implementation depends on C libraries (`libfcft.so`, `libpixman-1.so`, `libwayland-client.so`) and a build system (`build.zig`) that complicates reproducible packaging and cross-distro distribution. The Rust rewrite eliminates all external C graphics dependencies, replaces them with pure-Rust crates, and establishes the hardened security foundation (locked, non-dumpable, zeroized secret memory) and INI configuration engine required by all downstream stages. Stage 0 + Stage 1 deliver the Nix flake environment and the two foundational modules (`src/secret.rs`, `src/config.rs`) that every later stage builds upon.

## What Changes

- **NEW**: `flake.nix` with `devShells.default` declaring the Rust toolchain (`rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`), `pkg-config`, `libxkbcommon` development libraries, and `nixpkgs` inputs.
- **NEW**: `Cargo.toml` workspace initialization with locked dependencies (`libc`, `zeroize`, `memmap2`).
- **NEW**: `src/secret.rs` — direct `mmap(2)` OS page allocator (`MAP_PRIVATE | MAP_ANONYMOUS`), `libc::mlock` page locking, `libc::MADV_DONTDUMP` coredump protection, `libc::MADV_WIPEONFORK` fork protection, `RLIMIT_CORE = 0` enforcement, and `zeroize::Zeroize` on `Drop`.
- **NEW**: `src/config.rs` — custom streaming `std::io::BufRead` line parser for `wayprompt.5` INI configurations, trailing semicolon stripping, hyphen-to-underscore field normalization, and hex `0xRRGGBB`/`0xRRGGBBAA` to premultiplied alpha color conversion.

## Capabilities

### New Capabilities

- `secret-memory`: OS-level locked, non-dumpable, zeroized secret byte buffer with 100% behavioral parity to `legacy/src/SecretBuffer.zig`.
- `config-parser`: `wayprompt.5` INI configuration parser with section dispatch, trailing semicolon handling, and color conversion.
- `nix-dev-env`: Nix flake development shell providing the Rust toolchain and native build dependencies required by all stages.

### Modified Capabilities

<!-- None. This is a greenfield Rust rewrite; no existing Rust specs are modified. -->