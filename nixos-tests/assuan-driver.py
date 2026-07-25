#!/usr/bin/env python3
"""Stage 2 Assuan IPC differential driver.

Run inside the NixOS test VM, once per binary:

    python3 assuan-driver.py <pinentry-binary> <report.json>

Drives the binary through the scripted Assuan stream defined by tasks 15.2
and records a JSON transcript report:

    {
      "binary": "...",
      "startup": {"greeting": "OK ..." | null,
                   "exit_code": int | null,
                   "stderr": "..."},
      "sessions": {
          "<name>": {"steps": [{"cmd": ..., "resp": [...], ...}],
                     "exit_code": int | null,
                     "stderr": "...",
                     "pty": "<base64 of bytes rendered to the pty>",
                     "skipped": "..."}
      }
    }

Mechanics:
  * Each session runs the binary with stdin/stdout pipes and a fresh pty
    pair (80x24 winsize). The pty slave path is handed to the binary via
    `OPTION ttyname=<pts>`; keystrokes for GETPIN/CONFIRM/MESSAGE are written
    to the pty master, and everything the binary renders to the pty is
    captured (stage 3 reuses the same capture technique).
  * The pty master is drained continuously — the TTY render path writes to
    the tty fd, and an undrained ~8 KiB pty buffer would deadlock the binary
    mid-render.
  * WAYLAND_DISPLAY is scrubbed from the environment: the differential
    contract is the TTY/Assuan path, and the legacy binary would otherwise
    attempt a Wayland connection.
  * Per-step and per-session timeouts keep the VM test from hanging on a
    binary that refuses to start or stalls mid-prompt.
  * GETPIN/CONFIRM/MESSAGE produce no stdout at command receipt — the
    response arrives only after the scripted keystroke. Plans therefore use
    `("!key", bytes)` followed by `("!wait", n, label)` to name the response
    after the keystroke that elicits it.

The comparison and tolerance contract live in stage-2-assuan.nix's
testScript; this driver only records.
"""

import base64
import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

STEP_TIMEOUT = 6.0     # seconds to wait for one command's response lines
SETTLE = 0.25          # extra drain after the expected lines arrive
STARTUP_TIMEOUT = 6.0  # seconds to wait for the greeting
KEY_DELAY = 0.6        # wait for the prompt to render before typing
SESSION_EXIT_TIMEOUT = 8.0


def set_winsize(fd, rows=24, cols=80):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


class Dead(Exception):
    """The binary exited mid-session."""

    def __init__(self, code, stderr):
        super().__init__(f"binary exited with code {code}: {stderr!r}")
        self.code = code
        self.stderr = stderr


class Session:
    def __init__(self, binary):
        self.master, self.slave = pty.openpty()
        set_winsize(self.master)
        self.pts = os.ttyname(self.slave)
        env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
        self.proc = subprocess.Popen(
            [binary],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        self.buf = b""
        self.lines = []      # newline-split stdout transcript
        self.pty_log = b""   # everything rendered to the pty
        self.stdout_eof = False
        self.master_eof = False

    # -- low-level ------------------------------------------------------

    def read_stderr(self):
        """Only call once the process has exited (read() blocks otherwise)."""
        try:
            return self.proc.stderr.read().decode("utf-8", "replace")
        except Exception:
            return ""

    def _consume(self, f):
        try:
            b = os.read(f.fileno(), 65536)
        except OSError:
            b = b""
        if f is self.proc.stdout:
            if not b:
                self.stdout_eof = True
                return
            self.buf += b
            while b"\n" in self.buf:
                ln, self.buf = self.buf.split(b"\n", 1)
                self.lines.append(ln.decode("utf-8", "replace"))
        else:
            if not b:
                self.master_eof = True
                return
            self.pty_log += b

    def _drain_final(self):
        """After process exit: collect any remaining stdout/pty bytes."""
        deadline = time.time() + 1.0
        while time.time() < deadline:
            rfds = []
            if not self.stdout_eof:
                rfds.append(self.proc.stdout)
            if not self.master_eof:
                rfds.append(self.master)
            if not rfds:
                break
            r, _, _ = select.select(rfds, [], [], 0.05)
            for f in r:
                self._consume(f)

    def _pump_once(self, timeout):
        rfds = []
        if not self.stdout_eof:
            rfds.append(self.proc.stdout)
        if not self.master_eof:
            rfds.append(self.master)
        if not rfds:
            time.sleep(timeout)
            return
        try:
            r, _, _ = select.select(rfds, [], [], timeout)
        except (OSError, ValueError):
            self.stdout_eof = True
            self.master_eof = True
            return
        for f in r:
            self._consume(f)

    def pump(self, want_lines, timeout):
        """Drain stdout+pty until `want_lines` total stdout lines or timeout.

        Raises Dead if the process exits before reaching want_lines.
        """
        deadline = time.time() + timeout
        while True:
            if len(self.lines) >= want_lines:
                settle_end = time.time() + SETTLE
                while time.time() < settle_end:
                    self._pump_once(0.02)
                return
            if time.time() >= deadline:
                raise TimeoutError(
                    f"timed out waiting for line {want_lines}; "
                    f"have {len(self.lines)}: {self.lines!r}"
                )
            self._pump_once(0.05)
            if self.proc.poll() is not None:
                self._drain_final()
                if len(self.lines) >= want_lines:
                    return
                raise Dead(self.proc.returncode, self.read_stderr())

    # -- high-level -----------------------------------------------------

    def send(self, s):
        try:
            self.proc.stdin.write(s.encode())
            self.proc.stdin.flush()
        except (BrokenPipeError, OSError):
            pass  # recorded when the next pump notices the exit

    def key(self, s):
        """Type into the pty master (as the user would on the tty)."""
        os.write(self.master, s.encode())

    def wait_exit(self, timeout=SESSION_EXIT_TIMEOUT):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc.poll() is not None:
                self._drain_final()
                return self.proc.returncode
            self._pump_once(0.05)
        try:
            self.proc.kill()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=5.0)
        except Exception:
            pass
        self._drain_final()
        return None  # had to be killed

    def kill(self):
        try:
            if self.proc.poll() is None:
                self.proc.kill()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=5.0)
        except Exception:
            pass
        self._drain_final()


def greeting(sess):
    """Wait for the startup greeting line. Returns the line or None.

    On startup refusal (binary exits before greeting) the process stderr is
    stashed on `sess.startup_stderr` for the caller to report.
    """
    sess.startup_stderr = ""
    try:
        sess.pump(1, STARTUP_TIMEOUT)
    except Dead as d:
        sess.startup_stderr = d.stderr
        return None
    except TimeoutError:
        return None
    return sess.lines[0] if sess.lines else None


def run_steps(sess, plan, steps):
    """Execute plan entries, appending step records to `steps`.

    Entry shapes:
      (cmd, n)                — send `cmd\\n`, wait for n new stdout lines
      ("!key", bytes)         — after KEY_DELAY, type bytes on the pty
      ("!wait", n, label)     — wait for n new lines, record under `label`
      ("!sleep", seconds)     — pause
      ("!raw", str)           — send without newline or line accounting
    Raises Dead/TimeoutError on failure (partial records stay in `steps`).
    """
    for entry in plan:
        kind = entry[0]
        if kind == "!sleep":
            time.sleep(entry[1])
            continue
        if kind == "!key":
            time.sleep(KEY_DELAY)
            sess.key(entry[1])
            continue
        if kind == "!raw":
            sess.send(entry[1])
            continue
        if kind == "!wait":
            _, n, label = entry
            before = len(sess.lines)
            sess.pump(before + n, STEP_TIMEOUT)
            steps.append({"cmd": label, "resp": sess.lines[before:]})
            continue
        cmd, n = entry
        before = len(sess.lines)
        sess.send(cmd + "\n")
        sess.pump(before + n, STEP_TIMEOUT)
        steps.append({"cmd": cmd, "resp": sess.lines[before:]})


# ---------------------------------------------------------------------------
# Session definitions (tasks 15.2 / 15.3).
# ---------------------------------------------------------------------------

NOT_IMPL = [
    "CANCEL",
    "SETGENPIN",
    "SETGENPIN_TT",
    "SETTIMEOUT 30",
    "END",
    "QUIT",
    "AUTH",
    "CLEARPASSPHRASE",
    "SETREPEAT 2",
    "SETREPEATERROR again",
    "SETQUALITYBAR",
    "SETQUALITYBAR_TT",
]

HELP_LINES = 10  # 9 "# <CMD>" lines + OK


def matrix_plan(pts):
    """Full command matrix on one session."""
    plan = [
        ("SETTITLE Test_Title", 1),
        ("SETPROMPT Prompt%20Text", 1),
        ("SETDESC Desc%20With%20Spaces", 1),
        ("SETERROR Error_Msg", 1),
        ("SETOK _OK", 1),
        ("SETNOTOK Not_OK", 1),
        ("SETCANCEL Cancel", 1),
        ("OPTION ttyname=" + pts, 1),
        ("OPTION default-ok=unused1", 1),
        ("OPTION default-cancel=unused2", 1),
        ("OPTION default-yes=unused3", 1),
        ("OPTION default-no=unused4", 1),
        ("OPTION putenv=WAYLAND_DISPLAY=wayland-0", 1),
        ("OPTION nosuchoption=whatever", 1),
        ("NOP", 1),
        ("HELP", HELP_LINES),
        ("SETKEYINFO 1234ABCD", 1),
    ]
    plan += [(c, 1) for c in NOT_IMPL]
    plan += [
        ("BOGUS", 1),
        ("GETINFO flavor", 3),
        ("GETINFO version", 3),
        ("GETINFO pid", 3),
        ("GETINFO nosuchinfo", 1),
        # GETPIN with a non-empty secret: no response at command receipt —
        # the D/END/OK triplet arrives after the scripted keystroke.
        ("GETPIN", 0),
        ("!key", "hunter2\r"),
        ("!wait", 3, "GETPIN<resp>"),
        # CONFIRM answered with Enter (user_ok).
        ("CONFIRM", 0),
        ("!key", "\r"),
        ("!wait", 1, "CONFIRM<resp>"),
        ("RESET", 1),
        ("BYE", 1),
    ]
    return plan


def run_session(binary, plan_fn):
    """Run one session; plan_fn(sess) returns the plan (may use sess.pts)."""
    sess = Session(binary)
    rep = {}
    greet = greeting(sess)
    if greet is None:
        code = sess.wait_exit(2.0)
        stderr = sess.startup_stderr
        sess.kill()
        return {
            "greeting": None,
            "steps": [],
            "exit_code": code,
            "stderr": stderr,
            "pty": base64.b64encode(sess.pty_log).decode("ascii"),
            "skipped": "no greeting (binary exited before the Assuan loop)",
        }
    rep["greeting"] = greet
    steps = []
    error = None
    try:
        run_steps(sess, plan_fn(sess), steps)
    except (Dead, TimeoutError) as e:
        error = repr(e)
    code = sess.wait_exit()
    if code is None:
        sess.kill()
    rep.update({
        "steps": steps,
        "exit_code": sess.proc.returncode,
        "stderr": sess.read_stderr(),
        "pty": base64.b64encode(sess.pty_log).decode("ascii"),
    })
    if error:
        rep["error"] = error
    return rep


def partial_line_session(binary):
    """Partial-line stdin handling (design decision D9).

    Sends `SETT`, waits, then completes `ITLE X\\n`. A buffering reader
    (the target) assembles one SETTITLE command → OK; the legacy
    read()-and-split loop mis-splits it into two unknown commands →
    two ERR 536871187 lines. The comparator documents this intentional
    divergence; this session only records each binary's behavior.
    """
    sess = Session(binary)
    if greeting(sess) is None:
        code = sess.wait_exit(2.0)
        stderr = sess.startup_stderr
        sess.kill()
        return {
            "greeting": None,
            "steps": [],
            "exit_code": code,
            "stderr": stderr,
            "pty": "",
            "skipped": "no greeting (binary exited before the Assuan loop)",
        }
    steps = []
    error = None
    try:
        sess.send("SETT")       # partial line; no newline yet
        time.sleep(0.4)         # give a non-buffering reader time to mis-split
        before = len(sess.lines)
        sess.send("ITLE X\n")
        time.sleep(0.4)
        sess.send("BYE\n")
        try:
            sess.pump(before + 3, STEP_TIMEOUT)  # oracle: ERR ERR OK
        except (Dead, TimeoutError):
            pass               # target exits after BYE with fewer lines
        steps.append({"cmd": "SETT|ITLE X|BYE", "resp": sess.lines[before:]})
    except (Dead, TimeoutError) as e:
        error = repr(e)
    code = sess.wait_exit()
    rep = {
        "greeting": sess.lines[0] if sess.lines else None,
        "steps": steps,
        "exit_code": sess.proc.returncode,
        "stderr": sess.read_stderr(),
        "pty": base64.b64encode(sess.pty_log).decode("ascii"),
    }
    if error:
        rep["error"] = error
    sess.kill()
    return rep


def main():
    binary, out_path = sys.argv[1], sys.argv[2]
    report = {"binary": binary, "startup": {}, "sessions": {}}

    # -- startup probe ----------------------------------------------------
    probe = Session(binary)
    greet = greeting(probe)
    exited = probe.proc.poll() is not None
    report["startup"] = {
        "greeting": greet,
        "exit_code": probe.proc.returncode if exited else None,
        "stderr": probe.startup_stderr or (probe.read_stderr() if exited else ""),
    }
    probe.kill()

    if greet is None:
        report["startup"]["note"] = (
            "binary exited before emitting the greeting — both the legacy "
            "oracle and the current target initialize their frontend before "
            "the Assuan loop, so a headless VM (no WAYLAND_DISPLAY, no "
            "pre-configured tty) cannot start either binary"
        )
        with open(out_path, "w") as f:
            json.dump(report, f, indent=2)
        print(f"report written to {out_path} (startup refusal recorded)")
        return 0

    # -- sessions ----------------------------------------------------------
    report["sessions"]["matrix"] = run_session(
        binary, lambda s: matrix_plan(s.pts)
    )
    report["sessions"]["empty_pin"] = run_session(
        binary,
        lambda s: [
            ("OPTION ttyname=" + s.pts, 1),
            ("GETPIN", 0),
            ("!key", "\r"),  # Enter on an empty prompt -> OK (no D)
            ("!wait", 1, "GETPIN<resp>"),
            ("BYE", 1),
        ],
    )
    report["sessions"]["message"] = run_session(
        binary,
        lambda s: [
            ("SETTITLE MsgTitle", 1),
            ("MESSAGE", 0),
            ("!key", "\r"),  # user_ok on a message -> OK
            ("!wait", 1, "MESSAGE<resp>"),
            ("BYE", 1),
        ],
    )
    report["sessions"]["defaults"] = run_session(
        binary,
        lambda s: [
            ("OPTION ttyname=" + s.pts, 1),
            ("OPTION default-ok=_Save", 1),
            ("OPTION default-cancel=C_ancel", 1),
            ("GETPIN", 0),
            ("!key", "pw\r"),
            ("!wait", 3, "GETPIN<resp>"),
            ("BYE", 1),
        ],
    )
    report["sessions"]["partial_line"] = partial_line_session(binary)

    with open(out_path, "w") as f:
        json.dump(report, f, indent=2)
    print(f"report written to {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
