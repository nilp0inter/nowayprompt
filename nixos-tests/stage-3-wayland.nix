# Stage 3: Wayland frontend geometry parity under sway.
#
# The frontend requires `zwlr_layer_shell_v1`, which cage (a single
# fullscreen-surface kiosk) does NOT implement. Use sway — the reference
# wlroots compositor, which supports layer-shell.
#
# sway needs a logind session/seat to start, so it is launched via getty
# autologin on tty1 (the proven nixpkgs sway-test pattern), with the
# pixman software renderer (GLES2/EGL does not work in the VM). A sway
# config execs a wrapper that loops the Rust target's
# `nowayprompt-wayland-test` binary (logging to a file the test script
# reads). The test exercises:
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
  # Escape->UserAbort phases run within one sway session.
  runner = pkgs.writeShellScript "wayland-test-runner" ''
    LOG=/tmp/wayland-test.log
    : > "$LOG"
    export RUST_BACKTRACE=full
    # Targeted key-processing trace (keycode/keysym) to diagnose input.
    export NOWAYPROMPT_DEBUG=1
    # Record the sway socket path so the test driver's wtype can connect
    # (sway's XDG_RUNTIME_DIR is not necessarily /run/user/<uid>).
    if [ "''${WAYLAND_DISPLAY#/}" != "$WAYLAND_DISPLAY" ]; then
      echo "$WAYLAND_DISPLAY" > /tmp/wayland-socket
    else
      echo "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" > /tmp/wayland-socket
    fi
    while true; do
      ${target}/bin/nowayprompt-wayland-test \
        --title Test --prompt "PIN:" --ok OK --cancel Cancel >> "$LOG" 2>&1
      echo "--- restart ---" >> "$LOG"
      sleep 1
    done
  '';

  # Minimal sway config: no bar, no XWayland; exec the wrapper.
  swayConfig = pkgs.writeText "sway-test-config" ''
    xwayland disable
    bar { mode invisible }
    exec ${runner}
  '';

  user = "alice";
  uid = 1000;
  display = "wayland-test";
in
{
  name = "stage-3-wayland";

  nodes.machine =
    { config, ... }:
    {
      users.users.${user} = {
        isNormalUser = true;
        inherit uid;
      };

      # Autologin on tty1 gives sway a real logind session/seat (required
      # to start). On login, configure + start sway with the pixman
      # renderer (GLES2/EGL is unavailable in the VM).
      services.getty.autologinUser = user;
      programs.sway.enable = true;
      environment.systemPackages = [ pkgs.wtype ];
      programs.bash.loginShellInit = ''
        if [ "$(tty)" = "/dev/tty1" ]; then
          mkdir -p ~/.config/sway
          cp ${swayConfig} ~/.config/sway/config
          export WAYLAND_DISPLAY=${display}
          export WLR_BACKENDS=drm
          export WLR_RENDERER=pixman
          sway > /tmp/sway.log 2>&1
        fi
      '';

      # sway (wlroots DRM backend) needs a GPU device; use virtio-gpu
      # instead of the default -vga std (same as nixpkgs' sway test).
      virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
      virtualisation.memorySize = 1024;
    };

  testScript =
    ''
      import re
      import time

      def wtype(key):
          # The runner records sway's actual socket path (absolute) in
          # /tmp/wayland-socket; libwayland uses an absolute WAYLAND_DISPLAY
          # directly, so no XDG_RUNTIME_DIR is needed here.
          sock = machine.succeed("cat /tmp/wayland-socket").strip()
          machine.succeed(
              "su - ${user} -c '"
              f"WAYLAND_DISPLAY={sock} "
              f"wtype -k {key}'"
          )

      def surface_log():
          return machine.succeed("cat /tmp/wayland-test.log || true")

      start_all()
      machine.wait_for_unit("multi-user.target")

      # Phase 1: configure + hotspots + Return -> UserOk. On failure,
      # surface the binary's log + sway's log for crash diagnostics.
      try:
          machine.wait_until_succeeds(
              "grep -E 'configured: [0-9]+x[0-9]+ scale=[0-9]+' /tmp/wayland-test.log",
              timeout=60,
          )
      except Exception:
          print("--- wayland-test.log ---")
          print(surface_log())
          print("--- sway.log ---")
          print(machine.execute("cat /tmp/sway.log || true")[1])
          raise

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

      # Wait for the runner to record the sway socket path.
      machine.wait_for_file("/tmp/wayland-socket")

      # Give the surface time to gain keyboard focus + keymap before
      # injecting keys (a fixed delay is more robust than gating on the
      # debug-only "keymap ready" log line).
      time.sleep(2)
      wtype("Return")
      try:
          machine.wait_until_succeeds("grep 'event: UserOk' /tmp/wayland-test.log", timeout=10)
      except Exception:
          # Dump the keyboard-related protocol trace (WAYLAND_DEBUG) to
          # diagnose whether keymap/enter/key events reached the surface.
          print(machine.execute(
              "grep -E 'keymap|wl_keyboard|\\.key|UserOk' /tmp/wayland-test.log | tail -40 || true"
          )[1])
          raise
      print("Return -> UserOk: OK")

      # Phase 2: wrapper restarts the binary; wait for a second configure,
      # then Escape -> UserAbort.
      machine.wait_until_succeeds(
          "test $(grep -c -E 'configured: [0-9]+x[0-9]+ scale=[0-9]+' /tmp/wayland-test.log) -ge 2",
          timeout=30,
      )
      # Give the restarted binary time to re-acquire keyboard focus.
      time.sleep(2)
      wtype("Escape")
      machine.wait_until_succeeds("grep 'event: UserAbort' /tmp/wayland-test.log", timeout=10)
      print("Escape -> UserAbort: OK")

      print("stage-3 wayland geometry: PASS")
    '';
}