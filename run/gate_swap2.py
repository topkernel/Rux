#!/usr/bin/env python3
"""dfx-migrate round 5: swap bespoke dfx= switch gates for unified
tracepoint gates.

Replaces every
    crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::X)
(including the multi-line formatting variants) with the migrated
event's armed check, e.g.
    crate::trace::EVENT_SCHB.armed()
Idempotent; run from the kernel source root or pass the root as argv[1].
"""
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else "kernel/src")

MAP = {
    "Schb": "EVENT_SCHB",
    "ParkDump": "EVENT_PARKDUMP",
    "SleepSnap": "EVENT_SLEEPSNAP",
    "KsProbe": "EVENT_KSPROBE",
    "WwCensus": "EVENT_WWCENSUS",
    "BinderTrace": "EVENT_BINDERTRACE",
    "SyscallTrace": "EVENT_SYSCALLTRACE",
    "InputTrace": "EVENT_INPUTTRACE",
    "ExtHunt": "EVENT_EXTHUNT",
    "SfdDbg": "EVENT_SFDDBG",
    "DbFileTrace": "EVENT_DBTRACE",
    "DataTap": "EVENT_DATATAP",
    "Ams46": "EVENT_AMS46",
    "VmaProbe": "EVENT_VMAP",
    "PoolCensus": "EVENT_POOLCENSUS",
    "SlowTask": "EVENT_SLOWTASK",
    "ExecCost": "EVENT_EXECCOST",
    "TfdCensus": "EVENT_TFDCENSUS",
    "ScmTap": "EVENT_SCMTAP",
    "SysForensic": "EVENT_SYSFORENSIC",
    "FilerdProbe": "EVENT_FILERD",
    "LostParkTrace": "EVENT_LOSTPARK",
    "AllocWatch": "EVENT_ALLOCWATCH",
    "R83HeapScan": "EVENT_R83SCAN",
    "MetaWatch": "EVENT_METAWATCH",
    "MonoWatch": "EVENT_MONOWATCH",
    "EpollScan": "EVENT_EPOLLSCAN",
    "TreeHunt": "EVENT_TREEHUNT",
    "TcpTrace": "EVENT_TCPTRACE",
    "Strace1": "EVENT_STRACE1",
    "RipSample": "EVENT_RIPSAMPLE",
    "SockSnap": "EVENT_SOCKSNAP",
    "WriteTrace": "EVENT_WRITETRACE",
    "Ledger": "EVENT_LEDGER",
    "Pid1Spin": "EVENT_PID1SPIN",
    "Steptrace": "EVENT_STEPTRACE",
    # short-form callers (use switches::enabled / enabled(...) imported):
    # none outside switches.rs after the previous rounds; kept out.
}

# Two call shapes:
#   crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::X)
#   switches::enabled(DfxSwitch::X)          (inside dfx/ modules)
#   crate::dfx::switches::enabled(DfxSwitch::X)  (partial paths)
variants = "|".join(MAP)
pats = [
    re.compile(
        r"crate::dfx::switches::enabled\(\s*"
        r"(?:crate::dfx::switches::)?DfxSwitch::(" + variants + r")"
        r"\s*,?\s*\)"
    ),
    re.compile(
        r"(?<![\w:.])switches::enabled\(\s*DfxSwitch::(" + variants + r")\s*,?\s*\)"
    ),
    re.compile(
        r"(?<![\w:.])enabled\(\s*DfxSwitch::(" + variants + r")\s*,?\s*\)"
    ),
]

total = 0
for f in ROOT.rglob("*.rs"):
    if f.name == "switches.rs":
        continue
    text = f.read_text()
    n = 0

    def sub(m):
        global n
        n += 1
        return f"crate::trace::{MAP[m.group(1)]}.armed()"

    for pat in pats:
        new = pat.sub(sub, text)
        text = new
    if n:
        f.write_text(text)
        total += n
        print(f"{f}: {n} sites")
print(f"TOTAL: {total}")
