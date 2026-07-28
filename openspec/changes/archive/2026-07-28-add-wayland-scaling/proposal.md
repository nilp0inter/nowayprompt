## Why

nowayprompt inherited wayprompt's fixed 1× Wayland rendering model, so scaled
outputs receive a logical-resolution SHM buffer that the compositor must
upscale, producing blurry text and controls. The existing fractional-scale
requirement is also incomplete because it omits `wp_viewporter`, without which
a scaled buffer changes the surface's logical size instead of its pixel density.

## What Changes

- Automatically render at the compositor's preferred fractional scale when
  `wp_fractional_scale_v1` and `wp_viewporter` are available.
- Fall back to `wl_output.scale` integer scaling when fractional scaling is not
  available.
- Keep configured dimensions, layout, surface geometry, pointer coordinates,
  and button hotspots in logical pixels while allocating and drawing SHM
  buffers at physical-pixel dimensions.
- Track surface output entry and exit, select the highest entered-output scale,
  and rerender when the effective scale changes.
- Preserve 1× rendering behavior and unscaled wayprompt parity.
- Extend the automated Wayland gate with target-specific scaled rendering and
  dynamic scale-change scenarios, plus deterministic integer-fallback coverage;
  scaled output is not compared with the legacy oracle's blurry rendering.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `wayland-frontend`: Bind and manage output, fractional-scale, and viewporter
  protocol objects and derive the effective scale across surface lifecycle
  events.
- `wayland-render`: Separate logical layout from physical buffer rendering and
  apply integer or fractional scale without changing logical geometry or input
  coordinates.
- `wayland-parity-testing`: Retain legacy parity at 1× and add target assertions
  for correctly sized scaled rendering, integer fallback, and dynamic scale
  changes.

## Impact

Affected code is concentrated in `src/frontend/wayland/mod.rs`,
`src/frontend/wayland/render.rs`, and `src/frontend/wayland/shm.rs`, plus the
Wayland test binary and NixOS compositor driver. The existing
`wayland-protocols` dependency already exposes stable `wp_viewporter`; no new
runtime dependency or public configuration option is required. This change is
not an API or configuration-file break.
