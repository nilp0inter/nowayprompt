#!/usr/bin/env python3
"""TTY console fallback driver.

Run inside the NixOS test VM, once per binary:

    python3 tty-driver.py <pinentry-binary> <report.json>

Exercises the hardened TTY frontend contract:

  * termios_flags  — while a GETPIN prompt is active on /dev/tty1, ECHO,
                     ICANON and ISIG MUST be cleared (raw mode).
  * signal_sigint  — SIGINT MUST restore tty1's pre-prompt cooked termios
                     before the process exits (target hardening; the pinned
                     oracle has no signal handlers and leaves the terminal
                     raw — recorded, not asserted).
  * signal_sigtstp — SIGTSTP: the target restores termios and exits; the
                     pinned oracle takes the default action, stopping with
                     the terminal still raw — recorded, not asserted.
  * ansi_capture   — fixed 80x24 geometry on a pty: the rendered byte stream
                     MUST contain ESC[2J (clear), ESC[H (home) and the
                     ` > ` pin-row prefix with one '*' per entered secret
                     byte; the stream is recorded for the testScript's ANSI
                     structure assertions.
  * zero_leak      — RLIMIT_CORE MUST be 0; while the prompt holds the pin
                     "hunter2", a /proc/<pid>/mem scan counts the secret's
                     resident copies (the target MUST be <= the pinned
                     oracle — strict-superset leak contract: the target may
                     not leak where the oracle might).

Headless startup constraint: the pinned oracle initializes its frontend
before the Assuan loop and cannot run in this VM. Subtests that need a
live prompt record "startup_refusal" instead of hanging; the testScript
fails the gate with that diagnostic.
"""

import base64
import fcntl
import json
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

TTY1 = "/dev/tty1"
STEP_TIMEOUT = 6.0
STARTUP_TIMEOUT = 6.0
KEY_DELAY = 0.6


def set_winsize(fd, rows=24, cols=80):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def stty_state(path=TTY1):
    """Current termios state of `path` via stty -a (None on failure)."""
    r = subprocess.run(
        ["stty", "-a", "-F", path], capture_output=True, text=True
    )
    return r.stdout if r.returncode == 0 else None


def raw_flags_cleared(stty_out):
    """True when ECHO, ICANON and ISIG are all cleared in stty -a output."""
    if not stty_out:
        return False
    flags = stty_out.split()
    return "-icanon" in flags and "-echo" in flags and "-isig" in flags


class Session:
    """Binary + stdin/stdout pipes + 80x24 pty (drained continuously)."""

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
        self.lines = []
        self.pty_log = b""
        self.stdout_eof = False
        self.master_eof = False

    def _consume(self, f):
        try:
            fd = f if isinstance(f, int) else f.fileno()
            b = os.read(fd, 65536)
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

    def pump(self, timeout=0.1):
        deadline = time.time() + timeout
        while time.time() < deadline:
            rfds = []
            if not self.stdout_eof:
                rfds.append(self.proc.stdout)
            if not self.master_eof:
                rfds.append(self.master)
            if not rfds:
                time.sleep(0.02)
                return
            try:
                r, _, _ = select.select(rfds, [], [], 0.02)
            except (OSError, ValueError):
                self.stdout_eof = True
                self.master_eof = True
                return
            for f in r:
                self._consume(f)

    def wait_lines(self, n, timeout=STEP_TIMEOUT):
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.pump(0.05)
            if len(self.lines) >= n:
                self.pump(0.25)  # settle
                return True
            if self.proc.poll() is not None:
                self.pump(0.2)
                return len(self.lines) >= n
        return False

    def send(self, s):
        try:
            self.proc.stdin.write(s.encode())
            self.proc.stdin.flush()
        except (BrokenPipeError, OSError):
            pass

    def key(self, s):
        os.write(self.master, s.encode())

    def alive(self):
        return self.proc.poll() is None

    def start_getpin(self, extra_cmds=()):
        """Greeting + OPTION ttyname + extra label commands + GETPIN.

        Returns the greeting line or None if the binary refused to start.
        """
        if not self.wait_lines(1, STARTUP_TIMEOUT):
            return None
        greeting = self.lines[0]
        self.send("OPTION ttyname=" + self.pts + "\n")
        for c in extra_cmds:
            self.send(c + "\n")
        # 1 line (greeting) + 1 per OPTION/cmd.
        if not self.wait_lines(1 + 1 + len(extra_cmds)):
            return None
        self.send("GETPIN\n")
        time.sleep(KEY_DELAY)  # let the prompt render
        return greeting

    def wait_exit(self, timeout=5.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc.poll() is not None:
                self.pump(0.1)
                return self.proc.returncode
            time.sleep(0.05)
        return None

    def terminate(self):
        for sig in (signal.SIGTERM, signal.SIGKILL):
            if not self.alive():
                break
            try:
                self.proc.send_signal(sig)
            except OSError:
                break
            self.wait_exit(2.0)
        try:
            self.proc.wait(timeout=2.0)
        except Exception:
            pass

    def read_stderr(self):
        try:
            return self.proc.stderr.read().decode("utf-8", "replace")
        except Exception:
            return ""


# ---------------------------------------------------------------------------
# Subtests
# ---------------------------------------------------------------------------

def termios_flags(binary):
    """Raw termios flags on tty1 while the prompt is active."""
    baseline = stty_state()
    # Assuan over a pipe; frontend on /dev/tty1 (NOT the pty — this subtest
    # is about the virtual console).
    env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
    proc = subprocess.Popen(
        [binary],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    try:
        proc.stdin.write(b"OPTION ttyname=/dev/tty1\nGETPIN\n")
        proc.stdin.flush()
    except (BrokenPipeError, OSError):
        pass
    time.sleep(1.5)
    alive = proc.poll() is None
    during = stty_state() if alive else None
    result = {
        "baseline": baseline,
        "alive_during_prompt": alive,
        "during": during,
        "raw_flags_cleared": raw_flags_cleared(during) if alive else None,
    }
    if alive:
        try:
            proc.kill()
        except OSError:
            pass
    try:
        proc.wait(timeout=3.0)
    except Exception:
        pass
    # The terminal MUST be cooked again once the pinentry is gone.
    time.sleep(0.3)
    result["after"] = stty_state()
    result["restored_after_exit"] = result["after"] == baseline
    if not alive:
        result["startup_refusal"] = True
        try:
            result["stderr"] = proc.stderr.read().decode("utf-8", "replace")
        except Exception:
            result["stderr"] = ""
    return result


def signal_test(binary, sig, signame):
    """Signal delivery while raw; termios MUST be restored on exit."""
    baseline = stty_state()
    env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
    proc = subprocess.Popen(
        [binary],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    try:
        proc.stdin.write(b"OPTION ttyname=/dev/tty1\nGETPIN\n")
        proc.stdin.flush()
    except (BrokenPipeError, OSError):
        pass
    time.sleep(1.5)
    result = {"signal": signame, "baseline": baseline}
    if proc.poll() is not None:
        result["startup_refusal"] = True
        try:
            result["stderr"] = proc.stderr.read().decode("utf-8", "replace")
        except Exception:
            result["stderr"] = ""
        return result
    result["raw_during_prompt"] = raw_flags_cleared(stty_state())
    proc.send_signal(sig)
    code = None
    deadline = time.time() + 4.0
    while time.time() < deadline:
        if proc.poll() is not None:
            code = proc.returncode
            break
        time.sleep(0.05)
    result["exited_on_signal"] = code is not None
    result["exit_code"] = code
    if code is None:
        # Process merely stopped (default SIGTSTP action) or ignored the
        # signal: record the state, then clean up.
        result["stty_while_stopped"] = stty_state()
        try:
            proc.send_signal(signal.SIGCONT)
        except OSError:
            pass
        time.sleep(0.3)
        try:
            proc.kill()
        except OSError:
            pass
        try:
            proc.wait(timeout=3.0)
        except Exception:
            pass
    time.sleep(0.3)
    result["after"] = stty_state()
    result["restored_after_exit"] = result["after"] == baseline
    return result


def ansi_capture(binary):
    """ANSI byte capture at fixed 80x24 geometry on a pty."""
    sess = Session(binary)
    result = {}
    greeting = sess.start_getpin(
        extra_cmds=["SETTITLE T", "SETDESC D", "SETPROMPT P"]
    )
    if greeting is None:
        result["startup_refusal"] = True
        result["stderr"] = sess.read_stderr() if not sess.alive() else ""
        sess.terminate()
        return result
    # Type three secret bytes; keep the prompt up for the capture.
    sess.key("abc")
    sess.pump(0.8)
    rendered = sess.pty_log
    result["rendered"] = base64.b64encode(rendered).decode("ascii")
    result["has_clear"] = b"\x1b[2J" in rendered
    result["has_home"] = b"\x1b[H" in rendered
    result["has_pin_row"] = b" > ***" in rendered  # 3 bytes -> 3 squares
    # Complete the round trip: Enter -> D abc/END/OK, then BYE.
    sess.key("\r")
    ok = sess.wait_lines(len(sess.lines) + 3, STEP_TIMEOUT)
    result["getpin_response"] = sess.lines[-3:] if ok else sess.lines
    sess.send("BYE\n")
    result["exit_code"] = sess.wait_exit()
    sess.terminate()
    return result


def zero_leak(binary):
    """RLIMIT_CORE=0 + secret residency scan while the prompt holds it."""
    sess = Session(binary)
    result = {}
    greeting = sess.start_getpin()
    if greeting is None:
        result["startup_refusal"] = True
        result["stderr"] = sess.read_stderr() if not sess.alive() else ""
        sess.terminate()
        return result
    pid = sess.proc.pid
    # RLIMIT_CORE MUST be 0 (no core can be written).
    with open(f"/proc/{pid}/limits") as f:
        for line in f:
            if line.startswith("Max core file size"):
                result["core_limit"] = line.strip()
                break
    fields = result.get("core_limit", "").split()
    # "Max core file size <soft> <hard> bytes" -> fields[4], fields[5].
    result["core_limit_zero"] = (
        len(fields) >= 6 and fields[4] == "0" and fields[5] == "0"
    )
    # Enter the pin; scan resident memory while the prompt holds it.
    sess.key("hunter2")
    sess.pump(0.6)
    result["hits"] = scan_proc_mem(pid, b"hunter2")
    result["copies"] = sum(h["count"] for h in result["hits"])
    # Complete + exit; verify no core appears.
    sess.key("\r")
    sess.wait_lines(len(sess.lines) + 3, STEP_TIMEOUT)
    sess.send("BYE\n")
    result["exit_code"] = sess.wait_exit()
    time.sleep(0.3)
    cores = [
        f for f in os.listdir("/tmp")
        if f.startswith("core")
    ]
    result["core_files_in_tmp"] = cores
    sess.terminate()
    return result


def scan_proc_mem(pid, needle):
    """Count occurrences of `needle` in readable private mappings of pid."""
    hits = []
    try:
        with open(f"/proc/{pid}/maps") as f:
            maps = f.readlines()
        mem_fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)
    except OSError as e:
        return [{"error": str(e)}]
    try:
        for line in maps:
            parts = line.split()
            rng, perms = parts[0], parts[1]
            # Private writable mappings only (heap, stack, anon, mlocked
            # secret pages). File-backed .so text cannot hold the secret.
            if "w" not in perms or "p" not in perms:
                continue
            start_s, end_s = rng.split("-")
            start, end = int(start_s, 16), int(end_s, 16)
            count = 0
            pos = start
            while pos < end:
                chunk = min(1 << 16, end - pos)
                try:
                    os.lseek(mem_fd, pos, os.SEEK_SET)
                    data = os.read(mem_fd, chunk)
                except OSError:
                    break  # unmapped hole inside the region
                if not data:
                    break
                count += data.count(needle)
                pos += len(data)
            if count:
                hits.append({
                    "range": rng,
                    "name": parts[-1] if len(parts) > 5 else "",
                    "count": count,
                })
    finally:
        os.close(mem_fd)
    return hits


def main():
    binary, out_path = sys.argv[1], sys.argv[2]
    report = {"binary": binary}
    report["termios_flags"] = termios_flags(binary)
    report["signal_sigint"] = signal_test(binary, signal.SIGINT, "SIGINT")
    report["signal_sigtstp"] = signal_test(binary, signal.SIGTSTP, "SIGTSTP")
    report["ansi_capture"] = ansi_capture(binary)
    report["zero_leak"] = zero_leak(binary)
    with open(out_path, "w") as f:
        json.dump(report, f, indent=2)
    print(f"report written to {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
