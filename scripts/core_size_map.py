#!/usr/bin/env python3
"""core_size_map.py — per-subsystem worklist for the core-size goal.

Regenerates docs/development/core-size/<subsystem>.md (design of record:
docs/architecture/seed-core-design.md §1a).

A function is HOT when a hot-path file mentions its name followed by
`(`. Hot-path files are the ones on the trap/switch/fault/walk/lock/
alloc/tick paths (HOT_FILES below). Hot functions stay in the core
because a hot site calls them directly; every other function leaves
for its subsystem's service crate.

The test is deliberately conservative (name-level, not resolved): a
cold function that a hot file calls through a pointer, macro or
re-export is missed, and the lane that finds it records why it stays.
A function whose name a hot file mentions for any reason (a comment
is stripped, but a same-named function in another module is not)
counts as hot — that over-keeps, which is the safe direction.

Usage:  python3 scripts/core_size_map.py            # write + summary
        python3 scripts/core_size_map.py --summary  # summary only
"""
import collections
import glob
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "development", "core-size")

HOT_FILES = re.compile(
    r"kernel/src/("
    r"arch/[^/]+/(trap|context|cpu|uaccess|exception|mm/mmu_ops|mm/exception|thread|smp)\.rs"
    r"|sched/(sched|edges)\.rs"
    r"|fs/(vfs|bio|page_cache|dentry|buffer|file)\.rs"
    r"|sync/(spinlock|rwlock|percpu|seqlock|edges)\.rs"
    r"|interrupt/[^/]+\.rs"
    r"|timer\.rs"
    r"|mm/(page_alloc|buddy_allocator|small_alloc|slab|page_desc|edges)\.rs"
    r"|process/(task_ref|unsafe_edges)\.rs"
    r"|safety/.*\.rs)$")

FN = re.compile(
    r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:const[ \t]+)?(?:unsafe[ \t]+)?"
    r'(?:extern[ \t]+"C"[ \t]+)?fn[ \t]+([a-zA-Z_][a-zA-Z0-9_]*)', re.M)
CALL = re.compile(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*\(")
NESTED = {"fs", "drivers", "mm", "arch", "process", "ipc", "sched", "debug",
          "sync", "interrupt", "timer", "trace", "perf", "syscall", "safety",
          "net", "io_uring", "module"}


def strip(s):
    s = re.sub(r'"(?:\\.|[^"\\])*"', '""', s)
    s = re.sub(r"'(?:\\.|[^'\\])'", "''", s)
    s = re.sub(r"//[^\n]*", "", s)
    return re.sub(r"/\*.*?\*/", "", s, flags=re.S)


def subsystem(rel):
    parts = rel.split("/")
    if len(parts) > 2 and parts[0] in NESTED:
        return "arch/" + parts[1] if parts[0] == "arch" else parts[0]
    return parts[0] if len(parts) > 1 else "(root)"


HEADER = """# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

"""


def main():
    src = os.path.join(ROOT, "kernel", "src")
    files = [f for f in glob.glob(os.path.join(src, "**", "*.rs"), recursive=True)
             if "ksyms_generated" not in f and os.sep + "tests" + os.sep not in f]
    texts = {p: strip(open(p, errors="replace").read()) for p in files}
    hot_used = set()
    for p in files:
        if HOT_FILES.search(os.path.relpath(p, ROOT)):
            hot_used.update(m.group(1) for m in CALL.finditer(texts[p]))

    rows = collections.defaultdict(list)
    for p in files:
        s = texts[p]
        ms = list(FN.finditer(s))
        rel = os.path.relpath(p, src)
        for i, m in enumerate(ms):
            start = s.count("\n", 0, m.start()) + 1
            end = s.count("\n", 0, ms[i + 1].start()) if i + 1 < len(ms) else s.count("\n")
            rows[subsystem(rel)].append((rel, m.group(1), start, end, m.group(1) in hot_used))

    summary_only = "--summary" in sys.argv
    if not summary_only:
        os.makedirs(OUT, exist_ok=True)
        for f in glob.glob(os.path.join(OUT, "*.md")):
            os.remove(f)
    th = tc = 0
    for sub, rs in sorted(rows.items(), key=lambda kv: -sum(r[3] - r[2] + 1 for r in kv[1] if not r[4])):
        hot = sum(r[3] - r[2] + 1 for r in rs if r[4])
        cold = sum(r[3] - r[2] + 1 for r in rs if not r[4])
        th += hot
        tc += cold
        print("%-14s hot %6d  cold %6d" % (sub, hot, cold))
        if summary_only:
            continue
        with open(os.path.join(OUT, sub.replace("/", "-") + ".md"), "w") as f:
            f.write(HEADER)
            f.write("# %s — hot %d lines (stay) / cold %d lines (leave)\n\n" % (sub, hot, cold))
            f.write("| file | fn | line | lines | hot |\n|---|---|---|---|---|\n")
            for rel, name, start, end, h in sorted(rs, key=lambda r: (r[0], r[2])):
                f.write("| %s | `%s` | %d | %d | %s |\n"
                        % (rel, name, start, end - start + 1, "yes" if h else ""))
    print("TOTAL          hot %6d  cold %6d" % (th, tc))


if __name__ == "__main__":
    main()
