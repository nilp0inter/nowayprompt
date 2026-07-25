# Stage 3: hardened virtual-TTY console fallback parity.
#
# Boots a NixOS VM with agetty on tty1 (no display server) with both the
# pinned legacy oracle (`pkgs.wayprompt` v0.1.2, `pinentry-wayprompt`) and the
# Rust target (`nowayprompt`) installed. A Python driver (tty-driver.py, run
# inside the VM) exercises each binary on the tty1 console and an 80x24 pty
# and records a JSON report; the assertions below implement tasks 16.2-16.5.
#
# Asserted behavior (spec "Stage 3 test"):
#   * 16.2 Raw termios flag clearing on tty1: while the pinentry prompts,
#          ECHO/ICANON/ISIG MUST be cleared.
#   * 16.3 Signal restoration: SIGINT MUST restore tty1's pre-prompt cooked
#          termios before exit; SIGTSTP MUST restore as well (target hardening
#          per design decision D8 — asserted on the target; the legacy oracle
#          leaves the terminal raw on SIGINT and merely stops on SIGTSTP, so
#          its behavior is recorded, not asserted against).
#   * 16.4 ANSI byte capture at 80x24: ESC[2J, ESC[H and the ` > ` pin-row
#          prefix MUST be present; the rendered byte stream MUST be
#          byte-identical between baseline and target.
#   * 16.5 Zero leak: RLIMIT_CORE MUST be 0; while the prompt holds "hunter2"
#          the target's resident secret copies MUST be <= the baseline's
#          (strict superset: the target may not leak where the baseline might);
#          no core file may be produced.
#
# Known state (documented): both binaries initialize their frontend BEFORE the
# Assuan loop, so in a headless VM neither reaches a live prompt — subtests
# record `startup_refusal` and the gate FAILS with that diagnostic rather than
# passing vacuously.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  oracle = pkgs.wayprompt;
  target = selfpkgs.x86_64-linux.nowayprompt;

  driver = ./tty-driver.py;
in
{
  name = "stage-3-tty";

  nodes.machine = { pkgs, ... }: {
    virtualisation.memorySize = 1024;
    # agetty on tty1, no kmscon, no display manager: a plain virtual console.
    services.kmscon.enable = false;
    environment.systemPackages = [
      oracle
      target
      pkgs.python3
      pkgs.coreutils   # stty
      pkgs.procps      # ps / procps tooling
    ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("getty@tty1.service")
    # tty1 must exist as a usable console device.
    machine.succeed("test -c /dev/tty1")

    machine.copy_from_host("${driver}", "/tmp/tty_driver.py")

    oracle_bin = "${oracle}/bin/pinentry-wayprompt"
    target_bin = "${target}/bin/nowayprompt"

    machine.succeed(
        f"python3 /tmp/tty_driver.py {oracle_bin} /tmp/report-oracle.json",
        timeout=600,
    )
    machine.succeed(
        f"python3 /tmp/tty_driver.py {target_bin} /tmp/report-target.json",
        timeout=600,
    )

    import base64
    import json

    orc = json.loads(machine.succeed("cat /tmp/report-oracle.json"))
    tgt = json.loads(machine.succeed("cat /tmp/report-target.json"))


    def live(rep, name, sub):
        """Return subtest `sub` of report `name`, failing the gate if the
        binary refused to start (no live prompt to test)."""
        st = rep[sub]
        assert not st.get("startup_refusal"), (
            f"{name}/{sub}: binary refused to start (frontend.init precedes "
            f"the Assuan loop; headless VM offers no display/tty at startup). "
            f"stderr={st.get('stderr')!r}"
        )
        return st


    # ------------------------------------------------------------------
    # 16.2: raw termios flags cleared on tty1 while the prompt is active.
    # ------------------------------------------------------------------
    for name, rep in (("oracle", orc), ("target", tgt)):
        tf = live(rep, name, "termios_flags")
        assert tf["alive_during_prompt"], (
            f"{name}: pinentry not alive during the prompt"
        )
        assert tf["raw_flags_cleared"], (
            f"{name}: ECHO/ICANON/ISIG not all cleared during prompt; "
            f"stty -a = {tf['during']!r}"
        )
        assert tf["restored_after_exit"], (
            f"{name}: tty1 termios not restored after the pinentry exited"
        )
    print("16.2: raw flags cleared and restored on both binaries")

    # ------------------------------------------------------------------
    # 16.3: signal restoration (target hardening per D8).
    # ------------------------------------------------------------------
    si = live(tgt, "target", "signal_sigint")
    assert si["exited_on_signal"], "target: did not exit on SIGINT"
    assert si["restored_after_exit"], (
        f"target: tty1 termios not restored after SIGINT; "
        f"baseline={si['baseline']!r} after={si['after']!r}"
    )
    o_si = orc["signal_sigint"]
    print(f"16.3 SIGINT: target restored={si['restored_after_exit']}; "
          f"oracle restored={o_si.get('restored_after_exit')} "
          f"(legacy divergence recorded, not asserted)")

    sz = live(tgt, "target", "signal_sigtstp")
    assert sz["restored_after_exit"], (
        f"target: tty1 termios not restored after SIGTSTP; "
        f"after={sz['after']!r}"
    )
    o_sz = orc["signal_sigtstp"]
    print(f"16.3 SIGTSTP: target restored={sz['restored_after_exit']}; "
          f"oracle restored={o_sz.get('restored_after_exit')}")

    # ------------------------------------------------------------------
    # 16.4: ANSI byte capture at 80x24 — byte-identical between binaries.
    # ------------------------------------------------------------------
    oa = live(orc, "oracle", "ansi_capture")
    ta = live(tgt, "target", "ansi_capture")
    for name, a in (("oracle", oa), ("target", ta)):
        assert a["has_clear"], f"{name}: ESC[2J (clear) missing from render"
        assert a["has_home"], f"{name}: ESC[H (home) missing from render"
        assert a["has_pin_row"], (
            f"{name}: ' > ***' pin-row prefix missing from render"
        )
        assert a["getpin_response"] == ["D abc", "END", "OK"], (
            f"{name}: GETPIN response {a['getpin_response']!r}"
        )
    o_bytes = base64.b64decode(oa["rendered"])
    t_bytes = base64.b64decode(ta["rendered"])
    assert o_bytes == t_bytes, (
        f"16.4: ANSI render byte divergence at 80x24 "
        f"(oracle {len(o_bytes)}B vs target {len(t_bytes)}B)\n"
        f"  oracle: {o_bytes[:200]!r}\n  target: {t_bytes[:200]!r}"
    )
    print(f"16.4: ANSI capture byte-identical ({len(t_bytes)} bytes)")

    # ------------------------------------------------------------------
    # 16.5: zero password-buffer leak.
    # ------------------------------------------------------------------
    tl = live(tgt, "target", "zero_leak")
    assert tl["core_limit_zero"], (
        f"target: RLIMIT_CORE not zero: {tl.get('core_limit')!r}"
    )
    assert tl["core_files_in_tmp"] == [], (
        f"target: core file(s) produced: {tl['core_files_in_tmp']!r}"
    )
    assert tl["exit_code"] == 0, (
        f"target: zero-leak run exit code {tl['exit_code']} != 0"
    )
    if not orc["zero_leak"].get("startup_refusal"):
        ol = orc["zero_leak"]
        assert tl["copies"] <= ol["copies"], (
            f"16.5: target leaks more secret copies than baseline: "
            f"target={tl['copies']} {tl['hits']!r} vs "
            f"oracle={ol['copies']} {ol['hits']!r}"
        )
        print(f"16.5: target copies={tl['copies']} <= "
              f"oracle copies={ol['copies']}")
    else:
        print(f"16.5: target copies={tl['copies']} "
              f"(oracle unavailable; absolute RLIMIT_CORE=0 asserted)")
    print("stage-3 tty parity: PASS")
  '';
}
