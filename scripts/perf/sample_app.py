#!/usr/bin/env python3
"""Sample a running OpenAlgo Desktop for the endurance targets.

For the maintainer's runs on a real install (docs/audit/fault-drill.md):
every INTERVAL seconds it records the app's memory, open descriptors (on
Windows, handles), threads and the data folder's size to a CSV, and at the
end (Ctrl-C, or after --hours) prints whether memory and descriptors
stayed within 5% of the sample at the end of the warm-up.

It only reads: process counters from the OS, and file sizes in the data
folder. It never opens a database or any file in the folder.

    python3 scripts/perf/sample_app.py --pid 12345 --hours 8 --out soak.csv
    python3 scripts/perf/sample_app.py --summary soak.csv

Memory is the physical footprint on macOS (`footprint`, what Activity
Monitor shows), the private working set on Windows and the resident set
on Linux. Descriptors are `lsof -p` lines on macOS, /proc/<pid>/fd on
Linux, the handle count on Windows.

Standard library only; Python 3.9+.
"""

from __future__ import annotations

import argparse
import csv
import os
import platform
import re
import subprocess
import sys
import time
from pathlib import Path

FLAT = 1.05
FIELDS = ["elapsed_s", "memory_kib", "descriptors", "threads", "folder_kib"]


def data_folder() -> Path:
    system = platform.system()
    if system == "Darwin":
        return Path.home() / "Library/Application Support/com.openalgo.desktop"
    if system == "Windows":
        return Path(os.environ.get("APPDATA", "")) / "com.openalgo.desktop"
    return Path.home() / ".local/share/com.openalgo.desktop"


def run(cmd: list[str]) -> str:
    return subprocess.run(cmd, capture_output=True, text=True, timeout=60).stdout


def folder_kib(folder: Path) -> int:
    total = 0
    for root, _dirs, files in os.walk(folder):
        for f in files:
            try:
                total += (Path(root) / f).stat().st_size
            except OSError:
                pass
    return total // 1024


def macos(pid: int) -> tuple[int, int, int]:
    out = run(["footprint", str(pid)])
    m = re.search(r"Footprint:\s+([\d.]+)\s+(KB|MB|GB)", out)
    scale = {"KB": 1, "MB": 1024, "GB": 1024 * 1024}
    memory = int(float(m.group(1)) * scale[m.group(2)]) if m else 0
    if not memory:
        memory = int(run(["ps", "-o", "rss=", "-p", str(pid)]).strip() or 0)
    fds = max(len(run(["lsof", "-n", "-P", "-p", str(pid)]).splitlines()) - 1, 0)
    threads = len(run(["ps", "-M", "-p", str(pid)]).splitlines()) - 1
    return memory, fds, max(threads, 0)


def linux(pid: int) -> tuple[int, int, int]:
    status = Path(f"/proc/{pid}/status").read_text()
    rss = re.search(r"VmRSS:\s+(\d+)", status)
    threads = re.search(r"Threads:\s+(\d+)", status)
    fds = len(os.listdir(f"/proc/{pid}/fd"))
    return int(rss.group(1)) if rss else 0, fds, int(threads.group(1)) if threads else 0


def windows(pid: int) -> tuple[int, int, int]:
    ps = (
        f"$p = Get-Process -Id {pid}; "
        "\"$($p.PrivateMemorySize64) $($p.HandleCount) $($p.Threads.Count)\""
    )
    out = run(["powershell", "-NoProfile", "-Command", ps]).split()
    memory, handles, threads = (int(x) for x in out[:3])
    return memory // 1024, handles, threads


def sample(pid: int) -> tuple[int, int, int]:
    system = platform.system()
    if system == "Darwin":
        return macos(pid)
    if system == "Windows":
        return windows(pid)
    return linux(pid)


def summarise(path: Path, warm_up_minutes: float | None) -> int:
    rows = [{k: float(v) for k, v in r.items()} for r in csv.DictReader(path.open())]
    if len(rows) < 3:
        print("Not enough samples to judge.")
        return 2
    total_min = rows[-1]["elapsed_s"] / 60
    warm_min = warm_up_minutes if warm_up_minutes is not None else max(30.0, total_min / 10)
    warm = next((r for r in rows if r["elapsed_s"] >= warm_min * 60), rows[0])
    after = [r for r in rows if r["elapsed_s"] > warm["elapsed_s"]]
    if not after:
        print(f"The run is shorter than its {warm_min:.0f} min warm-up.")
        return 2
    verdict = 0
    for key in ("memory_kib", "descriptors"):
        peak = max(r[key] for r in after)
        limit = warm[key] * FLAT
        ok = peak <= limit
        verdict |= 0 if ok else 1
        print(
            f"{key}: warm-up {warm[key]:.0f}, peak after {peak:.0f}, "
            f"end {after[-1][key]:.0f} ({100 * (peak / max(warm[key], 1) - 1):+.1f}%): "
            f"{'within 5%' if ok else 'NOT within 5%'}"
        )
    hours = (after[-1]["elapsed_s"] - warm["elapsed_s"]) / 3600
    growth = (after[-1]["folder_kib"] - warm["folder_kib"]) / 1024
    print(f"data folder: {growth:+.1f} MB after the warm-up ({growth / max(hours, 0.01):+.2f} MB an hour)")
    print(f"samples: {len(rows)} over {total_min:.0f} min, warm-up {warm_min:.0f} min")
    return verdict


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--pid", type=int, help="the OpenAlgo Desktop process id")
    ap.add_argument("--hours", type=float, default=8.0)
    ap.add_argument("--interval", type=float, default=60.0, help="seconds between samples")
    ap.add_argument("--out", type=Path, default=Path("openalgo-soak.csv"))
    ap.add_argument("--folder", type=Path, default=None, help="data folder (default: the app's)")
    ap.add_argument("--warm-up", type=float, default=None, help="minutes (default: a tenth, at least 30)")
    ap.add_argument("--summary", type=Path, help="only summarise an earlier CSV")
    a = ap.parse_args()
    if a.summary:
        return summarise(a.summary, a.warm_up)
    if not a.pid:
        ap.error("--pid is required (find it in Activity Monitor or Task Manager)")
    folder = a.folder or data_folder()
    started = time.monotonic()
    with a.out.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(FIELDS)
        try:
            while time.monotonic() - started < a.hours * 3600:
                memory, fds, threads = sample(a.pid)
                elapsed = int(time.monotonic() - started)
                w.writerow([elapsed, memory, fds, threads, folder_kib(folder)])
                f.flush()
                print(f"{elapsed // 60:>5} min  memory {memory} KiB  descriptors {fds}  threads {threads}")
                time.sleep(a.interval)
        except KeyboardInterrupt:
            pass
        except (subprocess.SubprocessError, OSError, ValueError) as e:
            print(f"Sampling stopped: {e} (has the app quit?)", file=sys.stderr)
    return summarise(a.out, a.warm_up)


if __name__ == "__main__":
    sys.exit(main())
