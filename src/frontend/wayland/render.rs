//! Layer-shell surface + software render pipeline (parity `Wayland.zig:740-1254`).
//!
//! Renders the prompt into `wl_shm` Argb8888 buffers with tiny-skia; text
//! is shaped once per surface with cosmic-text (bundled DejaVu faces, D7)
//! and rasterized per render through swash. Each [`Surface::render`]
//! composites background, labels, pin area and buttons, converts the
//! premultiplied RGBA pixmap to Argb8888 byte order (`swap_rb`) and
//! attaches the buffer to the layer-shell surface.

use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, SwashImage,
    Weight,
};
use tiny_skia::{
    Color as SkColor, FillRule, Paint, Path, PathBuilder, PixmapMut, PremultipliedColorU8, Rect,
    Shader, Stroke, Transform,
};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use crate::config::{Colour, WaylandColours, WaylandUi};
use crate::frontend::{FrontendError, InterfaceMode};

use super::WaylandState;

// --- Fonts (D7) --------------------------------------------------------------

/// Bundled font faces: DejaVu Sans regular + bold, no system font scan.
const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../assets/DejaVuSans-Bold.ttf");

/// Legacy `sans:size=14` / `sans:size=20`.
const FONT_SIZE_REGULAR: f32 = 14.0;
const FONT_SIZE_LARGE: f32 = 20.0;

fn new_font_system() -> FontSystem {
    FontSystem::new_with_fonts([
        fontdb::Source::Binary(Arc::new(FONT_REGULAR.to_vec())),
        fontdb::Source::Binary(Arc::new(FONT_BOLD.to_vec())),
    ])
}

// --- HotSpots ----------------------------------------------------------------

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

// --- Text views --------------------------------------------------------------

/// One shaped text label (parity `Wayland.zig:48-217`, fcft → cosmic-text).
struct TextView {
    buffer: Buffer,
    width: u32,
    height: u32,
}

impl TextView {
    /// Shape `text` and measure it (parity `Wayland.zig:62-143`).
    ///
    /// Labels above the regular size use the bundled bold face (title and
    /// prompt, design D7). The line height mirrors fcft's `font.height`
    /// for the bundled DejaVu metrics: `ceil((ascent + descent) * size /
    /// em) ≈ size * 1.2` (17px at 14, 24px at 20).
    fn new(font_system: &mut FontSystem, text: &str, font_size: f32) -> Self {
        let mut buffer = Buffer::new(
            font_system,
            Metrics::new(font_size, (font_size * 1.2).round()),
        );
        buffer.set_size(font_system, None, None);
        let mut attrs = Attrs::new().family(Family::SansSerif);
        if font_size > FONT_SIZE_REGULAR {
            attrs = attrs.weight(Weight::BOLD);
        }
        buffer.set_text(font_system, text, attrs, Shaping::Advanced);
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

    /// Rasterize the label at `(x, y)` onto `pixmap` (parity
    /// `Wayland.zig:155-216`).
    fn draw(
        &self,
        pixmap: &mut PixmapMut<'_>,
        swash_cache: &mut SwashCache,
        font_system: &mut FontSystem,
        colour: PremultipliedColorU8,
        x: u32,
        y: u32,
    ) {
        let pixmap_w = pixmap.width();
        let pixmap_h = pixmap.height();
        let data = pixmap.data_mut();
        for run in self.buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((x as f32, y as f32 + run.line_y), 1.0);
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

/// Shape one trimmed label into a [`TextView`] (parity
/// `Wayland.zig:1567-1577`). `None` → no view; a label that is empty after
/// trimming errors (parity fcft `error.EmptyString`, `Wayland.zig:63`).
fn make_view(
    font_system: &mut FontSystem,
    label: Option<&str>,
    large: bool,
) -> Result<Option<TextView>, FrontendError> {
    let Some(text) = label else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(FrontendError::Init("empty text label".into()));
    }
    let size = if large {
        FONT_SIZE_LARGE
    } else {
        FONT_SIZE_REGULAR
    };
    Ok(Some(TextView::new(font_system, trimmed, size)))
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

/// Bordered rectangle (parity `Wayland.zig:1228-1253`): fill the interior,
/// then the four border strips, un-antialiased like pixman
/// `fillRectangles`. Coordinates are scaled by `scale`.
#[allow(clippy::too_many_arguments)]
fn bordered_rectangle(
    pixmap: &mut PixmapMut<'_>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    border: u32,
    scale: u32,
    background: Colour,
    border_colour: Colour,
) {
    let s = scale as f32;
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

/// The layer-shell surface. Parity with `Wayland.zig:740-1254`.
pub struct Surface {
    pub configured: bool,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub hotspots: Vec<HotSpot>,

    // Render-owned protocol objects.
    wl_surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    fractional_scale: Option<WpFractionalScaleV1>,

    // Text shaping/rasterization state (cosmic-text + swash, D7).
    font_system: FontSystem,
    swash_cache: SwashCache,

    // Shaped labels (parity the `Wayland` TextView fields).
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
    /// Create the layer-shell surface. Parity `Wayland.zig:756-786` (plus
    /// the TextView shaping the legacy does in `initTextViews` before
    /// `Surface.init`, `Wayland.zig:1567-1577`).
    pub fn new(
        state: &mut WaylandState,
        qh: &QueueHandle<WaylandState>,
        compositor: &WlCompositor,
        layer_shell: &ZwlrLayerShellV1,
        _shm: &WlShm,
        fractional: Option<&WpFractionalScaleManagerV1>,
        mode: InterfaceMode,
    ) -> Result<Self, FrontendError> {
        let wl_surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &wl_surface,
            None,
            Layer::Overlay,
            "wayprompt".to_string(),
            qh,
            (),
        );
        let fractional_scale = fractional.map(|m| m.get_fractional_scale(&wl_surface, qh, ()));

        let mut font_system = new_font_system();
        let swash_cache = SwashCache::new();

        // Title/prompt use the large/bold font, everything else regular.
        let labels = &state.config().labels;
        let title = make_view(&mut font_system, labels.title.as_deref(), true)?;
        let description = make_view(&mut font_system, labels.description.as_deref(), false)?;
        let errmessage = make_view(&mut font_system, labels.err_message.as_deref(), false)?;
        let prompt = make_view(&mut font_system, labels.prompt.as_deref(), true)?;
        let ok = make_view(&mut font_system, labels.ok.as_deref(), false)?;
        let notok = make_view(&mut font_system, labels.not_ok.as_deref(), false)?;
        let cancel = make_view(&mut font_system, labels.cancel.as_deref(), false)?;

        let mut surface = Self {
            configured: false,
            width: 0,
            height: 0,
            scale: 1,
            hotspots: Vec::new(),
            wl_surface: Some(wl_surface),
            layer_surface: Some(layer_surface),
            fractional_scale,
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
            ui: state.config().wayland_ui.clone(),
            colours: state.config().wayland_colours.clone(),
        };
        surface.calculate_size();

        // The listener setup (`Wayland.zig:782`) is implicit in the
        // `Dispatch` impls below.
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

    /// Parity `Wayland.zig:850-855` (the circle mask is a no-op here:
    /// rounded corners are drawn per-render with paths).
    pub fn deinit(self) {
        if let Some(fs) = self.fractional_scale {
            fs.destroy();
        }
        if let Some(ls) = self.layer_surface {
            ls.destroy();
        }
        if let Some(ws) = self.wl_surface {
            ws.destroy();
        }
    }

    /// Find the hotspot containing `(x, y)`. Parity `Wayland.zig:857-864`.
    pub fn hotspot_from_point(&self, x: u32, y: u32) -> Option<&HotSpot> {
        self.hotspots.iter().find(|hs| hs.contains_point(x, y))
    }

    /// Compute the surface dimensions from the shaped labels and the UI
    /// metrics. Parity `Wayland.zig:788-849`.
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

    /// Render the surface (parity `Wayland.zig:887-1036`).
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

        let shm = state
            .shm
            .clone()
            .ok_or_else(|| FrontendError::Init("no shm".into()))?;
        let slot = state.buffer_pool.next_buffer(&shm, qh, width, height)?;
        // Parity `Wayland.zig:914`: the pin square count tracks the secret
        // buffer length; read before borrowing the buffer pool below.
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
        let mut pixmap = PixmapMut::from_bytes(&mut mmap[..], width, height)
            .ok_or_else(|| FrontendError::Init("invalid buffer dimensions".into()))?;
        pixmap.fill(SkColor::TRANSPARENT);

        let vp = u32::from(self.ui.vertical_padding);
        let hp = u32::from(self.ui.horizontal_padding);
        let bip = u32::from(self.ui.button_inner_padding);
        let btn_border = u32::from(self.ui.button_border);

        self.draw_background(&mut pixmap, &self.ui, &self.colours);

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
                );
                y += prompt.height + vp;
            }
            y += self.draw_pin_area(&mut pixmap, pin_len, y, &self.ui, &self.colours);
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
            );
            y += errmessage.height + vp;
        }

        // Buttons (parity `Wayland.zig:925-1028`). The hotspot list is
        // populated on the first render.
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
                self.scale,
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
                self.scale,
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
                self.scale,
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
                );
            }
        }

        // Premultiplied RGBA8888 → Argb8888 little-endian byte order (D6).
        swap_rb(&mut mmap[..]);

        let wl_surface = self
            .wl_surface
            .as_ref()
            .ok_or_else(|| FrontendError::Init("no wl_surface".into()))?;
        wl_surface.set_buffer_scale(self.scale as i32);
        wl_surface.attach(buffer.wl_buffer.as_ref(), 0, 0);
        wl_surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        wl_surface.commit();
        buffer.busy = true;
        Ok(())
    }

    /// Background: bordered rectangle, with rounded corners when
    /// configured (parity `Wayland.zig:1038-1186`; the legacy circle-mask
    /// compositing is replaced by a rounded-rect path fill + stroke,
    /// behavioural parity per D6).
    fn draw_background(
        &self,
        pixmap: &mut PixmapMut<'_>,
        ui: &WaylandUi,
        colours: &WaylandColours,
    ) {
        let s = self.scale as f32;
        let w = self.width as f32 * s;
        let h = self.height as f32 * s;

        if ui.corner_radius > 0 {
            let r = u32::from(ui.corner_radius)
                .min(self.width / 2)
                .min(self.height / 2) as f32
                * s;
            let Some(path) = rounded_rect_path(0.0, 0.0, w, h, r) else {
                return;
            };
            // Anti-aliased (grayscale AA, D7); `Paint::default()` has
            // `anti_alias = true`.
            let mut paint = Paint {
                shader: Shader::SolidColor(to_sk_color(colours.background)),
                ..Default::default()
            };
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            paint.shader = Shader::SolidColor(to_sk_color(colours.border));
            // The stroke is centred on the path; double the width so
            // `border` pixels land inside (the outer half clips off).
            let stroke = Stroke {
                width: f32::from(ui.border) * s * 2.0,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        } else {
            bordered_rectangle(
                pixmap,
                0,
                0,
                self.width,
                self.height,
                u32::from(ui.border),
                self.scale,
                colours.background,
                colours.border,
            );
        }
    }

    /// Draw the pin area and return the vertical space consumed (parity
    /// `Wayland.zig:1188-1226`).
    fn draw_pin_area(
        &self,
        pixmap: &mut PixmapMut<'_>,
        len: usize,
        pinarea_y: u32,
        ui: &WaylandUi,
        colours: &WaylandColours,
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
            self.scale,
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
                self.scale,
                colours.pin_square,
                colours.pin_border,
            );
        }

        pinarea_height + u32::from(ui.vertical_padding)
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

// --- Pixel format -------------------------------------------------------------

/// Swap R and B channels in place, converting tiny-skia's premultiplied
/// RGBA8888 byte order to Wayland's little-endian Argb8888 (D6).
pub fn swap_rb(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

// --- Dispatch impls for render-owned proxies ----------------------------------

impl Dispatch<WlSurface, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSurface,
        _event: <WlSurface as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WlSurface emits `enter`/`leave`; no output tracking needed.
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
            // Parity `Wayland.zig:868-878`: mark configured, ack, render.
            // The compositor-requested sizes are intentionally ignored
            // (parity).
            Event::Configure { serial, .. } => {
                if let Some(mut surface) = state.surface.take() {
                    surface.configured = true;
                    state.surface = Some(surface);
                }
                layer_surface.ack_configure(serial);
                render_surface(state, qh);
            }
            // Parity `Wayland.zig:880-883`.
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
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event;
        // Legacy pins scale to 1 (`Wayland.zig:749` TODO); the fractional
        // scale protocol reports 120 units per integer step.
        if let Event::PreferredScale { scale } = event {
            if let Some(surface) = state.surface.as_mut() {
                surface.scale = ((f64::from(scale) / 120.0).round() as u32).max(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{swap_rb, HotSpot, HotSpotEffect};

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
}
