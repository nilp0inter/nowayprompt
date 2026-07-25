# Wayland & Layer-Shell Rust API Reference Manual
### Crate Versions Covered: `wayland-client` v0.31+ & `wayland-protocols-wlr` v0.3+

This reference manual provides a detailed API specification and implementation guide for building Wayland desktop shell components in Rust using `wayland-client` (v0.31.0+) and the wlroots layer shell extension `zwlr_layer_shell_v1` (v0.3.0+) from `wayland-protocols-wlr`.

---

## Table of Contents
1. [Core Architecture: Connection, EventQueue, and Dispatch Pattern](#1-core-architecture-connection-eventqueue-and-dispatch-pattern)
2. [Global Registry Binding](#2-global-registry-binding)
3. [Shared Memory (wl_shm) Management & Double-Buffering](#3-shared-memory-wl_shm-management--double-buffering)
4. [Layer-Shell Surface Lifecycle (zwlr_layer_shell_v1)](#4-layer-shell-surface-lifecycle-zwlr_layer_shell_v1)
5. [Input Device Event Listeners (Seat, Pointer, and Keyboard)](#5-input-device-event-listeners-seat-pointer-and-keyboard)
6. [Complete Implementation Skeleton](#6-complete-implementation-skeleton)

---

## 1. Core Architecture: Connection, EventQueue, and Dispatch Pattern

Wayland is an asynchronous, object-oriented, protocol-driven IPC system. In `wayland-client` v0.31+, the Rust bindings leverage a strongly typed, safe state machine pattern. 

### Core Components

#### `Connection`
The `Connection` struct represents the active Unix domain socket connection to the Wayland compositor.
- **Connection Methods:**
  - `Connection::connect_to_env()`: Inspects the environment (specifically `$WAYLAND_DISPLAY`) and establishes the connection. Returns a `Result<Connection, ConnectError>`.
  - `Connection::display(&self)`: Returns the `WlDisplay` proxy representing the display singleton.

#### `EventQueue`
The `EventQueue` is a thread-local queue that buffers incoming Wayland events from the socket and dispatches them to your application.
- **Creation:** `connection.new_event_queue()`
- **Dispatching Methods:**
  - `event_queue.blocking_dispatch(&mut self, state: &mut State)`: Blocks until events are available on the socket, reads them, and dispatches them to the registered callbacks.
  - `event_queue.dispatch_pending(&mut self, state: &mut State)`: Dispatches any events already buffered in memory without blocking on socket I/O.

#### `Proxy`
A `Proxy` is a Rust wrapper around a Wayland protocol object. Every generated proxy struct (like `WlSurface`, `WlRegistry`, etc.) implements the `Proxy` trait. Proxies are reference-counted handles; cloning them simply creates another handle pointing to the same server-side object.

#### `Dispatch` Trait
Instead of registering closure-based callbacks on individual objects, `wayland-client` v0.31+ implements event handling by routing all events to a centralized application `State` struct. This is accomplished via the `Dispatch` trait.

##### Trait Definition:
```rust
pub trait Dispatch<I: Proxy, U>: Sized {
    fn event(
        state: &mut Self,
        proxy: &I,
        event: I::Event,
        data: &U,
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    );
}
```

##### Parameter Breakdown:
1. `state: &mut Self`: A mutable reference to your application state struct. Allows you to modify your custom application state directly in response to events.
2. `proxy: &I`: The proxy object that received the event.
3. `event: I::Event`: The generated enum containing the variant and payload of the specific event.
4. `data: &U`: User data associated with this proxy object during instantiation or binding.
5. `conn: &Connection`: A reference to the connection.
6. `qhandle: &QueueHandle<Self>`: A handle to register new proxy objects created during event handling.

##### The `delegate_dispatch!` Macro:
If you implement `Dispatch` on your state struct directly, you must use the `delegate_dispatch!` macro to link your state to the internal dispatch mechanics of `wayland-client`:
```rust
wayland_client::delegate_dispatch!(State: [Interface: UserData] => State);
```
This is required even for self-delegation (where `State` dispatches to itself) to implement the internal glue traits that the event loop uses to locate handlers.

---

## 2. Global Registry Binding

To interact with the compositor, clients must discover available global singletons (e.g., compositor, shared memory, seat, shell interfaces) by binding to the registry.

### The Binding Sequence

1. **Get Registry:** Call `display.get_registry(&qhandle, ())` to obtain a `WlRegistry` proxy and associate it with a user data token (usually `()`).
2. **Listen for Globals:** The registry emits `wl_registry::Event::Global` events when the connection is established.
3. **Bind Interfaces:** For each global you require, call `registry.bind::<Interface, _, _>(name, version, &qhandle, ())`.

### Registry Dispatch Implementation

```rust
use wayland_client::{
    protocol::wl_registry,
    Connection, Dispatch, QueueHandle,
};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm, wl_seat::WlSeat};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

struct State {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    layer_shell: Option<ZwlrLayerShellV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    // Bind compositor (v4 is standard for modern clients)
                    state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, 4, qh, ()));
                }
                "wl_shm" => {
                    // Bind shared memory interface
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, 1, qh, ()));
                }
                "wl_seat" => {
                    // Bind seat input interface
                    state.seat = Some(registry.bind::<WlSeat, _, _>(name, 7, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    // Bind wlroots layer shell interface
                    state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, 4, qh, ()));
                }
                _ => {}
            }
        }
    }
}

wayland_client::delegate_dispatch!(State: [wl_registry::WlRegistry: ()] => State);
```

---

## 3. Shared Memory (`wl_shm`) Management & Double-Buffering

Wayland requires that clients allocate and render into buffers, then share these buffers with the compositor. For software-rendered graphics, this is done using shared memory (`wl_shm`).

### Shared Memory Allocation (`memfd_create`)

Linux's `memfd_create` is the preferred syscall for shared memory because it creates an anonymous file descriptor residing entirely in RAM without polluting `/dev/shm` or requiring disk writes.

```rust
use std::fs::File;
use std::os::fd::FromRawFd;

fn create_shm_file(size: usize) -> std::io::Result<File> {
    // memfd_create is accessed via libc or nix crate
    let name = std::ffi::CString::new("wayland-shm").unwrap();
    
    // MFD_CLOEXEC prevents fd leaks to child processes
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr(),
            libc::MFD_CLOEXEC,
        )
    };
    
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    
    // Resize the memory file to the target size
    let ret = unsafe { libc::ftruncate(fd as i32, size as libc::off_t) };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd as i32) };
        return Err(err);
    }
    
    // Wrap in standard std::fs::File for safety
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}
```

### Memory Mapping & Creating Buffers

To write pixels into the shared memory file, the client maps the file to its address space via `mmap`. The compositor maps the same file descriptor to read pixels.

```rust
use wayland_client::protocol::{wl_shm, wl_shm_pool, wl_buffer};
use std::os::fd::AsFd;

struct ShmBuffer {
    wl_buffer: wl_buffer::WlBuffer,
    ptr: *mut u8,
    size: usize,
    busy: bool,
}

impl ShmBuffer {
    fn create(
        shm: &wl_shm::WlShm,
        width: i32,
        height: i32,
        qh: &QueueHandle<State>,
    ) -> Result<Self, std::io::Error> {
        let stride = width * 4; // 4 bytes per pixel (XRGB8888)
        let size = (stride * height) as usize;
        
        let file = create_shm_file(size)?;
        
        // Map to client's address space for rendering
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_fd().as_raw_fd(),
                0,
            )
        };
        
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        
        // Create the Wayland pool from the file descriptor
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        
        // Create the wl_buffer referencing the pool
        let wl_buffer = pool.create_buffer(
            0, // offset
            width,
            height,
            stride,
            wl_shm::Format::Xrgb8888,
            qh,
            (),
        );
        
        Ok(Self {
            wl_buffer,
            ptr: ptr as *mut u8,
            size,
            busy: false,
        })
    }
}
```

### Double-Buffering & Buffer Release Lifecycle

Compositors read directly from your shared memory buffer. If you write to a buffer while the compositor is reading it, graphical tearing occurs. Therefore, clients must use double-buffering.

1. **Busy State:** A buffer is marked `busy = true` when attached to a surface and committed.
2. **Release Event:** The client must listen to `wl_buffer::Event::Release`. When received, the compositor is finished with the buffer and it can be safely reused (`busy = false`).

```rust
impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        wl_buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            // Find buffer in state and release it
            if let Some(ref mut buf) = state.buffer_pool.iter_mut().find(|b| b.wl_buffer == *wl_buffer) {
                buf.busy = false;
            }
        }
    }
}

wayland_client::delegate_dispatch!(State: [wl_buffer::WlBuffer: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_shm_pool::WlShmPool: ()] => State);
```

### The Frame Callback Loop

To animate smoothly, the client requests a frame callback from the compositor using `wl_surface.frame()`. This informs the client when the compositor is ready to display the next frame, avoiding redundant CPU rendering cycles.

```rust
use wayland_client::protocol::{wl_surface, wl_callback};

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { callback_data: _ } = event {
            // Compositor is ready. Render the next frame.
            state.draw_frame(qh);
        }
    }
}

wayland_client::delegate_dispatch!(State: [wl_callback::WlCallback: ()] => State);
```

---

## 4. Layer-Shell Surface Lifecycle (`zwlr_layer_shell_v1`)

The Layer-Shell protocol allows clients to display surfaces in desktop layer zones: Background, Bottom, Top, and Overlay. This is standard for panels, lockscreens, and overlays.

### Initializing the Layer Surface

To map a layer surface, you must obtain a generic `wl_surface::WlSurface` from the compositor, and pass it to `ZwlrLayerShellV1::get_layer_surface`.

```rust
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

// Create a surface
let wl_surface = compositor.create_surface(&qh, ());

// Convert to layer surface
let layer_surface = layer_shell.get_layer_surface(
    &wl_surface,
    None, // Target monitor output (None defaults to active/primary)
    Layer::Top, // Display layer
    "wayprompt-namespace".to_string(), // Namespace identifier
    &qh,
    (),
);
```

### Surface Configuration

Once initialized, the shell surface is configured using double-buffered requests. You must specify the dimensions, screen anchor points, margins, and keyboard behaviors.

```rust
// Set margins relative to screen edges (top, right, bottom, left)
layer_surface.set_margin(10, 10, 10, 10);

// Anchor to top and stretch left-to-right
layer_surface.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);

// Request a specific size. Setting size to 0 on an anchored edge lets 
// the compositor stretch it dynamically.
layer_surface.set_size(0, 50); 

// Keyboard interaction modes: None, Exclusive, OnDemand
layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
```

### The Configure-Commit Sequence (Crucial Protocol Rule)

A common point of failure is committing a buffer to a layer surface immediately after creation. Layer surfaces **must** follow the configure-commit sequence:

1. **Initial Commit:** The client sets the surface configuration parameters (anchor, size, margins) and commits the surface with **no buffer attached**:
   ```rust
   wl_surface.commit(); // Triggers the layout engine on the compositor
   ```
2. **Configure Event:** The compositor calculates the surface layout and sends a `ZwlrLayerSurfaceV1::Event::Configure` event back to the client detailing the finalized width and height.
3. **Acknowledge Configure:** The client receives the event, acknowledges it via `ack_configure(serial)`, renders a matching buffer, attaches it, and commits the state.

```rust
impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                // Acknowledge the layout change
                layer_surface.ack_configure(serial);
                
                // Track sizes
                state.width = width as i32;
                state.height = height as i32;
                
                if !state.configured {
                    state.configured = true;
                    // First render: attach a buffer now that we have layout dimensions
                    state.draw_frame(qh);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}

wayland_client::delegate_dispatch!(State: [ZwlrLayerSurfaceV1: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_surface::WlSurface: ()] => State);
wayland_client::delegate_dispatch!(State: [ZwlrLayerShellV1: ()] => State);
```

---

## 5. Input Device Event Listeners (Seat, Pointer, and Keyboard)

The `wl_seat` interface handles user input. Clients must read the seat capabilities to bind pointer (mouse) and keyboard devices safely.

### Capabilities Matching & Initialization

When you bind `WlSeat`, it emits a `Capabilities` event. The capabilities argument contains flags indicating what hardware devices are attached.

```rust
use wayland_client::protocol::wl_seat::{Capability, WlSeat};
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_keyboard::WlKeyboard;

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            // Check for mouse pointer capability
            let has_pointer = capabilities.contains(Capability::Pointer);
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            } else if !has_pointer && state.pointer.is_some() {
                if let Some(pointer) = state.pointer.take() {
                    pointer.release(); // Free pointer
                }
            }

            // Check for keyboard capability
            let has_keyboard = capabilities.contains(Capability::Keyboard);
            if has_keyboard && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            } else if !has_keyboard && state.keyboard.is_some() {
                if let Some(keyboard) = state.keyboard.take() {
                    keyboard.release(); // Free keyboard
                }
            }
        }
    }
}

wayland_client::delegate_dispatch!(State: [WlSeat: ()] => State);
```

### Pointer Event Handling

The pointer listener intercepts mouse updates. Events are framed atomically; you should cache updates (like motion coordinate changes) and apply them when you receive the `Frame` event.

```rust
use wayland_client::protocol::wl_pointer::{self, ButtonState};

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _pointer: &WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface_x, surface_y, .. } => {
                state.mouse_x = surface_x;
                state.mouse_y = surface_y;
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                state.mouse_x = surface_x;
                state.mouse_y = surface_y;
            }
            wl_pointer::Event::Button { button, state: click_state, .. } => {
                if button == 272 { // Left Click (BTN_LEFT)
                    state.mouse_clicked = click_state == ButtonState::Pressed;
                }
            }
            wl_pointer::Event::Leave { .. } => {
                state.mouse_inside = false;
            }
            wl_pointer::Event::Frame => {
                // Apply accumulated pointer coordinates/actions atomically here
            }
            _ => {}
        }
    }
}

wayland_client::delegate_dispatch!(State: [WlPointer: ()] => State);
```

### Keyboard Event Handling

Key events supply a raw scan code. To determine ASCII characters or key names, you must parse the layout via `xkbcommon` using the descriptor provided in the `Keymap` event.

```rust
use wayland_client::protocol::wl_keyboard::{self, KeyState};

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _keyboard: &WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                // Initialize xkbcommon context using fd & size
                // (e.g. mmap fd with length 'size' and parse keymap strings)
            }
            wl_keyboard::Event::Enter { .. } => {
                state.has_focus = true;
            }
            wl_keyboard::Event::Leave { .. } => {
                state.has_focus = false;
            }
            wl_keyboard::Event::Key { key, state: key_state, .. } => {
                // key is raw keycode. Add +8 to translate to XKB keycodes.
                let pressed = key_state == KeyState::Pressed;
                if pressed && key == 1 { // Escape key (KEY_ESC)
                    state.running = false;
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, .. } => {
                // Update modifiers in your xkbcommon state machine
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                // Configure key repeat timers
            }
            _ => {}
        }
    }
}

wayland_client::delegate_dispatch!(State: [WlKeyboard: ()] => State);
```

---

## 6. Complete Implementation Skeleton

This standalone skeleton demonstrates a compile-ready desktop panel anchored to the top of the display. It connects, binds protocols, configures double-buffered shared memory, sets up the layer shell, and processes events in a blocking event loop.

```rust
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use wayland_client::{
    protocol::{wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

// --- Shared Memory Allocator ---
fn create_shm_file(size: usize) -> std::io::Result<File> {
    let name = std::ffi::CString::new("wayprompt-shm").unwrap();
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr(),
            libc::MFD_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let ret = unsafe { libc::ftruncate(fd as i32, size as libc::off_t) };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd as i32) };
        return Err(err);
    }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

// --- Graphical Buffer Representation ---
struct Buffer {
    wl_buffer: wl_buffer::WlBuffer,
    ptr: *mut u8,
    size: usize,
    busy: bool,
}

impl Buffer {
    fn new(shm: &wl_shm::WlShm, width: i32, height: i32, qh: &QueueHandle<State>) -> Self {
        let stride = width * 4;
        let size = (stride * height) as usize;
        let file = create_shm_file(size).expect("Failed to allocate SHM file");
        
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "Failed to map memory");
        
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let wl_buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, ());
        
        Self {
            wl_buffer,
            ptr: ptr as *mut u8,
            size,
            busy: false,
        }
    }
}

// --- Application State ---
struct State {
    running: bool,
    configured: bool,
    width: i32,
    height: i32,
    
    // Globals
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    seat: Option<wl_seat::WlSeat>,
    layer_shell: Option<ZwlrLayerShellV1>,
    
    // Proxies
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    
    // Double-buffered framebuffers
    buffers: Vec<Buffer>,
}

impl State {
    fn new() -> Self {
        Self {
            running: true,
            configured: false,
            width: 0,
            height: 0,
            compositor: None,
            shm: None,
            seat: None,
            layer_shell: None,
            surface: None,
            layer_surface: None,
            pointer: None,
            keyboard: None,
            buffers: Vec::new(),
        }
    }

    fn draw_frame(&mut self, qh: &QueueHandle<Self>) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        // Allocate double-buffers if they don't exist
        if self.buffers.is_empty() {
            let shm = self.shm.as_ref().unwrap();
            self.buffers.push(Buffer::new(shm, self.width, self.height, qh));
            self.buffers.push(Buffer::new(shm, self.width, self.height, qh));
        }

        // Find an idle buffer
        let buffer_index = self.buffers.iter().position(|b| !b.busy).expect("All buffers are busy");
        let buffer = &mut self.buffers[buffer_index];

        // Draw basic color pattern (e.g., solid gray header panel)
        unsafe {
            let slice = std::slice::from_raw_parts_mut(buffer.ptr as *mut u32, buffer.size / 4);
            for pixel in slice.iter_mut() {
                *pixel = 0xFF333333; // Dark charcoal gray (ARGB)
            }
        }

        let wl_surface = self.surface.as_ref().unwrap();
        
        // Request the next frame callback before committing
        wl_surface.frame(qh, ());
        
        // Attach, damage the whole region, and commit the state
        buffer.busy = true;
        wl_surface.attach(Some(&buffer.wl_buffer), 0, 0);
        wl_surface.damage_buffer(0, 0, self.width, self.height);
        wl_surface.commit();
    }
}

// --- Event Handlers (Dispatch Implementations) ---

// 1. Registry Handler
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, 4, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ())),
                "wl_seat" => state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 7, qh, ())),
                "zwlr_layer_shell_v1" => state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, 4, qh, ())),
                _ => {}
            }
        }
    }
}

// 2. Layer Shell Handler
impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer_surface.ack_configure(serial);
                state.width = width as i32;
                state.height = height as i32;
                
                if !state.configured {
                    state.configured = true;
                    state.draw_frame(qh);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}

// 3. Seat Handler
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
            let has_keyboard = capabilities.contains(wl_seat::Capability::Keyboard);
            
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            } else if !has_pointer && state.pointer.is_some() {
                state.pointer.take().unwrap().release();
            }
            
            if has_keyboard && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            } else if !has_keyboard && state.keyboard.is_some() {
                state.keyboard.take().unwrap().release();
            }
        }
    }
}

// 4. Pointer Handler
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        _state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        _event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

// 5. Keyboard Handler
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { key, state: key_state, .. } = event {
            if key_state == wl_keyboard::KeyState::Pressed && key == 1 { // Escape key
                state.running = false;
            }
        }
    }
}

// 6. Callback / Frame Done Handler
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.draw_frame(qh);
        }
    }
}

// 7. Core Compositor, Surface & Buffer Handlers
impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn event(_: &mut Self, _: &wl_compositor::WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(_: &mut Self, _: &wl_surface::WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        wl_buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(buf) = state.buffers.iter_mut().find(|b| b.wl_buffer == *wl_buffer) {
                buf.busy = false;
            }
        }
    }
}
impl Dispatch<ZwlrLayerShellV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: zwlr_layer_shell_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

// --- Delegate Macros Registration ---
wayland_client::delegate_dispatch!(State: [wl_registry::WlRegistry: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_compositor::WlCompositor: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_shm::WlShm: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_shm_pool::WlShmPool: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_buffer::WlBuffer: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_surface::WlSurface: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_callback::WlCallback: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_seat::WlSeat: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_pointer::WlPointer: ()] => State);
wayland_client::delegate_dispatch!(State: [wl_keyboard::WlKeyboard: ()] => State);
wayland_client::delegate_dispatch!(State: [ZwlrLayerShellV1: ()] => State);
wayland_client::delegate_dispatch!(State: [ZwlrLayerSurfaceV1: ()] => State);

// --- Application Entry Point ---
fn main() {
    // Connect to compositor socket
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland display");
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    
    let mut state = State::new();
    
    // Retrieve the display singleton and request the global registry
    let display = conn.display();
    display.get_registry(&qh, ());
    
    // Initial roundtrip to populate registry and bind globals
    event_queue.blocking_dispatch(&mut state).unwrap();
    
    // Ensure all required globals are found
    assert!(state.compositor.is_some(), "wl_compositor not found");
    assert!(state.shm.is_some(), "wl_shm not found");
    assert!(state.layer_shell.is_some(), "zwlr_layer_shell_v1 not found");
    assert!(state.seat.is_some(), "wl_seat not found");
    
    // Create underlying Wayland surface
    let surface = state.compositor.as_ref().unwrap().create_surface(&qh, ());
    
    // Create layer surface
    let layer_surface = state.layer_shell.as_ref().unwrap().get_layer_surface(
        &surface,
        None,
        Layer::Top,
        "wayprompt-panel".to_string(),
        &qh,
        (),
    );
    
    // Set layout parameters
    layer_surface.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
    layer_surface.set_size(0, 48); // 48 pixels height, stretch width
    layer_surface.set_margin(0, 0, 0, 0);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    
    // Crucial: Initial commit with no buffer to trigger Configure event
    surface.commit();
    
    state.surface = Some(surface);
    state.layer_surface = Some(layer_surface);
    
    // Main event loop
    while state.running {
        event_queue.blocking_dispatch(&mut state).expect("Error in event loop");
    }
}
```
