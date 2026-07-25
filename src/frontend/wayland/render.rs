//! Layer-shell surface + software render pipeline (parity `Wayland.zig:740-1254`).
//!
//! Foundation stub: `Surface` struct + `HotSpot` contract are frozen.
//! The tiny-skia + cosmic-text render pipeline is filled in by the render
//! implementation pass.

use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::QueueHandle;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::frontend::{FrontendError, InterfaceMode};

use super::WaylandState;

/// A clickable region. Parity with `Wayland.zig:24-45`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotSpot {
    pub effect: HotSpotEffect,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotSpotEffect {
    Cancel,
    NotOk,
    Ok,
}

impl HotSpot {
    /// Parity `Wayland.zig:33-36`.
    pub fn contains_point(&self, x: u32, y: u32) -> bool {
        x >= self.x
            && x <= self.x.saturating_add(self.width)
            && y >= self.y
            && y <= self.y.saturating_add(self.height)
    }

    /// Trigger the effect (parity `Wayland.zig:38-44`).
    pub fn act(&self, state: &mut WaylandState) {
        use super::ExitReason;
        let reason = match self.effect {
            HotSpotEffect::Cancel => ExitReason::UserAbort,
            HotSpotEffect::NotOk => ExitReason::UserNotOk,
            HotSpotEffect::Ok => ExitReason::UserOk,
        };
        state.abort(reason);
    }
}

/// The layer-shell surface. Parity with `Wayland.zig:740-1254`.
pub struct Surface {
    pub configured: bool,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub hotspots: Vec<HotSpot>,
}

impl Surface {
    /// Create the layer-shell surface. Parity `Wayland.zig:756-786`.
    ///
    /// Foundation stub: returns a placeholder; the render pass creates the
    /// real `wl_surface` + `zwlr_layer_surface_v1` and wires the Dispatch
    /// impls.
    pub fn new(
        _state: &mut WaylandState,
        _qh: &QueueHandle<WaylandState>,
        _compositor: &WlCompositor,
        _layer_shell: &ZwlrLayerShellV1,
        _shm: &WlShm,
        _fractional: Option<&WpFractionalScaleManagerV1>,
        _mode: InterfaceMode,
    ) -> Result<Self, FrontendError> {
        Ok(Self {
            configured: false,
            width: 0,
            height: 0,
            scale: 1,
            hotspots: Vec::new(),
        })
    }

    pub fn deinit(self) {
        // Foundation stub: nothing to tear down yet.
    }

    /// Find the hotspot containing `(x, y)`. Parity `Wayland.zig:857-864`.
    pub fn hotspot_from_point(&self, x: u32, y: u32) -> Option<&HotSpot> {
        self.hotspots.iter().find(|hs| hs.contains_point(x, y))
    }

    /// Render the surface (parity `Wayland.zig:887-1036`).
    ///
    /// Foundation stub: returns Ok(()); the render pass implements the
    /// tiny-skia + cosmic-text pipeline.
    pub fn render(
        &mut self,
        _state: &mut WaylandState,
        _qh: &QueueHandle<WaylandState>,
    ) -> Result<(), FrontendError> {
        Ok(())
    }
}

/// Convenience function for input handlers: takes the surface out of state,
/// renders it, puts it back, and aborts on error (parity with legacy pattern
/// where input calls `self.w.surface.?.render() catch self.w.abort(...)`).
pub fn render_surface(state: &mut WaylandState, qh: &QueueHandle<WaylandState>) {
    use super::ExitReason;
    if let Some(mut surface) = state.surface.take() {
        let result = surface.render(state, qh);
        state.surface = Some(surface);
        if let Err(e) = result {
            state.abort(ExitReason::Error(e));
        }
    }
}
