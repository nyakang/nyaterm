#!/usr/bin/env python3
"""Exercise the actual packaged portable application on a focal X11 desktop."""

import os
import hashlib
import json
from pathlib import Path
import re
import shutil
import signal
import socket
import struct
from concurrent.futures import ThreadPoolExecutor
import subprocess
import sys
import tarfile
import tempfile
import time


def android_probe(application):
    root = application.parent
    manifest = json.loads((root / "android-resources.json").read_text())
    metadata = Path(__file__).resolve().parents[2] / "release/android-package.json"
    if (manifest.get("schema") != 1 or manifest.get("target") != "x86_64-unknown-linux-gnu"
            or manifest.get("dependencies") != json.loads(metadata.read_text(encoding="utf-8"))):
        raise RuntimeError("invalid packaged Android manifest")
    required = {"adb/adb", "adb/NOTICE.txt", "adb/SOURCE.json", "android-libs/LICENSE.txt",
                "android-libs/SOURCE.json", *["android-libs/" + name for name in
                ("libavcodec.so.62", "libavformat.so.62", "libavutil.so.60", "libswscale.so.9")]}
    if not required <= manifest["files"].keys():
        raise RuntimeError("incomplete packaged Android runtime")
    for name, digest in manifest["files"].items():
        path = (root / name).resolve()
        if root.resolve() not in path.parents or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise RuntimeError("Android resource checksum mismatch: " + name)
    environment = dict(os.environ)
    environment.pop("LD_LIBRARY_PATH", None)
    subprocess.run([str(root / "adb/adb"), "version"], check=True, timeout=10, env=environment,
                   stdout=subprocess.DEVNULL)
    probe_android_helper(root / "nyaterm-android-helper", environment)


def probe_android_helper(helper, environment):
    # Starting the isolated decoder also verifies the packaged FFmpeg loader
    # paths. No device or ADB daemon is needed for this IPC lifecycle probe.
    def packet(value):
        control = json.dumps(value).encode()
        return b"\x01" + struct.pack("<I", len(control)) + control

    with socket.socket() as listener, ThreadPoolExecutor(max_workers=1) as executor:
        listener.bind(("127.0.0.1", 0))
        listener.listen(2)
        listener.settimeout(10)

        def scrcpy_peer():
            video, _ = listener.accept()
            with video:
                video.settimeout(10)
                video.sendall(bytes(65) + b"h264" + struct.pack(">III", 1 << 31, 8, 8))
                control, _ = listener.accept()
                with control:
                    video.recv(1)

        peer = executor.submit(scrcpy_peer)
        connect = {"type": "connect", "binding": {"server": "127.0.0.1:5037", "serial": "synthetic",
                   "transport_id": None, "generation": 1},
                   "endpoint": "127.0.0.1:" + str(listener.getsockname()[1]),
                   "settings": {"max_size": 1280, "max_fps": 30, "bit_rate": 8000000}}
        result = subprocess.run([str(helper)],
                                input=packet(connect) + packet({"type": "disconnect"}),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
                                timeout=15, env=environment)
        peer.result(timeout=10)
    packets = []
    data = result.stdout
    while data:
        if len(data) < 5 or data[0] != 1:
            raise RuntimeError("invalid Android helper control packet")
        length = struct.unpack("<I", data[1:5])[0]
        if length > 128 * 1024 or len(data) < length + 5:
            raise RuntimeError("invalid Android helper control length")
        packets.append(json.loads(data[5:5 + length]))
        data = data[5 + length:]
    protocol_source = Path(__file__).resolve().parents[3] / "crates/nyaterm-android/src/ipc.rs"
    protocol = int(re.search(r"pub const PROTOCOL_VERSION: u32 = (\d+);", protocol_source.read_text(encoding="utf-8"))[1])
    if (len(packets) < 2 or packets[0].get("type") != "hello"
            or packets[0].get("protocol") != protocol or packets[-1].get("type") != "disconnected"
            or any(p.get("type") not in {"hello", "ready", "disconnected"} for p in packets)):
        raise RuntimeError("Android helper handshake/disconnect failed")


def main():
    output = Path(sys.argv[1]).resolve()
    for name in ("workbench.xwd", "workbench.png", "window-tree.txt", "wm-class.txt", "android-runtime.txt"):
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
        android_probe(application)
        debs = list(dist.glob("NyaTerm_*_linux_x64_glibc231.deb"))
        if debs:
            if len(debs) != 1:
                raise RuntimeError("expected one focal deb")
            installed = Path(directory) / "installed"
            subprocess.run(["dpkg-deb", "--extract", str(debs[0]), str(installed)], check=True)
            android_probe(installed / "opt/nyaterm-compat/nyaterm")
        (output / "android-runtime.txt").write_text("PASS: packaged ADB, FFmpeg loader, helper IPC and checksums\n")
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
