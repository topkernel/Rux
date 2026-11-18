#!/usr/bin/env python3
"""Coverage summarizer: per-top-module table over ALL kernel source files.

Reads `llvm-cov export --summary-only` JSON and joins it against the
complete kernel/src file list, so files that never appear in the coverage
mapping (never instrumented / never referenced) still show as 0% — the
monitoring mandate is "see everything", not "see what executed".

Usage:
    python3 scripts/coverage_summary.py cov.json [file.rs ...]

Prints: overall line/region/function totals, a per-top-subsystem table,
and a per-file table sorted ascending by coverage (worst first).
"""

import json
import sys
from collections import defaultdict
from pathlib import Path


def subsystem_of(path: str) -> str:
    parts = Path(path).parts
    # kernel/src/<subsys>/... or kernel/src/<name>.rs
    if len(parts) >= 3 and parts[0] == "kernel" and parts[1] == "src":
        return parts[2] if len(parts) > 3 else "<top>"
    return "<other>"


def main() -> None:
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    cov_json = json.loads(Path(sys.argv[1]).read_text())
    all_files = [f for f in sys.argv[2:] if f.endswith(".rs")]

    # llvm-cov export --summary-only: {"data": [{"files": [{filename,
    # summary: {lines: {count, covered}, regions: {...}, functions: {...}}}],
    # "totals": {...}}]}
    data = cov_json["data"][0]["files"]
    by_path = {f["filename"]: f["summary"] for f in data}

    def stats(path):
        s = by_path.get(path)
        if s is None:
            return (0, 0, 0, 0, 0, 0)  # lines_cov, lines_total, regions, funcs, 0
        ln = s["lines"]
        rg = s["regions"]
        fn = s["functions"]
        return (ln["covered"], ln["count"], rg["covered"], rg["count"],
                fn["covered"], fn["count"])

    # Per-subsystem aggregation
    sys_agg = defaultdict(lambda: [0, 0, 0, 0, 0, 0])
    mapped = set(by_path)
    for path in all_files:
        lc, lt, rc, rt, fc, ft = stats(path)
        agg = sys_agg[subsystem_of(path)]
        agg[0] += lc; agg[1] += lt; agg[2] += rc; agg[3] += rt
        agg[4] += fc; agg[5] += ft

    unmapped = sorted(mapped - set(all_files))
    if unmapped:
        print(f"NOTE: {len(unmapped)} mapped file(s) outside kernel/src (ignored):")
        for u in unmapped[:10]:
            print(f"    {u}")
        if len(unmapped) > 10:
            print(f"    ... and {len(unmapped) - 10} more")

    tl = sum(a[1] for a in sys_agg.values())
    tlc = sum(a[0] for a in sys_agg.values())
    trt = sum(a[3] for a in sys_agg.values())
    trc = sum(a[2] for a in sys_agg.values())
    tft = sum(a[5] for a in sys_agg.values())
    tfc = sum(a[4] for a in sys_agg.values())
    files_total = len(all_files)
    files_with_cov = sum(1 for f in all_files if f in by_path)

    print(f"KERNEL COVERAGE (in-kernel test suite boot)")
    print(f"  files: {files_with_cov}/{files_total} exercised "
          f"({100.0 * files_with_cov / max(files_total, 1):.1f}%)")
    print(f"  lines: {tlc}/{tl} ({100.0 * tlc / max(tl, 1):.1f}%)  "
          f"regions: {trc}/{trt} ({100.0 * trc / max(trt, 1):.1f}%)  "
          f"functions: {tfc}/{tft} ({100.0 * tfc / max(tft, 1):.1f}%)")
    print()
    print(f"{'subsystem':14s} {'lines%':>8s} {'lines':>12s} {'regions%':>9s} "
          f"{'funcs%':>8s} {'files':>6s}")
    for subsys in sorted(sys_agg, key=lambda s: -sys_agg[s][1]):
        lc, lt, rc, rt, fc, ft = sys_agg[subsys]
        nf = sum(1 for f in all_files if subsystem_of(f) == subsys)
        print(f"{subsys:14s} {100.0 * lc / max(lt, 1):7.1f}% "
              f"{lc:6d}/{lt:<5d} {100.0 * rc / max(rt, 1):8.1f}% "
              f"{100.0 * fc / max(ft, 1):7.1f}% {nf:6d}")
    print()
    print("worst files (lines% ascending, files with any coverage):")
    rows = []
    for f in all_files:
        lc, lt, _, _, _, _ = stats(f)
        if lt == 0:
            continue
        rows.append((100.0 * lc / lt, lc, lt, f))
    rows.sort()
    for pct, lc, lt, f in rows[:25]:
        print(f"  {pct:5.1f}%  {lc:6d}/{lt:<6d} {f}")


if __name__ == "__main__":
    main()
