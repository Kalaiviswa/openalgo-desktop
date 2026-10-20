#!/usr/bin/env python3
"""Line-coverage floors for the critical areas of the Rust code.

    python3 scripts/ci/coverage_areas.py <coverage-summary.json> [<areas.json>]

The rust-coverage job in .github/workflows/ci.yml runs it on the summary
`cargo llvm-cov report --json --summary-only` writes, after the total
ratchet (.github/coverage-baseline.txt). The total can rise while an area
that decides a trader's money falls, so each area named in
.github/coverage-areas.json has a floor of its own: the order path, the
sandbox, strategies, scalping, risk rules, security and the rest listed
there.

An area is a list of path patterns relative to src-tauri/src: `*` stays
within one path segment, `**` spans any number of them. Its coverage is
the covered lines over the instrumented lines of every file it matches.
The check fails when an area is more than 0.1 points under its floor (the
total ratchet's tolerance), or matches no file at all (a moved folder must
not drop its floor unseen). An area at least 1 point above its floor gets
a notice to raise it. The table goes to stdout and, in CI, to the job
summary.
"""

from __future__ import annotations

import json
import os
import re
import sys

DEFAULT_AREAS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".github", "coverage-areas.json")
TOLERANCE = 0.1
RAISE_AT = 1.0
ROOT = "/src-tauri/src/"


def pattern(glob: str) -> re.Pattern[str]:
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out += "(?:.*/)?"
            i += 3
        elif glob.startswith("**", i):
            out += ".*"
            i += 2
        elif glob[i] == "*":
            out += "[^/]*"
            i += 1
        else:
            out += re.escape(glob[i])
            i += 1
    return re.compile("^" + out + "$")


def relative(filename: str) -> str | None:
    """The path under src-tauri/src, or None for a file outside it."""
    i = filename.replace("\\", "/").rfind(ROOT)
    return filename.replace("\\", "/")[i + len(ROOT) :] if i >= 0 else None


def measure(summary: dict, areas: dict) -> list[dict]:
    files = []
    for f in summary["data"][0]["files"]:
        rel = relative(f.get("filename", ""))
        if rel is not None:
            lines = f["summary"]["lines"]
            files.append((rel, lines["count"], lines["covered"]))
    rows = []
    for name, area in areas.items():
        pats = [pattern(p) for p in area["paths"]]
        hit = [(c, v) for rel, c, v in files if any(p.match(rel) for p in pats)]
        count = sum(c for c, _ in hit)
        covered = sum(v for _, v in hit)
        rows.append(
            {
                "name": name,
                "files": len(hit),
                "lines": count,
                "percent": 100.0 * covered / count if count else None,
                "floor": float(area["lines"]),
            }
        )
    return rows


def verdict(rows: list[dict]) -> tuple[list[str], list[str]]:
    errors, notices = [], []
    for r in rows:
        if r["percent"] is None:
            errors.append(f"{r['name']}: no instrumented file matches its paths; update .github/coverage-areas.json")
        elif r["percent"] + TOLERANCE < r["floor"]:
            errors.append(f"{r['name']}: line coverage {r['percent']:.2f}% is below its floor {r['floor']:.1f}%")
        elif r["percent"] >= r["floor"] + RAISE_AT:
            notices.append(f"{r['name']}: line coverage {r['percent']:.2f}% is at least {RAISE_AT:.0f} point above its floor {r['floor']:.1f}%; raise it")
    return errors, notices


def table(rows: list[dict]) -> str:
    out = ["| Area | Files | Lines | Covered | Floor |", "| --- | ---: | ---: | ---: | ---: |"]
    for r in rows:
        pct = "none" if r["percent"] is None else f"{r['percent']:.2f}%"
        out.append(f"| {r['name']} | {r['files']} | {r['lines']} | {pct} | {r['floor']:.1f}% |")
    return "\n".join(out)


def main(argv: list[str]) -> int:
    if len(argv) not in (1, 2):
        print(__doc__)
        return 2
    with open(argv[0], encoding="utf-8") as f:
        summary = json.load(f)
    with open(argv[1] if len(argv) == 2 else DEFAULT_AREAS, encoding="utf-8") as f:
        areas = json.load(f)["areas"]
    rows = measure(summary, areas)
    text = table(rows)
    print(text)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            f.write("### Rust coverage by critical area (lines)\n\n" + text + "\n")
    errors, notices = verdict(rows)
    for n in notices:
        print(f"::notice title=Coverage area::{n}")
    for e in errors:
        print(f"::error title=Coverage area::{e}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
