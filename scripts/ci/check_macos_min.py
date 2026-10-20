#!/usr/bin/env python3
"""The macOS apps run on the oldest macOS they claim (PLATFORM-03).

    python3 scripts/ci/check_macos_min.py <OpenAlgo Desktop.app> [...]

The claim is bundle.macOS.minimumSystemVersion in src-tauri/tauri.conf.json,
which README "Supported systems" states (macOS 10.15). For each app:

- Info.plist's LSMinimumSystemVersion equals the claim, so the Finder and
  the installer tell a trader on an older macOS before anything starts;
- every architecture slice of the executable asks for no newer macOS than
  the claim (LC_BUILD_VERSION minos, or LC_VERSION_MIN_MACOSX on older
  linkers), or than 11.0 for Apple Silicon, the first macOS that runs
  there. A native library built for a newer macOS raises the slice's
  minimum, and the app would then refuse to start on the claimed one.

Needs macOS (otool, lipo). install-smoke.yml runs it on the Apple Silicon
and the Intel app of the run's .dmg files: the Intel app is checked here
though no Intel Mac runs it in CI.
"""

from __future__ import annotations

import json
import os
import plistlib
import re
import subprocess
import sys

CONF = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "src-tauri", "tauri.conf.json")
# The first macOS each architecture runs on, where that is newer than any claim.
ARCH_FLOOR = {"arm64": (11, 0)}


def version(text: str) -> tuple[int, ...]:
    return tuple(int(p) for p in text.strip().split("."))


def claim() -> str:
    with open(CONF, encoding="utf-8") as f:
        return json.load(f)["bundle"]["macOS"]["minimumSystemVersion"]


def minos(binary: str, arch: str) -> str | None:
    """The macOS version the slice asks for, from its load commands."""
    out = subprocess.run(["otool", "-arch", arch, "-l", binary], capture_output=True, text=True, check=True).stdout
    for cmd, key in (("LC_BUILD_VERSION", "minos"), ("LC_VERSION_MIN_MACOSX", "version")):
        m = re.search(rf"cmd {cmd}\n(?:.*\n){{0,4}}?\s*{key} ([0-9.]+)", out)
        if m:
            return m.group(1)
    return None


def check(app: str, wanted: str) -> list[str]:
    problems: list[str] = []
    with open(os.path.join(app, "Contents", "Info.plist"), "rb") as f:
        info = plistlib.load(f)
    declared = info.get("LSMinimumSystemVersion")
    print(f"{app}: LSMinimumSystemVersion {declared}")
    if declared != wanted:
        problems.append(f"{app}: Info.plist says macOS {declared}, tauri.conf.json and the README say {wanted}")
    binary = os.path.join(app, "Contents", "MacOS", info["CFBundleExecutable"])
    archs = subprocess.run(["lipo", "-archs", binary], capture_output=True, text=True, check=True).stdout.split()
    if not archs:
        problems.append(f"{binary}: no architecture found")
    for arch in archs:
        found = minos(binary, arch)
        allowed = max(version(wanted), ARCH_FLOOR.get(arch, (0,)))
        print(f"  {arch}: needs macOS {found} (allowed up to {'.'.join(map(str, allowed))})")
        if found is None:
            problems.append(f"{binary} ({arch}): no minimum macOS in its load commands")
        elif version(found) > allowed:
            problems.append(
                f"{binary} ({arch}) needs macOS {found}, newer than the {'.'.join(map(str, allowed))} it may ask for"
            )
    return problems


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    wanted = claim()
    problems = [p for app in argv for p in check(app, wanted)]
    for p in problems:
        print(f"::error title=Minimum macOS::{p}")
    if not problems:
        print(f"OK: every app and architecture starts on macOS {wanted} or its architecture's first macOS.")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
