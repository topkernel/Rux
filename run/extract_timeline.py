#!/usr/bin/env python3
"""Extract heap-corruption event timeline from OH serial logs (b86-b90).

Serial logs carry no timestamps, so line numbers are the time axis.
Emits per-log TSV event streams + per-event context windows for the
timeline doc.  Read-only over the evidence; outputs go to run/timeline/.
"""
import os
import re
import sys
import json

RUN = "/home/william/rux-agents/oh-display/run"
OUT = "/home/william/rux-agents/heap-forensics/run/timeline"
LOGS = [f"oh-b{n}-serial.log" for n in (86, 87, 88, 89, 90)]

PATTERNS = [
    # (event-type, compiled-regex, group-names)
    ("fork",   re.compile(r"^fork: parent=(\d+), child=(\d+), flags=(0x[0-9a-f]+)$"), ("parent","child","flags")),
    ("exec",   re.compile(r"^exec: pid=(\d+) path=(\S+)"), ("pid","path")),
    ("gp",     re.compile(r"^trap: #GP at rip=(0x[0-9a-f]+), mode=(\w+), pid (\d+)"), ("rip","mode","pid")),
    ("segv",   re.compile(r"^pagefault: Segfault at (0x[0-9a-f]+), rip=(0x[0-9a-f]+), rsp=(0x[0-9a-f]+), pid=(\d+), mode=(\w+)"), ("cr2","rip","rsp","pid","mode")),
    ("usegv",  re.compile(r"^USEGV-FOR: pid=(\d+) rip=(0x[0-9a-f]+) cr2=(0x[0-9a-f]+) rsp=(0x[0-9a-f]+) rbp=(0x[0-9a-f]+)"), ("pid","rip","cr2","rsp","rbp")),
    ("bc0",    re.compile(r"^binder: pid (\d+) tid (\d+) unknown BC command (0x[0-9a-f]+), stopping"), ("pid","tid","cmd")),
    ("pte_ow", re.compile(r"^PTE-OVERWRITE-LIVE va=(0x[0-9a-f]+) old=(0x[0-9a-f]+) new=(0x[0-9a-f]+) pid=(\d+) comm=(\S+) ctx=(\d+) (\S+)"), ("va","old","new","pid","comm","ctx","tag")),
    ("exit",   re.compile(r"^exit: pid=(\d+), tgid=(\d+), exit_code=(-?\d+), ppid=(\d+) \((thread|leader)\)"), ("pid","tgid","code","ppid","role")),
    ("death",  re.compile(r"^SIGDEATH: pid=(\d+) tgid=(\d+) comm=Ok\(\"([^\"]+)\"\) sig=(\d+)"), ("pid","tgid","comm","sig")),
    ("bopen",  re.compile(r"^binder: pid (\d+) opened the device \(proc (\d+)\)"), ("pid","proc")),
    ("bmap",   re.compile(r"^binder: proc (\d+) mapped (0x[0-9a-f]+) bytes at (0x[0-9a-f]+)"), ("proc","size","vaddr")),
    ("stall",  re.compile(r"^EXT4-BIGLOCK-STALL.*"), ()),
]

def parse_log(path):
    events = []
    with open(path, "r", errors="replace") as f:
        for lineno, line in enumerate(f, 1):
            line = line.rstrip("\n")
            for typ, rx, groups in PATTERNS:
                m = rx.match(line)
                if m:
                    ev = {"t": lineno, "type": typ, "raw": line[:400]}
                    for name, val in zip(groups, m.groups()):
                        ev[name] = val
                    events.append(ev)
                    break
    return events

def main():
    os.makedirs(OUT, exist_ok=True)
    summary = {}
    for name in LOGS:
        path = os.path.join(RUN, name)
        tag = name.split("-serial")[0]
        evs = parse_log(path)
        with open(os.path.join(OUT, f"{tag}-events.json"), "w") as f:
            json.dump(evs, f, indent=1)
        counts = {}
        for e in evs:
            counts[e["type"]] = counts.get(e["type"], 0) + 1
        summary[tag] = {"events": len(evs), "counts": counts}
        print(f"{tag}: {len(evs)} events {counts}")
    with open(os.path.join(OUT, "summary.json"), "w") as f:
        json.dump(summary, f, indent=1)

if __name__ == "__main__":
    main()
