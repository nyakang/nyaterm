"""Native architecture readers shared by release and Python 3.8 compatibility audits."""

import struct

MACOS_MINIMUM_VERSION = (12, 0, 0)


def pe_header_offset(data: bytes) -> int:
    if len(data) < 64 or data[:2] != b"MZ":
        raise RuntimeError("Windows executable is missing the MZ header")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if len(data) < pe_offset + 6 or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise RuntimeError("Windows executable is missing the PE header")
    return pe_offset


def pe_machine(data: bytes) -> int:
    return struct.unpack_from("<H", data, pe_header_offset(data) + 4)[0]


def elf_machine(data: bytes) -> int:
    if len(data) < 20 or data[:4] != b"\x7fELF":
        raise RuntimeError("Linux executable is missing the ELF header")
    byte_order = "little" if data[5] == 1 else "big"
    return int.from_bytes(data[18:20], byte_order)


def macho_cpu_type(data: bytes) -> int:
    if len(data) < 8:
        raise RuntimeError("macOS executable is too short")
    magic = data[:4]
    if magic == b"\xcf\xfa\xed\xfe":
        byte_order = "little"
    elif magic == b"\xfe\xed\xfa\xcf":
        byte_order = "big"
    else:
        raise RuntimeError("macOS executable is not a 64-bit Mach-O file")
    return int.from_bytes(data[4:8], byte_order)


def macho_minimum_os(data: bytes) -> tuple:
    """Read the deployment target, not the SDK version used to build a dylib."""
    macho_cpu_type(data)
    if len(data) < 32:
        raise RuntimeError("Truncated Mach-O header")
    endian = "<" if data[:4] == b"\xcf\xfa\xed\xfe" else ">"
    count, size = struct.unpack_from(endian + "II", data, 16)
    end, offset = 32 + size, 32
    if count > 4096 or end > len(data):
        raise RuntimeError("Invalid Mach-O load commands")
    versions = []
    for _ in range(count):
        if offset + 8 > end:
            raise RuntimeError("Truncated Mach-O load command")
        command, length = struct.unpack_from(endian + "II", data, offset)
        if length < 8 or length % 4 or offset + length > end:
            raise RuntimeError("Invalid Mach-O load command size")
        if command == 0x24:  # LC_VERSION_MIN_MACOSX
            if length < 16:
                raise RuntimeError("Truncated Mach-O deployment version")
            versions.append(struct.unpack_from(endian + "I", data, offset + 8)[0])
        elif command == 0x32:  # LC_BUILD_VERSION
            if length < 24:
                raise RuntimeError("Truncated Mach-O build version")
            platform, version = struct.unpack_from(endian + "II", data, offset + 8)
            if platform != 1:
                raise RuntimeError("SPICE library is not built for macOS")
            tools = struct.unpack_from(endian + "I", data, offset + 20)[0]
            if length != 24 + tools * 8:
                raise RuntimeError("Invalid Mach-O build tool inventory")
            versions.append(version)
        offset += length
    if offset != end or not versions or len(set(versions)) != 1:
        raise RuntimeError("Missing or inconsistent Mach-O deployment version")
    version = versions[0]
    if version < (10 << 16):
        raise RuntimeError("Invalid Mach-O macOS deployment version")
    return (version >> 16, (version >> 8) & 0xff, version & 0xff)


def verify_macos_minimum_os(data: bytes) -> None:
    actual = macho_minimum_os(data)
    if actual > MACOS_MINIMUM_VERSION:
        raise RuntimeError("SPICE library requires macOS " + ".".join(map(str, actual))
                           + "; package baseline is macOS " + ".".join(map(str, MACOS_MINIMUM_VERSION[:2])))
