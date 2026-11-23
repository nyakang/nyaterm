#!/usr/bin/env python3
"""Load a prepared Unix SPICE runtime and instantiate its video pipeline elements.

This is a native-library packaging probe, not a helper/VM interoperability test.
The Rust helper's example probe remains the production ABI verification path.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import prepare_spice


def child(directory):
    macos = sys.platform == "darwin"
    names = ("libglib-2.0.0.dylib", "libgobject-2.0.0.dylib", "libspice-client-glib-2.0.8.dylib", "libgstreamer-1.0.0.dylib") if macos else (
        "libglib-2.0.so.0", "libgobject-2.0.so.0", "libspice-client-glib-2.0.so.8", "libgstreamer-1.0.so.0")
    glib, gobject, spice, gst = [ctypes.CDLL(str(directory / name)) for name in names]
    spice.spice_glib_get_version_string.restype = ctypes.c_char_p
    spice.spice_session_get_type.restype = ctypes.c_size_t
    gobject.g_object_new.argtypes = [ctypes.c_size_t, ctypes.c_char_p]
    gobject.g_object_new.restype = ctypes.c_void_p
    gobject.g_object_unref.argtypes = [ctypes.c_void_p]
    session = gobject.g_object_new(spice.spice_session_get_type(), b"enable-usbredir", ctypes.c_int(0),
                                   ctypes.c_char_p(b"enable-smartcard"), ctypes.c_int(0),
                                   ctypes.c_char_p(b"enable-audio"), ctypes.c_int(0), ctypes.c_void_p())
    if not session:
        raise RuntimeError("SPICE session allocation failed")
    gobject.g_object_unref(session)
    gst.gst_init_check.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
    gst.gst_init_check.restype = ctypes.c_int
    if not gst.gst_init_check(None, None, None):
        raise RuntimeError("GStreamer initialization failed")
    gst.gst_plugin_load_file.argtypes = [ctypes.c_char_p, ctypes.c_void_p]
    gst.gst_plugin_load_file.restype = ctypes.c_void_p
    gst.gst_object_unref.argtypes = [ctypes.c_void_p]
    suffix = ".dylib" if macos else ".so"
    for name in prepare_spice.VIDEO_PLUGINS:
        names = (name,)
        if name == "videoconvertscale" and not (directory / "gstreamer-1.0" / f"libgst{name}{suffix}").is_file():
            names = ("videoconvert", "videoscale")
        for name in names:
            plugin = gst.gst_plugin_load_file(os.fsencode(directory / "gstreamer-1.0" / f"libgst{name}{suffix}"), None)
            if not plugin:
                raise RuntimeError("Unable to load bundled plugin: " + name)
            gst.gst_object_unref(plugin)
    gst.gst_element_factory_make.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
    gst.gst_element_factory_make.restype = ctypes.c_void_p
    gst.gst_object_unref.argtypes = [ctypes.c_void_p]
    names = ["appsrc", "appsink", "decodebin", "videoconvert", "jpegdec", "vp8dec", "vp9dec", "openh264dec", "h264parse", "queue"]
    for name in names:
        element = gst.gst_element_factory_make(name.encode(), None)
        if not element:
            raise RuntimeError("Missing or unloadable video element: " + name)
        gst.gst_object_unref(element)
    print(json.dumps({"spice_version": spice.spice_glib_get_version_string().decode(),
                      "session_created": True, "instantiated_video_elements": names}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--child", action="store_true")
    args = parser.parse_args()
    directory = args.directory.resolve(strict=True)
    if args.child:
        child(directory)
        return
    if sys.platform not in ("darwin", "linux"):
        parser.error("Use cargo run -p nyaterm-spice-helper --example probe -- --require-video on Windows")
    manifest = json.loads((directory / "SOURCE.json").read_text())
    prepare_spice.verify_runtime(directory, manifest["target"])
    env = os.environ.copy()
    for key in ("LD_LIBRARY_PATH", "LD_PRELOAD", "DYLD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES"):
        env.pop(key, None)
    env.update({
        "GST_PLUGIN_PATH": "", "GST_PLUGIN_PATH_1_0": "",
        "GST_PLUGIN_SYSTEM_PATH": "", "GST_PLUGIN_SYSTEM_PATH_1_0": "",
        "GST_REGISTRY_DISABLE": "yes",
        "GST_REGISTRY_FORK": "no", "GST_REGISTRY_UPDATE": "no", "GST_DEBUG": "0",
        "SASL_PATH": str(directory / "sasl2"), "GIO_MODULE_DIR": str(directory / "gio-modules"),
        "GIO_EXTRA_MODULES": "", "OPENSSL_CONF": str(directory / "openssl.cnf"),
    })
    with tempfile.TemporaryDirectory() as cwd:
        subprocess.run([sys.executable, str(Path(__file__).resolve()), str(directory), "--child"],
                       env=env, cwd=cwd, check=True, timeout=30)
    prepare_spice.verify_runtime(directory, manifest["target"])


if __name__ == "__main__":
    main()
