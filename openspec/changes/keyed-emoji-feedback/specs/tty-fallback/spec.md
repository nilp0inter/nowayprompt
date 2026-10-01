## MODIFIED Requirements

### Requirement: ANSI rendering layout

The `render` function MUST write to stdout in order: (1) clear screen + cursor home (`\x1b[2J\x1b[H`); (2) if `config.labels.title` is `Some`, render it with bold + green background + black foreground, space-padded to width; (3) if `config.labels.description` is `Some`, render it with default attributes, space-padded; (4) if `config.labels.prompt` is `Some`, render it bold, space-padded; (5) if mode is `GetPin`, render a line beginning ` > ` followed by the selected feedback row; (6) if `config.labels.err_message` is `Some`, render it bold + red foreground; (7) if `config.labels.ok` is `Some`, render a button line `enter: <ok>`; (8) if `config.labels.not_ok` is `Some`, render `C-c: <not_ok>`; (9) if `config.labels.cancel` is `Some`, render `escape: <cancel>`. Multi-line label strings MUST wrap at the terminal width. Each label section is followed by a blank line.

When global emoji feedback is off or TTY `use-emoji` is false, the selected feedback row MUST remain `*` repeated `min(pin_square_amount, len)` times followed by `_` repeated `pin_square_amount - len` times. When TTY emoji feedback is enabled and no signature is revealed, it MUST be exactly `count` copies of `mask-emoji` separated by one ASCII space. When a signature is revealed, it MUST be the `count` selected `emoticon` entries separated by one ASCII space.

#### Scenario: Legacy GetPin render with secret
- **WHEN** emoji feedback is off, mode is `GetPin`, `len` is 3, and `pin_square_amount` is 8
- **THEN** the pin row renders ` > ***_____`

#### Scenario: Enabled TTY renders fixed mask
- **WHEN** TTY emoji feedback is enabled, `count` is 3, and the configured mask is the default
- **THEN** the pin row renders ` > ✳️ ✳️ ✳️` independent of secret length

#### Scenario: Revealed TTY signature
- **WHEN** revealed indices select `(^_^)`, `(o_o)`, and `(-_-)`
- **THEN** the pin row renders ` > (^_^) (o_o) (-_-)`

#### Scenario: Title rendered with green background
- **WHEN** `config.labels.title` is `Some("Login")` and the terminal is 20 cols wide
- **THEN** the title line is bold, green background, black foreground, space-padded to 20 columns

#### Scenario: Buttons rendered with key prefixes
- **WHEN** `ok`, `not_ok`, and `cancel` labels are all `Some`
- **THEN** three button lines render: `enter: <ok>`, `C-c: <not_ok>`, `escape: <cancel>`

### Requirement: Input event handling per mode

On `handle_event`, the frontend MUST read raw bytes and run the input parser. For each parsed input: `Enter` → return `Event::UserOk` without calculating or refreshing emoji feedback; `Escape` → return `Event::UserAbort`; `C-c` → if `config.labels.not_ok` is `Some`, return `Event::UserNotOk`, else return `Event::UserAbort`; `C-u` / `C-w` / `C-backspace` → if mode is `GetPin`, call `SecretBuffer::reset()`, notify feedback state only if the buffer changed, and re-render; `Backspace` → if mode is `GetPin`, call `SecretBuffer::delete_backwards()`, notify feedback state only if deletion succeeded, and re-render; `Codepoint(c)` → if mode is `GetPin`, encode `c` as UTF-8, call `SecretBuffer::append_slice(bytes)`, notify feedback state only if append succeeded, then re-render. Inputs in `Message` mode other than `Enter`/`Escape`/`C-c` MUST be ignored. After a terminal event (`UserOk`/`UserAbort`/`UserNotOk`), the frontend MUST clear feedback timing/indices and return the event; the dispatch loop calls `enter_mode(None)` / `deinit` as appropriate.

#### Scenario: Enter returns UserOk without derivation
- **WHEN** mode is `GetPin`, the mask row is visible, and the user presses Enter
- **THEN** `handle_event` returns `Ok(Event::UserOk)` without calculating a signature

#### Scenario: C-u clears the secret and feedback samples
- **WHEN** mode is `GetPin`, the secret buffer holds `"abc"`, and the user presses Ctrl+U
- **THEN** `SecretBuffer::reset()` is called, the buffer and auto-idle samples are empty, and the mask row re-renders if TTY emoji is enabled

#### Scenario: Codepoint appended in GetPin
- **WHEN** mode is `GetPin` and the user types `a`
- **THEN** `SecretBuffer::append_slice(b"a")` is called, feedback processes the successful append, and the selected row re-renders

#### Scenario: C-c with not_ok label
- **WHEN** mode is active, `config.labels.not_ok` is `Some`, and the user presses Ctrl+C
- **THEN** feedback timing state is cleared and `handle_event` returns `Ok(Event::UserNotOk)`

#### Scenario: C-c without not_ok label
- **WHEN** mode is active, `config.labels.not_ok` is `None`, and the user presses Ctrl+C
- **THEN** feedback timing state is cleared and `handle_event` returns `Ok(Event::UserAbort)`

## ADDED Requirements

### Requirement: TTY deadline rendering
The TTY implementation of `next_deadline` MUST expose only an armed eligible idle/auto-idle deadline. `handle_timeout` MUST calculate the current signature only when that deadline is due, replace the fixed mask row with the revealed emoticon row, and render immediately to the TTY fd. It MUST emit no user event.

#### Scenario: TTY idle timeout reveals emoticons
- **WHEN** TTY emoji feedback is enabled and an eligible idle deadline expires without queued input
- **THEN** `handle_timeout` renders the selected emoticon row and the prompt remains active

### Requirement: TTY append batching for automatic cadence
All codepoints parsed from one successful TTY `read(2)` MUST count as one append activity timestamp for auto-idle estimation, even though each codepoint is appended to `SecretBuffer`. Paste and buffered terminal input MUST NOT create artificial zero-duration inter-codepoint samples.

#### Scenario: Pasted password contributes one activity point
- **WHEN** one `read(2)` returns UTF-8 bytes containing eight codepoints
- **THEN** auto-idle records one append activity timestamp and no internal zero-duration intervals

### Requirement: TTY rendering remains terminal-controlled
nowayprompt MUST write configured mask/emoticon UTF-8 bytes exactly and MUST NOT scan, load, or validate terminal fonts. TTY `emoji-font` configuration MUST have no effect because glyph rendering belongs to the terminal.

#### Scenario: Custom Wayland font does not affect TTY bytes
- **WHEN** `emoji-font` is configured and TTY emoji feedback is enabled
- **THEN** TTY emits the same configured mask/emoticon UTF-8 strings as it would without the font path
