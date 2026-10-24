# FINDINGS-r74 — R74 slotless double-run producer hunt

Branch: feature/gnome-integration (worktree /home/william/rux-agents/gnome-int)
Session start: 2026-10-14 ~05:20. Instrumentation commit: 9c298148.

## Evidence recap (pre-existing captures, re-derived — not re-collected)

1. r56_print_pid prints DECIMAL. The historical "ksp=0x4294967..." strings are
   two concatenated decimal u32s (hi = ksp>>32 printed first). True values:
   - gate-r2 l1287 (early R74, pid 344): ksp = 0x1999_9995_1b81_8400
   - gate-r2 l14993 (final, pid 402):    ksp = 0x1999_9995_1cf9_8400
   - bigalloc9 (pid 325, printed 4x):    ksp = 0x05a9_15d6_04ce_0000
2. All three are bit63-CLEAR (user-range numeric data), so no in-kernel
   ksp writer (trap.S exit stores, exec pin, fork init=0, exit clear=0)
   can produce them. bigalloc9 printed the SAME value four times — the
   write happened ONCE and stayed frozen.
3. Lower halves (0x1b818400 = 461,472,768; 0x1cf98400 = 486,114,304) are
   exact microseconds-since-boot scale (guest uptime at those points),
   1 KiB-aligned. Upper half 0x1999_9995 = 429,496,725 ≈ 2^32/10 is
   IDENTICAL across the two gate-r2 captures (different tasks, minutes
   apart) — repeated constant next to a live timestamp = a real heap
   object's fields, not a transient register spill.
4. Same-task pairing: "PREEMPT-UNDERFLOW pid=402 ... healed to 0" fires
   immediately before "R74-SLOTLESS-TP ... tp_pid=402" — ti_preempt_count
   (+4) and ti_kernel_sp (+8) corrupted together, or the double-run alone
   drives preempt negative while a foreign store hits ksp.
5. Trigger context (bigalloc9): EXT4-BIGLOCK-STALL lines + dbus-daemon
   launch-helper exec + EXITNOTIFY bursts, immediately followed by
   PREEMPT-UNDERFLOW + R74 (pid 325 was an EXT4-BIGLOCK waiter).
6. Task 402 was STILL in the pid hash at panic time (92-task dump lists
   it) — the Task page was not freed-and-unhashed at that moment;
   garbage looks like live heap data either written over the Task
   (wild write) or the page recycled after a premature free then
   re-hashed by a new occupant.

## R83 instrumentation (commit 9c298148)

- kernel/src/dfx/r83hunt.rs: idle-loop audit (1 scan / 4 jiffies,
  system-wide CAS) validates every hashed task's header; first
  implausible word -> raw-UART report + 0x40-byte header hex dump.
  Validations: ti_kernel_sp (0, or kernel-half, not inside [_stext,
  _etext)), ti_preempt_count NEGATIVE only (r1 lesson: 0x1_0000 =
  HARDIRQ_OFFSET is a legal transient on a running task — 8 false
  hits in r1 before narrowing), ti_pick_state <= 2, running_on_cpu in
  [-1, MAX_CPUS), state != TASK_POISON while hashed, and no 0xC3..
  freed-paint qword.
- R74-SLOTLESS-TP and PREEMPT-UNDERFLOW sites now dump the victim's
  header (dump_victim) — byte-level shape even if the audit misses the
  write window.
- free_task_slot paints freed Task pages' first 0x100 bytes with
  0xC3C3_C3C3_C3C3_C3C3 (R47/R12-4 poison words rewritten after) —
  any later read through a stale Task pointer proves the UAF shape.

## Producer identification (r3, R83b live capture)

fire-r74hunt-r3 caught the foreign store IN THE ACT (jiffies=48248):
parked pid 412's header changed
`user_sp: 0x0000003fffffe400 -> 0xffffffff803c9740` — a KERNEL-TEXT
address written while the task slept in the ext4 biglock wait.

Producer: `kernel/src/arch/riscv64/trap.S` `.Lfrom_user` stashed the
live `sp` into `ti_user_sp` UNCONDITIONALLY, BEFORE the sstatus.SPP
origin check (the "ROUTING FIX" block). Any kernel-origin trap that
mis-entered through the sscratch=tp window wrote its kernel sp into
ti_user_sp and the SPP branch left the garbage behind. This is the
thread_info SMEAR ENGINE: the corrupted sp words are the seeds the
double-run partner's exit stores then propagate (`ksp = sp + PT_SIZE`
with a wild sp -> the R74-SLOTLESS-TP garbage ksp).

Fix (commit c1539f6e): stash t0 first, read SPP, and only a genuine
user trap (SPP=0) stores ti_user_sp. Kernel-origin traps branch with
t0 stashed exactly as before; ti_user_sp keeps the last real user sp.

(r3's kernel ALSO panicked inside my own diagnostic — the R83-AROUND
printer read before the struct (ti_flags=0, offset -0x10 wraps); fixed
in the same commit.)

## Verification

fire-r74fix-v1 (trap.S fix, full 2100s window, smp4/2G, MTTCG):
- ZERO KERNPANIC, ZERO PREEMPT-UNDERFLOW (gate-r2 baseline died of
  exactly these at 989s).
- gnome-session-binary alive through the whole window (hb t=2060s:
  gs-b:v53s), no SIGDEATH events, screen lit (M4_screen_lit @600s,
  nonblack 0.155); M5 missed by 0.007 nonblack (r2 passed M5 at 0.162)
  — verdict FIRE-PARTIAL for that reason only.
- ONE contained R74-SLOTLESS-TP at guest t~186s (pid 345 xkbcomp,
  ksp=0x19999995_1b900000) — see residual finding below.
- Residual finding: ONE contained R74-SLOTLESS-TP still occurred at
  guest t~186s (pid 345 xkbcomp, ksp=0x19999995_1b900000 — same
  constant upper half) and was healed (R46-RUNNING-PHANTOM + R74 gates).
  The ti_kernel_sp foreign store has a SECOND producer not yet named;
  refined signature: hi32 = 0x19999995 = (2^32-46)/10 EXACTLY in every
  capture (4 runs, 5 victims), lo32 = 0x400-aligned values that track
  guest-monotonic microseconds. The committed traps (R83 audit,
  R83b wait-watch on ksp+user_sp) will name it on the next capture;
  candidate shape: a two-u32 {us-time, constant≈429496725} field pair
  of a heap object.

## make test attribution

`make test` boots the test kernel and wedges (noctx pid=0 + VW-WEDGE
loop). Reproduced IDENTICALLY on commit 9e844568 (parent of the trap.S
fix) in a scratch worktree: NOT caused by the R83 changes. This is the
pre-documented layout-sensitive test-kernel boot failure (FINDINGS.md
open item 3, from the previous session); GUI builds boot fine.

## Run log

- fire-r74hunt-r1: aborted at ~6 min (audit noise burned the 16-report
  cap: preempt 0x10001/0x10000 = legal mid-hardirq values). Validator
  narrowed to negative-only, rebuilt.
- fire-r74hunt-r2 (2100s, smp4/2G, MTTCG): **FIRE-PASS** — full window,
  zero panics, desktop lit at t=1800s (M5_desktop_visible, nonblack
  0.162, colors 8), gnome-shell alive at end. ONE contained R74:
  - t~1610s guest (host 1810s): `EXT4-BIGLOCK-STALL waiter=311 holder
    =390 held=50j` (Xorg blocked on the ext4 biglock), then
    `R74-SLOTLESS-TP cpu=2 tp_pid=311(Xorg) slot_pid=450(gsd-
    housekeeping gdbus thread) ksp=0x19999995f5372800`
    followed by `NOVMA-EXEC pid=450 ... pgd page already recycled,
    mapcount=-1, ZLEDGER installs=0`.
  - R83 header dump (same __schedule, µs later): ksp had CHANGED to
    0xffffffd606cc0000 = pid 450's kernel-stack top — the writer stores
    DURING the R74 print itself (concurrent hart). user_sp (+16) clean,
    flags/preempt clean → the original foreign store is a PRECISE
    8-byte write at exactly Task+8. pad@44 = 0xc3c3c3c3 (freed-page
    paint residue in never-initialized padding — page recycling
    proven, not proof of live UAF).
  - Early-phase R74 window (log l1188, host ~190s): same shape —
    Xorg waits EXT4-BIGLOCK (holder=332, 4j) right after EXITNOTIFY
    bursts of the holder's children + fake-OOM SIGKILL wave
    (t=187-201s: gmain/gdbus/dconf workers sig=9) + PICK-SKIP churn.
  - Buddy allocator taints in r2 AND gate-r2: only DIE — NO BAD_PAGE:
    the R9-14 double-free and R18-1 double-handoff tripwires never
    fired; the buddy is not handing out overlapping blocks.
- fire-r74hunt-r3 (R83b waiter self-watch build): running.
- runs/bigalloc11 (minimal image, smp1, R83 build): no events by 790s.

## Current working theory (to be confirmed by R83b)

1. Fake-OOM SIGKILLs (separate known bug) kill glib workers mid-ext4 →
   EXT4-BIGLOCK contention spikes; victims (always the waiter) park in
   the ext4 acquire loop.
2. During the park window a foreign, precise 8-byte store hits the
   waiter's ti_kernel_sp with kernel-heap-data-looking bits
   ({0x19999995 constant, 0x400-aligned lo}).
3. The corrupted ksp + heal churn turns the waiter into the slotless
   double-run (R74); the partner hart (running a second context of the
   same task / a cross-stack continuation) then overwrites ksp with its
   own trap-exit store (450's stack top observed live).
4. R80 containment usually holds (r2 FIRE-PASS); under deeper load
   (gate-r2, 989s) the second hart escapes → PREEMPT-UNDERFLOW →
   KERNPANIC.

## Producer #2 (R84/R85 session, 2026-10-14 ~07:30)

Instrumentation (commit 436c5717): the R83 audit moved from the idle
loop (which never idles during the dbus storm) into scheduler_tick —
per-tick self-check, 4-jiffy pid-hash walk, and a 64-jiffy 2 MiB heap
scan for the fingerprint qword (hi32 == 0x1999_9995), plus one-shot
`r84_ksp_trap` reporters at all four trap.S ti_kernel_sp touch points.

### What the widened trap caught (fire-r84/r1, 900 s, 2 R74s, 0 panics)

- R74 #1 jiffies=31948 time=0xbfc80c0a (321.76 s): tp=pid 332
  gnome-session-b, ksp=0x19999995_1ae18400 — garbage present at the
  R74 read, ALREADY a legal sp+PT_SIZE value in the header dump
  microseconds later.
- tick-hash caught pid 392 (gsd-usb-protect) SIX jiffies before R74 #2:
  ti_preempt_count == -1 (double-run shape) plus ASCII "AppI"
  (a glib "AppInfo" string fragment) in journal_handle — user-data
  debris inside the header.
- R74 #2 jiffies=49143: tp = cpu2's STATIC IDLE TASK — its ksp read
  the garbage mid-schedule. A static .bss Task cannot be a recycled
  heap page: the page-reuse theory for producer #2 is dead.
- ZERO R83-WAITWATCH hits (no victim was in the ext4 wait when the
  value appeared) and ZERO fingerprint hits in the whole 128 MiB heap
  across seven full scan cycles → the qword is NOT a standing heap
  object; it exists at ti_kernel_sp only transiently (µs) around each
  R74 window.
- The same log shows gdbus executing at a garbage USER epc
  (0x3000442e56) — user contexts are getting corrupted registers.

### Value arithmetic (first-hand calibration)

Every capture satisfies ksp == (2^64 − X)/10 EXACTLY, with
X = 193.06e9, 193.41e9 (this run), 192.95e9, 192.71e9, 192.94e9
(gate-r2/r74fix-v1) and 156.43e9 (r74hunt-r2) — five of six cluster
inside 192.7–193.4 s. `objdump` shows the kernel binary contains NO
division by 10 (constant or magic) at all — the arithmetic runs in
USER code (glib clock math; the dbus storm's glib timeouts compute
exactly such wrapped-negative µs values). The 0x400-aligned lo32 and
the (2^32−46)/10 shape are artifacts of that user expression, not a
kernel timekeeping constant.

### Root cause

The store is the victim's OWN trap-return ksp anchor:
`.Lreturn_user` / `.Lcheck_resched_kernel` compute
ti_kernel_sp = sp + PT_SIZE from the LIVE sp register with no
validation. Writer enumeration is exhaustive — every Rust writer
stores 0 or a stack-top/frame pointer; the two asm stores are the only
writers that can propagate a corrupted sp. When the slotless
double-run engine corrupts a context's registers (same engine that
left gdbus at a garbage epc), the wild sp — a user-range value holding
glib's wrapped-negative timestamp arithmetic — is written verbatim
into ti_kernel_sp. Every ksp consumer then propagates it: the
from-user entry stack switch and both post-schedule reloads load
sp = ksp − PT_SIZE, pointing harts at unmapped frames (the smear that
turns a contained double-run into R74-SLOTLESS-TP /
PREEMPT-UNDERFLOW / panics). Producer #1 (c1539f6e) was the same
engine's ti_user_sp hop; producer #2 is the sp→ti_kernel_sp hop.

### Fix (commit 8b8bdd16, R85)

Every ti_kernel_sp touch in trap.S is now gated (one-shot
r84_ksp_trap report, then repair):

- store sites anchor ksp at the task's OWN kernel-stack top whenever
  sp+PT_SIZE is user-half — ksp can never again hold a user-range
  value;
- reload sites never load a user-half ksp into sp;
- entry paths (.Lfrom_user stack switch, .Lsave_kernel_frame push)
  repair a user-half sp to the stack top BEFORE any push — the old
  path pushed pt_regs at the wild sp, faulted inside the save, and
  cascaded with SIE off (silent hart wedge). The repaired frame
  records the repaired sp in PT_SP so the unwind cannot re-enter the
  wild pointer.

The upstream register corruption (slotless double-run, R56/R46 hunt)
remains a separate, already-contained hunt; R85 removes its ability to
smear through the thread_info anchor.

### Verification

(pending — full 2100 s smp4 fire6 on the R85 build plus the
minimal-session paint check; numbers appended when complete.)
