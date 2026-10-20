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
ELF = f"{ROOT}/target/x86_64-unknown-none/debug/rux"

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
