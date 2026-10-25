## MODIFIED Requirements

### Requirement: Section dispatch (general, colours)

The parser MUST track the current section (`[general]`, `[colours]`, `[emoji]`, or `[tty]`) and dispatch assignments to the corresponding field set. Unknown sections MUST produce a parse error with file path and line number.

#### Scenario: Section header recognition
- **WHEN** the parser reads `[general]`
- **THEN** subsequent assignments are dispatched to general/UI fields until the next section header

#### Scenario: Emoji section recognition
- **WHEN** the parser reads `[emoji]`
- **THEN** subsequent assignments are dispatched to emoji feedback fields until the next section header

#### Scenario: TTY section recognition
- **WHEN** the parser reads `[tty]`
- **THEN** subsequent assignments are dispatched to TTY-specific fields until the next section header

#### Scenario: Unknown section
- **WHEN** the parser reads `[unknown]`
- **THEN** a parse error is returned with the file path and line number

#### Scenario: Assignment outside any section
- **WHEN** the parser reads a `key = value` line before any section header
- **THEN** a parse error is returned indicating assignments must be part of a section

## ADDED Requirements

### Requirement: Emoji feedback configuration fields
The `[emoji]` section MUST accept `mode`, `secret-file`, `emoji-font`, `count`, `mask-emoji`, `minimum-length`, `idle-timeout-ms`, `size`, and repeatable `table-entry` fields. `mode` MUST accept exactly `off`, `idle`, `auto-idle`, or `live` and default to `off`. `count`, `minimum-length`, `idle-timeout-ms`, and `size` MUST parse as decimal integers and default to `3`, `8`, `1500`, and `32` respectively. `mask-emoji` MUST default to `✳️` (U+2733 U+FE0F). `secret-file` and optional `emoji-font` MUST be retained as path references; `Config` MUST NOT contain seed contents or loaded font bytes.

#### Scenario: Existing configuration stays disabled
- **WHEN** a configuration has no `[emoji]` section
- **THEN** emoji mode is `off` and existing square feedback behavior is unchanged

#### Scenario: Complete manual-idle configuration
- **WHEN** `[emoji]` contains `mode = idle`, `secret-file = /run/user/1000/emoji.key`, `minimum-length = 10`, and `idle-timeout-ms = 2500`
- **THEN** those values populate the emoji configuration and the path remains a non-secret path value

#### Scenario: Unknown emoji mode rejected
- **WHEN** `[emoji]` contains `mode = decoy`
- **THEN** parsing fails with the file path and line number

### Requirement: Configurable fixed-length mask emoji
`mask-emoji` MUST parse as one non-empty trimmed UTF-8 value. While emoji feedback is enabled but no signature is revealed, a frontend with emoji rendering enabled MUST render exactly `count` copies of this value, independent of the current secret length. The mask value is public, is not password-derived, and need not appear in `table-entry`.

#### Scenario: Default asterisk emoji mask
- **WHEN** emoji feedback is enabled without `mask-emoji`
- **THEN** every masked position uses `✳️`

#### Scenario: Custom mask emoji
- **WHEN** `[emoji]` contains `count = 3` and `mask-emoji = 🔒`
- **THEN** masked feedback is `🔒 🔒 🔒` regardless of whether the candidate contains zero, one, or seven codepoints

#### Scenario: Empty mask rejected
- **WHEN** `[emoji]` contains an empty `mask-emoji`
- **THEN** configuration validation fails at that line

### Requirement: Optional explicit emoji font path
`emoji-font` MUST parse as one non-empty filesystem path naming the exact font file Wayland should prefer for mask and signature glyphs. The parser MUST NOT interpret it as a system font family, scan font directories, or load file contents. If omitted, only the bundled default emoji face is selected.

#### Scenario: Custom emoji font path retained
- **WHEN** `[emoji]` contains `emoji-font = /home/alice/.local/share/fonts/MyEmoji.ttf`
- **THEN** the exact path is retained for Wayland initialization

#### Scenario: Empty emoji font path rejected
- **WHEN** `[emoji]` contains an empty `emoji-font`
- **THEN** configuration validation fails at that line

### Requirement: Emoji numeric validation
Emoji `count` MUST be in `1..=8`, `minimum-length` in `1..=1024`, `idle-timeout-ms` in `1..=60000`, and `size` in `8..=256`. A value outside its range or a non-decimal value MUST produce a configuration error containing the field and source location.

#### Scenario: Zero idle timeout rejected
- **WHEN** `[emoji]` contains `idle-timeout-ms = 0`
- **THEN** configuration validation fails rather than collapsing idle mode into live mode

#### Scenario: Excessive signature count rejected
- **WHEN** `[emoji]` contains `count = 9`
- **THEN** configuration validation fails with the source field and location

### Requirement: Repeatable public emoji table entries
Each `table-entry` assignment in `[emoji]` MUST append its trimmed UTF-8 value in source order. If no entries are configured, the canonical built-in table MUST remain selected. An explicitly configured table MUST contain between 2 and 1024 non-empty, byte-distinct entries.

#### Scenario: Repeated entries preserve order
- **WHEN** `[emoji]` contains `table-entry = 🍎`, then `table-entry = 🦊`, then `table-entry = 🚲`
- **THEN** the configured ordered table is `["🍎", "🦊", "🚲"]`

#### Scenario: Empty entry rejected
- **WHEN** `[emoji]` contains `table-entry =` with no value
- **THEN** configuration validation fails at that line

### Requirement: Enabled mode requires a seed path
A final configuration with `mode` other than `off` MUST contain a non-empty `secret-file` path. The parser MUST reject an enabled mode without the reference and MUST NOT silently select `off`.

#### Scenario: Live mode without secret path rejected
- **WHEN** `[emoji]` selects `mode = live` without `secret-file`
- **THEN** configuration validation fails before frontend initialization

### Requirement: TTY emoji configuration fields
The `[tty]` section MUST accept `use-emoji` and repeatable `emoticon` fields. `use-emoji` MUST parse exactly `true` or `false` and default to `false`. Repeated `emoticon` values MUST preserve source order and, when TTY emoji is enabled, MUST form a table of 2 to 1024 non-empty, byte-distinct UTF-8 strings. TTY emoji activation additionally requires the global emoji mode to be non-off.

#### Scenario: TTY remains square-only by default
- **WHEN** no `[tty]` section is present
- **THEN** `use-emoji` is false and TTY renders the existing square row

#### Scenario: Ordered emoticon table
- **WHEN** `[tty]` enables `use-emoji` and supplies repeated `emoticon` values `(^_^)` and `(o_o)`
- **THEN** the values form the ordered TTY feedback table

#### Scenario: TTY opt-in without table rejected
- **WHEN** `use-emoji = true` and no valid `emoticon` values are configured
- **THEN** configuration validation fails instead of falling back to terminal-dependent glyphs
