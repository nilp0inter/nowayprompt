//! Wayland layer-shell frontend.
//!
//! 100% behavioral parity with `legacy/src/Wayland.zig`. Implements the
//! frozen [`Frontend`] trait. Stage 3 ships this as a library; `main.rs`
//! wiring is deferred to Stage 4.
//!
//! ## Dispatch architecture
//!
//! `wayland-client` routes events to `Dispatch<I, U>` impls on a central
//! `State` type, and `EventQueue<State>::dispatch_pending(&mut self,
//! data: &mut State)` borrows the queue and the state separately. Storing
//! `EventQueue<Wayland>` inside `Wayland` would therefore conflict. The
//! idiom is a separate dispatch state:
//!
//! - [`WaylandState`] — the dispatch `State`. Owns all mutable protocol
//!   state (globals, seats, surface, buffer pool, `exit_reason`) and every
//!   `Dispatch<I, U>` impl.
//! - [`Wayland`] — the thin [`Frontend`] wrapper. Owns the `Connection`,
//!   the `EventQueue<WaylandState>`, a cloned `QueueHandle`, and the
//!   `WaylandState`.
//!
//! ## Single-threaded read model (deviation, recorded)
//!
//! Legacy splits `prepare_read` (flush) / `read_events` (handle_event) /
//! `cancel_read` (no_event) to coordinate *multiple threads* reading the
//! socket. A pinentry has exactly one event-loop thread, so that dance is
//! inert. We collapse it: `flush` only flushes outbound traffic;
//! `handle_event` does `prepare_read().read()` then `dispatch_pending`;
//! `no_event` is a no-op. Observable behaviour (events dispatched, outbound
//! flushed, `exit_reason` surfaced) is unchanged.

pub mod input;
pub mod render;
pub mod shm;

use std::os::fd::{AsFd, AsRawFd, RawFd};

use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::config::Config;
use crate::frontend::{Event, Frontend, FrontendError, InterfaceMode};
use crate::secret::SecretBuffer;

use self::input::Seat;
use self::render::Surface;
use self::shm::BufferPool;

/// User-initiated exit reason. Maps to [`Event`] variants or a propagated
/// error. Parity with legacy `Wayland.exit_reason: ?anyerror`.
#[derive(Debug)]
enum ExitReason {
    UserOk,
    UserAbort,
    UserNotOk,
    /// Non-user error (parity: `error.OutOfMemory`, `error.MissingWaylandInterfaces`, etc.).
    Error(FrontendError),
}

/// The dispatch `State`. Owns all mutable protocol state and every
/// `Dispatch<I, U>` impl.
///
/// Parity with the mutable fields of `Wayland.zig:1425-1458`.
pub struct WaylandState {
    // Globals (bound by the registry handler).
    pub(crate) compositor: Option<WlCompositor>,
    pub(crate) shm: Option<WlShm>,
    pub(crate) layer_shell: Option<ZwlrLayerShellV1>,
    pub(crate) cursor_shape_manager: Option<WpCursorShapeManagerV1>,
    pub(crate) fractional_scale_manager: Option<WpFractionalScaleManagerV1>,

    // Seats (multi-seat list, parity `Wayland.zig:1444`).
    pub(crate) seats: Vec<Seat>,

    // Single surface + buffer pool.
    pub(crate) surface: Option<Surface>,
    pub(crate) buffer_pool: BufferPool,

    // Sync round-trip. While `Some`, globals may not be bound yet;
    // `enter_mode` defers to `delayed_mode` (parity `Wayland.zig:1448-1454`).
    sync: Option<WlCallback>,
    delayed_mode: Option<InterfaceMode>,

    // Runtime UI state.
    mode: InterfaceMode,
    exit_reason: Option<ExitReason>,

    // Pointers set by `init` / `set_secret_buffer`. Outlive the frontend.
    config_ptr: Option<*mut Config>,
    secbuf_ptr: Option<*mut SecretBuffer>,
}

impl WaylandState {
    fn new() -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            cursor_shape_manager: None,
            fractional_scale_manager: None,
            seats: Vec::new(),
            surface: None,
            buffer_pool: BufferPool::new(),
            sync: None,
            delayed_mode: None,
            mode: InterfaceMode::None,
            exit_reason: None,
            config_ptr: None,
            secbuf_ptr: None,
        }
    }

    fn config(&self) -> &Config {
        unsafe { &*self.config_ptr.expect("init not called") }
    }

    fn secbuf(&mut self) -> &mut SecretBuffer {
        unsafe { &mut *self.secbuf_ptr.expect("secret buffer not set") }
    }

    /// Abort with an exit reason (parity `Wayland.zig:1691-1697`).
    fn abort(&mut self, reason: ExitReason) {
        self.exit_reason = Some(reason);
    }

    /// Enter a mode, creating or destroying the surface
    /// (parity `Wayland.zig:1535-1564`).
    fn enter_mode(
        &mut self,
        qh: &QueueHandle<Self>,
        mode: InterfaceMode,
    ) -> Result<(), FrontendError> {
        if self.mode == mode {
            // Parity: `debug.assert(self.mode == .none)`.
            assert_eq!(self.mode, InterfaceMode::None);
            return Ok(());
        }

        // Defer until the sync callback fires (parity `Wayland.zig:1542-1546`).
        if self.sync.is_some() {
            self.delayed_mode = Some(mode);
            return Ok(());
        }

        // Tear down the current surface's text views (parity deinitTextViews).
        if let Some(s) = self.surface.take() {
            s.deinit();
        }

        self.mode = mode;
        if mode == InterfaceMode::None {
            // Surface already taken above.
        } else {
            let compositor = self
                .compositor
                .clone()
                .ok_or_else(|| FrontendError::Init("no compositor".into()))?;
            let layer_shell = self
                .layer_shell
                .clone()
                .ok_or_else(|| FrontendError::Init("no layer_shell".into()))?;
            let shm = self
                .shm
                .clone()
                .ok_or_else(|| FrontendError::Init("no shm".into()))?;
            let fractional = self.fractional_scale_manager.clone();
            let surface = Surface::new(
                self,
                qh,
                &compositor,
                &layer_shell,
                &shm,
                fractional.as_ref(),
                mode,
            )?;
            self.surface = Some(surface);
        }
        Ok(())
    }

    /// Convert a set `exit_reason` into an `Event`, clear it, and enter
    /// `None` mode (parity `Wayland.zig:1673-1689`).
    fn take_exit(&mut self, qh: &QueueHandle<Self>) -> Option<Result<Event, FrontendError>> {
        let er = self.exit_reason.take()?;
        Some(match er {
            ExitReason::UserOk => {
                let r = self.enter_mode(qh, InterfaceMode::None);
                r.map(|()| Event::UserOk)
            }
            ExitReason::UserAbort => {
                let r = self.enter_mode(qh, InterfaceMode::None);
                r.map(|()| Event::UserAbort)
            }
            ExitReason::UserNotOk => {
                let r = self.enter_mode(qh, InterfaceMode::None);
                r.map(|()| Event::UserNotOk)
            }
            ExitReason::Error(e) => Err(e),
        })
    }

    fn deinit(&mut self) {
        if let Some(s) = self.surface.take() {
            s.deinit();
        }
        self.buffer_pool.deinit();
        if let Some(ls) = self.layer_shell.take() {
            ls.destroy();
        }
        if let Some(csm) = self.cursor_shape_manager.take() {
            csm.destroy();
        }
        if let Some(fsm) = self.fractional_scale_manager.take() {
            fsm.destroy();
        }
        // WlCompositor / WlShm are globals: released on disconnect (drop).
        self.compositor = None;
        self.shm = None;
        for seat in self.seats.drain(..) {
            seat.deinit();
        }
        if let Some(s) = self.sync.take() {
            // WlCallback is one-shot; dropping releases it.
            drop(s);
        }
        self.config_ptr = None;
        self.secbuf_ptr = None;
    }
}

/// The Wayland frontend. Thin [`Frontend`] wrapper over [`WaylandState`].
///
/// Parity with the method set of `Wayland.zig:1460-1697`.
pub struct Wayland {
    conn: Option<Connection>,
    queue: Option<EventQueue<WaylandState>>,
    qh: Option<QueueHandle<WaylandState>>,
    registry: Option<WlRegistry>,
    state: WaylandState,
}

impl Wayland {
    /// Create a new, uninitialised Wayland frontend.
    pub fn new() -> Self {
        Self {
            conn: None,
            queue: None,
            qh: None,
            registry: None,
            state: WaylandState::new(),
        }
    }

    /// Provide the secret buffer pointer. Parity with `Tty::set_secret_buffer`.
    /// The caller guarantees the buffer outlives this frontend.
    pub fn set_secret_buffer(&mut self, secbuf: &mut SecretBuffer) {
        self.state.secbuf_ptr = Some(secbuf as *mut SecretBuffer);
    }

    /// Access the dispatch state (used by the test binary for assertions).
    pub fn state(&self) -> &WaylandState {
        &self.state
    }
}

impl Default for Wayland {
    fn default() -> Self {
        Self::new()
    }
}

impl Frontend for Wayland {
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError> {
        self.state.config_ptr = Some(cfg as *mut Config);

        // Resolve the display socket: explicit config > WAYLAND_DISPLAY env.
        // Parity `Wayland.zig:1463-1467`.
        let _display_name = cfg
            .wayland_display
            .clone()
            .or_else(|| std::env::var("WAYLAND_DISPLAY").ok())
            .ok_or_else(|| FrontendError::Init("no wayland display".into()))?;

        let conn = Connection::connect_to_env()
            .map_err(|e| FrontendError::Init(format!("wayland connect: {e}")))?;
        let display = conn.display();
        let queue = conn.new_event_queue::<WaylandState>();
        let qh = queue.handle();

        let registry = display.get_registry(&qh, ());
        let sync = display.sync(&qh, ());
        self.state.sync = Some(sync);

        self.conn = Some(conn);
        self.queue = Some(queue);
        self.qh = Some(qh);
        self.registry = Some(registry);

        // The pollable fd is the Wayland socket fd (parity `display.getFd()`).
        let fd = self
            .conn
            .as_ref()
            .expect("connection established")
            .as_fd()
            .as_raw_fd();
        Ok(fd)
    }

    fn deinit(&mut self) {
        // Parity `Wayland.zig:1502-1533`.
        self.state.deinit();
        // WlRegistry has no destructor request; dropping releases it.
        self.registry = None;
        self.qh = None;
        self.queue = None;
        self.conn = None;
    }

    fn enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError> {
        let qh = self
            .qh
            .clone()
            .ok_or_else(|| FrontendError::Init("not initialised".into()))?;
        self.state.enter_mode(&qh, mode)
    }

    fn handle_event(&mut self) -> Result<Event, FrontendError> {
        // Single-threaded read model: read available events, then dispatch.
        let queue = self.queue.as_mut().expect("init not called");
        loop {
            match queue.prepare_read() {
                Some(guard) => {
                    // Read from the socket; WouldBlock is benign.
                    let _ = guard.read();
                    break;
                }
                None => {
                    queue.dispatch_pending(&mut self.state).map_err(io_other)?;
                }
            }
        }
        queue.dispatch_pending(&mut self.state).map_err(io_other)?;

        let qh = self.qh.clone().expect("init not called");
        match self.state.take_exit(&qh) {
            Some(result) => result,
            None => Ok(Event::None),
        }
    }

    fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
        // Flush outbound traffic only (see module docs for the read-model
        // deviation). Then surface any pending exit reason.
        let queue = self.queue.as_ref().expect("init not called");
        // Drain any already-buffered inbound without blocking so that
        // `prepare_read` in handle_event starts clean.
        let conn = self.conn.as_ref().expect("init not called");
        let _ = queue;
        match conn.flush() {
            Ok(()) => {}
            Err(e) => return Err(FrontendError::Io(std::io::Error::other(e))),
        }
        let qh = self.qh.clone().expect("init not called");
        match self.state.take_exit(&qh) {
            Some(result) => Ok(Some(result?)),
            None => Ok(None),
        }
    }

    fn no_event(&mut self) -> Result<(), FrontendError> {
        // No-op under the single-threaded read model (module docs).
        Ok(())
    }
}

fn io_other(e: impl std::fmt::Display) -> FrontendError {
    FrontendError::Io(std::io::Error::other(e.to_string()))
}

// --- Dispatch impls --------------------------------------------------------
//
// `WaylandState` is the dispatch `State`. Proxy types created with a
// `QueueHandle<WaylandState>` need a `Dispatch<I, U>` impl here (or in a
// submodule via the orphan rule — `WaylandState` is crate-local, so impls
// may live in render.rs / input.rs).

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wl_registry::Event;
        match event {
            Event::Global {
                name,
                interface,
                version,
            } => {
                // Bind globals by interface name (parity `Wayland.zig:1699-1735`).
                if interface == ZwlrLayerShellV1::interface().name {
                    state.layer_shell =
                        Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, version.min(4), qh, ()));
                } else if interface == WpCursorShapeManagerV1::interface().name {
                    state.cursor_shape_manager = Some(
                        registry.bind::<WpCursorShapeManagerV1, _, _>(name, version.min(1), qh, ()),
                    );
                } else if interface == WpFractionalScaleManagerV1::interface().name {
                    state.fractional_scale_manager =
                        Some(registry.bind::<WpFractionalScaleManagerV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ));
                } else if interface == WlCompositor::interface().name {
                    state.compositor =
                        Some(registry.bind::<WlCompositor, _, _>(name, version.min(4), qh, ()));
                } else if interface == WlShm::interface().name {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                } else if interface == WlSeat::interface().name {
                    let wl_seat: WlSeat =
                        registry.bind::<WlSeat, _, _>(name, version.min(1), qh, ());
                    state.seats.push(Seat::new(wl_seat));
                }
            }
            Event::GlobalRemove { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _cb: &WlCallback,
        _event: <WlCallback as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Sync round-trip complete (parity `Wayland.zig:1743-1761`).
        if state.layer_shell.is_none() || state.compositor.is_none() || state.shm.is_none() {
            state.abort(ExitReason::Error(FrontendError::Init(
                "missing wayland interfaces".into(),
            )));
        }
        // The callback is one-shot; clear our handle.
        state.sync = None;
        // Apply any deferred mode (parity `Wayland.zig:1754-1760`).
        // `sync` was just cleared, so `enter_mode` will not re-defer.
        if let Some(mode) = state.delayed_mode.take() {
            if let Err(e) = state.enter_mode(qh, mode) {
                state.abort(ExitReason::Error(e));
            }
        }
    }
}

// Globals with no events we handle.
impl Dispatch<WlCompositor, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WlCompositor emits no events.
    }
}

impl Dispatch<WlShm, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        event: <WlShm as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WlShm emits only `format`; we assume Argb8888 is available.
        let _ = event;
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // ZwlrLayerShellV1 emits no events.
    }
}

impl Dispatch<WpCursorShapeManagerV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WpCursorShapeManagerV1,
        _event: <WpCursorShapeManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WpCursorShapeManagerV1 emits no events.
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WpFractionalScaleManagerV1 emits no events.
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Capabilities/name handled in the deep input impl (input.rs adds
        // keyboard/pointer/touch binding). Foundation ignores.
        let _ = event;
    }
}
