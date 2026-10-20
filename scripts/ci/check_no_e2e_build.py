#!/usr/bin/env python3
"""The binary is not the Windows E2E test build (src-tauri/src/e2e.rs).

    python3 scripts/ci/check_no_e2e_build.py <binary> [--expect-e2e]

The e2e-debug-port feature makes the main window's WebView2 open a remote
debugging port. Only CI's Windows native E2E builds it
(.github/workflows/native-e2e-windows.yml); a shipped binary must not carry
its strings. Exit 0 when none is found, 1 when one is. With --expect-e2e
the answer is inverted: the E2E workflow runs it on its own test build to
show the check finds what it looks for.
"""

from __future__ import annotations

import sys

MARKERS = (b"OPENALGO_E2E_DEBUG_PORT", b"--remote-debugging-port")


def found(path: str) -> list[str]:
    with open(path, "rb") as f:
        data = f.read()
    return [m.decode() for m in MARKERS if m in data]


def main(argv: list[str]) -> int:
    expect = "--expect-e2e" in argv
    paths = [a for a in argv if a != "--expect-e2e"]
    if len(paths) != 1:
        print(__doc__)
        return 2
    hits = found(paths[0])
    if expect:
        if hits:
            print(f"The test build carries {hits}, as expected: the check finds them.")
            return 0
        print(f"::error title=E2E build check::{paths[0]} should be the E2E test build but carries none of {MARKERS}")
        return 1
    if hits:
        print(f"::error title=E2E build check::{paths[0]} is the E2E test build (it carries {hits}); it must never ship.")
        return 1
    print(f"OK: {paths[0]} is not the E2E test build.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
