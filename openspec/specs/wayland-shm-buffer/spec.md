## Purpose

Defines the Wayland frontend's SHM buffer management: memfd-backed `wl_shm` pools, a triple-buffer arena with busy-state tracking, buffer identity via `Dispatch` user-data index, and teardown.

## Requirements

### Requirement: SHM buffer allocation via memfd_create

The `src/frontend/wayland/shm.rs` module MUST allocate per-buffer SHM via `libc::memfd_create("/wayprompt", MFD_CLOEXEC)` + `libc::ftruncate(fd, size)` + `memmap2::MmapMut` with `MAP_SHARED` + `PROT_READ|PROT_WRITE` (writeable pixel buffer). The `wl_shm_pool` MUST be created from the fd with the buffer size, and `wl_buffer` created with `Format::Argb8888` and stride `width * 4` (parity with `Wayland.zig:1362-1410`).

#### Scenario: buffer allocated with correct format
- **WHEN** a new buffer is created at width W and height H
- **THEN** the `wl_buffer` is created with `Format::Argb8888`, stride `W*4`, size `W*H*4`, and a writeable `MmapMut` backing

#### Scenario: memfd is CLOEXEC
- **WHEN** `memfd_create` is called
- **THEN** the `MFD_CLOEXEC` flag is set so the fd does not leak across exec

### Requirement: Triple-buffer pool with busy-state tracking

The `BufferPool` MUST hold up to `max_buffer_multiplicity=3` buffers per surface (`globalSurfaceCount=1`). `nextBuffer` MUST reuse an idle (`busy=false`) buffer of matching dimensions, else re-init an idle mismatched-size buffer, else allocate a new buffer. `cullBuffers` MUST destroy idle buffers exceeding the cap. The `wl_buffer.release` event MUST flip the buffer's `busy` flag to `false` (parity with `Wayland.zig:1256-1351,1418-1422`).

#### Scenario: idle buffer reused
- **WHEN** `nextBuffer` is called and an idle buffer of matching dimensions exists
- **THEN** that buffer is returned without allocation

#### Scenario: idle mismatched buffer re-init
- **WHEN** `nextBuffer` is called, no matching-size idle buffer exists, but an idle mismatched-size buffer exists
- **THEN** that buffer is deinit'd and re-init'd to the new dimensions

#### Scenario: new buffer allocated when pool not full
- **WHEN** `nextBuffer` is called and no idle buffer is available and the pool is under the cap
- **THEN** a new buffer is allocated and appended to the pool

#### Scenario: release flips busy to false
- **WHEN** the compositor sends `wl_buffer.release` for buffer index I
- **THEN** `buffers[I].busy` becomes `false`

### Requirement: Buffer identity via Dispatch user-data index

The buffer pool MUST be a `Vec<Buffer>` arena. Each `wl_buffer` MUST carry its index in the `Dispatch<WlBuffer, usize>` user-data slot so the `.release` event can locate the buffer without stable pointers (D5).

#### Scenario: release event locates the buffer
- **WHEN** a `wl_buffer.release` event arrives with user-data index I
- **THEN** the handler indexes `state.buffers[I]` and sets `busy = false`

### Requirement: Buffer teardown

`Buffer::deinit` MUST unref the `tiny-skia` pixmap (or equivalent), destroy the `wl_buffer`, and `munmap`/drop the `MmapMut` (parity with `Wayland.zig:1412-1416`). `BufferPool::deinit` MUST deinit all buffers and clear the pool.

#### Scenario: buffer deinit releases all resources
- **WHEN** `Buffer::deinit` is called
- **THEN** the `wl_buffer` is destroyed, the mmap is dropped, and the pixmap is unreferenced
