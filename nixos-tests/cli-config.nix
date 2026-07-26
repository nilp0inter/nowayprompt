# CLI & config parsing parity.
#
# Boots a minimal VM (no display server) with both the pinned behavioral
# oracle (`pkgs.wayprompt` v0.1.2 from nixos-26.05) and the target
# (`nowayprompt`) installed, and asserts identical CLI and `wayprompt.5`
# config-parsing behavior.
#
# The CLI comparison deliberately uses the oracle `wayprompt` executable and
# the target `nowayprompt` executable. Pinentry lifecycle parity is covered
# by the Assuan parity test and TTY behavior by the TTY parity test.
# Configuration diagnostics may appear through the oracle's syslog channel
# or the target's stderr; comparison normalizes the
# `config.ini:<line>: <message>` core.
#
# A final target-only block (no oracle involved) covers the documented
# config-selection compatibility extension: within one config base the
# first existing of `nowayprompt/config.ini` then `wayprompt/config.ini`
# wins, the fallback is silent, and a winner's parse or I/O error is
# fatal rather than falling through to the loser.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  oracle = pkgs.wayprompt;
  target = selfpkgs.x86_64-linux.nowayprompt;

  # Valid wayprompt(5) fixture: `#` comments (full-line and inline),
  # semicolon-terminated assignments, `[general]` integers, `[colours]` hex.
  validConfig = pkgs.writeTextDir "wayprompt/config.ini" ''
    # Parity fixture: a valid wayprompt(5) configuration.
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
    # Parity fixture: malformed configuration.
    [bogus-section]
    not-a-variable = 1;
  '';

  # Target-only fixture for the config-selection compatibility extension
  # (XDG config base) holding BOTH candidates: a valid
  # `nowayprompt/config.ini` primary beside a malformed
  # `wayprompt/config.ini` loser. The cascade must select the primary and
  # never open the loser.
  migrationValidPrimary = pkgs.runCommand "migration-valid-primary" { } ''
    mkdir -p $out/nowayprompt $out/wayprompt
    cp ${validConfig}/wayprompt/config.ini $out/nowayprompt/config.ini
    cp ${malformedConfig}/wayprompt/config.ini $out/wayprompt/config.ini
  '';

  # Target-only fixture for the config-selection compatibility extension
  # (a HOME directory, so the base resolves to `$HOME/.config`) holding
  # BOTH candidates: a malformed `nowayprompt/config.ini` primary beside
  # a valid `wayprompt/config.ini` loser. The primary's parse error must
  # be fatal; no fallthrough.
  migrationBadPrimaryHome = pkgs.runCommand "migration-bad-primary-home" { } ''
    mkdir -p $out/.config/nowayprompt $out/.config/wayprompt
    cp ${malformedConfig}/wayprompt/config.ini $out/.config/nowayprompt/config.ini
    cp ${validConfig}/wayprompt/config.ini $out/.config/wayprompt/config.ini
  '';
in
{
  name = "cli-config";

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
    # Malformed config — both reject with a diagnostic; parity of the
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
        (the oracle appends '.', the target does not).
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
    # ------------------------------------------------------------------
    # Target-only (oracle NOT run): documented config-selection
    # compatibility extension beyond pkgs.wayprompt. Within a single
    # config base (non-empty XDG_CONFIG_HOME, else non-empty HOME/.config,
    # else /etc) the first existing candidate wins: nowayprompt/config.ini,
    # then the silent
    # wayprompt/config.ini fallback. A winner's parse or I/O error is
    # fatal: no fallthrough past an existing file.
    # ------------------------------------------------------------------
    mig_valid_primary = "${migrationValidPrimary}"
    mig_bad_primary_home = "${migrationBadPrimaryHome}"

    # Case 1: XDG base holding BOTH candidates — valid nowayprompt primary,
    # malformed wayprompt loser. The loser must never be opened: parsing it
    # would surface its line-2 "unknown section" diagnostic, so the absence
    # of any config.ini diagnostic proves the valid primary won (case 2
    # proves the cascade consults the primary path first).
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME={mig_valid_primary} {target_cli} --title config-test </dev/null"
    )
    assert trc != 0, (
        f"valid primary: headless run must still exit nonzero: rc={trc}"
    )
    assert "config.ini" not in terr and "config.ini" not in tout, (
        f"valid nowayprompt primary must win and the malformed wayprompt "
        f"loser must stay unopened; any parse of the loser emits a "
        f"line-2 diagnostic: stderr={terr!r} stdout={tout!r}"
    )
    print("config-selection: valid nowayprompt primary won; malformed wayprompt loser unopened")

    # Case 2: HOME/.config base holding BOTH candidates — malformed
    # nowayprompt primary, valid wayprompt loser. The primary's parse error
    # must surface as the single fatal diagnostic; the valid loser must not
    # be consulted (falling through would drop the diagnostic entirely).
    trc, tout, terr = run_capture(
        f"XDG_CONFIG_HOME= HOME={mig_bad_primary_home} {target_cli} --title config-test </dev/null"
    )
    assert trc != 0, f"malformed primary must fail the run: rc={trc}"
    diag_lines = [line for line in terr.splitlines() if line.strip()]
    assert len(diag_lines) == 1, (
        f"malformed primary: expected exactly one fatal diagnostic line: {terr!r}"
    )
    core = extract_config_error(terr)
    assert core == "config.ini:2: unknown section 'bogus-section'", (
        f"primary parse error must surface verbatim: stderr={terr!r} core={core!r}"
    )
    assert "/.config/nowayprompt/config.ini:2:" in terr, (
        f"diagnostic must name the nowayprompt primary path: stderr={terr!r}"
    )
    assert "/.config/wayprompt/config.ini" not in terr, (
        f"valid wayprompt loser must not be referenced after a primary error "
        f"(no fallthrough on error): stderr={terr!r}"
    )
    print("config-selection: primary error fatal; no fallthrough to valid wayprompt loser")
  '';
}
