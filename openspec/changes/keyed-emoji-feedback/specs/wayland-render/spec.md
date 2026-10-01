## MODIFIED Requirements

### Requirement: Software render pipeline with tiny-skia + cosmic-text

The `src/frontend/wayland/render.rs` module MUST render the UI into the SHM buffer's `MmapMut` slice via `tiny_skia::PixmapMut::from_bytes`. The pipeline: clear to transparent → draw background (bordered rectangle + rounded corners) → draw TextViews (title, description, prompt, errmessage) → draw pin feedback area (legacy squares when feedback is off, otherwise repeated mask emoji or a revealed signature in GetPin mode) → draw buttons (ok, notok, cancel) with hotspots → swap R/B channels in-place → return the buffer for commit.

#### Scenario: render produces a committable buffer
- **WHEN** `Surface::render` is called after `configured = true`
- **THEN** it acquires a buffer from the pool, renders the selected feedback state, swaps R/B, and the buffer is ready for `wl_surface.attach`+`commit`

#### Scenario: render skipped before configure
- **WHEN** `Surface::render` is called before the first `configure` event
- **THEN** it returns without rendering

#### Scenario: Armed feedback renders fixed mask
- **WHEN** idle or auto-idle feedback has an armed deadline and `count` is N
- **THEN** the pin area blits N copies of the configured cached mask emoji and does not expose signature indices

#### Scenario: revealed feedback renders indices
- **WHEN** feedback state contains revealed table indices
- **THEN** the pin area blits the corresponding cached table entries in order

### Requirement: Bundled fallback font, no system font scan

The render pipeline MUST load bundled DejaVu text faces and one redistributable color emoji face via `fontdb::load_font_data(include_bytes!(...))` and MUST NOT call `fontdb::load_system_fonts()`. If `emoji-font` is configured, it MUST load that exact file, resolve its face family, and place it before the bundled emoji face for mask and signature shaping. An explicit file that cannot be opened, parsed, or provide a usable face MUST fail Wayland initialization rather than be ignored. The bundled font's license text MUST be distributed with the asset; user-provided fonts are not redistributed.

#### Scenario: no system font scan at startup
- **WHEN** `Wayland::init` initializes the font system
- **THEN** only the bundled faces and any one explicitly configured emoji-font file are loaded and `load_system_fonts` is never called

#### Scenario: Canonical emoji and default mask have bundled coverage
- **WHEN** the canonical table entries and default `✳️` mask are shaped during frontend initialization
- **THEN** all resolve entirely through the bundled emoji face without a missing-glyph fallback

#### Scenario: Explicit emoji font is preferred
- **WHEN** `emoji-font` names a valid face that covers the configured mask and table entries
- **THEN** those entries are rasterized from that face before considering the bundled fallback

#### Scenario: Missing explicit emoji font fails
- **WHEN** `emoji-font` names an unreadable or invalid file
- **THEN** Wayland initialization fails with the configured path in the error

### Requirement: Cached pin mask glyph

For `GetPin` mode with emoji feedback off, the legacy pin mask glyph (`•` or `*`) MUST remain cached and blitted per pin square. With emoji feedback enabled, the configured `mask-emoji` and every active public table entry MUST be shaped and rasterized into cached bitmaps during frontend initialization. A masked row MUST blit the mask bitmap exactly `count` times, independent of secret length. The cosmic-text shaper MUST NOT be re-invoked for unchanged feedback during keystrokes or redraws.

#### Scenario: Fixed mask blits cached emoji
- **WHEN** the pin area is masked with `count = N`
- **THEN** N cached mask-emoji blits are performed with no cosmic-text shaping call

#### Scenario: revealed emoji blit cached entries
- **WHEN** a revealed signature selects N table indices
- **THEN** N cached entry bitmaps are blitted with no cosmic-text shaping call

## ADDED Requirements

### Requirement: Color emoji raster blending
The renderer MUST accept both grayscale-mask and color-image results from swash. Color emoji pixels MUST be alpha-composited onto the premultiplied tiny-skia pixmap without applying the text foreground color. Grayscale glyphs MUST retain the existing foreground-color behavior.

#### Scenario: Color glyph preserves palette
- **WHEN** a bundled color emoji glyph is rasterized and drawn
- **THEN** its source RGB palette is preserved while its alpha is composited over the pin background

### Requirement: Configured emoji glyph validation
The configured Wayland `mask-emoji` and every configured `table-entry` MUST shape to at least one non-missing glyph after trying the explicitly configured emoji face, if any, followed by the bundled face. A missing glyph, zero-size raster, or shaping failure after the full explicit fallback chain MUST report whether the mask or which table index failed and MUST fail frontend initialization rather than display tofu or silently substitute legacy squares.

#### Scenario: Unsupported custom emoji rejected
- **WHEN** configured table entry 4 is not covered by the bundled fonts
- **THEN** Wayland initialization fails with an error identifying entry 4

#### Scenario: Unsupported custom mask rejected
- **WHEN** the configured `mask-emoji` is not covered by the bundled fonts
- **THEN** Wayland initialization fails with an error identifying the mask value

### Requirement: Stable feedback geometry
The GetPin feedback box MUST reserve dimensions sufficient for `count` entries at configured `size`, including existing padding and borders, whenever emoji feedback is enabled. Switching between the repeated mask emoji and a revealed signature MUST NOT change the layer-surface dimensions or button hotspot coordinates. Fractional scaling MUST apply to mask/signature size and spacing with the same scale used for other UI geometry.

#### Scenario: Idle reveal does not resize surface
- **WHEN** an armed fixed-length mask row changes to a revealed emoji signature
- **THEN** the existing surface and hotspot geometry are retained and only pixel contents change

#### Scenario: Emoji row respects fractional scale
- **WHEN** emoji size is 32 and preferred scale is 1.5
- **THEN** emoji bitmaps, spacing, and reserved feedback geometry are rendered at the corresponding scaled dimensions
