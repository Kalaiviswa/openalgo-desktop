#!/usr/bin/env python3
"""Build latest.json, the in-app update manifest of a release, from its assets.

    python3 scripts/ci/updater_manifest.py --assets DIR --tag v1.2.0 \\
        --repo marketcalls/openalgo-desktop --changelog CHANGELOG.md \\
        --config src-tauri/tauri.conf.json [--minisign minisign] --out latest.json

DIR holds the release's assets under the names GitHub published them with
(the release workflow downloads them there). For each platform the app
updates itself on, the manifest names the updater bundle, its download
address on the release and the contents of its .sig file:

    darwin-aarch64   OpenAlgo.Desktop_<v>_aarch64.app.tar.gz   (.app.tar.gz.sig)
    darwin-x86_64    OpenAlgo.Desktop_<v>_x64.app.tar.gz
    windows-x86_64   OpenAlgo.Desktop_<v>_x64-setup.exe        (.exe.sig)
    linux-x86_64     OpenAlgo.Desktop_<v>_amd64.AppImage       (.AppImage.sig)
    linux-aarch64    OpenAlgo.Desktop_<v>_aarch64.AppImage

each under `{os}-{arch}` and `{os}-{arch}-{installer}` (`app`, `nsis`,
`appimage`), the keys the update plugin looks for. A .deb or .rpm signature
is never listed: those installs are told to download the new package.

The release fails (exit 1) when a platform is missing or ambiguous, a
signature is not a Tauri .sig, a signature was made for another version
than the tag, or (with --minisign) a signature does not verify against the
public key built into the app (`plugins.updater.pubkey` in --config). That
last check is what proves the signing secret matches the key every
installed copy trusts. The release notes are the CHANGELOG.md section of
the version.

No network; tests: scripts/ci/test_updater_manifest.py.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import datetime as dt
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.parse

# platform key -> (installer name, how its bundle's name ends)
PLATFORMS: dict[str, tuple[str, str]] = {
    "darwin-aarch64": ("app", "_aarch64.app.tar.gz"),
    "darwin-x86_64": ("app", "_x64.app.tar.gz"),
    "windows-x86_64": ("nsis", "_x64-setup.exe"),
    "linux-x86_64": ("appimage", "_amd64.AppImage"),
    "linux-aarch64": ("appimage", "_aarch64.AppImage"),
}


class ManifestError(Exception):
    """A reason the release must not publish this manifest."""


def platform_of(name: str) -> str | None:
    """The platform key an updater bundle serves, or None for any other asset."""
    for key, (_installer, suffix) in PLATFORMS.items():
        if name.endswith(suffix):
            return key
    return None


def signature_text(sig_b64: str) -> str:
    """The minisign signature file inside a Tauri .sig (base64 of the file)."""
    try:
        text = base64.b64decode(sig_b64.strip(), validate=True).decode("utf-8")
    except (binascii.Error, ValueError, UnicodeDecodeError) as e:
        raise ManifestError(f"not a Tauri signature (base64 of a minisign file): {e}") from e
    lines = text.splitlines()
    if (
        len(lines) < 4
        or not lines[0].startswith("untrusted comment:")
        or not lines[2].startswith("trusted comment: ")
    ):
        raise ManifestError("not a minisign signature file")
    return text


def signed_version(sig_text: str) -> str | None:
    """The `version:` field the Tauri CLI writes into the trusted comment."""
    trusted = sig_text.splitlines()[2][len("trusted comment: ") :]
    for field in trusted.split("\t"):
        if field.startswith("version:"):
            return field[len("version:") :]
    return None


def changelog_section(text: str, version: str) -> str | None:
    """The body of `## <version>` in CHANGELOG.md, up to the next `## `."""
    out: list[str] = []
    inside = False
    for line in text.splitlines():
        if line.startswith("## "):
            if inside:
                break
            inside = line[3:].strip() == version
            continue
        if inside:
            out.append(line)
    body = "\n".join(out).strip()
    return body or None


def pubkey_line(config_path: str) -> str:
    """The key line of the public key built into the app."""
    with open(config_path, encoding="utf-8") as f:
        conf = json.load(f)
    encoded = conf["plugins"]["updater"]["pubkey"]
    lines = base64.b64decode(encoded).decode("utf-8").splitlines()
    if len(lines) < 2 or not lines[0].startswith("untrusted comment:"):
        raise ManifestError("plugins.updater.pubkey is not a minisign public key")
    return lines[1].strip()


def collect(assets: str) -> dict[str, str]:
    """platform key -> bundle file name, refusing a missing or doubled one."""
    found: dict[str, list[str]] = {k: [] for k in PLATFORMS}
    for name in sorted(os.listdir(assets)):
        key = platform_of(name)
        if key:
            found[key].append(name)
    chosen: dict[str, str] = {}
    for key, names in found.items():
        if not names:
            raise ManifestError(f"no updater bundle for {key} (a file ending {PLATFORMS[key][1]})")
        if len(names) > 1:
            raise ManifestError(f"more than one updater bundle for {key}: {', '.join(names)}")
        name = names[0]
        if not os.path.isfile(os.path.join(assets, name + ".sig")):
            raise ManifestError(f"{name} has no signature ({name}.sig)")
        chosen[key] = name
    return chosen


def verify(minisign: str, path: str, sig_text: str, key: str) -> None:
    """Check one bundle's signature with the minisign tool."""
    with tempfile.NamedTemporaryFile("w", suffix=".minisig", delete=False) as f:
        f.write(sig_text)
        sig_path = f.name
    try:
        r = subprocess.run(
            [minisign, "-V", "-q", "-m", path, "-x", sig_path, "-P", key],
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        os.unlink(sig_path)
    if r.returncode != 0:
        raise ManifestError(
            f"{os.path.basename(path)} does not verify against the key built into the app: "
            f"{(r.stderr or r.stdout).strip()}"
        )


def build(
    assets: str,
    tag: str,
    repo: str,
    changelog: str,
    pub_date: str,
    key: str | None = None,
    minisign: str | None = None,
) -> dict:
    if not re.fullmatch(r"v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", tag):
        raise ManifestError(f"'{tag}' is not a release tag (v<major>.<minor>.<patch>)")
    version = tag[1:]
    platforms: dict[str, dict[str, str]] = {}
    for key_name, name in collect(assets).items():
        with open(os.path.join(assets, name + ".sig"), encoding="utf-8") as f:
            sig_b64 = f.read().strip()
        text = signature_text(sig_b64)
        signed = signed_version(text)
        if signed != version:
            raise ManifestError(
                f"{name}.sig was signed for version {signed!r}, not {version}: "
                "the app refuses an update whose signature names another version"
            )
        if minisign:
            if not key:
                raise ManifestError("--minisign needs the app's public key (--config)")
            verify(minisign, os.path.join(assets, name), text, key)
        url = f"https://github.com/{repo}/releases/download/{tag}/{urllib.parse.quote(name)}"
        entry = {"signature": sig_b64, "url": url}
        installer = PLATFORMS[key_name][0]
        platforms[key_name] = entry
        platforms[f"{key_name}-{installer}"] = entry
    manifest: dict = {"version": version, "pub_date": pub_date}
    notes = changelog_section(changelog, version)
    if notes:
        manifest["notes"] = notes
    manifest["platforms"] = dict(sorted(platforms.items()))
    return manifest


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--assets", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--changelog", required=True)
    p.add_argument("--config", help="tauri.conf.json, for the app's public key")
    p.add_argument("--minisign", help="the minisign tool, to verify every signature")
    p.add_argument("--out", required=True)
    args = p.parse_args(argv)
    try:
        with open(args.changelog, encoding="utf-8") as f:
            changelog = f.read()
        key = pubkey_line(args.config) if args.config else None
        now = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
        manifest = build(
            args.assets,
            args.tag,
            args.repo,
            changelog,
            now.isoformat().replace("+00:00", "Z"),
            key=key,
            minisign=args.minisign,
        )
    except ManifestError as e:
        print(f"::error title=Update manifest::{e}")
        return 1
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    for k, v in manifest["platforms"].items():
        print(f"{k}: {v['url']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
