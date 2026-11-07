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

## Round 2 — 2026-11-08 ~03:00 (kernel 59f8d535, SMP 8)

**User report**: "我还是看不到界面" — the session runs, the SDL window
stays black.

**What was established this round** (probes on the current kernel):

1. The kernel display chain is INTACT: a guest probe writing a colour
   pattern straight into /dev/fb0 shows up in the host screendump
   (100% non-black; scripts/test/gnome-diag/fbpaint-boot.sh).
2. mprotect semantics clean (middle-of-anon / two-span / middle-of-fb
   patterns all pass — the 1f2f18cb-era ENOMEM bug is fixed on main).
3. SysV shm cross-process sharing clean (50 rounds, both directions).
4. AF_UNIX stream integrity clean (2000x8KB, checksums both ways).
5. The live X server answers protocol requests (connection setup +
   GetInputFocus round-trip via a hand-rolled client, /run/xprobe.c).
6. Xorg runs with a SHADOW framebuffer ("FBDEV(0): using shadow
   framebuffer") — rendering goes to a shadow copy; only dirty regions
   reach the real fb.
7. gnome-shell at idle is HEALTHY (main thread clock_nanosleep, workers
   in futex cond-wait) — it is not the park face.

**Bisect** (image work/gnome-x86.img, screendump 99% = good):
- good: 95778e17, 561db355, d7085761, 7c850d6d, 7791d772 (99% paint)
- bad: 1d9c8b75, 39975867, 166e4516, current main (shell up, 0% paint)
- Round-1 bisect named 1f2f18cb, but 4 of its "bad" verdicts were
  boots where the SHELL NEVER STARTED (the known freeze face) — invalid
  as paint verdicts. Honest window: **7791d772..1d9c8b75** (451
  commits).
- Round-2 bisect started with a validity rule (paint verdict counts
  only when the shell started); first midpoint a8fae0b7 froze pre-shell
  twice — inconclusive.

**Open discriminator** (next round): the X-level paint test
(/run/xfill.c — create a white window via raw X protocol; host
screendump tells whether Xorg's render+shadow->fb copy works). It
needs a session-started boot; this round's attempt hit the freeze
face. Run it with the btest.sh retry discipline.

**Next**: xfill with retries; then the valid-judgment bisect over
7791d772..1d9c8b75. The freeze-before-shell face (1-in-5) also still
needs its own capture — the taskdump from the patrol boot shows the
check-accelerated chain wedged (the §8.3 family).
