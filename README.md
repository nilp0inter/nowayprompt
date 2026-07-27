<!-- markdownlint-disable MD013 MD033 MD041 -->
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.png">
    <img src="assets/logo-light.png" alt="nowayprompt logo" width="640">
  </picture>
  <br>
  <a href="https://github.com/nilp0inter/nowayprompt/actions/workflows/ci.yml"><img src="https://github.com/nilp0inter/nowayprompt/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://app.renovatebot.com/dashboard"><img src="https://img.shields.io/badge/maintained%20with-renovate-blue?logo=renovatebot" alt="renovate"></a>
  <a href="https://nixos.org/"><img src="https://img.shields.io/badge/Built_with-Nix-5277C3?logo=nixos&logoColor=white" alt="Built with Nix"></a>
  <img src="https://img.shields.io/badge/Zig_content-0%25-brightgreen?logo=zig" alt="Zig content: 0%">
</p>
<!-- markdownlint-enable MD013 MD033 MD041 -->

`nowayprompt` is a small, stubborn Wayland prompt tool written in Rust. It asks
for passwords without making a scene, then falls back to a TUI when Wayland has
gone missing—such as in a TTY console.

It needs a compositor with the layer-shell protocol
(`zwlr_layer_shell_v1`). No layer shell, no tiny prompt.

> [!NOTE]
> This project is a Rust port and fork of the excellent
> [Wayprompt](https://sr.ht/~leon_plickat/wayprompt/), by Leon Henrik Plickat.
>
> The _Wayprompt_ project has been seemingly dormant for almost two years[^1] at
> this point, and while still relevant, being based on Zig 0.13[^2] puts it at
> risk of being dropped from Nixpkgs[^3] in the next release (or two), as the
> older Zig versions are retired.
>
> This is a humble contribution to the community, to carry the project forward.
> By porting it to Rust, we hope to provide a foundation that will be easier to
> maintain over the long term, while remaining a drop-in[^4] replacement for
> _Wayprompt_.
>
> We thank _Herr Plickat_ for unleashing _Wayprompt_ onto the world.  At first
> glance you might perceive it as a seemingly small and unassuming tool, yet if
> you embrace it, as we did, it soon becomes an *indispensable* tool in your
> daily workflow.

[^1]: At the time of writing, July 27, 2026. [Last
    commit](https://git.sr.ht/~leon_plickat/wayprompt/commit/66fe87408d3cfba8c8cc6ff65c1868e5db6ad3bb)
    on August 25, 2024.
[^2]: Zig 0.16 [was released](https://ziglang.org/news/0.16.0-released/) on
    April 14, 2026.
[^3]: Zig 0.12 [was dropped](https://github.com/NixOS/nixpkgs/pull/434644) from
    Nixpkgs (unstable branch) in August, 2025.
[^4]: Yes, Home Manager and Stylix handle it, no sweat.

---

## Executables

The package installs one binary plus two basename aliases; the invocation
basename selects the contract each entry point provides.

* **`$out/bin/nowayprompt`**: the prompt tool proper.
* **`$out/bin/pinentry-nowayprompt`**: a GPG Pinentry replacement, installed
  as a symlink to `nowayprompt`.
* **`$out/bin/nowayprompt-ssh-askpass`**: an `ssh-askpass` provider for SSH
  and Git, installed as a symlink to `nowayprompt`.

They all speak the `wayprompt.5` configuration dialect. Configuration is
looked up inside a single base directory — `$XDG_CONFIG_HOME`, else
`$HOME/.config`, else `/etc` — where `nowayprompt/config.ini` wins over
`wayprompt/config.ini` (the fallback is silent, files are never merged, and
an existing file that fails to parse is an error rather than a skip; no
candidate in the selected base means built-in defaults). See
`nowayprompt.conf(5)` for the full cascade and format, and
`reference/security_tty_ipc.md` for dialect details.

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

## Reference

* Specifications and API documentation live in `reference/`:
  * `reference/wayland.md`
  * `reference/graphics.md`
  * `reference/xkb_input.md`
  * `reference/security_tty_ipc.md`
  * `reference/critic_security.md`
  * `reference/critic_wayland_graphics.md`

---

## License

`nowayprompt` is licensed under the GNU General Public License v3.0 (GPLv3).
Share it freely; it is already good at asking for things.
