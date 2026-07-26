# Reachable layer-shell Wayland differential parity.
#
# Sway is selected because it implements zwlr_layer_shell_v1. Cage is excluded:
# it does not provide that protocol. ydotoold owns a persistent uinput keyboard;
# one-shot wtype is deliberately not used because its device lifetime races
# delivery to layer-shell clients.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  oracle = pkgs.wayprompt; # pinned oracle/reference binary
  target = selfpkgs.x86_64-linux.nowayprompt;
  geometry = selfpkgs.x86_64-linux.nowayprompt-wayland-test;
  driver = ./wayland-driver.py;
  swayConfig = pkgs.writeText "nowayprompt-sway.conf" ''
    output * mode 1280x720
    default_border none
  '';
in
{
  name = "wayland";

  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    boot.kernelModules = [ "uinput" ];
    environment.systemPackages = [
      oracle
      target
      geometry
      pkgs.sway
      pkgs.ydotool
      pkgs.grim
      pkgs.imagemagick
      pkgs.python3
    ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("modprobe uinput")
    machine.succeed("install -d -m 700 /run/nowayprompt-wayland")
    machine.succeed(
        "XDG_RUNTIME_DIR=/run/nowayprompt-wayland "
        "WLR_BACKENDS=headless,libinput WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman "
        "WLR_LIBINPUT_NO_DEVICES=1 sway --unsupported-gpu --config ${swayConfig} "
        ">/tmp/sway.log 2>&1 &"
    )
    machine.wait_until_succeeds(
        "test -S /run/nowayprompt-wayland/wayland-1",
        timeout=30,
    )

    machine.succeed(
        "ydotoold --socket-path=/run/nowayprompt-wayland/ydotool.sock "
        ">/tmp/ydotoold.log 2>&1 &"
    )
    machine.wait_until_succeeds(
        "test -S /run/nowayprompt-wayland/ydotool.sock",
        timeout=30,
    )

    machine.copy_from_host("${driver}", "/tmp/wayland-driver.py")
    machine.succeed(
        "XDG_RUNTIME_DIR=/run/nowayprompt-wayland "
        "WAYLAND_DISPLAY=wayland-1 "
        "YDOTOOL_SOCKET=/run/nowayprompt-wayland/ydotool.sock "
        "python3 /tmp/wayland-driver.py "
        "${target}/bin/nowayprompt "
        "${oracle}/bin/wayprompt "
        "${geometry}/bin/nowayprompt-wayland-test",
        timeout=180,
    )
  '';
}
