#!/usr/bin/env python3
"""ww-gdbscript.py — generate the gdb command file for the wildwrite hunt.

Hardware watchpoints on the hpet victim statics, addresses derived from
`nm` of the CURRENT build (the wildwatch commit moved the quiet statics
into hpet::GUARD — a page-exclusive cluster; the watch set therefore
targets the GUARD members directly so watchpoint boots work on both the
pre- and post-relocation layouts).

Note: do NOT combine with dfx=wildwatch — the RO trap faults the store
before it lands, so the watchpoint would never see a value change.

Legit writers post-attach are only the heal paths in
validated_read_base / read_time_nominal (recognizable rip). Anything
else IS the wild writer.
"""

import re
import subprocess
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12371
ROOT = "/home/william/rux-agents/wildwrite-hunt"
# Frozen copy of the hunt kernel ELF — must stay in sync with the
# ww-boot.sh KERNEL frozen bzImage (a rebuild for unrelated mm work
# would move GUARD and put the watchpoints at stale addresses).
ELF = f"{ROOT}/run/ww-hunt-frozen-rux.elf"

nm = subprocess.run(["nm", ELF], capture_output=True, text=True, check=True).stdout

def addr(sym_suffix):
    for line in nm.splitlines():
        m = re.match(r"^([0-9a-f]+) [bBdDtTrR] (\S+)$", line)
        if m and m.group(2).endswith(sym_suffix):
            return int(m.group(1), 16)
    raise SystemExit(f"symbol ending in {sym_suffix} not found")

guard = addr("4hpet5GUARD")

WATCHES = {
    # GUARD cluster: canary page (two spots) + the member qwords.
    "GUARD_CANARY_LO": guard + 0x000,
    "GUARD_CANARY_MID": guard + 0x7F8,
    "BASE_LOCKED": guard + 0x1000,
    "BASE_LOCKED_INV": guard + 0x1008,
    "DIV": guard + 0x1010,
    "DIV_INV": guard + 0x1018,
    "BOOT_OFFSET": guard + 0x1020,
    "BOOT_OFFSET_INV": guard + 0x1028,
    # Session-10 expansion (capacity proven >= 10 by run/ww-cap1): the
    # static the engine actually smashed twice (b77/wt1) was OUTSIDE the
    # cluster and unwatched; TSC_MULT is the timekeeper-class middle
    # qword of the TSC trio (OFFSET/BASE/MULT at +0x90/+0xa0/+0xa8 of
    # 0xffffffff80be8f90). Both are storm-quiet: hpet::BASE's only
    # post-attach writers are the heal paths (a hit IS a corruption
    # event — the rip classifies writer vs heal); TSC_MULT's only store
    # is the HPET-gated TSC-fallback calibration, which returns early
    # forever once hpet::ready() and never runs on these boots.
    "HPET_BASE": addr("4hpet4BASE"),
    "TSC_MULT": addr("8TSC_MULT"),
}

out = []
out.append("set pagination off")
out.append("set confirm off")
out.append("set height 0")
out.append("set width 0")
out.append("set print thread-events off")
out.append(f"file {ELF}")
out.append(f"target remote 127.0.0.1:{PORT}")
for name, a in WATCHES.items():
    out.append(f"watch *(unsigned long long*)0x{a:x}")
    out.append("commands")
    out.append("silent")
    out.append(
        f'printf "WW-HIT {name} rip=%p rsp=%p rbp=%p\\n", $rip, $rsp, $rbp'
    )
    out.append("info symbol $rip")
    out.append("info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15")
    out.append("x/24gx $rsp")
    out.append("continue")
    out.append("end")
out.append(f'printf "WW-ARMED {len(WATCHES)} watchpoints, GUARD=0x{guard:x}\\n"')
out.append("continue")

print("\n".join(out))
