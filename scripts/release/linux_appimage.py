"""Bundle the OpenSSL ABI needed by an AppImage on the native Linux builder."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path

from native_headers import elf_machine


OPENSSL_SONAME = re.compile(r"lib(?:ssl|crypto)\.so\.[0-9.]+")
# Linux release builders install libssl-dev alongside its matching runtime.
OPENSSL_COPYRIGHT = Path("/usr/share/doc/libssl-dev/copyright")


def loader_environment() -> dict[str, str]:
    # Validate the package's own search paths, independently of the build shell.
    return {
        key: value for key, value in os.environ.items()
        if key not in {"LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"}
    } | {"LC_ALL": "C"}


def dependencies(binary: Path) -> dict[str, Path]:
    output = subprocess.check_output(
        ["ldd", str(binary.resolve())], text=True, env=loader_environment()
    )
    resolved = {}
    for line in output.splitlines():
        if "=>" not in line:
            continue
        soname, location = (part.strip() for part in line.split("=>", 1))
        location = location.rsplit(" (", 1)[0].strip()
        if not Path(location).is_absolute() or not Path(location).is_file():
            raise RuntimeError(f"{binary.name}: unresolved dependency {soname}: {location}")
        resolved[soname] = Path(location)
    return resolved


def prepend_runpath(binary: Path, relative: str) -> None:
    previous = subprocess.check_output(
        ["patchelf", "--print-rpath", str(binary)], text=True
    ).strip()
    entries = list(dict.fromkeys([relative, *filter(None, previous.split(":"))]))
    subprocess.run(
        ["patchelf", "--set-rpath", ":".join(entries), str(binary)], check=True
    )


def bundle_openssl(appdir: Path, executables: list[Path]) -> None:
    libraries: dict[str, Path] = {}
    for binary in executables:
        for soname, source in dependencies(binary).items():
            if not OPENSSL_SONAME.fullmatch(soname):
                continue
            previous = libraries.setdefault(soname, source)
            if previous.resolve() != source.resolve():
                raise RuntimeError(f"conflicting AppImage dependency: {soname}")
    if not libraries:
        return

    destination = appdir / "usr/lib"
    destination.mkdir(parents=True, exist_ok=True)
    for soname, source in sorted(libraries.items()):
        output = destination / soname
        # Copy the contents under the SONAME, never a symlink to the build host.
        shutil.copy2(source, output)
        output.chmod(0o644)
        # DT_RUNPATH is not transitive: libssl must find its sibling libcrypto.
        prepend_runpath(output, "$ORIGIN")
    for binary in executables:
        prepend_runpath(binary, "$ORIGIN/../lib")

    notices = appdir / "usr/share/doc/openssl"
    notices.mkdir(parents=True, exist_ok=True)
    shutil.copy2(OPENSSL_COPYRIGHT, notices / "copyright")
    (notices / "copyright").chmod(0o644)


def verify_openssl(appdir: Path, executables: list[Path], machine: int) -> None:
    library_dir = (appdir / "usr/lib").resolve()
    bundled = [
        path for path in library_dir.glob("lib*.so.*")
        if OPENSSL_SONAME.fullmatch(path.name)
    ]
    for library in bundled:
        if library.is_symlink():
            raise RuntimeError(f"AppImage OpenSSL library must be a regular file: {library}")
        with library.open("rb") as stream:
            if elf_machine(stream.read(64)) != machine:
                raise RuntimeError(f"AppImage OpenSSL architecture mismatch: {library.name}")
    required = set()
    for binary in [*executables, *bundled]:
        for soname, source in dependencies(binary).items():
            if OPENSSL_SONAME.fullmatch(soname):
                required.add(soname)
                if source.resolve() != library_dir / soname:
                    raise RuntimeError(
                        f"{binary.name}: AppImage loads {soname} outside its bundle: {source}"
                    )
    if required or bundled:
        copyright_file = appdir / "usr/share/doc/openssl/copyright"
        if not copyright_file.is_file() or not copyright_file.stat().st_size:
            raise RuntimeError("AppImage is missing the OpenSSL copyright notice")
