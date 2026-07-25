# Stage 3 Wayland Frontend — Shared Contract & Verified APIs

You are implementing part of the `nowayprompt` Rust rewrite (port of legacy Zig
`wayprompt`). The **foundation is committed** (HEAD `ee1e7d3`) and compiles clean.
Target: 100% **behavioral** parity with `reference/legacy/src/Wayland.zig`
(pixel parity is OUT of scope). Pure-Rust stack; the sole C exception is
`xkbcommon` (dlopens `libxkbcommon.so`).

Repo root: `/home/nil/Projects/github.com/nilp0inter/nowayprompt`
Legacy parity reference: `reference/legacy/src/Wayland.zig` (1761 LOC, in-tree).

## Cargo deps (already in Cargo.toml — do NOT change versions)
- `wayland-client = "0.31"` (0.31.15; pure-Rust `rs` backend)
- `wayland-protocols-wlr = { version = "0.3", features = ["client"] }` (0.3.12) — **`client` feature is REQUIRED**
- `wayland-protocols = { version = "0.31", features = ["client", "staging", "unstable"] }` (0.31.2) — `staging`+`unstable` needed for cursor_shape/fractional_scale
- `tiny-skia = "0.11"` (0.11.4), `cosmic-text = "0.12"` (0.12.1)
- `xkbcommon = { version = "0.8", features = ["x11"] }` (0.8.0)
- existing: `libc`, `zeroize`, `memmap2 = "0.9"`, `signal-hook`

## Frozen architecture (do NOT reshape)
- `WaylandState` (in `src/frontend/wayland/mod.rs`) is the wayland-client
  dispatch `State`. It owns all mutable protocol state and every
  `Dispatch<I, U>` impl. Fields (`pub(crate)`): `compositor: Option<WlCompositor>`,
  `shm: Option<WlShm>`, `layer_shell: Option<ZwlrLayerShellV1>`,
  `cursor_shape_manager: Option<WpCursorShapeManagerV1>`,
  `fractional_scale_manager: Option<WpFractionalScaleManagerV1>`,
  `seats: Vec<Seat>`, `surface: Option<Surface>`, `buffer_pool: BufferPool`,
  private: `sync`, `delayed_mode`, `mode: InterfaceMode`,
  `exit_reason: Option<ExitReason>`, `config_ptr`, `secbuf_ptr`.
- `Wayland` is the thin `Frontend` wrapper (owns `conn`, `queue`, `qh`, `state`).
- `ExitReason` is a **private** enum in mod.rs: `UserOk | UserAbort | UserNotOk | Error(FrontendError)`.
  Child modules (render.rs, input.rs) CAN name it and call the private
  `WaylandState::abort(reason)` (Rust: children see ancestor private items).
- Single-threaded read model: `handle_event` does `prepare_read().read()` then
  `dispatch_pending`; `no_event` is a no-op. (Deviation from legacy's
  multi-thread prepare/read/cancel split — documented in mod.rs.)

## Frozen render/input contract (in render.rs — DO NOT change these signatures)
```rust
pub struct HotSpot { pub effect: HotSpotEffect, pub x: u32, pub y: u32, pub width: u32, pub height: u32 }
pub enum HotSpotEffect { Cancel, NotOk, Ok }
impl HotSpot {
    pub fn contains_point(&self, x: u32, y: u32) -> bool;
    pub fn act(&self, state: &mut WaylandState);   // aborts with the mapped ExitReason
}
pub struct Surface { pub configured: bool, pub width: u32, pub height: u32, pub scale: u32, pub hotspots: Vec<HotSpot> }
impl Surface {
    pub fn new(state: &mut WaylandState, qh: &QueueHandle<WaylandState>, compositor: &WlCompositor,
               layer_shell: &ZwlrLayerShellV1, shm: &WlShm,
               fractional: Option<&WpFractionalScaleManagerV1>, mode: InterfaceMode)
        -> Result<Self, FrontendError>;
    pub fn deinit(self);
    pub fn hotspot_from_point(&self, x: u32, y: u32) -> Option<&HotSpot>;
    pub fn render(&mut self, state: &mut WaylandState, qh: &QueueHandle<WaylandState>) -> Result<(), FrontendError>;
}
pub fn render_surface(state: &mut WaylandState, qh: &QueueHandle<WaylandState>); // take/render/put-back + abort on err
```
`Seat` (in input.rs): `pub struct Seat { pub wl_seat: WlSeat }`, `Seat::new(wl_seat: WlSeat) -> Self`, `Seat::deinit(self)`.
`Buffer`/`BufferPool` (in shm.rs): `BufferPool::next_buffer(&mut self, shm: &WlShm, qh, width: u32, height: u32) -> Result<usize, io::Error>` (returns slot index), `get_mut(idx) -> Option<&mut Buffer>`, `Buffer { wl_buffer: Option<WlBuffer>, mmap: Option<MmapMut>, width, height, busy: bool }`.

## VERIFIED wayland-client 0.31 API facts (save yourself the discovery)
- Import `wayland_client::Proxy` to call `WlFoo::interface().name` (needed for registry interface matching).
- `Connection::connect_to_env() -> Result<Connection, ConnectError>`; `conn.display() -> WlDisplay`;
  `conn.new_event_queue::<State>() -> EventQueue<State>`; `queue.handle() -> QueueHandle<State>` (Clone).
- `queue.prepare_read() -> Option<ReadEventsGuard>`; `guard.read() -> Result<usize, WaylandError>` (consumes guard);
  `queue.dispatch_pending(&mut state) -> Result<usize, DispatchError>`; `conn.flush() -> Result<(), WaylandError>`;
  `conn.as_fd().as_raw_fd()` for the pollable fd.
- `display.get_registry(&qh, ()) -> WlRegistry`; `display.sync(&qh, ()) -> WlCallback`.
- `registry.bind::<I, U, D>(name, version, &qh, udata) -> I`.
- Globals `WlCompositor`/`WlShm`/`WlSeat` have **NO destroy()** — just drop them. `WlRegistry` has no destroy — drop it. `WlBuffer`/`WlShmPool`/`ZwlrLayerSurfaceV1`/`WlSurface` have `.destroy()`. `WlKeyboard`/`WlPointer`/`WlTouch`/`WlSeat` have `.release()`.
- `shm.create_pool(fd.as_fd(), size: i32, &qh, ())` — **first arg is `BorrowedFd` via `.as_fd()`, NOT a raw i32**. `pool.create_buffer(offset: i32, w: i32, h: i32, stride: i32, Format::Argb8888, &qh, udata)`.
- `Dispatch` trait: `fn event(state: &mut State, proxy: &I, event: <I as Proxy>::Event, data: &U, conn: &Connection, qh: &QueueHandle<State>)`. For an interface with no events, the `Event` type is uninhabited — name the param `_event` and leave the body empty.
- Module paths (these exact paths work):
  - `wayland_client::protocol::wl_{registry,compositor,shm,seat,callback,surface,buffer,keyboard,pointer,touch,shm_pool}::*`
  - `wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{ZwlrLayerShellV1, ...}` and `...::zwlr_layer_surface_v1::{ZwlrLayerSurfaceV1, Layer, ...}`
  - `wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1` and `...::wp_cursor_shape_device_v1::{WpCursorShapeDeviceV1, Shape}`
  - `wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1` and `...::wp_fractional_scale_v1::WpFractionalScaleV1`
- **Duplicate Dispatch impl = compile error.** mod.rs currently has a STUB `impl Dispatch<WlSeat, ()> for WaylandState`. Whoever implements real seat input MUST delete that stub from mod.rs and own the real impl in input.rs.

## Conventions
- 4-space indent is NOT used here — the repo uses standard `rustfmt` (4 spaces is the rustfmt default; run `cargo fmt`). Lines ≤ 100. Run `cargo fmt` and `cargo clippy -- -D warnings` before finishing (the pre-commit hook enforces both).
- Match existing doc-comment style (`//!` module, `///` items, cite `Wayland.zig:LINE` parity refs).
- SecretBuffer API (`crate::secret::SecretBuffer`): `append_slice(&[u8]) -> Result`, `delete_backwards()`, `reset() -> Result`, `len() -> usize`, `slice() -> Option<&[u8]>`. Access via `state.secbuf()` (private method, visible to child modules).
- Config (`crate::config::Config`): `config.labels` (title/description/prompt/err_message/ok/not_ok/cancel: Option<String>), `config.wayland_ui` (WaylandUi: padding/sizes/fonts), `config.wayland_colours` (WaylandColours: 16-bit premultiplied `Colour {red,green,blue,alpha}` fields), `config.wayland_display`. Access via `state.config()`.

## Verify before coding
Crate sources are at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/<crate>-<ver>/`.
READ the actual API (grep `pub fn`/`pub struct`) before writing code against an
unfamiliar crate (esp. `xkbcommon`, `cosmic-text`, `tiny-skia`). Iterate
`cargo build` until YOUR module compiles.

## VERIFIED render API facts (cosmic-text 0.12.1 + tiny-skia 0.11.4)
**Bundled fonts (already copied to `assets/`):** `assets/DejaVuSans.ttf`, `assets/DejaVuSans-Bold.ttf`.
From `src/frontend/wayland/render.rs`, reference them as `include_bytes!("../../../assets/DejaVuSans.ttf")`.

**Font setup (design D7 — NO system font scan):**
```rust
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Attrs, Family, Shaping, Color as CtColor};
let font_data: &'static [u8] = include_bytes!("../../../assets/DejaVuSans.ttf");
let mut font_system = FontSystem::new_with_fonts([
    fontdb::Source::Binary(std::sync::Arc::new(font_data.to_vec())),
]);
```
`fontdb` is a transitive dep of cosmic-text; if `fontdb::Source` is not directly nameable, add `fontdb = "0.16"` to Cargo.toml (cosmic-text 0.12 uses fontdb 0.16). Verify with `cargo tree -i fontdb`.

**Text measure + layout:**
```rust
let mut buffer = Buffer::new(&mut font_system, Metrics::new(font_size_f32, line_height_f32));
buffer.set_size(&mut font_system, Some(max_width), None);
buffer.set_text(&mut font_system, text, Attrs::new().family(Family::SansSerif), Shaping::Advanced);
buffer.shape_until_scroll(&mut font_system, false);
// metrics: iterate buffer.layout_runs() -> run.line_w (f32 line width), run.line_height; total height = sum of line_height
```

**Text draw (glyph rasterization via swash):**
```rust
let mut swash_cache = SwashCache::new();
for run in buffer.layout_runs() {
    for glyph in run.glyphs {
        let physical = glyph.physical((origin_x_f32, origin_y_f32 + run.line_y), scale_f32);
        // physical: PhysicalGlyph { cache_key, x: i32, y: i32 }
        if let Some(image) = swash_cache.get_image_uncached(&mut font_system, physical.cache_key) {
            // image: SwashImage { content: Content (Mask|Color), placement: Placement { left: i32, top: i32, width: u32, height: u32 }, data: Vec<u8> }
            let gx = physical.x + image.placement.left;
            let gy = physical.y - image.placement.top;
            // For Content::Mask: data is 1 byte/pixel alpha coverage. Blend text_color with that alpha onto the pixmap (premultiplied).
        }
    }
}
```
Verify the exact `SwashImage`/`Placement`/`Content` field names by reading `~/.cargo/registry/src/.../cosmic-text-0.12.1/src/swash.rs` (they may be re-exported from the `swash` crate).

**tiny-skia rasterization:**
```rust
use tiny_skia::{PixmapMut, Paint, Color as SkColor, Transform, FillRule, PathBuilder, Stroke};
// Wrap the SHM buffer (premultiplied RGBA8888) — buffer.mmap is a MmapMut (DerefMut to [u8]):
let mut pixmap = PixmapMut::from_bytes(&mut mmap[..], width, height).unwrap();
pixmap.fill(SkColor::TRANSPARENT); // clear
let mut paint = Paint::default();
paint.shader = tiny_skia::Shader::SolidColor(SkColor::from_rgba8(r, g, b, a)); // NOTE: from_rgba8 takes STRAIGHT alpha and premultiplies internally
paint.anti_alias = true; // grayscale AA (design D7)
// filled rect:
pixmap.fill_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap(), &paint, Transform::identity(), None);
// filled path (rounded corners): build with PathBuilder (move_to/line_to/quad_to/close or push_rect), then fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None)
// stroked border: pixmap.stroke_path(&path, &paint, &Stroke { width: border_f32, ..Default::default() }, Transform::identity(), None)
```

**Pixel format + R/B swap (design D6):** tiny-skia writes premultiplied RGBA8888 (`[R,G,B,A]` bytes). Wayland `wl_shm::Format::Argb8888` on little-endian is `[B,G,R,A]` bytes. Before commit, swap R and B in-place over the whole pixel buffer:
```rust
pub fn swap_rb(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}
```
(A SIMD `u32` bitwise form is a later optimization; the scalar form is correct and the nixosTest is geometry-only.)

**Config colour conversion:** `crate::config::Colour { red, green, blue, alpha: u16 }` is 16-bit premultiplied. Convert to tiny-skia: `r8 = (red >> 8) as u8` etc. — but note these are PREMULTIPLIED. tiny-skia's `Color::from_rgba8` expects STRAIGHT alpha and premultiplies; to avoid double-premultiply, either un-premultiply first, or build a premultiplied color directly via `tiny_skia::PremultipliedColorU8::from_rgba(r,g,b,a)` and set pixels. For fill Paint, simplest: un-premultiply the 16-bit value to straight 8-bit, then `Color::from_rgba8`.

## Surface private fields to ADD (the public contract fields stay frozen)
The render impl OWNS render.rs and may add PRIVATE fields to `Surface`, e.g.:
`wl_surface: Option<WlSurface>`, `layer_surface: Option<ZwlrLayerSurfaceV1>`, `fractional_scale: Option<WpFractionalScaleV1>`, `font_system: FontSystem`, `swash_cache: SwashCache`, the seven `Option<TextView>` labels, `mode: InterfaceMode`, and config snapshots. Add `Dispatch<WlSurface,()>`, `Dispatch<ZwlrLayerSurfaceV1,()>`, `Dispatch<WpFractionalScaleV1,()>` for `WaylandState` IN render.rs (orphan rule allows it; WaylandState is crate-local). Do NOT edit mod.rs.