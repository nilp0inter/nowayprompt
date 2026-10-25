## MODIFIED Requirements

### Requirement: Evdev keycode +8 offset and keysym lookup

On a `wl_keyboard.key` press event, the module MUST add 8 to the raw keycode, call `xkb::State::key_get_one_sym`, and dispatch on the keysym. Ctrl+BackSpace/u/w clears the pin buffer; Return → `UserOk`; Escape → `UserAbort`; BackSpace deletes one char; Delete is a no-op; other keys append UTF-8 via `key_get_utf8` to the `SecretBuffer`. Every successful mutation in `GetPin` mode MUST notify the shared feedback controller before the surface re-renders. No-op deletes, rejected appends, modifiers, and unrecognized keys MUST NOT change feedback deadlines or automatic timing samples.

#### Scenario: Return emits UserOk
- **WHEN** the user presses Return (keysym `Return`)
- **THEN** the frontend sets `exit_reason = UserOk` without calculating or refreshing emoji feedback

#### Scenario: Escape emits UserAbort
- **WHEN** the user presses Escape (keysym `Escape`)
- **THEN** the frontend sets `exit_reason = UserAbort` and clears feedback timing state

#### Scenario: BackSpace deletes one pin char
- **WHEN** the user presses BackSpace in `GetPin` mode and one codepoint is deleted
- **THEN** `SecretBuffer::delete_backwards` is called, the feedback controller processes the mutation, and the surface re-renders

#### Scenario: BackSpace on empty input is not activity
- **WHEN** the user presses BackSpace while the pin buffer is empty
- **THEN** no deadline or automatic cadence sample changes

#### Scenario: Ctrl+u clears the pin buffer
- **WHEN** the user presses Ctrl+u in `GetPin` mode with non-empty input
- **THEN** `SecretBuffer::reset` is called, feedback state returns to below-minimum, samples are discarded, and the surface re-renders

#### Scenario: Unicode key appends to pin
- **WHEN** the user presses a Unicode key in `GetPin` mode and the append succeeds
- **THEN** `key_get_utf8` output is appended to the `SecretBuffer`, feedback state processes one append activity, and the surface re-renders

### Requirement: Pointer and touch hotspot hit-testing

The `Seat` MUST bind `wl_pointer` and `wl_touch`, track motion/down/up events, and on a button press/touch-down consult the `Surface` hotspot list to determine the `HotSpotEffect` (Cancel/NotOk/Ok), mapping it to `UserAbort`/`UserNotOk`/`UserOk`. Cursor shape MUST be set via `wp_cursor_shape_manager_v1`. Terminal hotspot effects MUST clear feedback timing state and MUST NOT trigger emoji derivation.

#### Scenario: click on OK hotspot emits UserOk
- **WHEN** a pointer button press lands within the OK button's hotspot rectangle
- **THEN** the frontend sets `exit_reason = UserOk` immediately without calculating or refreshing emoji feedback

#### Scenario: touch on cancel hotspot emits UserAbort
- **WHEN** a touch-down event lands within the cancel button's hotspot rectangle
- **THEN** the frontend clears feedback timing state and sets `exit_reason = UserAbort`

#### Scenario: cursor shape set on pointer enter
- **WHEN** the pointer enters the surface
- **THEN** the cursor shape is set to `Pointer` via `wp_cursor_shape_device_v1`

## ADDED Requirements

### Requirement: Non-secret Wayland events preserve feedback timing
Pointer motion outside terminal hotspots, modifier changes, configure events, redraws, and output-scale changes MUST preserve the current feedback state and deadline. A redraw MUST render the existing state and MUST NOT recalculate a signature.

#### Scenario: Configure event preserves armed deadline
- **WHEN** a surface configure arrives while idle feedback is armed
- **THEN** the surface redraws the fixed-length mask row and retains the original absolute deadline

#### Scenario: Redraw preserves revealed signature
- **WHEN** a redraw occurs while a signature is visible
- **THEN** the same derived indices are rendered without another HMAC calculation
