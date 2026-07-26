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
    saw_layer_surface = False
    while time.monotonic() < deadline:
        line = process.stderr.readline()
        if line:
            saw_layer_surface |= "get_layer_surface" in line
            if saw_layer_surface and ".attach(" in line:
                return
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


def main():
    configured = run_geometry_probe()
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
    print(f"Wayland parity OK: {configured}; frame={target_frame}")


if __name__ == "__main__":
    main()
