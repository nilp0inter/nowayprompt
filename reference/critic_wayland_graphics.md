# Adversarial Wayland, Graphics, and XKB Critique of `RUST_REWRITE.md`

This document provides a rigorous, adversarial review of the proposed architecture and implementation plan for `nowayprompt` in `RUST_REWRITE.md`. It identifies critical protocol compliance failures, security vulnerabilities, graphics performance bottlenecks, and structural edge cases, along with concrete mitigations.

---

## 1. Wayland Layer-Shell Protocol Compliance (`zwlr_layer_shell_v1`)

### 1.1 The Configure-Commit Sequence and Initial Surface Commit Violations
*   **The Flaw**: The rewrite plan does not detail the initial surface mapping lifecycle. In the Wayland Layer-Shell protocol, the client **must** commit the surface with *no buffer attached* during the initial layout phase.
*   **Protocol Failure Mode**: If the client attaches a buffer to the `wl_surface` on or before the first commit (prior to receiving the first `zwlr_layer_surface_v1::Event::Configure` event), the compositor will immediately raise a protocol error (often killing the client) or refuse to map the layer surface. On compositors like Sway and Hyprland, this triggers a fatal `WL_SURFACE_ERROR_INVALID_SIZE` or `ZWLR_LAYER_SURFACE_V1_ERROR_ALREADY_CONSTRUCTED` crash.
*   **Concrete Mitigation**: The client must explicitly construct the `wl_surface` and the `zwlr_layer_surface_v1` handle, set layout parameters (anchors, margins, keyboard interactivity), and perform a `wl_surface.commit()` with **no buffer attached**. The client must then wait for the `Configure` event, call `ack_configure(serial)` to acknowledge the size, draw/attach the buffer, damage the surface, and commit.
*   **Correct Lifecycle Implementation**:
    ```rust
    // 1. Setup layer surface properties
    layer_surface.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
    layer_surface.set_size(0, 50);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);

    // 2. Commit WITHOUT buffer attached (mandatory first commit)
    wl_surface.commit();

    // 3. Listen for the initial Configure event
    fn event(...) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                // Acknowledge configuration
                layer_surface.ack_configure(serial);
                
                state.width = width as i32;
                state.height = height as i32;
                
                if !state.configured {
                    state.configured = true;
                    // First draw: render a matching buffer and commit it
                    state.draw_and_attach_frame(qh);
                }
            }
        }
    }
    ```

### 1.2 Mismatched Dimensions and Dynamic Resizing on Subsequent Configure Events
*   **The Flaw**: The reference implementation only draws the frame once when `!state.configured` is true.
*   **Protocol Failure Mode**: Dynamic events such as screen resolution changes, dynamic output scaling, or shifting panels trigger subsequent `Configure` events with new dimensions. If the client fails to adjust its buffer size and draw/commit with the new dimensions, the compositor will either stretch/compress the old buffer (causing blurriness) or crash the client due to size mismatches.
*   **Concrete Mitigation**: The event handler must compare the new configure `width` and `height` to the active buffer's dimensions. If they differ, the client must recreate the SHM pool/buffer at the new size, redraw the UI, attach the new buffer, damage the surface using buffer-local coordinates (`wl_surface.damage_buffer`), and commit.
*   **Correct Handling**:
    ```rust
    zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
        layer_surface.ack_configure(serial);
        let size_changed = state.width != width as i32 || state.height != height as i32;
        state.width = width as i32;
        state.height = height as i32;
        
        if !state.configured || size_changed {
            state.configured = true;
            state.recreate_buffers(qh);
            state.draw_and_attach_frame(qh);
        }
    }
    ```

### 1.3 Asynchronous Buffer Release (`wl_buffer.release` vs. Frame Callbacks)
*   **The Flaw**: The plan assumes double-buffering is sufficient but does not address the decoupling of frame callbacks and buffer release events.
*   **Protocol Failure Mode**: Wayland's `wl_callback::Event::Done` (frame callback) informs the client when the compositor is ready to display the next frame, but it does **not** guarantee that the compositor has released the previously committed buffer. The compositor may keep holding a buffer for multiple frames (e.g. during animations or when cached). If the client writes to a buffer before receiving its `wl_buffer.release` event, it will cause severe graphical tearing, visual corruption, or protocol errors.
*   **Concrete Mitigation**: Track the state of each buffer (`busy: bool`) via `wl_buffer.release` event handlers. If all existing buffers in the double-buffer pool are marked `busy` when a redraw is triggered (e.g. during rapid typing), the client must dynamically allocate a new `ShmBuffer` (triple buffering) rather than overwriting a busy one. Prune idle buffers once they are released.
*   **Correct Buffer Selection**:
    ```rust
    struct BufferPool {
        buffers: Vec<ShmBuffer>,
    }

    impl BufferPool {
        fn get_free_buffer(&mut self, shm: &WlShm, width: i32, height: i32, qh: &QueueHandle<State>) -> &mut ShmBuffer {
            if let Some(idx) = self.buffers.iter().position(|b| !b.busy && b.width == width && b.height == height) {
                return &mut self.buffers[idx];
            }
            // Allocate a new buffer on-demand to prevent tearing/blocking
            let new_buf = ShmBuffer::create(shm, width, height, qh).unwrap();
            self.buffers.push(new_buf);
            self.buffers.last_mut().unwrap()
        }
    }
    ```

### 1.4 Output Geometry & Multi-Monitor Output Handling
*   **The Flaw**: Passing `None` to `get_layer_surface` delegates output placement to the compositor's default output (typically the primary monitor).
*   **Protocol Failure Mode**: On multi-monitor setups, if the user's active cursor/focus is on a secondary monitor, but the prompt maps to the primary monitor, the user will be forced to look away. Worse, because keyboard interactivity is `Exclusive`, the keyboard focus is grabbed globally. If the prompt renders on an inactive or turned-off monitor, the user will type their password blindly into a black screen, presenting a shoulder-surfing/spoofing vulnerability.
*   **Concrete Mitigation**: The client must bind to `wl_output` globals in the registry. It should track output enter/leave events or query seat focus to dynamically spawn the surface on the active output. For high-security pinentry prompts, the client should spawn overlay blocker surfaces on **all** active `wl_output` objects to prevent desktop spoofing or input interception.

### 1.5 Fractional Scaling Support
*   **The Flaw**: The rewrite plan does not account for fractional scaling.
*   **Protocol Failure Mode**: Under high-DPI or fractional scaling setups (e.g., 1.25x or 1.5x scaling on Sway/Hyprland), if the client does not bind to `wp_fractional_scale_manager_v1`, the compositor will scale the client's integer-rendered output. This interpolates pixels, rendering fonts blurry and degrading layout legibility.
*   **Concrete Mitigation**: Bind to the `wp_fractional_scale_manager_v1` global. For the layer surface, create a `wp_fractional_scale_v1` object, listen for the `preferred_scale` event, scale the layout dimensions and font metrics by this fractional scale factor, and set the buffer size accordingly. The client must set `wl_surface.set_buffer_scale(1)` as required by the fractional scaling specification.

---

## 2. Graphics & Font Rendering Pipeline (`cosmic-text`, `tiny-skia`, `fontdb`, `swash`)

### 2.1 The Red-Blue Pixel Channel Swap Bottleneck and SIMD Optimization
*   **The Flaw**: `tiny-skia` outputs premultiplied `RGBA8888` (byte order `[R, G, B, A]`), but little-endian Wayland expects `ARGB8888`/`XRGB8888` (byte order `[B, G, R, A]`). The plan proposes a scalar byte swap: `chunk.swap(0, 2)`.
*   **Performance Failure Mode**: For a high-DPI display (e.g., 3840x2160 at 60Hz), a scalar loop swapping bytes on 8.3 million pixels takes roughly 15-30ms of CPU time per frame. This introduces massive frame latency, input lag, and CPU spikes during redraws, violating the performance target.
*   **Concrete Mitigation**: Implement a vectorized (SIMD) byte swapping loop. In Rust, this can be achieved using the `std::simd` API or by performing bitwise operations on `u32` slices:
    ```rust
    #[inline(always)]
    pub fn swap_rb_simd(buf: &mut [u32]) {
        for pixel in buf.iter_mut() {
            let p = *pixel;
            let r = (p & 0x000000FF) << 16;
            let g = p & 0x0000FF00;
            let b = (p & 0x00FF0000) >> 16;
            let a = p & 0xFF000000;
            *pixel = a | r | g | b;
        }
    }
    ```
    On modern x86_64 compilers, this bitwise loop gets automatically vectorized (using SSE2/AVX2 instructions), reducing the conversion time to less than 1ms.

### 2.2 System-Wide Font Scanning Startup Latency
*   **The Flaw**: The plan suggests calling `fontdb::Database::load_system_fonts()` at startup.
*   **Performance Failure Mode**: System-wide font scanning reads config files (`fonts.conf`) and parses metadata of hundreds of TTF/OTF files. This takes between 200ms and 2 seconds depending on disk type (HDD vs. SSD) and cold vs. warm cache. A password prompt utility must be instantaneous (<20ms startup).
*   **Concrete Mitigation**: DO NOT call `load_system_fonts()`. Instead, bundle a light, high-legibility monospace/sans-serif font (e.g., *DejaVu Sans* or *Fira Mono*) in the binary using `include_bytes!`. Load it directly into `fontdb` via `load_font_data`. This bypasses system I/O entirely, guarantees a fallback font is present, and cuts startup time to <5ms.
    ```rust
    let mut db = fontdb::Database::new();
    const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");
    db.load_font_data(FONT_DATA.to_vec());
    ```

### 2.3 Subpixel Antialiasing and Chromatic Aberration in Software Canvas
*   **The Flaw**: Text rendering libraries (`cosmic-text` and `swash`) support subpixel antialiasing (LCD rendering).
*   **Graphics Failure Mode**: Subpixel antialiasing adjusts individual R, G, B subpixels to increase horizontal resolution. However, software blending subpixel text onto a transparent or dynamically changing background via `tiny-skia` requires separate alpha blending for each color channel. Without knowing the exact monitor subpixel geometry (RGB vs. BGR, vertical vs. horizontal) and implementing a custom subpixel compositor, this results in ugly chromatic aberration (red/blue fringing around text edges).
*   **Concrete Mitigation**: Disable subpixel antialiasing. Force grayscale antialiasing for font rendering. Grayscale is layout-agnostic, works perfectly on transparent surfaces, and is computationally cheaper.

### 2.4 Text Layout and Cache Re-allocations during Password Entry
*   **The Flaw**: Shaping and measuring the text buffer (`cosmic-text::Buffer`) on every keystroke.
*   **Performance Failure Mode**: As the user types, characters are appended or replaced by `*`. If the application runs the full `cosmic-text` shaping and layout pipeline on every keypress, it re-allocates memory and re-calculates kerning, ligatures, and font fallback. For long inputs, this creates lag.
*   **Concrete Mitigation**: Since the password mask character (e.g., `•` or `*`) is uniform, shape the single mask character exactly once at startup. Cache its glyph layout and rasterized pixels. When drawing the input field, manually calculate the cursor position and blit the cached glyph iteratively across the canvas using a simple loop. This bypasses the layout shaper entirely during typing.

---

## 3. XKB Keyboard Mapping & File Descriptor Safety

### 3.1 Security Critical: SIGBUS Page Faults on Memory-Mapped Keymap FDs
*   **The Flaw**: The rewrite plan uses `memmap2` to map the `wl_keyboard.keymap` file descriptor (`MAP_PRIVATE` by default).
*   **Security & Stability Failure Mode**: Under the Wayland protocol, the compositor passes a file descriptor to a keymap file. If the compositor crashes or truncates/closes this shared memory file while the client is reading it, a page fault occurs. This triggers a `SIGBUS` (Signal 7 - Bus Error) on the client process. By default, `SIGBUS` terminates the client process immediately. Because termination is abrupt, Rust's `Drop` implementation for `SecretBuffer` is bypassed. The plaintext password remains in the system RAM, unzeroed, creating a severe security leak.
*   **Concrete Mitigation**: DO NOT use memory mapping for the keymap file descriptor. The keymap data is small (typically <10KB). Instead, read the raw file descriptor directly using `std::io::Read` into a heap-allocated buffer or string, and parse it via `Keymap::new_from_string`. This eliminates the `SIGBUS` risk entirely.
    ```rust
    use std::os::fd::FromRawFd;
    use std::io::Read;

    pub fn safe_parse_keymap(
        context: &Context,
        raw_fd: std::os::unix::io::RawFd,
        size: usize,
    ) -> Result<Keymap, std::io::Error> {
        let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
        let mut buffer = vec![0u8; size];
        file.read_exact(&mut buffer)?;
        
        let keymap_str = std::str::from_utf8(&buffer)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
            .trim_end_matches('\0');
            
        Keymap::new_from_string(
            context,
            keymap_str.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "xkbcommon failed to compile keymap",
        ))
    }
    ```

### 3.2 Linux Evdev to XKB Keycode Shift Offset
*   **The Flaw**: A common trap is using the raw keycode sent by `wl_keyboard::Event::Key` directly.
*   **Input Failure Mode**: Linux `evdev` keycodes are 0-indexed. The X11/XKB standard offsets keycodes by **8**. If the +8 shift is omitted, all keys map incorrectly (e.g., the Esc key maps to an invalid action, and Backspace deletes characters instead of mapping correctly), rendering input non-functional.
*   **Concrete Mitigation**: The raw `key` code from the Wayland event must be incremented by 8 before query:
    ```rust
    let xkb_keycode = xkb::Keycode::new(raw_key + 8);
    let keysym = xkb_state.key_get_one_sym(xkb_keycode);
    ```

### 3.3 Modifier State Desynchronization
*   **The Flaw**: Tracking modifier states using `state.update_key` on modifier keycodes.
*   **Input Failure Mode**: If the client updates its XKB modifier state by calling `update_key` for physical keypresses of Ctrl, Shift, Alt, etc., it will conflict with the compositor's authoritative `wl_keyboard.modifiers` event updates. This leads to stuck keys or incorrect character capitalization (e.g. shift locks).
*   **Concrete Mitigation**: The client must NOT update its XKB keyboard modifier state using physical key events. It must rely exclusively on the `wl_keyboard.modifiers` event sent by the compositor, updating its state with `state.update_mask` using the values supplied by the compositor.
