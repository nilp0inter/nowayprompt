## MODIFIED Requirements

### Requirement: Automated compositor test

The repository MUST provide a deterministic automated compositor gate for the
reachable layer-shell prompt. At output scale 1, the gate MUST compare the
target and pinned `pkgs.wayprompt` oracle for successful secret input,
cancellation, configured geometry, and observable configured surface behavior.
The gate MUST additionally exercise the target alone at a fractional output
scale and verify the effective scale, logical geometry, SHM buffer dimensions,
buffer scale, and viewport destination observable through the test binary and
Wayland protocol trace. It MUST verify a scale change while the surface remains
open. Deterministic tests MUST separately exercise the integer fallback when
fractional-scale or viewporter support is absent. The gate MUST use a compositor
implementing
`zwlr_layer_shell_v1`, retain the virtual keyboard through input delivery, and
wait on explicit client readiness rather than fixed sleeps. `cage` MUST NOT be
used because it lacks `zwlr_layer_shell_v1`; a one-shot `wtype` client under
headless Sway MUST NOT be the sole input mechanism because its device lifetime
is racy. Scaled output MUST NOT be compared against the legacy oracle, which
only renders a 1× buffer.

#### Scenario: deterministic 1x Wayland parity gate
- **WHEN** the registered Wayland parity derivation runs at output scale 1
- **THEN** target and oracle complete the same prompt scenarios without input-delivery races and retain frame-geometry parity

#### Scenario: integer scaling fallback test
- **WHEN** deterministic scale-state and render tests select a 2× output without fractional-scale support
- **THEN** they observe unchanged logical geometry, 2× physical dimensions, and integer buffer scale 2

#### Scenario: fractional scaling gate
- **WHEN** the target receives a preferred scale of 180 for a 1.5× output
- **THEN** the gate observes unchanged logical geometry, physical dimensions rounded up from logical dimensions times 1.5, `set_buffer_scale(1)`, and a viewport destination equal to logical geometry

#### Scenario: live scale change gate
- **WHEN** the compositor changes the active output scale while the target surface remains open
- **THEN** the gate observes a new correctly scaled buffer commit and the prompt remains interactive

#### Scenario: readiness is event-driven
- **WHEN** any compositor scenario injects keyboard input or captures render state
- **THEN** it first observes the required surface configure and buffer commit rather than waiting for a fixed delay
