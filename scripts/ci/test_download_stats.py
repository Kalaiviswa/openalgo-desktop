#!/usr/bin/env python3
"""Tests of scripts/ci/download_stats.py and the workflow that runs it.

    python3 scripts/ci/test_download_stats.py

No network: releases are fabricated in the GitHub API's shape, and the
workflow and README.md are read as text, so no YAML library is needed.
"""

from __future__ import annotations

import csv
import json
import os
import re
import sys
import tempfile
import unittest
import urllib.parse

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import download_stats as ds  # noqa: E402

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "download-stats.yml")
README = os.path.join(ROOT, "README.md")
REPO = "marketcalls/openalgo-desktop"


def asset(name: str, downloads: int) -> dict:
    return {"name": name, "download_count": downloads}


def release_assets(version: str, n: int) -> list[dict]:
    """Every file a release carries, as the release workflow names them.
    Counted files get n, n+1, ...; the others a large count that must never
    reach a total."""
    p = f"OpenAlgo.Desktop_{version}"
    counted = [
        f"{p}_x64-setup.exe",
        f"{p}_aarch64.dmg",
        f"{p}_x64.dmg",
        f"{p}_aarch64.app.tar.gz",
        f"{p}_x64.app.tar.gz",
        f"{p}_amd64.AppImage",
        f"{p}_aarch64.AppImage",
        f"{p}_amd64.deb",
        f"{p}_arm64.deb",
    ]
    ignored = [
        f"{p}_x64-setup.exe.sig",
        f"{p}_aarch64.app.tar.gz.sig",
        f"{p}_x64.app.tar.gz.sig",
        f"{p}_amd64.AppImage.sig",
        f"{p}_aarch64.AppImage.sig",
        f"{p}_amd64.deb.sig",
        "SHA256SUMS.txt",
    ]
    return (
        [asset(name, n + i) for i, name in enumerate(counted)]
        + [asset(name, 100_000) for name in ignored]
        + [asset("latest.json", 5_000)]
    )


def api_release(tag: str, published: str, n: int, *, draft=False, prerelease=False) -> dict:
    return {
        "tag_name": tag,
        "draft": draft,
        "prerelease": prerelease,
        "published_at": None if draft else f"{published}T04:30:00Z",
        "created_at": f"{published}T04:00:00Z",
        "assets": release_assets(tag[1:], n),
    }


# Counted files of release_assets(n=10): Windows 10; macOS 11+12 (.dmg) and
# 13+14 (.app.tar.gz); Linux 15+16 (AppImage) and 17+18 (.deb).
N10 = {"windows": 10, "macos": 11 + 12 + 13 + 14, "linux": 15 + 16 + 17 + 18}
N10_TOTAL = sum(N10.values())


class Classification(unittest.TestCase):
    def test_every_release_file_is_classified_by_its_name(self) -> None:
        table = {
            "OpenAlgo.Desktop_1.0.0_x64-setup.exe": ("windows", ds.INSTALL_OR_UPDATE),
            "OpenAlgo.Desktop_1.0.0_aarch64.dmg": ("macos", ds.INSTALL),
            "OpenAlgo.Desktop_1.0.0_x64.dmg": ("macos", ds.INSTALL),
            "OpenAlgo.Desktop_1.0.0_aarch64.app.tar.gz": ("macos", ds.UPDATE),
            "OpenAlgo.Desktop_1.0.0_x64.app.tar.gz": ("macos", ds.UPDATE),
            "OpenAlgo.Desktop_1.0.0_amd64.AppImage": ("linux", ds.INSTALL_OR_UPDATE),
            "OpenAlgo.Desktop_1.0.0_aarch64.AppImage": ("linux", ds.INSTALL_OR_UPDATE),
            "OpenAlgo.Desktop_1.0.0_amd64.deb": ("linux", ds.INSTALL),
            "OpenAlgo.Desktop_1.0.0_arm64.deb": ("linux", ds.INSTALL),
            "latest.json": ("", ds.UPDATE_CHECK),
        }
        for name, expected in table.items():
            with self.subTest(name=name):
                self.assertEqual(ds.classify(name), expected)

    def test_signatures_checksums_and_other_files_are_not_counted(self) -> None:
        for name in [
            "OpenAlgo.Desktop_1.0.0_x64-setup.exe.sig",
            "OpenAlgo.Desktop_1.0.0_aarch64.app.tar.gz.sig",
            "OpenAlgo.Desktop_1.0.0_amd64.AppImage.sig",
            "OpenAlgo.Desktop_1.0.0_amd64.deb.sig",
            "SHA256SUMS.txt",
            "latest.json.sig",
            "OpenAlgo.Desktop_1.0.0_x64.exe",
            "OpenAlgo.Desktop_1.0.0_x64_en-US.msi",
            "source.zip",
        ]:
            with self.subTest(name=name):
                self.assertIsNone(ds.classify(name))


class NumberFormat(unittest.TestCase):
    def test_counts_read_like_a_badge_and_never_round_up(self) -> None:
        table = {
            0: "0",
            7: "7",
            999: "999",
            1000: "1k",
            1050: "1k",
            1234: "1.2k",
            1999: "1.9k",
            12_345: "12.3k",
            99_999: "99.9k",
            100_000: "100k",
            123_456: "123k",
            999_999: "999k",
            1_000_000: "1M",
            1_250_000: "1.2M",
            2_500_000_000: "2.5B",
            1_500_000_000_000: "1500B",
        }
        for n, text in table.items():
            with self.subTest(n=n):
                self.assertEqual(ds.readable(n), text)


class Stats(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.out = self.tmp.name

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_on(self, raw: list[dict], today: str) -> ds.Totals:
        return ds.write_stats(self.out, REPO, raw, today)

    def badge(self, name: str) -> dict:
        with open(os.path.join(self.out, "badges", f"{name}.json"), encoding="utf-8") as f:
            return json.load(f)

    def history(self) -> list[dict]:
        with open(os.path.join(self.out, "downloads.csv"), newline="", encoding="utf-8") as f:
            return list(csv.DictReader(f))

    def page(self) -> str:
        with open(os.path.join(self.out, "README.md"), encoding="utf-8") as f:
            return f.read()

    def files(self) -> dict[str, str]:
        out = {}
        for root, _, names in os.walk(self.out):
            for name in names:
                with open(os.path.join(root, name), encoding="utf-8") as f:
                    out[os.path.relpath(os.path.join(root, name), self.out)] = f.read()
        return out

    def test_totals_add_installs_and_updates_of_every_platform_and_no_update_check(self) -> None:
        t = self.run_on([api_release("v1.0.0", "2026-10-20", 10)], "2026-10-22")
        self.assertEqual(t.by_platform, N10)
        self.assertEqual(t.downloads, N10_TOTAL)
        self.assertEqual(t.update_checks, 5_000)
        self.assertEqual(t.latest.tag, "v1.0.0")
        self.assertEqual(
            self.badge("downloads"),
            {"schemaVersion": 1, "label": "desktop downloads", "message": "126", "color": "blue"},
        )
        self.assertEqual(self.badge("downloads-month")["label"], "desktop downloads/month")
        self.assertEqual(self.badge("windows")["message"], "10")
        self.assertEqual(self.badge("macos")["message"], "50")
        self.assertEqual(self.badge("linux")["message"], "66")
        self.assertEqual(self.badge("latest-version")["label"], "v1.0.0 downloads")

    def test_the_latest_version_is_the_newest_final_release(self) -> None:
        t = self.run_on(
            [
                api_release("v1.1.0-beta.1", "2026-11-05", 1, prerelease=True),
                api_release("v1.0.0", "2026-10-20", 10),
                api_release("v0.9.0", "2026-09-01", 1000),
            ],
            "2026-11-06",
        )
        self.assertEqual(t.latest.tag, "v1.0.0")
        self.assertEqual(self.badge("latest-version")["message"], "126")
        # A pre-release's downloads are still downloads.
        self.assertEqual(t.downloads, N10_TOTAL + 9 * 1 + 36 + 9 * 1000 + 36)
        self.assertIn("v1.1.0-beta.1 (pre-release)", self.page())

    def test_no_release_at_all_writes_zeros(self) -> None:
        t = self.run_on([], "2026-10-22")
        self.assertEqual((t.downloads, t.month, t.update_checks, t.latest), (0, 0, 0, None))
        for name in ds.BADGES:
            with self.subTest(badge=name):
                self.assertEqual(self.badge(name)["message"], "0")
        self.assertEqual(self.history(), [])
        self.assertIn("No release has been published yet.", self.page())

    def test_drafts_are_skipped(self) -> None:
        t = self.run_on(
            [
                api_release("v1.1.0", "2026-10-25", 500, draft=True),
                api_release("v1.0.0", "2026-10-20", 10),
            ],
            "2026-10-26",
        )
        self.assertEqual(t.downloads, N10_TOTAL)
        self.assertEqual(t.latest.tag, "v1.0.0")
        self.assertEqual({row["tag"] for row in self.history()}, {"v1.0.0"})
        self.assertNotIn("v1.1.0", self.page())

    def test_the_history_keeps_counted_files_and_update_checks_only(self) -> None:
        self.run_on([api_release("v1.0.0", "2026-10-20", 10)], "2026-10-22")
        rows = self.history()
        self.assertEqual(len(rows), 10)  # nine downloads and latest.json
        self.assertFalse(any(r["asset"].endswith(".sig") for r in rows))
        check = [r for r in rows if r["kind"] == ds.UPDATE_CHECK]
        self.assertEqual(
            check,
            [
                {
                    "date": "2026-10-22",
                    "tag": "v1.0.0",
                    "asset": "latest.json",
                    "platform": "",
                    "kind": "update-check",
                    "downloads": "5000",
                }
            ],
        )

    def test_a_second_run_on_the_same_day_replaces_that_days_rows(self) -> None:
        self.run_on([api_release("v1.0.0", "2026-10-20", 10)], "2026-10-21")
        self.run_on([api_release("v1.0.0", "2026-10-20", 20)], "2026-10-22")
        self.run_on([api_release("v1.0.0", "2026-10-20", 30)], "2026-10-22")
        rows = self.history()
        by_date: dict[str, list[dict]] = {}
        for r in rows:
            by_date.setdefault(r["date"], []).append(r)
        self.assertEqual(sorted(by_date), ["2026-10-21", "2026-10-22"])
        self.assertEqual(len(by_date["2026-10-22"]), 10)
        setup = [r for r in by_date["2026-10-22"] if r["asset"].endswith("-setup.exe")]
        self.assertEqual([r["downloads"] for r in setup], ["30"])
        # The earlier day is kept as it was.
        setup = [r for r in by_date["2026-10-21"] if r["asset"].endswith("-setup.exe")]
        self.assertEqual([r["downloads"] for r in setup], ["10"])

    def test_a_run_with_nothing_new_writes_the_same_files(self) -> None:
        raw = [api_release("v1.0.0", "2026-10-20", 10)]
        self.run_on(raw, "2026-10-22")
        before = self.files()
        self.run_on(raw, "2026-10-22")
        self.assertEqual(self.files(), before)

    def test_a_history_this_script_did_not_write_is_refused(self) -> None:
        with open(os.path.join(self.out, "downloads.csv"), "w", encoding="utf-8") as f:
            f.write("day,count\n2026-10-01,5\n")
        with self.assertRaisesRegex(ds.StatsError, "refusing to rewrite"):
            self.run_on([], "2026-10-22")

    def test_a_date_that_is_not_one_is_refused(self) -> None:
        with self.assertRaisesRegex(ds.StatsError, "not a date"):
            self.run_on([], "22-10-2026")

    def test_the_page_explains_what_the_counts_mean(self) -> None:
        self.run_on([api_release("v1.0.0", "2026-10-20", 10)], "2026-10-22")
        page = self.page()
        self.assertIn("read on 2026-10-22 (UTC)", page)
        self.assertIn("| v1.0.0 | 2026-10-20 | 10 | 23 | 27 | 31 | 35 | 126 | 5,000 |", page)
        self.assertIn("OpenAlgo Desktop sends no usage data", page)
        self.assertIn("Windows downloads include in-app updates", page)
        self.assertIn("so its count includes updates", page)
        self.assertNotIn("SHA256SUMS.txt` |", page)


class LastThirtyDays(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.out = self.tmp.name

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def month(self, raw: list[dict], today: str) -> int:
        return ds.write_stats(self.out, REPO, raw, today).month

    def test_the_first_count_of_an_older_release_has_no_history_to_compare(self) -> None:
        self.assertEqual(self.month([api_release("v0.9.0", "2026-08-01", 10)], "2026-10-22"), 0)

    def test_a_release_published_in_the_window_counts_every_download(self) -> None:
        self.assertEqual(
            self.month([api_release("v1.0.0", "2026-10-20", 10)], "2026-10-22"), N10_TOTAL
        )

    def test_the_count_thirty_days_ago_is_subtracted(self) -> None:
        old = "2026-08-01"
        self.month([api_release("v0.9.0", old, 10)], "2026-09-01")  # cutoff of 10-01 is 09-01
        self.month([api_release("v0.9.0", old, 15)], "2026-09-10")
        # Each of the nine files gained 10 since 09-01, and 5 since 09-10.
        self.assertEqual(self.month([api_release("v0.9.0", old, 20)], "2026-10-01"), 90)

    def test_a_shorter_history_compares_with_its_earliest_count(self) -> None:
        old = "2026-08-01"
        self.month([api_release("v0.9.0", old, 10)], "2026-10-10")
        self.month([api_release("v0.9.0", old, 12)], "2026-10-15")
        self.assertEqual(self.month([api_release("v0.9.0", old, 13)], "2026-10-22"), 27)

    def test_a_file_added_after_the_base_day_counts_from_zero(self) -> None:
        old = "2026-08-01"
        first = api_release("v0.9.0", old, 10)
        first["assets"] = [a for a in first["assets"] if not a["name"].endswith(".deb")]
        self.month([first], "2026-09-01")
        later = api_release("v0.9.0", old, 10)
        self.assertEqual(self.month([later], "2026-10-01"), 17 + 18)

    def test_a_count_that_went_down_adds_nothing(self) -> None:
        old = "2026-08-01"
        self.month([api_release("v0.9.0", old, 50)], "2026-09-01")
        self.assertEqual(self.month([api_release("v0.9.0", old, 10)], "2026-10-01"), 0)


class Pages(unittest.TestCase):
    def test_every_page_is_read_and_the_token_goes_only_to_the_api(self) -> None:
        calls: list[tuple[str, str]] = []
        pages = {
            f"{ds.API}/repos/{REPO}/releases?per_page=100": (
                [api_release("v1.0.0", "2026-10-20", 1)],
                f"{ds.API}/repositories/1/releases?per_page=100&page=2",
            ),
            f"{ds.API}/repositories/1/releases?per_page=100&page=2": (
                [api_release("v0.9.0", "2026-09-20", 1)],
                None,
            ),
        }

        def get(url: str, token: str):
            calls.append((url, token))
            return pages[url]

        raw = ds.fetch_releases(REPO, "t0ken", get=get)
        self.assertEqual([r["tag_name"] for r in raw], ["v1.0.0", "v0.9.0"])
        self.assertEqual([c[1] for c in calls], ["t0ken", "t0ken"])

    def test_the_next_page_must_be_on_the_api_host(self) -> None:
        header = (
            '<https://api.github.com/repositories/1/releases?page=2>; rel="next", '
            '<https://api.github.com/repositories/1/releases?page=4>; rel="last"'
        )
        self.assertEqual(
            ds.next_page(header), "https://api.github.com/repositories/1/releases?page=2"
        )
        self.assertIsNone(ds.next_page('<https://api.github.com/x?page=1>; rel="prev"'))
        self.assertIsNone(ds.next_page(""))
        with self.assertRaisesRegex(ds.StatsError, "not on"):
            ds.next_page('<https://example.com/releases?page=2>; rel="next"')

    def test_a_repository_name_that_is_not_owner_and_name_is_refused(self) -> None:
        with self.assertRaisesRegex(ds.StatsError, "owner/name"):
            ds.fetch_releases("marketcalls/openalgo-desktop/../x", "", get=None)


class Workflow(unittest.TestCase):
    """download-stats.yml commits to the stats branch only, with the least
    permission, and README.md shows badges the script writes."""

    def setUp(self) -> None:
        with open(WORKFLOW, encoding="utf-8") as f:
            self.text = f.read()
        # The workflow without its comments, which name master to say why
        # it is never touched.
        self.code = "\n".join(
            line for line in self.text.splitlines() if not line.lstrip().startswith("#")
        )

    def test_runs_every_six_hours_and_by_hand(self) -> None:
        self.assertRegex(self.text, r'\n    - cron: "\d+ \*/6 \* \* \*"')
        self.assertIn("\n  workflow_dispatch:\n", self.text)

    def test_only_the_job_may_write_and_only_contents(self) -> None:
        self.assertIn("\npermissions: {}\n", self.text)
        grants = re.findall(r"\n\s+permissions:\n((?:\s+\w[\w-]*: \w+\n)+)", self.text)
        self.assertEqual(grants, ["      contents: write\n"])

    def test_pushes_only_to_the_stats_branch_and_never_forces(self) -> None:
        pushes = re.findall(r"git push[^\n]*", self.code)
        self.assertEqual(pushes, ['git push origin "${commit}:refs/heads/stats"'])
        for forbidden in ["--force", "master", "main", "README.md", "git commit ", "git checkout"]:
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, self.code)

    def test_actions_are_pinned_by_commit(self) -> None:
        uses = re.findall(r"uses: (\S+)", self.text)
        self.assertTrue(uses)
        for ref in uses:
            with self.subTest(uses=ref):
                self.assertRegex(ref, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")

    def test_the_readme_shows_the_two_download_badges_from_the_stats_branch(self) -> None:
        with open(README, encoding="utf-8") as f:
            readme = f.read()
        raw = f"https://raw.githubusercontent.com/{REPO}/stats/badges/"
        shown = []
        for url, link in re.findall(
            r"\[!\[[^\]]*\]\((https://img\.shields\.io/endpoint\?url=[^)]+)\)\]\(([^)]+)\)",
            readme,
        ):
            target = urllib.parse.unquote(url.split("?url=", 1)[1])
            self.assertTrue(target.startswith(raw), target)
            self.assertEqual(link, f"https://github.com/{REPO}/tree/stats")
            shown.append(target[len(raw) : -len(".json")])
        self.assertEqual(shown, ["downloads", "downloads-month"])
        for name in shown:
            self.assertIn(name, ds.BADGES)


if __name__ == "__main__":
    unittest.main(verbosity=2)
