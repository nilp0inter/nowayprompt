# NixOS differential parity test harness.
#
# Each `nixosTest` runs the `nowayprompt` target and the pinned behavioral
# oracle `pkgs.wayprompt` (v0.1.2, from nixos-26.05) through the same
# scripted scenario and asserts behavioral parity. Behaviors the oracle
# cannot reach in a headless VM are gated on the target directly, against
# the documented contract.
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
  # CLI & config parsing parity.
  cli-config = mkTest ./cli-config.nix;

  # Assuan IPC wire-protocol parity.
  assuan = mkTest ./assuan.nix;

  # Hardened TTY console fallback parity.
  tty = mkTest ./tty.nix;

  # Reachable layer-shell Wayland parity under headless Sway.
  wayland = mkTest ./wayland.nix;
}
