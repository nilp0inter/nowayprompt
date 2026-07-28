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
    ///
    /// All physical dimensions, the 4-byte stride, the total byte size,
    /// and the conversions to Wayland's signed request arguments and the
    /// host `usize` are validated with checked arithmetic before any
    /// allocation. On failure no partial buffer is created or attached.
    pub fn new(
        shm: &WlShm,
        qh: &QueueHandle<WaylandState>,
        slot: usize,
        width: u32,
        height: u32,
    ) -> Result<Self, std::io::Error> {
        // All physical-dimension, stride, byte-size, and signed-argument
        // conversions are validated up front; on failure no partial
        // buffer is created or attached.
        let dims = validate_buffer_dimensions(width, height)?;
        let ValidatedDims {
            size_usize,
            width_i,
            height_i,
            stride_i,
            size_i,
            ..
        } = dims;
        let size_off = size_i as libc::off_t;

        // memfd_create with MFD_CLOEXEC.
        let fd: OwnedFd = unsafe {
            let raw = libc::memfd_create(c"/nowayprompt".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(raw)
        };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size_off) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // mmap MAP_SHARED + PROT_READ|PROT_WRITE via MmapOptions. The
        // client's fd is closed when `fd` drops at function end (the
        // compositor dups it for the pool; the mmap persists).
        let mmap = unsafe {
            memmap2::MmapOptions::new()
                .len(size_usize)
                .map_mut(&fd)
                .map_err(std::io::Error::other)?
        };

        // Create the wl_shm_pool and buffer. The pool is destroyed
        // immediately; the buffer keeps the backing alive.
        let pool: WlShmPool = shm.create_pool(fd.as_fd(), size_i, qh, ());
        let wl_buffer =
            pool.create_buffer(0, width_i, height_i, stride_i, Format::Argb8888, qh, slot);
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

fn overflow() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "buffer dimensions overflow Wayland or address-space limits",
    )
}

/// Validated SHM buffer dimensions: the physical width/height, the
/// 4-byte Argb8888 stride, the total byte size, and the signed-32-bit
/// Wayland request arguments. Produced by [`validate_buffer_dimensions`]
/// and consumed by [`Buffer::new`].
struct ValidatedDims {
    #[allow(dead_code)]
    stride: u32,
    #[allow(dead_code)]
    size: u32,
    size_usize: usize,
    width_i: i32,
    height_i: i32,
    stride_i: i32,
    size_i: i32,
}

/// Validate SHM buffer physical dimensions, the 4-byte Argb8888 stride,
/// the total byte size, and every conversion to Wayland's signed request
/// arguments and the host `usize`. Pure (no allocation, no compositor).
/// Returns the validated values so [`Buffer::new`] can proceed without
/// re-checking.
fn validate_buffer_dimensions(width: u32, height: u32) -> Result<ValidatedDims, std::io::Error> {
    // Physical dimensions are positive.
    if width == 0 || height == 0 {
        return Err(zero());
    }
    // 4-byte stride (Argb8888), checked.
    let stride = width.checked_mul(4).ok_or_else(overflow)?;
    // Total byte size, checked.
    let size = stride.checked_mul(height).ok_or_else(overflow)?;
    // Host usize must hold the byte size.
    let size_usize = usize::try_from(size).map_err(|_| overflow())?;
    if size_usize == 0 {
        return Err(zero());
    }
    // Wayland request arguments are signed 32-bit.
    let width_i = i32::try_from(width).map_err(|_| overflow())?;
    let height_i = i32::try_from(height).map_err(|_| overflow())?;
    let stride_i = i32::try_from(stride).map_err(|_| overflow())?;
    let size_i = i32::try_from(size).map_err(|_| overflow())?;
    Ok(ValidatedDims {
        stride,
        size,
        size_usize,
        width_i,
        height_i,
        stride_i,
        size_i,
    })
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

    // --- SHM dimension validation (Task 5.1) ---
    // Pure tests of `validate_buffer_dimensions`: zero rejection,
    // stride overflow, byte-size overflow, and i32/usize limit
    // rejection — no live WlShm or compositor needed.

    #[test]
    fn validate_rejects_zero_dimensions() {
        assert!(validate_buffer_dimensions(0, 100).is_err());
        assert!(validate_buffer_dimensions(100, 0).is_err());
        assert!(validate_buffer_dimensions(0, 0).is_err());
    }

    #[test]
    fn validate_accepts_normal_dimensions() {
        let dims = validate_buffer_dimensions(100, 50).unwrap();
        assert_eq!(dims.stride, 400);
        assert_eq!(dims.size, 20_000);
        assert_eq!(dims.size_usize, 20_000);
        assert_eq!(dims.width_i, 100);
        assert_eq!(dims.height_i, 50);
        assert_eq!(dims.stride_i, 400);
        assert_eq!(dims.size_i, 20_000);
    }

    #[test]
    fn validate_rejects_stride_overflow() {
        // width * 4 overflows u32: width near u32::MAX.
        assert!(validate_buffer_dimensions(u32::MAX, 1).is_err());
        assert!(validate_buffer_dimensions(u32::MAX / 4 + 1, 1).is_err());
    }

    #[test]
    fn validate_rejects_byte_size_overflow() {
        // stride * height overflows u32: large width and height.
        // width = 65536 → stride = 262144; height = u32::MAX → overflow.
        assert!(validate_buffer_dimensions(65536, u32::MAX).is_err());
    }

    #[test]
    fn validate_rejects_exceeding_i32_width() {
        // width > i32::MAX overflows the signed Wayland argument.
        // width = i32::MAX as u32 + 1; stride = (that)*4 which overflows
        // u32 first, so pick a value where stride fits but width > i32::MAX.
        // Actually width > i32::MAX means width >= 2^31, stride = width*4
        // always overflows u32 for width >= 2^31. So this is covered by
        // stride overflow. Instead test height exceeding i32::MAX with
        // a small width so stride fits but height_i overflows.
        let w = 1; // stride = 4, fits
        let h = i32::MAX as u32 + 1; // > i32::MAX
                                     // size = 4 * h; for h = 2^31, size = 2^33 which overflows u32.
                                     // So byte-size overflow catches it first. The signed-arg check
                                     // is reachable only when size fits i32 but a dimension doesn't,
                                     // which can't happen since size >= height when stride >= 1.
                                     // This test confirms the overflow is caught regardless.
        assert!(validate_buffer_dimensions(w, h).is_err());
    }

    #[test]
    fn validate_boundary_just_fits() {
        // width = i32::MAX, height = 1: stride = i32::MAX * 4 overflows
        // u32. Find the largest width where stride fits u32: width =
        // u32::MAX / 4 = 1073741823. stride = 4294967292. size = stride.
        // But stride * 1 = 4294967292 which exceeds i32::MAX, so size_i
        // overflows i32.
        // The largest fully-valid: width = 536870911 (just under
        // i32::MAX / 4), stride = 2147483644, size = 2147483644.
        let dims = validate_buffer_dimensions(536_870_911, 1).unwrap();
        assert_eq!(dims.stride, 2_147_483_644);
        assert_eq!(dims.width_i, 536_870_911);
        assert_eq!(dims.height_i, 1);
        assert_eq!(dims.stride_i, 2_147_483_644);
        assert_eq!(dims.size_i, 2_147_483_644);
    }
}
