## 1. Nix Flake & Workspace Scaffold

- [x] 1.1 Create `flake.nix` with `nixpkgs` input and `devShells.default` providing `rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`, `pkg-config`, and `libxkbcommon` from nixpkgs (no overlay)
- [x] 1.2 Create `Cargo.toml` workspace root with `[package] nowayprompt`, `edition = "2021"`, and dependencies `libc`, `zeroize`, `memmap2`
- [x] 1.3 Create `src/main.rs` minimal stub (`fn main() {}`) so `cargo build` resolves
- [x] 1.4 Run `cargo generate-lockfile` to produce `Cargo.lock` and verify `cargo build` succeeds with only `libc` native dep
- [x] 1.5 Run `nix develop --command cargo build` to verify the flake dev shell builds the workspace

## 2. Secret Memory Module (`src/secret.rs`)

- [x] 2.1 Implement `set_rlimit_core_zero()` calling `libc::setrlimit(RLIMIT_CORE, 0)` with hard error on failure; call at process startup path (export from `secret.rs`)
- [x] 2.2 Implement `SecretBuffer` struct holding `*mut u8` (page-aligned), `len` (codepoint count), `byte_len`, `capacity` (page size via `libc::_SC_PAGESIZE`)
- [x] 2.3 Implement `SecretBuffer::new()`: `mmap(MAP_PRIVATE | MAP_ANONYMOUS)`, `mlock` retry loop (10 attempts, retry on `EAGAIN`, hard error otherwise), `madvise(MADV_DONTDUMP)` retry loop (hard error on exhaustion), `madvise(MADV_WIPEONFORK)` best-effort (log warning on `EINVAL`/`ENOSYS`)
- [x] 2.4 Implement `SecretBuffer::append_slice(&[u8])`: validate UTF-8, count codepoints, check capacity, copy bytes into page, update `len` and `byte_len`; return overflow error if capacity exceeded
- [x] 2.5 Implement `SecretBuffer::delete_backwards()`: decode trailing UTF-8 byte sequence length from last byte, truncate `byte_len` by sequence length, decrement `len` by 1; no-op if empty
- [x] 2.6 Implement `SecretBuffer::reset()`: `zeroize`, `munmap`, re-`mmap`, re-`mlock`, re-`madvise` — equivalent to drop + new
- [x] 2.7 Implement `SecretBuffer::slice() -> Option<&[u8]>`: return `Some(&self.buf[..self.byte_len])` or `None` if empty
- [x] 2.8 Implement `Drop for SecretBuffer`: `zeroize::Zeroize::zeroize(&mut self.buf[..self.byte_len])`, then `libc::munmap`
- [x] 2.9 Write unit tests mirroring legacy `SecretBuffer` test: empty slice is `None`, append `"hello"`, assert `slice() == Some("hello")`, reset, append `"1234"`, delete backwards to `"1"`, delete on empty no-op, append `"a" * 500`, assert overflow on `"a" * 1000` beyond page capacity
- [x] 2.10 Write unit test for multi-byte codepoint append/delete (`"é"`, `"𝕏"` 4-byte)
- [x] 2.11 Run `cargo test secret` and verify all secret-memory tests pass

## 3. Config Parser Module (`src/config.rs`)

- [x] 3.1 Implement `Colour` struct `{ red: u16, green: u16, blue: u16, alpha: u16 }` (premultiplied) and `fn parse_colour(hex: &str) -> Result<Colour, ConfigError>`
- [x] 3.2 Implement hex parser: validate `0x` prefix, 6 or 8 hex digits, default alpha `0xff` for 6-digit, premultiplied alpha math (`channel_16 = round(channel_8/255 * 65535)`, `premul = round(channel_16 * alpha_16 / 0xffff)`)
- [x] 3.3 Implement `WaylandUi` struct with integer fields (`vertical_padding`, `horizontal_padding`, `button_inner_padding`, `pin_square_size`, `pin_square_border`, `button_border`, `border`, `corner_radius: u16`, `pin_square_amount`) and optional string fields (`font_regular`, `font_large`)
- [x] 3.4 Implement `WaylandColours` struct with all 16 legacy color fields and `Colour` defaults matching legacy `comptimePixmanColourFromRGB` defaults
- [x] 3.5 Implement `Labels` struct (`title`, `description`, `prompt`, `err_message`, `not_ok`, `ok`, `cancel` as `Option<String>`) — runtime-populated, not config-parsed
- [x] 3.6 Implement `Config` struct aggregating `labels`, `wayland_colours`, `wayland_ui`, `allow_tty_fallback`, `tty_name`, `wayland_display`
- [x] 3.7 Implement `fn hyphen_to_underscore(key: &str) -> String` field normalization helper and unit test (parity with legacy `fieldEql`)
- [x] 3.8 Implement `Config::parse()`: resolve config path (`XDG_CONFIG_HOME` → `HOME/.config` → `/etc`), skip silently if no file exists, open file, wrap in `BufReader`, iterate lines
- [x] 3.9 Implement line parser: trim whitespace, strip `#` inline comments, strip trailing `;`, detect `[section]` headers, parse `key = value`, dispatch to `assign_general` / `assign_colour` by current section
- [x] 3.10 Implement `assign_general`: match normalized key against `WaylandUi` fields, parse integers (`u16`/`u31` equivalent) or copy strings (`font_regular`, `font_large`); error on unknown variable with file path + line number
- [x] 3.11 Implement `assign_colour`: match normalized key against `WaylandColours` fields, parse hex color; error on unknown variable or bad color with file path + line number
- [x] 3.12 Write unit test for `fieldEql`/`hyphen_to_underscore` parity (`"test-test"` matches `"test_test"`, mismatch on `"test-testA"` vs `"test-testB"`)
- [x] 3.13 Write unit test for color conversion: `0xffffff` → all 65535, `0xff000080` → alpha 128, red premultiplied, `0xe0002b` matches legacy default
- [x] 3.14 Write unit test for INI parsing: full sample config with `[general]` and `[colours]` sections, trailing semicolons, inline comments, verify all fields populated correctly
- [x] 3.15 Write unit test for error cases: unknown section, assignment outside section, unknown variable, invalid integer, invalid color format
- [x] 3.16 Run `cargo test config` and verify all config-parser tests pass

## 4. Verification & Parity

- [x] 4.1 Run `cargo test` (full suite) and verify all Stage 1 tests pass
- [x] 4.2 Run `cargo clippy -- -D warnings` and verify zero warnings for Stage 1 code
- [x] 4.3 Run `cargo fmt --check` and verify formatting compliance
- [x] 4.4 Run `nix develop --command cargo test` to verify tests pass inside the Nix dev shell
- [x] 4.5 Cross-reference legacy `SecretBuffer.zig` test and `Config.zig` `fieldEql` test; confirm behavioral parity on all test vectors
- [x] 4.6 Verify no `libfcft`/`libpixman-1`/`libwayland-client` linkage in Stage 1 build (`ldd target/debug/nowayprompt` shows only `libc` and standard libs)