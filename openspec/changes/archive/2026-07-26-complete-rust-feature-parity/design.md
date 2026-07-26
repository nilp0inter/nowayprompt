## Context

The crate already contains a `Frontend` trait, concrete `Wayland` and `Tty`
frontends, Assuan parsing, and a Wayland-only test binary. `main.rs` bypasses
that architecture by constructing `Tty` directly before any Assuan setup
options are read. `Wayland::init` validates `Config.wayland_display`, but then
uses `Connection::connect_to_env()`, so an explicit display does not actually
select the connection target. The package only exposes `nowayprompt`.

The compatibility oracle is `pkgs.wayprompt` from the flake's pinned
nixos-26.05 input. Existing differential NixOS tests cover CLI/config, Assuan,
and TTY; the documented Wayland test is not registered or present.

## Goals / Non-Goals

**Goals:**

- Expose every legacy invocation class through one Rust executable selected by
  executable basename.
- Preserve the legacy CLI output and exit-status contract, Assuan protocol
  behavior, and SSH askpass stdout contract.
- Use an explicit display name for the actual Wayland connection.
- Select a frontend only when a prompt requires one, after protocol options or
  CLI arguments have populated `Config`.
- Fall back to TTY only for unavailable or unreachable Wayland displays and
  only when `allow_tty_fallback` is true.
- Produce a Nix package with all executables and manual pages.
- Demonstrate reachable Wayland parity in a registered headless NixOS test.

**Non-Goals:**

- Fractional-scale or high-DPI rendering, which the legacy oracle also lacks.
- X11 support, an async runtime, SCTK, winit, or calloop.
- Changing secret-memory allocation, Assuan command support, configuration
  syntax, rendering geometry, XKB semantics, or UI design.
- Extending the legacy CLI or pinentry contracts.

## Decisions

### Use a concrete selector, not trait-object allocation

Add a frontend owner represented by an enum over `Wayland` and `Tty` that
implements the existing `Frontend` trait by delegation. It attempts Wayland
first and contains the exact fallback classification.

This keeps the poll-based contract and concrete ownership already used by the
crate. A `Box<dyn Frontend>` would add unnecessary allocation and obscure
ownership of the secret-buffer pointer. Duplicating the dispatch loop per
frontend would risk protocol divergence.

### Initialize at the first prompt request

CLI mode parses all arguments before selecting a frontend. Pinentry maintains
its Assuan setup-only state without a frontend and creates one immediately
before the first `GETPIN`, `CONFIRM`, or non-empty `MESSAGE` flow.

This makes `OPTION putenv=WAYLAND_DISPLAY=` and `OPTION ttyname=` effective.
It also avoids opening a display or TTY for a session that only exchanges
setup, query, reset, or goodbye commands. Eager initialization cannot meet
that contract because relevant Assuan options arrive after greeting.

### Restrict fallback to transport absence

A missing display name or connection failure may select `Tty` only when the
configuration permits it. Missing globals, dispatch failure, rendering error,
unsupported keymap, and other post-connection failures propagate as errors.

Treating every Wayland error as a fallback condition would hide compositor or
security failures and may unexpectedly redirect graphical authentication to a
TTY.

### Treat the configured display as connection input

Use the `wayland-client` connection API that accepts the selected display name,
or temporarily and locally apply the selected name only around connection
creation if that API is unavailable. The selected `Config` value has precedence
over `WAYLAND_DISPLAY`; the environment is used only when configuration leaves
it unset.

The current presence check plus `connect_to_env()` is incorrect because it can
connect to a different socket than the caller requested.

### Native askpass behavior

The askpass basename invokes the shared CLI prompt path with a fixed password
prompt configuration: the argument string supplies the title when non-empty,
otherwise the legacy default title is used; it writes only the accepted secret
and a newline to stdout. Cancel, not-ok, empty password, and frontend failure
return nonzero without a secret.

A shell wrapper that parses human-readable CLI output is rejected: it creates a
second parser boundary around sensitive output and does not need to exist once
the executable already owns the secret buffer.

### Package aliases and documentation are build outputs

Nix installs one compiled binary, creates the pinentry and askpass aliases in
`$out/bin`, and installs Rust-owned manpages for the CLI, pinentry, askpass,
and configuration format. The aliases preserve `argv[0]` so selection occurs
inside the binary.

Separate binaries would duplicate build artifacts without different code or
security boundaries.

### Use a deterministic layer-shell-capable NixOS test harness

Add and register a Wayland NixOS test that runs target and oracle under a
compositor that implements `zwlr_layer_shell_v1`, uses a persistent virtual
keyboard client, and captures frame evidence with `grim`. The driver waits for
explicit readiness output and asserts behavior and the Wayland-specific
invariants observable through the existing test binary.

`cage` is rejected because it does not implement `zwlr_layer_shell_v1`.
One-shot `wtype` under headless Sway is also rejected: its keyboard disappears
before reliable layer-shell input delivery. Unit tests cannot establish
layer-shell configure ordering, real input routing, or frame geometry. The
existing target-only `nowayprompt-wayland-test` remains test infrastructure,
not a public package entrypoint.

## Risks / Trade-offs

- Delayed frontend initialization changes when environmental errors surface;
  errors now occur at the first prompt command rather than process greeting.
  This is necessary for Assuan-provided configuration and must be tested.
- A Wayland client API may not expose a named connection directly. Any
  environment-based bridge must restore process state and must never leak the
  override across threads; a direct API is preferred.
- The compositor and persistent virtual-keyboard harness increase the NixOS
  test closure and can expose timing sensitivity. The driver must wait on
  explicit readiness output rather than fixed sleeps.
- CLI and askpass output include secrets. Tests must assert behavior without
  logging literal test secrets or retaining process output beyond the
  assertion.
- Package aliases are an external compatibility boundary. Their exact names,
  modes, and manpage paths must be asserted in the package test.
