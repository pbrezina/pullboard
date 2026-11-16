#!/usr/bin/env python3
"""Real PTY, mock discovery/prerequisites, safe HOME. Never installs or contacts a forge.

Build with cargo test --no-run and pass the wizard-* integration-test executable.
The ignored terminal_fixture runs the actual linear wizard and prompt adapter.
"""
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def smoke(binary, no_color=False, termination=None, at_password=False, manual=False,
          discovery_menu=False, forge_cancel=False, forge_names=False):
    temp_root = "/tmp/opencode" if Path("/tmp/opencode").is_dir() else None
    with tempfile.TemporaryDirectory(prefix="pullboard-inline-pty-", dir=temp_root) as home:
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 110, 0, 0))
        env = dict(os.environ, HOME=home, TERM="xterm-256color", PULLBOARD_PTY_FIXTURE="1")
        env.pop("NO_COLOR", None)
        env.pop("PULLBOARD_PTY_MANUAL", None)
        if manual:
            env["PULLBOARD_PTY_MANUAL"] = "1"
        if no_color:
            env["NO_COLOR"] = "1"
        child = subprocess.Popen(
            [binary, "--ignored", "--exact", "terminal_fixture", "--nocapture", "--color", "never"],
            stdin=slave, stdout=slave, stderr=slave, env=env,
        )
        output = bytearray()
        position = 0

        def drain(seconds=0.08):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    try:
                        output.extend(os.read(master, 65536))
                    except OSError:
                        break

        def wait(text):
            nonlocal position
            deadline = time.monotonic() + 8
            needle = text.encode()
            while time.monotonic() < deadline:
                drain()
                found = output.find(needle, position)
                if found >= 0:
                    position = found + len(needle)
                    return
                if child.poll() is not None:
                    break
            raise AssertionError(f"missing {text!r}: {bytes(output)[-3000:]!r}")

        def answer(text, value=b"\r"):
            wait(text)
            os.write(master, value)
            drain()

        try:
            answer("Configure Jira Cloud?")
            answer("Jira Cloud tenant URL:", b"https://first.atlassian.net\r")
            answer("Atlassian account email:", b"human@example.invalid\r")
            wait("Token:")
            sentinel = b"PTY-SYNTHETIC-SECRET-DO-NOT-ECHO"
            os.write(master, sentinel)
            drain()
            # The optional visibility toggle must remain disabled.
            os.write(master, b"\x12")
            drain()
            if at_password:
                if termination:
                    child.send_signal(termination)
                else:
                    os.write(master, b"\x03")
            else:
                os.write(master, b"\r")
                if manual:
                    answer("Project actions:", b"add project\r")
                    answer("Project key, exact name", b"TWO\r")
                    answer("Project actions:")
                    answer("Board actions:", b"add board\r")
                    answer("Board ID or same-tenant", b"88\r")
                    answer("Board actions:")
                else:
                    wait("Jira projects:")
                    if discovery_menu:
                        os.write(master, b"\x0f")
                        answer("Project actions:")
                        wait("Jira projects:")
                    # Type-filter, arrow navigation and Space in a real MultiSelect.
                    os.write(master, b"two\x1b[B \r")
                    wait("Jira boards:")
                    if discovery_menu:
                        os.write(master, b"\x0f")
                        answer("Board actions:")
                        wait("Jira boards:")
                    os.write(master, b"\r")
                wait("Assignee groups (at least one):")
                if discovery_menu:
                    os.write(master, b"\x0f")
                    answer("Group actions:")
                    wait("Assignee groups (at least one):")
                os.write(master, b"\r")
                answer("Team field (semantic name):")
                wait("Team:")
                if not no_color:
                    frame = bytes(output[position:])
                    assert b"\x1b[36m" in frame, "team picker lost selection color"
                    assert b"\x1b[34mCtrl-O More options" in frame, "picker help lost blue styling"
                if discovery_menu:
                    os.write(master, b"\x0f")
                    answer("Team actions:")
                    wait("Team:")
                os.write(master, b"\r")
                answer("Regular expression to identify your sprints:")
                answer("Sprint pattern actions:")
                if forge_cancel:
                    answer("Add a forge account?", b"y\r")
                    # Esc is local at provider, name, URL and masked token.
                    for stage in range(4):
                        wait("Forge provider:")
                        if stage > 0:
                            os.write(master, b"\r")
                            wait("Forge name:")
                        if stage > 1:
                            os.write(master, b"\r")
                            wait("Forge URL")
                        if stage > 2:
                            os.write(master, b"\r")
                            wait("Token:")
                            os.write(master, sentinel)
                            drain()
                        os.write(master, b"\x1b")
                        answer("Add another forge?", b"y\r" if stage < 3 else b"n\r")
                elif forge_names:
                    answer("Add a forge account?", b"y\r")
                    for stage, name in enumerate([b"\r", b"\r", b"Work GitHub\r"]):
                        answer("Forge provider:")
                        answer("Forge name:", name)
                        answer("Forge URL")
                        answer("Token:", sentinel + b"\r")
                        answer("Add another forge?", b"y\r" if stage < 2 else b"n\r")
                else:
                    answer("Add a forge account?")
                wait("AI provider:")
                # Select supports type-filter too: type Disabled then accept it.
                os.write(master, b"disabled\r")
                answer("Local host port:")
                answer("Container channel:", b"\x1b[B\r")
                answer("Start in Test mode?")
                wait("Install:")
                assert b"projects TWO (primary first)" in output
                assert b"pullboard:nightly" in output
                assert b"[hidden]" in output
                assert b"non-exhaustive" in output
                if forge_names:
                    assert b"Forge: github (github-1)" in output
                    assert b"Forge: github-2 (github-2)" in output
                    assert b"Forge: Work GitHub (github-3)" in output
                if manual:
                    assert b"manual entry remains available" in output
                    assert b"board 88" in output
                if termination:
                    child.send_signal(termination)
                else:
                    os.write(master, b"\x03")
            child.wait(timeout=8)
            drain()
            assert child.returncode == 0, bytes(output)[-2000:]
            assert sentinel not in output
            assert b"Configure advanced issue-type/workflow" not in output
            assert b"Configure advanced repository" not in output
            assert b"Include repository globs" not in output
            assert b"Exclude repository globs" not in output
            assert b"Default Jira project:" not in output
            assert b"Draft status suggestion:" not in output
            assert b"Ready status suggestion:" not in output
            assert b"\x1b[?1049" not in output, "alternate screen used"
            assert b"\x1b[?2004l" in output, "bracketed paste not disabled"
            assert b"cancelled" in output
            assert termios.tcgetattr(slave) == original, "terminal flags not restored exactly"
            if no_color:
                assert not re.search(rb"\x1b\[[0-9;]*m", output), "SGR styles in no-color mode"
            assert list(Path(home).iterdir()) == [], "fixture wrote to HOME"
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            os.close(master)
            os.close(slave)


def startup_cancellation(binary, attempts=1000):
    """Queued cancellation at the first async yield, before worker startup settles.

    No input is provided: cleanup must neither wait for a blocked library prompt
    nor allow late raw-mode/paste initialization after restoring the terminal.
    """
    temp_root = "/tmp/opencode" if Path("/tmp/opencode").is_dir() else None
    with tempfile.TemporaryDirectory(prefix="pullboard-inline-race-", dir=temp_root) as home:
        for attempt in range(attempts):
            master, slave = pty.openpty()
            original = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 110, 0, 0))
            child = subprocess.Popen(
                [binary, "--ignored", "--exact", "terminal_startup_cancel_fixture", "--nocapture", "--color", "never"],
                stdin=slave, stdout=slave, stderr=slave,
                env=dict(os.environ, HOME=home, TERM="xterm-256color", NO_COLOR="1", PULLBOARD_PTY_FIXTURE="1"),
            )
            try:
                child.wait(timeout=5)
                output = bytearray()
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                assert child.returncode == 0, (attempt, bytes(output))
                assert termios.tcgetattr(slave) == original, (attempt, bytes(output))
                cleanup = output.rfind(b"\x1b[?2004l\x1b[?25h")
                assert cleanup >= 0, (attempt, bytes(output))
                assert b"\x1b[?2004h" not in output[cleanup:], (attempt, bytes(output))
                assert b"\x1b[?25l" not in output[cleanup:], (attempt, bytes(output))
                assert b"\x1b[?1049" not in output, (attempt, bytes(output))
                assert output.rstrip().endswith(b"cancelled; no files changed."), (attempt, bytes(output))
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
                termios.tcsetattr(slave, termios.TCSANOW, original)
                os.close(master)
                os.close(slave)
        assert list(Path(home).iterdir()) == [], "startup fixture wrote to HOME"


if __name__ == "__main__":
    executable = str(Path(sys.argv[1]).resolve())
    smoke(executable)
    smoke(executable, no_color=True)
    smoke(executable, no_color=True, termination=signal.SIGTERM)
    smoke(executable, no_color=True, termination=signal.SIGHUP)
    smoke(executable, at_password=True)
    smoke(executable, no_color=True, termination=signal.SIGTERM, at_password=True)
    smoke(executable, no_color=True, termination=signal.SIGHUP, at_password=True)
    smoke(executable, manual=True)
    smoke(executable, no_color=True, manual=True)
    smoke(executable, discovery_menu=True)
    smoke(executable, no_color=True, discovery_menu=True)
    smoke(executable, forge_cancel=True)
    smoke(executable, no_color=True, forge_cancel=True)
    smoke(executable, forge_names=True)
    smoke(executable, no_color=True, forge_names=True)
    startup_cancellation(executable)
    print("15 PTY flows + 1000 queued-startup cancellation cases passed: direct recent/manual/team pickers, blue help/cyan selections, Ctrl-O More options, local forge Esc, default/custom forge names, no advanced repository prompts, arrows, multiselect/search, Enter defaults, hidden secrets, no color/alternate screen, Ctrl-C/TERM/HUP exact termios/cursor/paste restoration; no network/install/files")
