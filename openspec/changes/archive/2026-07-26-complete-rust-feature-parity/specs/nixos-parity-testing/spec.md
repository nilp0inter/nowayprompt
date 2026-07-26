## ADDED Requirements

### Requirement: Registered Wayland differential test

The `nixosTests` flake output MUST expose and register a Wayland differential
test alongside `stage-1-cli-config`, `stage-2-assuan`, and `stage-3-tty`. The
test MUST install both the pinned `pkgs.wayprompt` oracle and the Rust target,
exercise the deterministic layer-shell compositor gate, and fail when their
observable prompt result or configured geometry differs outside explicitly
documented tolerances.

#### Scenario: flake exposes the Wayland gate
- **WHEN** the `nixosTests` attribute set is evaluated
- **THEN** it contains a derivation for the Wayland differential test

#### Scenario: test installs both implementations
- **WHEN** the Wayland differential test boots its VM or equivalent test
  environment
- **THEN** the pinned oracle and target package are both available to the
  driver
