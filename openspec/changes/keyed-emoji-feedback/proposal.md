## Why

Masked squares confirm only that input occurred; they do not let a user recognize whether the intended password was typed before Enter sends it. Public live hashes expose a recording-friendly prefix oracle, so nowayprompt needs user-keyed visual feedback whose derivation remains unavailable to passive observers and whose safer modes minimize intermediate-prefix exposure.

## What Changes

- Add deterministic emoji feedback derived from the exact password bytes with an explicit, high-entropy user secret and a public, canonical emoji table.
- Add a configurable minimum codepoint length and a configurable mask emoji (default `✳️`), repeated across every signature position until a real signature is revealed.
- Add manual-idle mode: show the fixed mask row while typing, reveal the emoji signature after a configured inactivity threshold, and return to the mask row after any secret mutation.
- Add auto-idle mode: derive a prompt-local inactivity threshold from ephemeral append cadence, bounded by fixed safety limits, without persisting typing biometrics.
- Add explicitly insecure live mode: reveal a fresh signature after every eligible secret mutation and document its prefix-transcript risk.
- Preserve Enter semantics in every mode: Enter submits immediately without calculating or requiring emoji feedback.
- Render fixed-length masked and deterministic signature feedback on Wayland with bundled default glyph coverage plus an explicit user-specified emoji-font file; keep TTY emoji feedback disabled by default with an opt-in text representation.
- Handle the derivation secret outside ordinary clonable configuration strings, avoid copying password bytes into ordinary heap allocations, and zeroize derived key material.
- Document that manual-idle and auto-idle reduce prefix sampling but can still reveal intermediate prefixes after pauses, while live mode exposes every eligible edit state.

## Capabilities

### New Capabilities
- `emoji-feedback`: User-keyed password-signature derivation, public emoji encoding, configurable fixed-length mask feedback, minimum-length gating, manual-idle, auto-idle, live feedback, state transitions, secret handling, rendering, and leakage contract.

### Modified Capabilities
- `config-parser`: Parse and validate emoji feedback mode, mask emoji, optional emoji-font path, minimum length, manual timeout, auto-idle policy, public table, TTY opt-in, and secure derivation-secret reference.
- `frontend-trait`: Extend the frozen frontend timing contract so the shared poll loops can query deadlines and dispatch inactivity expiry without changing Enter events.
- `wayland-input`: Update feedback state and deadlines after successful secret mutations while preserving existing input semantics.
- `wayland-render`: Select the repeated mask emoji or revealed signature from feedback state, provide deterministic bundled default glyph coverage, and prefer an explicitly configured emoji-font file without scanning system fonts.
- `tty-fallback`: Preserve legacy square feedback when TTY emoji is disabled and support explicitly enabled fixed-length mask/signature text feedback with the same state transitions.

## Impact

- Affected code: `src/config.rs`, `src/secret.rs`, `src/command.rs`, `src/frontend/mod.rs`, `src/frontend/tty.rs`, and `src/frontend/wayland/{input,mod,render}.rs`.
- Affected assets and dependencies: bundled default visual symbol coverage, optional explicit font-file loading, plus cryptographic KDF/HMAC and zeroization support.
- Affected documentation: `man/nowayprompt.conf.5`, user-facing security guidance, and configuration examples.
- Affected tests: config parsing, deterministic derivation vectors, minimum-length boundaries, state-machine transitions, poll timeout behavior, TTY rendering, and Wayland rendering/input scenarios.
- Existing configurations remain valid and emoji feedback remains disabled unless an explicit derivation secret and mode are configured.
