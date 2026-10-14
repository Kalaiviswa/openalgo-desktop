#!/usr/bin/env python3
"""Tests of scripts/ci/updater_manifest.py on a fabricated release folder.

    python3 scripts/ci/test_updater_manifest.py

No network and no signing key: signatures are minisign-shaped text with a
fake key, and the minisign tool is a stand-in script that records how it
was called. The real signature check runs in the release workflow.
"""

from __future__ import annotations

import base64
import io
import json
import os
import stat
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import updater_manifest as um  # noqa: E402

REPO = "marketcalls/openalgo-desktop"
TAG = "v1.2.0"
# Stands for the key line of the app's public key (not a real key).
APP_KEY_LINE = "RWQ-the-apps-public-key"
BUNDLES = {
    "darwin-aarch64": "OpenAlgo.Desktop_1.2.0_aarch64.app.tar.gz",
    "darwin-x86_64": "OpenAlgo.Desktop_1.2.0_x64.app.tar.gz",
    "windows-x86_64": "OpenAlgo.Desktop_1.2.0_x64-setup.exe",
    "linux-x86_64": "OpenAlgo.Desktop_1.2.0_amd64.AppImage",
    "linux-aarch64": "OpenAlgo.Desktop_1.2.0_aarch64.AppImage",
}
# Installers that are not updater bundles, as the release carries them.
OTHERS = [
    "OpenAlgo.Desktop_1.2.0_aarch64.dmg",
    "OpenAlgo.Desktop_1.2.0_x64.dmg",
    "OpenAlgo.Desktop_1.2.0_amd64.deb",
    "OpenAlgo.Desktop_1.2.0_amd64.deb.sig",
    "OpenAlgo.Desktop_1.2.0_arm64.deb",
    "SHA256SUMS.txt",
]
CHANGELOG = """# Changelog

## 1.2.0

### Updates

- OpenAlgo checks for updates and installs them when you choose Update.

## 1.1.0

- Older notes.
"""


def tauri_sig(file_name: str, version: str | None) -> str:
    """A .sig as the Tauri CLI writes it: base64 of a minisign signature file."""
    trusted = f"timestamp:1760500000\tfile:{file_name}"
    if version is not None:
        trusted += f"\tversion:{version}"
    text = (
        "untrusted comment: signature from tauri secret key\n"
        "RUSxq8DlKpo3E0FBQUFBQUFB\n"
        f"trusted comment: {trusted}\n"
        "R0xPQkFMU0lH\n"
    )
    return base64.b64encode(text.encode()).decode()


def release(dir_: str, *, skip: str | None = None, version: str = "1.2.0", extra=()) -> None:
    for key, name in BUNDLES.items():
        if key == skip:
            continue
        with open(os.path.join(dir_, name), "wb") as f:
            f.write(b"bundle " + key.encode())
        with open(os.path.join(dir_, name + ".sig"), "w") as f:
            f.write(tauri_sig(name, version))
    for name in [*OTHERS, *extra]:
        with open(os.path.join(dir_, name), "w") as f:
            f.write("x")


class Manifest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def build(self, **kw) -> dict:
        return um.build(self.dir, TAG, REPO, CHANGELOG, "2026-10-15T04:30:00Z", **kw)

    def test_every_platform_under_both_keys_with_its_signature_and_address(self) -> None:
        release(self.dir)
        m = self.build()
        self.assertEqual(m["version"], "1.2.0")
        self.assertEqual(m["pub_date"], "2026-10-15T04:30:00Z")
        self.assertEqual(
            sorted(m["platforms"]),
            sorted(
                [
                    "darwin-aarch64",
                    "darwin-aarch64-app",
                    "darwin-x86_64",
                    "darwin-x86_64-app",
                    "windows-x86_64",
                    "windows-x86_64-nsis",
                    "linux-x86_64",
                    "linux-x86_64-appimage",
                    "linux-aarch64",
                    "linux-aarch64-appimage",
                ]
            ),
        )
        for key, name in BUNDLES.items():
            entry = m["platforms"][key]
            self.assertEqual(
                entry["url"], f"https://github.com/{REPO}/releases/download/{TAG}/{name}"
            )
            with open(os.path.join(self.dir, name + ".sig")) as f:
                self.assertEqual(entry["signature"], f.read())
        # A .deb install never gets an AppImage or a .deb from the manifest
        # under a deb key: it is told to download the package.
        self.assertFalse(any("deb" in k for k in m["platforms"]))
        self.assertTrue(m["platforms"]["linux-x86_64"]["url"].endswith(".AppImage"))

    def test_release_notes_are_the_changelog_section_of_the_version(self) -> None:
        release(self.dir)
        notes = self.build()["notes"]
        self.assertEqual(
            notes,
            "### Updates\n\n- OpenAlgo checks for updates and installs them when you choose Update.",
        )
        self.assertNotIn("Older notes", notes)

    def test_a_missing_platform_fails_the_release(self) -> None:
        release(self.dir, skip="windows-x86_64")
        with self.assertRaisesRegex(um.ManifestError, "no updater bundle for windows-x86_64"):
            self.build()

    def test_a_bundle_without_its_signature_fails(self) -> None:
        release(self.dir)
        os.remove(os.path.join(self.dir, BUNDLES["linux-aarch64"] + ".sig"))
        with self.assertRaisesRegex(um.ManifestError, "has no signature"):
            self.build()

    def test_two_bundles_for_one_platform_fail(self) -> None:
        release(self.dir, extra=["OpenAlgo.Desktop_1.1.0_amd64.AppImage"])
        with self.assertRaisesRegex(um.ManifestError, "more than one updater bundle for linux-x86_64"):
            self.build()

    def test_a_signature_for_another_version_fails(self) -> None:
        release(self.dir, version="1.1.0")
        with self.assertRaisesRegex(um.ManifestError, "signed for version '1.1.0', not 1.2.0"):
            self.build()

    def test_a_signature_without_a_version_fails(self) -> None:
        release(self.dir)
        name = BUNDLES["darwin-x86_64"]
        with open(os.path.join(self.dir, name + ".sig"), "w") as f:
            f.write(tauri_sig(name, None))
        with self.assertRaisesRegex(um.ManifestError, "signed for version None"):
            self.build()

    def test_a_file_that_is_not_a_tauri_signature_fails(self) -> None:
        release(self.dir)
        with open(os.path.join(self.dir, BUNDLES["darwin-aarch64"] + ".sig"), "w") as f:
            f.write("not base64 !!")
        with self.assertRaisesRegex(um.ManifestError, "not a Tauri signature"):
            self.build()

    def test_a_tag_that_is_not_a_release_tag_fails(self) -> None:
        release(self.dir)
        with self.assertRaisesRegex(um.ManifestError, "not a release tag"):
            um.build(self.dir, "1.2.0", REPO, CHANGELOG, "x")

    def test_a_prerelease_tag_keeps_its_version(self) -> None:
        names = {k: v.replace("1.2.0", "1.3.0-beta.1") for k, v in BUNDLES.items()}
        for key, name in names.items():
            with open(os.path.join(self.dir, name), "wb") as f:
                f.write(b"x")
            with open(os.path.join(self.dir, name + ".sig"), "w") as f:
                f.write(tauri_sig(name, "1.3.0-beta.1"))
        m = um.build(self.dir, "v1.3.0-beta.1", REPO, CHANGELOG, "x")
        self.assertEqual(m["version"], "1.3.0-beta.1")
        self.assertNotIn("notes", m)

    def fake_minisign(self, ok: bool) -> tuple[str, str]:
        log = os.path.join(self.dir, "minisign.log")
        script = os.path.join(tempfile.mkdtemp(dir=self.dir), "minisign")
        with open(script, "w") as f:
            f.write(
                "#!/bin/sh\n"
                f'echo "$@" >> "{log}"\n'
                + ("exit 0\n" if ok else 'echo "Signature verification failed" >&2; exit 1\n')
            )
        os.chmod(script, os.stat(script).st_mode | stat.S_IEXEC)
        return script, log

    @unittest.skipIf(os.name == "nt", "the stand-in minisign is a shell script")
    def test_every_signature_is_checked_against_the_apps_key(self) -> None:
        release(self.dir)
        script, log = self.fake_minisign(ok=True)
        self.build(key=APP_KEY_LINE, minisign=script)
        with open(log) as f:
            calls = f.read().splitlines()
        self.assertEqual(len(calls), 5)
        for call in calls:
            self.assertIn(f"-P {APP_KEY_LINE}", call)
            self.assertTrue(call.startswith("-V -q -m "), call)

    @unittest.skipIf(os.name == "nt", "the stand-in minisign is a shell script")
    def test_a_signature_from_another_key_fails_the_release(self) -> None:
        release(self.dir)
        script, _ = self.fake_minisign(ok=False)
        with self.assertRaisesRegex(um.ManifestError, "does not verify against the key built into the app"):
            self.build(key="RWQother", minisign=script)

    def test_the_apps_public_key_is_read_from_the_config(self) -> None:
        conf = os.path.join(self.dir, "tauri.conf.json")
        key_file = "untrusted comment: minisign public key: 13379A2AE5C0AB71\nRWRkey\n"
        with open(conf, "w") as f:
            json.dump({"plugins": {"updater": {"pubkey": base64.b64encode(key_file.encode()).decode()}}}, f)
        self.assertEqual(um.pubkey_line(conf), "RWRkey")

    def test_the_shipped_config_carries_a_minisign_key(self) -> None:
        conf = os.path.join(os.path.dirname(__file__), "..", "..", "src-tauri", "tauri.conf.json")
        self.assertEqual(
            um.pubkey_line(conf), "RWRxq8DlKpo3EyeRRTVYwVdzrC1g69AqjhlisuK113HG2h4FxIn87WLQ"
        )

    def test_main_writes_the_manifest_and_fails_with_an_annotation(self) -> None:
        release(self.dir)
        changelog = os.path.join(self.dir, "CHANGELOG.md")
        with open(changelog, "w") as f:
            f.write(CHANGELOG)
        out = os.path.join(self.dir, "latest.json")
        args = ["--assets", self.dir, "--tag", TAG, "--repo", REPO, "--changelog", changelog, "--out", out]
        with redirect_stdout(io.StringIO()):
            self.assertEqual(um.main(args), 0)
        with open(out) as f:
            self.assertEqual(json.load(f)["version"], "1.2.0")
        os.remove(os.path.join(self.dir, BUNDLES["darwin-x86_64"]))
        buf = io.StringIO()
        with redirect_stdout(buf):
            self.assertEqual(um.main(args), 1)
        self.assertIn("::error title=Update manifest::no updater bundle for darwin-x86_64", buf.getvalue())


if __name__ == "__main__":
    unittest.main()
