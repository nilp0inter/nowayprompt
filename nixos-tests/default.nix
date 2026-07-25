# NixOS differential parity test harness.
#
# Each stage of the Rust rewrite (see RUST_REWRITE.md §4-5) has a
# corresponding `nixosTest` that runs the Rust `nowayprompt` target and the
# legacy `pkgs.wayprompt` baseline (pinned to nixos-26.05, v0.1.2) through the
# same scripted scenario and asserts behavioral parity. A stage is not done
# until its test passes against the pinned oracle ("staggered" strategy).
#
# This attrset is wired into `flake.nix` as the top-level `nixosTests` output
# (NOT perSystem): the VM tests only run on x86_64-linux and must resolve the
# oracle from the pinned nixos-26.05 input while installing the target built
# by this flake.
#
# Arguments (provided by flake.nix):
#   lib       — nixpkgs lib (from the dev/build nixpkgs)
#   nixpkgs   — the pinned nixos-26.05 flake input (oracle source)
#   pkgs      — nixpkgs-26.05 legacyPackages for x86_64-linux (oracle + test
#               driver)
#   selfpkgs  — `self.packages` (the Rust target, per system)
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  # Build a `nixosTest` from a test module file. The module receives the
  # harness arguments and returns `{ name, nodes, testScript, ... }`.
  # (In nixpkgs 26.05 `pkgs.nixosTest` moved to `pkgs.testers.nixosTest`.)
  mkTest = path:
    pkgs.testers.nixosTest (import path { inherit lib nixpkgs pkgs selfpkgs; });
in
{
  # Stage 1 backfill: CLI & config parsing parity (omitted from the archived
  # Stage 0-1 change).
  stage-1-cli-config = mkTest ./stage-1-cli-config.nix;

  # Stage 2: Assuan IPC wire-protocol parity.
  stage-2-assuan = mkTest ./stage-2-assuan.nix;

  # Stage 3: hardened TTY console fallback parity.
  stage-3-tty = mkTest ./stage-3-tty.nix;

  # Stage 3: Wayland frontend geometry under headless cage.
  stage-3-wayland = mkTest ./stage-3-wayland.nix;
}
