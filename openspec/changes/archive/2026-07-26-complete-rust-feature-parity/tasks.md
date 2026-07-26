## 1. Frontend selection and display connection

- [x] 1.1 Add the enum-based frontend owner that delegates `Frontend` to
  `Wayland` or `Tty` and classifies fallback errors exactly as specified.
- [x] 1.2 Make `Wayland::init` connect to `Config.wayland_display` when set,
  otherwise to `WAYLAND_DISPLAY`; add focused coverage for the precedence and
  no-display paths.
- [x] 1.3 Refactor the pinentry dispatch lifecycle so frontend initialization
  is deferred until a prompt command, preserving Assuan setup-only handling and
  ensuring `deinit` runs once for an initialized frontend.
- [x] 1.4 Add integration coverage for Assuan `OPTION ttyname` and
  `OPTION putenv=WAYLAND_DISPLAY` before a prompt, TTY fallback eligibility,
  and non-fallback on a post-connection Wayland error.

## 2. Public entrypoint contracts

- [x] 2.1 Add basename dispatch for `nowayprompt`, `pinentry-nowayprompt`, and
  `nowayprompt-ssh-askpass`; reject unknown basenames before frontend setup.
- [x] 2.2 Implement CLI option parsing, request validation, prompt execution,
  plain output, JSON output, and legacy exit-status mapping.
- [x] 2.3 Implement native SSH askpass prompt configuration and secret-only
  stdout behavior without passing secret output through a text parser.
- [x] 2.4 Add deterministic tests for basename dispatch, CLI validation and
  output/exit mappings, and askpass success, empty-secret, and cancellation
  behavior without logging secret fixture values.

## 3. Package interface

- [x] 3.1 Update the Nix package derivation to install the base binary and
  basename-preserving pinentry and askpass aliases.
- [x] 3.2 Add Rust-owned manual pages for the three executables and the shared
  configuration format, then install them through the Nix derivation.
- [x] 3.3 Add package-level assertions for executable aliases, main program,
  and installed manual-page paths.

## 4. Deterministic Wayland parity gate

- [x] 4.1 Select and validate a layer-shell-capable headless compositor and a
  persistent virtual-keyboard harness that keeps input alive through delivery;
  document the selected mechanisms and reject Cage and one-shot wtype as the
  sole input path.
- [x] 4.2 Implement the Wayland parity driver using explicit readiness signals
  and frame capture; exercise accepted secret input, cancellation, configured
  geometry, and observable surface behavior for target and oracle.
- [x] 4.3 Add and register the Wayland NixOS test derivation with both the
  pinned oracle and target installed, including explicit comparison tolerances.
- [x] 4.4 Run the focused Rust tests, package build checks, and all four NixOS
  parity derivations; resolve any behavioral divergence from the pinned oracle.
