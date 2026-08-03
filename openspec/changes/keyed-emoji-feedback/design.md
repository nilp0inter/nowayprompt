## Context

nowayprompt currently renders square feedback after each successful secret mutation and blocks indefinitely in two `poll(2)` loops until frontend or Assuan activity occurs. The frontend implementations mutate a locked `SecretBuffer` directly, while the shared `Frontend` trait has no deadline or timeout callback. Wayland bundles only DejaVu faces and does not scan system fonts; TTY rendering depends on terminal glyph support.

The feature protects against a passive person, camera, or screenshot recorder that can observe feedback but does not possess the user-supplied derivation secret. It also minimizes the value of retained recordings if that secret is compromised later. It does not protect against a same-UID process that can read the secret file or invoke the keyed derivation as an oracle.

The user deliberately transports one high-entropy derivation secret to machines that should produce identical signatures. The emoji table is public. Enter, pointer OK, and touch OK retain their current behavior and submit immediately; they never trigger a new emoji calculation.

## Goals / Non-Goals

**Goals:**

- Produce the same ordered visual signature for identical password bytes, derivation secret, table, count, and derivation version on every machine.
- Render a configurable fixed-length mask row below the minimum and while idle feedback is armed.
- Support manual-idle, auto-idle, and explicitly insecure live modes.
- Revert an idle-mode reveal to the mask row on every successful secret mutation.
- Integrate monotonic inactivity deadlines into the existing single-threaded `poll(2)` architecture without an async runtime or timer thread.
- Keep the derivation secret and password bytes out of clonable configuration strings and ordinary heap copies.
- Provide deterministic bundled Wayland glyph coverage, an explicit custom emoji-font file override, and an opt-in TTY text representation.
- State the leakage and compromise boundary of each mode precisely.

**Non-Goals:**

- Proving that the entered password is accepted by GPG, SSH, LUKS, or another backend.
- Zero-knowledge proof or PAKE semantics.
- Protecting against a process with access to the derivation secret or an unrestricted local derivation oracle.
- Synchronizing, backing up, rotating, or recovering the user-supplied secret.
- Changing Enter/OK submission semantics or requiring a verification step.
- Persistent typing profiles, biometric identification, animation, or network services.
- Hiding keystroke timing; the fixed-length mask row hides candidate length only in rendered feedback.

## Decisions

### Use a high-entropy seed file as the explicit security root

Configuration stores only `secret-file`, a path to a user-managed file containing exactly 64 hexadecimal digits encoding 32 random bytes, with at most one trailing LF. Emoji feedback is disabled by default. Selecting a non-off mode without a valid secret file is a hard configuration/init error rather than a silent downgrade.

The file is opened with `O_RDONLY | O_CLOEXEC | O_NOFOLLOW`; it must be a regular file, owned by the effective UID, with no group or other permission bits. The decoded seed is placed in locked, non-dumpable memory and temporary encoded/decoded buffers are zeroized.

A literal config value, command-line argument, and environment variable were rejected because the current parser stores and clones strings, process arguments are observable, and environment values are inherited and exposed through process inspection. Deriving from the protected password was rejected because it recreates a public offline password verifier. Public-key wrapping was rejected because it only relocates transport to another secret and is circular when that key is what the prompt unlocks.

### Key the password derivation; keep tables public

Derive a feedback subkey with HKDF-SHA-256 from the 32-byte seed using the fixed domain `nowayprompt/emoji-feedback/key/v1`. For output position `i`, compute HMAC-SHA-256 over:

```text
"nowayprompt/emoji-feedback/value/v1" || u32be(i) || password_bytes
```

Map digest words to a table index with rejection sampling, extending with a domain-separated block counter only if all words are rejected. This avoids modulo bias and makes positions independent. The password input is the exact `SecretBuffer` byte slice; no Unicode normalization or heap `String` is introduced.

A secret table permutation alone was rejected because it preserves equality structure and can be recovered with chosen inputs. A public ordered table is simpler to reproduce and the security argument remains valid if all table contents and algorithms are known.

### Version and validate the public table

The project provides a canonical ordered default table. Users may replace it with repeated public table entries. Entries must be non-empty and byte-distinct, the table must contain at least two entries, and `count` must be positive and bounded. Identical signatures across machines require the same seed, derivation version, table order, and count.

Masked states use one separate public `mask-emoji` value repeated exactly `count` times, independent of the current candidate length. The default is `✳️` (U+2733 U+FE0F, EIGHT SPOKED ASTERISK with emoji presentation). It is not derived from the password and need not be a member of the signature table. Legacy squares remain only when emoji feedback is globally off or TTY emoji rendering is not enabled.

The derivation includes no machine, account, protocol, or mutable label context. This intentionally gives the same password the same pattern across machines and prompt consumers. Domain separation prevents reuse outside this feature without multiplying the patterns a user must remember.

### Use one shared feedback state machine

A frontend-local `FeedbackState` consumes successful secret mutations and owns the display phase, monotonic deadline, derived indices, and prompt-local auto-idle samples. Both frontends use the same state-machine logic; rendering only reads its output.

States are:

```text
BelowMinimum  --eligible mutation-->  Armed
Armed         --deadline-->           Revealed
Revealed      --mutation-->           BelowMinimum or Armed
Any           --clear/leave mode-->   reset
```

Eligibility is `SecretBuffer::len() >= minimum_length`, using the existing Unicode codepoint count. HMAC always consumes exact raw bytes. Empty and below-minimum candidates never arm or reveal.

In manual-idle mode, every eligible successful mutation shows `count` copies of the configured mask emoji and sets `deadline = now + configured_timeout`. Deadline expiry calculates and reveals the current signature. A subsequent mutation clears the signature before rearming and restores the fixed-length mask row.

In live mode, every eligible successful mutation calculates and reveals immediately; below-minimum input shows the fixed-length mask row. This mode is named and documented as insecure because a recording contains every eligible edit state.

Enter, keyboard/pointer/touch OK, Escape, and not-OK remain terminal events. Enter/OK returns `UserOk` immediately and does not calculate a signature when none is already displayed.

### Derive auto-idle from ephemeral append cadence

Auto-idle records monotonic intervals between append activity batches. Deletes and clears reset the deadline but do not train the estimator; a full clear discards all samples. A TTY read containing several codepoints is one activity batch so paste does not create zero-duration samples. Intervals that cross a reveal are censored and are not fed back.

With at least three usable intervals, the candidate timeout is:

```text
clamp(5 * median(intervals), 1000 ms, 5000 ms)
```

With fewer samples, the timeout is 1500 ms. At most the latest 32 intervals are held in a fixed-size prompt-local ring. The estimate updates after each append while the mask row remains visible and freezes at the first reveal. Subsequent corrections reuse the frozen timeout until the buffer is fully cleared or the prompt ends.

A mean, maximum, and persistent EWMA profile were rejected because outliers dominate them or they create durable typing-biometric state. The constants are intentionally conservative initial policy and are a primary review point for usability feedback.

### Extend the frontend deadline contract

Extend the public `Frontend` trait with:

```text
next_deadline(&self) -> Option<Instant>
handle_timeout(&mut self) -> Result<(), FrontendError>
```

Both shared poll loops calculate a finite timeout from `next_deadline`; otherwise they preserve the current infinite wait. `handle_timeout` is called only when `poll` returns zero and the monotonic deadline is due. Readable frontend input takes precedence over an apparent simultaneous expiry, preventing a stale-prefix reveal before queued input is processed.

The existing `no_event` hook is not reused as a timeout callback because it is also called when another descriptor was readable. A timerfd, background thread, and async runtime were rejected as unnecessary for a single monotonic deadline in the existing poll loop.

This is a source-breaking extension of the public Rust trait, but nowayprompt is pre-1.0 and both in-tree implementations migrate atomically.

### Render cached table glyphs on Wayland

Bundle a color emoji font with a license compatible with redistribution and load it alongside the existing DejaVu faces without system font scanning. An optional public `emoji-font` path may name one user-managed font file. When configured, initialization loads that exact file, resolves its face family, and places it before the bundled emoji face in the fallback chain. Failure to open or parse an explicitly configured file is an initialization error; no system-font scan or silent ignore occurs.

Shape and rasterize the configured mask emoji and each configured table entry into cached bitmaps during frontend initialization; masked states blit the mask bitmap `count` times and revealed signatures blit selected table entries at configured size. The configured face supplies covered glyphs and the bundled face covers missing glyphs. This avoids shaping and allocation on every live-mode keypress while allowing deliberate user styling.

The feedback area always reserves one `count`-position emoji row, so switching from mask entries to a revealed signature cannot resize the layer surface. Legacy squares are rendered only when emoji feedback is disabled.

Missing glyphs after both the configured and bundled faces are tried are initialization errors rather than tofu or silent substitution. Color and grayscale swash images use the existing blending path. Signature indices remain deterministic across machines; identical artwork additionally requires the same font file/version.

Unrestricted system-font fallback was rejected because it makes coverage and selection implicit and machine-dependent. An exact configured file retains explicit behavior. Rendering arbitrary image packs was rejected as additional format, decoding, and asset-management scope.

### Keep TTY feedback opt-in

TTY continues to render legacy squares unless explicitly enabled. When enabled, it repeats the configured mask emoji `count` times in masked states and uses its public repeated `emoticon` table for revealed indices; both are emitted as UTF-8 text, and terminal font coverage and display width remain the user's responsibility. TTY mode does not attempt font discovery or graphical sizing.

The TTY table must satisfy the same non-empty/distinct/minimum-size validation. If TTY feedback is enabled without a valid TTY table, initialization fails. Disabling TTY feedback does not disable Wayland feedback and retains the legacy square row.

### Treat derived material as sensitive

The seed and HKDF/HMAC key remain in locked, non-dumpable memory for the prompt process lifetime and are zeroized before unmapping. HMAC state and digest blocks are stack/fixed-buffer values and are zeroized immediately after indices are selected. The password is read directly from `SecretBuffer::slice()` and never copied into an ordinary `Vec` or `String`.

Displayed indices are retained only while `Revealed`; mutation, mode exit, reset, cancellation, and submission clear them. Timing samples are never logged, persisted, serialized, or shared between prompt sessions.

### Document modes as leakage policies

Manual-idle and auto-idle reveal every eligible prefix at which inactivity crosses the threshold. A comfortable threshold and minimum length reduce sampling but do not prove that input is complete. Live mode reveals every eligible mutation. Anyone who later obtains both the seed and a recording can test candidate prefixes, so idle modes reduce retrospective evidence rather than providing forward secrecy.

## Risks / Trade-offs

- [User pauses mid-password and reveals a prefix] → Preserve the minimum-length gate, default to idle rather than live, expose a comfortable manual timeout, and document sampled-prefix leakage.
- [Auto-idle surprises fast typists at word boundaries] → Apply a one-second floor, five-times-median multiplier, and five-second ceiling; solicit review of constants before implementation.
- [Seed compromise turns recordings into candidate verifiers] → Keep seed provisioning explicit, validate file ownership/mode, lock and zeroize memory, and state that no same-UID protection is claimed.
- [Seed or table differs across machines] → Specify exact seed format, fixed derivation domains, table ordering, count, and test vectors.
- [Seed rotation changes every remembered signature] → Treat rotation as an explicit user migration; never regenerate silently.
- [Bundled color font increases package size] → Bundle one deterministic default font and retain no-system-scan startup behavior.
- [Configured font is missing, invalid, or changes artwork] → Fail on explicit load errors, retain bundled glyph fallback, and document that font choice affects appearance but not derived indices.
- [Mask or custom table entry is uncovered by both fonts] → Fail initialization with the offending mask/table index instead of rendering ambiguous fallback.
- [TTY character widths differ] → Keep TTY disabled by default and make opt-in rendering explicitly best-effort.
- [Finite poll timeout changes dispatch ordering] → Process readable input before timeout expiry and use monotonic absolute deadlines.
- [Frontend trait extension breaks downstream implementors] → Make a clean pre-1.0 cutover with no compatibility shim.
- [Auto-idle timing becomes biometric data] → Use fixed-size ephemeral samples only and clear them at every terminal transition.

## Migration Plan

1. Add the new configuration and derivation behavior disabled by default; existing config files retain square-only behavior.
2. Add the seed-file format and permission validation, then deterministic derivation vectors before wiring rendering.
3. Migrate both frontend implementations and both shared poll loops atomically to the extended deadline contract.
4. Add bundled Wayland font assets, explicit custom-font loading/caching, and opt-in TTY representation.
5. Update the manual and security guidance with mode leakage and cross-machine reproducibility requirements.

Rollback is code-only while the feature remains disabled. Users who enable it must remove the new configuration section before running an older nowayprompt version because the strict parser rejects unknown sections and variables. Seed files remain user-owned and are never removed automatically.

## Open Questions

- Are `5 × median`, a 1000 ms floor, a 5000 ms ceiling, and a 1500 ms fallback comfortable across representative typing styles?
- Which redistributable color emoji font and canonical default table provide the best package-size/visual-distinctness trade-off?
- Should the TTY opt-in use the canonical emoji table when no emoticon table is supplied, or require an explicit terminal table as designed above?
