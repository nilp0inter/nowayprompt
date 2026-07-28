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

The render pipeline MUST retain logical layout dimensions and use the exact
scale factor `F = P / 120` when a surface has both
`wp_fractional_scale_v1` and `wp_viewport` and receives `preferred_scale(P)`.
It MUST allocate an SHM buffer of
`ceil(logical_width * F)` by `ceil(logical_height * F)` physical pixels, render
all geometry and glyphs at `F`, call
`wp_viewport.set_destination(logical_width, logical_height)`, and keep
`wl_surface.set_buffer_scale(1)`. Logical text metrics, configured UI values,
surface geometry, pointer coordinates, and hotspots MUST NOT be multiplied by
`F`. Buffer dimension and byte-size arithmetic MUST be checked and MUST return
a frontend error rather than wrap, truncate, or allocate an invalid buffer.

#### Scenario: fractional scale applied
- **WHEN** the logical surface is 200 by 100 and the compositor reports `preferred_scale(180)`
- **THEN** a 300 by 150 buffer is rendered at 1.5×, the viewport destination is 200 by 100, and buffer scale remains 1

#### Scenario: non-integral physical dimension rounded up
- **WHEN** the logical width is 101 and the preferred scale is 150/120
- **THEN** the physical buffer width is `ceil(101 * 150 / 120)`, preserving the complete logical extent

#### Scenario: logical interaction geometry is unchanged
- **WHEN** a button hotspot at logical coordinates `(x, y, width, height)` is rendered at a fractional scale
- **THEN** pointer and touch hit-testing continues to use those unchanged logical coordinates

#### Scenario: physical size arithmetic overflows
- **WHEN** a logical dimension or scale cannot be represented safely by the SHM allocation path
- **THEN** rendering returns a frontend error without allocating or attaching a buffer

### Requirement: Integer output-scale fallback

The render pipeline MUST use the effective positive integer output scale N when
fractional scaling cannot be enabled because either
`wp_fractional_scale_v1` or `wp_viewporter` is unavailable. It MUST allocate an SHM
buffer of `logical_width * N` by `logical_height * N`, render geometry and
glyphs at N, call `wl_surface.set_buffer_scale(N)`, and MUST NOT set a viewport
destination. At N = 1, observable geometry and rendered output MUST remain
compatible with the existing 1× implementation.

#### Scenario: integer scale applied
- **WHEN** the effective output scale is 2 and fractional scaling is unavailable
- **THEN** the buffer has twice the logical width and height, drawing occurs at 2×, and `set_buffer_scale(2)` preserves the logical surface size

#### Scenario: scale one preserves current behavior
- **WHEN** the effective output scale is 1
- **THEN** buffer and logical dimensions are equal and the rendered frame retains existing 1× geometry

### Requirement: Scale-independent logical layout

The render pipeline MUST shape and measure text and calculate all UI layout in
logical pixels. Physical rendering MUST scale backgrounds, borders, corner
radii, pin masks, button geometry, and glyph rasterization from the same logical
layout. A scale change MUST reuse or recompute physical rendering resources as
needed without changing the logical layout result. Cursor size MUST remain
compositor-managed through `wp_cursor_shape_manager_v1`.

#### Scenario: scale change preserves logical dimensions
- **WHEN** an already configured surface changes from 1× to 1.5×
- **THEN** its logical width, logical height, text placement, and hotspots remain unchanged while its physical buffer and rasterization scale change

#### Scenario: glyphs rasterized at physical scale
- **WHEN** text is drawn at scale F
- **THEN** glyph rasterization uses F rather than enlarging a glyph image rasterized at 1×

#### Scenario: cursor remains compositor-scaled
- **WHEN** the surface scale changes while a pointer is present
- **THEN** cursor shape selection remains unchanged and no client-side cursor buffer is allocated

### Requirement: Scale-aware buffer lifecycle

The buffer pool MUST key reusable buffers by physical width and height. When the
effective scale changes, rendering MUST acquire a buffer at the new physical
dimensions and MUST NOT reuse a busy or mismatched buffer. Previously attached
buffers MUST remain alive until their `wl_buffer.release` events, after which
normal pool culling MAY reclaim them.

#### Scenario: scale change allocates matching buffer
- **WHEN** a 200 by 100 logical surface changes from 1× to 2× while its old buffer is busy
- **THEN** rendering uses a distinct 400 by 200 buffer and retains the old buffer until release

#### Scenario: idle matching physical buffer reused
- **WHEN** the pool contains an idle buffer whose physical dimensions match the current scaled render
- **THEN** that buffer is reused without allocation
