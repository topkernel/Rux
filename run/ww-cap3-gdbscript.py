#!/usr/bin/env python3
"""ww-cap3-gdbscript.py — 20-point capacity probe (session 28).

Arms the chain's 15-point set PLUS the four remaining timekeeper
neighborhood statics (TSC_ACTIVE, JIFFIES_BASE, TSC_CONTINUITY_OFFSET,
TSC_BASE — all writer-classified quiet: TSC_ACTIVE/TSC_CONT_OFF/TSC_BASE
stores sit in the same HPET-gated calibration blocks as TSC_MULT's
(r1-r45 zero legit hits); JIFFIES_BASE is written once by the first-tick
CAS, long before attach), with JIFFIES (100 Hz hot) at the DEEPEST
position as the firing proof. If WW-CAP3-HOT lines appear from position
20 and all 20 arm with no insertion errors, this gdbstub honors >= 20
simultaneous hw watchpoints with firing at every depth, and the chain
set can grow to 19.

Usage: ww-cap3-gdbscript.py <port>
"""

import re
import subprocess
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12403
ROOT = "/home/william/rux-agents/wildwrite-hunt"
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
    # --- the chain's current 15 (see ww-gdbscript.py) ---
    "GUARD_CANARY_LO": guard + 0x000,
    "GUARD_CANARY_MID": guard + 0x7F8,
    "BASE_LOCKED": guard + 0x1000,
    "BASE_LOCKED_INV": guard + 0x1008,
    "DIV": guard + 0x1010,
    "DIV_INV": guard + 0x1018,
    "BOOT_OFFSET": guard + 0x1020,
    "BOOT_OFFSET_INV": guard + 0x1028,
    "HPET_BASE": addr("4hpet4BASE"),
    "TSC_MULT": addr("8TSC_MULT"),
    "CONSOLE_UART": addr("3rux7console4UART"),
    "PHYS_RAM_BASE": addr("13PHYS_RAM_BASE"),
    "CMDLINE_PTR": addr("7cmdline11CMDLINE_PTR"),
    "GDT_PER_CPU": addr("4trap11GDT_PER_CPU"),
    "VIRTIO_MMIO_OFF": addr("11MMIO_OFFSET"),
    # --- session-28 expansion candidates (quiet) ---
    "TSC_ACTIVE": addr("10TSC_ACTIVE"),
    "JIFFIES_BASE": addr("12JIFFIES_BASE"),
    "TSC_CONT_OFF": addr("21TSC_CONTINUITY_OFFSET"),
    "TSC_BASE": addr("8TSC_BASE"),
    # --- firing proof at the deepest position (hot) ---
    "JIFFIES": addr("7JIFFIES"),
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
    tag = "WW-CAP3-HOT" if name == "JIFFIES" else "WW-CAP3-HIT"
    out.append(f'printf "{tag} {name} rip=%p\\n", $rip')
    out.append("info symbol $rip")
    out.append("continue")
    out.append("end")
out.append(f'printf "WW-CAP3-ARMED {len(WATCHES)} watchpoints\\n"')
out.append("continue")

print("\n".join(out))
