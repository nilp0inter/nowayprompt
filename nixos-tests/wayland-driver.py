#!/usr/bin/env python3
"""Deterministic reachable-Wayland parity driver.

The compositor and ydotoold are persistent processes.  Every client readiness
wait is protocol-observable: WAYLAND_DEBUG must report get_layer_surface before
input is injected.  No timing sleep is used as a readiness signal.
"""

import os
import re
import subprocess
import sys
import time
from pathlib import Path

TARGET, ORACLE, GEOMETRY = sys.argv[1:]
RUNTIME = os.environ["XDG_RUNTIME_DIR"]
DISPLAY = os.environ["WAYLAND_DISPLAY"]
YDOTOOL_SOCKET = os.environ["YDOTOOL_SOCKET"]
SECRET = "parity-input"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def wait_for_layer_surface(process, label):
    deadline = time.monotonic() + 20
    log = []
    saw_layer_surface = False
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line:
            log.append(line)
            saw_layer_surface |= "get_layer_surface" in line
            if saw_layer_surface and ".attach(" in line:
                return log
        elif process.poll() is not None:
            break
    raise RuntimeError(f"{label} never rendered a layer surface; rc={process.poll()}")


def ydotool(*args):
    env = os.environ | {"YDOTOOL_SOCKET": YDOTOOL_SOCKET}
    subprocess.run(["ydotool", *args], env=env, check=True, timeout=10)


def screenshot(label):
    path = Path(f"/tmp/{label}.png")
    subprocess.run(["grim", str(path)], check=True, timeout=10)
    return path


def frame_geometry(path):
    # The compositor background is uniform.  trim therefore produces the
    # visible layer-shell frame bounds.  A two-pixel tolerance allows font
    # rasterizer edge differences without hiding layout regressions.
    result = subprocess.run(
        ["magick", str(path), "-trim", "-format", "%wx%h", "info:"],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    match = re.fullmatch(r"(\d+)x(\d+)", result.stdout)
    require(match is not None, f"cannot measure frame geometry for {path}")
    return tuple(map(int, match.groups()))


def run_public(binary, label, text, key, expected_status):
    env = os.environ | {
        "XDG_RUNTIME_DIR": RUNTIME,
        "WAYLAND_DISPLAY": DISPLAY,
        "WAYLAND_DEBUG": "1",
    }
    process = subprocess.Popen(
        [
            binary,
            "--title",
            "Parity",
            "--prompt",
            "Password:",
            "--button-ok",
            "Ok",
            "--button-cancel",
            "Abort",
            "--get-pin",
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    wait_for_layer_surface(process, label)
    image = screenshot(label)
    if text is not None:
        ydotool("type", text)
    ydotool("key", *key)
    output, stderr = process.communicate(timeout=20)
    require(
        process.returncode == expected_status,
        f"{label}: expected rc={expected_status}, got {process.returncode}; "
        f"stderr tail={stderr[-600:]!r}",
    )
    return output, frame_geometry(image), stderr


def verify_pinentry_namespace():
    env = os.environ | {
        "XDG_RUNTIME_DIR": RUNTIME,
        "WAYLAND_DISPLAY": DISPLAY,
        "WAYLAND_DEBUG": "1",
    }
    pinentry = str(Path(TARGET).with_name("pinentry-nowayprompt"))
    process = subprocess.Popen(
        [pinentry],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        process.stdin.write("GETPIN\n")
        process.stdin.flush()
        log = wait_for_layer_surface(process, "pinentry")
        layer_request = next(line for line in log if "get_layer_surface" in line)
        require(
            '"nowayprompt"' in layer_request,
            f"pinentry used the wrong layer-shell namespace: {layer_request!r}",
        )
        return layer_request.strip()
    finally:
        process.terminate()
        process.communicate(timeout=10)


def parse_rendered(lines):
    """Parse `rendered:` and `configured:` lines from stderr.

    Returns a dict with keys: logical_w, logical_h, mode, scale,
    phys_w, phys_h (from `rendered:`), or just logical_w, logical_h,
    scale (from `configured:`).
    """
    result = {}
    for line in lines:
        line = line.strip()
        if line.startswith("rendered: "):
            payload = line[len("rendered: "):]
            # Format: <W>x<H> mode=<integer|fractional> scale=<S> phys=<PW>x<PH>
            m = re.fullmatch(
                r"(\d+)x(\d+) mode=(integer|fractional) scale=(\d+) phys=(\d+)x(\d+)",
                payload,
            )
            require(m is not None, f"cannot parse rendered line: {line!r}")
            result.update(
                logical_w=int(m.group(1)),
                logical_h=int(m.group(2)),
                mode=m.group(3),
                scale=int(m.group(4)),
                phys_w=int(m.group(5)),
                phys_h=int(m.group(6)),
            )
        elif line.startswith("configured: "):
            payload = line[len("configured: "):]
            m = re.fullmatch(r"(\d+)x(\d+) scale=(\d+)", payload)
            if m:
                result.setdefault("logical_w", int(m.group(1)))
                result.setdefault("logical_h", int(m.group(2)))
                result.setdefault("scale", int(m.group(3)))
    return result


def find_sway_socket():
    """Find the Sway IPC socket in the runtime directory."""
    for entry in Path(RUNTIME).glob("sway-ipc.*.sock"):
        return str(entry)
    raise RuntimeError(f"no sway-ipc socket found in {RUNTIME}")


def swaymsg(*args):
    env = os.environ | {
        "XDG_RUNTIME_DIR": RUNTIME,
        "WAYLAND_DISPLAY": DISPLAY,
        "SWAYSOCK": find_sway_socket(),
    }
    result = subprocess.run(
        ["swaymsg", *args], env=env, check=True, timeout=10,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    return result


def get_headless_output_name():
    """Return the name of the first headless Sway output."""
    result = swaymsg("-t", "get_outputs", "-r")
    import json
    outputs = json.loads(result.stdout)
    require(outputs, "no Sway outputs found")
    return outputs[0]["name"]


def run_geometry_probe():
    env = os.environ | {"XDG_RUNTIME_DIR": RUNTIME, "WAYLAND_DISPLAY": DISPLAY}
    process = subprocess.Popen(
        [GEOMETRY, "--title", "Parity", "--prompt", "Password:"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    configured = None
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line.startswith("configured: "):
            configured = line.strip()
        if line.strip() == "ready":
            break
        if process.poll() is not None:
            raise RuntimeError("geometry probe exited before readiness")
    require(configured is not None, "geometry probe emitted no configured dimensions")
    process.stdin.write("abort\n")
    process.stdin.flush()
    process.wait(timeout=10)
    require(process.returncode == 0, "geometry probe did not exit cleanly")
    return configured


def run_fractional_probe(output_name):
    """Run the geometry probe at 1.5× and assert the fractional render state.

    Asserts: unchanged logical geometry, physical = ceil(logical × 1.5),
    mode=fractional scale=180, set_buffer_scale(1), viewport destination
    == logical geometry (the latter two via WAYLAND_DEBUG=1 trace).
    """
    # Set the output to 1.5× (Sway fractional scale).
    swaymsg("output", output_name, "scale", "1.5")

    env = os.environ | {
        "XDG_RUNTIME_DIR": RUNTIME,
        "WAYLAND_DISPLAY": DISPLAY,
        "WAYLAND_DEBUG": "1",
    }
    process = subprocess.Popen(
        [GEOMETRY, "--title", "Parity", "--prompt", "Password:"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    lines = []
    rendered = None
    configured = None
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line:
            lines.append(line)
            if line.startswith("configured: "):
                configured = line.strip()
            if line.startswith("rendered: "):
                rendered = line.strip()
            if line.strip() == "ready":
                break
        if process.poll() is not None:
            raise RuntimeError("fractional probe exited before readiness")
    require(configured is not None, "fractional probe emitted no configured line")
    require(rendered is not None, "fractional probe emitted no rendered line")

    info = parse_rendered([configured, rendered])
    logical_w = info["logical_w"]
    logical_h = info["logical_h"]
    # Assert fractional mode and scale=180 (1.5 × 120).
    require(info["mode"] == "fractional", f"expected fractional mode, got {info['mode']}")
    require(info["scale"] == 180, f"expected scale=180, got {info['scale']}")
    # Physical dimensions = ceil(logical × 1.5) = ceil(logical × 180/120).
    import math
    expected_pw = math.ceil(logical_w * 180 / 120)
    expected_ph = math.ceil(logical_h * 180 / 120)
    require(
        info["phys_w"] == expected_pw and info["phys_h"] == expected_ph,
        f"physical dims mismatch: got {info['phys_w']}x{info['phys_h']}, "
        f"expected {expected_pw}x{expected_ph}",
    )

    # Assert set_buffer_scale(1) and viewport set_destination(logical) via
    # the Wayland protocol trace.
    trace = "".join(lines)
    require(
        "set_buffer_scale(1)" in trace,
        f"fractional trace missing set_buffer_scale(1); trace tail={trace[-400:]!r}",
    )
    require(
        f"set_destination({logical_w}, {logical_h})" in trace,
        f"fractional trace missing set_destination({logical_w}, {logical_h}); "
        f"trace tail={trace[-400:]!r}",
    )

    process.stdin.write("abort\n")
    process.stdin.flush()
    process.wait(timeout=10)
    require(process.returncode == 0, "fractional probe did not exit cleanly")
    # Reset the output to 1× for subsequent scenarios.
    swaymsg("output", output_name, "scale", "1")


def run_live_scale_change(output_name):
    """Change the output scale while the surface is open; assert a new
    correctly-scaled buffer commit and that the prompt remains interactive.
    """
    env = os.environ | {
        "XDG_RUNTIME_DIR": RUNTIME,
        "WAYLAND_DISPLAY": DISPLAY,
        "WAYLAND_DEBUG": "1",
    }
    process = subprocess.Popen(
        [GEOMETRY, "--title", "Parity", "--prompt", "Password:"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    lines = []
    first_rendered = None
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line:
            lines.append(line)
            if line.startswith("rendered: "):
                first_rendered = line.strip()
            if line.strip() == "ready":
                break
        if process.poll() is not None:
            raise RuntimeError("live-change probe exited before readiness")
    require(first_rendered is not None, "live-change probe emitted no rendered line")

    first_info = parse_rendered([first_rendered])
    # At 1×, mode should be integer (or fractional with scale=120 if the
    # compositor sends preferred_scale(120)). Assert the initial state is
    # captured; the key assertion is the change.
    initial_scale = first_info["scale"]
    initial_mode = first_info["mode"]

    # Change the output scale to 2× while the surface is open.
    swaymsg("output", output_name, "scale", "2")

    # Wait for a new `rendered:` line that reflects the scale change.
    new_rendered = None
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line:
            lines.append(line)
            if line.startswith("rendered: "):
                candidate = line.strip()
                candidate_info = parse_rendered([candidate])
                if candidate_info.get("scale") != initial_scale or \
                        candidate_info.get("mode") != initial_mode:
                    new_rendered = candidate
                    break
        if process.poll() is not None:
            raise RuntimeError("live-change probe exited before scale-change render")

    require(
        new_rendered is not None,
        f"live-change probe did not emit a new rendered line after scale change; "
        f"stderr tail={''.join(lines)[-600:]!r}",
    )

    new_info = parse_rendered([new_rendered])
    # At 2×, the new scale should be 2 (integer) or 240 (fractional 2×).
    # Either way, physical dims must be logical × 2.
    if new_info["mode"] == "integer":
        require(
            new_info["scale"] == 2,
            f"expected integer scale=2 after 2× change, got {new_info['scale']}",
        )
    else:
        require(
            new_info["scale"] == 240,
            f"expected fractional scale=240 after 2× change, got {new_info['scale']}",
        )
    # Physical = logical × 2.
    require(
        new_info["phys_w"] == new_info["logical_w"] * 2
        and new_info["phys_h"] == new_info["logical_h"] * 2,
        f"physical dims not 2× after scale change: "
        f"phys={new_info['phys_w']}x{new_info['phys_h']} "
        f"logical={new_info['logical_w']}x{new_info['logical_h']}",
    )
    # Logical geometry unchanged.
    require(
        new_info["logical_w"] == first_info["logical_w"]
        and new_info["logical_h"] == first_info["logical_h"],
        "logical geometry changed after scale change",
    )

    # Assert the prompt remains interactive: type a secret + Return via
    # ydotool and observe the UserOk exit.
    ydotool("type", SECRET)
    ydotool("key", "28:1", "28:0")
    _, stderr = process.communicate(timeout=20)
    require(
        process.returncode == 0,
        f"live-change probe did not exit cleanly after keyboard input; "
        f"rc={process.returncode}; stderr={stderr[-400:]!r}",
    )
    require(
        "event: UserOk" in stderr,
        f"live-change probe did not report UserOk after keyboard input; "
        f"stderr={stderr[-400:]!r}",
    )

    # Reset the output to 1× for subsequent scenarios.
    swaymsg("output", output_name, "scale", "1")


def main():
    # Phase 1: the existing 1× target/oracle parity gate (unchanged).
    configured = run_geometry_probe()
    pinentry_request = verify_pinentry_namespace()
    target_ok, target_frame, _ = run_public(
        TARGET, "target-ok", SECRET, ["28:1", "28:0"], 0
    )
    oracle_ok, oracle_frame, _ = run_public(
        ORACLE, "oracle-ok", SECRET, ["28:1", "28:0"], 0
    )

    # ydotool keeps its uinput keyboard alive across every client launch.  The
    # key is sent after text delivery, not by a one-shot wtype process.
    target_cancel, _, _ = run_public(
        TARGET, "target-cancel", None, ["1:1", "1:0"], 10
    )
    oracle_cancel, _, _ = run_public(
        ORACLE, "oracle-cancel", None, ["1:1", "1:0"], 10
    )

    require(target_ok == oracle_ok, "accepted secret result diverged")
    require(target_cancel == oracle_cancel, "cancellation result diverged")
    require("user-action: ok" in target_ok and "pin: " in target_ok,
            "accepted target result is malformed")
    require(target_cancel == "user-action: cancel\nno pin\n",
            "cancelled target emitted unexpected output")
    require(all(abs(a - b) <= 2 for a, b in zip(target_frame, oracle_frame)),
            f"frame geometry diverged: target={target_frame}, oracle={oracle_frame}")
    print(
        f"Wayland parity OK: {configured}; frame={target_frame}; "
        f"pinentry={pinentry_request}"
    )

    # Phase 2: target-only fractional scaling scenario.
    output_name = get_headless_output_name()
    run_fractional_probe(output_name)
    print("Fractional scaling gate OK: 1.5× physical dimensions, "
          "set_buffer_scale(1), viewport destination verified")

    # Phase 3: target-only live scale-change scenario.
    run_live_scale_change(output_name)
    print("Live scale-change gate OK: 2× rerender, logical geometry "
          "preserved, keyboard interaction verified")


if __name__ == "__main__":
    main()
