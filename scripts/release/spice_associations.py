"""OS document declarations for user-selected virt-viewer connection files.

Register as an alternate handler; never replace the user's default viewer.
Stable and preview packages own distinct registration names.
"""

from pathlib import Path
import xml.etree.ElementTree as ET


MIME_TYPE = "application/x-virt-viewer"
MACOS_TYPE = "org.virt-manager.virt-viewer.connection"


def macos_declarations() -> dict:
    return {
        "CFBundleDocumentTypes": [{
            "CFBundleTypeName": "SPICE connection",
            "CFBundleTypeRole": "Viewer",
            "LSHandlerRank": "Alternate",
            "LSItemContentTypes": [MACOS_TYPE],
        }],
        "UTImportedTypeDeclarations": [{
            "UTTypeIdentifier": MACOS_TYPE,
            "UTTypeDescription": "Virt-viewer connection",
            "UTTypeConformsTo": ["public.data", "public.content"],
            "UTTypeTagSpecification": {
                "public.filename-extension": ["vv"],
                "public.mime-type": MIME_TYPE,
            },
        }],
    }


def write_linux_mime(root: Path, desktop_id: str) -> None:
    path = root / "usr/share/mime/packages" / (desktop_id + "-spice.xml")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">\n'
        f'  <mime-type type="{MIME_TYPE}">\n'
        '    <comment>Virt-viewer connection</comment>\n'
        '    <glob pattern="*.vv"/>\n'
        '  </mime-type>\n'
        '</mime-info>\n', encoding="utf-8",
    )
    path.chmod(0o644)


def verify_linux_mime(data: bytes) -> None:
    root = ET.fromstring(data)
    ns = "{http://www.freedesktop.org/standards/shared-mime-info}"
    mime = root.find(ns + "mime-type")
    if root.tag != ns + "mime-info" or mime is None or mime.get("type") != MIME_TYPE:
        raise RuntimeError("missing SPICE connection MIME declaration")
    if not any(glob.get("pattern") == "*.vv" for glob in mime.findall(ns + "glob")):
        raise RuntimeError("missing SPICE connection file extension")


def linux_cache_commands() -> str:
    return (
        "if command -v update-mime-database >/dev/null 2>&1; then\n"
        "    update-mime-database /usr/share/mime || true\nfi\n"
        "if command -v update-desktop-database >/dev/null 2>&1; then\n"
        "    update-desktop-database /usr/share/applications || true\nfi\n"
    )


def write_linux_postrm(path: Path) -> None:
    path.write_text(
        '#!/bin/sh\nset -e\ncase "${1:-}" in\nremove|purge)\n'
        + linux_cache_commands() + ';;\nesac\nexit 0\n', encoding="utf-8",
    )
    path.chmod(0o755)
