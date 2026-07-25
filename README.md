# nowayprompt

`nowayprompt` is a multi-purpose (password-)prompt tool for Wayland, written in Rust.
It includes a TUI fallback mode for when no Wayland connection can be established (e.g., in a TTY console).

Requires a Wayland compositor supporting the layer-shell protocol (`zwlr_layer_shell_v1`).

---

## Executables

* **`nowayprompt`**: CLI prompt tool.
* **`pinentry-nowayprompt`** (symlink / drop-in `pinentry-wayprompt`): Pinentry replacement for GPG.
* **`nowayprompt-ssh-askpass`** (symlink / drop-in `wayprompt-ssh-askpass`): `ssh-askpass` provider for SSH and Git.

All executables share the same configuration file syntax (read `reference/security_tty_ipc.md` for `wayprompt.5` format details).

---

## Architecture & Security

* **Pure-Rust Wayland Backend**: Uses `wayland-client` pure-Rust `rs` socket implementation. Zero dynamic C library dependencies.
* **Software Text & Graphics Engine**: `cosmic-text` + `tiny-skia` + `fontdb` + `swash` for font fallback, OpenType shaping, and SIMD-accelerated software rendering into `wl_shm` buffers.
* **Protected Secret Memory**: `mmap(2)` kernel-level page allocations locked via `mlock`, protected with `MADV_DONTDUMP` and `MADV_WIPEONFORK`, and zeroed on drop using `zeroize`.
* **Zero Async Overhead**: Pure synchronous poll-based REPL and Wayland event dispatch loops.

---

## Building

### Cargo
```sh
cargo build --release
```

### Nix
```sh
nix build
```

---

## Reference & Legacy Code

* Specifications & API Documentation: `reference/`
  * `reference/wayland.md`
  * `reference/graphics.md`
  * `reference/xkb_input.md`
  * `reference/security_tty_ipc.md`
  * `reference/critic_security.md`
  * `reference/critic_wayland_graphics.md`
* Legacy Zig Codebase: `reference/legacy/`

---

## License

`nowayprompt` is licensed under the GNU General Public License v3.0 (GPLv3).
