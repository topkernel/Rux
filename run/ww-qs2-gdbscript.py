#!/usr/bin/env python3
"""ww-qs2-gdbscript.py — storm-quietness probe watchset (session 30).

The s30 chain change (dfx=wildwatch on every hunt boot) makes the eight
GUARD-cluster watchpoints redundant — the kernel RO trap covers the whole
2-page cluster with full rip attribution — freeing those hardware slots
for statics OUTSIDE the cluster. This probe set is the candidate list;
run/ww-qs2.sh boots a full OH storm with these 8 armed (+ wildwatch on,
validating coexistence). Any candidate that HITS during the storm is not
chain-safe and is dropped from the repurpose set.

Writer analysis (source-grounded, s30):
- HEAL_LAST      hpet heal_report::LAST — stored only inside heal_report
                 (rate-gated), zero heal lines in 51 chain boots.
- KERNEL_MAP     memory_layout init-once struct (7 qwords), written once
                 at memory-layout init; PHYS_RAM_BASE neighborhood.
- VMEMMAP_FLAG   vmemmap::VMEMMAP_INIT + security INIT_DONE (one byte
                 each, one 8-byte watch covers both) — init-once flags.
- ISIG_JIFFY /   console input-signal path — writers run only on ISIG
  ISIG_SIGNO     (signal-generating) RX chars; headless boots get none.
- TX_NOTICE      console TX-recovery notice timestamp — rate-gated by
                 itself; MEDIUM risk (storm TX stalls) — probe decides.
- DUMP_MAGIC /   console RX magic-sequence scanners — written per RX
  LEDGER_MAGIC   char; no RX on headless boots. MEDIUM — probe decides.

Usage: ww-qs2-gdbscript.py [port]
"""

import re
import subprocess
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 12377
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
    "HEAL_LAST": addr("4hpet11heal_report4LAST"),
    "KERNEL_MAP": addr("10KERNEL_MAP"),
    "VMEMMAP_FLAG": addr("12VMEMMAP_INIT"),
    "ISIG_JIFFY": addr("15LAST_ISIG_JIFFY"),
    "ISIG_SIGNO": addr("15LAST_ISIG_SIGNO"),
    "TX_NOTICE": addr("14TX_LAST_NOTICE"),
    "DUMP_MAGIC": addr("14DUMP_MAGIC_POS"),
    "LEDGER_MAGIC": addr("16LEDGER_MAGIC_POS"),
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
    out.append("continue")
    out.append("end")
out.append(f'printf "WW-ARMED {len(WATCHES)} storm-quiet candidates\\n"')
out.append("continue")

print("\n".join(out))
