//! Seat input: keyboard (XKB), pointer, touch (parity `Wayland.zig:219-633`).
//!
//! Foundation stub: `Seat` struct is frozen. XKB keymap compilation,
//! modifier sync, key dispatch, pointer, and touch are filled in by the
//! input implementation pass.

use wayland_client::protocol::wl_seat::WlSeat;

/// A Wayland seat. Parity with `Wayland.zig:219-633`.
pub struct Seat {
    pub wl_seat: WlSeat,
}

impl Seat {
    /// Bind a new seat. Parity `Wayland.zig:248-251`.
    pub fn new(wl_seat: WlSeat) -> Self {
        Self { wl_seat }
    }

    pub fn deinit(self) {
        // Foundation stub: release the seat.
        self.wl_seat.release();
    }
}
