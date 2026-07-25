# Rigorous Adversarial Security & POSIX Systems Critique of `nowayprompt`

This document provides a detailed security and systems-level critique of the proposed Rust rewrite plan outlined in `RUST_REWRITE.md` and the architecture specifications in `reference/security_tty_ipc.md`.

---

## 1. Protected Secret Memory (`SecretBuffer`)

### 1.1 Memory Allocation Boundaries and Allocator Reuse Risks
*   **The Flaw:** Rounding up the allocation size to system page size and calling `std::alloc::alloc` does not guarantee that the returned pointer points to a dedicated set of physical memory pages managed exclusively from the OS kernel level.
*   **Mechanics & Failure Modes:** The default allocator (`dlmalloc`, `jemalloc`, or Rust's system allocator wrapper) manages large blocks of memory (arenas) and satisfies small aligned allocations by partitioning those arenas. When `libc::mlock` is called on a sub-segment of an allocator arena:
    1.  Adjacent heap metadata or unrelated active allocations sharing the same page will also be locked into RAM, causing resource exhaustion.
    2.  More critically, upon deallocation (`alloc::dealloc`), the memory is returned to the allocator's free list. The allocator does not release this physical memory back to the kernel immediately. Un-zeroed cached structures or thread-local caches may reassign the same memory address containing residues of the secret, or expose it to other parts of the program through allocator-internal copies.
*   **Mitigation:** Bypass the standard library allocator entirely for secret buffers. Allocate and deallocate memory directly from the operating system kernel using `mmap(2)` / `munmap(2)` syscalls with `MAP_PRIVATE | MAP_ANONYMOUS`.
    ```rust
    // Direct page-aligned OS allocation bypassing heap allocators
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            aligned_capacity,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(SecretBufferError::AllocationFailed);
    }
    ```

### 1.2 Fork/Exec Memory Safety & Copy-On-Write (COW) Leaks
*   **The Flaw:** Memory locks (`mlock`) and dump restrictions (`MADV_DONTDUMP`) are **not** inherited by child processes across a `fork()` system call.
*   **Mechanics & Failure Modes:** 
    When the parent process forks:
    1.  The child process inherits the parent's address space. However, all page locks applied via `mlock` are discarded in the child.
    2.  The kernel marks these pages as Copy-on-Write (COW). When either parent or child writes to the secret buffer, a new page allocation occurs. The secret buffer is copied to a new physical page in the child process, which is **unlocked** (can be swapped to disk) and **unprotected** (will be dumped on crash).
    3.  If the child does not immediately execute an `exec()` family call, or if it crashes/coredumps, the secret is leaked to the disk.
*   **Mitigation:** 
    Mark the memory pages with `libc::MADV_WIPEONFORK` (on Linux 4.14+) using `madvise`. This ensures that after a fork, the child's copy of the page is immediately replaced with zeroes, preventing any COW secret propagation.
    ```rust
    unsafe {
        libc::madvise(
            self.ptr.as_ptr() as *mut libc::c_void,
            self.capacity,
            libc::MADV_WIPEONFORK,
        );
    }
    ```

### 1.3 Temporary Heap Allocations During Formatting and Decapsulation
*   **The Flaw:** Secret text data is formatted or decoded using standard library types (`String`, `Vec<u8>`) and macro formatters, bypassing the locked buffer protections.
*   **Mechanics & Failure Modes:**
    In the Assuan REPL command processing:
    1.  The command `GETPIN` returns the secret by calling `format!("D {}\n", mock_pin)`. The `format!` macro allocates a temporary `String` on the general heap. Once written to the stream, this string is dropped, leaving the plaintext password in un-zeroed heap memory.
    2.  The percent-decoding function `assuan_decode` returns a `Result<String, _>`. This allocates a standard `String` on the un-locked heap before copying it to the client.
*   **Mitigation:** Implement streaming writes that bypass standard library allocations. Write prefix/suffix frames first, then write the raw bytes directly from the `SecretBuffer` pointer to the output file descriptor using unbuffered `std::io::Write` or direct `libc::write`.
    ```rust
    // Secure non-allocating response format
    writer.write_all(b"D ")?;
    writer.write_all(secret_buffer.as_slice())?;
    writer.write_all(b"\nEND\nOK\n")?;
    ```

### 1.4 Signal Termination and Coredump Leaks
*   **The Flaw:** Abnormal termination (e.g., `SIGSEGV`, `SIGKILL`, or `SIGSYS`) prevents the Rust `Drop` implementation of `SecretBuffer` from running, bypassing the zeroization phase.
*   **Mechanics & Failure Modes:** Rust destructors are only executed during clean stack unwinding or normal exit. If the process is terminated by an unhandled signal, the virtual memory space is destroyed, but pages may remain resident in RAM or written to core dumps if the kernel begins dumping before the process page tables are cleaned.
*   **Mitigation:** Use `libc::madvise(..., libc::MADV_DONTDUMP)` to ensure pages are excluded from core dumps even if abnormal termination occurs. Additionally, block core dump generation globally on process startup using `setrlimit` to set `RLIMIT_CORE` to `0`.

---

## 2. POSIX TTY & Raw Mode Restoration

### 2.1 Failure to Restore Termios State on Signal Termination
*   **The Flaw:** The raw TTY mode disables `ISIG`, which prevents `SIGINT`/`SIGQUIT`/`SIGTSTP` from being generated by keypresses, but does not block external signal delivery.
*   **Mechanics & Failure Modes:** If the process receives a termination signal from the OS (such as a remote `SIGINT`, `SIGTERM`, `SIGHUP`, or `SIGQUIT`), the process terminates instantly. Rust's `Drop` for `RawTty` is not invoked. The controlling terminal is left in raw mode (no local echo, non-canonical input), leaving the user's shell unresponsive and corrupted.
*   **Mitigation:** Register signal handlers using the `signal-hook` crate or a dedicated POSIX signal-handling loop. Upon catching a termination signal, the handler must restore the original termios settings using `tcsetattr(fd, TCSAFLUSH, &orig_termios)` before calling `_exit`.
    ```rust
    static mut ORIG_TERMIOS: Option<libc::termios> = None;
    static TTY_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

    unsafe extern "C" fn signal_handler(sig: libc::c_int) {
        if let Some(ref orig) = ORIG_TERMIOS {
            let fd = TTY_FD.load(std::sync::atomic::Ordering::Relaxed);
            if fd >= 0 {
                libc::tcsetattr(fd, libc::TCSAFLUSH, orig);
            }
        }
        libc::_exit(128 + sig);
    }
    ```

### 2.2 Broken Job Control & SIGTSTP Suspension Handling
*   **The Flaw:** If the application is suspended using `SIGTSTP` (e.g., manually sent by the shell or window manager), the terminal is left in raw mode while the program is stopped in the background.
*   **Mechanics & Failure Modes:** When a command-line process is backgrounded via `SIGTSTP`, control of the terminal is handed back to the parent shell. If the process does not restore the terminal settings prior to suspending, the shell will inherit the raw, non-echoing state, rendering it unusable. When the process receives `SIGCONT` to resume, it must re-apply raw mode.
*   **Mitigation:** Catch `SIGTSTP` explicitly. When caught, restore canonical mode, raise `SIGSTOP` on the process self to suspend, and re-apply raw TTY mode when `SIGCONT` resumes execution.
    ```rust
    // In signal handler
    if sig == libc::SIGTSTP {
        libc::tcsetattr(fd, libc::TCSAFLUSH, &orig_termios);
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        libc::raise(libc::SIGTSTP); // Suspends process
    }
    // After returning from suspend (SIGCONT handler)
    if sig == libc::SIGCONT {
        libc::tcsetattr(fd, libc::TCSAFLUSH, &raw_termios);
        // Re-register TSTP handler
    }
    ```

### 2.3 Input Stream Buffering Conflict
*   **The Flaw:** Mixing low-level `libc::read` directly on `RawFd` with buffered standard library readers (`std::io::Stdin`) introduces buffer incoherency.
*   **Mechanics & Failure Modes:** If any part of the application locks `stdin` or uses a buffered reader, bytes will be prefetched from the TTY into standard library memory buffers. Subsequent calls to `libc::read(fd, ...)` will block, failing to read keypresses already buffered in user-space, causing input hangs or dropped keystrokes.
*   **Mitigation:** Exclusively use direct raw reads on the descriptor, or use unbuffered standard library streams (`std::io::Read` directly on a raw stdin wrapper without locking).

---

## 3. INI Configuration & Assuan IPC Parser Security

### 3.1 Unbounded Buffering and Denial of Service (OOM)
*   **The Flaw:** Reading lines via `BufRead::lines()` allows unbounded memory allocation.
*   **Mechanics & Failure Modes:**
    1.  In `parse_config`, `reader.lines()` reads from a stream until a newline is found. If an attacker inputs a line containing megabytes of data without a newline, Rust will continuously reallocate the buffer on the heap until the process is terminated by the OS Out-Of-Memory (OOM) killer.
    2.  This vulnerability exists in both the INI configuration parser and the Assuan IPC REPL stdin loop.
*   **Mitigation:** Enforce strict line length limits by reading bytes incrementally up to a maximum threshold (e.g., 4096 bytes) using `take(4096).read_line(&mut buf)`.
    ```rust
    pub fn read_bounded_line<R: BufRead>(reader: &mut R, buf: &mut String, limit: u64) -> io::Result<usize> {
        buf.clear();
        let mut handle = reader.take(limit);
        let bytes_read = handle.read_line(buf)?;
        if bytes_read as u64 >= limit && !buf.ends_with('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Line length limit exceeded"));
        }
        Ok(bytes_read)
    }
    ```

### 3.2 Insecure Fallback in Assuan Percent-Decoding
*   **The Flaw:** The Assuan command parser falls back to raw undecoded strings when percent decoding fails.
*   **Mechanics & Failure Modes:**
    In `AssuanRepl::handle_line`:
    ```rust
    "SETDESC" => {
        let decoded = assuan_decode(args, false).unwrap_or_else(|_| args.to_string());
        self.description = Some(decoded);
        self.send_ok()?;
    }
    ```
    If `assuan_decode` encounters an invalid escape sequence (e.g. `%FF` in non-UTF-8, or a trailing `%` character), it returns an error. Instead of aborting, the code falls back to using the raw `args` string. This allows malformed escapes to bypass verification and sanitization, risking parameter injection, string formatting flaws, or TTY escape code injections.
*   **Mitigation:** Any parsing or decoding failure must immediately abort command execution and return a protocol-compliant error frame (`ERR <error_code> <message>\n`) to the client.
    ```rust
    "SETDESC" => {
        match assuan_decode(args, false) {
            Ok(decoded) => {
                self.description = Some(decoded);
                self.send_ok()?;
            }
            Err(e) => {
                self.send_error(83886179, &format!("Invalid percent encoding: {}", e))?;
            }
        }
    }
    ```

### 3.3 Float Arithmetic in Color Premultiplication
*   **The Flaw:** Floating-point conversion and rounding arithmetic (`f32::round`) is used for parsing hex color channels.
*   **Mechanics & Failure Modes:** Floating point arithmetic is non-deterministic on some target platforms, prone to rounding discrepancies, and slower than integer operations. Since colors are 16-bit premultiplied values, float operations are unnecessary.
*   **Mitigation:** Replace float arithmetic with pure integer math.
    *   $A_{16} = a \times 257$ (since $255 \times 257 = 65535$).
    *   $R_{16} = \frac{r \times a \times 65535}{255 \times 255} = \frac{r \times a \times 257}{255}$. This is accurately simplified in integer math as:
    ```rust
    let alpha_16 = (a as u32) * 257;
    let red_16 = (((r as u32) * (a as u32) * 257) / 255) as u16;
    ```
