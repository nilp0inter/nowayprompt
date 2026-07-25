# Rust API Reference Manual: Security, TTY, & Assuan IPC

This document details the specifications, API designs, safety contracts, and implementation blueprints for the pure-Rust rewrite of the security memory buffer, the raw TTY terminal fallback, the INI configuration parser, and the Pinentry Assuan IPC REPL protocol handler in `nowayprompt`.

---

## 1. Protected Secret Memory (`SecretBuffer`)

### Design & Threat Model
The `SecretBuffer` is designed to store highly sensitive credentials (passwords, PINs) in RAM while mitigating the following threat vectors:
1. **Swapping to Disk:** Plaintext secrets could be written to unencrypted swap partitions, persisting long after the process terminates. This is mitigated by locking the memory pages into RAM using `libc::mlock`.
2. **Core Dumps:** If the process crashes or is core-dumped, plaintext secrets could be written to disk. This is mitigated by marking the memory pages with `libc::madvise(..., libc::MADV_DONTDUMP)`.
3. **Memory Reuse & Allocator Leaks:** Freed memory containing secrets could be reallocated to other parts of the application or other processes before being cleared. This is mitigated by zeroing the buffer via the `zeroize::Zeroize` trait upon `Drop` before the page is deallocated.
4. **Adjacent Memory Leaks:** Page-aligned allocations ensure that memory protection operations (`mlock`, `madvise`) apply strictly to the secret buffer, preventing accidental exposure of adjacent heap allocations or locking unrelated memory.

### Layout Requirements
- Memory must be allocated using `std::alloc::Layout::from_size_align` where the alignment is strictly set to the platform's page size.
- Page size is retrieved dynamically via `libc::sysconf(libc::_SC_PAGESIZE)`.

### API Specification

```rust
use std::alloc::{self, Layout};
use std::ptr::NonNull;
use zeroize::Zeroize;

/// A page-aligned, memory-locked, and zeroized-on-drop secret buffer.
pub struct SecretBuffer {
    ptr: NonNull<u8>,
    layout: Layout,
    capacity: usize,
    len: usize,
    char_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SecretBufferError {
    #[error("Failed to determine system page size")]
    PageSizeUnavailable,
    #[error("Invalid allocation layout: {0}")]
    InvalidLayout(alloc::LayoutError),
    #[error("Memory allocation failed")]
    AllocationFailed,
    #[error("mlock failed after 10 attempts due to EAGAIN")]
    MlockFailedEagain,
    #[error("mlock failed with system error: {0}")]
    MlockFailed(std::io::Error),
    #[error("madvise (MADV_DONTDUMP) failed after 10 attempts due to EAGAIN")]
    MadviseFailedEagain,
    #[error("madvise failed with system error: {0}")]
    MadviseFailed(std::io::Error),
    #[error("Buffer capacity exceeded")]
    OutOfMemory,
    #[error("Invalid UTF-8 sequence")]
    InvalidUtf8,
}
```

### Implementation Blueprint

```rust
impl SecretBuffer {
    /// Creates a new `SecretBuffer` allocated on page-aligned, locked memory.
    pub fn new(capacity: usize) -> Result<Self, SecretBufferError> {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            return Err(SecretBufferError::PageSizeUnavailable);
        }
        let page_size = page_size as usize;

        // Ensure capacity is rounded up to the nearest page boundary
        let aligned_capacity = (capacity + page_size - 1) & !(page_size - 1);

        let layout = Layout::from_size_align(aligned_capacity, page_size)
            .map_err(SecretBufferError::InvalidLayout)?;

        // Allocate memory page
        let raw_ptr = unsafe { alloc::alloc(layout) };
        let ptr = NonNull::new(raw_ptr).ok_or(SecretBufferError::AllocationFailed)?;

        let mut secret_buf = Self {
            ptr,
            layout,
            capacity: aligned_capacity,
            len: 0,
            char_count: 0,
        };

        secret_buf.lock_memory()?;
        secret_buf.protect_memory()?;

        Ok(secret_buf)
    }

    /// Locks the allocated page in RAM using mlock.
    fn lock_memory(&mut self) -> Result<(), SecretBufferError> {
        let mut attempts = 0;
        loop {
            let res = unsafe {
                libc::mlock(self.ptr.as_ptr() as *const libc::c_void, self.capacity)
            };
            if res == 0 {
                return Ok(());
            }

            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) {
                attempts += 1;
                if attempts >= 10 {
                    return Err(SecretBufferError::MlockFailedEagain);
                }
                std::thread::yield_now();
            } else {
                return Err(SecretBufferError::MlockFailed(err));
            }
        }
    }

    /// Excludes the allocated page from core dumps using madvise (MADV_DONTDUMP).
    fn protect_memory(&mut self) -> Result<(), SecretBufferError> {
        #[cfg(target_os = "linux")]
        {
            let mut attempts = 0;
            loop {
                let res = unsafe {
                    libc::madvise(
                        self.ptr.as_ptr() as *mut libc::c_void,
                        self.capacity,
                        libc::MADV_DONTDUMP,
                    )
                };
                if res == 0 {
                    return Ok(());
                }

                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EAGAIN) {
                    attempts += 1;
                    if attempts >= 10 {
                        return Err(SecretBufferError::MadviseFailedEagain);
                    }
                    std::thread::yield_now();
                } else {
                    return Err(SecretBufferError::MadviseFailed(err));
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(())
        }
    }

    /// Appends a string slice to the secret buffer.
    pub fn append(&mut self, s: &str) -> Result<(), SecretBufferError> {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.capacity {
            return Err(SecretBufferError::OutOfMemory);
        }

        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.as_ptr().add(self.len),
                bytes.len(),
            );
        }
        self.len += bytes.len();
        self.char_count += s.chars().count();
        Ok(())
    }

    /// Deletes the last UTF-8 codepoint in the buffer.
    pub fn delete_backwards(&mut self) {
        if self.len == 0 {
            return;
        }

        let slice = unsafe {
            std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len)
        };

        // Find the beginning of the last UTF-8 character
        let mut i = self.len - 1;
        while i > 0 && (slice[i] & 0xC0) == 0x80 {
            i -= 1;
        }

        // Zero out the deleted character's bytes
        let char_len = self.len - i;
        slice[i..self.len].zeroize();
        self.len = i;
        self.char_count -= 1;
    }

    /// Returns a read-only view of the current secret buffer as a string slice.
    pub fn as_str(&self) -> Option<&str> {
        if self.len == 0 {
            return None;
        }
        let slice = unsafe {
            std::slice::from_raw_parts(self.ptr.as_ptr(), self.len)
        };
        std::str::from_utf8(slice).ok()
    }

    /// Resets the buffer, zeroing active contents but maintaining the allocation.
    pub fn clear(&mut self) {
        let slice = unsafe {
            std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len)
        };
        slice.zeroize();
        self.len = 0;
        self.char_count = 0;
    }

    /// Returns the length of the string in UTF-8 bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns the count of UTF-8 codepoints in the buffer.
    pub fn char_count(&self) -> usize {
        self.char_count
    }
}

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        unsafe {
            // 1. Verbatim zeroization of all allocated capacity
            let slice = std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.capacity);
            slice.zeroize();

            // 2. Unlock the memory
            libc::munlock(self.ptr.as_ptr() as *const libc::c_void, self.capacity);

            // 3. Deallocate page
            alloc::dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}
```

### Safety & Soundness Contracts
- **`Layout` Lifetime:** The `Layout` must be stored with the struct to ensure the exact same `Layout` is passed to `dealloc`. Modifying size or alignment between `alloc` and `dealloc` is undefined behavior.
- **Double Free Prevention:** Ownership of `self.ptr` is bound to the lifecycle of the `SecretBuffer` struct. Raw pointers are not cloned, and `dealloc` is only called inside the `Drop` implementation.
- **Zeroization Integrity:** Zeroing the memory is performed using `zeroize::Zeroize` which utilizes compiler barriers (e.g., volatile writes or inline assembly) to ensure the optimizer does not optimize away the memory-clearing operation.

---

## 2. Raw TTY Terminal Fallback

### Design & TTY Control
When Wayland is unavailable (or requested), input is processed on a raw TTY console. This requires placing the terminal in a non-canonical, non-echoing state using `termios`.

#### Modifying `termios` Flags
To switch to raw mode, retrieve the terminal state using `tcgetattr` and clear the following flags in `c_lflag`:
- `ECHO`: Disables local echoing of typed characters. This ensures the password/PIN is not shown on stdout.
- `ICANON`: Disables canonical mode. Input is processed character-by-character rather than waiting for a newline.
- `ISIG`: Disables processing of signal-generating characters (`SIGINT` on `Ctrl+C`, `SIGQUIT` on `Ctrl+\`, `SIGTSTP` on `Ctrl+Z`). The application must trap and handle these inputs directly.

### API Specification

```rust
use std::os::unix::io::RawFd;

/// RAII wrapper to manage Raw TTY states and restore properties upon termination.
pub struct RawTty {
    fd: RawFd,
    orig_termios: libc::termios,
}

#[derive(Debug, thiserror::Error)]
pub enum TtyError {
    #[error("Failed to get terminal attributes: {0}")]
    GetAttrFailed(#[source] std::io::Error),
    #[error("Failed to set terminal attributes: {0}")]
    SetAttrFailed(#[source] std::io::Error),
    #[error("Failed to read from TTY: {0}")]
    ReadFailed(#[source] std::io::Error),
    #[error("TTY Stream reached EOF")]
    UnexpectedEof,
}
```

### Implementation Blueprint

```rust
impl RawTty {
    /// Configures the terminal file descriptor into raw mode.
    pub fn new(fd: RawFd) -> Result<Self, TtyError> {
        let mut orig_termios = std::mem::maybe_uninit::<libc::termios>();

        if unsafe { libc::tcgetattr(fd, orig_termios.as_mut_ptr()) } != 0 {
            return Err(TtyError::GetAttrFailed(std::io::Error::last_os_error()));
        }
        let orig_termios = unsafe { orig_termios.assume_init() };

        let mut raw = orig_termios;
        
        // Clear ECHO (echo input), ICANON (canonical mode), ISIG (signals)
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        
        // Read blocking configuration: block until at least 1 byte is available
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(TtyError::SetAttrFailed(std::io::Error::last_os_error()));
        }

        Ok(Self { fd, orig_termios })
    }

    /// Reads a single byte from the TTY without echoing it.
    pub fn read_byte(&self) -> Result<u8, TtyError> {
        let mut buf = [0u8; 1];
        let res = unsafe {
            libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, 1)
        };

        if res < 0 {
            Err(TtyError::ReadFailed(std::io::Error::last_os_error()))
        } else if res == 0 {
            Err(TtyError::UnexpectedEof)
        } else {
            Ok(buf[0])
        }
    }

    /// Restores the original terminal state.
    pub fn restore(&self) -> Result<(), TtyError> {
        if unsafe { libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.orig_termios) } != 0 {
            Err(TtyError::SetAttrFailed(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

impl Drop for RawTty {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
```

### Screen Control Escape Sequences
Standard ANSI escape sequences are written directly to stdout to format the terminal interface:
- **Clear Screen:** `\x1b[2J` instructs the terminal emulator to clear all characters on the viewport.
- **Move to Home:** `\x1b[H` moves the cursor to row 1, column 1 (top-left).

```rust
use std::io::{self, Write};

pub fn clear_and_home() -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[2J\x1b[H")?;
    stdout.flush()
}
```

### Signal Handling Safety
Because raw mode disables terminal-level processing of `Ctrl+C` (`SIGINT`), the application should set up signal hooks (e.g., using `signal-hook` or `tokio::signal`) to gracefully exit the main event loop and trigger the `Drop` mechanism of `RawTty`, restoring terminal state before the process exits.

---

## 3. INI Configuration Parser

### Configuration Format & Specifications
The INI configuration file (matching `wayprompt.5`) organizes assignments within sections:
```ini
[general]
# Comments start with hash or semicolon
font-regular = sans:size=14;
vertical-padding = 10;

[colours]
background = 0xFFFFFF;
border = 0x000000;
error-text = 0xE0002B;
```

#### Parsing Rules
1. **Comment Handling:** Skip lines where the first non-whitespace character is `#` or `;`.
2. **Section Headers:** Lines starting with `[` and ending with `]` declare a section (e.g. `[general]`, `[colours]`).
3. **Key-Value Split:** Variable assignments are parsed via `split_once('=')`.
4. **Semicolon Trimming:** Values are trimmed of trailing semicolons and trailing whitespace.
5. **Key Normalization:** Keys are normalized to snake_case by converting hyphens (`-`) to underscores (`_`) (e.g., `font-regular` becomes `font_regular`).

### Data Structures & Hex Color Premultiplication
Colors in `wayprompt` are converted into 16-bit premultiplied RGBA representations used by the backend rendering libraries (e.g., Pixman).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub alpha: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Line {0}: Syntax error or missing section")]
    SyntaxError(usize),
    #[error("Line {0}: Invalid color format: '{1}'")]
    InvalidColor(usize, String),
    #[error("Line {0}: Invalid integer: '{1}'")]
    InvalidInteger(usize, String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
```

### Pre-multiplied Hex Color Parsing Algorithm
Colors can be specified as `0xRRGGBB` or `0xRRGGBBAA`.
1. Extract numerical red ($r$), green ($g$), blue ($b$), and alpha ($a$, defaulting to $255$) values as `u8`.
2. Compute the normalized float alpha value: $a_{float} = a / 255.0$.
3. Compute the 16-bit alpha representation: $A_{16} = \text{round}(a_{float} \times 65535.0)$.
4. Apply premultiplication to each RGB channel and scale to 16-bit:
   - $R_{16} = \text{round}\left(\frac{r}{255.0} \times 65535.0 \times a_{float}\right)$
   - $G_{16} = \text{round}\left(\frac{g}{255.0} \times 65535.0 \times a_{float}\right)$
   - $B_{16} = \text{round}\left(\frac{b}{255.0} \times 65535.0 \times a_{float}\right)$

### Implementation Blueprint

```rust
use std::io::BufRead;

/// Parses hex color strings like "0xFFFFFF" or "0xFFE53EAA" into 16-bit premultiplied Color values.
pub fn parse_color(hex_str: &str) -> Result<Color, &'static str> {
    let clean = hex_str.trim().trim_end_matches(';');
    if !clean.starts_with("0x") {
        return Err("Missing 0x prefix");
    }
    let hex = &clean[2..];
    if hex.len() != 6 && hex.len() != 8 {
        return Err("Hex string must be 6 or 8 characters long");
    }

    let val = u32::from_str_radix(hex, 16).map_err(|_| "Invalid hex digits")?;
    
    let (r, g, b, a) = if hex.len() == 6 {
        (((val >> 16) & 0xFF) as u8, ((val >> 8) & 0xFF) as u8, (val & 0xFF) as u8, 255u8)
    } else {
        (((val >> 24) & 0xFF) as u8, ((val >> 16) & 0xFF) as u8, ((val >> 8) & 0xFF) as u8, (val & 0xFF) as u8)
    };

    let alpha_f = a as f32 / 255.0;
    let alpha = (alpha_f * 65535.0).round() as u16;

    let red = ((r as f32 / 255.0) * 65535.0 * alpha_f).round() as u16;
    let green = ((g as f32 / 255.0) * 65535.0 * alpha_f).round() as u16;
    let blue = ((b as f32 / 255.0) * 65535.0 * alpha_f).round() as u16;

    Ok(Color { red, green, blue, alpha })
}

/// Parses an INI configuration from a reader stream.
pub fn parse_config<R: BufRead>(reader: R) -> Result<Config, ConfigError> {
    let mut config = Config::default();
    let mut current_section = String::new();
    let mut line_num = 0;

    for line_res in reader.lines() {
        line_num += 1;
        let line = line_res?;
        let trimmed = line.trim();

        // 1. Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        // 2. Parse section headers
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
            continue;
        }

        // 3. Extract key-value assignments
        if let Some((raw_key, raw_val)) = trimmed.split_once('=') {
            let key = raw_key.trim().replace('-', "_");
            let mut val = raw_val.trim();
            if val.ends_with(';') {
                val = val[..val.len() - 1].trim();
            }

            match current_section.as_str() {
                "general" => {
                    match key.as_str() {
                        "font_regular" => config.font_regular = Some(val.to_string()),
                        "font_large" => config.font_large = Some(val.to_string()),
                        "vertical_padding" => config.vertical_padding = val.parse::<u32>()
                            .map_err(|_| ConfigError::InvalidInteger(line_num, val.to_string()))?,
                        "horizontal_padding" => config.horizontal_padding = val.parse::<u32>()
                            .map_err(|_| ConfigError::InvalidInteger(line_num, val.to_string()))?,
                        _ => {} // Ignore unknown configurations
                    }
                }
                "colours" => {
                    let parsed_color = parse_color(val)
                        .map_err(|_| ConfigError::InvalidColor(line_num, val.to_string()))?;
                    match key.as_str() {
                        "background" => config.background = parsed_color,
                        "border" => config.border = parsed_color,
                        "text" => config.text = parsed_color,
                        "error_text" => config.error_text = parsed_color,
                        _ => {}
                    }
                }
                _ => return Err(ConfigError::SyntaxError(line_num)),
            }
        } else {
            return Err(ConfigError::SyntaxError(line_num));
        }
    }

    Ok(config)
}

#[derive(Default)]
pub struct Config {
    pub font_regular: Option<String>,
    pub font_large: Option<String>,
    pub vertical_padding: u32,
    pub horizontal_padding: u32,
    pub background: Color,
    pub border: Color,
    pub text: Color,
    pub error_text: Color,
}

impl Default for Color {
    fn default() -> Self {
        Color { red: 0, green: 0, blue: 0, alpha: 65535 }
    }
}
```

---

## 4. Pinentry Assuan IPC REPL

### Protocol Specification
The Assuan protocol runs synchronously over stdin and stdout. Communications are framed line-by-line using Unix newline conventions (`\n`).

#### Outgoing Output Formatting
- **Data frame:** `D <data>\n` (sends a piece of the decoded data).
- **Termination of data series:** `END\n` (required per specification after `D` events, before issuing `OK`).
- **Success response:** `OK\n` or `OK <info>\n` (signifies command completed successfully).
- **Error response:** `ERR <error_code> <message>\n` (signifies command failed. For example, `ERR 83886179 Operation cancelled`).

#### Command Handling & Operations
1. `GETPIN`: Prompts the user to input their pin/password using the configured Wayland UI (or TTY fallback). Blocks until complete, then outputs the secret (with `D <secret>\nEND\nOK\n`) or returns cancellation (`ERR 83886179 Operation cancelled\n`).
2. `SETDESC <desc>`: Sets the description text displayed inside the prompt dialogue window.
3. `SETPROMPT <prompt>`: Sets the label placed directly next to the input box.
4. `BYE`: Directs the pinentry program to respond with `OK\n` and exit the REPL loop.
5. `OPTION <key>=<val>`: Configures environment options. Key options parsed include:
   - `ttyname=<device>`: Directs TTY fallback path to use a designated virtual console.
   - `putenv=WAYLAND_DISPLAY=<socket>`: Points the frontend to the correct Wayland socket path.
   - `default-ok=<string>`: Updates default text label for OK confirmation button.
   - `default-cancel=<string>`: Updates default text label for Cancel button.

### Percent-Decoding & Hotkey Stripping
Assuan commands and options utilize percent-decoding for special characters (escaped as `%XX` where `XX` is the hex value). Additionally, button labels contain hotkey markers designated by a preceding underscore `_` (e.g. `_Cancel`), which must be stripped out during parsing to yield the actual label text (e.g. `Cancel`).

```rust
/// Decodes a percent-encoded Assuan parameter, optionally stripping hotkey underscores.
pub fn assuan_decode(input: &str, strip_hotkey: bool) -> Result<String, &'static str> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("Malformed percent escape");
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| "Invalid UTF-8 in percent escape")?;
            let val = u8::from_str_radix(hex, 16)
                .map_err(|_| "Invalid hex sequence")?;
            output.push(val);
            i += 3;
        } else if bytes[i] == b'_' && strip_hotkey {
            i += 1;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    
    String::from_utf8(output).map_err(|_| "Output is not valid UTF-8")
}
```

### Implementation Blueprint

```rust
use std::io::{self, BufRead, Write};

pub struct AssuanRepl<W: Write> {
    writer: W,
    title: Option<String>,
    description: Option<String>,
    prompt: Option<String>,
    tty_name: Option<String>,
    wayland_display: Option<String>,
    ok_label: Option<String>,
    cancel_label: Option<String>,
    is_running: bool,
}

impl<W: Write> AssuanRepl<W> {
    pub fn new(mut writer: W) -> io::Result<Self> {
        writer.write_all(b"OK wayprompt Pinentry Assuan REPL ready\n")?;
        writer.flush()?;
        Ok(Self {
            writer,
            title: None,
            description: None,
            prompt: None,
            tty_name: None,
            wayland_display: None,
            ok_label: None,
            cancel_label: None,
            is_running: true,
        })
    }

    /// Handles a single incoming command line.
    pub fn handle_line(&mut self, line: &str) -> io::Result<()> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        let (command, args) = match trimmed.split_once(' ') {
            Some((cmd, arg)) => (cmd.to_uppercase(), arg.trim()),
            None => (trimmed.to_uppercase(), ""),
        };

        match command.as_str() {
            "BYE" => {
                self.writer.write_all(b"OK\n")?;
                self.writer.flush()?;
                self.is_running = false;
            }
            "SETTITLE" => {
                let decoded = assuan_decode(args, false).unwrap_or_else(|_| args.to_string());
                self.title = Some(decoded);
                self.send_ok()?;
            }
            "SETDESC" => {
                let decoded = assuan_decode(args, false).unwrap_or_else(|_| args.to_string());
                self.description = Some(decoded);
                self.send_ok()?;
            }
            "SETPROMPT" => {
                let decoded = assuan_decode(args, false).unwrap_or_else(|_| args.to_string());
                self.prompt = Some(decoded);
                self.send_ok()?;
            }
            "OPTION" => {
                if let Err(e) = self.parse_option(args) {
                    self.send_error(83886254, e)?;
                } else {
                    self.send_ok()?;
                }
            }
            "GETPIN" => {
                // Mock behavior representation: In the actual implementation, this blocks
                // while rendering the UI (Wayland/TTY fallback) and collecting characters
                // into a SecretBuffer.
                let mock_pin = "secret123"; 
                
                self.writer.write_all(format!("D {}\n", mock_pin).as_bytes())?;
                self.writer.write_all(b"END\n")?;
                self.writer.write_all(b"OK\n")?;
                self.writer.flush()?;
            }
            "RESET" => {
                self.title = None;
                self.description = None;
                self.prompt = None;
                self.send_ok()?;
            }
            "NOP" => {
                self.send_ok()?;
            }
            _ => {
                // Return "Unknown IPC command" error frame
                self.send_error(536871187, "Unknown IPC command")?;
            }
        }
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.is_running
    }

    fn parse_option(&mut self, option_arg: &str) -> Result<(), &'static str> {
        let (key, val) = option_arg.split_once('=').ok_or("Invalid option format")?;
        match key.trim() {
            "ttyname" => {
                self.tty_name = Some(assuan_decode(val, false)?);
            }
            "putenv" => {
                if let Some((env_name, env_val)) = val.split_once('=') {
                    if env_name.trim() == "WAYLAND_DISPLAY" {
                        self.wayland_display = Some(assuan_decode(env_val, false)?);
                    }
                }
            }
            "default-ok" => {
                self.ok_label = Some(assuan_decode(val, true)?);
            }
            "default-cancel" => {
                self.cancel_label = Some(assuan_decode(val, true)?);
            }
            _ => {} // Silently ignore unsupported options per Assuan protocol compliance
        }
        Ok(())
    }

    fn send_ok(&mut self) -> io::Result<()> {
        self.writer.write_all(b"OK\n")?;
        self.writer.flush()
    }

    fn send_error(&mut self, code: u32, message: &str) -> io::Result<()> {
        self.writer.write_all(format!("ERR {} {}\n", code, message).as_bytes())?;
        self.writer.flush()
    }
}
```
