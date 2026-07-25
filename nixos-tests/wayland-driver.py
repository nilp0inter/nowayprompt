#!/usr/bin/env python3
"""Stage 3 Wayland frontend geometry driver.

Runs inside the NixOS VM under headless cage. Drives the
nowayprompt-wayland-test binary, exercising surface configuration and
keyboard input via wtype. Outputs a JSON report.

Usage: wayland-driver.py <test-binary> <report-path>

Log format expected from the test binary (stderr):
  configured: <W>x<H> scale=<S>
  hotspots: [(Ok, x, y, w, h), (Cancel, x, y, w, h)]
  event: UserOk
  event: UserAbort
"""

import json
import os
import re
import subprocess
import sys
import time

LOG_PATH = "/tmp/wayland-test.log"
WAYLAND_DISPLAY = "wayland-cage"
# A dedicated runtime dir under /tmp avoids any /run/user ownership/mode
# constraints wlroots imposes on XDG_RUNTIME_DIR.
XDG_RUNTIME_DIR = "/tmp/wayland-runtime"

CAGE_ENV = {
    "WLR_BACKEND": "headless",
    "WLR_RENDERER": "pixman",
    "WLR_LIBINPUT_NO_DEVICES": "1",
    "WAYLAND_DISPLAY": WAYLAND_DISPLAY,
    "XDG_RUNTIME_DIR": XDG_RUNTIME_DIR,
}

DEFAULT_ARGS = [
    "--width", "400",
    "--height", "300",
    "--title", "Test",
    "--prompt", "PIN:",
    "--ok", "OK",
    "--cancel", "Cancel",
]

_cage_proc = None


def _client_env():
    """Env for wtype / clients connecting to the cage socket."""
    env = {**os.environ}
    env["WAYLAND_DISPLAY"] = WAYLAND_DISPLAY
    env["XDG_RUNTIME_DIR"] = XDG_RUNTIME_DIR
    return env


def wait_for_log(pattern, timeout=10):
    """Poll LOG_PATH until a line matching `pattern` (regex) appears.

    Returns the matched line (stripped). Raises TimeoutError with a log
    tail for diagnostics.
    """
    rx = re.compile(pattern)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with open(LOG_PATH) as f:
                for line in f:
                    if rx.search(line):
                        return line.strip()
        except FileNotFoundError:
            pass
        time.sleep(0.2)
    tail = ""
    try:
        with open(LOG_PATH) as f:
            tail = f.read()[-500:]
    except FileNotFoundError:
        tail = "(log file missing)"
    raise TimeoutError(
        f"pattern {pattern!r} not found in {LOG_PATH} within {timeout}s; "
        f"log tail:\n{tail}"
    )


def send_key(key):
    """Send a keypress to the cage surface via wtype."""
    subprocess.run(["wtype", "-k", key], check=True, timeout=5, env=_client_env())


def _dump_log(prefix):
    """Emit the cage/test log to stdout for diagnostics."""
    try:
        with open(LOG_PATH) as f:
            sys.stdout.write(f"--- {prefix} log ---\n{f.read()}--- end log ---\n")
    except FileNotFoundError:
        sys.stdout.write(f"--- {prefix} log (missing) ---\n")


def restart_test_binary(binary, args=None):
    """Kill any running cage + test binary, truncate the log, and start
    a fresh ``cage -- <binary> <args>`` in the background.

    Returns the Popen handle.
    """
    global _cage_proc
    if _cage_proc is not None and _cage_proc.poll() is None:
        _cage_proc.terminate()
        try:
            _cage_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            _cage_proc.kill()
            _cage_proc.wait()
    subprocess.run(["pkill", "-9", "-f", "cage"], capture_output=True)
    time.sleep(0.5)

    with open(LOG_PATH, "w"):
        pass

    cmd = ["cage", "--", binary] + (args if args is not None else DEFAULT_ARGS)
    # Dedicated runtime dir; wlroots requires 0700.
    os.makedirs(XDG_RUNTIME_DIR, mode=0o700, exist_ok=True)
    os.chmod(XDG_RUNTIME_DIR, 0o700)
    env = {**os.environ, **CAGE_ENV}

    log_fh = open(LOG_PATH, "a")
    _cage_proc = subprocess.Popen(cmd, stdout=log_fh, stderr=log_fh, env=env)
    # Give cage a moment to create the Wayland socket.
    time.sleep(2.0)
    if _cage_proc.poll() is not None:
        # cage exited immediately — surface its error.
        _dump_log("cage-exited-immediately")
    return _cage_proc


def main():
    binary = sys.argv[1]
    report_path = sys.argv[2]

    report = {
        "configured": False,
        "configured_line": None,
        "hotspots_ok": False,
        "hotspots_line": None,
        "user_ok": False,
        "user_abort": False,
        "errors": [],
    }

    try:
        # --- Phase 1: configure + hotspots + Return -> UserOk ---
        restart_test_binary(binary)
        try:
            line = wait_for_log(r"configured: \d+x\d+ scale=\d+")
            m = re.search(r"configured: (\d+)x(\d+) scale=(\d+)", line)
            # Surface must configure with positive geometry (the exact
            # size depends on cosmic-text font metrics, not 400x300).
            if m and int(m.group(1)) > 0 and int(m.group(2)) > 0:
                report["configured"] = True
                report["configured_line"] = line
            else:
                report["errors"].append(f"phase1 configure: bad geometry {line!r}")
        except TimeoutError as e:
            report["errors"].append(f"phase1 configure: {e}")

        if report["configured"]:
            try:
                hl = wait_for_log(r"hotspots:", timeout=5)
                report["hotspots_line"] = hl
                # Real hotspots: contain Ok + Cancel with non-zero geometry.
                report["hotspots_ok"] = (
                    "Ok" in hl
                    and "Cancel" in hl
                    and not re.search(r"\((Ok|Cancel), 0, 0, 0, 0\)", hl)
                )
            except TimeoutError as e:
                report["errors"].append(f"hotspots: {e}")

            send_key("Return")
            time.sleep(0.5)
            try:
                wait_for_log(r"event: UserOk", timeout=5)
                report["user_ok"] = True
            except TimeoutError as e:
                report["errors"].append(f"UserOk: {e}")

        # --- Phase 2: restart + Escape -> UserAbort ---
        restart_test_binary(binary)
        try:
            wait_for_log(r"configured: \d+x\d+ scale=\d+")
        except TimeoutError as e:
            report["errors"].append(f"phase2 configure: {e}")

        send_key("Escape")
        time.sleep(0.5)
        try:
            wait_for_log(r"event: UserAbort", timeout=5)
            report["user_abort"] = True
        except TimeoutError as e:
            report["errors"].append(f"UserAbort: {e}")

    finally:
        global _cage_proc
        if _cage_proc is not None and _cage_proc.poll() is None:
            _cage_proc.terminate()
            try:
                _cage_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                _cage_proc.kill()
        subprocess.run(["pkill", "-9", "-f", "cage"], capture_output=True)
        # Surface the final cage log for diagnostics.
        _dump_log("final")

    with open(report_path, "w") as f:
        json.dump(report, f, indent=2)

    return 0 if not report["errors"] else 1


if __name__ == "__main__":
    sys.exit(main())