## Context

The Rust rewrite of `nowayprompt` replaces the Zig `wayprompt` implementation. Stage 0 establishes the Nix flake development environment; Stage 1 delivers the two foundational modules (`src/secret.rs`, `src/config.rs`) that every downstream stage depends on.

Current state (legacy Zig):
- `SecretBuffer.zig`: 1-page (1024 B) aligned allocation, `mlock`, `MADV_DONTDUMP`, `FixedBufferAllocator`, UTF-8 codepoint-aware append/delete. No `MADV_WIPEONFORK`, no `RLIMIT_CORE`, no explicit zeroization (relies on `alloc.free`).
- `Config.zig`: `zig-ini` library tokenize, `[general]`/`[colours]` sections, hyphen-to-underscore field matching (`foo-bar` == `foo_bar`), hex `0xRRGGBB`/`0xRRGGBBAA` to premultiplied pixman color.

Constraints (from `RUST_REWRITE.md` §3):
- No general heap secret allocations (`String`, `Vec<u8>`, `std::alloc::alloc`).
- No external C graphics deps (fcft/pixman/cairo/wayland-client.so).
- No async runtimes.
- 100% behavioral parity with legacy Zig modules.

Stakeholders: single-user NixOS deployment; `pinentry-nowayprompt` (GPG agent), `nowayprompt-ssh-askpass`, CLI prompt.

## Goals / Non-Goals

**Goals:**
- `flake.nix` `devShells.default` with reproducible Rust toolchain + native deps.
- `Cargo.toml` workspace with locked deps (`libc`, `zeroize`, `memmap2`).
- `src/secret.rs`: direct `mmap(2)` page allocation, `mlock`, `MADV_DONTDUMP`, `MADV_WIPEONFORK`, `RLIMIT_CORE = 0`, `Zeroize` on `Drop`. Append-slice, delete-backwards, reset, slice accessor.
- `src/config.rs`: custom `BufRead` INI parser, section dispatch (`[general]`, `[colours]`), trailing-semicolon strip, hyphen-to-underscore field match, hex-to-premultiplied color.
- Unit tests achieving behavioral parity with legacy Zig tests (`SecretBuffer` test, `fieldEql` test, color conversion).

**Non-Goals:**
- Wayland frontend (Stage 3).
- TTY fallback (Stage 2).
- Assuan IPC (Stage 2).
- CLI entrypoint multiplexer (Stage 4).
- Nix package output (`packages.default`) — deferred to Stage 4.
- Third-party INI crates (`ini`, `rust-ini`).
- Multi-page secret buffer (single fixed page; capacity matches legacy 1024 B).

## Decisions

### D1: Direct `mmap(2)` over `std::alloc` for secret memory

**Choice**: `libc::mmap(MAP_PRIVATE | MAP_ANONYMOUS)` page allocation, `libc::mlock`, `libc::madvise(MADV_DONTDUMP | MADV_WIPEONFORK)`, `RLIMIT_CORE = 0` at process startup, `zeroize::Zeroize` on `Drop`, `libc::munmap` on drop.

**Rationale**: Negative Constraint #3 forbids standard heap allocators for secret data. `mmap` gives page-aligned, kernel-managed memory excluded from core dumps and swap. Legacy used `alloc.alignedAlloc` (heap) + `mlock` — a partial measure. The Rust version hardens further with `WIPEONFORK` and explicit zeroization.

**Alternatives**:
- `std::alloc::alloc_zeroed` + `mlock`: violates Constraint #3; heap metadata remnants risk.
- `secrecy` crate: wraps `String`/`Vec` (heap); violates Constraint #3.
- `zeroize::Zeroizing<Vec<u8>>`: heap-backed; violates Constraint #3.

### D2: Fixed 4096-byte single-page buffer (legacy parity: 1024 B → 4096 B page)

**Choice**: Allocate exactly one system page (`libc::_SC_PAGESIZE`, typically 4096). Capacity 4096 B supersedes legacy 1024 B. No growth/realloc.

**Rationale**: `mmap` operates in page granularity. A single page is the minimum lockable unit and matches the "fixed buffer" design of legacy `FixedBufferAllocator`. Overflow returns an error (parity with legacy `OutOfMemory`). Increasing to full page size is a strict improvement with zero behavioral regression.

**Alternatives**:
- Multi-page growth: adds complexity, fragmentation, and violates the fixed-buffer invariant.
- 1024 B via `mmap` + manual sub-alloc: wastes a full page anyway.

### D3: Custom `BufRead` INI parser over third-party crate

**Choice**: Single-pass line reader over `std::io::BufRead`. Handles `[section]` headers, `key = value` assignments, inline `#` comments, trailing `;` stripping. Field dispatch via hyphen-to-underscore normalization (`foo-bar` → `foo_bar`).

**Rationale**: Legacy uses `zig-ini` with `.semicolon` tokenization mode — a Zig-specific library with no Rust equivalent matching the trailing-semicolon behavior. `wayprompt.5` manpage specifies the semicolon syntax. A custom parser is ~150 LOC, zero-dep, and guarantees parity.

**Alternatives**:
- `rust-ini` crate: heap-allocated `HashMap`, no trailing-semicolon strip, wrong semantics.
- `ini` crate: same issues.

### D4: Color representation — premultiplied `u16` RGBA (legacy parity)

**Choice**: Store colors as `struct Colour { red: u16, green: u16, blue: u16, alpha: u16 }` premultiplied alpha, matching legacy `pixman.Color` layout.

**Rationale**: Legacy `pixmanColourFromRGB` produces premultiplied 16-bit channels. `tiny-skia` (Stage 3) uses `PremultipliedColorU8` internally; converting from 16-bit premultiplied is a direct cast. Keeping the legacy representation ensures the config parser feeds Stage 3 without intermediate conversion.

**Alternatives**:
- `tiny_skia::Color` (non-premultiplied f32): requires conversion at use-site; loses parity with legacy field order.
- `csscolorparser` crate: heap allocation, no premultiplied output.

### D5: Nix flake toolchain — `nixpkgs` Rust toolchain (no overlay)

**Choice**: Use `nixpkgs.rustToolchain` via `pkgs.rustPlatform` / direct `rustc`+`cargo` from nixpkgs. No `fenix`/`rust-overlay`.

**Rationale**: Single-user NixOS, nixpkgs-unstable provides sufficiently recent stable Rust. Avoids overlay maintenance and flake input proliferation. `libxkbcommon` is required for Stage 3 but included now for environment completeness.

**Alternatives**:
- `fenix` overlay: exact toolchain pinning, but overkill for single-user reproducible env.
- `rust-overlay`: same tradeoff.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| `mlock` fails under resource limits (`RLIMIT_MEMLOCK`) | Secret buffer un-lockable; process must abort | Retry loop (10 attempts, parity with legacy); hard error on exhaustion |
| `MADV_WIPEONFORK` unavailable on older kernels (<4.14) | `madvise` returns EINVAL | Best-effort: log warning, continue (not a hard failure) |
| `RLIMIT_CORE = 0` affects whole process | No core dumps for debugging | Acceptable for secret-handling process; documented |
| Custom INI parser misses edge cases in `wayprompt.5` | Config parse errors | Unit tests covering legacy test vectors + manpage examples |
| 4096 B cap vs legacy 1024 B | Behavioral change (larger cap) | Strict superset; no regression for inputs ≤1024 B |
| No `MADV_WIPEONFORK` on non-Linux | Build portability | Guarded by `#[cfg(target_os = "linux")]` (legacy did same for DONTDUMP) |