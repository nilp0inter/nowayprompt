<!-- markdownlint-disable MD013 MD033 MD041 -->
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.png">
    <img src="assets/logo-light.png" alt="nowayprompt logo" width="640">
  </picture>
  <br>
  <a href="https://github.com/nilp0inter/nowayprompt/actions/workflows/ci.yml"><img src="https://github.com/nilp0inter/nowayprompt/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://app.renovatebot.com/dashboard"><img src="https://img.shields.io/badge/maintaied%20with-renovate-blue?logo=renovatebot" alt="renovate"></a>
  <a href="https://nixos.org/"><img src="https://img.shields.io/badge/Built_with-Nix-5277C3?logo=nixos&logoColor=white" alt="Built with Nix"></a>
  <img src="https://img.shields.io/badge/Zig_content-0%25-brightgreen?logo=zig" alt="Zig content: 0%">
</p>
<!-- markdownlint-enable MD013 MD033 MD041 -->

`nowayprompt` is a small, stubborn Wayland prompt tool written in Rust. It asks
for passwords without making a scene, then falls back to a TUI when Wayland has
gone missing—such as in a TTY console.

It needs a compositor with the layer-shell protocol
(`zwlr_layer_shell_v1`). No layer shell, no tiny prompt stage.

---

## Executables

* **`nowayprompt`**: the prompt tool proper.
* **`pinentry-nowayprompt`** (symlink / drop-in `pinentry-wayprompt`): a GPG
  Pinentry replacement.
* **`nowayprompt-ssh-askpass`** (symlink / drop-in
  `wayprompt-ssh-askpass`): an `ssh-askpass` provider for SSH and Git.

They all speak the same configuration dialect. See
`reference/security_tty_ipc.md` for the `wayprompt.5` details.

---

## Architecture & Security

* **Pure-Rust Wayland Backend**: `wayland-client` uses its pure-Rust `rs`
  socket implementation—no dynamic C library entourage.
* **Software Text & Graphics Engine**: `cosmic-text`, `tiny-skia`, `fontdb`,
  and `swash` provide font fallback, OpenType shaping, and SIMD-accelerated
  software rendering into `wl_shm` buffers.
* **Protected Secret Memory**: `mmap(2)` pages are locked with `mlock`,
  excluded from dumps with `MADV_DONTDUMP`, wiped on fork with
  `MADV_WIPEONFORK`, and zeroed on drop. Secrets get the paranoid treatment.
* **Zero Async Overhead**: a synchronous, poll-based REPL and Wayland event
  loop. No runtime is hiding under the rug.

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

* Specifications and API documentation live in `reference/`:
  * `reference/wayland.md`
  * `reference/graphics.md`
  * `reference/xkb_input.md`
  * `reference/security_tty_ipc.md`
  * `reference/critic_security.md`
  * `reference/critic_wayland_graphics.md`
* The legacy Zig codebase lives in `reference/legacy/`, for archaeology and
  historical curiosity.

---

## License

`nowayprompt` is licensed under the GNU General Public License v3.0 (GPLv3).
Share it freely; it is already good at asking for things.
