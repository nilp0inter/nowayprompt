# Stage 1 (backfill): CLI & config parsing parity.
#
# Boots a minimal VM (no display server) with both the pinned legacy oracle
# (`pkgs.wayprompt` v0.1.2 from nixos-26.05) and the Rust target
# (`nowayprompt`) installed, and asserts identical CLI and `wayprompt.5`
# config-parsing behavior.
#
# The CLI comparison deliberately uses the legacy `wayprompt` executable and
# the Rust `nowayprompt` executable. Pinentry lifecycle parity is covered by
# stage 2 and TTY behavior by stage 3. Configuration diagnostics may appear
# through the legacy syslog channel or target stderr; comparison normalizes the
# `config.ini:<line>: <message>` core.
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

    oracle_cli = "${oracle}/bin/wayprompt"
    target_cli = "${target}/bin/nowayprompt"
    valid_cfg = "${validConfig}"
    malformed_cfg = "${malformedConfig}"

    for b in (oracle_cli, target_cli):
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
    # CLI help and rejected unknown option behavior.
    # ------------------------------------------------------------------
    rc, out, _ = run_capture(f"{oracle_cli} --help </dev/null")
    assert rc == 0 and "Usage:" in out, f"oracle CLI --help broken: rc={rc}"
    rc, out, _ = run_capture(f"{target_cli} --help </dev/null")
    assert rc == 0 and "Usage:" in out, f"target CLI --help broken: rc={rc}"

    for binary in (oracle_cli, target_cli):
        rc, _, _ = run_capture(f"{binary} --unknown </dev/null")
        assert rc != 0, f"{binary}: unknown flag must fail"
    # ------------------------------------------------------------------
    # Valid configuration parses before the headless frontend failure.
    # ------------------------------------------------------------------
    machine.succeed("journalctl --rotate && journalctl --vacuum-time=1s")

    orc, oout, oerr = run_capture(
        f"XDG_CONFIG_HOME={valid_cfg} {oracle_cli} --title config-test </dev/null"
    )
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME={valid_cfg} {target_cli} --title config-test </dev/null"
    )
    assert orc != 0 and trc != 0, (
        f"valid config headless failures must be nonzero: oracle={orc} target={trc}"
    )
    oj = journal("wayprompt")
    assert "config.ini" not in oerr and "config.ini" not in oj, (
        f"oracle reported a config error for the VALID fixture: "
        f"stderr={oerr!r} journal={oj!r}"
    )
    assert "config.ini" not in terr, (
        f"target reported a config error for the VALID fixture: stderr={terr!r}"
    )
    print("valid config: both parsed without configuration diagnostics")
    # ------------------------------------------------------------------
    # 14.3: malformed config — both reject with a diagnostic; parity of the
    # normalized error line.
    # ------------------------------------------------------------------
    machine.succeed("journalctl --rotate && journalctl --vacuum-time=1s")

    orc, oout, oerr = run_capture(
        f"XDG_CONFIG_HOME={malformed_cfg} {oracle_cli} --title config-test </dev/null"
    )
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME={malformed_cfg} {target_cli} --title config-test </dev/null"
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

    oj = journal("wayprompt")
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
