#!/usr/bin/env python3
"""The binary is not CI's native E2E test build (src-tauri/src/e2e.rs).

    python3 scripts/ci/check_no_e2e_build.py <binary> [--expect-e2e]
    python scripts/ci/check_no_e2e_build.py --tauri-hook
    python scripts/ci/check_no_e2e_build.py --same-as-checked <binary>

Only CI's native E2E (.github/workflows/native-e2e-test-build.yml) builds
the e2e features: e2e-broker signs in to a scripted broker
(src-tauri/src/brokers/scripted.rs), e2e-debug-port makes the Windows main
window's WebView2 open a remote debugging port. A shipped binary must not
carry their strings. Exit 0 when none is found, 1 when one is. With
--expect-e2e the answer is inverted: the E2E workflow runs it on its own
test build to show the check finds what it looks for.

--tauri-hook is the release build's `beforeBundleCommand`
(src-tauri/release-checks.conf.json): Tauri runs it after compiling and
before bundling, in the same build, so the binary it passes is the one
the installers are made of, and a refused one stops the build before
anything is bundled or uploaded. It finds the binary from Tauri's
TAURI_ENV_TARGET_TRIPLE and TAURI_ENV_PLATFORM, refuses a debug build, and
records the binary's code digest in GITHUB_ENV
(OPENALGO_CHECKED_BINARY_DIGEST).

--same-as-checked runs after the upload, as a backstop: the binary in the
target folder must still have the digest the hook recorded, and a missing
record (the hook did not run) fails too.

The code digest is the SHA-256 of the file, except that for a Windows (PE)
binary its Authenticode signature is left out (the CheckSum field, the
certificate table's entry and the table, and the zero padding before it).
Tauri's bundler (tauri-bundler 2.x, `bundle.rs`) patches and, on Windows,
signs the target binary in place for each package, then puts the unsigned,
unpatched binary back; on macOS it signs the copy inside the .app
(`macos/app.rs`). So the binary left in the target folder is the one
checked, byte for byte; the PE rule keeps the backstop right if a later
bundler leaves it signed.
"""

from __future__ import annotations

import hashlib
import os
import sys

MARKERS = (b"OPENALGO_E2E_DEBUG_PORT", b"--remote-debugging-port", b"OPENALGO_E2E_SCRIPTED_BROKER")
DIGEST_VAR = "OPENALGO_CHECKED_BINARY_DIGEST"


def read(path: str) -> bytes:
    with open(path, "rb") as f:
        return f.read()


def found(data: bytes) -> list[str]:
    return [m.decode() for m in MARKERS if m in data]


def u32(data: bytes, at: int) -> int:
    return int.from_bytes(data[at : at + 4], "little")


def code_digest(data: bytes) -> str:
    """SHA-256 of `data`, a PE file's without its Authenticode signature."""
    plain = hashlib.sha256(data).hexdigest()
    if len(data) < 0x40 or data[:2] != b"MZ":
        return plain
    pe = u32(data, 0x3C)
    if data[pe : pe + 4] != b"PE\0\0":
        return plain
    opt = pe + 24
    magic = int.from_bytes(data[opt : opt + 2], "little")
    directories = {0x10B: opt + 96, 0x20B: opt + 112}.get(magic)
    if directories is None:
        return plain
    security = directories + 8 * 4  # IMAGE_DIRECTORY_ENTRY_SECURITY
    if security + 8 > len(data):
        return plain
    table, size = u32(data, security), u32(data, security + 4)
    code = bytearray(data)
    if table and size and table + size == len(code):
        del code[table:]
    code[opt + 64 : opt + 68] = bytes(4)  # CheckSum
    code[security : security + 8] = bytes(8)
    return "pe:" + hashlib.sha256(bytes(code).rstrip(b"\0")).hexdigest()


def tauri_hook() -> int:
    triple = os.environ.get("TAURI_ENV_TARGET_TRIPLE", "")
    if not triple:
        print("::error title=E2E build check::TAURI_ENV_TARGET_TRIPLE is not set: run this as Tauri's beforeBundleCommand.")
        return 1
    if os.environ.get("TAURI_ENV_DEBUG") == "true":
        print("::error title=E2E build check::a debug build is not a release.")
        return 1
    exe = ".exe" if os.environ.get("TAURI_ENV_PLATFORM") == "windows" else ""
    repo = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
    path = os.path.normpath(os.path.join(repo, "src-tauri", "target", triple, "release", f"openalgo-desktop{exe}"))
    if not os.path.isfile(path):
        print(f"::error title=E2E build check::{path} was not built.")
        return 1
    data = read(path)
    hits = found(data)
    if hits:
        print(f"::error title=E2E build check::{path} is the E2E test build (it carries {hits}); nothing is bundled.")
        return 1
    digest = code_digest(data)
    env_file = os.environ.get("GITHUB_ENV")
    if env_file:
        with open(env_file, "a", encoding="utf-8") as f:
            f.write(f"{DIGEST_VAR}={digest}\n")
    print(f"OK: {path} is not the E2E test build; bundling it (code digest {digest}).")
    return 0


def same_as_checked(path: str) -> int:
    checked = os.environ.get(DIGEST_VAR, "")
    if not checked:
        print(f"::error title=E2E build check::no {DIGEST_VAR}: the check before bundling did not run.")
        return 1
    digest = code_digest(read(path))
    if digest != checked:
        print(f"::error title=E2E build check::{path} is not the binary checked before bundling ({digest}, checked {checked}).")
        return 1
    print(f"OK: {path} is the binary checked before bundling ({digest}).")
    return 0


def main(argv: list[str]) -> int:
    if argv == ["--tauri-hook"]:
        return tauri_hook()
    if len(argv) == 2 and argv[0] == "--same-as-checked":
        return same_as_checked(argv[1])
    expect = "--expect-e2e" in argv
    paths = [a for a in argv if a != "--expect-e2e"]
    if len(paths) != 1 or paths[0].startswith("--"):
        print(__doc__)
        return 2
    hits = found(read(paths[0]))
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
