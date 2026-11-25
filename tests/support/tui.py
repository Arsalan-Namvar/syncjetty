"""PTY acceptance driver: real Rust TUI, real IPC/SQLite, controlled Drive adapter."""
import errno
import fcntl
import os
import pathlib
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

executable, directory = sys.argv[1:]
root = pathlib.Path(directory)
runtime = root / "runtime"
runtime.mkdir(mode=0o700)
populated = root / "populated"
populated.mkdir()
(populated / "keep.txt").write_bytes(b"untouched user contents")
env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_STATE_HOME=str(root / "state"))


def launch():
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 48, 160, 0, 0))
    process = subprocess.Popen([executable, "--exact", "keyboard_onboarding_browsing_and_restart_on_real_terminal", "--nocapture"], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
    os.close(slave)
    return master, process


def resize(rows, columns):
    global buffer
    while select.select([master], [], [], 0)[0]:
        os.read(master, 65536)
    buffer = b""
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
    os.killpg(process.pid, signal.SIGWINCH)


master, process = launch()
buffer = b""


def expect(text):
    global buffer
    needle = text.encode()
    deadline = time.monotonic() + 12
    while needle not in buffer:
        if time.monotonic() >= deadline:
            raise AssertionError(f"Missing visible {text!r}; terminal tail: {buffer[-7000:]!r}")
        if select.select([master], [], [], 0.1)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    raise AssertionError(f"TUI exited before {text!r}: {buffer[-7000:]!r}") from error
                raise
            if not data:
                raise AssertionError(f"TUI EOF before {text!r}")
            buffer += data
    buffer = buffer[buffer.index(needle) + len(needle):]


def key(data):
    global buffer
    # Discard queued earlier frames so expectations observe the next action.
    while select.select([master], [], [], 0)[0]:
        os.read(master, 65536)
    buffer = b""
    os.write(master, data)


def close():
    key(b"q")
    process.wait(timeout=8)
    if process.returncode:
        raise AssertionError(f"Rust TUI child failed with {process.returncode}")
    os.close(master)


try:
    expect("No connected accounts")
    key(b"c")
    expect("Authorizing:")
    key(b"\x1b")
    expect("Authorization cancelled")
    key(b"c")
    expect("Google is unavailable")
    key(b"c")
    expect("PENDING ACTIVATION")
    expect("名前-é-Alice")
    expect("permanent and automatic")
    resize(10, 80)
    expect("approval is disabled")
    key(b"y")
    expect("approval is disabled")
    resize(48, 160)
    expect("PENDING ACTIVATION")
    key(b"m\x15" + str(populated).encode() + b"\r")
    key(b"y")
    expect("Mount location is populated")
    key(b"m\x15" + str(root / "名前-é-drive").encode() + b"\r")
    key(b"b\x152\r")
    expect("Evictable cache budget: 2 GiB")
    key(b"y")
    expect("Loading metadata")
    expect("Documents")
    expect("available (n)")
    key(b"n")
    expect("Incomplete discovery")
    expect("second-page.txt")
    key(b"\r")
    expect("nested.txt")
    key(b"\x1b[B\r")
    expect("> [file] viewport-first.txt")
    resize(10, 80)
    expect("alice@example.test")
    expect("> [file] viewport-first.txt")
    key(b"\x1b[B")
    expect("> [file] viewport-second.txt")
    key(b"\x1b[A")
    expect("> [file] viewport-first.txt")
    key(b"\x7f")
    expect("> [file] nested.txt")
    resize(48, 160)
    expect("nested.txt")
    key(b"\x7f")
    expect("second-page.txt")
    close()
    master, process = launch()
    buffer = b""
    expect("alice@example.test")
    expect("second-page.txt")
    expect("no mounted filesystem")
    close()
    print("Observed real TUI: cancellation, provider failure, activation rejection/review, 2 GiB cache, loading, pagination, nested browsing, close and restart retention")
finally:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        os.close(master)
