#!/usr/bin/env python3
"""ww-gdbscript.py — generate the gdb command file for the wildwrite hunt.

Hardware watchpoints on the hpet victim statics (this build's addresses,
from `nm target/x86_64-unknown-none/debug/rux`):
  .data: BASE_LOCKED_INV / BOOT_OFFSET_INV / DIV_INV
  .bss : BASE_LOCKED / BOOT_OFFSET / DIV / BASE
(LAST_GOOD is written on every clock read — excluded.)

Every hit logs rip + symbol + registers + stack window and continues.
Legit writers are a known small set (hpet::init at early boot — before
attach; heal paths in validated_read_base / read_time_nominal /
read_counter). Any OTHER rip is the wild writer.
"""

import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12371
ELF = (
    "/home/william/rux-agents/wildwrite-hunt/"
    "target/x86_64-unknown-none/debug/rux"
)

# name -> address (regenerate with nm if the kernel is rebuilt)
WATCHES = {
    "BASE_LOCKED_INV": 0xFFFFFFFF807BF608,
    "BOOT_OFFSET_INV": 0xFFFFFFFF807BF610,
    "DIV_INV": 0xFFFFFFFF807BF618,
    "BASE_LOCKED": 0xFFFFFFFF80D02D90,
    "BOOT_OFFSET": 0xFFFFFFFF80D02D98,
    "DIV": 0xFFFFFFFF80D02DA0,
    "BASE": 0xFFFFFFFF80D02DA8,
}

out = []
out.append("set pagination off")
out.append("set confirm off")
out.append("set height 0")
out.append("set width 0")
out.append("set print thread-events off")
out.append(f"file {ELF}")
out.append(f"target remote 127.0.0.1:{PORT}")
for name, addr in WATCHES.items():
    out.append(f"watch *(unsigned long long*)0x{addr:x}")
    out.append("commands")
    out.append("silent")
    out.append(
        f'printf "WW-HIT {name} old->new, rip=%p rsp=%p rbp=%p\\n", '
        "$rip, $rsp, $rbp"
    )
    out.append("info symbol $rip")
    out.append("info registers")
    out.append("x/32gx $rsp")
    out.append("continue")
    out.append("end")
out.append("printf \"WW-ARMED %d watchpoints\\n\"" % len(WATCHES))
out.append("continue")

print("\n".join(out))
