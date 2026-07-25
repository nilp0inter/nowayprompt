# Stage 3: Wayland frontend geometry parity under cage.
#
# Uses the nixpkgs `services.cage` module (the proven pattern from
# nixpkgs' own cage/ydotool tests): cage runs as a systemd service on a
# virtio-gpu device, launching a wrapper that runs the Rust target's
# `nowayprompt-wayland-test` binary in a loop, logging stderr to a file
# the test script reads. The test exercises:
#
#   * surface configure: the binary logs `configured: <W>x<H> scale=<S>`
#   * hotspot layout: the log contains `hotspots: [(Ok, ...), (Cancel, ...)]`
#     with non-zero geometry
#   * keyboard Return  -> `event: UserOk`
#   * keyboard Escape  -> `event: UserAbort` (after the wrapper restarts
#     the binary for the second phase)
#
# Stage 4 extends this harness by adding grim frame capture and swapping
# the test binary for the real nowayprompt pinentry.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  target = selfpkgs.x86_64-linux.nowayprompt;

  # Wrapper: run the test binary in a loop, logging stdout+stderr to a
  # file the test script greps. After each terminal event the binary
  # exits; the wrapper restarts it so both the Return->UserOk and
  # Escape->UserAbort phases run within one cage session.
  runner = pkgs.writeShellScript "wayland-test-runner" ''
    LOG=/tmp/wayland-test.log
    : > "$LOG"
    while true; do
      ${target}/bin/nowayprompt-wayland-test \
        --title Test --prompt "PIN:" --ok OK --cancel Cancel >> "$LOG" 2>&1
      echo "--- restart ---" >> "$LOG"
      sleep 1
    done
  '';

  user = "alice";
  uid = 1000;
in
{
  name = "stage-3-wayland";

  nodes.machine =
    { pkgs, ... }:
    {
      users.users.${user} = {
        isNormalUser = true;
        inherit uid;
      };

      services.cage = {
        enable = true;
        user = "${user}";
        program = "${runner}";
      };

      environment.systemPackages = [ pkgs.wtype ];

      # cage (wlroots DRM backend) needs a GPU device; use virtio-gpu
      # instead of the default -vga std (same as nixpkgs' cage test).
      virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
      virtualisation.memorySize = 1024;
    };

  testScript =
    ''
      import re

      def wtype(key):
          # Run wtype as the cage user with the cage Wayland socket.
          machine.succeed(
              "su - ${user} -c '"
              f"WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/${toString uid} "
              f"wtype -k {key}'"
          )

      start_all()
      machine.wait_for_unit("cage.service")

      # Phase 1: configure + hotspots + Return -> UserOk.
      machine.wait_until_succeeds(
          "grep -E 'configured: [0-9]+x[0-9]+ scale=[0-9]+' /tmp/wayland-test.log",
          timeout=30,
      )
      cfg = machine.succeed(
          "grep -E 'configured: [0-9]+x[0-9]+ scale=[0-9]+' /tmp/wayland-test.log | head -1"
      ).strip()
      m = re.search(r"configured: (\d+)x(\d+) scale=(\d+)", cfg)
      assert m and int(m.group(1)) > 0 and int(m.group(2)) > 0, f"bad geometry: {cfg}"
      print(f"configured: {cfg}")

      hl = machine.succeed("grep 'hotspots:' /tmp/wayland-test.log | head -1").strip()
      assert "Ok" in hl and "Cancel" in hl, f"hotspots incomplete: {hl}"
      assert not re.search(r"\((Ok|Cancel), 0, 0, 0, 0\)", hl), (
          f"hotspots have zero geometry: {hl}"
      )
      print(f"hotspots: {hl}")

      wtype("Return")
      machine.wait_until_succeeds("grep 'event: UserOk' /tmp/wayland-test.log", timeout=10)
      print("Return -> UserOk: OK")

      # Phase 2: wrapper restarts the binary; wait for a second configure,
      # then Escape -> UserAbort.
      machine.wait_until_succeeds(
          "test $(grep -c -E 'configured: [0-9]+x[0-9]+ scale=[0-9]+' /tmp/wayland-test.log) -ge 2",
          timeout=30,
      )
      wtype("Escape")
      machine.wait_until_succeeds("grep 'event: UserAbort' /tmp/wayland-test.log", timeout=10)
      print("Escape -> UserAbort: OK")

      print("stage-3 wayland geometry: PASS")
    '';
}