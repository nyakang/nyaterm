#!/usr/bin/env python3
"""Stage a relocatable private OpenGL runtime, with third-party notices."""
import json
import os
from pathlib import Path
import shutil
import sys


def main():
    source, output = map(Path, sys.argv[1:])
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    # Copy only runtime libraries/data, not a package manager, compilers or headers.
    for name in ("lib", "share"):
        shutil.copytree(source / name, output / name, symlinks=True)
    for path in (output / "lib").rglob("*.a"):
        path.unlink()
    records = []
    for record in sorted((source / "conda-meta").glob("*.json")):
        data = json.loads(record.read_text())
        name = data["name"]
        records.append({key: data.get(key) for key in ("name", "version", "build", "url", "sha256", "license")})
        notices = Path(data["link"]["source"]) / "info" / "licenses"
        destination = output / "licenses" / name
        if notices.exists():
            shutil.copytree(notices, destination)
        else:
            installed_notices = [source / f for f in data["files"] if f.startswith("share/licenses/")]
            if not installed_notices:
                raise RuntimeError("missing third-party notices: " + name)
            destination.mkdir(parents=True)
            for notice in installed_notices:
                shutil.copy2(notice, destination / notice.name)
            if name in ("libgcc", "libstdcxx", "libgomp"):
                shutil.copy2("/usr/share/common-licenses/GPL-3", destination / "GPL-3")
        recipe = Path(data["link"]["source"]) / "info" / "recipe"
        if recipe.exists():
            shutil.copytree(recipe, destination / "recipe", symlinks=True)
    (output / "packages.json").write_text(json.dumps(records, indent=2) + "\n")
    # No copied absolute symlinks may refer back to the build machine.
    for path in output.rglob("*"):
        if path.is_symlink() and Path(os.readlink(path)).is_absolute():
            raise RuntimeError("non-relocatable runtime symlink: " + str(path))


if __name__ == "__main__":
    main()
