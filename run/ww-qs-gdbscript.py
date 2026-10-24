#!/usr/bin/env python3
"""ww-qs-gdbscript.py — STORM-quietness probe watchset (session 13).

Watches ONLY the 5 candidate statics that the idle bare-boot probe
(run/ww-cap1) showed as post-init quiet but which have never been
observed under the OH service storm:
  CONSOLE_UART, PHYS_RAM_BASE, CMDLINE_PTR, GDT_PER_CPU, VIRTIO_MMIO_OFF
A candidate is chain-expandable only if it stays quiet under storm (any
legit writer would auto-stop the chain via the WW-HIT grep). Used by
run/ww-qs.sh — hit tag is WW-HIT so existing tooling classifies hits,
but this probe runs OUTSIDE the chain (own port, own tag).

Usage: ww-qs-gdbscript.py <port>
"""

import re
import subprocess
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12398
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
    "CONSOLE_UART": addr("3rux7console4UART"),
    "PHYS_RAM_BASE": addr("13PHYS_RAM_BASE"),
    "CMDLINE_PTR": addr("7cmdline11CMDLINE_PTR"),
    "GDT_PER_CPU": addr("4trap11GDT_PER_CPU"),
    "VIRTIO_MMIO_OFF": addr("11MMIO_OFFSET"),
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
    out.append("info registers rip rsp rbp rax rbx rcx rdx rsi rdi")
    out.append("x/16gx $rsp")
    out.append("continue")
    out.append("end")
out.append(f'printf "WW-ARMED {len(WATCHES)} storm-quiet candidates\\n"')
out.append("continue")

print("\n".join(out))
