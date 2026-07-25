# Stage 3: Wayland frontend geometry parity under headless cage.
#
# Boots a minimal NixOS VM (no display server) with cage (wlroots kiosk
# compositor), wtype (Wayland key injector), and the Rust target's
# `nowayprompt-wayland-test` binary. A Python driver (wayland-driver.py,
# run inside the VM) launches the test binary under headless cage and
# exercises:
#
#   * surface configure: the test binary logs `configured: 400x300 scale=1`
#   * hotspot layout: the log contains `hotspots: [(Ok, ...), (Cancel, ...)]`
#   * keyboard Return  -> `event: UserOk`
#   * keyboard Escape  -> `event: UserAbort`
#
# Environment: WLR_BACKEND=headless, WLR_RENDERER=pixman (software),
# WLR_LIBINPUT_NO_DEVICES=1. No GPU or real input devices required.
#
# Stage 4 extends this harness by adding grim frame capture and swapping
# the test binary for the real nowayprompt pinentry.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  target = selfpkgs.x86_64-linux.nowayprompt;

  driver = ./wayland-driver.py;
in
{
  name = "stage-3-wayland";

  nodes.machine = { pkgs, ... }: {
    virtualisation.memorySize = 1024;
    environment.systemPackages = [
      target
      pkgs.cage
      pkgs.wtype
      pkgs.python3
    ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")

    machine.copy_from_host("${driver}", "/tmp/wayland_driver.py")

    target_bin = "${target}/bin/nowayprompt-wayland-test"

    machine.succeed(
        f"python3 /tmp/wayland_driver.py {target_bin} /tmp/report-wayland.json",
        timeout=300,
    )

    import json

    rep = json.loads(machine.succeed("cat /tmp/report-wayland.json"))

    assert rep["configured"], (
        f"surface did not configure: errors={rep['errors']!r}"
    )
    print(f"configured: {rep['configured_line']}")

    assert rep["hotspots_ok"], (
        f"hotspots missing or incomplete: line={rep['hotspots_line']!r} "
        f"errors={rep['errors']!r}"
    )
    print(f"hotspots: {rep['hotspots_line']}")

    assert rep["user_ok"], (
        f"Return did not emit UserOk: errors={rep['errors']!r}"
    )
    print("Return -> UserOk: OK")

    assert rep["user_abort"], (
        f"Escape did not emit UserAbort: errors={rep['errors']!r}"
    )
    print("Escape -> UserAbort: OK")

    print("stage-3 wayland geometry: PASS")
  '';
}
