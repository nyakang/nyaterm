"""WiX authoring and read-only Windows Installer database inspection."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path


WIX_VERSION = "6.0.2"
UPGRADE_CODE = "F4B0DCC1-48C8-53C7-A0CF-7F36C5F1B27F"
# This namespace and the relative installed paths are component identity contracts.
IDENTITY_NAMESPACE = uuid.UUID(UPGRADE_CODE)
WIX_NAMESPACE = "http://wixtoolset.org/schemas/v4/wxs"


def guid(identity: str) -> str:
    return str(uuid.uuid5(IDENTITY_NAMESPACE, identity)).upper()


def product_code(version: str, architecture: str) -> str:
    return guid(f"product:{architecture}:{version}")


def identifier(kind: str, relative: str) -> str:
    return kind + uuid.uuid5(IDENTITY_NAMESPACE, relative).hex


def find_wix() -> str:
    tool = shutil.which("wix") or shutil.which("wix.exe")
    if not tool:
        raise RuntimeError(f"WiX {WIX_VERSION} is required to package/verify Stable MSI files")
    version = subprocess.check_output([tool, "--version"], text=True).strip()
    if version.split("+", 1)[0] != WIX_VERSION:
        raise RuntimeError(f"expected WiX {WIX_VERSION}, found {version}")
    return tool


def write_source(staging: Path, output: Path, version: str, architecture: str) -> None:
    """Author only files in the shared installer staging tree, never user data."""
    ET.register_namespace("", WIX_NAMESPACE)

    def element(parent: ET.Element, tag: str, **attributes: str) -> ET.Element:
        return ET.SubElement(parent, f"{{{WIX_NAMESPACE}}}{tag}", attributes)

    root = ET.Element(f"{{{WIX_NAMESPACE}}}Wix")
    package = element(
        root, "Package", Name="NyaTerm", Manufacturer="kang", Version=version,
        ProductCode=product_code(version, architecture), UpgradeCode=UPGRADE_CODE,
        Scope="perMachine", Language="1033", InstallerVersion="500", Compressed="yes",
    )
    element(package, "MajorUpgrade", Schedule="afterInstallInitialize",
            AllowDowngrades="no", AllowSameVersionUpgrades="no",
            DowngradeErrorMessage="The same or a newer version of NyaTerm is already installed.")
    upgrade = element(package, "Upgrade", Id=UPGRADE_CODE)
    element(upgrade, "UpgradeVersion", Minimum=version, Maximum=version,
            IncludeMinimum="yes", IncludeMaximum="yes", OnlyDetect="yes",
            Property="NYATERM_SAME_VERSION_DETECTED")
    element(package, "Launch", Condition="Installed OR NOT NYATERM_SAME_VERSION_DETECTED",
            Message="This version of NyaTerm is already installed.")
    element(package, "MediaTemplate", EmbedCab="yes")
    element(package, "Icon", Id="NyaTermIcon", SourceFile=str((staging / "icon.ico").resolve()))
    element(package, "Property", Id="ARPPRODUCTICON", Value="NyaTermIcon")
    element(package, "Property", Id="ARPNOMODIFY", Value="1")
    program_files = element(package, "StandardDirectory", Id="ProgramFiles64Folder")
    install_dir = element(program_files, "Directory", Id="INSTALLDIR", Name="NyaTerm")
    menu = element(package, "StandardDirectory", Id="ProgramMenuFolder")
    element(menu, "Directory", Id="ApplicationProgramsFolder", Name="NyaTerm")
    element(package, "StandardDirectory", Id="DesktopFolder")
    feature = element(package, "Feature", Id="Application", Title="NyaTerm", Level="1")
    directories = {Path("."): install_dir}
    for source in sorted(staging.rglob("*")):
        if not source.is_file():
            continue
        relative = source.relative_to(staging)
        for parent in reversed(relative.parents):
            if parent not in directories:
                directories[parent] = element(
                    directories[parent.parent], "Directory",
                    Id=identifier("d", parent.as_posix()), Name=parent.name,
                )
        component_id = identifier("c", relative.as_posix())
        component = element(
            directories[relative.parent], "Component", Id=component_id,
            Guid=guid(f"component:{architecture}:{relative.as_posix()}"), Bitness="always64",
        )
        file_id = identifier("f", relative.as_posix())
        element(component, "File", Id=file_id, Source=str(source.resolve()),
                Name=relative.name, KeyPath="yes")
        element(feature, "ComponentRef", Id=component_id)
        if relative.as_posix() == "NyaTerm.exe":
            for shortcut_id, directory in (("StartMenuShortcut", "ApplicationProgramsFolder"),
                                            ("DesktopShortcut", "DesktopFolder")):
                element(component, "Shortcut", Id=shortcut_id, Directory=directory,
                        Name="NyaTerm", Target=f"[#{file_id}]", WorkingDirectory="INSTALLDIR",
                        Icon="NyaTermIcon")
            element(component, "RemoveFolder", Id="RemoveStartMenuFolder",
                    Directory="ApplicationProgramsFolder", On="uninstall")
            # Native helper commands perform ownership-aware registration/cleanup;
            # Registry rows would blindly remove another app's replacement values.
            element(package, "CustomAction", Id="RegisterUrlProtocols", FileRef=file_id,
                    ExeCommand="--register-url-protocols", Execute="deferred", Impersonate="no", Return="check")
            element(package, "CustomAction", Id="UnregisterUrlProtocols", FileRef=file_id,
                    ExeCommand="--unregister-url-protocols", Execute="deferred", Impersonate="no", Return="check")
            sequence = element(package, "InstallExecuteSequence")
            element(sequence, "Custom", Action="RegisterUrlProtocols", After="InstallFiles",
                    Condition='NOT REMOVE~="ALL"')
            element(sequence, "Custom", Action="UnregisterUrlProtocols", Before="RemoveFiles",
                    Condition='REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE')
    ET.indent(root)
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)


TABLE_COLUMNS = {
    "Property": ["Property", "Value"],
    "Directory": ["Directory", "Directory_Parent", "DefaultDir"],
    "Component": ["Component", "ComponentId", "Directory_", "Attributes", "KeyPath"],
    "File": ["File", "Component_", "FileName"],
    "Upgrade": ["UpgradeCode", "VersionMin", "VersionMax", "Attributes", "ActionProperty"],
    "InstallExecuteSequence": ["Action", "Condition", "Sequence"],
    "Shortcut": ["Shortcut", "Directory_", "Name", "Component_", "Target"],
    "Registry": ["Registry", "Root", "Key", "Name", "Value", "Component_"],
    "RemoveFile": ["FileKey", "Component_", "FileName", "DirProperty", "InstallMode"],
    "CustomAction": ["Action", "Type", "Source", "Target"],
    "LaunchCondition": ["Condition", "Description"],
}


def read_database(path: Path) -> dict:
    # OpenDatabase mode 0 is read-only. No Installer.InstallProduct or msiexec.
    script = r"""
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.OpenDatabase($env:NYATERM_VERIFY_MSI, 0)
$columns = $env:NYATERM_MSI_COLUMNS | ConvertFrom-Json
$tables = @{}
$tableView = $database.OpenView('SELECT `Name` FROM `_Tables`')
$tableView.Execute()
$available = @()
while ($record = $tableView.Fetch()) { $available += $record.StringData(1) }
$tableView.Close()
foreach ($entry in $columns.PSObject.Properties) {
    $rows = @()
    if ($available -contains $entry.Name) {
        $names = ($entry.Value | ForEach-Object { '`' + $_ + '`' }) -join ','
        $view = $database.OpenView('SELECT ' + $names + ' FROM `' + $entry.Name + '`')
        $view.Execute()
        while ($record = $view.Fetch()) {
            $row = @{}
            for ($i = 0; $i -lt $entry.Value.Count; $i++) {
                $row[$entry.Value[$i]] = $record.StringData($i + 1)
            }
            $rows += $row
        }
        $view.Close()
    }
    $tables[$entry.Name] = $rows
}
@{ template = $database.SummaryInformation(0).Property(7); tables = $tables } |
    ConvertTo-Json -Depth 6 -Compress
"""
    return json.loads(subprocess.check_output(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script],
        text=True, env={**os.environ, "NYATERM_VERIFY_MSI": str(path.resolve()),
                        "NYATERM_MSI_COLUMNS": json.dumps(TABLE_COLUMNS)},
    ))
