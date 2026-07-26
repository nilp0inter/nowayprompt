# Assuan protocol contract under a real TTY fallback.
#
# The pinned oracle initializes its frontend before Assuan setup, so it cannot
# greet in this headless VM: its TTY name is only supplied by OPTION ttyname.
# The Rust target intentionally defers selection until the first prompt. This
# gate therefore checks the target's complete wire contract directly; the
# reachable Wayland gate remains differential.
{ lib, nixpkgs, pkgs, selfpkgs }:

let
  target = selfpkgs.x86_64-linux.nowayprompt;
  driver = ./assuan-driver.py;
in
{
  name = "assuan";

  nodes.machine = { pkgs, ... }: {
    virtualisation.memorySize = 1024;
    environment.systemPackages = [ target pkgs.python3 ];
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.copy_from_host("${driver}", "/tmp/assuan-driver.py")
    target_bin = "${target}/bin/pinentry-nowayprompt"
    machine.succeed(
        f"python3 /tmp/assuan-driver.py {target_bin} /tmp/report-target.json",
        timeout=600,
    )

    import base64, json, re

    target = json.loads(machine.succeed("cat /tmp/report-target.json"))
    startup = target["startup"]
    assert startup["greeting"] and startup["greeting"].startswith("OK "), startup

    expected = {
        "SETTITLE Test_Title": ["OK"],
        "SETPROMPT Prompt%20Text": ["OK"],
        "SETDESC Desc%20With%20Spaces": ["OK"],
        "SETERROR Error_Msg": ["OK"],
        "SETOK _OK": ["OK"],
        "SETNOTOK Not_OK": ["OK"],
        "SETCANCEL Cancel": ["OK"],
        "NOP": ["OK"],
        "GETINFO flavor": ["D wayprompt", "END", "OK"],
        "GETPIN<resp>": ["D hunter2", "END", "OK"],
        "CONFIRM<resp>": ["OK"],
        "RESET": ["OK"],
        "BYE": ["OK"],
    }
    for command in [
        "CANCEL", "SETGENPIN", "SETGENPIN_TT", "SETTIMEOUT 30", "END",
        "QUIT", "AUTH", "CLEARPASSPHRASE", "SETREPEAT 2",
        "SETREPEATERROR again", "SETQUALITYBAR", "SETQUALITYBAR_TT",
    ]:
        expected[command] = ["ERR 536870981 Not implemented"]

    for name, session in target["sessions"].items():
        assert "skipped" not in session, f"{name} skipped: {session}"
        assert "error" not in session, f"{name} error: {session}"

    matrix = {step["cmd"]: step["resp"] for step in target["sessions"]["matrix"]["steps"]}
    for command, response in expected.items():
        assert matrix.get(command) == response, (
            f"{command}: expected {response!r}, got {matrix.get(command)!r}"
        )
    assert any(
        step["cmd"].startswith("OPTION ttyname=") and step["resp"] == ["OK"]
        for step in target["sessions"]["matrix"]["steps"]
    )
    assert re.match(r"^D \d+\.\d+\.\d+$", matrix["GETINFO version"][0])
    assert re.match(r"^D \d+$", matrix["GETINFO pid"][0])

    rendered = base64.b64decode(target["sessions"]["defaults"]["pty"])
    assert b"Save" in rendered and b"Cancel" in rendered
    assert b"_Save" not in rendered and b"C_ancel" not in rendered

    partial = target["sessions"]["partial_line"]["steps"]
    assert partial and partial[0]["resp"] == ["OK", "OK"], partial
    print("Assuan target contract: deferred frontend and protocol matrix verified")
  '';
}
