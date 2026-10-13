#!/usr/bin/env python3
"""Run one acceptance command under an owned, private Xvfb lifetime."""
import argparse
import os
from pathlib import Path
import secrets
import signal
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("an acceptance command is required")
    with tempfile.TemporaryDirectory(prefix="nds-screen-x11-") as directory:
        root = Path(directory)
        root.chmod(0o700)
        authority = root / "Xauthority"
        authority.touch(mode=0o600)
        env = dict(os.environ)
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("WAYLAND_SOCKET", None)
        env.update(XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11", NDS_OWNED_XVFB="1")
        source = None
        with (root / "xserver.log").open("wb") as log:
            for number in range(200, 264):
                if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
                    continue
                display = f":{number}"
                subprocess.run(["xauth", "-f", str(authority), "add", display, "MIT-MAGIC-COOKIE-1", secrets.token_hex(16)], check=True, stdout=subprocess.DEVNULL)
                candidate = subprocess.Popen(["Xvfb", display, "-screen", "0", "640x480x24", "-nolisten", "tcp", "-auth", str(authority)], stdout=subprocess.DEVNULL, stderr=log)
                env["DISPLAY"] = display
                deadline = time.monotonic() + 3
                while candidate.poll() is None and time.monotonic() < deadline:
                    try:
                        ready = subprocess.run(["xdpyinfo"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.5).returncode == 0
                    except subprocess.TimeoutExpired:
                        ready = False
                    if ready:
                        source = candidate
                        break
                    time.sleep(0.02)
                if source is not None:
                    break
                candidate.terminate()
                candidate.wait(timeout=3)
            if source is None:
                raise RuntimeError("no owned X server became ready")
            # Retain the Popen owner without polling/reaping while tests run.
            # Even an unexpected server exit cannot allow its PID to be reused.
            env["NDS_OWNED_XVFB_PID"] = str(source.pid)
            try:
                result = subprocess.run(command, env=env)
            finally:
                source.send_signal(signal.SIGCONT)
                source.terminate()
                try:
                    source.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    source.kill()
                    source.wait(timeout=3)
            raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
