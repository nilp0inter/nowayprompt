//! Seat input: keyboard (XKB), pointer, touch (parity `Wayland.zig:219-633`).
//!
//! `WaylandState` is the dispatch `State`; each seat's devices are bound
//! with the seat's index as user-data so events route to the right `Seat`.
//! Keyboard input drives the pin buffer and exit reasons; pointer/touch
//! drive hotspot activation.

use memmap2::MmapOptions;
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use xkbcommon::xkb;
use xkbcommon::xkb::keysyms;

use crate::frontend::{FrontendError, InterfaceMode};

use super::render::{render_surface, HotSpot};
use super::{ExitReason, WaylandState};

/// A tracked touch point (parity `Wayland.zig:220-223`).
#[derive(Debug, Clone, Copy)]
struct TouchPoint {
    id: i32,
    hotspot: HotSpot,
}

/// A Wayland seat. Parity with `Wayland.zig:219-633`.
pub struct Seat {
    pub wl_seat: WlSeat,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,
    cursor_shape_device: Option<WpCursorShapeDeviceV1>,
    xkb_context: Option<xkb::Context>,
    xkb_state: Option<xkb::State>,
    pointer_x: u32,
    pointer_y: u32,
    last_enter_serial: u32,
    press_hotspot: Option<HotSpot>,
    touchpoints: Vec<TouchPoint>,
}

impl Seat {
    /// Bind a new seat (parity `Wayland.zig:248-251`). Device binding
    /// happens on the `capabilities` event.
    pub fn new(wl_seat: WlSeat) -> Self {
        Self {
            wl_seat,
            keyboard: None,
            pointer: None,
            touch: None,
            cursor_shape_device: None,
            xkb_context: Some(xkb::Context::new(xkb::CONTEXT_NO_FLAGS)),
            xkb_state: None,
            pointer_x: 0,
            pointer_y: 0,
            last_enter_serial: 0,
            press_hotspot: None,
            touchpoints: Vec::new(),
        }
    }

    pub fn deinit(mut self) {
        if let Some(k) = self.keyboard.take() {
            k.release();
        }
        if let Some(p) = self.pointer.take() {
            p.release();
        }
        if let Some(t) = self.touch.take() {
            t.release();
        }
        if let Some(c) = self.cursor_shape_device.take() {
            c.destroy();
        }
        self.xkb_state = None;
        self.xkb_context = None;
        self.wl_seat.release();
    }

    fn bind_keyboard(&mut self, qh: &QueueHandle<WaylandState>, idx: usize) {
        if self.keyboard.is_some() {
            return;
        }
        self.keyboard = Some(self.wl_seat.get_keyboard(qh, idx));
    }

    fn release_keyboard(&mut self) {
        if let Some(k) = self.keyboard.take() {
            k.release();
        }
        self.xkb_state = None;
    }

    fn release_pointer(&mut self) {
        self.press_hotspot = None;
        if let Some(p) = self.pointer.take() {
            p.release();
        }
        if let Some(c) = self.cursor_shape_device.take() {
            c.destroy();
        }
    }

    fn bind_touch(&mut self, qh: &QueueHandle<WaylandState>, idx: usize) {
        if self.touch.is_some() {
            return;
        }
        self.touch = Some(self.wl_seat.get_touch(qh, idx));
    }

    fn release_touch(&mut self) {
        if let Some(t) = self.touch.take() {
            t.release();
        }
        self.touchpoints.clear();
    }

    fn set_cursor(&mut self, shape: Shape) {
        if let Some(csd) = &self.cursor_shape_device {
            csd.set_shape(self.last_enter_serial, shape);
        }
    }

    fn touchpoint_from_id(&self, id: i32) -> Option<usize> {
        self.touchpoints.iter().position(|tp| tp.id == id)
    }
}

/// Locate the seat index owning `seat`.
fn seat_index_of(state: &WaylandState, seat: &WlSeat) -> Option<usize> {
    state.seats.iter().position(|s| &s.wl_seat == seat)
}

// --- wl_seat ---------------------------------------------------------------

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(idx) = seat_index_of(state, proxy) else {
            return;
        };
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                // `capabilities` arrives wrapped in `WEnum`; unwrap to the
                // bitflags (unknown future bits → ignore the event).
                let WEnum::Value(capabilities) = capabilities else {
                    return;
                };
                if capabilities.contains(wl_seat::Capability::Keyboard) {
                    state.seats[idx].bind_keyboard(qh, idx);
                } else {
                    state.seats[idx].release_keyboard();
                }
                if capabilities.contains(wl_seat::Capability::Pointer) {
                    // Borrow split: clone the seat, create the pointer, then
                    // (optionally) bind a cursor-shape device from the manager.
                    if state.seats[idx].pointer.is_none() {
                        let seat = state.seats[idx].wl_seat.clone();
                        let pointer = seat.get_pointer(qh, idx);
                        let csd = state
                            .cursor_shape_manager
                            .as_ref()
                            .map(|csm| csm.get_pointer(&pointer, qh, idx));
                        state.seats[idx].cursor_shape_device = csd;
                        state.seats[idx].pointer = Some(pointer);
                    }
                } else {
                    state.seats[idx].release_pointer();
                }
                if capabilities.contains(wl_seat::Capability::Touch) {
                    state.seats[idx].bind_touch(qh, idx);
                } else {
                    state.seats[idx].release_touch();
                }
            }
            wl_seat::Event::Name { .. } => {}
            _ => {}
        }
    }
}

// --- wl_keyboard -----------------------------------------------------------

impl Dispatch<WlKeyboard, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: <WlKeyboard as wayland_client::Proxy>::Event,
        idx: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let idx = *idx;
        if idx >= state.seats.len() {
            return;
        }
        match event {
            // Parity `Wayland.zig:445-474`: mmap the keymap fd MAP_PRIVATE
            // read-only, compile via xkb, create state. No SIGBUS guard (D2).
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if !matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    state.abort(ExitReason::Error(FrontendError::Init(
                        "unsupported keymap format".into(),
                    )));
                    return;
                }
                let mmap = match unsafe { MmapOptions::new().len(size as usize).map(&fd) } {
                    Ok(m) => m,
                    Err(_) => {
                        state.abort(ExitReason::Error(FrontendError::Init(
                            "keymap mmap failed".into(),
                        )));
                        return;
                    }
                };
                // The keymap is NUL-terminated; drop the trailing byte. The
                // OwnedFd drops at end of this arm (client closes its copy;
                // the mmap persists for the compile below).
                let len = (size as usize).saturating_sub(1);
                let keymap_str = String::from_utf8_lossy(&mmap[..len]).into_owned();

                let context = state.seats[idx]
                    .xkb_context
                    .get_or_insert_with(|| xkb::Context::new(xkb::CONTEXT_NO_FLAGS));
                let keymap = match xkb::Keymap::new_from_string(
                    context,
                    keymap_str,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                ) {
                    Some(k) => k,
                    None => {
                        state.abort(ExitReason::Error(FrontendError::Init(
                            "keymap compile failed".into(),
                        )));
                        return;
                    }
                };
                state.seats[idx].xkb_state = Some(xkb::State::new(&keymap));
            }
            // Parity `Wayland.zig:475-479`: update modifier mask.
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(xs) = state.seats[idx].xkb_state.as_mut() {
                    xs.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            // Parity `Wayland.zig:480-528`: key dispatch.
            wl_keyboard::Event::Key { key, state: ks, .. } => {
                if !matches!(ks, WEnum::Value(wl_keyboard::KeyState::Pressed)) {
                    return;
                }
                handle_key(state, qh, idx, key);
            }
            _ => {}
        }
    }
}

/// Key dispatch (parity `Wayland.zig:480-528`).
fn handle_key(state: &mut WaylandState, qh: &QueueHandle<WaylandState>, idx: usize, key: u32) {
    // Wayland evdev keycodes are offset by 8 from xkb keycodes.
    let keycode = xkb::Keycode::new(key + 8);
    let keysym = match state.seats[idx].xkb_state.as_ref() {
        Some(xs) => xs.key_get_one_sym(keycode).raw(),
        None => return,
    };
    if keysym == keysyms::KEY_NoSymbol {
        return;
    }

    let ctrl = state.seats[idx]
        .xkb_state
        .as_ref()
        .map(|xs| xs.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE))
        .unwrap_or(false);

    if ctrl {
        // Ctrl+BackSpace / Ctrl+u / Ctrl+w → reset pin (parity 490-498).
        if matches!(
            keysym,
            keysyms::KEY_BackSpace | keysyms::KEY_u | keysyms::KEY_w
        ) && state.mode == InterfaceMode::GetPin
        {
            // Parity `Wayland.zig:494`: reset failure (OOM) → abort.
            if state.secbuf().reset().is_err() {
                state.abort(ExitReason::Error(crate::frontend::FrontendError::Init(
                    "secret buffer reset failed".into(),
                )));
                return;
            }
            render_surface(state, qh);
        }
        return;
    }

    match keysym {
        keysyms::KEY_Return => {
            state.abort(ExitReason::UserOk);
            return;
        }
        keysyms::KEY_Escape => {
            state.abort(ExitReason::UserAbort);
            return;
        }
        keysyms::KEY_BackSpace => {
            if state.mode == InterfaceMode::GetPin {
                state.secbuf().delete_backwards();
                render_surface(state, qh);
            }
            return;
        }
        keysyms::KEY_Delete => return,
        _ => {}
    }

    if state.mode != InterfaceMode::GetPin {
        return;
    }

    // Append the key's UTF-8 to the pin buffer (parity 522-524).
    if let Some(xs) = state.seats[idx].xkb_state.as_ref() {
        let utf8 = xs.key_get_utf8(keycode);
        if !utf8.is_empty() {
            let _ = state.secbuf().append_slice(utf8.as_bytes());
        }
    }
    render_surface(state, qh);
}

// --- wl_pointer ------------------------------------------------------------

impl Dispatch<WlPointer, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlPointer,
        event: <WlPointer as wayland_client::Proxy>::Event,
        idx: &usize,
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let idx = *idx;
        if idx >= state.seats.len() {
            return;
        }
        // Ignore pointer events after the surface is gone (parity 318-319).
        if state.surface.is_none() {
            return;
        }
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                update_pointer(state, idx, surface_x, surface_y, Some(serial));
            }
            wl_pointer::Event::Leave { .. } => {}
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                update_pointer(state, idx, surface_x, surface_y, None);
            }
            // Activate on release for better UX (parity 326-339).
            // BTN_LEFT = 0x110.
            wl_pointer::Event::Button {
                button, state: bs, ..
            } => {
                if button != 0x110 {
                    return;
                }
                let (px, py) = (state.seats[idx].pointer_x, state.seats[idx].pointer_y);
                match bs {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        state.seats[idx].press_hotspot = state
                            .surface
                            .as_ref()
                            .and_then(|s| s.hotspot_from_point(px, py))
                            .copied();
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        if let Some(pressed) = state.seats[idx].press_hotspot.take() {
                            // `.copied()` ends the surface borrow before
                            // `act` mutably borrows state.
                            let hs = state
                                .surface
                                .as_ref()
                                .and_then(|s| s.hotspot_from_point(px, py))
                                .copied();
                            if let Some(hs) = hs {
                                if hs == pressed {
                                    hs.act(state);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Update pointer position + cursor shape (parity `Wayland.zig:345-365`).
fn update_pointer(state: &mut WaylandState, idx: usize, x: f64, y: f64, serial: Option<u32>) {
    state.seats[idx].pointer_x = if x > 0.0 { x as u32 } else { 0 };
    state.seats[idx].pointer_y = if y > 0.0 { y as u32 } else { 0 };
    if let Some(s) = serial {
        state.seats[idx].last_enter_serial = s;
    }
    let (px, py) = (state.seats[idx].pointer_x, state.seats[idx].pointer_y);
    let over_hotspot = state
        .surface
        .as_ref()
        .map(|s| s.hotspot_from_point(px, py).is_some())
        .unwrap_or(false);
    let shape = if over_hotspot {
        Shape::Pointer
    } else {
        Shape::Default
    };
    state.seats[idx].set_cursor(shape);
}

// --- wl_touch --------------------------------------------------------------

impl Dispatch<WlTouch, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlTouch,
        event: <WlTouch as wayland_client::Proxy>::Event,
        idx: &usize,
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let idx = *idx;
        if idx >= state.seats.len() {
            return;
        }
        match event {
            // Activate on touch-up (parity 565-591).
            wl_touch::Event::Down { id, x, y, .. } => {
                let (x, y) = (clamp24(x), clamp24(y));
                let hotspot = match state
                    .surface
                    .as_ref()
                    .and_then(|s| s.hotspot_from_point(x, y))
                {
                    Some(hs) => *hs,
                    None => return,
                };
                state.seats[idx]
                    .touchpoints
                    .push(TouchPoint { id, hotspot });
            }
            wl_touch::Event::Up { id, .. } => {
                if let Some(pos) = state.seats[idx].touchpoint_from_id(id) {
                    let tp = state.seats[idx].touchpoints.remove(pos);
                    tp.hotspot.act(state);
                }
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                let (x, y) = (clamp24(x), clamp24(y));
                if let Some(pos) = state.seats[idx].touchpoint_from_id(id) {
                    if !state.seats[idx].touchpoints[pos]
                        .hotspot
                        .contains_point(x, y)
                    {
                        state.seats[idx].touchpoints.remove(pos);
                    }
                }
            }
            wl_touch::Event::Cancel => {
                state.seats[idx].touchpoints.clear();
            }
            _ => {}
        }
    }
}

fn clamp24(v: f64) -> u32 {
    if v <= 0.0 {
        0
    } else if v > u32::MAX as f64 {
        u32::MAX
    } else {
        v as u32
    }
}

// --- wp_cursor_shape_device_v1 ---------------------------------------------

impl Dispatch<WpCursorShapeDeviceV1, usize> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WpCursorShapeDeviceV1,
        _event: <WpCursorShapeDeviceV1 as wayland_client::Proxy>::Event,
        _idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WpCursorShapeDeviceV1 emits no events.
    }
}
