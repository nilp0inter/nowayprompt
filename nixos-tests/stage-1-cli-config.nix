# Stage 1 (backfill): CLI & config parsing parity.
#
# Boots a minimal VM (no display server) with both the pinned legacy oracle
# (`pkgs.wayprompt` v0.1.2 from nixos-26.05) and the Rust target
# (`nowayprompt`) installed, and asserts identical CLI and `wayprompt.5`
# config-parsing behavior.
#
# Known implementation state (documented per the byte-tolerance contract —
# allowed divergences MUST be documented explicitly in the test):
#
#   * The legacy pinentry binary parses no CLI flags at all (`--version`,
#     `--help`, anything: argv is ignored). The spec's "both exit 0 for
#     --version/--help" scenario is a Stage 4 CLI contract; this harness
#     asserts the differential form — oracle and target MUST behave
#     identically (same exit code, same output class) — which is the core
#     parity contract and fails loudly if either side grows or loses flags.
#   * Both binaries initialize their frontend BEFORE any Assuan I/O, so in a
#     headless VM (no WAYLAND_DISPLAY, no pre-configured tty) both exit 1
#     before doing interactive work. Exit-code parity across that failure is
#     asserted; interactive flows live in stage-2/stage-3 tests.
#   * Legacy pinentry logs config errors via syslog(3) (journald in the VM),
#     never stderr; the target reports them on stderr as
#     `<path>:<line>: <message>`. Error-message parity is therefore compared
#     after extracting the `config.ini:<line>: <message>` core from either
#     channel, lower-cased, with a trailing period stripped (legacy appends
#     one, the target does not).
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  oracle = pkgs.wayprompt;
  target = selfpkgs.x86_64-linux.nowayprompt;

  # Valid wayprompt(5) fixture: `#` comments (full-line and inline),
  # semicolon-terminated assignments, `[general]` integers, `[colours]` hex.
  validConfig = pkgs.writeTextDir "wayprompt/config.ini" ''
    # Stage-1 parity fixture: a valid wayprompt(5) configuration.
    [general]
    pin-square-amount = 8; # inline comment after the terminating semicolon
    vertical-padding = 12;
    corner-radius = 0;

    [colours]
    background = 0xFFFFFF;
    error-text = 0xE0002B;
    ok-button = 0xD5F200;
  '';

  # Malformed fixture: line 2 is an unknown section. Both parsers MUST reject
  # it at line 2 with an "unknown section" diagnostic.
  malformedConfig = pkgs.writeTextDir "wayprompt/config.ini" ''
    # Stage-1 parity fixture: malformed configuration.
    [bogus-section]
    not-a-variable = 1;
  '';
in
{
  name = "stage-1-cli-config";

  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    environment.systemPackages = [ oracle target ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")

    oracle_pinentry = "${oracle}/bin/pinentry-wayprompt"
    oracle_cli = "${oracle}/bin/wayprompt"
    target_bin = "${target}/bin/nowayprompt"
    valid_cfg = "${validConfig}"
    malformed_cfg = "${malformedConfig}"

    for b in (oracle_pinentry, oracle_cli, target_bin):
        machine.succeed(f"test -x {b}")


    def run_capture(cmd):
        """Run cmd in the VM; return (rc, stdout, stderr) separately."""
        rc, _ = machine.execute(f"{cmd} >/tmp/cap.out 2>/tmp/cap.err")
        out = machine.succeed("cat /tmp/cap.out")
        err = machine.succeed("cat /tmp/cap.err")
        return (rc, out, err)


    def journal(identifier):
        machine.sleep(1)  # let journald's async flush settle
        rc, out = machine.execute(f"journalctl -b -t {identifier} --no-pager -o cat")
        return out if rc == 0 else ""


    # ------------------------------------------------------------------
    # 14.1: --version / --help parity.
    #
    # Contract (spec "Stage 1 backfill", scenarios "--version exit code
    # parity"): identical behavior between baseline and target. The literal
    # "exit 0 + non-empty stdout" expectation requires CLI flag support,
    # which the legacy pinentry never had (it ignores argv) — that is a
    # Stage 4 CLI deliverable. Assert the differential contract instead:
    # exit codes identical, stdout non-emptiness identical.
    # ------------------------------------------------------------------
    for flag in ("--version", "--help"):
        orc, oout, oerr = run_capture(f"{oracle_pinentry} {flag} </dev/null")
        trc, tout, terr = run_capture(f"{target_bin} {flag} </dev/null")
        assert orc == trc, (
            f"{flag}: exit-code divergence: oracle={orc} target={trc} "
            f"(oracle stderr={oerr!r}, target stderr={terr!r})"
        )
        assert (oout.strip() != "") == (tout.strip() != ""), (
            f"{flag}: stdout class divergence: oracle={oout!r} target={tout!r}"
        )
        print(f"{flag}: oracle rc={orc} target rc={trc} (parity OK)")

    # Positive control for the fixture environment: the oracle CLI supports
    # --help (exit 0, usage on stdout). Not compared against the target —
    # different binary class (CLI vs pinentry).
    rc, out, _ = run_capture(f"{oracle_cli} --help </dev/null")
    assert rc == 0 and "Usage:" in out, f"oracle CLI --help broken: rc={rc}"

    # ------------------------------------------------------------------
    # 14.2: valid config — both parse without a config diagnostic.
    #
    # Headless, both binaries then fail frontend init (exit 1); the exit
    # codes must still match, and neither channel may carry a config.ini
    # diagnostic for the valid fixture.
    # ------------------------------------------------------------------
    machine.succeed("journalctl --rotate && journalctl --vacuum-time=1s")

    orc, oout, oerr = run_capture(
        f"XDG_CONFIG_HOME={valid_cfg} {oracle_pinentry} </dev/null"
    )
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME={valid_cfg} {target_bin} </dev/null"
    )
    assert orc == trc, f"valid config: exit-code divergence oracle={orc} target={trc}"
    oj = journal("pinentry-wayprompt")
    assert "config.ini" not in oerr and "config.ini" not in oj, (
        f"oracle reported a config error for the VALID fixture: "
        f"stderr={oerr!r} journal={oj!r}"
    )
    assert "config.ini" not in terr, (
        f"target reported a config error for the VALID fixture: stderr={terr!r}"
    )
    print(f"valid config: oracle rc={orc} target rc={trc}, no config diagnostics")

    # ------------------------------------------------------------------
    # 14.3: malformed config — both reject with a diagnostic; parity of the
    # normalized error line.
    # ------------------------------------------------------------------
    machine.succeed("journalctl --rotate && journalctl --vacuum-time=1s")

    orc, oout, oerr = run_capture(
        f"XDG_CONFIG_HOME={malformed_cfg} {oracle_pinentry} </dev/null"
    )
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME={malformed_cfg} {target_bin} </dev/null"
    )
    assert orc != 0 and trc != 0, (
        f"malformed config: both binaries must exit non-zero "
        f"(oracle={orc} target={trc})"
    )

    import re

    def extract_config_error(text):
        """Pull the `config.ini:<line>: <message>` core out of a diagnostic.

        Tolerance (documented above): lower-cased, trailing period stripped
        (legacy appends '.', the target does not).
        """
        m = re.search(r"config\.ini:(\d+):[^\n]*", text)
        if not m:
            return None
        core = m.group(0).rstrip(".").lower()
        return core

    oj = journal("pinentry-wayprompt")
    oerr_msg = extract_config_error(oj) or extract_config_error(oerr)
    terr_msg = extract_config_error(terr)
    assert oerr_msg is not None, (
        f"oracle emitted no config.ini diagnostic for the malformed fixture "
        f"(stderr={oerr!r}, journal={oj!r})"
    )
    assert terr_msg is not None, (
        f"target emitted no config.ini diagnostic for the malformed fixture "
        f"(stderr={terr!r})"
    )
    assert oerr_msg == terr_msg, (
        f"config error line divergence:\n  oracle: {oerr_msg}\n  target: {terr_msg}"
    )
    print(f"malformed config: both rejected with: {terr_msg}")
  '';
}
