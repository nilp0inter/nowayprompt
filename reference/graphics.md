# Pure-Rust Graphics Stack API Reference

This reference manual documents the APIs, data structures, memory layouts, safety constraints, and integration patterns for the pure-Rust graphics and typography stack used in the `nowayprompt` rewrite. The stack comprises four core libraries:

*   **`fontdb`**: Core font database management, system-wide font directory scanning, and CSS-style querying.
*   **`cosmic-text`**: Multi-line text shaping (via `harfrust`/`rustybuzz`), font fallback selection, text wrapping, and high-level layout calculations.
*   **`swash`**: Font scaling, outline extraction, and glyph rasterization. It acts as the backend for glyph rendering in `cosmic-text`.
*   **`tiny-skia`**: CPU-based 2D vector graphics rendering engine, handling geometries, path builder operations, and paint compositions.

---

## 1. `fontdb::Database`

The `fontdb` crate provides an in-memory index of font faces available on the system. It collects metadata from font files (TrueType, OpenType, and collections) without reading full outline tables into memory unless requested, making it lightweight and fast.

### Key Types and Signatures

#### `Database`
```rust
pub struct Database { /* private fields */ }

impl Database {
    pub fn new() -> Self;
    pub fn load_system_fonts(&mut self);
    pub fn load_font_file<P: AsRef<Path>>(&mut self, path: P) -> Result<(), Error>;
    pub fn load_fonts_dir<P: AsRef<Path>>(&mut self, dir: P);
    pub fn load_font_data(&mut self, data: Vec<u8>);
    
    pub fn set_sans_serif_family(&mut self, name: impl Into<String>);
    pub fn set_monospace_family(&mut self, name: impl Into<String>);
    pub fn set_serif_family(&mut self, name: impl Into<String>);
    
    pub fn query(&self, query: &Query) -> Option<ID>;
    pub fn face(&self, id: ID) -> Option<&FaceInfo>;
    pub fn len(&self) -> usize;
}
```

#### `Query`
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Query<'a> {
    pub families: &'a [Family<'a>],
    pub weight: Weight,
    pub stretch: Stretch,
    pub style: Style,
}
```

#### `Family`
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family<'a> {
    Name(&'a str),
    Serif,
    SansSerif,
    Cursive,
    Fantasy,
    Monospace,
}
```

#### `Weight`, `Style`, `Stretch`
```rust
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Weight(pub u16);

impl Weight {
    pub const THIN: Weight = Weight(100);
    pub const EXTRA_LIGHT: Weight = Weight(200);
    pub const LIGHT: Weight = Weight(300);
    pub const NORMAL: Weight = Weight(400);
    pub const MEDIUM: Weight = Weight(500);
    pub const SEMIBOLD: Weight = Weight(600);
    pub const BOLD: Weight = Weight(700);
    pub const EXTRA_BOLD: Weight = Weight(800);
    pub const BLACK: Weight = Weight(900);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stretch {
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    Normal,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}
```

#### `ID` and `FaceInfo`
```rust
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ID(pub u32);

#[derive(Debug, Clone)]
pub struct FaceInfo {
    pub id: ID,
    pub source: Source,
    pub index: u32,
    pub families: Vec<(String, String)>, // (language_code, family_name)
    pub post_script_name: String,
    pub style: Style,
    pub weight: Weight,
    pub stretch: Stretch,
    pub monospaced: bool,
}
```

### Usage Patterns

#### Loading and Querying System Fonts
```rust
let mut db = fontdb::Database::new();

// Load OS-specific system font directories recursively (cross-platform)
db.load_system_fonts();

// Set system-wide font aliases
db.set_sans_serif_family("Liberation Sans");
db.set_monospace_family("DejaVu Sans Mono");

// Perform a CSS-style query
let query = fontdb::Query {
    families: &[
        fontdb::Family::Name("Fira Code"),
        fontdb::Family::Monospace,
    ],
    weight: fontdb::Weight::NORMAL,
    stretch: fontdb::Stretch::Normal,
    style: fontdb::Style::Normal,
};

if let Some(font_id) = db.query(&query) {
    let info = db.face(font_id).expect("Face must exist");
    println!("Found font: {}", info.post_script_name);
}
```

---

## 2. `cosmic-text`

`cosmic-text` implements font caching, text layout calculation, multi-line paragraph wrapping, alignment, and rendering management. It uses `rustybuzz` for shaping OpenType tables and relies on `fontdb` for font discovery.

### Key Types and Signatures

#### `FontSystem`
Maintains the cache of loaded fonts and provides access to the underlying `fontdb::Database`. Typically instanced once per application lifecycle because of the scan overhead.
```rust
pub struct FontSystem { /* private fields */ }

impl FontSystem {
    pub fn new() -> Self;
    pub fn new_with_locale_and_db(locale: String, db: fontdb::Database) -> Self;
    pub const fn db(&self) -> &fontdb::Database;
    pub fn db_mut(&mut self) -> &mut fontdb::Database;
    pub fn locale(&self) -> &str;
}
```

#### `SwashCache`
Holds scaled and rasterized glyph textures/pixel representations using `swash`. 
```rust
pub struct SwashCache { /* private fields */ }

impl SwashCache {
    pub fn new() -> Self;
    pub fn get_image(&mut self, font_system: &mut FontSystem, cache_key: CacheKey) -> &Option<SwashImage>;
    
    // Iterates over pixels of the glyph. Base color is mixed into the output pixels.
    pub fn with_pixels<F: FnMut(i32, i32, Color)>(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
        base: Color,
        f: F,
    );
}
```

#### `Metrics`
Defines font rendering properties.
```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub font_size: f32,
    pub line_height: f32,
}

impl Metrics {
    pub const fn new(font_size: f32, line_height: f32) -> Self;
    pub const fn relative(font_size: f32, line_height_scale: f32) -> Self;
}
```

#### `Attrs<'a>` and `Color`
Attributes applied to spans of text.
```rust
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Attrs<'a> {
    pub color_opt: Option<Color>,
    pub family: Family<'a>,
    pub stretch: Stretch,
    pub style: Style,
    pub weight: Weight,
    pub metadata: usize,
    pub cache_key_flags: CacheKeyFlags,
    pub metrics_opt: Option<CacheMetrics>,
    // ...
}

#[derive(Clone, Copy, Debug, PartialOrd, Ord, Eq, Hash, PartialEq)]
pub struct Color(pub u32); // 0xAARRGGBB format

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self;
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self;
    pub const fn r(self) -> u8;
    pub const fn g(self) -> u8;
    pub const fn b(self) -> u8;
    pub const fn a(self) -> u8;
}
```

#### `Buffer`
The primary multiline text state and shaping controller.
```rust
#[derive(Debug)]
pub struct Buffer { /* private fields */ }

impl Buffer {
    pub fn new_empty(metrics: Metrics) -> Self;
    pub fn new(font_system: &mut FontSystem, metrics: Metrics) -> Self;
    
    pub fn borrow_with<'a>(&'a mut self, font_system: &'a mut FontSystem) -> BorrowedWithFontSystem<'a, Self>;
    
    pub fn set_text(&mut self, text: &str, attrs: &Attrs, shaping: Shaping, alignment: Option<Align>);
    pub fn set_size(&mut self, width_opt: Option<f32>, height_opt: Option<f32>);
    pub fn set_wrap(&mut self, wrap: Wrap);
    pub fn layout_runs(&self) -> LayoutRunIter<'_>;
    pub fn shape_until_scroll(&mut self, font_system: &mut FontSystem, prune: bool);
}
```

#### `LayoutRun` and `LayoutGlyph`
Calculated glyph placements representing one visible line of layout.
```rust
#[derive(Debug)]
pub struct LayoutRun<'a> {
    pub line_i: usize,
    pub text: &'a str,
    pub rtl: bool,
    pub glyphs: &'a [LayoutGlyph],
    pub decorations: &'a [DecorationSpan],
    pub line_y: f32,
    pub line_top: f32,
    pub line_height: f32,
    pub line_w: f32,
}

#[derive(Clone, Debug)]
pub struct LayoutGlyph {
    pub start: usize,
    pub end: usize,
    pub font_size: f32,
    pub font_weight: fontdb::Weight,
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub level: unicode_bidi::Level,
    pub x_offset: f32,
    pub y_offset: f32,
    pub color_opt: Option<Color>,
    pub metadata: usize,
    pub cache_key_flags: CacheKeyFlags,
}

impl LayoutGlyph {
    pub fn physical(&self, offset: (f32, f32), scale: f32) -> PhysicalGlyph;
}
```

#### `PhysicalGlyph` and `CacheKey`
Quantized glyph coordinates used for pixel-grid alignment.
```rust
#[derive(Clone, Debug)]
pub struct PhysicalGlyph {
    pub cache_key: CacheKey,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CacheKey {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size_bits: u32,
    pub x_bin: SubpixelBin,
    pub y_bin: SubpixelBin,
    pub font_weight: fontdb::Weight,
    pub flags: CacheKeyFlags,
}
```

### Wrapping Modes (`Wrap`)
*   `Wrap::None`: Text is laid out on a single line and overflows constraints.
*   `Wrap::Word`: Breaks layout only on whitespace word boundaries.
*   `Wrap::Glyph`: Force-wraps mid-character at boundary edges (fallback when words are longer than constraints).
*   `Wrap::WordOrGlyph`: Default wrapping strategy. Wraps on word boundaries, but falls back to glyph boundaries when a single word exceeds the constraint.

### Shaping Strategy (`Shaping`)
*   `Shaping::Basic`: Very cheap shaper. No font fallback, no ligature substitution, and no complex script support. Best for known ASCII strings where performance is critical.
*   `Shaping::Advanced`: Employs full OpenType features (ligatures, kerning, bidirectional ordering, script complex shaping) and font fallbacks.

### Measurement Patterns

Measuring bounds of shaped buffer requires resolving the layout runs:
```rust
// Measure actual bounding box after shaping
let mut max_width = 0.0f32;
let mut total_height = 0.0f32;

for run in buffer.layout_runs() {
    max_width = max_width.max(run.line_w);
    total_height += run.line_height;
}
```

---

## 3. `tiny-skia`

`tiny-skia` is a CPU-only, anti-aliased 2D rendering library that processes paths, geometries, transforms, and paints. It outputs directly to pixel buffers.

### Key Types and Signatures

#### `Pixmap` and `PixmapMut`
Containers for raw, premultiplied RGBA pixels.
```rust
pub struct Pixmap { /* data: Vec<u8> */ }

impl Pixmap {
    pub fn new(width: u32, height: u32) -> Option<Self>;
    pub fn as_mut(&mut self) -> PixmapMut<'_>;
    pub fn data(&self) -> &[u8];
    pub fn data_mut(&mut self) -> &mut [u8];
}

pub struct PixmapMut<'a> {
    data: &'a mut [u8],
    size: IntSize,
}

impl<'a> PixmapMut<'a> {
    pub fn from_bytes(data: &'a mut [u8], width: u32, height: u32) -> Option<Self>;
    pub fn width(&self) -> u32;
    pub fn height(&self) -> u32;
    pub fn fill(&mut self, color: Color);
    pub fn data_mut(&mut self) -> &mut [u8];
    pub fn pixels_mut(&mut self) -> &mut [PremultipliedColorU8];
}
```

#### `PathBuilder` and `Path`
```rust
pub struct PathBuilder { /* private fields */ }

impl PathBuilder {
    pub fn new() -> Self;
    pub fn move_to(&mut self, x: f32, y: f32);
    pub fn line_to(&mut self, x: f32, y: f32);
    pub fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32);
    pub fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32);
    pub fn close(&mut self);
    
    pub fn push_rect(&mut self, rect: Rect);
    pub fn push_circle(&mut self, cx: f32, cy: f32, radius: f32);
    
    pub fn finish(self) -> Option<Path>;
    pub fn from_rect(rect: Rect) -> Path; // Static shortcut
}
```

#### `Paint<'a>` and `Color`
```rust
#[derive(Clone, PartialEq, Debug)]
pub struct Paint<'a> {
    pub shader: Shader<'a>,
    pub blend_mode: BlendMode,
    pub anti_alias: bool,
    pub colorspace: ColorSpace,
    pub force_hq_pipeline: bool,
}

impl Default for Paint<'_> {
    fn default() -> Self {
        Paint {
            shader: Shader::SolidColor(Color::BLACK),
            blend_mode: BlendMode::SourceOver,
            anti_alias: true,
            colorspace: ColorSpace::Linear,
            force_hq_pipeline: false,
        }
    }
}

// Color representing straight RGBA values, each from 0.0 to 1.0
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Color {
    r: NormalizedF32,
    g: NormalizedF32,
    b: NormalizedF32,
    a: NormalizedF32,
}

impl Color {
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self;
    pub fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Option<Self>;
}
```

#### Drawing Rectangles
```rust
// Background
let rect = Rect::from_xywh(10.0, 10.0, 150.0, 40.0).unwrap();
let mut bg_paint = Paint::default();
bg_paint.set_color(Color::from_rgba8(40, 44, 52, 255));
pixmap.fill_rect(rect, &bg_paint, Transform::identity(), None);

// Border Outline
let border_path = PathBuilder::from_rect(rect);
let mut border_paint = Paint::default();
border_paint.set_color(Color::from_rgba8(100, 110, 125, 255));
let stroke = Stroke {
    width: 1.5,
    ..Default::default()
};
pixmap.stroke_path(&border_path, &border_paint, &stroke, Transform::identity(), None);
```

---

## 4. Integration Pipeline

To render user interface windows (like input prompt windows, text entry inputs, buttons, and error notification boxes) for Wayland using software buffers (`wl_shm`), all libraries must operate inside a unified rendering pipeline.

```mermaid
+-------------------------------------------------------+
|  Wayland Shared Memory Segment (memfd_create + mmap)  |
+-------------------------------------------------------+
                           |
                           v
              +-------------------------+
              |   tiny_skia::PixmapMut  |
              +-------------------------+
                           |
            +--------------+--------------+
            |                             |
            v                             v
  +--------------------+        +--------------------+
  |  Render Backdrop   |        | Render Styled Text |
  |  Borders & Buttons |        | (Swash pixel loop) |
  |  (using tiny-skia) |        | (using cosmic-text)|
  +--------------------+        +--------------------+
            |                             |
            +--------------+--------------+
                           |
                           v
+-------------------------------------------------------+
|   Premultiplied RGBA blending into Shared Memory      |
+-------------------------------------------------------+
                           |
                           v
+-------------------------------------------------------+
|  In-place Red-Blue swap (RGBA -> BGRA / ARGB8888)     |
+-------------------------------------------------------+
                           |
                           v
+-------------------------------------------------------+
|     Wayland Compositor Presentation (wl_surface)      |
+-------------------------------------------------------+
```

### Color Space and Format Conversions

1.  **Wayland Buffer Specification**: The Wayland protocol defines `wl_shm::Format::Argb8888` and `wl_shm::Format::Xrgb8888` formats. On little-endian architectures, these represent 32-bit values ordered in memory as **`[Blue, Green, Red, Alpha/unused]`** (which corresponds to `BGRA8888` byteorder).
2.  **`tiny-skia` Pixel Buffer Format**: `tiny-skia` strictly expects and outputs **premultiplied `RGBA8888`** pixels, ordered in memory as **`[Red, Green, Blue, Alpha]`**.
3.  **`cosmic-text::Color` Structure**: Stores values internally as `0xAARRGGBB` (big-endian), yielding:
    *   `r()` = byte 2 (red)
    *   `g()` = byte 1 (green)
    *   `b()` = byte 0 (blue)
    *   `a()` = byte 3 (alpha)
4.  **Compatibility Mapping**:
    *   Step 1: Perform all graphics composition (using `tiny-skia` operations and `cosmic-text` glyph rendering) within the `RGBA8888` format.
    *   Step 2: Swap the Red and Blue channels in-place over the final pixel buffer before presenting to the Wayland server, effectively converting `RGBA8888` to little-endian `ARGB8888`/`XRGB8888` (`BGRA`).

### Alpha Blending Mathematics

When rasterizing glyphs from `SwashCache::with_pixels`, the source pixels output from the rasterizer use straight alpha (`0..=255`). These must be blended manually on top of the `tiny-skia` target buffer, which is in **premultiplied** RGBA format.

Given:
*   Source Color (straight): $C_{src} = (R_{src}, G_{src}, B_{src})$
*   Source Coverage/Alpha: $A_{src}$ (from glyph rasterizer, normalized to $[0, 1]$)
*   Destination Color (premultiplied): $C'_{dst} = (R'_{dst}, G'_{dst}, B'_{dst})$
*   Destination Alpha: $A_{dst}$

The premultiplied compositing equations are:
$$R'_{src} = R_{src} 	imes A_{src}$$
$$G'_{src} = G_{src} 	imes A_{src}$$
$$B'_{src} = B_{src} 	imes A_{src}$$

$$R'_{out} = R'_{src} + R'_{dst} 	imes (1 - A_{src})$$
$$G'_{out} = G'_{src} + G'_{dst} 	imes (1 - A_{src})$$
$$B'_{out} = B'_{src} + B'_{dst} 	imes (1 - A_{src})$$
$$A_{out} = A_{src} + A_{dst} 	imes (1 - A_{src})$$

### Complete Software Rendering Example

The following code is a complete, dependency-isolated example demonstrating the pipeline. It constructs a shared memory segment, creates a vector UI with `tiny-skia`, shapes and blends styled text with `cosmic-text`, and converts the channel bytes to output a Wayland-compatible `ARGB8888` memory buffer.

```rust
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::ptr;
use tiny_skia::{Paint, PathBuilder, PixmapMut, Rect, Stroke, Transform, Color as SkiaColor};
use cosmic_text::{Attrs, Buffer, Color as CosmicColor, FontSystem, Metrics, Shaping, SwashCache, Renderer, PhysicalGlyph};

// Allocation of Software Shared Memory Buffer for Wayland wl_shm
pub struct WaylandBuffer {
    file: File,
    ptr: *mut u8,
    size: usize,
}

impl WaylandBuffer {
    pub fn allocate(width: u32, height: u32) -> Result<Self, std::io::Error> {
        let size = (width * height * 4) as usize;
        
        // Create an anonymous, in-memory descriptor (modern Linux system standard)
        let name = std::ffi::CString::new("wayprompt-shm").unwrap();
        let fd = unsafe {
            libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC)
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        
        let file = unsafe { File::from_raw_fd(fd) };
        file.set_len(size as u64)?;
        
        // Map descriptor into virtual address space
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        
        Ok(Self {
            file,
            ptr: ptr as *mut u8,
            size,
        })
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

impl Drop for WaylandBuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size);
        }
    }
}

// In-place byte swapper: Converts RGBA (tiny-skia format) to BGRA (Wayland native format)
pub fn swap_red_blue_inplace(buf: &mut [u8]) {
    for chunk in buf.chunks_exact_mut(4) {
        // chunk is [R, G, B, A] -> Swap index 0 and 2 -> [B, G, R, A]
        chunk.swap(0, 2);
    }
}

// Blends straight RGBA onto a premultiplied RGBA target slice
#[inline]
pub fn blend_glyph_pixel(dst: &mut [u8], src_r: u8, src_g: u8, src_b: u8, src_a: u8) {
    if src_a == 0 {
        return;
    }
    let src_a = src_a as u32;
    let one_minus_src_a = 255 - src_a;

    // Premultiply source color components
    let src_r_pre = (src_r as u32 * src_a + 127) / 255;
    let src_g_pre = (src_g as u32 * src_a + 127) / 255;
    let src_b_pre = (src_b as u32 * src_a + 127) / 255;

    // Load destination (assumed premultiplied)
    let dst_r = dst[0] as u32;
    let dst_g = dst[1] as u32;
    let dst_b = dst[2] as u32;
    let dst_a = dst[3] as u32;

    // Composite: dst_pre' = src_pre + dst_pre * (1 - src_a)
    let out_r = src_r_pre + (dst_r * one_minus_src_a + 127) / 255;
    let out_g = src_g_pre + (dst_g * one_minus_src_a + 127) / 255;
    let out_b = src_b_pre + (dst_b * one_minus_src_a + 127) / 255;
    let out_a = src_a + (dst_a * one_minus_src_a + 127) / 255;

    dst[0] = out_r.min(255) as u8;
    dst[1] = out_g.min(255) as u8;
    dst[2] = out_b.min(255) as u8;
    dst[3] = out_a.min(255) as u8;
}

// Custom cosmic-text Renderer for tiny-skia surfaces
pub struct CustomPipelineRenderer<'a> {
    pub pixmap: &'a mut PixmapMut<'a>,
    pub font_system: &'a mut FontSystem,
    pub swash_cache: &'a mut SwashCache,
    pub scale: f32,
}

impl<'a> Renderer for CustomPipelineRenderer<'a> {
    fn rectangle(&mut self, x: i32, y: i32, w: u32, h: u32, color: CosmicColor) {
        let rect = if let Some(r) = Rect::from_xywh(
            x as f32 / self.scale,
            y as f32 / self.scale,
            w as f32 / self.scale,
            h as f32 / self.scale,
        ) {
            r
        } else {
            return;
        };

        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(
            color.r(),
            color.g(),
            color.b(),
            color.a(),
        ));

        self.pixmap.fill_rect(
            rect,
            &paint,
            Transform::from_scale(self.scale, self.scale),
            None,
        );
    }

    fn glyph(&mut self, physical_glyph: PhysicalGlyph, color: CosmicColor) {
        let width = self.pixmap.width();
        let height = self.pixmap.height();
        let buffer = self.pixmap.data_mut();

        self.swash_cache.with_pixels(
            self.font_system,
            physical_glyph.cache_key,
            color,
            |x, y, pixel_color| {
                let abs_x = physical_glyph.x + x;
                let abs_y = physical_glyph.y + y;
                
                if abs_x >= 0 && abs_x < width as i32 && abs_y >= 0 && abs_y < height as i32 {
                    let offset = ((abs_y as usize * width as usize) + abs_x as usize) * 4;
                    let dst_pixel = &mut buffer[offset..offset + 4];
                    blend_glyph_pixel(
                        dst_pixel,
                        pixel_color.r(),
                        pixel_color.g(),
                        pixel_color.b(),
                        pixel_color.a(),
                    );
                }
            },
        );
    }
}

pub fn render_ui_frame(
    width: u32,
    height: u32,
    scale: f32,
    prompt_text: &str,
) -> Result<WaylandBuffer, Box<dyn std::error::Error>> {
    // 1. Allocate Shared Memory
    let mut wl_buffer = WaylandBuffer::allocate(width, height)?;
    let slice = wl_buffer.as_slice_mut();

    // 2. Wrap buffer in tiny-skia PixmapMut
    let mut pixmap = PixmapMut::from_bytes(slice, width, height)
        .ok_or("Failed to construct tiny-skia PixmapMut")?;

    // Clear buffer to transparent
    pixmap.fill(SkiaColor::TRANSPARENT);

    // 3. Draw Prompt Window Background and Borders using tiny-skia
    let bounds = Rect::from_xywh(10.0, 10.0, (width as f32 / scale) - 20.0, (height as f32 / scale) - 20.0).unwrap();
    let mut bg_paint = Paint::default();
    bg_paint.set_color(SkiaColor::from_rgba8(30, 30, 34, 255)); // Dark charcoal background
    pixmap.fill_rect(bounds, &bg_paint, Transform::from_scale(scale, scale), None);

    let mut border_paint = Paint::default();
    border_paint.set_color(SkiaColor::from_rgba8(75, 75, 85, 255)); // Slate grey border
    let stroke = Stroke {
        width: 2.0,
        ..Default::default()
    };
    let border_path = PathBuilder::from_rect(bounds);
    pixmap.stroke_path(&border_path, &border_paint, &stroke, Transform::from_scale(scale, scale), None);

    // 4. Set up cosmic-text and swash rendering stack
    let mut font_system = FontSystem::new();
    let mut swash_cache = SwashCache::new();

    // Initialize metrics (15px font size, 22px line height)
    let metrics = Metrics::new(15.0, 22.0);
    let mut buffer = Buffer::new(&mut font_system, metrics);

    // Wrap buffer with FontSystem borrow
    let mut borrowed = buffer.borrow_with(&mut font_system);

    // Constrain text layout width to the interior window size
    let text_padding = 20.0;
    let layout_width = (width as f32 / scale) - 20.0 - (text_padding * 2.0);
    borrowed.set_size(Some(layout_width), None);

    // Set prompt styled attributes
    let attrs = Attrs::new()
        .family(cosmic_text::Family::SansSerif)
        .weight(cosmic_text::Weight::BOLD)
        .color(CosmicColor::rgb(240, 240, 245)); // Off-white text

    borrowed.set_text(prompt_text, &attrs, Shaping::Advanced, None);
    borrowed.shape_until_scroll(false);

    // 5. Construct custom pipeline renderer
    let mut renderer = CustomPipelineRenderer {
        pixmap: &mut pixmap,
        font_system: borrowed.font_system,
        swash_cache: &mut swash_cache,
        scale,
    };

    // Draw shaped text. Offset text start position inside prompt box.
    let text_start_x = (10.0 + text_padding) * scale;
    let text_start_y = (10.0 + text_padding) * scale;
    
    // Resolve layout runs and render text
    for run in borrowed.inner.layout_runs() {
        for glyph in run.glyphs {
            let physical_glyph = glyph.physical((text_start_x, text_start_y + (run.line_y * scale)), scale);
            let glyph_color = glyph.color_opt.unwrap_or(CosmicColor::rgb(240, 240, 245));
            renderer.glyph(physical_glyph, glyph_color);
        }
        // Render underlines, strikethroughs, etc.
        cosmic_text::render_decoration(&mut renderer, &run, CosmicColor::rgb(240, 240, 245));
    }

    // 6. Channel conversion: Swap RGBA to BGRA (ARGB8888 representation)
    swap_red_blue_inplace(wl_buffer.as_slice_mut());

    Ok(wl_buffer)
}
```

### Safety and Optimization Invariants

1.  **Alignment Boundaries**: `tiny-skia` strictly assumes no padding between lines of pixels. The buffer size must exactly match `width * height * 4` bytes. Passing an improperly sized slice to `PixmapMut::from_bytes` will result in `None`.
2.  **Premultiplication Invariant**: Failing to premultiply colors before performing destination blend operations results in color "bleeding" and dark halos around glyph edges. Ensure the `blend_glyph_pixel` logic is strictly implemented as described.
3.  **Shared Memory Lifetime**: Wayland buffers must not be unmapped or closed while actively referenced by the Wayland Compositor. The compositor takes ownership of the memory region until a `wl_buffer.release` event is emitted. The application must double-buffer or delay dropping `WaylandBuffer` until safe.
4.  **CPU Cache Friendly Blending**: Software layout rendering is CPU-bound. Iterate over the lines of the layout buffer linearly. Quantize subpixel positions (`SubpixelBin`) to reduce the total size of `SwashCache` allocations, avoiding memory fragmentation and cache thrashing.
