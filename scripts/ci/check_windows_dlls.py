#!/usr/bin/env python3
"""The Windows app needs no DLL that Windows does not ship.

    python3 scripts/ci/check_windows_dlls.py <openalgo-desktop.exe> [...]

Reads the executable's import and delay-import tables (the DLLs Windows
loads before the app's first line runs) and refuses any DLL outside
SYSTEM_DLLS and the api-ms-win-* / ext-ms-win-* API sets. A DLL missing on
the trader's PC stops the app before it can show anything; 1.0.0 needed
MSVCP140.dll, Microsoft's Visual C++ runtime, and did not start on a PC
without it ("The code execution cannot proceed because MSVCP140.dll was not
found").

The C++ of the bundled DuckDB links that runtime unless the C runtime is
static: .cargo/config.toml sets `+crt-static` for MSVC builds, a RUSTFLAGS
variable replaces that setting (CI's Windows jobs repeat it), and Tauri's
own static VC runtime covers only vcruntime140.dll. GitHub's Windows
runners have the Visual C++ runtime installed, so the app starting there
proves nothing; this check does.

The install smoke runs it on the installed app of every CI run (CI's
installers are the ones a release ships), a release rebuild on the binary it
bundles, and the Windows native E2E on its test build.
A new import that is part of Windows belongs in SYSTEM_DLLS; anything else
is linked statically or not at all.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
from check_no_e2e_build import read, u32  # noqa: E402

# In System32 on every supported Windows: 10 and 11 (x64), and Server 2022
# and 2025 with Desktop Experience. Lower case.
SYSTEM_DLLS = frozenset(
    {
        "advapi32.dll",
        "bcrypt.dll",
        "bcryptprimitives.dll",
        "combase.dll",
        "comctl32.dll",
        "crypt32.dll",
        "dbghelp.dll",
        "dwmapi.dll",
        "gdi32.dll",
        "imm32.dll",
        "iphlpapi.dll",
        "kernel32.dll",
        "kernelbase.dll",
        "ntdll.dll",
        "ole32.dll",
        "oleaut32.dll",
        "pdh.dll",
        "powrprof.dll",
        "psapi.dll",
        "rstrtmgr.dll",
        "shcore.dll",
        "shell32.dll",
        "shlwapi.dll",
        "user32.dll",
        "uxtheme.dll",
        "version.dll",
        "ws2_32.dll",
    }
)
# API sets, which Windows resolves itself; api-ms-win-crt-* is the Universal
# C runtime, part of Windows 10 and later.
SYSTEM_PREFIXES = ("api-ms-win-", "ext-ms-win-")


def imported_dlls(data: bytes) -> list[str]:
    """The DLL names in a PE file's import and delay-import tables."""
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError("not a Windows executable (no MZ header)")
    pe = u32(data, 0x3C)
    if data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("not a Windows executable (no PE header)")
    sections = int.from_bytes(data[pe + 6 : pe + 8], "little")
    opt_size = int.from_bytes(data[pe + 20 : pe + 22], "little")
    opt = pe + 24
    magic = int.from_bytes(data[opt : opt + 2], "little")
    if magic == 0x20B:
        image_base, directories = int.from_bytes(data[opt + 24 : opt + 32], "little"), opt + 112
    elif magic == 0x10B:
        image_base, directories = u32(data, opt + 28), opt + 96
    else:
        raise ValueError(f"unknown optional header {magic:#x}")
    count = u32(data, directories - 4)
    table = [
        (u32(data, at + 12), max(u32(data, at + 8), u32(data, at + 16)), u32(data, at + 20))
        for at in range(opt + opt_size, opt + opt_size + 40 * sections, 40)
    ]

    def offset(rva: int) -> int:
        for start, size, raw in table:
            if start <= rva < start + size:
                return raw + rva - start
        raise ValueError(f"address {rva:#x} is in no section")

    def name(rva: int) -> str:
        at = offset(rva)
        return data[at : data.index(b"\0", at)].decode("ascii")

    def directory(index: int) -> int:
        return u32(data, directories + 8 * index) if index < count else 0

    names = []
    at = offset(directory(1)) if directory(1) else None  # IMAGE_DIRECTORY_ENTRY_IMPORT
    while at is not None and any(data[at : at + 20]):
        names.append(name(u32(data, at + 12)))
        at += 20
    at = offset(directory(13)) if directory(13) else None  # IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT
    while at is not None and u32(data, at + 4):
        attributes, rva = u32(data, at), u32(data, at + 4)
        names.append(name(rva if attributes & 1 else rva - image_base))
        at += 32
    return names


def not_shipped(names: list[str]) -> list[str]:
    return sorted(
        {n for n in names if n.lower() not in SYSTEM_DLLS and not n.lower().startswith(SYSTEM_PREFIXES)},
        key=str.lower,
    )


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    failed = False
    for path in argv:
        names = imported_dlls(read(path))
        print(f"{path}: {len(names)} DLLs: {', '.join(sorted(names, key=str.lower))}")
        for dll in not_shipped(names):
            failed = True
            print(f"::error title=Windows DLLs::{path} needs {dll}, which Windows does not ship")
    if not failed:
        print("OK: every DLL the app needs is part of Windows.")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
