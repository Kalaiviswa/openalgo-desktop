#!/usr/bin/env python3
"""Count the downloads of OpenAlgo Desktop's releases for the `stats` branch.

    GITHUB_TOKEN=... python3 scripts/ci/download_stats.py \\
        --repo marketcalls/openalgo-desktop --out DIR [--today YYYY-MM-DD]

Reads every published release through the GitHub API (drafts are skipped)
and writes into DIR, which holds the `stats` branch's files:

    badges/<name>.json   shields.io endpoint badges (BADGES below); the
                         README on master shows downloads and downloads-month
    downloads.csv        one row per day, release and file with that day's
                         cumulative count; a second run on the same day
                         replaces that day's rows
    README.md            the tables, the date they were read and what the
                         counts mean

A download is counted by the file's name (COUNTED below). latest.json is
fetched by every installed copy's update check, so it is reported as update
checks and never added to a download total; signatures, SHA256SUMS.txt and
any other file are not counted.

The downloads of the last 30 days are worked out from downloads.csv: the
count now minus the count recorded 30 days ago, or, while the history is
shorter, the earliest count recorded. A release published within the 30
days has every download inside them.

Standard library only. GITHUB_TOKEN is sent to api.github.com alone and
never printed. Tests: scripts/ci/test_download_stats.py.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import os
import re
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass

API = "https://api.github.com"
WINDOW_DAYS = 30

# Who downloads a file: a person from the release page, the app's in-app
# update, or either.
INSTALL = "install"
UPDATE = "update"
INSTALL_OR_UPDATE = "install-or-update"
UPDATE_CHECK = "update-check"

PLATFORMS = (("windows", "Windows"), ("macos", "macOS"), ("linux", "Linux"))

# How a counted file's name ends (lower case), its platform and who downloads
# it. Names as the release workflow publishes them (release.yml,
# updater_manifest.py): OpenAlgo.Desktop_<v>_x64-setup.exe, _aarch64.dmg,
# _x64.dmg, _aarch64.app.tar.gz, _x64.app.tar.gz, _amd64.AppImage,
# _aarch64.AppImage, _amd64.deb, _arm64.deb. A .sig ends in .sig and so
# matches none of them.
COUNTED: tuple[tuple[str, str, str], ...] = (
    # The NSIS setup is both the installer and the in-app update download.
    ("-setup.exe", "windows", INSTALL_OR_UPDATE),
    (".dmg", "macos", INSTALL),
    (".app.tar.gz", "macos", UPDATE),
    # The AppImage is downloaded to install and by the in-app update.
    (".appimage", "linux", INSTALL_OR_UPDATE),
    # A .deb install is told about a new version and downloads it by hand.
    (".deb", "linux", INSTALL),
)
UPDATE_MANIFEST = "latest.json"
DOWNLOADED_BY = {
    INSTALL: "a person (release page)",
    UPDATE: "the in-app update",
    INSTALL_OR_UPDATE: "a person or the in-app update",
    UPDATE_CHECK: "update checks (not a download)",
}

CSV_FIELDS = ["date", "tag", "asset", "platform", "kind", "downloads"]
BADGE_COLOR = "blue"
# File name under badges/ -> label. README.md on master shows the first two,
# where OpenAlgo web's README shows its package downloads: the labels say
# these count the desktop app's release downloads.
BADGES = {
    "downloads": "desktop downloads",
    "downloads-month": "desktop downloads/month",
    "windows": "Windows downloads",
    "macos": "macOS downloads",
    "linux": "Linux downloads",
    "latest-version": "latest version downloads",
}


class StatsError(Exception):
    """A reason no stats are written."""


@dataclass(frozen=True)
class Count:
    """One release file's cumulative download count."""

    tag: str
    asset: str
    platform: str
    kind: str
    downloads: int

    @property
    def is_download(self) -> bool:
        return self.kind != UPDATE_CHECK


@dataclass(frozen=True)
class Release:
    tag: str
    published: str  # YYYY-MM-DD
    prerelease: bool
    counts: tuple[Count, ...]

    def downloads(self, platform: str | None = None, kind: str | None = None) -> int:
        return sum(
            c.downloads
            for c in self.counts
            if c.is_download
            and (platform is None or c.platform == platform)
            and (kind is None or c.kind == kind)
        )

    @property
    def update_checks(self) -> int:
        return sum(c.downloads for c in self.counts if c.kind == UPDATE_CHECK)


@dataclass(frozen=True)
class Totals:
    downloads: int
    month: int
    by_platform: dict[str, int]
    update_checks: int
    latest: Release | None


def classify(name: str) -> tuple[str, str] | None:
    """(platform, kind) of a release file, or None for a file not counted."""
    lower = name.lower()
    if lower == UPDATE_MANIFEST:
        return "", UPDATE_CHECK
    for suffix, platform, kind in COUNTED:
        if lower.endswith(suffix):
            return platform, kind
    return None


def readable(n: int) -> str:
    """A count as a badge shows it: 999, 1.2k, 12.3k, 123k, 1.2M. Rounded
    down, so a badge never shows more downloads than were counted."""
    if n < 1000:
        return str(n)
    size, unit = next(
        (size, unit)
        for size, unit in ((10**3, "k"), (10**6, "M"), (10**9, "B"))
        if n < size * 1000 or unit == "B"
    )
    whole, tenths = n // size, n * 10 // size % 10
    if whole >= 100 or tenths == 0:
        return f"{whole}{unit}"
    return f"{whole}.{tenths}{unit}"


# ------------------------------------------------------------- GitHub API


def next_page(link_header: str) -> str | None:
    """The rel="next" address of a Link header, only if it is on the API host:
    the token is never sent anywhere else."""
    for part in link_header.split(","):
        m = re.fullmatch(r'\s*<([^>]+)>\s*;\s*rel="next"\s*', part)
        if m:
            url = m.group(1)
            if not url.startswith(API + "/"):
                raise StatsError(f"the next page is not on {API}: {url}")
            return url
    return None


def http_get(url: str, token: str) -> tuple[list, str | None]:
    """One page of a list endpoint and the address of the next page."""
    req = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "openalgo-desktop-download-stats",
        },
    )
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            body = json.load(resp)
            link = resp.headers.get("Link", "")
    except urllib.error.HTTPError as e:
        raise StatsError(f"GitHub answered {e.code} for {url}") from None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as e:
        raise StatsError(f"could not read {url}: {e}") from None
    if not isinstance(body, list):
        raise StatsError(f"{url} did not answer a list")
    return body, next_page(link)


def fetch_releases(repo: str, token: str, get=http_get) -> list[dict]:
    """Every release of the repository, all pages."""
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise StatsError(f"'{repo}' is not owner/name")
    url: str | None = f"{API}/repos/{repo}/releases?per_page=100"
    releases: list[dict] = []
    while url:
        page, url = get(url, token)
        releases.extend(page)
    return releases


def releases_from_api(raw: list[dict]) -> list[Release]:
    """Published releases, newest first, with their counted files."""
    out = []
    for r in raw:
        if r.get("draft"):
            continue
        tag = str(r.get("tag_name") or "")
        published = str(r.get("published_at") or r.get("created_at") or "")[:10]
        counts = []
        for a in r.get("assets") or []:
            name = str(a.get("name") or "")
            kind = classify(name)
            if kind is None:
                continue
            downloads = a.get("download_count")
            if not isinstance(downloads, int) or downloads < 0:
                raise StatsError(f"{tag} {name}: download count {downloads!r} is not a count")
            counts.append(Count(tag, name, kind[0], kind[1], downloads))
        counts.sort(key=lambda c: c.asset)
        out.append(Release(tag, published, bool(r.get("prerelease")), tuple(counts)))
    out.sort(key=lambda r: (r.published, r.tag), reverse=True)
    return out


# ---------------------------------------------------------------- history


def read_history(path: str) -> list[tuple[str, Count]]:
    if not os.path.exists(path):
        return []
    with open(path, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        if reader.fieldnames != CSV_FIELDS:
            raise StatsError(
                f"{path} has the columns {reader.fieldnames}, not {CSV_FIELDS}; "
                "refusing to rewrite a history this script did not write"
            )
        rows = []
        for row in reader:
            try:
                downloads = int(row["downloads"])
            except (TypeError, ValueError):
                raise StatsError(f"{path} line {reader.line_num}: not a count") from None
            count = Count(row["tag"], row["asset"], row["platform"], row["kind"], downloads)
            rows.append((row["date"], count))
    return rows


def merge_history(
    history: list[tuple[str, Count]], today: str, releases: list[Release]
) -> list[tuple[str, Count]]:
    """The history with today's rows replaced by the counts read now."""
    rows = [(d, c) for d, c in history if d != today]
    rows += [(today, c) for r in releases for c in r.counts]
    rows.sort(key=lambda row: (row[0], row[1].tag, row[1].asset))
    return rows


def write_history(path: str, rows: list[tuple[str, Count]]) -> None:
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(CSV_FIELDS)
        for d, c in rows:
            w.writerow([d, c.tag, c.asset, c.platform, c.kind, c.downloads])


def downloads_in_window(
    history: list[tuple[str, Count]], releases: list[Release], today: str
) -> int:
    """Downloads in the WINDOW_DAYS days up to today, from the history
    (which already holds today's rows)."""
    cutoff = (dt.date.fromisoformat(today) - dt.timedelta(days=WINDOW_DAYS)).isoformat()
    dates = sorted({d for d, _ in history})
    older = [d for d in dates if d <= cutoff]
    base_date = older[-1] if older else (dates[0] if dates else today)
    base = {(c.tag, c.asset): c.downloads for d, c in history if d == base_date}
    total = 0
    for r in releases:
        # Every download of a release published inside the window is in it.
        recent = r.published > cutoff
        for c in r.counts:
            if c.is_download:
                before = 0 if recent else base.get((c.tag, c.asset), 0)
                # A file uploaded again starts from zero: never below zero.
                total += max(c.downloads - before, 0)
    return total


# ---------------------------------------------------------------- outputs


def totals(releases: list[Release], month: int) -> Totals:
    final = [r for r in releases if not r.prerelease]
    return Totals(
        downloads=sum(r.downloads() for r in releases),
        month=month,
        by_platform={p: sum(r.downloads(p) for r in releases) for p, _ in PLATFORMS},
        update_checks=sum(r.update_checks for r in releases),
        latest=final[0] if final else None,
    )


def badges(t: Totals) -> dict[str, dict]:
    def badge(name: str, count: int, label: str | None = None) -> dict:
        return {
            "schemaVersion": 1,
            "label": label or BADGES[name],
            "message": readable(count),
            "color": BADGE_COLOR,
        }

    latest_label = f"{t.latest.tag} downloads" if t.latest else None
    return {
        "downloads": badge("downloads", t.downloads),
        "downloads-month": badge("downloads-month", t.month),
        **{p: badge(p, t.by_platform[p]) for p, _ in PLATFORMS},
        "latest-version": badge(
            "latest-version", t.latest.downloads() if t.latest else 0, latest_label
        ),
    }


def stats_page(repo: str, today: str, releases: list[Release], t: Totals) -> str:
    """README.md of the stats branch."""
    n = "{:,}".format
    lines = [
        "# OpenAlgo Desktop downloads",
        "",
        f"How many times the files of each [OpenAlgo Desktop](https://github.com/{repo}) "
        f"release were downloaded from GitHub, read on {today} (UTC). "
        "Updated every 6 hours.",
        "",
        "## Totals",
        "",
        "| | Downloads |",
        "| --- | ---: |",
        f"| All time | {n(t.downloads)} |",
        f"| Last {WINDOW_DAYS} days | {n(t.month)} |",
        *(f"| {label} | {n(t.by_platform[p])} |" for p, label in PLATFORMS),
        "",
        f"Update checks, which are not downloads and are in no total: {n(t.update_checks)}.",
        "",
        "## By release",
        "",
    ]
    if not releases:
        lines += ["No release has been published yet.", ""]
    else:
        lines += [
            "| Release | Published | Windows | macOS installer | macOS update "
            "| Linux AppImage | Linux .deb | Total | Update checks |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
        for r in releases:
            tag = f"{r.tag} (pre-release)" if r.prerelease else r.tag
            columns = [
                r.downloads("windows"),
                r.downloads("macos", INSTALL),  # .dmg
                r.downloads("macos", UPDATE),  # .app.tar.gz
                r.downloads("linux", INSTALL_OR_UPDATE),  # AppImage
                r.downloads("linux", INSTALL),  # .deb
                r.downloads(),
                r.update_checks,
            ]
            lines.append(f"| {tag} | {r.published} | " + " | ".join(map(n, columns)) + " |")
        lines += [
            "",
            "## By file",
            "",
            "| Release | File | Downloaded by | Downloads |",
            "| --- | --- | --- | ---: |",
        ]
        for r in releases:
            for c in r.counts:
                lines.append(
                    f"| {r.tag} | `{c.asset}` | {DOWNLOADED_BY[c.kind]} | {n(c.downloads)} |"
                )
        lines.append("")
    lines += [
        "## What the counts mean",
        "",
        "- These are GitHub's counts of release file downloads. OpenAlgo Desktop sends no "
        "usage data, so nothing here comes from the app itself.",
        "- A download is not a trader: one person may download a file more than once, and a "
        "downloaded file may never be installed.",
        "- Windows: the same setup file installs OpenAlgo Desktop and updates it from inside "
        "the app, so Windows downloads include in-app updates.",
        "- macOS: the `.dmg` is downloaded by a person to install; the `.app.tar.gz` is "
        "downloaded by the in-app update.",
        "- Linux: the AppImage is downloaded to install and by the in-app update, so its "
        "count includes updates. A `.deb` is always downloaded by a person: a `.deb` "
        "install is told about a new version and links to the release page.",
        f"- Last {WINDOW_DAYS} days: the count now minus the count recorded {WINDOW_DAYS} "
        "days ago (until that much history exists, the earliest count recorded); every "
        f"download of a release published in the last {WINDOW_DAYS} days is in it.",
        "- Update checks: each start of an installed copy, and each Check for Updates, reads "
        "`latest.json` from the newest release. They are counted apart and are in no total.",
        "- Not counted: draft releases, signatures (`.sig`) and `SHA256SUMS.txt`.",
        "- `downloads.csv` keeps one row per day, release and file with that day's "
        "cumulative count; `badges/` holds the badges the project README shows.",
        "",
    ]
    return "\n".join(lines)


def write_stats(out: str, repo: str, raw_releases: list[dict], today: str) -> Totals:
    """Read the history in `out`, add today's counts and write every output."""
    try:
        dt.date.fromisoformat(today)
    except ValueError:
        raise StatsError(f"'{today}' is not a date (YYYY-MM-DD)") from None
    releases = releases_from_api(raw_releases)
    csv_path = os.path.join(out, "downloads.csv")
    history = merge_history(read_history(csv_path), today, releases)
    t = totals(releases, downloads_in_window(history, releases, today))
    os.makedirs(os.path.join(out, "badges"), exist_ok=True)
    for name, body in badges(t).items():
        with open(os.path.join(out, "badges", f"{name}.json"), "w", encoding="utf-8") as f:
            json.dump(body, f)
            f.write("\n")
    write_history(csv_path, history)
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write(stats_page(repo, today, releases, t))
    return t


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--repo", required=True, help="owner/name")
    p.add_argument("--out", required=True, help="the stats branch's files")
    p.add_argument("--today", help="the UTC date to record (YYYY-MM-DD); default today")
    args = p.parse_args(argv)
    today = args.today or dt.datetime.now(dt.timezone.utc).date().isoformat()
    try:
        raw = fetch_releases(args.repo, os.environ.get("GITHUB_TOKEN", ""))
        t = write_stats(args.out, args.repo, raw, today)
    except StatsError as e:
        print(f"::error title=Download stats::{e}")
        return 1
    print(f"{today}: {t.downloads} downloads, {t.month} in the last {WINDOW_DAYS} days")
    for platform, label in PLATFORMS:
        print(f"  {label}: {t.by_platform[platform]}")
    if t.latest:
        print(f"  {t.latest.tag}: {t.latest.downloads()}")
    print(f"  update checks (not downloads): {t.update_checks}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
