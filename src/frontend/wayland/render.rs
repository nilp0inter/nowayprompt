//! Layer-shell surface + software render pipeline.
//!
//! Renders the prompt into `wl_shm` Argb8888 buffers with tiny-skia; text
//! is shaped once per surface with cosmic-text (bundled DejaVu faces)
//! and rasterized per render through swash. Each [`Surface::render`]
//! composites background, labels, pin area and buttons, converts the
//! premultiplied RGBA pixmap to Argb8888 byte order (`swap_rb`) and
//! attaches the buffer to the layer-shell surface.

use std::sync::Arc;

use super::scale::{self, Scale};
use super::WaylandState;
use crate::config::{Colour, WaylandColours, WaylandUi};
use crate::frontend::{FrontendError, InterfaceMode};
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, SwashImage,
    Weight,
};
use tiny_skia::{
    BlendMode, Color as SkColor, FillRule, Paint, Path, PathBuilder, PixmapMut,
    PremultipliedColorU8, Rect, Shader, Transform,
};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

// --- Fonts -------------------------------------------------------------------

/// Bundled fallback faces: DejaVu Sans regular + bold. `new_with_fonts`
/// loads them around the system font set (cosmic-text 0.19 scans system
/// fonts unconditionally), so configured family names resolve against
/// system fonts with the bundled faces as a guaranteed last resort.
const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../assets/DejaVuSans-Bold.ttf");

/// Pixel sizes used when a font description carries no `size=` attribute:
/// the wayprompt(5) defaults `sans:size=14` / `sans:size=20`.
const FONT_SIZE_REGULAR: f32 = 14.0;
const FONT_SIZE_LARGE: f32 = 20.0;

fn new_font_system() -> FontSystem {
    FontSystem::new_with_fonts([
        fontdb::Source::Binary(Arc::new(FONT_REGULAR.to_vec())),
        fontdb::Source::Binary(Arc::new(FONT_BOLD.to_vec())),
    ])
}

/// The font family a description selects: a fontconfig generic alias or a
/// concrete family name matched against the font database.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FamilySpec {
    SansSerif,
    Serif,
    Monospace,
    Name(String),
}

impl FamilySpec {
    fn as_family(&self) -> Family<'_> {
        match self {
            Self::SansSerif => Family::SansSerif,
            Self::Serif => Family::Serif,
            Self::Monospace => Family::Monospace,
            Self::Name(name) => Family::Name(name),
        }
    }
}

/// A parsed wayprompt(5) font description (`font-regular` / `font-large`
/// value): an fcft/fontconfig-style pattern `family[:attr=value...]`.
/// Only the `size` attribute (pixels) is interpreted; unknown attributes
/// are ignored. Glyph coverage beyond the selected family is left to
/// cosmic-text's own fallback over the shared font database.
#[derive(Debug, Clone, PartialEq)]
struct FontDescription {
    family: FamilySpec,
    size: f32,
}

impl FontDescription {
    /// The `[general] font-regular` description (default size 14).
    fn regular(description: Option<&str>) -> Self {
        Self::parse(description, FONT_SIZE_REGULAR)
    }

    /// The `[general] font-large` description (default size 20).
    fn large(description: Option<&str>) -> Self {
        Self::parse(description, FONT_SIZE_LARGE)
    }

    fn parse(description: Option<&str>, default_size: f32) -> Self {
        let mut family = FamilySpec::SansSerif;
        let mut size = default_size;
        if let Some(description) = description {
            let mut components = description.split(':');
            // The first component is the family; an empty family or a
            // generic alias keeps the sans-serif default (fcft
            // default-family behaviour).
            match components.next().unwrap_or_default().trim() {
                "" | "sans" | "sans-serif" | "Sans" | "Sans-Serif" => {}
                "serif" | "Serif" => family = FamilySpec::Serif,
                "mono" | "monospace" | "Mono" | "Monospace" => {
                    family = FamilySpec::Monospace;
                }
                name => family = FamilySpec::Name(name.to_string()),
            }
            for attr in components {
                let Some((key, value)) = attr.split_once('=') else {
                    continue;
                };
                if !key.trim().eq_ignore_ascii_case("size") {
                    continue;
                }
                if let Ok(px) = value.trim().parse::<f32>() {
                    if px.is_finite() && px > 0.0 {
                        size = px;
                    }
                }
            }
        }
        Self { family, size }
    }
}

// --- HotSpots ----------------------------------------------------------------

/// A clickable region.
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
    /// Whether the point `(x, y)` is inside this hotspot.
    pub fn contains_point(&self, x: u32, y: u32) -> bool {
        x >= self.x
            && x <= self.x.saturating_add(self.width)
            && y >= self.y
            && y <= self.y.saturating_add(self.height)
    }

    /// Trigger the effect.
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

// --- Text views --------------------------------------------------------------

/// One shaped text label.
struct TextView {
    buffer: Buffer,
    width: u32,
    height: u32,
}

impl TextView {
    /// Shape `text` and measure it.
    ///
    /// Family and pixel size come from the font description: `font-regular`
    /// shapes description, error message and buttons; `font-large` shapes
    /// title and prompt, which are additionally bold. Glyphs the selected
    /// family does not cover fall back through cosmic-text over the shared
    /// system + bundled font database. The line height mirrors fcft's
    /// `font.height` for the bundled DejaVu metrics:
    /// `ceil((ascent + descent) * size / em) ≈ size * 1.2`.
    fn new(
        font_system: &mut FontSystem,
        text: &str,
        font: &FontDescription,
        weight: Weight,
    ) -> Self {
        let mut buffer = Buffer::new(
            font_system,
            Metrics::new(font.size, (font.size * 1.2).round()),
        );
        buffer.set_size(None, None);
        let attrs = Attrs::new().family(font.family.as_family()).weight(weight);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);

        let mut width = 0.0f32;
        let mut height = 0.0f32;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height += run.line_height;
        }
        Self {
            buffer,
            width: width.ceil() as u32,
            height: height.round() as u32,
        }
    }

    /// Rasterize the label at logical `(x, y)` onto a physical pixmap at
    /// `scale`. Glyphs are shaped with logical metrics and rasterized at
    /// the effective physical scale so the compositor never enlarges a 1×
    /// glyph image.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        pixmap: &mut PixmapMut<'_>,
        swash_cache: &mut SwashCache,
        font_system: &mut FontSystem,
        colour: PremultipliedColorU8,
        x: u32,
        y: u32,
        scale: f32,
    ) {
        let pixmap_w = pixmap.width();
        let pixmap_h = pixmap.height();
        let data = pixmap.data_mut();
        let ox = x as f32 * scale;
        for run in self.buffer.layout_runs() {
            let oy = y as f32 * scale + run.line_y * scale;
            for glyph in run.glyphs {
                let physical = glyph.physical((ox, oy), scale);
                let Some(image) = swash_cache.get_image_uncached(font_system, physical.cache_key)
                else {
                    continue;
                };
                let gx = physical.x + image.placement.left;
                let gy = physical.y - image.placement.top;
                blend_glyph(data, pixmap_w, pixmap_h, &image, gx, gy, colour);
            }
        }
    }
}

/// Composite one rasterized glyph image onto premultiplied RGBA8888 `data`.
fn blend_glyph(
    data: &mut [u8],
    pixmap_w: u32,
    pixmap_h: u32,
    image: &SwashImage,
    gx: i32,
    gy: i32,
    colour: PremultipliedColorU8,
) {
    let iw = image.placement.width as usize;
    let ih = image.placement.height as usize;
    if iw == 0 || ih == 0 {
        return;
    }
    match image.content {
        // 1 byte/pixel coverage mask: `out = colour*cov + dst*(1-cov)`.
        SwashContent::Mask | SwashContent::SubpixelMask => {
            let (cr, cg, cb, ca) = (
                u32::from(colour.red()),
                u32::from(colour.green()),
                u32::from(colour.blue()),
                u32::from(colour.alpha()),
            );
            let bytes_per_pixel = match image.content {
                SwashContent::SubpixelMask => 3,
                _ => 1,
            };
            for py in 0..ih {
                let dy = gy + py as i32;
                if dy < 0 || dy >= pixmap_h as i32 {
                    continue;
                }
                let row = py * iw * bytes_per_pixel;
                for px in 0..iw {
                    let dx = gx + px as i32;
                    if dx < 0 || dx >= pixmap_w as i32 {
                        continue;
                    }
                    let i = row + px * bytes_per_pixel;
                    let cov = image.data[i..i + bytes_per_pixel]
                        .iter()
                        .copied()
                        .max()
                        .unwrap_or(0) as u32;
                    if cov == 0 {
                        continue;
                    }
                    let idx = ((dy as u32 * pixmap_w + dx as u32) * 4) as usize;
                    let d = &mut data[idx..idx + 4];
                    let inv = 255 - cov;
                    d[0] = ((cr * cov + u32::from(d[0]) * inv) / 255) as u8;
                    d[1] = ((cg * cov + u32::from(d[1]) * inv) / 255) as u8;
                    d[2] = ((cb * cov + u32::from(d[2]) * inv) / 255) as u8;
                    d[3] = ((ca * cov + u32::from(d[3]) * inv) / 255) as u8;
                }
            }
        }
        // 4 bytes/pixel premultiplied RGBA (colour glyphs): straight `over`.
        SwashContent::Color => {
            for py in 0..ih {
                let dy = gy + py as i32;
                if dy < 0 || dy >= pixmap_h as i32 {
                    continue;
                }
                let row = py * iw * 4;
                for px in 0..iw {
                    let dx = gx + px as i32;
                    if dx < 0 || dx >= pixmap_w as i32 {
                        continue;
                    }
                    let i = row + px * 4;
                    let sa = u32::from(image.data[i + 3]);
                    if sa == 0 {
                        continue;
                    }
                    let idx = ((dy as u32 * pixmap_w + dx as u32) * 4) as usize;
                    let d = &mut data[idx..idx + 4];
                    let inv = 255 - sa;
                    d[0] = (u32::from(image.data[i]) + u32::from(d[0]) * inv / 255) as u8;
                    d[1] = (u32::from(image.data[i + 1]) + u32::from(d[1]) * inv / 255) as u8;
                    d[2] = (u32::from(image.data[i + 2]) + u32::from(d[2]) * inv / 255) as u8;
                    d[3] = (sa + u32::from(d[3]) * inv / 255) as u8;
                }
            }
        }
    }
}

/// Shape one trimmed label into a [`TextView`] using the given font
/// description and weight. `None` → no view; a label that is empty after
/// trimming errors (matching fcft's `error.EmptyString`).
fn make_view(
    font_system: &mut FontSystem,
    label: Option<&str>,
    font: &FontDescription,
    weight: Weight,
) -> Result<Option<TextView>, FrontendError> {
    let Some(text) = label else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(FrontendError::Init("empty text label".into()));
    }
    Ok(Some(TextView::new(font_system, trimmed, font, weight)))
}

// --- Colour + drawing helpers ------------------------------------------------

/// Convert a 16-bit premultiplied config colour to straight-alpha 8-bit
/// for tiny-skia `Paint` (which premultiplies internally).
fn to_sk_color(c: Colour) -> SkColor {
    if c.alpha == 0 {
        return SkColor::TRANSPARENT;
    }
    let straight = |v: u16| ((u32::from(v) * 255) / u32::from(c.alpha)).min(255) as u8;
    SkColor::from_rgba8(
        straight(c.red),
        straight(c.green),
        straight(c.blue),
        (c.alpha >> 8) as u8,
    )
}

/// Convert a 16-bit premultiplied config colour to 8-bit premultiplied
/// (for glyph blending onto the premultiplied pixmap).
fn to_premul8(c: Colour) -> PremultipliedColorU8 {
    PremultipliedColorU8::from_rgba(
        (c.red >> 8) as u8,
        (c.green >> 8) as u8,
        (c.blue >> 8) as u8,
        (c.alpha >> 8) as u8,
    )
    .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// Bordered rectangle: fill the interior, then the four border strips,
/// un-antialiased like pixman `fillRectangles`. Coordinates are scaled by
/// `scale` (the effective physical scale factor).
#[allow(clippy::too_many_arguments)]
fn bordered_rectangle(
    pixmap: &mut PixmapMut<'_>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    border: u32,
    scale: f32,
    background: Colour,
    border_colour: Colour,
) {
    let s = scale;
    let (x, y) = (x as f32 * s, y as f32 * s);
    let (w, h) = (width as f32 * s, height as f32 * s);
    let b = border as f32 * s;

    let mut paint = Paint {
        anti_alias: false,
        shader: Shader::SolidColor(to_sk_color(background)),
        ..Default::default()
    };
    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    paint.shader = Shader::SolidColor(to_sk_color(border_colour));
    let side = (h - 2.0 * b).max(0.0);
    let strips = [
        Rect::from_xywh(x, y, w, b),                // Top
        Rect::from_xywh(x, y + h - b, w, b),        // Bottom
        Rect::from_xywh(x, y + b, b, side),         // Left
        Rect::from_xywh(x + w - b, y + b, b, side), // Right
    ];
    for strip in strips.into_iter().flatten() {
        pixmap.fill_rect(strip, &paint, Transform::identity(), None);
    }
}

/// Draw a rounded-corner background with a constant-width border ring.
/// The border fills the outer rounded rect first. The interior is then
/// written with [`BlendMode::Source`], so a translucent background
/// replaces the border below it instead of blending with it. This keeps
/// the outer border rasterization identical to the legacy-compatible
/// path while preserving the background's configured premultiplied alpha.
fn draw_rounded_background(
    pixmap: &mut PixmapMut<'_>,
    w: f32,
    h: f32,
    r: f32,
    b: f32,
    background: Colour,
    border: Colour,
) {
    let Some(outer) = rounded_rect_path(0.0, 0.0, w, h, r) else {
        return;
    };
    let border_paint = Paint {
        shader: Shader::SolidColor(to_sk_color(border)),
        ..Default::default()
    };
    pixmap.fill_path(
        &outer,
        &border_paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    let iw = w - 2.0 * b;
    let ih = h - 2.0 * b;
    if iw <= 0.0 || ih <= 0.0 {
        return;
    }
    let Some(inner) = rounded_rect_path(b, b, iw, ih, (r - b).max(0.0)) else {
        return;
    };
    let background_paint = Paint {
        shader: Shader::SolidColor(to_sk_color(background)),
        blend_mode: BlendMode::Source,
        ..Default::default()
    };
    pixmap.fill_path(
        &inner,
        &background_paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Rounded-rectangle path (arcs approximated with quadratics).
fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

// --- Surface -----------------------------------------------------------------

/// The layer-shell surface.
pub struct Surface {
    pub configured: bool,
    pub width: u32,
    pub height: u32,
    /// Effective scale, recomputed on output enter/leave, output scale,
    /// and `preferred_scale` events.
    pub scale: Scale,
    pub hotspots: Vec<HotSpot>,

    // Render-owned protocol objects.
    wl_surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    fractional_scale: Option<WpFractionalScaleV1>,
    /// `wp_viewport`, present only when both `wp_fractional_scale_manager_v1`
    /// and `wp_viewporter` are bound. Drives fractional commits; cleared of
    /// any destination during integer commits.
    viewport: Option<WpViewport>,

    // Output membership: registry names of `wl_output` globals the surface
    // has entered (highest positive integer scale among these is the integer
    // fallback).
    entered_outputs: Vec<u32>,
    /// Latest `wp_fractional_scale_v1.preferred_scale` numerator, when
    /// fractional scaling is enabled. `Some` takes precedence over the
    /// integer result.
    fractional_preferred: Option<u32>,

    // Text shaping/rasterization state (cosmic-text + swash).
    font_system: FontSystem,
    swash_cache: SwashCache,

    // Shaped labels.
    title: Option<TextView>,
    description: Option<TextView>,
    prompt: Option<TextView>,
    errmessage: Option<TextView>,
    ok: Option<TextView>,
    notok: Option<TextView>,
    cancel: Option<TextView>,

    mode: InterfaceMode,
    // Config snapshots taken at creation (labels are shaped from these).
    ui: WaylandUi,
    colours: WaylandColours,
}

impl Surface {
    /// Create the layer-shell surface and shape its text views.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: &mut WaylandState,
        qh: &QueueHandle<WaylandState>,
        compositor: &WlCompositor,
        layer_shell: &ZwlrLayerShellV1,
        _shm: &WlShm,
        fractional: Option<&WpFractionalScaleManagerV1>,
        viewporter: Option<&WpViewporter>,
        mode: InterfaceMode,
    ) -> Result<Self, FrontendError> {
        let wl_surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &wl_surface,
            None,
            Layer::Overlay,
            "nowayprompt".to_string(),
            qh,
            (),
        );
        // Fractional scaling requires both the fractional-scale manager and
        // the viewporter: without a viewport, a scaled buffer submitted at
        // buffer scale 1 would alter the surface's logical size. Create the
        // pair only when both managers are available; otherwise integer
        // output scaling is the complete fallback.
        let (fractional_scale, viewport) = match (fractional, viewporter) {
            (Some(m), Some(v)) => (
                Some(m.get_fractional_scale(&wl_surface, qh, ())),
                Some(v.get_viewport(&wl_surface, qh, ())),
            ),
            _ => (None, None),
        };

        let ui = state.config().wayland_ui.clone();
        let regular_font = FontDescription::regular(ui.font_regular.as_deref());
        let large_font = FontDescription::large(ui.font_large.as_deref());
        let mut font_system = new_font_system();
        let swash_cache = SwashCache::new();

        // Title/prompt use the large bold font, everything else regular.
        let labels = &state.config().labels;
        let title = make_view(
            &mut font_system,
            labels.title.as_deref(),
            &large_font,
            Weight::BOLD,
        )?;
        let description = make_view(
            &mut font_system,
            labels.description.as_deref(),
            &regular_font,
            Weight::NORMAL,
        )?;
        let errmessage = make_view(
            &mut font_system,
            labels.err_message.as_deref(),
            &regular_font,
            Weight::NORMAL,
        )?;
        let prompt = make_view(
            &mut font_system,
            labels.prompt.as_deref(),
            &large_font,
            Weight::BOLD,
        )?;
        let ok = make_view(
            &mut font_system,
            labels.ok.as_deref(),
            &regular_font,
            Weight::NORMAL,
        )?;
        let notok = make_view(
            &mut font_system,
            labels.not_ok.as_deref(),
            &regular_font,
            Weight::NORMAL,
        )?;
        let cancel = make_view(
            &mut font_system,
            labels.cancel.as_deref(),
            &regular_font,
            Weight::NORMAL,
        )?;

        let mut surface = Self {
            configured: false,
            width: 0,
            height: 0,
            scale: Scale::ONE,
            hotspots: Vec::new(),
            wl_surface: Some(wl_surface),
            layer_surface: Some(layer_surface),
            fractional_scale,
            viewport,
            entered_outputs: Vec::new(),
            fractional_preferred: None,
            font_system,
            swash_cache,
            title,
            description,
            prompt,
            errmessage,
            ok,
            notok,
            cancel,
            mode,
            ui,
            colours: state.config().wayland_colours.clone(),
        };
        surface.calculate_size();

        // Event listeners are provided by the `Dispatch` impls below.
        let layer_surface = surface
            .layer_surface
            .as_ref()
            .ok_or_else(|| FrontendError::Init("no layer surface".into()))?;
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer_surface.set_size(surface.width, surface.height);
        surface
            .wl_surface
            .as_ref()
            .ok_or_else(|| FrontendError::Init("no wl_surface".into()))?
            .commit();
        Ok(surface)
    }

    /// Release owned proxies. There is no circle mask to free: rounded
    /// corners are drawn per-render with paths.
    pub fn deinit(self) {
        if let Some(fs) = self.fractional_scale {
            fs.destroy();
        }
        if let Some(vp) = self.viewport {
            vp.destroy();
        }
        if let Some(ls) = self.layer_surface {
            ls.destroy();
        }
        if let Some(ws) = self.wl_surface {
            ws.destroy();
        }
    }

    /// Find the hotspot containing `(x, y)`.
    pub fn hotspot_from_point(&self, x: u32, y: u32) -> Option<&HotSpot> {
        self.hotspots.iter().find(|hs| hs.contains_point(x, y))
    }

    /// Whether fractional scaling is enabled (both the fractional-scale
    /// object and a viewport exist).
    fn fractional_enabled(&self) -> bool {
        self.fractional_scale.is_some() && self.viewport.is_some()
    }

    /// Compute the effective scale from the surface's entered outputs and
    /// the latest fractional preferred scale. A fractional preferred scale
    /// takes precedence when fractional scaling is enabled; otherwise the
    /// highest positive integer scale among entered outputs is used,
    /// defaulting to 1×.
    fn effective_scale(&self, state: &WaylandState) -> Scale {
        scale::compute_effective_scale(
            self.fractional_enabled(),
            self.fractional_preferred,
            self.entered_outputs
                .iter()
                .filter_map(|name| state.output_by_name(*name))
                .map(|o| o.scale),
        )
    }

    /// Recompute the effective scale; return `Some(new)` if it changed,
    /// `None` otherwise. The caller schedules a render only on a real
    /// (mode + value) change so transient events do not redraw.
    pub(crate) fn recompute_scale(&mut self, state: &WaylandState) -> Option<Scale> {
        let new = self.effective_scale(state);
        if new == self.scale {
            None
        } else {
            self.scale = new;
            Some(new)
        }
    }

    /// Record that the surface entered `output`. Returns the new effective
    /// scale when it changed. Idempotent: a repeated enter for an already
    /// tracked output is a no-op.
    pub fn handle_enter(&mut self, state: &WaylandState, output: &WlOutput) -> Option<Scale> {
        let name = state.output_name_of(output)?;
        if self.entered_outputs.contains(&name) {
            return None;
        }
        self.entered_outputs.push(name);
        self.recompute_scale(state)
    }

    /// Record that the surface left `output`. Returns the new effective
    /// scale when it changed.
    pub fn handle_leave(&mut self, state: &WaylandState, output: &WlOutput) -> Option<Scale> {
        let name = state.output_name_of(output)?;
        self.entered_outputs.retain(|n| *n != name);
        self.recompute_scale(state)
    }

    /// Record a `preferred_scale(P)` event. Only meaningful when fractional
    /// scaling is enabled; ignored otherwise. Returns the new effective
    /// scale when it changed.
    pub fn handle_preferred_scale(&mut self, state: &WaylandState, scale: u32) -> Option<Scale> {
        if !self.fractional_enabled() {
            return None;
        }
        if self.fractional_preferred == Some(scale) {
            return None;
        }
        self.fractional_preferred = Some(scale);
        self.recompute_scale(state)
    }

    /// Drop membership in an output that was removed globally, and
    /// recompute the effective scale. Returns the new effective scale when
    /// it changed.
    pub fn handle_output_removed(&mut self, state: &WaylandState, name: u32) -> Option<Scale> {
        self.entered_outputs.retain(|n| *n != name);
        self.recompute_scale(state)
    }

    /// Compute the surface dimensions from the shaped labels and the UI
    /// metrics.
    fn calculate_size(&mut self) {
        debug_assert!(self.hotspots.is_empty());
        let vp = u32::from(self.ui.vertical_padding);
        let hp = u32::from(self.ui.horizontal_padding);
        let bip = u32::from(self.ui.button_inner_padding);

        self.height = vp;
        self.width = hp;

        if self.mode == InterfaceMode::GetPin {
            if let Some(prompt) = &self.prompt {
                self.width = self.width.max(prompt.width + 2 * hp);
                self.height += prompt.height + vp;
            }

            let sqs = u32::from(self.ui.pin_square_size);
            let square_padding = sqs / 2;
            let pinarea_height = sqs + 2 * square_padding;
            let pinarea_width =
                u32::from(self.ui.pin_square_amount) * (sqs + square_padding) + square_padding;

            self.height += pinarea_height + vp;
            self.width = self.width.max(pinarea_width + 2 * hp);
        }

        if let Some(title) = &self.title {
            self.width = self.width.max(title.width + 2 * hp);
            self.height += title.height + vp;
        }
        if let Some(description) = &self.description {
            self.width = self.width.max(description.width + 2 * hp);
            self.height += description.height + vp;
        }
        if let Some(errmessage) = &self.errmessage {
            self.width = self.width.max(errmessage.width + 2 * hp);
            self.height += errmessage.height + vp;
        }

        let mut combined_button_length = 0u32;
        let mut max_button_height = 0u32;
        for tv in [self.ok.as_ref(), self.notok.as_ref(), self.cancel.as_ref()]
            .into_iter()
            .flatten()
        {
            combined_button_length += tv.width + hp + 2 * bip;
            max_button_height = max_button_height.max(tv.height + 2 * bip);
        }

        self.width = self.width.max(combined_button_length + hp);
        if max_button_height > 0 {
            self.height += max_button_height + vp;
        }
    }

    /// Render the surface.
    pub fn render(
        &mut self,
        state: &mut WaylandState,
        qh: &QueueHandle<WaylandState>,
    ) -> Result<(), FrontendError> {
        if !self.configured {
            return Ok(());
        }

        let width = self.width;
        let height = self.height;
        let scale = self.scale;
        let (phys_w, phys_h) = scale.physical_size(width, height)?;
        let scale_f = scale.as_f32();

        let shm = state
            .shm
            .clone()
            .ok_or_else(|| FrontendError::Init("no shm".into()))?;
        // The buffer pool keys idle buffers by physical width/height.
        let slot = state.buffer_pool.next_buffer(&shm, qh, phys_w, phys_h)?;
        // The pin square count tracks the secret buffer length; read it
        // before borrowing the buffer pool below.
        let pin_len = if self.mode == InterfaceMode::GetPin {
            state.secbuf().len()
        } else {
            0
        };

        let buffer = state
            .buffer_pool
            .get_mut(slot)
            .ok_or_else(|| FrontendError::Init("no buffer".into()))?;
        let mmap = buffer
            .mmap
            .as_mut()
            .ok_or_else(|| FrontendError::Init("buffer is not mapped".into()))?;
        let mut pixmap = PixmapMut::from_bytes(&mut mmap[..], phys_w, phys_h)
            .ok_or_else(|| FrontendError::Init("invalid buffer dimensions".into()))?;
        pixmap.fill(SkColor::TRANSPARENT);

        let vp = u32::from(self.ui.vertical_padding);
        let hp = u32::from(self.ui.horizontal_padding);
        let bip = u32::from(self.ui.button_inner_padding);
        let btn_border = u32::from(self.ui.button_border);

        self.draw_background(&mut pixmap, &self.ui, &self.colours, scale_f);

        let mut y = vp;
        if let Some(title) = &self.title {
            let x = width / 2 - title.width / 2;
            title.draw(
                &mut pixmap,
                &mut self.swash_cache,
                &mut self.font_system,
                to_premul8(self.colours.text),
                x,
                y,
                scale_f,
            );
            y += title.height + vp;
        }
        if let Some(description) = &self.description {
            let x = width / 2 - description.width / 2;
            description.draw(
                &mut pixmap,
                &mut self.swash_cache,
                &mut self.font_system,
                to_premul8(self.colours.text),
                x,
                y,
                scale_f,
            );
            y += description.height + vp;
        }

        if self.mode == InterfaceMode::GetPin {
            if let Some(prompt) = &self.prompt {
                let x = width / 2 - prompt.width / 2;
                prompt.draw(
                    &mut pixmap,
                    &mut self.swash_cache,
                    &mut self.font_system,
                    to_premul8(self.colours.text),
                    x,
                    y,
                    scale_f,
                );
                y += prompt.height + vp;
            }
            y += self.draw_pin_area(&mut pixmap, pin_len, y, &self.ui, &self.colours, scale_f);
        }

        if let Some(errmessage) = &self.errmessage {
            let x = width / 2 - errmessage.width / 2;
            errmessage.draw(
                &mut pixmap,
                &mut self.swash_cache,
                &mut self.font_system,
                to_premul8(self.colours.error_text),
                x,
                y,
                scale_f,
            );
            y += errmessage.height + vp;
        }

        // Buttons. The hotspot list is populated on the first render.
        let populate_hotspots = self.hotspots.is_empty();

        let mut combined_button_length = 0u32;
        for tv in [self.ok.as_ref(), self.notok.as_ref(), self.cancel.as_ref()]
            .into_iter()
            .flatten()
        {
            combined_button_length += tv.width + hp + 2 * bip;
        }
        let mut x = (width + hp) / 2;
        x = x.saturating_sub(combined_button_length / 2);

        if let Some((bw, bh)) = self
            .cancel
            .as_ref()
            .map(|t| (t.width + 2 * bip, t.height + 2 * bip))
        {
            if populate_hotspots {
                self.hotspots.push(HotSpot {
                    effect: HotSpotEffect::Cancel,
                    x,
                    y,
                    width: bw,
                    height: bh,
                });
            }
            bordered_rectangle(
                &mut pixmap,
                x,
                y,
                bw,
                bh,
                btn_border,
                scale_f,
                self.colours.cancel_button,
                self.colours.cancel_button_border,
            );
            if let Some(cancel) = &self.cancel {
                cancel.draw(
                    &mut pixmap,
                    &mut self.swash_cache,
                    &mut self.font_system,
                    to_premul8(self.colours.cancel_button_text),
                    x + bip,
                    y + bip,
                    scale_f,
                );
            }
            x += bw + hp;
        }
        if let Some((bw, bh)) = self
            .notok
            .as_ref()
            .map(|t| (t.width + 2 * bip, t.height + 2 * bip))
        {
            if populate_hotspots {
                self.hotspots.push(HotSpot {
                    effect: HotSpotEffect::NotOk,
                    x,
                    y,
                    width: bw,
                    height: bh,
                });
            }
            bordered_rectangle(
                &mut pixmap,
                x,
                y,
                bw,
                bh,
                btn_border,
                scale_f,
                self.colours.not_ok_button,
                self.colours.not_ok_button_border,
            );
            if let Some(notok) = &self.notok {
                notok.draw(
                    &mut pixmap,
                    &mut self.swash_cache,
                    &mut self.font_system,
                    to_premul8(self.colours.not_ok_button_text),
                    x + bip,
                    y + bip,
                    scale_f,
                );
            }
            x += bw + hp;
        }
        if let Some((bw, bh)) = self
            .ok
            .as_ref()
            .map(|t| (t.width + 2 * bip, t.height + 2 * bip))
        {
            if populate_hotspots {
                self.hotspots.push(HotSpot {
                    effect: HotSpotEffect::Ok,
                    x,
                    y,
                    width: bw,
                    height: bh,
                });
            }
            bordered_rectangle(
                &mut pixmap,
                x,
                y,
                bw,
                bh,
                btn_border,
                scale_f,
                self.colours.ok_button,
                self.colours.ok_button_border,
            );
            if let Some(ok) = &self.ok {
                ok.draw(
                    &mut pixmap,
                    &mut self.swash_cache,
                    &mut self.font_system,
                    to_premul8(self.colours.ok_button_text),
                    x + bip,
                    y + bip,
                    scale_f,
                );
            }
        }

        // Premultiplied RGBA8888 → Argb8888 little-endian byte order.
        swap_rb(&mut mmap[..]);

        let wl_surface = self
            .wl_surface
            .as_ref()
            .ok_or_else(|| FrontendError::Init("no wl_surface".into()))?;
        // Commit protocol: integer buffer scale vs. fractional viewport.
        match scale {
            Scale::Integer(n) => {
                wl_surface.set_buffer_scale(n as i32);
                // If a viewport exists but fractional mode is inactive,
                // clear any previously set destination so the buffer maps
                // to its native logical size.
                if let Some(vp) = self.viewport.as_ref() {
                    vp.set_destination(-1, -1);
                }
            }
            Scale::Fractional(_) => {
                wl_surface.set_buffer_scale(1);
                if let Some(vp) = self.viewport.as_ref() {
                    // Map the whole physical buffer to the logical surface
                    // size; set before each fractional commit to stay
                    // self-consistent after a scale transition.
                    vp.set_destination(width as i32, height as i32);
                }
            }
        }
        wl_surface.attach(buffer.wl_buffer.as_ref(), 0, 0);
        wl_surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        wl_surface.commit();
        buffer.busy = true;
        Ok(())
    }

    /// Background: bordered rectangle, with rounded corners when
    /// configured. The border fills the outer rounded rect first. The
    /// inset background uses `BlendMode::Source`, so its configured alpha
    /// replaces the border below it instead of blending over it. The two
    /// concentric paths keep the border thickness and outer rasterization
    /// compatible with the legacy frontend.
    fn draw_background(
        &self,
        pixmap: &mut PixmapMut<'_>,
        ui: &WaylandUi,
        colours: &WaylandColours,
        scale: f32,
    ) {
        let s = scale;
        let w = self.width as f32 * s;
        let h = self.height as f32 * s;

        if ui.corner_radius > 0 {
            let r = u32::from(ui.corner_radius)
                .min(self.width / 2)
                .min(self.height / 2) as f32
                * s;
            let b = f32::from(ui.border) * s;
            draw_rounded_background(pixmap, w, h, r, b, colours.background, colours.border);
        } else {
            bordered_rectangle(
                pixmap,
                0,
                0,
                self.width,
                self.height,
                u32::from(ui.border),
                scale,
                colours.background,
                colours.border,
            );
        }
    }

    fn draw_pin_area(
        &self,
        pixmap: &mut PixmapMut<'_>,
        len: usize,
        pinarea_y: u32,
        ui: &WaylandUi,
        colours: &WaylandColours,
        scale: f32,
    ) -> u32 {
        let sqs = u32::from(ui.pin_square_size);
        let square_padding = sqs / 2;
        let pinarea_height = sqs + 2 * square_padding;
        let pinarea_width =
            u32::from(ui.pin_square_amount) * (sqs + square_padding) + square_padding;
        let pinarea_x = self.width / 2 - pinarea_width / 2;

        bordered_rectangle(
            pixmap,
            pinarea_x,
            pinarea_y,
            pinarea_width,
            pinarea_height,
            u32::from(ui.border),
            scale,
            colours.pin_background,
            colours.pin_border,
        );

        let squares = (len as u32).min(u32::from(ui.pin_square_amount));
        for i in 0..squares {
            let x = pinarea_x + i * sqs + (i + 1) * square_padding;
            let y = pinarea_y + square_padding;
            bordered_rectangle(
                pixmap,
                x,
                y,
                sqs,
                sqs,
                u32::from(ui.pin_square_border),
                scale,
                colours.pin_square,
                colours.pin_border,
            );
        }

        pinarea_height + u32::from(ui.vertical_padding)
    }
}

/// Convenience function for input handlers: takes the surface out of state,
/// renders it, puts it back, and aborts on error.
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

// --- Pixel format -------------------------------------------------------------

/// Swap R and B channels in place, converting tiny-skia's premultiplied
/// RGBA8888 byte order to Wayland's little-endian Argb8888.
pub fn swap_rb(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

// --- Dispatch impls for render-owned proxies ----------------------------------

impl Dispatch<WlSurface, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_surface::Event;
        match event {
            Event::Enter { output } => {
                if let Some(mut surface) = state.surface.take() {
                    let changed = surface.handle_enter(state, &output);
                    state.surface = Some(surface);
                    if changed.is_some() {
                        render_surface(state, qh);
                    }
                }
            }
            Event::Leave { output } => {
                if let Some(mut surface) = state.surface.take() {
                    let changed = surface.handle_leave(state, &output);
                    state.surface = Some(surface);
                    if changed.is_some() {
                        render_surface(state, qh);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use super::ExitReason;
        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Event;
        match event {
            // Mark configured, ack, render. The compositor-requested
            // sizes are intentionally ignored; the surface computes its
            // own size.
            Event::Configure { serial, .. } => {
                if let Some(mut surface) = state.surface.take() {
                    surface.configured = true;
                    state.surface = Some(surface);
                }
                layer_surface.ack_configure(serial);
                render_surface(state, qh);
            }
            // The compositor closed the surface: abort.
            Event::Closed => {
                state.abort(ExitReason::Error(FrontendError::Init(
                    "layer surface closed".into(),
                )));
            }
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event;
        if let Event::PreferredScale { scale } = event {
            if let Some(mut surface) = state.surface.take() {
                let changed = surface.handle_preferred_scale(state, scale);
                state.surface = Some(surface);
                if changed.is_some() {
                    render_surface(state, qh);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{draw_rounded_background, swap_rb, HotSpot, HotSpotEffect};

    #[test]
    fn swap_rb_converts_rgba_to_bgra() {
        let mut data = [
            0x11, 0x22, 0x33, 0x44, // pixel 0: R=0x11 G=0x22 B=0x33 A=0x44
            0xaa, 0xbb, 0xcc, 0xdd, // pixel 1
        ];
        swap_rb(&mut data);
        assert_eq!(
            data,
            [
                0x33, 0x22, 0x11, 0x44, // pixel 0: B and R swapped
                0xcc, 0xbb, 0xaa, 0xdd, // pixel 1
            ]
        );
    }

    #[test]
    fn hotspot_contains_point_inside_and_outside() {
        let hs = HotSpot {
            effect: HotSpotEffect::Ok,
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        // Corners inclusive.
        assert!(hs.contains_point(10, 20));
        assert!(hs.contains_point(40, 60)); // x+w, y+h
        assert!(hs.contains_point(25, 40)); // interior
                                            // Outside.
        assert!(!hs.contains_point(9, 20));
        assert!(!hs.contains_point(41, 20));
        assert!(!hs.contains_point(10, 61));
    }

    // --- Font description parsing (wayprompt(5) font-regular/font-large) ---

    use super::{FamilySpec, FontDescription, TextView, FONT_BOLD, FONT_REGULAR};
    use cosmic_text::{FontSystem, Weight};

    #[test]
    fn font_description_defaults_without_config() {
        let regular = FontDescription::regular(None);
        assert_eq!(regular.family, FamilySpec::SansSerif);
        assert_eq!(regular.size, 14.0);
        let large = FontDescription::large(None);
        assert_eq!(large.family, FamilySpec::SansSerif);
        assert_eq!(large.size, 20.0);
    }

    #[test]
    fn font_description_parses_family_and_size() {
        let d = FontDescription::regular(Some("Iosevka:size=22"));
        assert_eq!(d.family, FamilySpec::Name("Iosevka".to_string()));
        assert_eq!(d.size, 22.0);
    }

    #[test]
    fn font_description_generic_aliases_and_attributes() {
        assert_eq!(
            FontDescription::regular(Some("sans:size=9")).family,
            FamilySpec::SansSerif
        );
        assert_eq!(
            FontDescription::regular(Some("Sans-Serif")).family,
            FamilySpec::SansSerif
        );
        assert_eq!(
            FontDescription::regular(Some("mono")).family,
            FamilySpec::Monospace
        );
        assert_eq!(
            FontDescription::regular(Some("monospace:size=11")).family,
            FamilySpec::Monospace
        );
        assert_eq!(
            FontDescription::regular(Some("serif")).family,
            FamilySpec::Serif
        );
        // Empty family keeps the sans-serif generic; size still applies.
        let d = FontDescription::regular(Some(":size=28"));
        assert_eq!(d.family, FamilySpec::SansSerif);
        assert_eq!(d.size, 28.0);
        // Unknown or malformed attributes are ignored.
        let d = FontDescription::large(Some("Iosevka:weight=bold:size=abc:slant=italic"));
        assert_eq!(d.family, FamilySpec::Name("Iosevka".to_string()));
        assert_eq!(d.size, 20.0);
        // Non-positive or non-finite sizes are rejected.
        assert_eq!(FontDescription::regular(Some("Foo:size=-3")).size, 14.0);
        assert_eq!(FontDescription::regular(Some("Foo:size=0")).size, 14.0);
    }

    #[test]
    fn descriptor_size_reaches_shaping_metrics() {
        // A bundled-only database keeps this test off the system font set.
        let mut db = fontdb::Database::new();
        db.load_font_data(FONT_REGULAR.to_vec());
        db.load_font_data(FONT_BOLD.to_vec());
        db.set_sans_serif_family("DejaVu Sans");
        let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);

        let regular = FontDescription::regular(None);
        let large = FontDescription::regular(Some("DejaVu Sans:size=28"));
        let a = TextView::new(&mut font_system, "Secret", &regular, Weight::NORMAL);
        let b = TextView::new(&mut font_system, "Secret", &large, Weight::NORMAL);
        // 14px → 17px line height, 28px → 34px: the configured size must
        // scale the shaped layout.
        assert_eq!(a.height, 17);
        assert_eq!(b.height, 34);
        assert!(b.width > a.width);
    }

    // --- Rounded background alpha compositing ---

    use crate::config::parse_colour;
    use tiny_skia::Pixmap;

    /// Sample a pixel from a premultiplied RGBA pixmap as (r, g, b, a) u8.
    fn pixel_at(pix: &Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let data = pix.data();
        let idx = ((y * pix.width() + x) * 4) as usize;
        (data[idx], data[idx + 1], data[idx + 2], data[idx + 3])
    }

    #[test]
    fn rounded_background_translucent_bg_composites_over_compositor() {
        // Background 0x282a36cc: dark blue with ~80% alpha.
        // Border 0x80bfff: light blue, fully opaque.
        let bg = parse_colour("0x282a36cc").unwrap();
        let border = parse_colour("0x80bfff").unwrap();
        let mut pixmap = Pixmap::new(200, 100).unwrap();

        draw_rounded_background(&mut pixmap.as_mut(), 200.0, 100.0, 10.0, 4.0, bg, border);

        // Center pixel: should be the background colour composited over
        // transparent (the initial pixmap is all-zero). With SourceOver,
        // bg over transparent = bg itself (premultiplied).
        let (r, g, b, a) = pixel_at(&pixmap, 100, 50);
        // Alpha ~0xcc (204). Background premultiplied: r=0x28*0xcc/0xff
        // The premultiplied values from parse_colour are already
        // premultiplied; to_sk_color un-premultiplies for tiny-skia
        // which re-premultiplies internally. The result should be the
        // original premultiplied colour.
        assert_eq!(a, 0xcc, "center alpha should be the background alpha");
        // Premultiplied channels: straight * alpha / 255 (truncated).
        // r: 0x28 * 0xcc / 0xff = 32 (0x20)
        // g: 0x2a * 0xcc / 0xff = 33 (0x21)
        // b: 0x36 * 0xcc / 0xff = 43 (0x2b)
        assert!(
            (r as i16 - 0x20).abs() <= 1,
            "center red should be ~0x20, got {r:#x}"
        );
        assert!(
            (g as i16 - 0x21).abs() <= 1,
            "center green should be ~0x21, got {g:#x}"
        );
        assert!(
            (b as i16 - 0x2b).abs() <= 1,
            "center blue should be ~0x2b, got {b:#x}"
        );
    }

    #[test]
    fn rounded_background_translucent_border_remains_standalone() {
        // Background fully opaque red, border 50% transparent green.
        let bg = parse_colour("0xff0000").unwrap();
        let border = parse_colour("0x00ff0080").unwrap();
        let mut pixmap = Pixmap::new(200, 100).unwrap();

        draw_rounded_background(&mut pixmap.as_mut(), 200.0, 100.0, 10.0, 8.0, bg, border);

        // The border occupies the outer ring independently of the
        // background, matching the legacy frontend's border geometry.
        let (r, g, b, a) = pixel_at(&pixmap, 2, 50);
        assert_eq!(a, 0x80);
        assert!(r <= 1, "border red should be ~0, got {r:#x}");
        assert!(
            (g as i16 - 0x80).abs() <= 1,
            "premultiplied border green should be ~0x80, got {g:#x}"
        );
        assert!(b <= 1, "border blue should be ~0, got {b:#x}");

        // The Source-blended interior replaces the border and remains
        // the configured opaque background.
        let (r, _g, _b, a) = pixel_at(&pixmap, 100, 50);
        assert_eq!(a, 0xff);
        assert_eq!(r, 0xff);
    }

    #[test]
    fn rounded_background_degenerate_no_inner_fills_border() {
        // Border width >= half the dimension: inner rect is zero/negative.
        // The whole outer rect should be filled with the border colour.
        let bg = parse_colour("0xff0000").unwrap();
        let border = parse_colour("0x00ff00").unwrap();
        let mut pixmap = Pixmap::new(20, 10).unwrap();

        // b=15 > w/2=10: inner width = 20 - 30 = -10 → degenerate
        draw_rounded_background(&mut pixmap.as_mut(), 20.0, 10.0, 5.0, 15.0, bg, border);

        let (r, g, _b, a) = pixel_at(&pixmap, 10, 5);
        assert_eq!(a, 0xff);
        assert_eq!(r, 0x00);
        assert_eq!(g, 0xff);
    }

    #[test]
    fn rounded_background_opaque_colours_no_alpha_artifacts() {
        // Both opaque: no alpha issues at all.
        let bg = parse_colour("0x282a36").unwrap();
        let border = parse_colour("0x80bfff").unwrap();
        let mut pixmap = Pixmap::new(200, 100).unwrap();

        draw_rounded_background(&mut pixmap.as_mut(), 200.0, 100.0, 10.0, 4.0, bg, border);

        // Center: background colour, fully opaque.
        let (r, g, b, a) = pixel_at(&pixmap, 100, 50);
        assert_eq!(a, 0xff);
        assert_eq!(r, 0x28);
        assert_eq!(g, 0x2a);
        assert_eq!(b, 0x36);

        // Border ring: border colour, fully opaque.
        let (r, g, b, a) = pixel_at(&pixmap, 2, 50);
        assert_eq!(a, 0xff);
        assert_eq!(r, 0x80);
        assert_eq!(g, 0xbf);
        assert_eq!(b, 0xff);
    }
}
