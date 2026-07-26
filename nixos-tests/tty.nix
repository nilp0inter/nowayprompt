# TTY fallback contract.
#
# The pinned oracle initializes before it can receive OPTION ttyname and is
# therefore unavailable in a headless VM. The Rust implementation intentionally
# defers selection; this gate exercises its reachable TTY fallback directly.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  target = selfpkgs.x86_64-linux.nowayprompt;
  driver = ./tty-driver.py;
in
{
  name = "tty";

  nodes.machine = { pkgs, ... }: {
    virtualisation.memorySize = 1024;
    services.kmscon.enable = false;
    environment.systemPackages = [
      target
      pkgs.python3
      pkgs.coreutils
      pkgs.procps
    ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("getty@tty1.service")
    machine.succeed("test -c /dev/tty1")
    machine.copy_from_host("${driver}", "/tmp/tty-driver.py")
    target_bin = "${target}/bin/pinentry-nowayprompt"
    machine.succeed(
        f"python3 /tmp/tty-driver.py {target_bin} /tmp/report-target.json",
        timeout=600,
    )

    import json

    target = json.loads(machine.succeed("cat /tmp/report-target.json"))

    def live(name):
        report = target[name]
        assert not report.get("startup_refusal"), f"{name}: {report}"
        return report

    flags = live("termios_flags")
    assert flags["alive_during_prompt"]
    assert flags["raw_flags_cleared"], flags["during"]
    assert flags["restored_after_exit"], flags

    sigint = live("signal_sigint")
    assert sigint["exited_on_signal"]
    assert sigint["restored_after_exit"], sigint
    sigtstp = live("signal_sigtstp")
    assert sigtstp["restored_after_exit"], sigtstp

    ansi = live("ansi_capture")
    assert ansi["has_clear"] and ansi["has_home"] and ansi["has_pin_row"], ansi
    assert ansi["getpin_response"] == ["D abc", "END", "OK"], ansi

    leak = live("zero_leak")
    assert leak["core_limit_zero"], leak
    assert leak["core_files_in_tmp"] == [], leak
    assert leak["exit_code"] == 0, leak
    print("TTY fallback: raw mode, signals, ANSI rendering, and secret handling verified")
  '';
}
