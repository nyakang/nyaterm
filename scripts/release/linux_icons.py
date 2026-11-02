"""Shared Linux icon staging, also usable by the Python 3.8 focal builder."""

from pathlib import Path
import shutil


ICON_SIZES = ("32x32", "64x64", "128x128", "256x256", "512x512")


def normalize_permissions(root: Path) -> None:
    # Staging must be independent of both checkout permissions and the builder's
    # umask. Only operate on the newly created package tree; never follow links.
    root.chmod(0o755)
    for path in root.rglob("*"):
        if not path.is_symlink():
            path.chmod(0o755 if path.is_dir() or path.stat().st_mode & 0o111 else 0o644)


def copy_icons(root: Path, source: Path, desktop_id: str) -> None:
    for size in ICON_SIZES:
        destination = root / "usr/share/icons/hicolor" / size / "apps" / (desktop_id + ".png")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / (size + ".png"), destination)
        # copy2 preserves a restrictive checkout mode (for example 0600).
        destination.chmod(0o644)


def write_postinst(path: Path, desktop_id: str) -> None:
    # Only the package's explicit regular icon files may be repaired. Missing
    # sizes and repeated configure calls are safe; unrelated icons are untouched.
    icons = " ".join("/usr/share/icons/hicolor/{}/apps/{}.png".format(size, desktop_id)
                     for size in ICON_SIZES)
    path.write_text(
        "#!/bin/sh\nset -e\n\n"
        "if [ \"${1:-}\" = configure ]; then\n"
        "    for icon in " + icons + "; do\n"
        "        if [ -f \"$icon\" ] && [ ! -L \"$icon\" ]; then\n"
        "            chmod 644 \"$icon\"\n"
        "        fi\n"
        "    done\n"
        "    if command -v gtk-update-icon-cache >/dev/null 2>&1; then\n"
        "        gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true\n"
        "    fi\n"
        "fi\n\nexit 0\n",
        encoding="utf-8",
    )
    path.chmod(0o755)
