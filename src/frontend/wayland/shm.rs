//! SHM buffer pool.
//!
//! Triple-buffer slot arena with `Dispatch<WlBuffer, usize>` user-data
//! index for `.release` busy-state tracking. Slots are
//! `Vec<Option<Buffer>>` so the user-data index stays stable across culls.

use memmap2::MmapMut;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::frontend::wayland::WaylandState;

/// Maximum buffers per surface.
pub const MAX_BUFFER_MULTIPLICITY: usize = 3;

/// A single SHM-backed buffer.
pub struct Buffer {
    pub wl_buffer: Option<WlBuffer>,
    pub mmap: Option<MmapMut>,
    pub width: u32,
    pub height: u32,
    pub busy: bool,
}

impl Buffer {
    /// Allocate a new SHM buffer of `width` x `height` (Argb8888):
    /// `memfd_create` + `ftruncate` + `MAP_SHARED` + `PROT_READ|PROT_WRITE`
    /// mmap, `wl_shm.create_pool`, `pool.create_buffer` with
    /// `Format::Argb8888` and stride `width*4`.
    pub fn new(
        shm: &WlShm,
        qh: &QueueHandle<WaylandState>,
        slot: usize,
        width: u32,
        height: u32,
    ) -> Result<Self, std::io::Error> {
        let stride = width.checked_mul(4).ok_or_else(zero)?;
        let size = stride.checked_mul(height).ok_or_else(zero)? as usize;
        if size == 0 {
            return Err(zero());
        }

        // memfd_create with MFD_CLOEXEC.
        let fd: OwnedFd = unsafe {
            let raw = libc::memfd_create(c"/nowayprompt".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(raw)
        };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // mmap MAP_SHARED + PROT_READ|PROT_WRITE via MmapOptions. The
        // client's fd is closed when `fd` drops at function end (the
        // compositor dups it for the pool; the mmap persists).
        let mmap = unsafe {
            memmap2::MmapOptions::new()
                .len(size)
                .map_mut(&fd)
                .map_err(std::io::Error::other)?
        };

        // Create the wl_shm_pool and buffer. The pool is destroyed
        // immediately; the buffer keeps the backing alive.
        let pool: WlShmPool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let wl_buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            Format::Argb8888,
            qh,
            slot,
        );
        pool.destroy();
        Ok(Self {
            wl_buffer: Some(wl_buffer),
            mmap: Some(mmap),
            width,
            height,
            busy: false,
        })
    }

    pub fn deinit(&mut self) {
        if let Some(b) = self.wl_buffer.take() {
            b.destroy();
        }
        self.mmap = None;
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.deinit();
    }
}

fn zero() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "zero-sized buffer")
}

/// Triple-buffer slot arena.
///
/// Slots are `Vec<Option<Buffer>>`; the `Dispatch<WlBuffer, usize>`
/// user-data is the slot index, which never shifts.
pub struct BufferPool {
    slots: Vec<Option<Buffer>>,
}

/// Pure slot-selection outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotDecision {
    /// Reuse an idle buffer of matching size (no re-allocation).
    Reuse(usize),
    /// Re-init an idle mismatched-size slot.
    Reinit(usize),
    /// Allocate into an existing free (`None`) slot.
    FillFree(usize),
    /// Push a brand-new slot.
    Push,
}

impl BufferPool {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Get the slot index of a buffer of the requested dimensions,
    /// reusing an idle buffer of matching size, else re-initing an idle
    /// mismatched-size buffer, else allocating a new slot. Culls idle
    /// buffers on every call.
    pub fn next_buffer(
        &mut self,
        shm: &WlShm,
        qh: &QueueHandle<WaylandState>,
        width: u32,
        height: u32,
    ) -> Result<usize, std::io::Error> {
        self.cull_buffers();
        let i = match self.select_slot(width, height) {
            SlotDecision::Reuse(i) => i,
            SlotDecision::Reinit(i) | SlotDecision::FillFree(i) => {
                self.slots[i] = Some(Buffer::new(shm, qh, i, width, height)?);
                i
            }
            SlotDecision::Push => {
                let i = self.slots.len();
                self.slots
                    .push(Some(Buffer::new(shm, qh, i, width, height)?));
                i
            }
        };
        Ok(i)
    }

    /// Pure slot-selection decision (testable without a live `WlShm`).
    fn select_slot(&self, width: u32, height: u32) -> SlotDecision {
        let mut first_idle_mismatched: Option<usize> = None;
        let mut first_free: Option<usize> = None;
        for (i, slot) in self.slots.iter().enumerate() {
            match slot {
                Some(b) if !b.busy => {
                    if b.width == width && b.height == height {
                        return SlotDecision::Reuse(i);
                    }
                    if first_idle_mismatched.is_none() {
                        first_idle_mismatched = Some(i);
                    }
                }
                None if first_free.is_none() => first_free = Some(i),
                _ => {}
            }
        }
        if let Some(i) = first_idle_mismatched {
            SlotDecision::Reinit(i)
        } else if let Some(i) = first_free {
            SlotDecision::FillFree(i)
        } else {
            SlotDecision::Push
        }
    }

    /// Access a buffer by slot index.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Buffer> {
        self.slots.get_mut(idx).and_then(|s| s.as_mut())
    }

    /// Destroy idle buffers exceeding `MAX_BUFFER_MULTIPLICITY`.
    pub fn cull_buffers(&mut self) {
        let live = self.slots.iter().filter(|s| s.is_some()).count();
        if live <= MAX_BUFFER_MULTIPLICITY {
            return;
        }
        let mut overhead = live - MAX_BUFFER_MULTIPLICITY;
        for slot in self.slots.iter_mut() {
            if overhead == 0 {
                break;
            }
            if let Some(b) = slot {
                if !b.busy {
                    b.deinit();
                    *slot = None;
                    overhead -= 1;
                }
            }
        }
    }

    pub fn deinit(&mut self) {
        for slot in self.slots.iter_mut() {
            if let Some(b) = slot.take() {
                drop(b);
            }
        }
        self.slots.clear();
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}

// --- Dispatch impls --------------------------------------------------------

impl Dispatch<WlShmPool, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // WlShmPool emits no events.
    }
}

impl Dispatch<WlBuffer, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        slot: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_buffer::Event;
        // `release` flips busy=false.
        if let Event::Release = event {
            if let Some(b) = state.buffer_pool.get_mut(*slot) {
                b.busy = false;
            }
        }
    }
}

#[cfg(test)]
impl Buffer {
    /// A buffer without a live `wl_buffer`/mmap, for slot-logic tests.
    fn dummy(width: u32, height: u32, busy: bool) -> Self {
        Self {
            wl_buffer: None,
            mmap: None,
            width,
            height,
            busy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_with(buffers: &[(u32, u32, bool)]) -> BufferPool {
        let mut pool = BufferPool::new();
        for &(w, h, busy) in buffers {
            pool.slots.push(Some(Buffer::dummy(w, h, busy)));
        }
        pool
    }

    #[test]
    fn reuse_idle_matching_slot() {
        let pool = pool_with(&[(100, 50, true), (100, 50, false)]);
        assert_eq!(pool.select_slot(100, 50), SlotDecision::Reuse(1));
    }

    #[test]
    fn matching_beats_earlier_mismatched() {
        let pool = pool_with(&[(200, 80, false), (100, 50, false)]);
        assert_eq!(pool.select_slot(100, 50), SlotDecision::Reuse(1));
    }

    #[test]
    fn reinit_idle_mismatched_slot() {
        let pool = pool_with(&[(100, 50, false)]);
        assert_eq!(pool.select_slot(200, 80), SlotDecision::Reinit(0));
    }

    #[test]
    fn fill_free_slot_when_no_idle() {
        let mut pool = pool_with(&[(100, 50, true)]);
        pool.slots.push(None); // slot 1: free
        assert_eq!(pool.select_slot(100, 50), SlotDecision::FillFree(1));
    }

    #[test]
    fn push_when_all_busy_no_free() {
        let pool = pool_with(&[(100, 50, true)]);
        assert_eq!(pool.select_slot(100, 50), SlotDecision::Push);
    }

    #[test]
    fn busy_buffers_are_never_selected() {
        // A busy matching-size buffer must not be reused.
        let pool = pool_with(&[(100, 50, true)]);
        assert_ne!(pool.select_slot(100, 50), SlotDecision::Reuse(0));
    }

    #[test]
    fn cull_removes_idle_over_cap() {
        let pool = pool_with(&[(100, 50, false); 5]);
        let mut pool = pool;
        pool.cull_buffers();
        let live = pool.slots.iter().filter(|s| s.is_some()).count();
        assert_eq!(live, MAX_BUFFER_MULTIPLICITY);
    }

    #[test]
    fn cull_preserves_busy_buffers() {
        // 1 busy + 4 idle = 5 live; cull to 3, busy must survive.
        let mut pool = pool_with(&[
            (100, 50, true),
            (100, 50, false),
            (100, 50, false),
            (100, 50, false),
            (100, 50, false),
        ]);
        pool.cull_buffers();
        assert!(pool.slots[0].as_ref().is_some_and(|b| b.busy));
        let live = pool.slots.iter().filter(|s| s.is_some()).count();
        assert_eq!(live, MAX_BUFFER_MULTIPLICITY);
    }

    #[test]
    fn cull_noop_at_or_under_cap() {
        let mut pool = pool_with(&[(100, 50, false); 3]);
        pool.cull_buffers();
        let live = pool.slots.iter().filter(|s| s.is_some()).count();
        assert_eq!(live, 3);
    }
}
