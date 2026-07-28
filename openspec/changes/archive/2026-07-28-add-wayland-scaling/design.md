## Context

The current renderer shapes and measures its UI in logical pixels, allocates an
SHM buffer at those same dimensions, rasterizes glyphs with a scale of 1, and
always calls `wl_surface.set_buffer_scale(1)`. `Surface.scale` is fixed at 1.
The registry already binds `wp_fractional_scale_manager_v1` and creates a
per-surface fractional-scale object, but its `preferred_scale` event is ignored.
No `wl_output` or `wp_viewporter` objects are bound.

This preserves parity with wayprompt v0.1.2, which also has a fixed scale of 1,
but forces compositors to enlarge a low-resolution buffer on scaled outputs.
The existing `wayland-render` requirement attempted to address fractional
scaling by enlarging the buffer while retaining buffer scale 1. That model is
incomplete: without `wp_viewport.set_destination`, the compositor interprets
the enlarged buffer dimensions as the surface's logical size.

The frontend has one layer-shell surface, a small triple-buffer SHM pool, a
single-threaded Wayland dispatch loop, logical pointer/touch coordinates, and
compositor-managed cursor shapes. The design must retain those properties and
must not introduce a configuration or dependency merely to express compositor
state.

## Goals / Non-Goals

**Goals:**

- Produce crisp buffers at compositor-selected integer and fractional scales.
- Preserve all layout, configured dimensions, surface size, and hit testing in
  logical pixels.
- Prefer the fractional-scale protocol when it can be used correctly and fall
  back to core output scale otherwise.
- Handle initial event ordering, live output scale changes, output migration,
  and multiple entered outputs.
- Keep 1× behavior and the existing unscaled oracle comparison unchanged.
- Reject unrepresentable physical dimensions before allocation or protocol
  requests.

**Non-Goals:**

- A manual scale setting, UI zoom control, or environment-variable override.
- Changing the meaning or units of existing configuration fields.
- Matching wayprompt's pixel output on scaled displays.
- Client-rendered cursor scaling; cursor size remains compositor-managed.
- Scaling changes to the TTY frontend or other non-Wayland interfaces.
- Supporting obsolete compositors that provide neither `wl_output.scale` nor a
  usable fractional-scale/viewporter pair beyond the safe 1× default.

## Decisions

### Represent scale exactly and distinguish protocol modes

Introduce an internal scale value that distinguishes:

- integer mode with a positive integer N; and
- fractional mode with the protocol numerator P over the fixed denominator 120.

Scale state and physical-dimension calculations use integers. Conversion to
`f32` occurs only at the tiny-skia/cosmic-text drawing boundary. A physical
dimension is calculated with checked arithmetic as
`ceil(logical * numerator / denominator)`.

This avoids floating-point equality and rounding errors in buffer reuse and
change detection. Storing only `f32` was rejected because protocol values are
exact rationals and physical dimensions are integer allocation keys.

### Bind outputs and viewporter without adding dependencies

Bind each advertised `wl_output`, storing its registry name, proxy identity, and
latest positive integer scale. Bind `wp_viewporter` when advertised. The
existing `wayland-protocols` dependency already includes stable viewporter
client bindings, so no dependency or feature change is needed.

Create `wp_fractional_scale_v1` and `wp_viewport` objects for a surface only
when both managers are available. Destroy both before destroying the
`wl_surface`. Track output globals by registry name so `global_remove` can
remove their state and surface membership. Bind an output protocol version that
supports `scale` and orderly release.

Treating fractional-scale without viewporter as usable was rejected: a physical
buffer submitted with buffer scale 1 would alter logical surface size. Making
either optional protocol a startup requirement was rejected because integer
scaling is a complete fallback.

### Keep one logical layout and render it into a physical buffer

`calculate_size`, text shaping metrics, `zwlr_layer_surface_v1.set_size`, UI
configuration, and hotspots remain logical. A render derives physical buffer
width and height from the logical dimensions and effective scale. Geometry
coordinates, border widths, corner radii, and pin squares are multiplied only
while drawing. Glyphs are shaped with logical metrics and rasterized with the
effective physical scale so the compositor never enlarges a 1× glyph image.

The previous requirement to multiply layout and font metrics was rejected
because it changes the surface's logical dimensions and interaction geometry.
Maintaining separate layouts per scale was rejected because scale affects
raster density, not the requested logical layout.

### Use viewporter for fractional scaling

For fractional scale `F = P/120`:

1. allocate a `ceil(W*F)` by `ceil(H*F)` SHM buffer;
2. render the logical scene and glyphs at F into that buffer;
3. keep `wl_surface.set_buffer_scale(1)`;
4. set `wp_viewport` destination to logical `(W, H)`;
5. damage the complete physical buffer, attach, and commit.

The viewport source remains unset, so the complete attached buffer is mapped to
the logical destination. The destination is set before each fractional commit;
this keeps commits self-consistent after a scale transition.

Using `ceil(F)` as `wl_surface` buffer scale was rejected because it cannot
represent the compositor's fractional preference. Enlarging the buffer without
a viewport was rejected because it enlarges the logical surface.

### Use core integer buffer scale as fallback

Until a fractional preferred scale is received, or whenever fractional scaling
cannot be enabled, select integer scale N from entered outputs. Render an
exactly `(W*N)` by `(H*N)` buffer, call `set_buffer_scale(N)`, and do not set a
viewport destination. If a viewport object exists but fractional mode has not
become active, clear any previously set destination before an integer commit.
The default is N=1 when no entered output has reported a positive scale.

Fractional-only support was rejected because compositors are permitted not to
advertise the staging protocol. Integer-only support was rejected because it
still leaves fractional-output rendering to compositor resampling.

### Derive effective scale from surface and output events

Handle `wl_surface.enter` and `leave` and associate entered output proxies with
the tracked output records. In integer mode, choose the highest scale among all
entered outputs, matching the standard strategy for a surface spanning outputs.
Recompute after enter, leave, output `scale`, and output removal events.

A valid `wp_fractional_scale_v1.preferred_scale(P)` event takes precedence over
the integer result for that surface. Store scale events received before the
first layer-surface configure; the first configured render uses the latest
state. After configuration, rerender only when the effective scale value or
mode changes. Existing logical hotspots remain valid and need not be rebuilt
solely because of scale.

Choosing the first entered output was rejected because event order is not an
output-priority policy. Averaging output scales was rejected because it yields
a value requested by no output and can reduce sharpness on the higher-density
output.

### Make scaled allocation and buffer reuse explicit

The buffer pool continues to key idle buffers by buffer width and height, but
those dimensions are now explicitly physical. A scale transition acquires a
matching physical buffer. Busy buffers at the old size remain alive until
`wl_buffer.release`; existing pool culling reclaims idle excess buffers.

Before `memfd_create`, validate checked physical dimensions, four-byte stride,
total byte size, conversions to Wayland's signed request arguments, and host
`usize`. Failure returns `FrontendError` without creating or attaching a
partial buffer.

Eagerly destroying old busy buffers was rejected because the compositor may
still read them. Maintaining separate permanent pools per scale was rejected
because scale changes are rare and the existing bounded pool already handles
dimension changes.

### Expose deterministic render diagnostics to the test binary

Evolve the existing test introspection so the Wayland test binary can report,
after every changed render state rather than only the first render:

- logical width and height;
- exact effective scale and integer/fractional mode;
- physical buffer width and height;
- logical hotspot geometry.

Keep the production prompt free of diagnostic output. Pure tests cover rational
rounding, overflow, mode precedence, multi-output selection, enter/leave,
output removal, and the integer fallback. The headless Sway NixOS gate retains
its 1× target/oracle checks, then runs a target-only fractional scale and live
scale-change scenario. It observes explicit configure/render diagnostics and
Wayland requests rather than sleeps. Keyboard interaction is exercised after
the scaled rerender.

A scaled target/oracle screenshot comparison was rejected because the oracle's
fixed 1× buffer is the defect being removed. Requiring the chosen Sway instance
to omit fractional-scale for an integer smoke test was rejected because its
advertised global set is not configurable; the integer branch instead receives
deterministic state/render coverage without a test-only production override.

## Risks / Trade-offs

- **Fractional buffer dimensions require rounding** → Use exact P/120 arithmetic,
  round each physical extent upward, and map the whole buffer to the exact
  logical viewport destination.
- **Several scale events can cause redundant redraws** → Compare the complete
  effective mode/value before rendering; scale changes are rare, so no separate
  scheduler or debounce is introduced.
- **A compositor can send events before configure** → Store state immediately
  but render only after the surface is configured.
- **A scale change can temporarily increase live SHM memory** → Retain busy old
  buffers for protocol correctness and rely on the existing bounded idle-pool
  culling after release.
- **Physical size arithmetic can exceed protocol or address-space limits** →
  Validate every multiplication, ceiling division, signed conversion, and byte
  size before allocation.
- **Fractional protocol availability varies by compositor** → Require the
  manager and viewporter as a pair and retain core integer scaling plus a 1×
  default.
- **Headless compositor rendering can vary across versions** → Assert protocol
  state, dimensions, geometry invariance, and interactivity rather than brittle
  pixel identity.

## Migration Plan

1. Add scale/output state and protocol bindings while retaining effective scale
   1 until the renderer consumes the new state.
2. Separate logical and physical render dimensions and harden SHM arithmetic.
3. Enable integer commits, then fractional viewport commits and dynamic
   rerendering.
4. Extend deterministic tests and the compositor gate; retain the existing 1×
   parity scenario unchanged.
5. Remove the fixed-scale field and ignored-event comment once every caller uses
   the new scale representation.

There is no persisted data or configuration migration. Rollback is a source
revert; 1× behavior remains the compatibility baseline throughout the change.

## Open Questions

None.
