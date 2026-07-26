## Why

The Rust Wayland frontend, renderer, input handling, Assuan REPL, and TTY
fallback exist, but `main.rs` always initializes `Tty`; the production binary
cannot select Wayland or expose the legacy CLI and SSH askpass interfaces.
The Nix package likewise installs only the base binary, so the rewrite has not
yet reached its user-facing replacement boundary.

## What Changes

- Add a production frontend selector that chooses Wayland first and falls back
to the configured TTY only for absent or unreachable Wayland displays when
fallback is permitted.
- Make the configured Wayland display name drive the actual client connection.
- Initialize a frontend only after the CLI or Assuan request has supplied the
configuration needed to do so.
- Add CLI prompt mode, native SSH askpass mode, and `argv[0]` dispatch for the
base, pinentry, and askpass executables.
- Package `nowayprompt`, `pinentry-nowayprompt`, and
`nowayprompt-ssh-askpass`, with Rust-owned manual pages.
- Add a deterministic headless differential NixOS test for the reachable
  Wayland path using a layer-shell-capable compositor and a persistent virtual
  keyboard harness, then register it with the existing parity test suite.

## Capabilities

### New Capabilities

- `command-entrypoints`: CLI, pinentry, and SSH askpass invocation contracts
  served by one Rust binary.
- `nix-package-interface`: Installed executable aliases and manual pages for
  the public package interface.

### Modified Capabilities

- `frontend-trait`: Select and initialize a concrete frontend at the point a
  prompt is requested, with constrained fallback behavior.
- `wayland-frontend`: Honor an explicit configured display when opening the
  Wayland connection.
- `wayland-parity-testing`: Prove the Wayland prompt path against the pinned
  legacy package under a headless compositor.
- `nixos-parity-testing`: Register the Wayland differential test in the
  NixOS test suite.

## Impact

- `src/main.rs`, `src/frontend/mod.rs`, and `src/frontend/wayland/mod.rs`
  gain production dispatch and selection responsibilities.
- New Rust modules may own CLI parsing, askpass behavior, and installed
  documentation generation or sources.
- `flake.nix` and `nixos-tests/default.nix` change; a new Wayland NixOS test
  and driver are added.
- No changes are made to the secret-memory allocation contract, layer-shell
  renderer semantics, XKB behavior, or configuration-file grammar.
