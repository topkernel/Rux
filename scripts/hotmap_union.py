#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""hotmap_union.py — union hotmap across per-load sampling reports.

Reads each window's report.json (perf_report.py --json --all output),
aggregates per-function self/total counts per window, applies the
hot-tier rule and emits:
  - the union hot table (per-function per-window counts)
  - the three-tier classification HOT / WARM / COLD
  - the per-subsystem summary

Tier rule (documented in docs/development/core-hotmap.md):
  hot threshold per window = max(window_samples * 0.001, 30) samples in
  the TOTAL accounting (call-chain inclusive, the port-cost proxy).
  HOT  : any window's total >= its threshold
  WARM : any window's total >= its threshold / 10 (0.01% or abs 3)
  COLD : present in the union but below WARM in every window
  absent functions are simply not in the union (never sampled).

enable_irq self-samples carry the IF=0-exit interrupt-shadow bias (the
pending LAPIC timer fires at the first sti after a long irq-off
stretch), so the report ALSO emits self counts excluding enable_irq for
per-function self ranking.

Usage: hotmap_union.py out-lm1000/out-pb1000 ... [--json OUT]
"""
import argparse
import json
import os
import re
import sys

# Subsystem bucketing by the leading path segment of the symbol name.
# Order matters (first match wins).
SUBSYS_RULES = [
    (r"^(rux::arch|x86_64|core::arch)", "arch"),
    (r"^trap_handler", "arch"),
    (r"^(rux::syscall|syscall::)", "syscall"),
    (r"^(rux::sched|sched::)", "sched"),
    (r"^(rux::mm|mm::|core::vm|page|buddy|slab|small_alloc)", "mm"),
    (r"^(rux::fs|fs::|ext4|file|inode|dentry|pipe|tmpfs|ramfs)", "fs"),
    (r"^(rux::ipc|ipc::|binder)", "ipc"),
    (r"^(rux::net|net::|tcp|udp|socket)", "net"),
    (r"^(rux::drivers|drivers::|virtio|uart|console|block|bio)", "drivers"),
    (r"^(rux::process|process::|task|pid|exit|fork|signal|wait|check_and_deliver)", "process"),
    (r"^(rux::sync|sync::|spinlock|mutex|rwlock)", "sync"),
    (r"^(rux::timer|timer::|hrtimer|jiffies|itimer)", "timer"),
    (r"^(rux::perf|perf::)", "perf"),
    (r"^(rux::trace|trace::)", "debug"),
    (r"^(rux::debug|debug::|dfx|seccomp)", "debug"),
    (r"^(rux::kernel|crate::|rux::)", "core-misc"),
    (r"^(core|alloc|compiler_builtins|hash_map|slice|mem|ptr|str|num|option|result|iter|ops::|fmt|memset|memcpy|strlen|bcmp)", "rust-core"),
]


def subsystem(name):
    # Generic closures like "<rux::fs::types::File>::read_slice" carry the
    # subsystem inside the <>: try the raw name, then the unwrapped form.
    for cand in (name, name.lstrip("<").split(">")[0]):
        for pat, sub in SUBSYS_RULES:
            if re.search(pat, cand):
                return sub
    return "other"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+", help="window out-dirs with report.json")
    ap.add_argument("--json", default=None, help="write union JSON here")
    ap.add_argument("--csv", default=None, help="write union CSV here")
    args = ap.parse_args()

    wins = {}
    meta = {}
    for d in args.dirs:
        path = os.path.join(d, "report.json")
        if not os.path.exists(path):
            print("hotmap_union: missing %s" % path, file=sys.stderr)
            continue
        j = json.load(open(path))
        tag = os.path.basename(d.rstrip("/")).replace("out-", "")
        wins[tag] = j["functions"]
        meta[tag] = {"samples": j["samples"], "lost": j["lost"],
                     "truncated": j["truncated_records"]}

    # thresholds per window
    thr = {t: max(m["samples"] * 0.001, 30) for t, m in meta.items()}
    warmthr = {t: max(m["samples"] * 0.0001, 3) for t, m in meta.items()}
    print("# window samples/thresholds: " + "  ".join(
        "%s: %d/%.0f(warm %.0f)" % (t, meta[t]["samples"], thr[t], warmthr[t])
        for t in sorted(meta)))

    union = {}
    for tag, funcs in wins.items():
        for f in funcs:
            u = union.setdefault(f["name"], {"self": {}, "total": {}})
            u["self"][tag] = u["self"].get(tag, 0) + f["self"]
            u["total"][tag] = u["total"].get(tag, 0) + f["total"]

    rows = []
    for name, u in union.items():
        hot = any(u["total"].get(t, 0) >= thr[t] for t in thr)
        warm = (not hot) and any(u["total"].get(t, 0) >= warmthr[t]
                                 for t in warmthr)
        tier = "HOT" if hot else ("WARM" if warm else "COLD")
        rows.append({"name": name, "tier": tier,
                     "subsystem": subsystem(name),
                     "total": u["total"], "self": u["self"],
                     "total_sum": sum(u["total"].values()),
                     "self_sum": sum(u["self"].values())})
    rows.sort(key=lambda r: (-r["total_sum"], r["name"]))

    counts = {"HOT": 0, "WARM": 0, "COLD": 0}
    subs = {}
    for r in rows:
        counts[r["tier"]] += 1
        s = subs.setdefault(r["subsystem"], {"HOT": 0, "WARM": 0, "COLD": 0,
                                             "samples": 0})
        s[r["tier"]] += 1
        s["samples"] += r["total_sum"]

    print("# union: %d functions  HOT=%d WARM=%d COLD=%d"
          % (len(rows), counts["HOT"], counts["WARM"], counts["COLD"]))
    print("# %-14s %5s %5s %5s %9s" %
          ("subsystem", "HOT", "WARM", "COLD", "samples"))
    for s in sorted(subs, key=lambda k: -subs[k]["samples"]):
        print("# %-14s %5d %5d %5d %9d" %
              (s, subs[s]["HOT"], subs[s]["WARM"], subs[s]["COLD"],
               subs[s]["samples"]))

    print()
    hdr = ["total_sum"] + ["tot:" + t for t in sorted(meta)] + \
          ["self:" + t for t in sorted(meta)] + ["tier", "subsystem", "function"]
    print("%-9s " % "total_sum" +
          " ".join("%8s" % ("tot:" + t) for t in sorted(meta)) + " " +
          " ".join("%8s" % ("slf:" + t) for t in sorted(meta)) +
          " %-4s %-10s %s" % ("tier", "subsystem", "function"))
    for r in rows[:args.top if hasattr(args, 'top') else 250]:
        print("%9d " % r["total_sum"] +
              " ".join("%8d" % r["total"].get(t, 0) for t in sorted(meta)) +
              " " +
              " ".join("%8d" % r["self"].get(t, 0) for t in sorted(meta)) +
              " %-4s %-10s %s" % (r["tier"], r["subsystem"], r["name"]))

    if args.json:
        with open(args.json, "w") as fh:
            json.dump({"meta": meta, "thresholds": thr,
                       "warm_thresholds": warmthr, "counts": counts,
                       "subsystems": subs, "functions": rows}, fh, indent=1)
    if args.csv:
        import csv
        with open(args.csv, "w", newline="") as fh:
            w = csv.writer(fh)
            w.writerow(["function", "tier", "subsystem", "total_sum",
                        "self_sum"] +
                       ["total_" + t for t in sorted(meta)] +
                       ["self_" + t for t in sorted(meta)])
            for r in rows:
                w.writerow([r["name"], r["tier"], r["subsystem"],
                            r["total_sum"], r["self_sum"]] +
                           [r["total"].get(t, 0) for t in sorted(meta)] +
                           [r["self"].get(t, 0) for t in sorted(meta)])


if __name__ == "__main__":
    main()
