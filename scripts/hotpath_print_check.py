#!/usr/bin/env python3
"""Hot-path print guard (print-audit lane, 2026-11).

One un-gated pr_err! on a per-op path costs ~0.4-0.7 ms of polled-16550
UART per hit (the EXITGRP class, commit e2717687: lmbench lat_proc,fork
was 20x reference because a single ~60-char line rode every exit). This
script is the compile-time-adjacent tripwire for that class: it fails
when a NEW pr_err!/pr_warn!/pr_info!/pr_notice!/println!/print! appears
in one of the hot-path files without a `// HOTPATH-PRINT:` annotation
carrying a justification.

Rule (per file in HOT_FILES):
  - count every occurrence of the printing macros above
  - an occurrence is ANNOTATED (always allowed) if a line within 6 lines
    above it, or its own line, contains `// HOTPATH-PRINT:`
  - un-annotated occurrences are held against a per-file ratchet
    baseline (scripts/hotpath_print_baseline.txt). Count may not grow.

So the existing budgeted/gated hunt prints pass as-is; adding one more
print to these files requires either:

  1. the annotation, e.g.
         // HOTPATH-PRINT: fatal-error path only, fires once per process
         crate::pr_err!("exec: bad ELF {}", pid);
  2. demotion to pr_debug! (compiled out in release), or
  3. gating behind a DfxSwitch / budget counter — and then the
     annotation, because the script cannot see the gate mechanically.

Removing prints shrinks the count: re-run with --update to ratchet the
baseline down (commit the result).

Usage:
  scripts/hotpath_print_check.py            # check (non-zero on growth)
  scripts/hotpath_print_check.py --update   # rewrite the baseline
  scripts/hotpath_print_check.py --list     # show per-file counts
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "hotpath_print_baseline.txt")

# Files whose print sites sit on syscall/fault/sched/IPC/alloc hot paths.
HOT_FILES = [
    "kernel/src/sync/futex.rs",
    "kernel/src/ipc/binder.rs",
    "kernel/src/syscall/process.rs",
    "kernel/src/syscall/file.rs",
    "kernel/src/syscall/memory.rs",
    "kernel/src/mm/page_alloc.rs",
    "kernel/src/mm/pglist.rs",
    "kernel/src/signal.rs",
    "kernel/src/sched/sched.rs",
    "kernel/src/arch/riscv64/trap.rs",
    "kernel/src/arch/x86_64/trap.rs",
]

# Printing macros that reach the UART at the default console loglevel (6).
# pr_debug! is compiled out in release and is not counted. \b keeps
# debug_println! from matching via its "print" substring.
MACRO_RE = re.compile(r"\b(?:pr_err|pr_warn|pr_info|pr_notice|println|print)!\(")
ANNOTATION = "// HOTPATH-PRINT:"
LOOKBACK = 6  # lines above a site that may carry the annotation


def file_counts(path):
    """Return (total, annotated) macro-site counts for one file."""
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
    total = 0
    annotated = 0
    for i, line in enumerate(lines):
        for _ in MACRO_RE.finditer(line):
            total += 1
            window = lines[max(0, i - LOOKBACK) : i + 1]
            if any(ANNOTATION in w for w in window):
                annotated += 1
    return total, annotated


def load_baseline():
    base = {}
    if os.path.exists(BASELINE):
        with open(BASELINE, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                path, _, count = line.rpartition(" ")
                base[path] = int(count)
    return base


def main():
    update = "--update" in sys.argv
    list_only = "--list" in sys.argv
    base = load_baseline()
    grown = []
    shrunk = []
    newfile = []
    missing = [p for p in HOT_FILES if not os.path.exists(os.path.join(ROOT, p))]
    for missing_path in missing:
        print(f"hotpath_print_check: hot file listed but absent: {missing_path}")

    lines_out = [
        "# Ratchet baseline for scripts/hotpath_print_check.py — the number",
        "# of UN-annotated pr_err/pr_warn/pr_info/pr_notice/println/print",
        "# sites allowed per hot-path file. Regenerate with --update after",
        "# removing prints (never after adding one without a review).",
    ]
    for rel in HOT_FILES:
        path = os.path.join(ROOT, rel)
        if not os.path.exists(path):
            continue
        total, annotated = file_counts(path)
        unannotated = total - annotated
        if list_only or update:
            print(f"{rel}: {total} sites, {annotated} annotated, {unannotated} un-annotated")
        if rel not in base:
            newfile.append(rel)
        elif unannotated > base[rel]:
            grown.append((rel, base[rel], unannotated))
        elif unannotated < base[rel]:
            shrunk.append((rel, base[rel], unannotated))
        lines_out.append(f"{rel} {unannotated}")

    if update:
        with open(BASELINE, "w", encoding="utf-8") as f:
            f.write("\n".join(lines_out) + "\n")
        print(f"baseline rewritten: {BASELINE}")
        return 0

    if newfile:
        for rel in newfile:
            print(f"hotpath_print_check: new hot file, run --update to baseline it: {rel}")

    rc = 0
    for rel, old, new in grown:
        print(
            f"hotpath_print_check: FAIL {rel}: un-annotated print sites grew "
            f"{old} -> {new}. Every pr_err!/pr_warn!/pr_info! on a hot path "
            f"needs a '// HOTPATH-PRINT: <why>' annotation, a pr_debug! "
            f"demotion, or a DfxSwitch/budget gate (annotate the gate too — "
            f"the script cannot see it). One un-gated line costs ~0.5 ms of "
            f"polled-UART per hit (e271687 class)."
        )
        rc = 1
    for rel, old, new in shrunk:
        print(
            f"hotpath_print_check: {rel} improved ({old} -> {new} un-annotated); "
            f"run scripts/hotpath_print_check.py --update and commit the "
            f"lower ratchet."
        )
    if rc == 0 and not shrunk:
        print("hotpath_print_check: OK (no un-annotated print growth in hot-path files)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
