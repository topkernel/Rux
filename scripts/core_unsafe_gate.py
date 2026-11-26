#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""Core unsafe-placement gate (seed-core pass 3, seed-core-design.md §2c.5).

Asserts the pass-3 end-state property of kernel/core/src:

  the `unsafe` KEYWORD appears only in `unsafe.rs` files and in the
  allowance list (docs/development/core-unsafe-allowance.txt).

- the keyword, not the word: the check is comment/string-aware (the same
  masking the census uses) and skips the raw identifier `r#unsafe` —
  `use super::r#unsafe::task_shared;` is a SAFE wrapper import, exactly
  the shape pass 3 leaves behind, so it does not count. Lint attribute
  names (`#![forbid(unsafe_code)]`) are one identifier (`unsafe_code`)
  and do not match either;
- `unsafe.rs` (and `r#unsafe` module files spelled otherwise — none
  today) are the allowed home;
- every other file holding the keyword must have a row in the allowance
  file: one path + a reason per line (tab-separated), the same shape as
  forbid-registry.txt. The seeded rows are the pre-collection state of
  the core at the pass-3 baseline; a subsystem's lane DELETES its rows
  when it collects — the file shrinking is the campaign's progress bar;
- --strict ignores the allowance file: only unsafe.rs may hold the
  keyword. The collected subsystems pass --strict, the rest fail with
  their honest counts.

Exit codes: 0 = every keyword-holding file is unsafe.rs or allowed,
1 = a violation (unallowed file, malformed allowance row, or --strict
hit). The default (allowance-respecting) run over the whole core is the
merge-gate check; a --strict --dir run is the per-subsystem check.

Usage:
    python3 scripts/core_unsafe_gate.py                    # whole core
    python3 scripts/core_unsafe_gate.py --dir kernel/core/src/sched
    python3 scripts/core_unsafe_gate.py --strict --dir kernel/core/src/sched
    python3 scripts/core_unsafe_gate.py --seed             # write allowance stub
"""

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORE_SRC = ROOT / "kernel" / "core" / "src"
ALLOWANCE_TXT = ROOT / "docs" / "development" / "core-unsafe-allowance.txt"

sys.path.insert(0, str(Path(__file__).resolve().parent))
from unsafe_census import mask_comments_and_strings  # noqa: E402

# the `unsafe` KEYWORD: not `r#unsafe` (the raw identifier — a wrapper
# import path, safe by construction), not `unsafe_code` (a lint name).
KEYWORD_RE = re.compile(r"(?<![A-Za-z0-9_#])unsafe(?![A-Za-z0-9_])")


def keyword_count(text):
    """Occurrences of the bare `unsafe` keyword in code (comments and
    strings masked; `r#unsafe` excluded)."""
    masked = mask_comments_and_strings(text)
    n = 0
    for m in KEYWORD_RE.finditer(masked):
        if masked[max(0, m.start() - 1): m.start()] != "#":
            n += 1
    return n


def load_allowance(failures):
    """{path: reason} — one row per line: path TAB reason."""
    rows = {}
    if not ALLOWANCE_TXT.exists():
        failures.append(f"allowance file missing: {ALLOWANCE_TXT.relative_to(ROOT)} "
                        f"(seed with --seed or collect the subsystems)")
        return rows
    for n, line in enumerate(ALLOWANCE_TXT.read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        f = [x.strip() for x in line.split("\t", 1)]
        if len(f) < 2 or not f[0] or not f[1]:
            failures.append(f"allowance:{n}: need path TAB reason, got: {line[:80]}")
            continue
        if f[0] in rows:
            failures.append(f"allowance:{n}: duplicate row for {f[0]}")
            continue
        rows[f[0]] = f[1]
    return rows


def scan(dirpath):
    """{rel path: keyword count} for every .rs file under dirpath that is
    not an unsafe.rs."""
    counts = {}
    for p in sorted(dirpath.rglob("*.rs")):
        if p.name == "unsafe.rs":
            continue
        n = keyword_count(p.read_text(encoding="utf-8", errors="replace"))
        if n:
            counts[str(p.relative_to(ROOT))] = n
    return counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", metavar="PATH", default=str(CORE_SRC.relative_to(ROOT)),
                    help="directory to check (default: the whole core)")
    ap.add_argument("--strict", action="store_true",
                    help="ignore the allowance list — only unsafe.rs may hold unsafe")
    ap.add_argument("--seed", action="store_true",
                    help="write/refresh the allowance stub from the current tree")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    dirpath = ROOT / args.dir
    if not dirpath.is_dir():
        print(f"core-unsafe gate: no such directory: {args.dir}")
        return 2

    counts = scan(dirpath)
    if args.seed:
        rows = ["# docs/development/core-unsafe-allowance.txt — the pass-3",
                "# allowance list (see scripts/core_unsafe_gate.py). One path +",
                "# reason per line; a subsystem's lane deletes its rows when it",
                "# collects. Seeded from the pass-3 baseline tree state."]
        for rel, n in sorted(counts.items()):
            floor = " (arch asm/register/CSR floor)" if "/arch/" in rel else ""
            probe = " (test/diagnostic probe compiled into the kernel)" if "/tests/" in rel or "/debug/" in rel else ""
            rows.append(f"{rel}\tpre-collection state at baseline f8e6eb94d{floor}{probe}: "
                        f"{n} unsafe keyword(s) to collect into the subsystem's unsafe.rs")
        ALLOWANCE_TXT.write_text("\n".join(rows) + "\n")
        print(f"core-unsafe gate: seeded {len(counts)} allowance row(s) -> "
              f"{ALLOWANCE_TXT.relative_to(ROOT)}")
        return 0

    failures = []
    # a missing allowance file only matters when something needs allowing
    if not ALLOWANCE_TXT.exists() and not counts:
        allowance = {}
    else:
        allowance = load_allowance(failures)

    bygroup = defaultdict(lambda: [0, 0])
    unallowed = []
    for rel, n in sorted(counts.items()):
        group = str(Path(rel).relative_to(args.dir)).split("/")[0]
        bygroup[group][0] += n
        bygroup[group][1] += 1
        reason = allowance.get(rel)
        if reason is None:
            unallowed.append(rel)
        elif args.strict:
            failures.append(f"strict: {rel} holds {n} unsafe keyword(s) "
                            f"(allowance row ignored under --strict)")
    total_kw = sum(counts.values())

    print(f"core-unsafe gate: {args.dir}: {len(counts)} file(s) outside unsafe.rs "
          f"hold {total_kw} unsafe keyword(s); "
          f"{len(counts) - len(unallowed)} allowed, {len(unallowed)} unallowed"
          + (" (--strict: allowance ignored)" if args.strict else ""))
    if args.verbose:
        for rel, n in sorted(counts.items()):
            mark = "OK" if (not args.strict and rel not in unallowed) else "FAIL"
            print(f"  {mark} {rel}: {n}")
    if unallowed:
        failures.extend(f"unallowed unsafe outside unsafe.rs: {rel} "
                        f"({counts[rel]} keyword(s))" for rel in sorted(unallowed))
    for rel in sorted(allowance):
        if rel not in counts and (ROOT / rel).is_file() and rel.startswith(args.dir):
            failures.append(f"stale allowance row (file no longer holds unsafe): {rel} "
                            f"— delete the row (the list must shrink)")
        elif not (ROOT / rel).is_file():
            failures.append(f"allowance row for a missing file: {rel}")

    if failures:
        print("\nCORE UNSAFE GATE FAILED:")
        for f in failures[:40]:
            print(f"  - {f}")
        if len(failures) > 40:
            print(f"  ... and {len(failures) - 40} more")
        return 1
    print("core-unsafe gate: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
