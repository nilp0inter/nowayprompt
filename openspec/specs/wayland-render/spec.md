## Purpose

Defines the Wayland frontend's software render pipeline: tiny-skia + cosmic-text drawing into SHM buffers, rounded-corner backgrounds, grayscale-AA text with bundled fonts, a cached pin-mask glyph, RGBA→Argb8888 byte swap, button hotspots, and fractional-scale-aware rendering.

## Requirements

### Requirement: Software render pipeline with tiny-skia + cosmic-text

The `src/frontend/wayland/render.rs` module MUST render the UI into the SHM buffer's `MmapMut` slice via `tiny_skia::PixmapMut::from_bytes`. The pipeline: clear to transparent → draw background (bordered rectangle + rounded corners) → draw TextViews (title, description, prompt, errmessage) → draw pin area (GetPin mode) → draw buttons (ok, notok, cancel) with hotspots → swap R/B channels in-place → return the buffer for commit (parity with `Wayland.zig:887-1036`).

#### Scenario: render produces a committable buffer
- **WHEN** `Surface::render` is called after `configured = true`
- **THEN** it acquires a buffer from the pool, renders into it, swaps R/B, and the buffer is ready for `wl_surface.attach`+`commit`

#### Scenario: render skipped before configure
- **WHEN** `Surface::render` is called before the first `configure` event
- **THEN** it returns without rendering (parity with `Wayland.zig:888`)

### Requirement: Bordered rectangle and rounded corners

`drawBackground` MUST fill the surface with the background colour and stroke the border, then composite rounded-corner masks if `corner_radius > 0`. The Rust implementation MUST use `tiny-skia` `fill_rect` (background) + `stroke_path` (border) + `PathBuilder` rounded-rect paths (corners) to reproduce the legacy `pixman.composite32` result behaviorally (D1: geometry, not pixels).

#### Scenario: corner radius applied
- **WHEN** `config.wayland_ui.corner_radius` is R > 0
- **THEN** the background has rounded corners of radius `min(R, width/2, height/2)`

#### Scenario: no corners when radius is zero
- **WHEN** `config.wayland_ui.corner_radius` is 0
- **THEN** the background is a plain rectangle

### Requirement: Text layout and draw via cosmic-text

`TextView` MUST use `cosmic-text::Buffer` with `FontSystem` + `SwashCache` to shape and lay out text, then blit glyphs onto the `tiny-skia` pixmap via a custom `Renderer` that blends straight-alpha swash pixels onto the premultiplied pixmap (parity with `Wayland.zig:48-218`). Font fallback MUST chain `[user_font, "sans:size=14", "mono:size=14"]` via `fontdb` query. Text metrics (width/height) MUST be available for layout-box positioning.

#### Scenario: TextView reports layout metrics
- **WHEN** a `TextView` is created for a string S with the regular font
- **THEN** it exposes `width` and `height` suitable for centering in the surface

#### Scenario: font fallback chain
- **WHEN** `config.wayland_ui.font_regular` is `Some(user_font)`
- **THEN** the font query tries `user_font`, then `sans:size=14`, then `mono:size=14`

### Requirement: Grayscale antialiasing only

The render pipeline MUST disable subpixel antialiasing and force grayscale AA (D7). This avoids chromatic aberration on transparent/dynamic backgrounds.

#### Scenario: subpixel AA disabled
- **WHEN** text is rasterized
- **THEN** the swash rasterizer uses grayscale AA, not subpixel/LCD AA

### Requirement: Bundled fallback font, no system font scan

The render pipeline MUST load a bundled fallback font (DejaVu Sans / Fira Mono) via `fontdb::load_font_data(include_bytes!(...))` and MUST NOT call `fontdb::load_system_fonts()` (D7: startup latency).

#### Scenario: no system font scan at startup
- **WHEN** `Wayland::init` initializes the font system
- **THEN** only the bundled font is loaded; `load_system_fonts` is never called

### Requirement: Cached pin mask glyph

For `GetPin` mode, the pin mask glyph (`•` or `*`) MUST be shaped and rasterized once at init, cached, and blitted iteratively per pin square. The cosmic-text shaper MUST NOT be re-invoked per keystroke (D7).

#### Scenario: pin squares blit cached glyph
- **WHEN** the pin area is drawn with N entered characters
- **THEN** N cached mask-glyph blits are performed, with no cosmic-text shaping call

### Requirement: Premultiplied RGBA to Argb8888 byte swap

Before `wl_surface.commit`, the render pipeline MUST swap the R and B channels of the `tiny-skia` premultiplied RGBA8888 buffer in-place, converting to little-endian `Argb8888` (`[B,G,R,A]`). The swap MUST be correct for all pixels in the buffer (D6).

#### Scenario: byte swap converts RGBA to BGRA
- **WHEN** the render pipeline completes drawing
- **THEN** an in-place R/B swap is applied so the buffer matches `wl_shm::Format::Argb8888` on little-endian

### Requirement: Button hotspots populated on first render

The `Surface` MUST populate its hotspot list on the first render (when `hotspots` is empty), recording the `Effect` (cancel/notok/ok), x, y, width, height of each button region (parity with `Wayland.zig:925-1008`).

#### Scenario: hotspots populated once
- **WHEN** the first `render` call draws the buttons
- **THEN** the hotspot list is populated with one entry per visible button (cancel, notok, ok)

### Requirement: Fractional-scale-aware rendering

When `wp_fractional_scale_v1.preferred_scale` reports a fractional factor F, the render pipeline MUST scale layout dimensions and font metrics by F, set `wl_surface.set_buffer_scale(1)`, and allocate the SHM buffer at the scaled dimensions (D8).

#### Scenario: fractional scale applied
- **WHEN** the compositor advertises a preferred scale of 1.5
- **THEN** the surface dimensions and font metrics are multiplied by 1.5 and `set_buffer_scale(1)` is called
