//! OS-level page-locked secret memory buffer.
//!
//! Direct `mmap(2)` (`MAP_PRIVATE | MAP_ANONYMOUS`) page allocation, `mlock(2)`
//! page locking, `madvise(MADV_DONTDUMP | MADV_WIPEONFORK)` protections,
//! process-wide `RLIMIT_CORE = 0`, and `zeroize::Zeroize` on `Drop`.
//!
//! Secret content is UTF-8 with codepoint-aware append/delete and a fixed
//! single-page capacity.
use std::fmt;
use std::ptr::NonNull;

use libc::{c_void, size_t};
use zeroize::Zeroize;

/// Error returned by [`SecretBuffer`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// `mlock(2)` failed too often (>= 10 attempts, mostly `EAGAIN`).
    MlockFailedTooOften,
    /// `madvise(2)` returned an unexpected errno (non-`EAGAIN`) or failed all
    /// 10 attempts.
    MadviseFailedTooOften,
    /// `mmap(2)` failed.
    MmapFailed(i32),
    /// `setrlimit(RLIMIT_CORE, 0)` failed.
    SetRlimitCoreFailed(i32),
    /// `munmap(2)` failed during reset.
    MunmapFailed(i32),
    /// `append_slice` exceeded the single-page capacity.
    Overflow,
    /// `append_slice` was given invalid UTF-8.
    InvalidUtf8,
    /// Buffer pointer became null unexpectedly.
    NullBuffer,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MlockFailedTooOften => f.write_str("mlock failed too often (>= 10 attempts)"),
            Self::MadviseFailedTooOften => f.write_str("madvise failed too often (>= 10 attempts)"),
            Self::MmapFailed(e) => {
                write!(f, "mmap failed with errno {e}")
            }
            Self::SetRlimitCoreFailed(e) => {
                write!(f, "setrlimit(RLIMIT_CORE, 0) failed with errno {e}")
            }
            Self::MunmapFailed(e) => {
                write!(f, "munmap failed with errno {e}")
            }
            Self::Overflow => f.write_str("secret buffer overflow"),
            Self::InvalidUtf8 => f.write_str("invalid UTF-8 in append_slice"),
            Self::NullBuffer => f.write_str("secret buffer pointer is null"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Maximum retry attempts for `mlock` and `MADV_DONTDUMP` (`EAGAIN` is
/// transient).
const RETRY_ATTEMPTS: usize = 10;

/// Set `RLIMIT_CORE` to 0 process-wide, preventing core dumps. MUST be called
/// at startup before any secret allocation. Hard error on failure.
///
/// # Safety
/// Calls `libc::setrlimit`, a POSIX syscall. Safe wrapper around unsafe FFI.
pub fn set_rlimit_core_zero() -> Result<(), SecretError> {
    // SAFETY: `setrlimit` with `RLIMIT_CORE` and a zero rlimStruct is a
    // well-defined POSIX operation. The rlimit struct is stack-allocated
    // and fully initialized.
    let rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `setrlimit` reads the rlimit struct by const reference; no
    // allocation or mutation occurs.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &rlim) };
    if rc == 0 {
        Ok(())
    } else {
        Err(SecretError::SetRlimitCoreFailed(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ))
    }
}

/// A page-aligned, locked, non-dumpable secret byte buffer.
///
/// Holds at most one system page of UTF-8 bytes and tracks the codepoint
/// count. Never heap-allocates; the backing
/// storage is a single `mmap(MAP_PRIVATE | MAP_ANONYMOUS)` page.
pub struct SecretBuffer {
    /// Page-aligned `mmap` pointer, non-null while allocated.
    ptr: NonNull<u8>,
    /// Total byte capacity (one system page).
    capacity: size_t,
    /// Current byte length of the UTF-8 content.
    byte_len: size_t,
    /// Current Unicode codepoint count.
    len: size_t,
}

impl SecretBuffer {
    /// Allocate a new locked, non-dumpable secret buffer of one system page.
    pub fn new() -> Result<Self, SecretError> {
        // SAFETY: `sysconf(_SC_PAGESIZE)` is a pure query, no side effects.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as size_t;
        if page_size == 0 {
            return Err(SecretError::MmapFailed(libc::EINVAL));
        }

        // SAFETY: `mmap(NULL, page_size, PROT_READ|PROT_WRITE,
        // MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)` is the canonical anonymous page
        // allocation. `fd` is ignored for `MAP_ANONYMOUS`.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(SecretError::MmapFailed(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ));
        }

        let ptr = NonNull::new(raw as *mut u8).ok_or(SecretError::NullBuffer)?;

        let mut buf = Self {
            ptr,
            capacity: page_size,
            byte_len: 0,
            len: 0,
        };

        buf.lock_and_protect()?;
        Ok(buf)
    }

    /// Apply `mlock`, `MADV_DONTDUMP`, `MADV_WIPEONFORK` to the current page.
    /// On failure the caller is responsible for `munmap`-ing the page.
    fn lock_and_protect(&mut self) -> Result<(), SecretError> {
        // mlock with bounded retry on EAGAIN; hard error otherwise.
        let mut attempts = 0;
        loop {
            // SAFETY: `mlock` on our freshly-mapped page with the exact
            // capacity length is well-defined.
            let rc = unsafe { libc::mlock(self.ptr.as_ptr() as *const c_void, self.capacity) };
            if rc == 0 {
                break;
            }
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EAGAIN {
                attempts += 1;
                if attempts >= RETRY_ATTEMPTS {
                    return Err(SecretError::MlockFailedTooOften);
                }
                continue;
            }
            return Err(SecretError::MlockFailedTooOften);
        }

        // MADV_DONTDUMP with bounded retry on EAGAIN; hard error otherwise
        // (Linux only).
        #[cfg(target_os = "linux")]
        {
            let mut attempts = 0;
            loop {
                // SAFETY: `madvise` on our locked page is well-defined.
                let rc = unsafe {
                    libc::madvise(
                        self.ptr.as_ptr() as *mut c_void,
                        self.capacity,
                        libc::MADV_DONTDUMP,
                    )
                };
                if rc == 0 {
                    break;
                }
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if errno == libc::EAGAIN {
                    attempts += 1;
                    if attempts >= RETRY_ATTEMPTS {
                        return Err(SecretError::MadviseFailedTooOften);
                    }
                    continue;
                }
                return Err(SecretError::MadviseFailedTooOften);
            }

            // MADV_WIPEONFORK is best-effort: log warning on EINVAL/ENOSYS,
            // continue. Not present on older kernels (<4.14).
            // SAFETY: same as MADV_DONTDUMP above.
            let rc = unsafe {
                libc::madvise(
                    self.ptr.as_ptr() as *mut c_void,
                    self.capacity,
                    libc::MADV_WIPEONFORK,
                )
            };
            if rc != 0 {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                // Best-effort: log and continue. Not a hard failure.
                eprintln!(
                    "warning: MADV_WIPEONFORK failed with errno {errno}; \
                     fork protection disabled"
                );
            }
        }

        Ok(())
    }

    /// Append a UTF-8 slice. Counts codepoints; returns [`SecretError::Overflow`]
    /// if `byte_len + bytes.len()` exceeds the page capacity. On overflow the
    /// buffer is unchanged: validation and capacity checks complete before
    /// mutation.
    pub fn append_slice(&mut self, bytes: &[u8]) -> Result<(), SecretError> {
        // Validate UTF-8 and count codepoints first; reject before mutation.
        let s = std::str::from_utf8(bytes).map_err(|_| SecretError::InvalidUtf8)?;
        let codepoints = s.chars().count();

        let new_byte_len = self
            .byte_len
            .checked_add(bytes.len())
            .ok_or(SecretError::Overflow)?;
        if new_byte_len > self.capacity {
            return Err(SecretError::Overflow);
        }

        // SAFETY: copy bytes into the mapped page at the current offset.
        // `new_byte_len <= self.capacity` guarantees in-bounds writes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.as_ptr().add(self.byte_len),
                bytes.len(),
            );
        }

        self.byte_len = new_byte_len;
        self.len += codepoints;
        Ok(())
    }

    /// Remove the last UTF-8 codepoint from the buffer. No-op if empty.
    ///
    /// Walks backwards from the last byte, decoding the trailing UTF-8
    /// sequence length, and truncates by exactly that many bytes.
    pub fn delete_backwards(&mut self) {
        if self.byte_len == 0 {
            return;
        }
        // Find the start of the last UTF-8 codepoint: walk back while the
        // byte is a continuation byte (10xxxxxx).
        let mut i = self.byte_len - 1;
        while i > 0 {
            let byte = self.get_byte(i).unwrap_or(0);
            // Continuation bytes have the top two bits `10`.
            if (byte & 0xC0) != 0x80 {
                break;
            }
            i -= 1;
        }
        // Truncate by the codepoint's byte length.
        let cp_byte_len = self.byte_len - i;
        // SAFETY: zeroize the removed bytes in place, then update length.
        // We zeroize the trailing `cp_byte_len` bytes for hygiene.
        unsafe {
            let start = self.ptr.as_ptr().add(i);
            std::ptr::write_bytes(start, 0u8, cp_byte_len);
        }
        self.byte_len = i;
        self.len = self.len.saturating_sub(1);
    }

    /// Read a single byte at `idx` from the buffer.
    fn get_byte(&self, idx: size_t) -> Option<u8> {
        if idx >= self.byte_len {
            return None;
        }
        // SAFETY: `idx < self.byte_len <= self.capacity`, in-bounds read.
        Some(unsafe { *self.ptr.as_ptr().add(idx) })
    }

    /// Reset the buffer: zeroize, munmap, re-mmap, re-mlock, re-madvise.
    /// Equivalent to `drop` + `new`.
    pub fn reset(&mut self) -> Result<(), SecretError> {
        self.zeroize_and_unmap()?;
        // Re-allocate a fresh page. `new` handles mlock + madvise.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as size_t;
        if page_size == 0 {
            return Err(SecretError::MmapFailed(libc::EINVAL));
        }
        // SAFETY: same canonical anonymous mmap as `new`.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(SecretError::MmapFailed(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ));
        }
        self.ptr = NonNull::new(raw as *mut u8).ok_or(SecretError::NullBuffer)?;
        self.capacity = page_size;
        self.byte_len = 0;
        self.len = 0;
        self.lock_and_protect()?;
        Ok(())
    }

    /// Return the valid secret bytes, or `None` if empty.
    pub fn slice(&self) -> Option<&[u8]> {
        if self.byte_len == 0 {
            return None;
        }
        // SAFETY: `byte_len <= capacity`, the slice is within the mapped page.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.byte_len) })
    }

    /// Current codepoint count.
    #[allow(dead_code)]
    pub fn len(&self) -> size_t {
        self.len
    }

    /// `true` if the buffer holds zero codepoints.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.byte_len == 0
    }

    /// Zeroize the page contents in place, then `munmap` the page.
    fn zeroize_and_unmap(&mut self) -> Result<(), SecretError> {
        // SAFETY: zeroize the valid bytes only; byte_len <= capacity.
        let valid: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.byte_len.max(1)) };
        // Zeroize only the actual content bytes.
        if self.byte_len > 0 {
            valid[..self.byte_len].zeroize();
        }
        // SAFETY: `munmap` releases the page we previously `mmap`-ed.
        let rc = unsafe { libc::munmap(self.ptr.as_ptr() as *mut c_void, self.capacity) };
        if rc != 0 {
            return Err(SecretError::MunmapFailed(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ));
        }
        Ok(())
    }
}

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        // Best-effort cleanup in Drop: zeroize + munmap. Ignore errors.
        let valid: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.byte_len.max(1)) };
        if self.byte_len > 0 {
            valid[..self.byte_len].zeroize();
        }
        // SAFETY: same as `zeroize_and_unmap`.
        unsafe {
            libc::munmap(self.ptr.as_ptr() as *mut c_void, self.capacity);
        }
    }
}

// Manual Send/Sync: the buffer owns its mmap'd page exclusively. It is safe
// to move across threads (`Send`); not safe to share references (`!Sync` by
// default since interior mutation is unsynchronized). We implement `Send`
// explicitly because the raw pointer opt-outs the auto-impl.
unsafe impl Send for SecretBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_buf() -> SecretBuffer {
        // Tests skip the RLIMIT_CORE process-wide mutation; `new()` exercises
        // the full mmap/mlock/madvise path.
        SecretBuffer::new().expect(
            "SecretBuffer::new failed; \
            check RLIMIT_MEMLOCK (`ulimit -l`) and kernel MADV support",
        )
    }

    #[test]
    fn empty_slice_is_none() {
        let buf = new_buf();
        assert!(buf.slice().is_none());
    }

    #[test]
    fn append_and_slice() {
        let mut buf = new_buf();
        buf.append_slice(b"hello").unwrap();
        assert_eq!(buf.slice().unwrap(), b"hello");
        assert_eq!(buf.len(), 5);
    }

    #[test]
    fn reset_then_append_and_delete() {
        let mut buf = new_buf();
        buf.append_slice(b"1234").unwrap();
        assert_eq!(buf.slice().unwrap(), b"1234");
        buf.delete_backwards();
        assert_eq!(buf.slice().unwrap(), b"123");
        buf.delete_backwards();
        assert_eq!(buf.slice().unwrap(), b"12");
        buf.delete_backwards();
        assert_eq!(buf.slice().unwrap(), b"1");
        buf.delete_backwards();
        assert!(buf.slice().is_none());
        // Delete on empty is a no-op.
        buf.delete_backwards();
        buf.delete_backwards();
        buf.append_slice(b"abc").unwrap();
        assert_eq!(buf.slice().unwrap(), b"abc");
    }

    #[test]
    fn reset_reinitializes() {
        let mut buf = new_buf();
        buf.append_slice(b"abc").unwrap();
        assert_eq!(buf.slice().unwrap(), b"abc");
        buf.reset().unwrap();
        assert!(buf.slice().is_none());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn overflow_returns_error_and_is_unchanged() {
        let mut buf = new_buf();
        // Capacity is one page (typically 4096 B). Fill most of it.
        let fill = b"a".repeat(buf.capacity - 100);
        buf.append_slice(&fill).unwrap();
        let before = buf.byte_len;
        // 200 bytes would exceed capacity by 100.
        let too_big = b"a".repeat(200);
        let err = buf.append_slice(&too_big).unwrap_err();
        assert_eq!(err, SecretError::Overflow, "expected Overflow");
        assert_eq!(buf.byte_len, before, "buffer must be unchanged on overflow");
    }

    #[test]
    fn multi_byte_codepoint_append_and_delete() {
        let mut buf = new_buf();
        // é is 2 bytes, 1 codepoint.
        buf.append_slice("é".as_bytes()).unwrap();
        assert_eq!(buf.byte_len, 2);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.slice().unwrap(), "é".as_bytes());
        buf.delete_backwards();
        assert!(buf.slice().is_none());
        assert_eq!(buf.len(), 0);

        // 𝕏 (U+1D54F) is 4 bytes, 1 codepoint.
        buf.append_slice("𝕏".as_bytes()).unwrap();
        assert_eq!(buf.byte_len, 4);
        assert_eq!(buf.len(), 1);
        buf.delete_backwards();
        assert!(buf.slice().is_none());
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut buf = new_buf();
        // Lone continuation byte is invalid UTF-8.
        let err = buf.append_slice(&[0x80]).unwrap_err();
        assert_eq!(err, SecretError::InvalidUtf8);
        assert_eq!(buf.byte_len, 0, "buffer must be unchanged on invalid UTF-8");
    }
}
