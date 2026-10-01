## 1. Dependencies and configuration

- [ ] 1.1 Add the HKDF/HMAC/SHA-256 dependencies and bundle the selected redistributable color emoji font plus its license
- [ ] 1.2 Add `[emoji]` configuration types and parsing for mode, secret/font paths, count, mask, minimum, timeout, size, and repeated table entries
- [ ] 1.3 Add `[tty]` configuration types and parsing for the opt-in flag and repeated emoticon entries
- [ ] 1.4 Implement final cross-field, range, table-uniqueness, and enabled-mode validation with parser coverage

## 2. Keyed derivation and secret handling

- [ ] 2.1 Generalize locked non-dumpable storage for the fixed-size derivation seed/key without weakening `SecretBuffer`
- [ ] 2.2 Implement owner/mode/type-checked no-follow loading and strict hexadecimal decoding of the 32-byte seed file
- [ ] 2.3 Implement domain-separated HKDF-SHA-256/HMAC-SHA-256 derivation and unbiased table-index selection directly over `SecretBuffer` bytes
- [ ] 2.4 Add deterministic cross-machine vectors, rejection-sampling boundaries, malformed seed cases, and key/digest zeroization coverage

## 3. Feedback controller

- [ ] 3.1 Implement the shared fixed-length mask, minimum gate, idle, live, mutation, reset, and terminal-transition state machine
- [ ] 3.2 Implement the fixed-size prompt-local auto-idle sample ring, median policy, bounds, fallback, freeze, and clear semantics
- [ ] 3.3 Cover fixed mask count independence, minimum boundaries, reveal/edit transitions, live updates, and no-hash submission behavior
- [ ] 3.4 Cover auto-idle sample sufficiency, floor/ceiling, censored intervals, correction reuse, and full-clear disposal

## 4. Frontend and poll deadlines

- [ ] 4.1 Extend `Frontend` with `next_deadline` and `handle_timeout`, then migrate both in-tree implementations without a compatibility shim
- [ ] 4.2 Compute monotonic poll timeouts in CLI/askpass and Assuan loops while preserving infinite waits when no deadline exists
- [ ] 4.3 Enforce readable-input precedence over coincident expiry and distinguish timeout dispatch from the existing `no_event` hook
- [ ] 4.4 Exercise finite timeout conversion, no-deadline blocking, due/early callbacks, simultaneous readiness, and mode-boundary cleanup

## 5. Wayland integration

- [ ] 5.1 Load the bundled emoji face and optional exact `emoji-font` file with custom-first fallback and no system font scan
- [ ] 5.2 Validate and cache configured mask/table glyph bitmaps, including color swash blending and explicit missing-glyph/load errors
- [ ] 5.3 Render exactly `count` cached mask entries or revealed indices in stable fractionally scaled feedback geometry
- [ ] 5.4 Route successful keyboard mutations through the controller while preserving no-op, Return, Escape, pointer, and touch semantics
- [ ] 5.5 Wire Wayland deadline exposure/expiry to immediate redraw without recalculating signatures on configure or redraw events
- [ ] 5.6 Exercise bundled/custom font selection, fixed mask rendering, reveal replacement, stable geometry, and input-driven state transitions

## 6. TTY integration

- [ ] 6.1 Initialize opt-in TTY mask/emoticon feedback while preserving the legacy square row when TTY emoji is disabled
- [ ] 6.2 Render fixed mask and revealed rows with exact UTF-8 entries and route successful parser mutations through the controller
- [ ] 6.3 Treat each TTY read as one append-activity batch and implement deadline exposure/expiry with immediate ANSI redraw
- [ ] 6.4 Exercise legacy rendering, three-position default/custom masks, revealed emoticons, paste cadence, expiry, and Enter bypass

## 7. Documentation and end-to-end verification

- [ ] 7.1 Document every `[emoji]`/`[tty]` field, seed-file format/permissions, custom font behavior, defaults, and reproducibility inputs in `nowayprompt.conf.5`
- [ ] 7.2 Document fixed-mask length behavior, sampled-prefix idle leakage, every-edit live leakage, retrospective seed compromise, and non-verification limits
- [ ] 7.3 Run focused Rust tests and the complete existing test suite after all frontend/config/crypto changes are integrated
- [ ] 7.4 Smoke-test real TTY idle reveal/submission and Wayland bundled/custom-font mask-to-signature rendering without submission delay
