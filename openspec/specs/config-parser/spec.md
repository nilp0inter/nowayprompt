## Purpose

Defines parsing and validation of the nowayprompt INI configuration format.

## Requirements

### Requirement: Streaming INI line parser via BufRead

The config parser MUST read `wayprompt.5` INI files via `std::io::BufRead` line-by-line without loading the entire file into a heap structure. Each line MUST be trimmed of leading/trailing whitespace. Inline comments starting with `#` MUST be stripped. Trailing semicolons (`;`) on assignment values MUST be stripped (`wayprompt.5` dialect compatibility). After semicolon stripping, one pair of matching surrounding quotes (single or double) around the value MUST be stripped (upstream `zig-ini` parity); interior content including `=` is preserved verbatim and mismatched quotes are retained. The first `=` on the line splits key from value, so values may themselves contain `=`.

#### Scenario: Basic key-value assignment
- **WHEN** the parser reads `button-inner-padding = 5;`
- **THEN** the key is `button-inner-padding`, the value is `5` (semicolon stripped)

#### Scenario: Quoted font description value
- **WHEN** the parser reads `font-regular = "Iosevka:size=22";`
- **THEN** the key is `font-regular`, the value is `Iosevka:size=22` (semicolon and matching surrounding quotes stripped, inner `=` preserved)

#### Scenario: Inline comment stripping
- **WHEN** the parser reads `border = 2  # thickness`
- **THEN** the key is `border`, the value is `2` (comment stripped, whitespace trimmed)

#### Scenario: Empty and whitespace-only lines
- **WHEN** the parser encounters a blank line or whitespace-only line
- **THEN** the line is skipped without error

### Requirement: Section dispatch (general, colours)

The parser MUST track the current section (`[general]` or `[colours]`) and dispatch assignments to the corresponding field set. Unknown sections MUST produce a parse error with file path and line number.

#### Scenario: Section header recognition
- **WHEN** the parser reads `[general]`
- **THEN** subsequent assignments are dispatched to general/UI fields until the next section header

#### Scenario: Unknown section
- **WHEN** the parser reads `[unknown]`
- **THEN** a parse error is returned with the file path and line number

#### Scenario: Assignment outside any section
- **WHEN** the parser reads a `key = value` line before any section header
- **THEN** a parse error is returned indicating assignments must be part of a section

### Requirement: Hyphen-to-underscore field name normalization

Field names in the config file use hyphens (`pin-square-size`); Rust struct fields use underscores (`pin_square_size`). The parser MUST normalize hyphens to underscores when matching keys to struct fields. Keys are hyphenated only (upstream `fieldEql` parity): underscore spellings of known fields (e.g. `font_regular`) MUST be rejected. A mismatched key MUST produce a parse error indicating the unknown variable and section; the error aborts the file, so assignments after the offending line are not applied.

#### Scenario: Hyphenated key match
- **WHEN** the parser reads `pin-square-size = 18` in `[general]`
- **THEN** the field `pin_square_size` is set to 18

#### Scenario: Unknown variable
- **WHEN** the parser reads `nonexistent = 1` in `[general]`
- **THEN** a parse error is returned indicating the unknown variable name

#### Scenario: Underscore-spelled key rejected
- **WHEN** the parser reads `font_regular = "Iosevka:size=22";` in `[general]`
- **THEN** a parse error is returned naming `font_regular` as an unknown variable, and a `pin-square-amount` assignment on a later line is not applied

### Requirement: Hex color to premultiplied u16 RGBA conversion

Color values in `[colours]` MUST be parsed as hex strings `0xRRGGBB` (6 hex digits) or `0xRRGGBBAA` (8 hex digits). The parser MUST convert to a premultiplied-alpha `Colour { red: u16, green: u16, blue: u16, alpha: u16 }` using the formula: `channel_16 = round(channel_8 / 255 * 65535)`, then `premul_channel_16 = round(channel_16 * alpha_16 / 0xffff)`. When 6-digit form is given, alpha defaults to `0xff` (opaque).

#### Scenario: 6-digit hex (opaque)
- **WHEN** the parser reads `background = 0xffffff`
- **THEN** the color is `{ red: 65535, green: 65535, blue: 65535, alpha: 65535 }` (premultiplied; alpha=opaque so channels unchanged)

#### Scenario: 8-digit hex with alpha
- **WHEN** the parser reads `background = 0xff000080`
- **THEN** alpha is 0x80 (128), and red/green/blue are premultiplied by `alpha/255` (≈50%)

#### Scenario: Invalid color format
- **WHEN** the parser reads `background = red` or `background = 0xGGG`
- **THEN** a parse error is returned indicating a bad color value

### Requirement: Config file path resolution

The parser MUST confine config lookup to a single base directory: `$XDG_CONFIG_HOME` when set and non-empty, else `$HOME/.config` when `HOME` is set and non-empty, else `/etc`. Within that base it MUST probe, in order, `<base>/nowayprompt/config.ini` and `<base>/wayprompt/config.ini`, and load the first candidate that exists. A missing candidate falls through to the next candidate silently; a candidate that exists but fails to read or parse MUST surface its error without falling through. Candidate files are never merged and candidates outside the selected base are never probed. If neither candidate exists in the selected base, parsing MUST succeed with defaults (no config loaded).

The `nowayprompt/config.ini` primary candidate is an intentional, documented divergence from `pkgs.wayprompt`, which reads only `wayprompt/config.ini`; the `wayprompt/config.ini` fallback keeps existing `wayprompt` installations working unchanged.

#### Scenario: XDG_CONFIG_HOME selects the base
- **WHEN** `XDG_CONFIG_HOME` is set to `/home/user/.config`
- **THEN** the candidates are `/home/user/.config/nowayprompt/config.ini` then `/home/user/.config/wayprompt/config.ini`, and no `$HOME/.config` or `/etc` path is probed

#### Scenario: No XDG, HOME selects the base
- **WHEN** `XDG_CONFIG_HOME` is unset but `HOME` is `/home/user`
- **THEN** the candidates are `/home/user/.config/nowayprompt/config.ini` then `/home/user/.config/wayprompt/config.ini`

#### Scenario: /etc base
- **WHEN** neither `XDG_CONFIG_HOME` nor `HOME` is set and non-empty
- **THEN** the candidates are `/etc/nowayprompt/config.ini` then `/etc/wayprompt/config.ini`

#### Scenario: Primary candidate wins without merging
- **WHEN** both `nowayprompt/config.ini` and `wayprompt/config.ini` exist in the selected base
- **THEN** only `nowayprompt/config.ini` is loaded; `wayprompt/config.ini` is ignored entirely

#### Scenario: Empty primary is an existing winner
- **WHEN** `nowayprompt/config.ini` exists but is empty and `wayprompt/config.ini` also exists
- **THEN** the empty primary is loaded (yielding defaults) and the fallback is not probed

#### Scenario: Silent fallback to wayprompt config
- **WHEN** `nowayprompt/config.ini` does not exist and `wayprompt/config.ini` exists
- **THEN** `wayprompt/config.ini` is loaded without any diagnostic

#### Scenario: Existing bad primary does not fall through
- **WHEN** `nowayprompt/config.ini` exists but cannot be read or fails to parse
- **THEN** the error is surfaced and `wayprompt/config.ini` is not loaded

#### Scenario: No config file exists
- **WHEN** neither candidate exists in the selected base
- **THEN** parsing succeeds with defaults (no error)

### Requirement: Wayland UI dimension fields

The `[general]` section MUST accept integer fields: `vertical-padding`, `horizontal-padding`, `button-inner-padding`, `pin-square-size`, `pin-square-border`, `button-border`, `border`, `corner-radius` (u16), `pin-square-amount`. It MUST accept optional string fields: `font-regular`, `font-large` (wayprompt(5) font descriptions: fcft/fontconfig-style patterns `family[:attr=value…]`, e.g. `sans:size=14`; values may carry matching surrounding quotes). Invalid integer values MUST produce a parse error.

#### Scenario: Integer field assignment
- **WHEN** the parser reads `vertical-padding = 10` in `[general]`
- **THEN** `vertical-padding` is set to 10

#### Scenario: Invalid integer
- **WHEN** the parser reads `border = wide` in `[general]`
- **THEN** a parse error is returned indicating an invalid positive integer

### Requirement: Color field names in [colours]

The `[colours]` section MUST accept the `wayprompt.5` field names: `background`, `border`, `text`, `error-text`, `pin-background`, `pin-border`, `pin-square`, `ok-button`, `ok-button-border`, `ok-button-text`, `not-ok-button`, `not-ok-button-border`, `not-ok-button-text`, `cancel-button`, `cancel-button-border`, `cancel-button-text`. Each accepts a hex color value.

#### Scenario: Known color field
- **WHEN** the parser reads `error-text = 0xe0002b` in `[colours]`
- **THEN** `error-text` is set to the premultiplied color

#### Scenario: Unknown color field
- **WHEN** the parser reads `unknown-colour = 0x000000` in `[colours]`
- **THEN** a parse error is returned indicating the unknown variable
