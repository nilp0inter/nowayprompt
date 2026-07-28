//! Wayland layer-shell frontend.
//!
//! Implements the [`Frontend`] trait.
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
//! ## Single-threaded read model
//!
//! `wayland-client` splits socket reads into `prepare_read` / `read_events`
//! / `cancel_read` so *multiple threads* can share the socket. A pinentry
//! has exactly one event-loop thread, so that dance is inert. We collapse
//! it: `flush` only flushes outbound traffic; `handle_event` does
//! `prepare_read().read()` then `dispatch_pending`; `no_event` is a no-op.

pub mod input;
pub mod render;
pub mod scale;
pub mod shm;

use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::config::Config;
use crate::frontend::{Event, Frontend, FrontendError, InterfaceMode};
use crate::secret::SecretBuffer;

use self::input::Seat;
use self::render::Surface;
use self::shm::BufferPool;

/// User-initiated exit reason. Maps to [`Event`] variants or a propagated
/// error.
#[derive(Debug)]
enum ExitReason {
    UserOk,
    UserAbort,
    UserNotOk,
    /// Non-user error.
    Error(FrontendError),
}

/// A tracked `wl_output` global: its registry name, proxy, and latest
/// positive integer scale (from `wl_output.scale`; 0 until reported).
#[derive(Debug)]
pub(crate) struct OutputRecord {
    /// Registry global name; the key used by `global_remove` and surface
    /// enter/leave membership.
    pub(crate) name: u32,
    pub(crate) proxy: WlOutput,
    pub(crate) scale: u32,
}

/// The dispatch `State`. Owns all mutable protocol state and every
/// `Dispatch<I, U>` impl.
pub struct WaylandState {
    // Globals (bound by the registry handler).
    pub(crate) compositor: Option<WlCompositor>,
    pub(crate) shm: Option<WlShm>,
    pub(crate) layer_shell: Option<ZwlrLayerShellV1>,
    pub(crate) cursor_shape_manager: Option<WpCursorShapeManagerV1>,
    pub(crate) fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    pub(crate) viewporter: Option<WpViewporter>,

    // Tracked `wl_output` globals, keyed by registry name.
    pub(crate) outputs: Vec<OutputRecord>,

    // Seats (multi-seat list).
    pub(crate) seats: Vec<Seat>,

    // Single surface + buffer pool.
    pub(crate) surface: Option<Surface>,
    pub(crate) buffer_pool: BufferPool,

    // Sync round-trip. While `Some`, globals may not be bound yet;
    // `enter_mode` defers to `delayed_mode`.
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
            viewporter: None,
            outputs: Vec::new(),
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

    /// Look up a tracked output by registry name.
    pub(crate) fn output_by_name(&self, name: u32) -> Option<&OutputRecord> {
        self.outputs.iter().find(|o| o.name == name)
    }

    /// Look up the registry name of a tracked output by its proxy.
    pub(crate) fn output_name_of(&self, output: &WlOutput) -> Option<u32> {
        self.outputs
            .iter()
            .find(|o| &o.proxy == output)
            .map(|o| o.name)
    }

    /// Abort with an exit reason.
    fn abort(&mut self, reason: ExitReason) {
        self.exit_reason = Some(reason);
    }

    /// Enter a mode, creating or destroying the surface.
    fn enter_mode(
        &mut self,
        qh: &QueueHandle<Self>,
        mode: InterfaceMode,
    ) -> Result<(), FrontendError> {
        if self.mode == mode {
            // Re-requesting the current mode is only legal for `None`.
            assert_eq!(self.mode, InterfaceMode::None);
            return Ok(());
        }

        // Defer until the sync callback fires.
        if self.sync.is_some() {
            self.delayed_mode = Some(mode);
            return Ok(());
        }

        // Tear down the current surface's text views.
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
            let viewporter = self.viewporter.clone();
            let surface = Surface::new(
                self,
                qh,
                &compositor,
                &layer_shell,
                &shm,
                fractional.as_ref(),
                viewporter.as_ref(),
                mode,
            )?;
            self.surface = Some(surface);
        }
        Ok(())
    }

    fn take_exit(&mut self) -> Option<Result<Event, FrontendError>> {
        let er = self.exit_reason.take()?;
        Some(match er {
            ExitReason::UserOk => Ok(Event::UserOk),
            ExitReason::UserAbort => Ok(Event::UserAbort),
            ExitReason::UserNotOk => Ok(Event::UserNotOk),
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
        if let Some(vp) = self.viewporter.take() {
            vp.destroy();
        }
        // WlCompositor / WlShm are globals: released on disconnect (drop).
        self.compositor = None;
        self.shm = None;
        // Release tracked wl_output objects (version >= 3 supports release).
        for output in self.outputs.drain(..) {
            output.proxy.release();
        }
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

/// Snapshot of the configured surface's render state for test
/// introspection (see [`Wayland::surface_info`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceInfo {
    /// Logical surface width (unscaled layout).
    pub logical_width: u32,
    /// Logical surface height (unscaled layout).
    pub logical_height: u32,
    /// Effective scale mode: `Integer` or `Fractional`.
    pub scale_mode: scale::ScaleMode,
    /// Scale numerator (N for integer, P for fractional over 120).
    pub scale_numerator: u32,
    /// Physical buffer width (logical × scale, rounded up).
    pub physical_width: u32,
    /// Physical buffer height (logical × scale, rounded up).
    pub physical_height: u32,
    /// Logical hotspot geometry (unaffected by scale).
    pub hotspots: Vec<render::HotSpot>,
}

/// The Wayland frontend. Thin [`Frontend`] wrapper over [`WaylandState`].
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

    /// Provide the secret buffer pointer.
    /// The caller guarantees the buffer outlives this frontend.
    pub fn set_secret_buffer(&mut self, secbuf: &mut SecretBuffer) {
        self.state.secbuf_ptr = Some(secbuf as *mut SecretBuffer);
    }

    /// Access the dispatch state (used by the test binary for assertions).
    pub fn state(&self) -> &WaylandState {
        &self.state
    }

    /// Test introspection: the configured surface's full render state —
    /// logical dimensions, exact scale mode + numerator, physical
    /// buffer dimensions, and logical hotspots. `None` until the first
    /// configure event renders the surface.
    ///
    /// Used by `wayland-test` to report changed render state on every
    /// commit that changes it, not only the first. The production prompt
    /// binary does not call this.
    pub fn surface_info(&self) -> Option<SurfaceInfo> {
        let s = self.state.surface.as_ref()?;
        if !s.configured {
            return None;
        }
        let (phys_w, phys_h) = s.scale.physical_size(s.width, s.height).ok()?;
        Some(SurfaceInfo {
            logical_width: s.width,
            logical_height: s.height,
            scale_mode: s.scale.mode(),
            scale_numerator: s.scale.numerator(),
            physical_width: phys_w,
            physical_height: phys_h,
            hotspots: s.hotspots.clone(),
        })
    }
}

impl Default for Wayland {
    fn default() -> Self {
        Self::new()
    }
}

impl Frontend for Wayland {
    fn init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError> {
        let display = display_socket(cfg)?;
        let stream = UnixStream::connect(&display).map_err(|error| {
            FrontendError::Unavailable(format!("cannot connect to {}: {error}", display.display()))
        })?;
        let conn = Connection::from_socket(stream).map_err(|error| {
            FrontendError::Unavailable(format!(
                "cannot establish Wayland connection to {}: {error}",
                display.display()
            ))
        })?;
        self.state.config_ptr = Some(cfg as *mut Config);

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

        // The pollable fd is the Wayland socket fd.
        let fd = self
            .conn
            .as_ref()
            .expect("connection established")
            .as_fd()
            .as_raw_fd();
        Ok(fd)
    }

    fn deinit(&mut self) {
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

        match self.state.take_exit() {
            Some(result) => result,
            None => Ok(Event::None),
        }
    }

    fn flush(&mut self) -> Result<Option<Event>, FrontendError> {
        // Flush outbound traffic only (see module docs for the read model).
        // Then surface any pending exit reason.
        let conn = self.conn.as_ref().expect("init not called");
        match conn.flush() {
            Ok(()) => {}
            Err(e) => return Err(FrontendError::Io(std::io::Error::other(e))),
        }
        match self.state.take_exit() {
            Some(result) => Ok(Some(result?)),
            None => Ok(None),
        }
    }

    fn no_event(&mut self) -> Result<(), FrontendError> {
        // No-op under the single-threaded read model (module docs).
        Ok(())
    }
}

fn display_socket(cfg: &Config) -> Result<PathBuf, FrontendError> {
    resolve_display_socket(
        cfg.wayland_display.as_deref(),
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

fn resolve_display_socket(
    configured: Option<&str>,
    environment: Option<std::ffi::OsString>,
    runtime_dir: Option<std::ffi::OsString>,
) -> Result<PathBuf, FrontendError> {
    let display = configured
        .map(PathBuf::from)
        .or_else(|| environment.map(PathBuf::from))
        .ok_or_else(|| FrontendError::Unavailable("no Wayland display".into()))?;

    if display.is_absolute() {
        return Ok(display);
    }

    let runtime_dir = runtime_dir
        .ok_or_else(|| FrontendError::Unavailable("XDG_RUNTIME_DIR is not set".into()))?;
    let runtime_dir = PathBuf::from(runtime_dir);
    if !runtime_dir.is_absolute() {
        return Err(FrontendError::Unavailable(
            "XDG_RUNTIME_DIR is not an absolute path".into(),
        ));
    }
    Ok(runtime_dir.join(display))
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
                // Bind globals by interface name.
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
                } else if interface == WpViewporter::interface().name {
                    state.viewporter =
                        Some(registry.bind::<WpViewporter, _, _>(name, version.min(1), qh, ()));
                } else if interface == WlCompositor::interface().name {
                    state.compositor =
                        Some(registry.bind::<WlCompositor, _, _>(name, version.min(4), qh, ()));
                } else if interface == WlShm::interface().name {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                } else if interface == WlSeat::interface().name {
                    let wl_seat: WlSeat =
                        registry.bind::<WlSeat, _, _>(name, version.min(8), qh, ());
                    state.seats.push(Seat::new(wl_seat));
                } else if interface == WlOutput::interface().name {
                    // Bind a version supporting `scale` (since 2) and
                    // `release` (since 3); wl_output caps at version 4.
                    let proxy: WlOutput =
                        registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ());
                    state.outputs.push(OutputRecord {
                        name,
                        proxy,
                        scale: 0,
                    });
                }
            }
            Event::GlobalRemove { name } => {
                // Drop the removed output's record and any surface
                // membership, then recompute + rerender if the effective
                // scale changed.
                if !state.outputs.iter().any(|o| o.name == name) {
                    return;
                }
                state.outputs.retain(|o| o.name != name);
                let Some(mut surface) = state.surface.take() else {
                    return;
                };
                let changed = surface.handle_output_removed(state, name);
                state.surface = Some(surface);
                if changed.is_some() {
                    render::render_surface(state, qh);
                }
            }
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
        // Sync round-trip complete.
        if state.layer_shell.is_none() || state.compositor.is_none() || state.shm.is_none() {
            state.abort(ExitReason::Error(FrontendError::Init(
                "missing wayland interfaces".into(),
            )));
        }
        // The callback is one-shot; clear our handle.
        state.sync = None;
        // Apply any deferred mode.
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

impl Dispatch<WlOutput, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wl_output::Event;
        if let Event::Scale { factor } = event {
            let factor = u32::try_from(factor).unwrap_or(0);
            // Only a positive scale is meaningful; ignore non-positive.
            let changed = state
                .outputs
                .iter_mut()
                .find(|o| &o.proxy == proxy)
                .map(|o| {
                    if o.scale != factor {
                        o.scale = factor;
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if changed {
                if let Some(mut surface) = state.surface.take() {
                    let scale_changed = surface.recompute_scale(state);
                    state.surface = Some(surface);
                    if scale_changed.is_some() {
                        render::render_surface(state, qh);
                    }
                }
            }
        }
        // `geometry`, `mode`, `done`, `name` are not needed for scaling.
    }
}

impl Dispatch<WpViewporter, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WpViewporter emits no events.
    }
}

impl Dispatch<WpViewport, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WpViewport emits no events.
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_display_socket;

    #[test]
    fn configured_display_takes_precedence() {
        assert_eq!(
            resolve_display_socket(
                Some("/configured/socket"),
                Some("environment".into()),
                Some("/runtime".into()),
            )
            .unwrap(),
            std::path::PathBuf::from("/configured/socket")
        );
    }

    #[test]
    fn no_display_is_unavailable() {
        assert!(matches!(
            resolve_display_socket(None, None, Some("/runtime".into())),
            Err(crate::frontend::FrontendError::Unavailable(_))
        ));
    }
}
