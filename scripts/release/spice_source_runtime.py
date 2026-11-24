"""Build the macOS runtime from pinned sources for the application's OS floor.

Homebrew supplies build tools only. Runtime pkg-config searches stay inside the
private prefix; every resulting dylib is checked again before packaging.
The same recipe can be exercised on Linux to validate its portable build steps.
"""
from __future__ import annotations

import os
from pathlib import Path
import platform

from spice_unix import output, run, source
from native_headers import MACOS_MINIMUM_VERSION


RUNTIME_SOURCES = (
    "libffi", "pcre2", "gettext", "glib", "openssl", "cyrus-sasl", "pixman",
    "jpeg-turbo", "json-glib", "opus", "orc", "lz4", "libvpx", "openh264",
    "gstreamer", "gst-plugins-base", "gst-plugins-good", "gst-plugins-bad",
)


def build(directory: Path, prefix: Path, *, macos: bool) -> dict:
    minimum = ".".join(map(str, MACOS_MINIMUM_VERSION[:2]))
    env = os.environ.copy()
    for name in ("CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBRARY_PATH",
                 "PKG_CONFIG_SYSROOT_DIR", "CMAKE_PREFIX_PATH", "CMAKE_TOOLCHAIN_FILE"):
        env.pop(name, None)
    env.update(
        PKG_CONFIG_LIBDIR=os.pathsep.join(str(prefix / p) for p in ("lib/pkgconfig", "share/pkgconfig")),
        PKG_CONFIG_PATH="", CMAKE_PREFIX_PATH=str(prefix),
        CPPFLAGS="-I" + str(prefix / "include"),
        LDFLAGS="-L" + str(prefix / "lib") + " -Wl,-rpath," + str(prefix / "lib"),
        CFLAGS="-O2", CXXFLAGS="-O2", PATH=str(prefix / "bin") + os.pathsep + env["PATH"],
    )
    if macos:
        parser_tools = [str(Path(output(["brew", "--prefix", name])) / "bin") for name in ("bison", "flex")]
        env["PATH"] = os.pathsep.join([str(prefix / "bin"), *parser_tools, os.environ["PATH"]])
        env.update(MACOSX_DEPLOYMENT_TARGET=minimum, SDKROOT=output(["xcrun", "--sdk", "macosx", "--show-sdk-path"]),
                   CC=output(["xcrun", "--find", "clang"]), CXX=output(["xcrun", "--find", "clang++"]))
        for name in ("CFLAGS", "CXXFLAGS", "LDFLAGS"):
            env[name] += " -mmacosx-version-min=" + minimum
        for name in ("CFLAGS", "CXXFLAGS"):
            env[name] += " -Werror=unguarded-availability-new"
    else:
        env["LD_LIBRARY_PATH"] = str(prefix / "lib")
    jobs = str(min(os.cpu_count() or 2, 8))

    def make(src, *targets):
        run(["make", "-j", jobs, *targets], cwd=src, env=env)

    def configure(name, *options):
        src = source(name, directory)
        run([src / "configure", "--prefix=" + str(prefix), "--libdir=" + str(prefix / "lib"),
             "--disable-static", "--enable-shared", *options], cwd=src, env=env)
        make(src)
        make(src, "install")

    def meson(name, *options, subdir=""):
        src = source(name, directory) / subdir
        dest = directory / (name + "-build")
        run(["meson", "setup", dest, src, "--prefix=" + str(prefix), "--libdir=lib",
             "--buildtype=release", "--wrap-mode=nodownload", "-Ddefault_library=shared", *options], env=env)
        run(["meson", "compile", "-C", dest, "-j", jobs], env=env)
        run(["meson", "install", "-C", dest], env=env)

    def cmake(name, *options, subdir=""):
        src = source(name, directory) / subdir
        dest = directory / (name + "-build")
        settings = ["-DCMAKE_INSTALL_PREFIX=" + str(prefix), "-DCMAKE_INSTALL_LIBDIR=lib",
                    "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=ON"]
        if macos:
            settings += ["-DCMAKE_OSX_DEPLOYMENT_TARGET=" + minimum, "-DCMAKE_OSX_SYSROOT=" + env["SDKROOT"]]
        run(["cmake", "-S", src, "-B", dest, "-G", "Ninja", *settings, *options], env=env)
        run(["cmake", "--build", dest, "--parallel", jobs], env=env)
        run(["cmake", "--install", dest], env=env)

    configure("libffi", "--disable-docs")
    cmake("pcre2", "-DBUILD_STATIC_LIBS=OFF", "-DPCRE2_BUILD_TESTS=OFF", "-DPCRE2_BUILD_PCRE2GREP=OFF",
          "-DPCRE2_SUPPORT_JIT=ON")
    gettext = source("gettext", directory)
    gettext_runtime = gettext / "gettext-runtime"
    run([gettext_runtime / "configure", "--prefix=" + str(prefix), "--libdir=" + str(prefix / "lib"),
         "--disable-static", "--enable-shared", "--disable-java", "--disable-csharp",
         "--disable-libasprintf", "--without-libiconv-prefix"], cwd=gettext_runtime, env=env)
    make(gettext_runtime / "intl")
    make(gettext_runtime / "intl", "install")
    meson("glib", "-Dtests=false", "-Dinstalled_tests=false", "-Ddocumentation=false", "-Dxattr=false",
          *["-D" + name + "=disabled" for name in ("introspection", "man-pages", "selinux", "libmount",
                                                 "libelf", "nls", "dtrace", "systemtap", "sysprof")])
    openssl = source("openssl", directory)
    arm = platform.machine() in ("arm64", "aarch64")
    target = ("darwin64-arm64-cc" if arm else "darwin64-x86_64-cc") if macos else ("linux-aarch64" if arm else "linux-x86_64")
    run([openssl / "Configure", target, "shared", "no-tests", "no-legacy", "no-module",
         "--prefix=" + str(prefix), "--libdir=lib"], cwd=openssl, env=env)
    make(openssl)
    make(openssl, "install_sw")
    configure("cyrus-sasl", "--disable-macos-framework", "--with-openssl=" + str(prefix), "--disable-sample",
              "--with-dblib=none", "--without-pam", "--without-ldap", "--disable-sql", "--disable-ldapdb",
              "--disable-gssapi", "--disable-krb4", "--disable-otp", "--disable-ntlm", "--disable-srp",
              "--enable-scram", "--enable-plain", "--enable-anon", "--enable-cram", "--enable-digest")
    meson("pixman", "-Dtests=disabled", "-Ddemos=disabled", "-Dgtk=disabled", "-Dlibpng=disabled")
    cmake("jpeg-turbo", "-DENABLE_STATIC=OFF", "-DENABLE_SHARED=ON", "-DWITH_TURBOJPEG=OFF",
          "-DWITH_TOOLS=OFF", "-DWITH_TESTS=OFF")
    meson("json-glib", "-Dintrospection=disabled", "-Ddocumentation=disabled", "-Dtests=false",
          "-Dinstalled_tests=false", "-Dconformance=false", "-Dnls=disabled")
    cmake("opus", "-DOPUS_BUILD_SHARED_LIBRARY=ON", "-DOPUS_BUILD_TESTING=OFF", "-DOPUS_BUILD_PROGRAMS=OFF")
    meson("orc", "-Dtests=disabled", "-Dexamples=disabled", "-Dbenchmarks=disabled", "-Dhotdoc=disabled")
    cmake("lz4", "-DLZ4_BUILD_CLI=OFF", "-DLZ4_BUILD_LEGACY_LZ4C=OFF", subdir="build/cmake")
    vpx = source("libvpx", directory)
    vpx_options = ["--prefix=" + str(prefix), "--enable-shared", "--disable-static", "--disable-examples",
                   "--disable-tools", "--disable-docs", "--disable-unit-tests", "--enable-vp9-highbitdepth"]
    if macos:
        arch = "arm64" if platform.machine() in ("arm64", "aarch64") else "x86_64"
        # libvpx reads LDFLAGS from env; configure has no --extra-ldflags option.
        vpx_options += ["--target=" + arch + "-darwin20-gcc", "--extra-cflags=-mmacosx-version-min=" + minimum]
    if platform.machine() in ("x86_64", "AMD64"):
        vpx_options += ["--as=nasm"]
    run([vpx / "configure", *vpx_options], cwd=vpx, env=env)
    make(vpx)
    make(vpx, "install")
    h264 = source("openh264", directory)
    make(h264, "BUILDTYPE=Release", "PREFIX=" + str(prefix), "install-shared")
    gst_options = ["-Dauto_features=disabled", "-Dtests=disabled", "-Dexamples=disabled", "-Dnls=disabled"]
    meson("gstreamer", *gst_options, "-Dintrospection=disabled", "-Dtools=disabled", "-Dbenchmarks=disabled")
    meson("gst-plugins-base", *gst_options, "-Dintrospection=disabled", "-Dtools=disabled", "-Dorc=enabled",
          *["-D" + name + "=enabled" for name in ("app", "playback", "typefind", "videoconvertscale")])
    meson("gst-plugins-good", *gst_options, "-Dorc=enabled", "-Djpeg=enabled", "-Dvpx=enabled")
    meson("gst-plugins-bad", *gst_options, "-Dintrospection=disabled", "-Dtools=disabled", "-Dorc=enabled",
          "-Dopenh264=enabled", "-Dvideoparsers=enabled")
    return env
