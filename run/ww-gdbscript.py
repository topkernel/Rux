#!/usr/bin/env python3
"""ww-gdbscript.py — generate the gdb command file for the wildwrite hunt.

Hardware watchpoints on the hpet victim statics, addresses derived from
`nm` of the CURRENT build (the wildwatch commit moved the quiet statics
into hpet::GUARD — a page-exclusive cluster; the watch set therefore
targets the GUARD members directly so watchpoint boots work on both the
pre- and post-relocation layouts).

Note (s30 policy change): hunt boots DO combine with dfx=wildwatch now
(ww-hunt.py appends it). On the GUARD-cluster points (1-8) the kernel RO
trap fires FIRST — a store there faults with a full KERNPANIC
attribution dump in the SERIAL log, and the gdb watchpoint never sees a
value change (kept armed as a backstop for a failed wildwatch arm). The
11 non-cluster points remain gdb's exclusive, rip-attributing surface.
The chain stop check greps BOTH logs (see ww-chain2.sh).

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
    # Session-13 expansion (storm-quietness proven by run/ww-qs1: 15.5
    # min full OH storm, 864 forks, wakes 3.57M, ZERO hits on all five;
    # idle-quietness already proven by run/ww-cap1). Boot-set-once
    # statics: console UART base, physmap base, cmdline pointer,
    # per-CPU GDT array, virtio-pci MMIO window offset.
    "CONSOLE_UART": addr("3rux7console4UART"),
    "PHYS_RAM_BASE": addr("13PHYS_RAM_BASE"),
    "CMDLINE_PTR": addr("7cmdline11CMDLINE_PTR"),
    "GDT_PER_CPU": addr("4trap11GDT_PER_CPU"),
    "VIRTIO_MMIO_OFF": addr("11MMIO_OFFSET"),
    # Session-28 expansion (capacity proven >= 20 by run/ww-cap3: 20/20
    # armed, position-20 firing at 3629 hits/40s, 0 quiet hits): the four
    # remaining timekeeper-neighborhood statics, tiling the full
    # 0xffffffff80be8f80-8fb0 stretch except the hot JIFFIES qword
    # (0x8f98, written every tick — a flood, correctly excluded).
    # Writer classes: TSC_ACTIVE/TSC_CONTINUITY_OFFSET/TSC_BASE stores
    # sit in the SAME HPET-gated calibration blocks as TSC_MULT's
    # (x86_64.rs :212/:211/:210 and :381/:380/:379) — dead code on these
    # boots (hpet::ready() since second 1; TSC_MULT 0 legit hits r19-r45).
    # JIFFIES_BASE: one compare_exchange at the FIRST increment-jiffies
    # call (x86_64.rs:433-436), seconds before attach. The wt1 engine
    # fingerprint was a WIDE smash into this neighborhood — the full
    # trio + hpet cluster + BASE now covers every confirmed geometry.
    "TSC_ACTIVE": addr("10TSC_ACTIVE"),
    "JIFFIES_BASE": addr("12JIFFIES_BASE"),
    "TSC_CONT_OFF": addr("21TSC_CONTINUITY_OFFSET"),
    "TSC_BASE": addr("8TSC_BASE"),
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
