#!/usr/bin/env python3
"""Exercise the actual packaged portable application on a focal X11 desktop."""

import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time


def main():
    output = Path(sys.argv[1]).resolve()
    for name in ("workbench.xwd", "workbench.png", "window-tree.txt", "wm-class.txt"):
        (output / name).unlink(missing_ok=True)
    dist = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("dist")
    archives = list(dist.glob("NyaTerm_*_linux_x64_glibc231.tar.gz"))
    if len(archives) != 1:
        raise RuntimeError("expected exactly one newly built focal archive in dist")
    with tempfile.TemporaryDirectory() as directory:
        # Only extract our just-created archive; this is not an untrusted archive importer.
        with tarfile.open(archives[0]) as archive:
            archive.extractall(directory)
        application = next(Path(directory).glob("*/nyaterm"))
        with (output / "application.log").open("w") as log:
            launcher = application.with_name("start-nyaterm.sh")
            process = subprocess.Popen(["dbus-run-session", "--", str(launcher), "--software"],
                                       stdout=log, stderr=log, start_new_session=True,
                                       env=dict(os.environ, RUST_LOG="info"))
            try:
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError("application exited: inspect application.log")
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    window = re.search(r'(0x[0-9a-f]+) "NyaTerm(?: Preview)?".*?1280x800', tree)
                    if window:
                        wm_class = re.search(r'\("(nyaterm(?:-preview)?)" "(NyaTerm(?: Preview)?)"\)', window[0])
                        if not wm_class:
                            # GPUI maps the native window before constructing the
                            # root view, where the X11 class is installed.
                            time.sleep(0.1)
                            continue
                        (output / "wm-class.txt").write_text(wm_class[0] + "\n", encoding="utf-8")
                        time.sleep(3)
                        if shutil.which("xwd"):
                            capture = ["xwd", "-silent", "-id", window[1], "-out",
                                       str(output / "workbench.xwd")]
                        else:
                            capture = ["import", "-window", window[1], str(output / "workbench.png")]
                        subprocess.run(capture, check=True)
                        if process.poll() is not None:
                            raise RuntimeError("application exited during rendering")
                        (output / "window-tree.txt").write_text(tree, encoding="utf-8")
                        print("PASS: packaged application window and screenshot captured")
                        return
                    time.sleep(0.5)
                raise RuntimeError("no rendered NyaTerm window within 60 seconds")
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()


if __name__ == "__main__":
    main()
