#!/usr/bin/env python3
"""Compare Android helpers with synthetic H.264 only (no ADB/device/GPU).

Windows process counters use only the standard library. Both executables must use
identical build profiles and FFmpeg DLLs. No decoded image or input is retained.
"""
from __future__ import annotations
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import re
import socket
import statistics
import struct
import subprocess
import threading
import time


def exact(stream, count):
    chunks = bytearray()
    while len(chunks) < count:
        part = stream.read(count - len(chunks))
        if not part:
            raise EOFError("helper ended")
        chunks.extend(part)
    return chunks


class Counters:
    def __init__(self, pid):
        if os.name != "nt":
            raise RuntimeError("process counters currently require Windows")
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD]
        self.handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())

    def read(self):
        times = [wintypes.FILETIME() for _ in range(4)]
        if not self.kernel.GetProcessTimes(self.handle, *(ctypes.byref(t) for t in times)):
            raise ctypes.WinError(ctypes.get_last_error())
        cpu = sum((t.dwHighDateTime << 32) | t.dwLowDateTime for t in times[2:]) / 1e7
        class Memory(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [(name, ctypes.c_size_t) for name in
                ["peak_rss", "rss", "peak_paged", "paged", "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile"]]
        memory = Memory()
        memory.cb = ctypes.sizeof(memory)
        if not self.psapi.GetProcessMemoryInfo(self.handle, ctypes.byref(memory), memory.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return cpu, memory.peak_rss / (1024 * 1024)

    def close(self):
        self.kernel.CloseHandle(self.handle)


def access_units(ffmpeg):
    # AUD marks complete access units; one cyclic GOP provides identical input.
    data = subprocess.check_output([ffmpeg, "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
        "testsrc2=size=1280x720:rate=120", "-frames:v", "120", "-c:v", "libx264", "-preset", "ultrafast",
        "-tune", "zerolatency", "-x264-params", "aud=1:keyint=120:repeat-headers=1", "-f", "h264", "pipe:1"])
    offsets = [m.start() for m in re.finditer(b"\x00\x00\x00\x01\x09", data)]
    if len(offsets) != 120:
        raise RuntimeError("unexpected synthetic access unit count")
    return [data[a:b] for a, b in zip(offsets, offsets[1:] + [len(data)])]


class Run:
    def __init__(self, helper, units, fps, hidden):
        self.lock = threading.Lock()
        self.stopped = threading.Event()
        self.pause = threading.Event()
        self.ready = threading.Event()
        self.frame_event = threading.Event()
        self.sampling = False
        self.frames = 0
        self.fed_frames = 0
        self.latencies = []
        self.errors = []
        self.sockets = []
        self.started = time.perf_counter()
        self.child = None
        self.counters = None
        self.reader = None
        self.sender = None
        self.listener = None
        try:
            self.listener = socket.socket()
            self.listener.bind(("127.0.0.1", 0))
            self.listener.listen(2)
            self.listener.settimeout(10)
            self.child = subprocess.Popen([str(helper)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            self.counters = Counters(self.child.pid)
            hello = self.packet()
            self.version = hello[1]["protocol"]
            if self.version not in (2, 3):
                raise RuntimeError("unsupported helper protocol")
            self.control({"type": "connect", "binding": {"server": "127.0.0.1:5037", "serial": "synthetic", "transport_id": None, "generation": 1},
                "endpoint": "127.0.0.1:" + str(self.listener.getsockname()[1]), "settings": {"max_size": 1280, "max_fps": fps, "bit_rate": 8000000}})
            self.reader = threading.Thread(target=self.read, daemon=True)
            self.reader.start()
            video, _ = self.listener.accept()
            self.sockets.append(video)
            video.settimeout(2)
            video.sendall(b"\0" + b"\0"*64 + b"h264" + struct.pack(">III", 0x80000000, 1280, 720))
            control, _ = self.listener.accept()
            self.sockets.append(control)
            self.listener.close()
            if not self.ready.wait(5):
                raise RuntimeError("helper not ready")
            if hidden:
                self.control({"type": "presentation", "visible": False})
            self.sender = threading.Thread(target=self.feed, args=(video, units, fps), daemon=True)
            self.sender.start()
        except BaseException:
            self.close()
            raise

    def control(self, value):
        data = json.dumps(value).encode()
        with self.lock:
            self.child.stdin.write(b"\1" + struct.pack("<I", len(data)) + data)
            self.child.stdin.flush()

    def packet(self):
        kind, length = struct.unpack("<BI", exact(self.child.stdout, 5))
        if length > 16*1024*1024 + 52:
            raise RuntimeError("helper packet limit")
        data = exact(self.child.stdout, length)
        return kind, json.loads(data) if kind == 1 else data

    def read(self):
        try:
            while not self.stopped.is_set():
                kind, data = self.packet()
                if kind == 1:
                    if data["type"] == "ready":
                        self.ready.set()
                    elif data["type"] == "fatal":
                        raise RuntimeError(data["code"])
                else:
                    sequence, _, _, pts = struct.unpack_from("<QQQQ", data)
                    if self.sampling:
                        self.frames += 1
                        self.latencies.append((time.perf_counter_ns() // 1000 - pts) / 1000)
                    self.frame_event.set()
                    self.control({"type": "frame_ack", "sequence": sequence})
        except (EOFError, OSError, RuntimeError) as error:
            if not self.stopped.is_set():
                self.errors.append(str(error))

    def feed(self, video, units, fps):
        index = 0
        next_tick = time.perf_counter()
        try:
            while not self.stopped.is_set():
                if self.pause.is_set():
                    self.stopped.wait(0.005)
                    next_tick = time.perf_counter()
                    continue
                pts = time.perf_counter_ns() // 1000
                frame = units[index % len(units)]
                video.sendall(struct.pack(">QI", pts, len(frame)) + frame)
                index += 1
                self.fed_frames += 1
                next_tick += 1 / fps
                self.stopped.wait(max(0, next_tick - time.perf_counter()))
        except OSError:
            if not self.stopped.is_set():
                self.errors.append("synthetic sender failed")

    def resume_ms(self):
        self.pause.set()
        self.control({"type": "presentation", "visible": False})
        time.sleep(0.1)  # Drain an in-flight conversion; no further media is sent.
        self.frame_event.clear()
        started = time.perf_counter()
        self.control({"type": "presentation", "visible": True})
        if not self.frame_event.wait(2):
            raise RuntimeError("cached static frame did not resume")
        return (time.perf_counter() - started) * 1000

    def close(self):
        self.stopped.set()
        if self.child is not None:
            try:
                self.control({"type": "disconnect"})
            except (OSError, ValueError):
                pass
        if self.listener is not None:
            self.listener.close()
        for connection in self.sockets:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        if self.child is not None:
            try:
                self.child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
        if self.counters is not None:
            self.counters.close()
            self.counters = None
        for thread in (self.sender, self.reader):
            if thread is not None:
                thread.join(timeout=2)
        if self.child is not None:
            self.child.stdin.close()
            self.child.stdout.close()
            self.child = None


def measure(helper, units, fps, count, hidden, seconds):
    runs = []
    try:
        for _ in range(count):
            runs.append(Run(helper, units, fps, hidden))
        time.sleep(0.5)
        for run in runs:
            run.sampling = True
        start_cpu = sum(run.counters.read()[0] for run in runs)
        start_fed = sum(run.fed_frames for run in runs)
        started = time.perf_counter()
        time.sleep(seconds)
        elapsed = time.perf_counter() - started
        for run in runs:
            run.sampling = False
        cpu = sum(run.counters.read()[0] for run in runs) - start_cpu
        peak = sum(run.counters.read()[1] for run in runs)
        frames = sum(run.frames for run in runs)
        fed = sum(run.fed_frames for run in runs) - start_fed
        latency = [v for run in runs for v in run.latencies]
        resume = [run.resume_ms() for run in runs for _ in range(5)]
        if any(run.errors for run in runs):
            raise RuntimeError("helper failed during benchmark")
        return {"fps_limit": fps, "screens": count, "hidden": hidden, "cpu_one_core_percent": round(cpu / elapsed * 100, 2),
            "peak_rss_mib_sum": round(peak, 2), "fed_frames": fed, "fed_fps_sum": round(fed / elapsed, 2), "presented_frames": frames,
            "presented_fps_sum": round(frames / elapsed, 2),
            "frame_latency_p50_ms": round(statistics.median(latency), 2) if latency else None,
            "frame_latency_p95_ms": round(sorted(latency)[int((len(latency)-1)*0.95)], 2) if latency else None,
            "static_resume_p50_ms": round(statistics.median(resume), 2)}
    finally:
        for run in runs:
            run.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--seconds", type=float, default=4)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    units = access_units(args.ffmpeg)
    results = []
    for fps, count, hidden in [(30, 1, False), (120, 1, False), (120, 3, True)]:
        for label, helper in [("baseline", args.baseline), ("candidate", args.candidate)]:
            result = {"build": label, **measure(helper.resolve(), units, fps, count, hidden, args.seconds)}
            results.append(result)
            print(json.dumps(result), flush=True)
    args.output.write_text(json.dumps({"scope": "synthetic 1280x720 H264 helper only; no capture or GPU", "results": results}, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
