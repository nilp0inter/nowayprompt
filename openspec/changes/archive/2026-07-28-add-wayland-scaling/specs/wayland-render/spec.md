## MODIFIED Requirements

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

## ADDED Requirements

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
