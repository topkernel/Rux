# Ubuntu GNOME boot patrol log

Each entry: one patrol round — what ran, what the evidence said, what
happened next. The patrol runs every 30 minutes until Ubuntu boots
clean; the automation deletes itself once that verdict is recorded.

## Round 1 — 2026-11-08 00:06 (kernel c127e4bc, SMP 8)

**Context**: the user's every-boot failure (gnome-shell up, then the
pango thread's FcInit SIGSEGV → "respawning too quickly" → failed
dialog) was root-caused earlier today and fixed in `c127e4bc` (the
break was per-TASK; a thread's stale copy made its brk shrink release
live pages — retrospective §8.5).

**What ran**

- The user's own `make ubuntu-run` GUI session was still alive at the
  start of the round (QEMU on pts/0, `-smp 8`, started 23:54 with the
  FIXED kernel image built 23:26). It was left untouched — a patrol
  must not kill the user's live session. It exited (window closed)
  during the round; no serial log was reachable (pts/0), so the round
  used its own headless boot for the evidence.
- Patrol boot: `timeout 160 qemu-system-x86_64 -M q35 -m 4G -smp 8
  -accel kvm ... -append "root=/dev/vda rw init=/sbin/runit
  console=ttyS0 trimq=1"` → /tmp/patrol-0006.log

**Verdict**: CLEAN

| Signal | Count |
|---|---|
| "Running GNOME Shell" | 8 |
| SIGSEGV deaths (`SIGDEATH ... sig=11`) | **0** |
| "respawning too quickly" | **0** |
| TRIMQ-HIT (touched a trimmed range) | **0** |
| KERNPANIC | 0 |
| STRAGGLER-RESCUE (lost-wake rescue, the separate park face) | 19 |

The trim family is silent: no first-touch ever landed in a reclaimed
band, and the shell survived the whole window. The 19 rescue firings
belong to the §8.3 lost-wake family (bounded, not this face).

**Next**: Tier-1/Tier-2 perf gate (mm is a guarded domain) once the
host frees capacity; push `c127e4bc` once it passes.
