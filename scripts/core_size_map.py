#!/usr/bin/env python3
"""core_size_map.py — per-subsystem worklist for the core-size goal.

Regenerates docs/development/core-size/<subsystem>.md (design of record:
docs/architecture/seed-core-design.md §1a).

A function STAYS in the core when any of three things holds:

1. it is defined inside a hot-path file (the trap/switch/fault/walk/
   lock/alloc/tick files, HOT_FILES below);
2. a hot-path file calls it by name and it is defined elsewhere — an
   UPWARD call from a hot site, which would become an indirect call
   through the hook table after a move;
3. an existing service crate calls its name (rule added after the
   signal lane: restore_sigcontext is called from syscall, so
   it cannot leave the core without a service -> service edge).

A fifth rule exists but is NOT applied by this scan: a function a
STAYING framework file names cannot move either (sole-copy; the
drivers lane found 15 of 16 zero-unsafe files blocked this way,
2026-11-24). Name-level closure over-approximates catastrophically
(common names defined in many files), so this rule is applied by the
lanes with path-resolved call reads. The scan's cold column is
therefore an optimistic bound, not a promise.

Every other function LEAVES for its subsystem's service crate: its
callers are cold, and its own calls into the core are direct
cross-crate calls that inline.

The scan cannot see unsafe content: a function that must stay because
it is unsafe and on the F-row floor (pt_regs writes, user-stack frame
building) is marked by its lane, not here.

The call resolution is name-level with a path check, not a resolved
call graph: a cold function that a hot file reaches through a pointer,
a macro or a re-export is missed, and the lane that finds it records
why it stays. A same-named function in a module the written path can
reach counts as upward-called — that over-keeps, the safe direction.

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
    r"kernel/core/src/("
    # arch: trap entry/return, context switch, register access, user
    # access, fault entry, thread/FPU state, per-CPU setup
    r"arch/[^/]+/(trap|context|cpu|uaccess|exception|thread|smp)\.rs"
    r"|arch/[^/]+/mm/(mmu_ops|exception)\.rs"
    # the scheduler's mechanism (hot-path code keeps its topic files;
    # the build gate, not a file name, keeps unsafe out of them)
    r"|sched/(sched|unsafe)\.rs"
    # the VFS walk, the block layer, the page cache, the dcache
    r"|fs/(vfs|bio|page_cache|dentry|buffer|file|unsafe)\.rs"
    # the lock primitives and the per-CPU layer
    r"|sync/(spinlock|rwlock|percpu|seqlock|unsafe)\.rs"
    r"|interrupt/[^/]+\.rs"
    # the timekeeping core and the allocator entry points
    r"|timer\.rs|mm/(page_alloc|buddy_allocator|small_alloc|slab|page_desc|unsafe)\.rs"
    r"|process/(hotpath|unsafe)\.rs"
    r"|safety/.*\.rs)$")

FN = re.compile(
    r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:const[ \t]+)?(?:unsafe[ \t]+)?"
    r'(?:extern[ \t]+"C"[ \t]+)?fn[ \t]+([a-zA-Z_][a-zA-Z0-9_]*)', re.M)
# A path segment may be a raw identifier (`r#unsafe`: the module of
# every unsafe.rs). The `r#` is not part of the module name, so it is
# dropped before the written path is compared with a module path.
CALL = re.compile(
    r"((?:crate|super|self|rux)::(?:(?:r#)?[a-zA-Z_][a-zA-Z0-9_]*::)*)?"
    r"([a-zA-Z_][a-zA-Z0-9_]*)\s*\(")
# Subsystems that already have a service crate: only there does a name a
# service crate calls force the function to stay.
# "api" is the interface-table directory (core/src/api): every file there
# is a table or wrapper a service crate calls by name, so rule 3 applies
# to it although it is not a subsystem of its own.
HAS_CRATE = {"fs", "drivers", "mm", "sched", "ipc", "syscall", "net", "debug",
             "(root)", "process", "sync", "timer", "io_uring", "api"}

NESTED = {"fs", "drivers", "mm", "arch", "process", "ipc", "sched", "debug",
          "sync", "interrupt", "timer", "trace", "perf", "syscall", "safety",
          "net", "io_uring", "module"}

# Directories under kernel/ that hold the service crates plus the
# composition bin — every crate that calls into the core from outside
# kernel/core/src. Named one by one on purpose: a `kernel/rux-*` glob used to
# stand here, and a renamed crate would have dropped out of the scan
# without a word.
SERVICE_DIRS = ("arch", "bin", "debug", "drivers", "ext4", "fs", "io_uring",
                "ipc", "mm", "net", "process", "sched", "signal", "sync",
                "syscall", "timer")


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
    src = os.path.join(ROOT, "kernel", "core", "src")
    files = [f for f in glob.glob(os.path.join(src, "**", "*.rs"), recursive=True)
             if "ksyms_generated" not in f and os.sep + "tests" + os.sep not in f]
    texts = {p: strip(open(p, errors="replace").read()) for p in files}
    # fns: (file, name, start, end, text)
    fns = []
    fn_at = {}
    for p in files:
        s = texts[p]
        ms = list(FN.finditer(s))
        for i, m in enumerate(ms):
            start = s.count("\n", 0, m.start()) + 1
            end = s.count("\n", 0, ms[i + 1].start()) + 1 if i + 1 < len(ms) else s.count("\n") + 1
            fns.append((p, m.group(1), start, end))
            fn_at.setdefault(m.group(1), []).append((p, start))
    by_name = collections.defaultdict(list)
    for f in fns:
        by_name[f[1]].append(f)

    def modpath(p):
        return "crate::" + os.path.relpath(p, src)[:-3].replace("/", "::")

    # Names any service crate calls (rule 3): a service crate reaches the
    # core, not another service crate, so a function a service crate
    # uses must stay in the core.
    svc_used = set()
    for d in SERVICE_DIRS:
        for p in glob.glob(os.path.join(ROOT, "kernel", d, "src", "**", "*.rs"), recursive=True):
            svc_used.update(m.group(2) for m in CALL.finditer(strip(open(p, errors="replace").read())))

    # STAY = defined in a hot-path file, OR called upward by name from one,
    # OR called by name from a service crate.
    # A site in a hot file is an upward call when the callee is defined in
    # ANOTHER file and the written path reaches that file's module. This
    # is the direction that would cost an indirect call after a move;
    # a moved function's own calls into the core are direct and free.
    stay = {}
    for f in fns:
        stay[(f[0], f[1], f[2])] = HOT_FILES.search(os.path.relpath(f[0], ROOT)) is not None
    for p in files:
        if not HOT_FILES.search(os.path.relpath(p, ROOT)):
            continue
        for m in CALL.finditer(texts[p]):
            name = m.group(2)
            if name not in by_name:
                continue
            path = (m.group(1) or "").replace("r#", "")
            for (dp, dn, st, en) in by_name[name]:
                if dp == p:
                    continue
                mp = modpath(dp)
                if path == "":
                    reach = os.path.dirname(dp) == os.path.dirname(p)
                elif path.startswith("crate::"):
                    # Normalize the trailing :: so a module-qualified call
                    # (crate::debug::syscost::tick_user) reaches its own
                    # module's top-level fns: mp has no trailing separator,
                    # so the raw startswith(path) missed every such call
                    # (found by the rux-obs lane's audit, 2026-11-23: the
                    # debug worklist marked 254 lines hot where the
                    # corrected rule finds 4,512).
                    pref = path.rstrip(":")
                    reach = mp == pref or mp.startswith(pref + "::")
                else:
                    last = path.rstrip(":").split("::")[-1]
                    reach = last in mp.split("::")
                if reach:
                    stay[(dp, dn, st)] = True

    rows = collections.defaultdict(list)
    for p, name, start, end in fns:
        rel = os.path.relpath(p, src)
        hot = stay.get((p, name, start), False) or (name in svc_used and subsystem(rel) in HAS_CRATE)
        rows[subsystem(rel)].append((rel, name, start, end, hot))

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
