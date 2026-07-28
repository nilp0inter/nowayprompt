## 1. Scale State Foundation

- [x] 1.1 Replace the fixed integer surface scale with an exact integer/fractional scale representation and checked logical-to-physical dimension helpers.
- [x] 1.2 Add tracked `wl_output` records, entered-output membership, highest-integer-scale selection, and fractional-preferred-scale precedence.

## 2. Wayland Protocol Lifecycle

- [x] 2.1 Bind and remove `wl_output` globals, dispatch output scale events, and release output objects during frontend teardown.
- [x] 2.2 Bind `wp_viewporter` and create/destroy per-surface `wp_viewport` and `wp_fractional_scale_v1` objects only when both managers are available.
- [x] 2.3 Handle `wl_surface.enter`/`leave` and `preferred_scale` events, retaining pre-configure state and rerendering configured surfaces only when the effective scale changes.

## 3. Physical-Pixel Rendering

- [x] 3.1 Keep layout, layer-surface size, and hotspots logical while scaling backgrounds, borders, corners, pin squares, and buttons into a physical-size pixmap.
- [x] 3.2 Rasterize cosmic-text glyphs and cached pin-mask glyphs at the effective physical scale without changing logical text metrics.
- [x] 3.3 Implement integer fallback commits with physical dimensions multiplied by N, viewport destination cleared, and `wl_surface.set_buffer_scale(N)`.
- [x] 3.4 Implement fractional commits with dimensions rounded up from P/120, buffer scale 1, and viewport destination set to the logical surface size.
- [x] 3.5 Harden SHM width, height, stride, byte-size, and Wayland argument conversions against overflow while preserving busy-buffer release and physical-dimension reuse.

## 4. Behavioral Smoke Verification

- [x] 4.1 Run the prompt under headless Sway at 1× and verify unchanged logical frame geometry and successful keyboard interaction.
- [x] 4.2 Change the live headless output to a fractional scale and verify a physical-size rerender, unchanged logical geometry, viewport destination, and continued interaction.

## 5. Post-Smoke Test Coverage

- [x] 5.1 Add deterministic tests for scale rounding, overflow rejection, integer fallback, fractional precedence, multi-output selection, enter/leave, and output removal.
- [x] 5.2 Extend Wayland surface diagnostics and the test binary to report changed logical geometry, exact scale mode/value, physical buffer dimensions, and logical hotspots.
- [x] 5.3 Extend the NixOS Wayland driver with target-only fractional and live scale-change scenarios while retaining the existing 1× target/oracle parity gate.
- [x] 5.4 Run the focused Rust tests and the NixOS Wayland test and resolve any scaling, input, lifecycle, or parity regressions.

## 6. Cleanup

- [x] 6.1 Remove the fixed-scale field, ignored fractional-event path, and obsolete comments after all callers use the new scale model.
- [x] 6.2 Run the repository formatter and lints on the completed change and review the final protocol-object teardown order.
