## Purpose

Defines the Wayland frontend's software render pipeline: tiny-skia + cosmic-text drawing into SHM buffers, rounded-corner backgrounds, grayscale-AA text with bundled fonts, a cached pin-mask glyph, RGBA→Argb8888 byte swap, button hotspots, and fractional-scale-aware rendering.

## Requirements

### Requirement: Software render pipeline with tiny-skia + cosmic-text

The `src/frontend/wayland/render.rs` module MUST render the UI into the SHM buffer's `MmapMut` slice via `tiny_skia::PixmapMut::from_bytes`. The pipeline: clear to transparent → draw background (bordered rectangle + rounded corners) → draw TextViews (title, description, prompt, errmessage) → draw pin area (GetPin mode) → draw buttons (ok, notok, cancel) with hotspots → swap R/B channels in-place → return the buffer for commit.

#### Scenario: render produces a committable buffer
- **WHEN** `Surface::render` is called after `configured = true`
- **THEN** it acquires a buffer from the pool, renders into it, swaps R/B, and the buffer is ready for `wl_surface.attach`+`commit`

#### Scenario: render skipped before configure
- **WHEN** `Surface::render` is called before the first `configure` event
- **THEN** it returns without rendering

### Requirement: Bordered rectangle and rounded corners

`draw_background` MUST fill the surface with the background colour and stroke the border, then composite rounded-corner masks if `corner_radius > 0`. The implementation MUST use `tiny-skia` `fill_rect` (background) + `stroke_path` (border) + `PathBuilder` rounded-rect paths (corners); geometry is the contract, not pixel identity.

#### Scenario: corner radius applied
- **WHEN** `config.wayland_ui.corner_radius` is R > 0
- **THEN** the background has rounded corners of radius `min(R, width/2, height/2)`

#### Scenario: no corners when radius is zero
- **WHEN** `config.wayland_ui.corner_radius` is 0
- **THEN** the background is a plain rectangle

### Requirement: Text layout and draw via cosmic-text

`TextView` MUST use `cosmic-text::Buffer` with `FontSystem` + `SwashCache` to shape and lay out text, then blit glyphs onto the `tiny-skia` pixmap via a custom `Renderer` that blends straight-alpha swash pixels onto the premultiplied pixmap. The configured font description (parsed from `font-regular` / `font-large` as an fcft-style pattern `family[:attr=value…]`) MUST reach shaping: its family leads the cosmic-text `Attrs` (generic aliases `sans`/`sans-serif`, `serif`, `mono`/`monospace` map to fontdb generics; an empty family implies sans-serif) and its `size=N` attribute overrides the class default pixel size (14 regular, 20 large); unknown attributes are ignored. Title and prompt shape from `font-large` at bold weight; description, error message and buttons shape from `font-regular` at normal weight. Glyph coverage beyond the selected family is cosmic-text's platform fallback over the shared database — no separate fallback chain is maintained. Text metrics (width/height) MUST be available for layout-box positioning.

#### Scenario: TextView reports layout metrics
- **WHEN** a `TextView` is created for a string S with the regular font
- **THEN** it exposes `width` and `height` suitable for centering in the surface

#### Scenario: configured font reaches shaping
- **WHEN** `config.wayland_ui.font_regular` is `Some("Iosevka:size=22")`
- **THEN** regular labels shape with family `Iosevka` at 22px (cosmic-text falls back to the default sans-serif and bundled faces when the family is unresolved), while title and prompt still shape from the `font-large` description

### Requirement: Grayscale antialiasing only

The render pipeline MUST disable subpixel antialiasing and force grayscale AA. This avoids chromatic aberration on transparent/dynamic backgrounds.

#### Scenario: subpixel AA disabled
- **WHEN** text is rasterized
- **THEN** the swash rasterizer uses grayscale AA, not subpixel/LCD AA

### Requirement: Bundled fallback faces in a system-backed font database

The render pipeline MUST load the bundled DejaVu Sans regular + bold faces via `fontdb::load_font_data(include_bytes!(...))` so rendering works with zero installed fonts. System fonts share the same database: cosmic-text 0.19's `FontSystem::new_with_fonts` scans the system font set unconditionally (the earlier "no system font scan" text described an intent the pinned cosmic-text does not implement), so configured user families resolve against system fonts, with the bundled faces as guaranteed last resort.

#### Scenario: bundled faces always present
- **WHEN** the font system is initialized
- **THEN** the database contains the bundled DejaVu Sans regular and bold faces alongside any system fonts

### Requirement: Cached pin mask glyph

For `GetPin` mode, the pin mask glyph (`•` or `*`) MUST be shaped and rasterized once at init, cached, and blitted iteratively per pin square. The cosmic-text shaper MUST NOT be re-invoked per keystroke.

#### Scenario: pin squares blit cached glyph
- **WHEN** the pin area is drawn with N entered characters
- **THEN** N cached mask-glyph blits are performed, with no cosmic-text shaping call

### Requirement: Premultiplied RGBA to Argb8888 byte swap

Before `wl_surface.commit`, the render pipeline MUST swap the R and B channels of the `tiny-skia` premultiplied RGBA8888 buffer in-place, converting to little-endian `Argb8888` (`[B,G,R,A]`). The swap MUST be correct for all pixels in the buffer.

#### Scenario: byte swap converts RGBA to BGRA
- **WHEN** the render pipeline completes drawing
- **THEN** an in-place R/B swap is applied so the buffer matches `wl_shm::Format::Argb8888` on little-endian

### Requirement: Button hotspots populated on first render

The `Surface` MUST populate its hotspot list on the first render (when `hotspots` is empty), recording the `Effect` (cancel/notok/ok), x, y, width, height of each button region.

#### Scenario: hotspots populated once
- **WHEN** the first `render` call draws the buttons
- **THEN** the hotspot list is populated with one entry per visible button (cancel, notok, ok)

### Requirement: Fractional-scale-aware rendering

When `wp_fractional_scale_v1.preferred_scale` reports a fractional factor F, the render pipeline MUST scale layout dimensions and font metrics by F, set `wl_surface.set_buffer_scale(1)`, and allocate the SHM buffer at the scaled dimensions.

#### Scenario: fractional scale applied
- **WHEN** the compositor advertises a preferred scale of 1.5
- **THEN** the surface dimensions and font metrics are multiplied by 1.5 and `set_buffer_scale(1)` is called
