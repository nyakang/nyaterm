"""Build headless spice-gtk and collect native Unix dependency closures.

Sources are pinned; macOS runtime dependencies are built in a private prefix.
Linux distribution dependency versions and source hashes are recorded in the
output. Builds are not claimed to be bit-reproducible across runner toolchains.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import platform
import posixpath
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile

import prepare_spice as common
from native_headers import verify_macos_minimum_os

TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-apple-darwin", "aarch64-apple-darwin")
SYSTEM_ELF = frozenset(("libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", "libresolv.so.2", "libutil.so.1", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"))


def run(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def output(args, **kwargs):
    return subprocess.check_output([str(arg) for arg in args], text=True, **kwargs).strip()


def pkg_config(name, variable):
    return Path(output(["pkg-config", f"--variable={variable}", name]))


def source(name, directory):
    metadata = json.loads(common.SOURCES_PATH.read_text(encoding="utf-8"))["sources"][name]
    archive = common.download(metadata["url"], metadata["url"].rsplit("/", 1)[-1], metadata["sha256"])
    destination = directory / name
    destination.mkdir()
    with tarfile.open(archive) as handle:
        extract_source(handle, destination, license_links=name in ("glib", "json-glib"),
                       internal_links=name == "pixman")
    roots = list(destination.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise ValueError("Expected one SPICE source root")
    return roots[0]


def extract_source(handle, destination, *, license_links=False, internal_links=False):
    """Extract only regular sources, also on focal's Python 3.8.

    Validate the complete inventory before writing. Reviewed GLib/json-glib
    license aliases and Pixman's internal aliases may be materialized as ordinary
    files; no filesystem links/devices are made.
    """
    members = handle.getmembers()
    # GLib's two COPYING aliases are materialized as ordinary files from the
    # archive's reviewed license, never as filesystem symlinks.
    aliases = {}
    for member in members:
        common.safe_relative(member.name.rstrip("/"))
        root = member.name.split("/", 1)[0]
        if internal_links and member.issym():
            current = member
            for _ in range(32):
                if not current.issym():
                    break
                target = posixpath.normpath(posixpath.join(posixpath.dirname(current.name), current.linkname))
                common.safe_relative(target)
                if not target.startswith(root + "/"):
                    raise ValueError("SPICE source alias leaves its archive root")
                current = handle.getmember(target)
            if not current.isfile():
                raise ValueError("SPICE source alias must resolve to a regular file")
            aliases[member.name] = current
            continue
        if license_links and member.issym() and (
            (member.name == root + "/COPYING" and member.linkname == "LICENSES/LGPL-2.1-or-later.txt")
            or (member.name == root + "/gmodule/COPYING" and member.linkname == "../LICENSES/LGPL-2.1-or-later.txt")
        ):
            license_member = handle.getmember(root + "/LICENSES/LGPL-2.1-or-later.txt")
            if not license_member.isfile():
                raise ValueError("Invalid GLib license member")
            aliases[member.name] = license_member
            continue
        if not (member.isfile() or member.isdir()):
            raise ValueError("Unsupported SPICE source archive member")
    for member in members:
        path = destination / member.name
        if member.isdir():
            path.mkdir(parents=True, exist_ok=True)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            regular = aliases.get(member.name, member)
            with handle.extractfile(regular) as source, path.open("xb") as target:
                shutil.copyfileobj(source, target)
            path.chmod(0o755 if regular.mode & 0o111 else 0o644)
            # Autotools release archives use mtimes to avoid regenerating their
            # checked-in configure scripts with host-specific tool versions.
            os.utime(path, (regular.mtime, regular.mtime))


def git_source(directory):
    metadata = json.loads(common.SOURCES_PATH.read_text(encoding="utf-8"))["unix_git"]
    destination = directory / "spice-gtk"

    def checkout(path, entry, depth):
        run(["git", "init", path])
        run(["git", "-C", path, "-c", "protocol.file.allow=never", "fetch", f"--depth={depth}",
             entry["repository"], entry["revision"]])
        run(["git", "-C", path, "checkout", "--detach", "FETCH_HEAD"])
        if output(["git", "-C", path, "rev-parse", "HEAD"]) != entry["revision"]:
            raise ValueError("SPICE source revision mismatch")

    checkout(destination, metadata, 64)
    run(["git", "-C", destination, "-c", "protocol.file.allow=never", "fetch", "--depth=1",
         metadata["repository"], "tag", metadata["base_tag"]])
    for name, entry in metadata["submodules"].items():
        common.safe_relative(name)
        tracked = output(["git", "-C", destination, "ls-tree", "HEAD", name]).split()
        if len(tracked) < 3 or tracked[:3] != ["160000", "commit", entry["revision"]]:
            raise ValueError("SPICE submodule does not match the locked source tree")
        checkout(destination / name, entry, 1)
    return destination


def build(directory, macos):
    prefix = directory / "prefix"
    env = os.environ.copy()
    search = [str(prefix / "lib/pkgconfig"), str(prefix / "share/pkgconfig")]
    search.append(env.get("PKG_CONFIG_PATH", ""))
    env["PKG_CONFIG_PATH"] = os.pathsep.join(filter(None, search))
    source_runtime = macos or os.environ.get("NYATERM_SPICE_SOURCE_RUNTIME") == "1"
    if source_runtime:
        from spice_source_runtime import build as build_sources
        env = build_sources(directory, prefix, macos=macos)
    elif not any((pkg_config("libsasl2", "libdir") / "sasl2").glob("libscram.so*")):
        build_sasl(directory, prefix, env)
    private_glib = not source_runtime and tuple(map(int, output(["pkg-config", "--modversion", "glib-2.0"]).split("."))) < (2, 68)
    names = ["glib"] if private_glib else []
    for name in [*names, "spice-protocol", "spice-gtk"]:
        src = git_source(directory) if name == "spice-gtk" else source(name, directory)
        dest = directory / (name + "-build")
        options = ["--prefix=" + str(prefix), "--libdir=lib", "--buildtype=release", "--wrap-mode=nodownload"]
        if name == "glib":
            options += ["-Ddefault_library=shared", "-Dtests=false", "-Dinstalled_tests=false",
                        "-Ddocumentation=false", "-Dxattr=false"]
            options += ["-D" + feature + "=disabled" for feature in (
                "introspection", "man-pages", "selinux", "libmount", "libelf", "nls",
                "dtrace", "systemtap", "sysprof",
            )]
        if name == "spice-gtk":
            options += [f"-D{feature}=disabled" for feature in (
                "gtk", "wayland-protocols", "webdav", "usbredir", "libcap-ng", "polkit",
                "introspection", "vapi", "smartcard", "egl", "gtk_doc",
            )]
            options += ["-Ddefault_library=shared", "-Dlz4=enabled", "-Dsasl=enabled", "-Dopus=enabled",
                        "-Dspice-common:tests=false", "-Dspice-common:manual=false"]
            # Keep every coroutine/callback on the GLib owner OS thread.
            # GThread coroutines violate the Rust backend's thread affinity.
            options += ["-Dcoroutine=ucontext"]
        run(["meson", "setup", dest, src, *options], env=env)
        run(["meson", "compile", "-C", dest, "-j", str(min(os.cpu_count() or 2, 8))], env=env)
        run(["meson", "install", "-C", dest], env=env)
    if not source_runtime and not (pkg_config("gstreamer-1.0", "pluginsdir") / "libgstopenh264.so").is_file():
        build_focal_openh264(directory, prefix, env)
    return prefix


def build_focal_openh264(directory, prefix, env):
    """Focal's GStreamer omits OpenH264; build its matching plugin privately."""
    if not output(["pkg-config", "--modversion", "gstreamer-1.0"]).startswith("1.16."):
        raise ValueError("Install the distribution's GStreamer OpenH264 plugin")
    codec = source("openh264", directory)
    run(["make", "-j", str(min(os.cpu_count() or 2, 8)), "BUILDTYPE=Release",
         "PREFIX=" + str(prefix), "install-shared"], cwd=codec, env=env)
    src = source("gst-plugins-bad-focal", directory)
    dest = directory / "gst-plugins-bad-focal-build"
    run(["meson", "setup", dest, src, "--prefix=" + str(prefix), "--libdir=lib",
         "--buildtype=release", "--wrap-mode=nodownload", "-Dauto_features=disabled",
         "-Dopenh264=enabled", "-Dexamples=disabled", "-Dtests=disabled",
         "-Dintrospection=disabled"], env=env)
    run(["meson", "compile", "-C", dest, "-j", str(min(os.cpu_count() or 2, 8))], env=env)
    run(["meson", "install", "-C", dest], env=env)


def build_sasl(directory, prefix, env):
    """Focal omits SCRAM; ship the same reviewed mechanism set on every OS."""
    src = source("cyrus-sasl", directory)
    run([src / "configure", "--prefix=" + str(prefix), "--libdir=" + str(prefix / "lib"),
         "--disable-static", "--enable-shared", "--disable-sample", "--with-dblib=none",
         "--without-pam", "--without-ldap", "--disable-sql", "--disable-ldapdb",
         "--disable-gssapi", "--disable-krb4", "--disable-otp", "--disable-ntlm", "--disable-srp",
         "--enable-scram", "--enable-plain", "--enable-anon", "--enable-cram", "--enable-digest"],
        cwd=src, env=env)
    run(["make", "-j", str(min(os.cpu_count() or 2, 8))], cwd=src, env=env)
    run(["make", "install"], cwd=src, env=env)


def check_architecture(path, target):
    data = path.read_bytes()
    arm = target.startswith("aarch64")
    if target.endswith("darwin"):
        if len(data) < 8 or data[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", data, 4)[0] != (0x0100000C if arm else 0x01000007):
            raise ValueError("Wrong SPICE Mach-O architecture: " + path.name)
        verify_macos_minimum_os(data)
    elif len(data) < 20 or data[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", data, 18)[0] != (183 if arm else 62):
        raise ValueError("Wrong SPICE ELF architecture: " + path.name)


def elf_dependencies(path, prefix=None):
    resolved = {}
    environment = dict(os.environ, LD_LIBRARY_PATH=str(prefix / "lib")) if prefix else None
    for line in output(["ldd", path], env=environment).splitlines():
        match = re.match(r"\s*(\S+) => (\S+)", line)
        if match:
            name, destination = match.groups()
            if destination == "not":
                raise ValueError("Unresolved SPICE ELF dependency: " + name)
            resolved[name] = Path(destination)
    needed = output(["patchelf", "--print-needed", path]).splitlines()
    return [(name, resolved[name]) for name in needed if name not in SYSTEM_ELF]


def macho_dependencies(path):
    lines = output(["otool", "-L", path]).splitlines()[1:]
    identity = output(["otool", "-D", path]).splitlines()[1:]
    dependencies = []
    for line in lines:
        name = line.strip().split(" (", 1)[0]
        if name in identity or name.startswith(("/usr/lib/", "/System/Library/")):
            continue
        if name.startswith("@loader_path/"):
            resolved = path.parent / name[len("@loader_path/"):]
        elif name.startswith("@rpath/"):
            relative = name[len("@rpath/"):]
            candidates = [path.parent / relative]
            load_commands = output(["otool", "-l", path])
            for rpath in re.findall(r"cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset", load_commands):
                candidates.append(Path(rpath.replace("@loader_path", str(path.parent))) / relative)
            resolved = next((p for p in candidates if p.is_file()), None)
            if resolved is None:
                raise ValueError("Unresolved SPICE Mach-O rpath: " + name)
        else:
            resolved = Path(name)
        if not resolved.is_absolute() or not resolved.is_file():
            raise ValueError("Unresolved SPICE Mach-O dependency: " + name)
        dependencies.append((name, resolved))
    return dependencies


def collect_libraries(staging, prefix, target):
    macos = target.endswith("darwin")
    suffix = ".dylib" if macos else ".so"
    core = "libspice-client-glib-2.0.8.dylib" if macos else "libspice-client-glib-2.0.so.8"
    seeds = [(prefix / "lib" / core, core)]
    for package, name in (("glib-2.0", "glib-2.0"), ("gobject-2.0", "gobject-2.0"), ("gio-2.0", "gio-2.0"), ("gstreamer-1.0", "gstreamer-1.0")):
        filename = f"lib{name}.0.dylib" if macos else f"lib{name}.so.0"
        private = prefix / "lib" / filename
        seeds.append((private if private.is_file() else pkg_config(package, "libdir") / filename, filename))
    def plugin_path(filename):
        private = prefix / "lib/gstreamer-1.0" / filename
        return private if private.is_file() else pkg_config("gstreamer-1.0", "pluginsdir") / filename
    for name in common.VIDEO_PLUGINS:
        names = (name,)
        if name == "videoconvertscale" and not plugin_path(f"libgst{name}{suffix}").exists():
            names = ("videoconvert", "videoscale")
        for item in names:
            filename = f"libgst{item}{suffix}"
            seeds.append((plugin_path(filename), "gstreamer-1.0/" + filename))
    sasl = prefix / "lib/sasl2"
    if not sasl.is_dir():
        sasl = pkg_config("libsasl2", "libdir") / "sasl2"
    for name in common.SASL_PLUGINS:
        choices = sorted(p for p in sasl.glob(f"lib{name}.*")
                         if p.is_file() and (p.name.endswith((".so", ".dylib")) or ".so." in p.name))
        if not choices:
            raise ValueError("Missing SASL module: " + name)
        path = choices[0]
        seeds.append((path, "sasl2/" + path.name))
    copied, origins, pending = {}, {}, list(seeds)
    while pending:
        original, name = pending.pop()
        common.safe_relative(name)
        real = original.resolve(strict=True)
        if macos and prefix not in real.parents:
            raise ValueError("SPICE macOS runtime dependency is outside the private source build: " + real.name)
        if name in copied:
            if copied[name] != real:
                raise ValueError("Conflicting native library basenames: " + name)
            continue
        check_architecture(real, target)
        copied[name] = real
        destination = staging / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(real, destination)
        destination.chmod(0o755)
        origins[name] = {"path": str(real), "sha256": common.sha256(real.read_bytes())}
        dependencies = macho_dependencies(real) if macos else elf_dependencies(real, prefix)
        for reference, dependency in dependencies:
            library_name = Path(reference).name
            pending.append((dependency, library_name))
            if macos:
                rewritten = "@loader_path/" + ("../" if "/" in name else "") + library_name
                run(["install_name_tool", "-change", reference, rewritten, destination])
        if macos:
            run(["install_name_tool", "-id", "@rpath/" + destination.name, destination])
        else:
            run(["patchelf", "--set-rpath", "$ORIGIN/.." if "/" in name else "$ORIGIN", destination])
    return origins


def distribution_notices(staging, origins, prefix, macos):
    records = {}
    for entry in origins.values():
        path = Path(entry["path"])
        if prefix in path.parents:
            continue
        if macos:
            keg = next((p for p in path.parents if (p / "INSTALL_RECEIPT.json").is_file()), None)
            if keg is None:
                raise ValueError("Native dependency has no Homebrew provenance: " + path.name)
            key = keg.parent.name + "-" + keg.name
            if key in records:
                continue
            records[key] = json.loads((keg / "INSTALL_RECEIPT.json").read_text())
            notices = [p for p in keg.rglob("*") if p.is_file() and p.name.lower().startswith(("license", "copying", "copyright"))]
        else:
            candidates = [str(path)]
            if str(path).startswith("/usr/lib/"):
                candidates.append(str(path)[len("/usr"):])
            owners = []
            for candidate in candidates:
                result = subprocess.run(["dpkg-query", "-S", candidate], capture_output=True, text=True)
                if result.returncode == 0:
                    owners = result.stdout.splitlines()
                    break
            if not owners:
                raise ValueError("Native dependency has no Debian provenance: " + path.name)
            package = owners[0].split(": ", 1)[0]
            version = output(["dpkg-query", "-W", "-f=${Version}", package])
            key = package.replace(":", "_")
            if key in records:
                continue
            records[key] = {"package": package, "version": version,
                            "source": output(["dpkg-query", "-W", "-f=${source:Package} ${source:Version}", package])}
            notices = [Path("/usr/share/doc") / package.split(":")[0] / "copyright"]
        if not notices:
            raise ValueError("Missing native dependency license: " + key)
        for index, notice in enumerate(notices):
            destination = staging / "licenses/distribution" / key / (str(index) + "-" + notice.name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(notice, destination)
    return records


def prepare(target):
    macos = target.endswith("darwin")
    machine = "aarch64" if platform.machine() in ("arm64", "aarch64") else platform.machine()
    if machine != target.split("-")[0] or (platform.system() == "Darwin") != macos or platform.system() not in ("Linux", "Darwin"):
        raise ValueError("Unix SPICE preparation must run natively on the target platform")
    # Source builds are intentionally refreshed: native dependency versions can
    # change independently of the pinned SPICE source on the CI runner.
    parent = common.STAGING / "unix" / target
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="build-", dir=parent) as temporary:
        work = Path(temporary)
        prefix = build(work, macos)
        staging = work / "spice"
        staging.mkdir()
        origins = collect_libraries(staging, prefix, target)
        if macos:
            # install_name_tool invalidates the input signatures. ARM64 macOS
            # requires valid code signatures even for a local library probe.
            for name in sorted(origins):
                run(["codesign", "--force", "--sign", "-", "--timestamp=none", staging / name])
        packages = distribution_notices(staging, origins, prefix, macos)
        required_sources = ["spice-gtk", "spice-protocol", "ca-certificates"]
        if macos or os.environ.get("NYATERM_SPICE_SOURCE_RUNTIME") == "1":
            from spice_source_runtime import RUNTIME_SOURCES
            required_sources.extend(RUNTIME_SOURCES)
        else:
            if (prefix / "lib/libglib-2.0.so.0").is_file():
                required_sources.append("glib")
            if (prefix / "lib/libsasl2.so").is_file():
                required_sources.append("cyrus-sasl")
            if (prefix / "lib/libopenh264.so").is_file():
                required_sources += ["openh264", "gst-plugins-bad-focal"]
        sources = common.source_notices(staging, {"packages": {}}, required_sources)
        lock = json.loads(common.LOCK_PATH.read_text())["targets"]["x86_64-pc-windows-msvc"]
        ca = lock["files"]["ca-bundle.pem"]
        package = lock["packages"][ca["package"]]
        with common.archive(common.download(package["url"], package["archive"], package["sha256"])) as handle:
            data = handle.extractfile(ca["member"]).read()
        if common.sha256(data) != ca["sha256"]:
            raise ValueError("CA bundle checksum mismatch")
        (staging / "ca-bundle.pem").write_bytes(data)
        (staging / "openssl.cnf").write_text("# Use the builtin default provider.\n")
        (staging / "NOTICE.txt").write_text(
            "NyaTerm SPICE runtime. Native libraries remain dynamically replaceable.\n"
            "SOURCE.json records pinned SPICE sources, distribution versions and file hashes.\n"
            "License and copyright notices are in licenses/.\n"
            "The CA bundle is a release-pinned Mozilla trust snapshot.\n", encoding="utf-8")
        manifest = {"schema": 1, "target": target, "sources": sources, "packages": packages,
                    "native_source": json.loads(common.SOURCES_PATH.read_text(encoding="utf-8"))["unix_git"],
                    "ca_package": package, "origins": origins,
                    "sources_sha256": common.sha256(common.SOURCES_PATH.read_bytes()),
                    "files": {p.relative_to(staging).as_posix(): common.sha256(p.read_bytes())
                              for p in sorted(staging.rglob("*")) if p.is_file()}}
        (staging / "SOURCE.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        destination = parent / common.sha256((staging / "SOURCE.json").read_bytes())[:16]
        staging.rename(destination)
    return destination
