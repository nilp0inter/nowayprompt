## ADDED Requirements

### Requirement: Explicit high-entropy derivation secret
Emoji feedback MUST remain disabled by default. Every enabled mode MUST load a user-provided seed from `secret-file`; the file MUST contain exactly 64 hexadecimal digits encoding 32 bytes and MAY end with one LF. The file MUST be opened without following symlinks, MUST be a regular file owned by the effective UID, and MUST have no group or other permission bits. Missing, malformed, insecurely permissioned, or unreadable seed files MUST fail initialization without falling back to unkeyed feedback.

#### Scenario: Valid seed file enables derivation
- **WHEN** emoji mode is enabled and `secret-file` names an owner-only regular file containing 64 hexadecimal digits
- **THEN** nowayprompt decodes a 32-byte seed into locked, non-dumpable memory

#### Scenario: Missing seed cannot downgrade security
- **WHEN** emoji mode is enabled but `secret-file` is absent or invalid
- **THEN** prompt initialization fails and no emoji or unkeyed password hash is shown

### Requirement: Deterministic keyed signature derivation
The system MUST derive a feedback key from the 32-byte seed with HKDF-SHA-256 and the domain `nowayprompt/emoji-feedback/key/v1`. For each output position it MUST compute HMAC-SHA-256 over the exact sequence `"nowayprompt/emoji-feedback/value/v1" || u32be(position) || password_bytes`. It MUST map digest words to the public table with unbiased rejection sampling. It MUST NOT normalize, re-encode, or copy the password into an ordinary heap `String` or `Vec`.

#### Scenario: Cross-machine deterministic vector
- **WHEN** two machines use identical seed bytes, raw password bytes, derivation version, ordered table, and count
- **THEN** they produce the same ordered table indices

#### Scenario: Byte-distinct passwords differ
- **WHEN** two visually similar passwords have different UTF-8 byte sequences
- **THEN** each exact byte sequence is independently keyed and no Unicode normalization makes them equivalent

### Requirement: Public table validation and encoding
The Wayland emoji table MUST be public and ordered. A custom table MUST use repeated `table-entry` values; entries MUST be non-empty and byte-distinct and the final table MUST contain at least two entries. If no custom entries are present, the canonical built-in ordered table MUST be used. `count` MUST be positive and bounded by the implementation limit. Table validation failures MUST prevent prompt initialization.

#### Scenario: Canonical table default
- **WHEN** emoji feedback is enabled without custom `table-entry` values
- **THEN** the canonical built-in table is used

#### Scenario: Duplicate entry rejected
- **WHEN** two configured table entries contain identical UTF-8 bytes
- **THEN** configuration validation fails before a password prompt is displayed

### Requirement: Fixed-length mask feedback
Whenever emoji feedback is enabled for a frontend and no keyed signature is revealed, the feedback row MUST contain exactly `count` copies of the configured `mask-emoji`, independent of whether the secret is empty, below the minimum, or armed. The default mask MUST be `✳️` (U+2733 U+FE0F). Rendering the mask MUST NOT read or derive from password bytes. Legacy length-dependent squares MUST remain only when emoji feedback is globally off or TTY emoji rendering is explicitly disabled.

#### Scenario: Three-position default mask
- **WHEN** `count` is 3, the default mask is selected, and no signature is revealed
- **THEN** the feedback row contains `✳️ ✳️ ✳️`

#### Scenario: Mask does not expose candidate length
- **WHEN** the candidate changes from one to seven codepoints while feedback remains masked
- **THEN** the same `count` mask positions remain visible

#### Scenario: Custom mask fills every position
- **WHEN** `count` is 4 and `mask-emoji` is `🔒`
- **THEN** the masked row contains four `🔒` entries

### Requirement: Minimum codepoint length gate
The feedback controller MUST use `SecretBuffer::len()` as the input length and MUST show the fixed-length mask row while the codepoint count is below `minimum-length`. It MUST never arm an idle deadline or calculate an emoji signature below that minimum. Reaching the minimum is eligible; deleting below it MUST clear any signature, disarm the deadline, and restore the fixed-length mask.

#### Scenario: Candidate below minimum stays masked
- **WHEN** `minimum-length` is 8, `count` is 3, and the candidate contains 7 codepoints
- **THEN** three configured mask emojis are shown and no HMAC calculation or idle deadline occurs

#### Scenario: Candidate reaches minimum
- **WHEN** the eighth codepoint is appended with `minimum-length = 8`
- **THEN** the configured mode applies from that successful mutation

### Requirement: Manual-idle feedback mode
In `idle` mode, every eligible successful secret mutation MUST show the fixed-length mask row and set a monotonic deadline equal to the mutation time plus `idle-timeout-ms`. Expiry with no queued input MUST calculate and reveal the current keyed signature. Any later successful append, delete, or clear MUST remove the revealed signature before rendering the mask row and MUST rearm only when the resulting candidate remains eligible.

#### Scenario: Idle expiry reveals current candidate
- **WHEN** an eligible candidate receives no input through its configured idle deadline
- **THEN** the fixed-length mask row is replaced by the keyed emoji signature of the current candidate

#### Scenario: Edit after reveal returns to mask
- **WHEN** a codepoint is appended or deleted after an idle signature is visible
- **THEN** the signature is cleared immediately and the fixed-length mask row is rendered with a fresh eligible deadline

### Requirement: Prompt-local auto-idle feedback mode
In `auto-idle` mode, the controller MUST record at most the latest 32 monotonic intervals between append activity batches while the mask row is visible. Deletes MUST reset the deadline without training the estimator; a full clear MUST discard all samples. With at least three usable intervals, the timeout MUST be `clamp(5 × median(intervals), 1000 ms, 5000 ms)`; otherwise it MUST be 1500 ms. The estimate MUST update while the mask row remains visible, freeze at the first reveal, and remain frozen through corrections until a full clear or prompt termination. Samples MUST never be persisted or logged.

#### Scenario: Median-based automatic threshold
- **WHEN** append intervals are 200 ms, 220 ms, and 240 ms
- **THEN** the automatic timeout is 1100 ms, five times the 220 ms median and within the fixed bounds

#### Scenario: Automatic floor
- **WHEN** at least three intervals have a median of 100 ms
- **THEN** the automatic timeout is clamped to 1000 ms

#### Scenario: Insufficient samples use fallback
- **WHEN** fewer than three usable append intervals exist
- **THEN** the automatic timeout is 1500 ms

#### Scenario: Clear discards biometric state
- **WHEN** the candidate is fully cleared
- **THEN** all interval samples and the frozen automatic threshold are discarded

### Requirement: Live feedback mode
In `live` mode, every successful eligible append, delete, or clear-result mutation MUST calculate and reveal the keyed signature immediately. A resulting length below the minimum MUST show the fixed-length mask row and MUST NOT calculate a signature. Documentation and configuration help MUST label live mode insecure because it records every eligible edit state.

#### Scenario: Eligible live mutation reveals immediately
- **WHEN** live mode is active and an append leaves the candidate at or above the minimum length
- **THEN** the newly keyed signature is rendered without waiting for inactivity

#### Scenario: Live delete crosses below minimum
- **WHEN** live mode is active and deletion leaves the candidate below the minimum
- **THEN** the fixed-length mask row replaces the signature and no new signature is calculated

### Requirement: Submission never triggers emoji hashing
Enter, pointer OK, and touch OK MUST preserve the existing immediate `UserOk` behavior in all modes. A submission action MUST NOT calculate, refresh, or wait for an emoji signature. If a signature is already visible, submission MUST send the existing secret without recalculating it.

#### Scenario: Enter before idle expiry
- **WHEN** the user presses Enter while idle or auto-idle mode is showing the mask row
- **THEN** `UserOk` is returned immediately and no HMAC calculation occurs

#### Scenario: Enter after reveal
- **WHEN** the user presses Enter while a signature is visible
- **THEN** `UserOk` is returned immediately without recalculating the signature

### Requirement: Deadline dispatch prioritizes input
The shared CLI/askpass and Assuan poll loops MUST use the frontend's next monotonic deadline as their finite `poll(2)` timeout and MUST preserve infinite polling when no deadline exists. A zero poll result with a due deadline MUST dispatch timeout handling. Readable frontend input MUST be processed before timeout handling when input readiness and expiry coincide.

#### Scenario: Poll timeout reveals idle feedback
- **WHEN** an eligible idle deadline expires and no descriptor is readable
- **THEN** the poll loop dispatches frontend timeout handling and the signature is rendered

#### Scenario: Queued input beats expiry
- **WHEN** frontend input is readable at the same instant an armed deadline becomes due
- **THEN** the input mutation is processed and the stale prefix is not revealed

### Requirement: Sensitive state lifecycle
The seed and derived key MUST remain in locked, non-dumpable memory and MUST be zeroized before release. HMAC state and digest buffers MUST be zeroized after index selection. Revealed indices, deadlines, automatic timing samples, and derived material MUST be cleared on full reset, mode exit, submission, cancellation, and process teardown.

#### Scenario: Cancellation clears feedback state
- **WHEN** a prompt with a visible signature is cancelled
- **THEN** the signature indices, deadline, timing samples, HMAC temporaries, and secret buffer are cleared during prompt teardown

### Requirement: Leakage contract
Documentation MUST state that the fixed-length mask row does not expose candidate length through its contents, but does not hide observed input timing. It MUST state that idle and auto-idle reveal every eligible prefix at which inactivity reaches the selected threshold, live reveals every eligible edit state, and possession of both a recording and the derivation seed permits candidate testing. It MUST state that the feature does not verify backend acceptance and does not protect against same-UID access to the seed or an unrestricted local derivation oracle.
#### Scenario: Manual documents retrospective risk
- **WHEN** a user reads the emoji feedback mode documentation
- **THEN** it distinguishes sampled-prefix idle leakage from every-edit live leakage and identifies the explicit seed as the security root
