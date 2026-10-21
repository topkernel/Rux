#!/usr/bin/env python3
"""ww-cap-gdbscript.py — watchpoint CAPACITY probe for this host's QEMU.

Arms 10 hardware watchpoints: positions 1-8 on quiet post-init statics
(arming proof), positions 9-10 on JIFFIES and timer::LAST_TICK
(100 Hz hot — firing proof at high positions). If WW-CAP-HOT lines
appear from points 9 and 10, this gdbstub honors >= 10 hw watchpoints
and firing works at every armed position (validates the chain's 8-point
set too). Any WW-HIT on the quiet candidates during a bare idle boot is
itself engine-class signal (classify by the logged rip).

Usage: ww-cap-gdbscript.py <port>
"""

import re
import subprocess
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12399
ROOT = "/home/william/rux-agents/wildwrite-hunt"
ELF = f"{ROOT}/run/ww-hunt-frozen-rux.elf"

nm = subprocess.run(["nm", ELF], capture_output=True, text=True, check=True).stdout

def addr(sym_suffix):
    for line in nm.splitlines():
        m = re.match(r"^([0-9a-f]+) [bBdDtTrR] (\S+)$", line)
        if m and m.group(2).endswith(sym_suffix):
            return int(m.group(1), 16)
    raise SystemExit(f"symbol ending in {sym_suffix} not found")

WATCHES = {
    # quiet candidates (arming proof + engine-class tripwires)
    "HPET_BASE": addr("4hpet4BASE"),
    "CONSOLE_UART": addr("3rux7console4UART"),
    "TSC_MULT": addr("8TSC_MULT"),
    "PHYS_RAM_BASE": addr("13PHYS_RAM_BASE"),
    "CMDLINE_PTR": addr("7cmdline11CMDLINE_PTR"),
    "GDT_PER_CPU": addr("4trap11GDT_PER_CPU"),
    "VIRTIO_MMIO_OFF": addr("11MMIO_OFFSET"),
    "JIFFIES_BASE": addr("12JIFFIES_BASE"),
    # hot tick statics (firing proof at positions 9-10)
    "JIFFIES": addr("7JIFFIES"),
    "LAST_TICK": addr("5timer9LAST_TICK"),
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
    out.append(f'printf "WW-CAP-HIT {name} rip=%p\\n", $rip')
    out.append("info symbol $rip")
    out.append("continue")
    out.append("end")
out.append(f'printf "WW-CAP-ARMED {len(WATCHES)} watchpoints\\n"')
out.append("continue")

print("\n".join(out))
