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

## Round 3 — 2026-11-08 ~04:30 (kernel 59f8d535, SMP 8)

**Experiments this round**:

1. 5-minute paint-latency boot (dumps at 60/120/180/240/300 s): the
   screen NEVER paints — the break is hard, not timer latency.
2. The xfill X-level paint probe could not complete (the probe binary
   itself dies between its first print and connect() in the guest; runs
   fine against the host X — probe bug, not adjudicated).
3. A D-state task appeared (pid 312, >120 s, stack in
   context_switch/is_fork_child) alongside the idle session.
4. Bisect: midpoints 11f7cae2 and a8fae0b7 freeze before the shell on
   repeated boots — the paint bisect cannot pass through the
   phantom-bits era; honest anchors remain 7791d772 (good, 99%) ..
   1d9c8b75 (bad, 0% with the shell running).

**Unified theory** (fits every probe): the black screen is the known
LOST-WAKE / wake-refused family hitting glib's worker pool. Evidence:
- gnome-shell's 8 GJS workers sit in pthread_cond_wait forever (never
  signaled) while the main thread idles in clock_nanosleep — the UI
  never builds, no first frame, no damage, Xorg's shadow never
  refreshes, the framebuffer stays zero. Every kernel path we probed
  (fb mmap/write/flush, mprotect, shm, unix sockets, X protocol) is
  clean.
- The user's own console shows the family firing:
  `W2-WAKE-REFUSED-SLEEPING pid=317 state=0x0 on_cpu=true ...` and a
  run of `R46-RUNNING-PHANTOM healed ... exiting=1` lines — phantom
  RUNNING marks and refused wakes in the session's early processes.
- The bisect window (7791d772..1d9c8b75) contains exactly the
  phantom-bits / preempt-heal / pick-machinery era.

**Next round**: attack the wake-refusal itself — capture who wakes the
GJS worker pool (the cond signal path), why the wake is refused
(lying curr_on slot / stuck claim), and fix that producer; the
STRAGGLER-RESCUE machinery covers exit-path stragglers but not
cond-wake refusals.

## Round 4 — 2026-11-08 ~05:30 (kernel 59f8d535 + phantom-pick experiment)

**Hypothesis tested**: the black screen is the wake-refused family.
Evidence for it: the black boots show 41 WAKE-HEAL-SKIP (why=c = a
BUSY slot names the sleeper) and 10 R46-RUNNING-PHANTOM heals; the
painted boot shows ZERO of both. The refusal census names the victims:
pid 391 (gnome-shell), 343, 324, 318, 317, 323.

**Experiment**: the straggler rescue re-ran the frozen member without
releasing its phantom pick state, so the rescue's own wake was refused
(same curr_on rule) — the drain re-parked it forever (199 rounds/boot)
while the lying slot refused every wake aimed at it. Fix under test:
sched::release_phantom_pick() rewrites a stale slot to the owning
CPU's idle task and releases the ownership word before the re-run.

**Result: NEGATIVE.** PHANTOM-PICK-RELEASED fired 0 times in 5 boots
(the stragglers' slots were not the lying ones) and the screen stayed
black in 4/5 boots (the 5th reached the 128-pixel "first frame
starting" state). The straggler drain is a SYMPTOM of the era, not the
painting cause; the fix is kept only as a hardening (it cannot fire
today, and it closes a real refusal path for the rescue it enables).

**Diagnostic-class comparison** (painted boot vs black boot — the
cleanest signal found so far):

| class | painted | black |
|---|---|---|
| PREEMPT-HEAL | 28 | 0 |
| PREEMPT-UNDERFLOW | 22 | 0 |
| STALE-CLAIM-RESET | 7 | 0 |
| WAKE-HEAL-SKIP | 0 | 41 |
| R46-RUNNING-PHANTOM | 0 | 10 |
| DRAIN-STRAGGLER | 0 | 199 |
| TIMEDSLEEP-PARK | 0 | 38 |
| SCHED-IRQOFF | 0 | 12 |
| RWFRZ-HIGH | 0 | 4 |

The painted boot has the tick-side heals and the pick-side stale-claim
resets; the black boot has the wake-side family, the exit-path
straggler storm, parked timers and schedule()-under-IF=0. The two
families are disjoint — a structural difference in how the scheduler
heals, not a difference in degree.

**Also measured**: the shell starts 5x later in black boots (30 s vs
6.5 s) — the session is starved before the shell, and the shell's
first frame never arrives within 5 minutes.

**Next**: unattended valid-judgment bisect (run/paint-bisect.sh —
a verdict counts only when the shell started; pre-shell freezes are
skipped) over 7791d772 (good) .. 1d9c8b75 (bad).

## Round 5 — 2026-11-08 ~09:40 — **终点：Ubuntu 干净启动（主线）**

**Root cause of the black screen + freeze faces**: commit 885640e5
"wake-side lying-slot heal" (+ its fixup 1ebfe066, Nov 8 03:11-04:06).
The heal ran in the WAKE path: it convicted a busy CPU slot naming the
being-woken task as "stale" and rewrote the slot + released the pick
ownership — but the naming CPU can legitimately be MID-RESUME of that
exact task (the PICKED→LIVE affirm window). The captured failure chain
(debug=taskdump,periodic,sysforensic,wwcensus boot bc1):

    SLOT-LIE-WAKE-HEALED pid=138 arm=a
    → WAKE-HEAL pid=138 was=1          (preempt count zeroed)
    → PREEMPT-UNDERFLOW pid=138        (unpaired decrement clamped)
    → BUG: Illegal instruction [138] on CPU1 → KERNPANIC → CPU halted
    → the stranded spinlock (near the GRQ/PER_CPU block) deadlocks the
      other CPUs — the freeze face; the half-dead machine limps the
      session with no paint — the black face.

Timeline correlation: the heal landed 03:11; boots before it start the
shell 4/5; every boot after it freezes (0/7) or panics. Interleaved
controls (1d9c8b75 == 4f77f0f7, both 2/3 painted) bracket the window.

**Fix**: revert 29c2acba (removes the heal + its fixup, 270 lines).

**Verification (main 29c2acba, SMP 8, 300 s windows, screendumps)**:

| boot | shell lines | non-black @300s | panic | SIGSEGV (session) |
|---|---|---|---|---|
| pv1 | 20 | 1,023,986 / 1,024,000 (99%) | 0 | 0 |
| pv2 | 20 | 1,023,986 (99%) | 0 | 1 (gsettings helper; shell unaffected) |
| pv3 | 15 | 1,023,986 (99%) | 0 | 0 |
| pv4 | 20 | 1,023,986 (99%) | 0 | 0 |

Screenshot: runs/gnome-x86-pv1/shot-0300s.ppm (99% non-black, the GNOME
default wallpaper #e0e0c0 with the top-panel color rows).

Every clean-boot criterion met: shell up, session stable through the
full 300 s window (paint held at t=240 and t=300), zero SIGSEGV in the
session, zero "respawning too quickly", zero TRIMQ-HIT, zero panic.

**Patrol terminated at its success condition.**
