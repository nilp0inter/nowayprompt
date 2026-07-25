# Stage 2: Assuan IPC wire-protocol parity.
#
# Boots a minimal VM (no display server) with both the pinned legacy oracle
# (`pkgs.wayprompt` v0.1.2, binary `pinentry-wayprompt`) and the Rust target
# (`nowayprompt`) installed. A Python driver (assuan-driver.py, run inside
# the VM) pipes an identical scripted Assuan command stream into each binary
# and records JSON transcript reports; the comparator below asserts the
# byte-tolerance contract from the nixos-parity-testing spec:
#
#   * byte-identical stdout by default;
#   * greeting: `OK ` prefix + non-empty suffix (wording may diverge);
#   * GETINFO version: format `D X.Y.Z` only (digits may diverge);
#   * GETINFO pid: excluded from byte comparison (process-specific);
#   * ERR codes and messages byte-identical for the not-implemented set
#     (`ERR 536870981 Not implemented`) and unknown commands
#     (`ERR 536871187 Unknown IPC command`), plus cancellation messages.
#
# Known state (documented divergence, NOT a tolerance):
#
#   * Headless startup: both binaries initialize their frontend BEFORE the
#     Assuan loop; in a VM without WAYLAND_DISPLAY or a pre-configured tty
#     both exit before the greeting. The test FAILS with the driver's
#     diagnostic until the flow supports headless operation — a vacuous
#     pass (both emit nothing) would make this gate meaningless.
#   * Partial-line stdin (session `partial_line`): the target intentionally
#     fixes the legacy read()-split bug (design decision D9, BufReader).
#     Excluded from the differential byte comparison; the target's behavior
#     is asserted against its own contract and the oracle's transcript is
#     recorded for diagnostics.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  oracle = pkgs.wayprompt;
  target = selfpkgs.x86_64-linux.nowayprompt;

  driver = ./assuan-driver.py;
in
{
  name = "stage-2-assuan";

  nodes.machine = { pkgs, ... }: {
    virtualisation.memorySize = 1024;
    environment.systemPackages = [
      oracle
      target
      pkgs.python3
    ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")

    machine.copy_from_host("${driver}", "/tmp/assuan_driver.py")

    oracle_bin = "${oracle}/bin/pinentry-wayprompt"
    target_bin = "${target}/bin/nowayprompt"

    # Run the driver once per binary (generous timeout: several sessions
    # with per-step waits).
    machine.succeed(
        f"python3 /tmp/assuan_driver.py {oracle_bin} /tmp/report-oracle.json",
        timeout=600,
    )
    machine.succeed(
        f"python3 /tmp/assuan_driver.py {target_bin} /tmp/report-target.json",
        timeout=600,
    )

    import json, re

    orc = json.loads(machine.succeed("cat /tmp/report-oracle.json"))
    tgt = json.loads(machine.succeed("cat /tmp/report-target.json"))

    # ------------------------------------------------------------------
    # Byte-tolerance contract (spec: "Byte-tolerance contract for
    # differential comparison"). Anything not listed here MUST be
    # byte-identical between baseline and target.
    # ------------------------------------------------------------------
    VERSION_RE = re.compile(r"^D \d+\.\d+\.\d+$")
    PID_RE = re.compile(r"^D \d+$")

    NOT_IMPL_RESP = ["ERR 536870981 Not implemented"]
    UNKNOWN_RESP = ["ERR 536871187 Unknown IPC command"]
    HELP_RESP = [
        "# NOP", "# SETTITLE", "# SETPROMPT", "# SETDESC", "# SETERROR",
        "# GETPIN", "# BYE", "# OPTION", "# RESET", "OK",
    ]

    # Canonical response table for the matrix session (absolute contract,
    # asserted on the target independently of the oracle so the gate stays
    # meaningful even while the oracle cannot run headlessly).
    EXPECTED = {
        "SETTITLE Test_Title": ["OK"],
        "SETPROMPT Prompt%20Text": ["OK"],
        "SETDESC Desc%20With%20Spaces": ["OK"],
        "SETERROR Error_Msg": ["OK"],
        "SETOK _OK": ["OK"],
        "SETNOTOK Not_OK": ["OK"],
        "SETCANCEL Cancel": ["OK"],
        "NOP": ["OK"],
        "HELP": HELP_RESP,
        "SETKEYINFO 1234ABCD": ["OK"],
        "BOGUS": UNKNOWN_RESP,
        "GETINFO flavor": ["D wayprompt", "END", "OK"],
        "GETINFO nosuchinfo": ["OK"],
        "GETPIN<resp>": ["D hunter2", "END", "OK"],
        "CONFIRM<resp>": ["OK"],
        "RESET": ["OK"],
        "BYE": ["OK"],
    }
    for c in [
        "CANCEL", "SETGENPIN", "SETGENPIN_TT", "SETTIMEOUT 30", "END",
        "QUIT", "AUTH", "CLEARPASSPHRASE", "SETREPEAT 2",
        "SETREPEATERROR again", "SETQUALITYBAR", "SETQUALITYBAR_TT",
    ]:
        EXPECTED[c] = NOT_IMPL_RESP
    for o in [
        "OPTION default-ok=unused1", "OPTION default-cancel=unused2",
        "OPTION default-yes=unused3", "OPTION default-no=unused4",
        "OPTION putenv=WAYLAND_DISPLAY=wayland-0",
        "OPTION nosuchoption=whatever",
    ]:
        EXPECTED[o] = ["OK"]
    # OPTION ttyname carries a per-run pts path — match by prefix.
    TTYNAME_PREFIX = "OPTION ttyname="


    def norm_resp(cmd, resp, greeting_mode=False):
        """Normalize a response per the tolerance contract."""
        resp = list(resp)
        if greeting_mode:
            assert resp and resp[0].startswith("OK ") and len(resp[0]) > 3, (
                f"greeting must be 'OK <non-empty>'; got {resp!r}"
            )
            return ["OK <greeting>"]
        if cmd == "GETINFO version":
            assert len(resp) == 3 and VERSION_RE.match(resp[0]), (
                f"GETINFO version must be D X.Y.Z / END / OK; got {resp!r}"
            )
            return ["D <version>", "END", "OK"]
        if cmd == "GETINFO pid":
            assert len(resp) == 3 and PID_RE.match(resp[0]), (
                f"GETINFO pid must be D <pid> / END / OK; got {resp!r}"
            )
            return ["D <pid>", "END", "OK"]
        return resp


    def norm_cmd(cmd):
        """Normalize a step command for comparison (per-run pts paths)."""
        if cmd.startswith("OPTION ttyname="):
            return "OPTION ttyname=<pts>"
        return cmd


    def steps_by_cmd(session):
        return {s["cmd"]: s for s in session.get("steps", [])}


    # ------------------------------------------------------------------
    # Startup gate: the greeting MUST be observed. A vacuous pass (both
    # binaries dead in a headless VM) would make this gate meaningless, so
    # refuse it loudly with the driver's diagnostic.
    # ------------------------------------------------------------------
    for name, rep in (("oracle", orc), ("target", tgt)):
        greet = rep["startup"].get("greeting")
        assert greet is not None, (
            f"{name} produced no greeting; startup report: "
            f"{json.dumps(rep['startup'])}"
        )
        assert greet.startswith("OK ") and len(greet) > 3, (
            f"{name} greeting malformed: {greet!r}"
        )
    print(f"greetings: oracle={orc['startup']['greeting']!r} "
          f"target={tgt['startup']['greeting']!r}")

    # ------------------------------------------------------------------
    # Per-session differential + absolute assertions.
    # ------------------------------------------------------------------
    for sname in ("matrix", "empty_pin", "message", "defaults"):
        o_ses = orc["sessions"][sname]
        t_ses = tgt["sessions"][sname]
        for name, ses in (("oracle", o_ses), ("target", t_ses)):
            assert "skipped" not in ses, (
                f"{name}/{sname} skipped: {ses.get('skipped')} "
                f"(exit_code={ses.get('exit_code')}, stderr={ses.get('stderr')!r})"
            )
            assert "error" not in ses, (
                f"{name}/{sname} failed mid-session: {ses.get('error')}"
            )

        # Greeting parity (tolerance: OK prefix + non-empty suffix).
        og = norm_resp("", [o_ses["greeting"]], greeting_mode=True)
        tg = norm_resp("", [t_ses["greeting"]], greeting_mode=True)
        assert og == tg, f"{sname}: greeting divergence {og!r} vs {tg!r}"

        # Step-by-step comparison.
        o_steps = o_ses["steps"]
        t_steps = t_ses["steps"]
        assert len(o_steps) == len(t_steps), (
            f"{sname}: step count divergence: "
            f"oracle={[s['cmd'] for s in o_steps]} "
            f"target={[s['cmd'] for s in t_steps]}"
        )
        for o_st, t_st in zip(o_steps, t_steps):
            o_cmd = norm_cmd(o_st["cmd"])
            t_cmd = norm_cmd(t_st["cmd"])
            assert o_cmd == t_cmd, (
                f"{sname}: step order divergence at {o_st['cmd']!r} "
                f"vs {t_st['cmd']!r}"
            )
            cmd = o_cmd
            o_resp = norm_resp(cmd, o_st["resp"])
            t_resp = norm_resp(cmd, t_st["resp"])
            assert o_resp == t_resp, (
                f"{sname} [{cmd}]: byte divergence\n"
                f"  oracle: {o_resp!r}\n  target: {t_resp!r}"
            )

        # Exit-code parity for the session.
        assert o_ses["exit_code"] == t_ses["exit_code"], (
            f"{sname}: exit-code divergence "
            f"oracle={o_ses['exit_code']} target={t_ses['exit_code']}"
        )
        print(f"session {sname}: {len(t_steps)} steps, byte-identical")

    # ------------------------------------------------------------------
    # Absolute protocol contract on the target's matrix session (keeps the
    # gate meaningful even if the oracle cannot run a given session).
    # ------------------------------------------------------------------
    t_matrix = steps_by_cmd(tgt["sessions"]["matrix"])
    for cmd, want in EXPECTED.items():
        if cmd == TTYNAME_PREFIX:
            continue
        assert cmd in t_matrix, f"target matrix missing step {cmd!r}"
        got = norm_resp(cmd, t_matrix[cmd]["resp"])
        assert got == want, (
            f"target [{cmd}]: expected {want!r}, got {got!r}"
        )
    # OPTION ttyname=<pts> → OK (prefix match: pts path is per-run).
    ttyname_steps = [
        s for s in tgt["sessions"]["matrix"]["steps"]
        if s["cmd"].startswith(TTYNAME_PREFIX)
    ]
    assert ttyname_steps and ttyname_steps[0]["resp"] == ["OK"], (
        f"target OPTION ttyname: {ttyname_steps!r}"
    )
    # GETINFO version/pid: format-checked inside norm_resp during the diff;
    # assert presence here.
    for cmd in ("GETINFO version", "GETINFO pid"):
        assert cmd in t_matrix, f"target matrix missing {cmd}"
    print("target matrix: absolute protocol contract satisfied")

    # ------------------------------------------------------------------
    # defaults session: hotkey-stripped default labels must appear in the
    # rendered TTY output of BOTH binaries (_Save → Save, C_ancel → Cancel).
    # ------------------------------------------------------------------
    import base64
    for name, rep in (("oracle", orc), ("target", tgt)):
        rendered = base64.b64decode(rep["sessions"]["defaults"]["pty"])
        assert b"Save" in rendered, (
            f"{name}: default-ok label 'Save' missing from TTY render"
        )
        assert b"Cancel" in rendered, (
            f"{name}: default-cancel label 'Cancel' missing from TTY render"
        )
        assert b"_Save" not in rendered and b"C_ancel" not in rendered, (
            f"{name}: hotkey marker not stripped from default labels"
        )
    print("defaults: hotkey stripping verified in both renders")

    # ------------------------------------------------------------------
    # partial_line: intentional divergence (D9). Target MUST assemble the
    # fragments into one SETTITLE (OK) then BYE (OK); the oracle's
    # read()-split misbehavior is recorded, not asserted against.
    # ------------------------------------------------------------------
    t_pl = tgt["sessions"]["partial_line"]
    assert "skipped" not in t_pl, f"target partial_line skipped: {t_pl}"
    pl_resp = t_pl["steps"][0]["resp"] if t_pl["steps"] else []
    assert pl_resp == ["OK", "OK"], (
        f"target partial-line handling: expected ['OK', 'OK'] "
        f"(assembled SETTITLE + BYE), got {pl_resp!r}"
    )
    o_pl = orc["sessions"]["partial_line"]
    print(f"partial_line: target={pl_resp} "
          f"oracle={[s.get('resp') for s in o_pl.get('steps', [])]} "
          f"(oracle divergence documented per D9)")
  '';
}
