## Purpose

Defines protected allocation, lifetime, and input handling for secret bytes.

## Requirements

### Requirement: OS-level page-locked secret memory allocation

The secret memory module MUST allocate a single page-aligned buffer via `libc::mmap(MAP_PRIVATE | MAP_ANONYMOUS)`, apply `libc::mlock` with a bounded retry loop (up to 10 attempts, retrying on `EAGAIN`), and abort with a hard error on exhaustion. The buffer MUST NOT use `std::alloc::alloc`, `String`, or `Vec<u8>`.

#### Scenario: Successful allocation and mlock
- **WHEN** `SecretBuffer::new()` is called on a Linux system with sufficient `RLIMIT_MEMLOCK`
- **THEN** a page-aligned buffer of one system page size is allocated via `mmap`, locked with `mlock`, marked `MADV_DONTDUMP` and `MADV_WIPEONFORK`, and returned as an initialized `SecretBuffer`

#### Scenario: mlock temporary failure (EAGAIN)
- **WHEN** `mlock` returns `EAGAIN` on an attempt
- **THEN** the module retries up to 10 times before returning a hard error

#### Scenario: mlock permanent failure
- **WHEN** `mlock` returns an error other than `EAGAIN` or fails all 10 attempts
- **THEN** the module returns a hard error and does not leave a locked/allocated buffer

### Requirement: Coredump and fork protection

The secret buffer page MUST be excluded from core dumps via `libc::madvise(MADV_DONTDUMP)` and wiped on fork via `libc::madvise(MADV_WIPEONFORK)` on Linux. `MADV_WIPEONFORK` failure MUST be treated as best-effort (log warning, continue); `MADV_DONTDUMP` failure MUST be a hard error.

#### Scenario: MADV_DONTDUMP unavailable
- **WHEN** `madvise(MADV_DONTDUMP)` returns an error other than `EAGAIN`
- **THEN** the module returns a hard error after retry exhaustion

#### Scenario: MADV_WIPEONFORK unavailable
- **WHEN** `madvise(MADV_WIPEONFORK)` returns `EINVAL` or `ENOSYS`
- **THEN** the module logs a warning and continues initialization (best-effort)

### Requirement: RLIMIT_CORE = 0 process-wide

The process MUST set `RLIMIT_CORE` to 0 at startup (before any secret allocation) via `libc::setrlimit`, preventing core dumps process-wide. Failure to set `RLIMIT_CORE` MUST be a hard error.

#### Scenario: setrlimit succeeds
- **WHEN** the process calls `setrlimit(RLIMIT_CORE, 0)` at startup
- **THEN** the core dump resource limit is zero for the process lifetime

#### Scenario: setrlimit fails
- **WHEN** `setrlimit(RLIMIT_CORE, 0)` fails
- **THEN** the process aborts with a hard error before allocating any secret memory

### Requirement: Zeroization and munmap on Drop

When a `SecretBuffer` is dropped, the buffer contents MUST be zeroized in place via `zeroize::Zeroize` before `libc::munmap` releases the page. No secret bytes may remain in process memory after drop.

#### Scenario: Drop zeroizes then unmaps
- **WHEN** a `SecretBuffer` holding secret bytes is dropped
- **THEN** the buffer is zeroized in place, then `munmap` releases the page; no copy of the secret remains in user space

### Requirement: Append-slice, delete-backwards, reset, and slice accessor

The `SecretBuffer` MUST provide:
- `append_slice(&[u8])`: append UTF-8 bytes, counting codepoints for the length field; return an error on overflow (capacity exceeded).
- `delete_backwards()`: remove the last UTF-8 codepoint (decode trailing byte sequence length); no-op if empty.
- `reset()`: zeroize, `munmap`, re-`mmap`, re-`mlock`, re-`madvise` — equivalent to drop + new.
- `slice() -> Option<&[u8]>`: return the valid secret bytes or `None` if empty.

#### Scenario: Append within capacity
- **WHEN** `append_slice("hello")` is called on an empty buffer
- **THEN** the buffer holds `"hello"`, length is 5 codepoints, and `slice()` returns `Some("hello")`

#### Scenario: Append exceeds capacity
- **WHEN** `append_slice` would exceed the page capacity
- **THEN** the method returns an overflow error and the buffer is unchanged

#### Scenario: Delete backwards on non-empty buffer
- **WHEN** `delete_backwards()` is called on a buffer holding `"1234"`
- **THEN** the buffer holds `"123"` and length decrements by one codepoint

#### Scenario: Delete backwards on empty buffer
- **WHEN** `delete_backwards()` is called on an empty buffer
- **THEN** the buffer remains empty (no-op)

#### Scenario: Reset clears and reinitializes
- **WHEN** `reset()` is called on a buffer holding `"abc"`
- **THEN** the previous content is zeroized, a fresh page is allocated/locked, and `slice()` returns `None`

### Requirement: UTF-8 codepoint-aware length tracking

The `len` field MUST count Unicode codepoints, not bytes. `append_slice` MUST validate UTF-8 and count codepoints; `delete_backwards` MUST decode the trailing UTF-8 byte sequence length to remove exactly one codepoint.

#### Scenario: Multi-byte codepoint append
- **WHEN** `append_slice("é")` (2 bytes, 1 codepoint) is called
- **THEN** byte length is 2 but codepoint length is 1

#### Scenario: Multi-byte codepoint delete
- **WHEN** `delete_backwards()` is called on a buffer ending in `"é"` (bytes `0xC3 0xA9`)
- **THEN** both bytes are removed and codepoint length decrements by 1
