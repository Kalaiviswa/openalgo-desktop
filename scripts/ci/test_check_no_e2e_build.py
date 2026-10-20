#!/usr/bin/env python3
"""Tests of scripts/ci/check_no_e2e_build.py.

    python3 scripts/ci/test_check_no_e2e_build.py

The release build runs it as Tauri's beforeBundleCommand (--tauri-hook)
and after the upload as a backstop (--same-as-checked); the install smoke
and the E2E workflow run it on a binary. These tests run each mode on
binaries written to a temporary folder.
"""

from __future__ import annotations

import io
import os
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_no_e2e_build as check  # noqa: E402

TRIPLE = "x86_64-pc-windows-msvc"


def pe(code: bytes, signature: bytes = b"", checksum: int = 0) -> bytes:
    """A PE32+ file: DOS header, PE header, optional header with 16 data
    directories, then `code`; signed, the certificate table follows at an
    8-byte boundary, as signtool writes it."""
    head = bytearray(0x200)
    head[:2] = b"MZ"
    head[0x3C:0x40] = (0x80).to_bytes(4, "little")
    head[0x80:0x84] = b"PE\0\0"
    opt = 0x80 + 24
    head[opt : opt + 2] = (0x20B).to_bytes(2, "little")
    head[opt + 64 : opt + 68] = checksum.to_bytes(4, "little")
    data = bytes(head) + code
    if signature:
        data += bytes(-len(data) % 8)
        security = opt + 112 + 8 * 4
        table = len(data)
        data = bytearray(data)
        data[security : security + 4] = table.to_bytes(4, "little")
        data[security + 4 : security + 8] = len(signature).to_bytes(4, "little")
        data = bytes(data) + signature
    return data


def run(argv, env=None):
    out = io.StringIO()
    old = dict(os.environ)
    try:
        os.environ.update(env or {})
        with redirect_stdout(out):
            code = check.main(argv)
    finally:
        os.environ.clear()
        os.environ.update(old)
    return code, out.getvalue()


class CodeDigest(unittest.TestCase):
    def test_signing_in_place_keeps_the_digest(self):
        unsigned = pe(b"\x90" * 1001)
        signed = pe(b"\x90" * 1001, signature=b"CERT" * 50, checksum=0x1234ABCD)
        self.assertNotEqual(unsigned, signed)
        self.assertEqual(check.code_digest(unsigned), check.code_digest(signed))

    def test_other_code_changes_the_digest(self):
        self.assertNotEqual(check.code_digest(pe(b"\x90" * 1001)), check.code_digest(pe(b"\x90" * 1000 + b"\xcc")))
        signed = pe(b"\x90" * 1001, signature=b"CERT" * 50)
        self.assertNotEqual(check.code_digest(signed), check.code_digest(pe(b"\x91" * 1001, signature=b"CERT" * 50)))

    def test_any_other_file_is_hashed_whole(self):
        elf = b"\x7fELF" + bytes(100)
        self.assertEqual(check.code_digest(elf), __import__("hashlib").sha256(elf).hexdigest())
        self.assertNotEqual(check.code_digest(elf), check.code_digest(elf + b"\0"))


class Modes(unittest.TestCase):
    def setUp(self):
        # The hook finds the binary under the repository's src-tauri/target,
        # from where the script lives: point it at a temporary copy.
        self.root = tempfile.mkdtemp()
        os.makedirs(os.path.join(self.root, "scripts", "ci"))
        shutil.copy(check.__file__, os.path.join(self.root, "scripts", "ci"))
        self.release = os.path.join(self.root, "src-tauri", "target", TRIPLE, "release")
        os.makedirs(self.release)
        self.exe = os.path.join(self.release, "openalgo-desktop.exe")
        self.env_file = os.path.join(self.root, "github_env")
        open(self.env_file, "w").close()
        self.orig_file = check.__file__
        check.__file__ = os.path.join(self.root, "scripts", "ci", "check_no_e2e_build.py")

    def tearDown(self):
        check.__file__ = self.orig_file
        shutil.rmtree(self.root)

    def write(self, data: bytes):
        with open(self.exe, "wb") as f:
            f.write(data)

    def hook(self, **env):
        base = {"TAURI_ENV_TARGET_TRIPLE": TRIPLE, "TAURI_ENV_PLATFORM": "windows", "GITHUB_ENV": self.env_file}
        base.update(env)
        return run(["--tauri-hook"], {k: v for k, v in base.items() if v is not None})

    def recorded(self) -> str:
        with open(self.env_file, encoding="utf-8") as f:
            lines = [ln for ln in f.read().splitlines() if ln.startswith(check.DIGEST_VAR + "=")]
        return lines[-1].split("=", 1)[1] if lines else ""

    def test_the_hook_passes_a_release_binary_and_records_its_digest(self):
        self.write(pe(b"\x90" * 64))
        code, out = self.hook()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.recorded(), check.code_digest(pe(b"\x90" * 64)))

    def test_the_hook_refuses_the_test_build_and_records_nothing(self):
        for marker in check.MARKERS:
            self.write(pe(b"\x90" * 64 + marker))
            code, out = self.hook()
            self.assertEqual(code, 1, marker)
            self.assertIn("nothing is bundled", out)
        self.assertEqual(self.recorded(), "")

    def test_the_hook_fails_closed(self):
        self.write(pe(b"\x90" * 64))
        self.assertEqual(self.hook(TAURI_ENV_TARGET_TRIPLE=None)[0], 1)
        self.assertEqual(self.hook(TAURI_ENV_DEBUG="true")[0], 1)
        self.assertEqual(self.hook(TAURI_ENV_TARGET_TRIPLE="aarch64-apple-darwin")[0], 1)
        self.assertEqual(self.recorded(), "")

    def test_the_backstop_accepts_the_checked_binary_signed_in_place(self):
        self.write(pe(b"\x90" * 64))
        self.hook()
        self.write(pe(b"\x90" * 64, signature=b"CERT" * 20, checksum=7))
        code, out = run(["--same-as-checked", self.exe], {check.DIGEST_VAR: self.recorded()})
        self.assertEqual(code, 0, out)

    def test_the_backstop_refuses_another_binary_or_a_missing_record(self):
        self.write(pe(b"\x90" * 64))
        self.hook()
        digest = self.recorded()
        self.write(pe(b"\x90" * 63 + b"\xcc"))
        self.assertEqual(run(["--same-as-checked", self.exe], {check.DIGEST_VAR: digest})[0], 1)
        code, out = run(["--same-as-checked", self.exe], {check.DIGEST_VAR: ""})
        self.assertEqual(code, 1)
        self.assertIn("did not run", out)

    def test_the_plain_check_and_its_inversion(self):
        self.write(pe(b"\x90" * 64))
        self.assertEqual(run([self.exe])[0], 0)
        self.assertEqual(run([self.exe, "--expect-e2e"])[0], 1)
        self.write(pe(b"\x90" * 64 + check.MARKERS[2]))
        self.assertEqual(run([self.exe])[0], 1)
        self.assertEqual(run([self.exe, "--expect-e2e"])[0], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
