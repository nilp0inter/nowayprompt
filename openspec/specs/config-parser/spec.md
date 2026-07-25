## Requirements

### Requirement: Streaming INI line parser via BufRead

The config parser MUST read `wayprompt.5` INI files via `std::io::BufRead` line-by-line without loading the entire file into a heap structure. Each line MUST be trimmed of leading/trailing whitespace. Inline comments starting with `#` MUST be stripped. Trailing semicolons (`;`) on assignment values MUST be stripped (parity with legacy `zig-ini` `.semicolon` tokenization).

#### Scenario: Basic key-value assignment
- **WHEN** the parser reads `button_inner_padding = 5;`
- **THEN** the key is `button_inner_padding`, the value is `5` (semicolon stripped)

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

Field names in the config file use hyphens (`pin-square-size`); Rust struct fields use underscores (`pin_square_size`). The parser MUST normalize hyphens to underscores when matching keys to struct fields. A mismatched key MUST produce a parse error indicating the unknown variable and section.

#### Scenario: Hyphenated key match
- **WHEN** the parser reads `pin-square-size = 18` in `[general]`
- **THEN** the field `pin_square_size` is set to 18

#### Scenario: Unknown variable
- **WHEN** the parser reads `nonexistent = 1` in `[general]`
- **THEN** a parse error is returned indicating the unknown variable name

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

The parser MUST resolve the config file path in order: `$XDG_CONFIG_HOME/wayprompt/config.ini`, then `$HOME/.config/wayprompt/config.ini`, then `/etc/wayprompt/config.ini`. If no file exists at any path, parsing MUST succeed silently (no config loaded; defaults used).

#### Scenario: XDG_CONFIG_HOME set
- **WHEN** `XDG_CONFIG_HOME` is set to `/home/user/.config`
- **THEN** the config path is `/home/user/.config/wayprompt/config.ini`

#### Scenario: No XDG, HOME set
- **WHEN** `XDG_CONFIG_HOME` is unset but `HOME` is `/home/user`
- **THEN** the config path is `/home/user/.config/wayprompt/config.ini`

#### Scenario: No config file exists
- **WHEN** no config file exists at any resolved path
- **THEN** parsing succeeds with defaults (no error)

### Requirement: Wayland UI dimension fields

The `[general]` section MUST accept integer fields: `vertical_padding`, `horizontal_padding`, `button_inner_padding`, `pin_square_size`, `pin_square_border`, `button_border`, `border`, `corner_radius` (u16), `pin_square_amount`. It MUST accept optional string fields: `font_regular`, `font_large` (font file paths). Invalid integer values MUST produce a parse error.

#### Scenario: Integer field assignment
- **WHEN** the parser reads `vertical_padding = 10` in `[general]`
- **THEN** `vertical_padding` is set to 10

#### Scenario: Invalid integer
- **WHEN** the parser reads `border = wide` in `[general]`
- **THEN** a parse error is returned indicating an invalid positive integer

### Requirement: Color field names in [colours]

The `[colours]` section MUST accept the legacy field names: `background`, `border`, `text`, `error_text`, `pin_background`, `pin_border`, `pin_square`, `ok_button`, `ok_button_border`, `ok_button_text`, `not_ok_button`, `not_ok_button_border`, `not_ok_button_text`, `cancel_button`, `cancel_button_border`, `cancel_button_text`. Each accepts a hex color value.

#### Scenario: Known color field
- **WHEN** the parser reads `error_text = 0xe0002b` in `[colours]`
- **THEN** `error_text` is set to the premultiplied color

#### Scenario: Unknown color field
- **WHEN** the parser reads `unknown_colour = 0x000000` in `[colours]`
- **THEN** a parse error is returned indicating the unknown variable
