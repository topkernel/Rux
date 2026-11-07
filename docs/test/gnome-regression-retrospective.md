# GNOME Desktop Regression Retrospective

**Date**: 2026-11-06
**Scope**: x86_64 GNOME desktop boot (`make ubuntu-run`), regression window
main@4ed009ac (2026-10-31) → main@264b6961 (2026-11-06).
**Status**: two of three root causes fixed and verified; one deep-water
item open; one fresh regression caught during the hunt (see §6).

## 1. What "worked" meant concretely

The x86 GNOME milestone (99% non-black frame, 17-minute stable session)
was accepted at main@4ed009ac by the gnome-x86 lane: per-boot manual
verification with the lane's fire.py driver. It was a campaign
acceptance, not a standing gate — after the lane demobilized, nothing
in the recurring test surface booted the GNOME desktop again.

## 2. Regression window

| Metric | Value |
|---|---|
| Commits in window | 2,825 |
| Commits touching perf/optimization paths (title match) | ~664 |
| Standing desktop-boot runs in window | 0 |

The desktop workload stopped being exercised the day its lane
demobilized. Every landing since was gated against LTP, make test,
pbench, syscost and the OH image — none of which boot gnome-shell.

## 3. Confirmed root causes (both performance reworks)

### 3.1 munmap chunked surgery — Face A/B races

- **Origin**: the OH campaign's 132 µs munmap convoy fix replaced the
  single VMA-write-lock hold with per-256KB chunks, re-deriving the VMA
  tree after each chunk guard drop.
- **Hole**: between guards, a same-mm racing mmap legally landed in the
  transient tree hole inside the still-processed range; the next chunk
  treated the fresh VMA as an unprocessed piece of the current munmap
  and deleted it (vma=NONE faults), and the deferred zap cleared fresh
  PTEs (Face B).
- **Fix** (main@5b4a4bc7): detach-all-then-zap — one guard detaches
  every piece; ≤4 MB ranges (the whole malloc/arena class) zap inside
  the guard. Reproducer: 2/3 boots crashed → 3/3 clean (12,000 checks
  per boot).

### 3.2 ext4 async batch read fill — cross-file page-cache poison

- **Origin**: the lmbench read-throughput hunt (bw_file_rd 50x
  attribution) moved the read path to async batch DMA with
  ReadDst::Discard cache warming.
- **Hole**: completion/buffer-reuse attribution served the previous
  read's buffer bytes under the new file's cache keys. Measured:
  /bin/bzip2's mmap pages carried /bin/bzexe's script bytes while
  pread of the same fd returned correct data. Downstream: ld.so read
  garbage .dynsym → "undefined symbol" with an empty name → lazy-bind
  NULL+8 → gnome-shell died at fontconfig init every boot.
- **Fix** (in main): interim gate disables async submission — the
  synchronous per-block path's attribution is exact. Cold-file probe:
  3/3 boots, zero mismatches. The async attribution bug itself is
  queued as its own lane (cache-poison branch, full evidence package).

## 4. Why the merge gate missed both

| Gate item | Why it could not see these |
|---|---|
| LTP 1,869 scan | read(2) tests never touch mmap fault-around; mmap tests are small single-threaded files; the munmap races need same-mm multi-threaded churn |
| make test | in-kernel unit suite, no userspace FS load |
| pbench / syscost | microbenchmarks measure speed, not content correctness |
| OH image boot | OHOS uses musl (different allocator behavior); the OH lane's own image-side failures masked mm-class crashes |
| GNOME boot | campaign-only; demobilized with the lane |

**Root pattern**: every lock removed and every I/O batched was verified
against microbenchmarks and LTP. The one workload that exercises
glibc's dynamic linker, heap trim and GObject lifecycles — a real
desktop — had disappeared from the standing test surface. The
regression was not an event; it was silent accumulation across 2,825
commits, exposed the first time someone ran `make ubuntu-run` again.

## 5. Compounding factors (honest list)

1. **Image artifacts lost**: the worktree cleanup took the gnome-x86
   build outputs; the central store only kept riscv64 GNOME disks. No
   old-image + new-kernel A/B was possible; the hunt started from zero.
2. **Two variables changed at once**: the freshly rebuilt image pulls
   today's jammy (733 packages; gjs/mutter newer than the validated
   set), so kernel-regression vs version-drift had to be separated
   before the kernel hunt could start.
3. **Probe false positives** (both mine, corrected in-session): one
   forensic print dereferenced the Arc handle instead of the File
   (constant "same pointer" artifact); one probe's CRC covered a
   different byte range than its checker (guaranteed false
   corruption). Lesson recorded: validate the probe against a known
   good case before treating its output as evidence.
4. **Organizational regressions in the same window**: the scripts/test
   relocation broke 37 drivers' repo-root anchors at once; an
   unanchored `debug/` gitignore rule left 36 module files untracked
   and origin unbuildable (caught by the post-push clone gate,
   main@484f2144). Same disease: moves and renames applied halfway.

## 6. The pattern repeated during the hunt

While the two fixes above were being verified, **perf-gap-hunt round 7**
(lease fast gates + memo v2 lockless) landed and the GNOME boot began
wedging before the shell (dbus library-load phase, 55–129
"fdreap close completion stalled" lines, zero shell starts).
Discriminator boot on the same tree without the hunt's diagnostic
parameter wedged identically — a third regression of the same class,
caught the same week. It needs its own bisect lane.

## 7. Open items

1. **Trim face** (deep water): glibc systrim reclaims heap the kernel
   agrees is free, then live pointers walk inside it — the metadata was
   destroyed before the trim. Every kernel-side hypothesis is
   eliminated by probe evidence (allocator placement, brk/VMA
   consistency, COW, mremap, swap roundtrip under real pressure,
   fault-around discipline, madvise bounds, rmap/LRU). The trim
   quarantine instrument (main@f8ffce15, boot param `trimq=1`) captures
   the first touch with full context: first capture = two READs by
   gobject code 0–2 jiffies after the trim, inside a 1.15 MB reclaimed
   range. Next: frame-content dump at the hit (distinguishes zero-fill
   from aliasing) plus a userspace malloc interposer.
2. **Round-7 early-boot wedge**: bisect the perf-gap-hunt round 7
   merge on the GNOME boot.
3. **fdreap close-stall floods**: present in every recent boot's late
   phase (55–129 per boot); its own lane.

## 8. The park face (2026-11-07): three stacked mechanisms, all fixed

After the §3/§6 fixes the desktop still froze before the shell in
3–5 of 5 boots (variance by kernel build). Three independent
mechanisms produced the same visible face — "gnome-session frozen at
check-accelerated" — and each had to be peeled off separately.

### 8.1 SOFTIRQ_OFFSET migration across a mid-window task switch (main@166e4516)

The first capture (taskdump on a frozen boot) showed
check-accelerated (pid 318) RUNNING on CPU1 with preempt=0x100 — the
softirq window's SOFTIRQ_OFFSET stranded on the entry task after a
context switch inside the window booked the add on one task and the
sub on another. Fixed by repairing the bracket on mismatch
(commit 166e4516). Necessary but not sufficient: 3/5 boots still froze.

### 8.2 Leaked preempt BASE bit defeating the drain's yield (DRAIN-PC-HEAL)

Second capture: pid 318 spins in `release_task`'s straggler drain
(tick samples: dead_threads SpinlockGuard drop, Vec push, slab alloc,
restore_irq, enable_irq — 38 s cumulative; 216 PREEMPT-LEAK lines at
pc=0x10001 pctick=1). The chain: a leaked +1 preempt base (the known
cross-task guard-drop engine — VFS/ICACHE-heavy paths, which the
ENOENT-heavy session start hammers) makes the drain's
`schedule()` a no-op (`__schedule` refuses to switch out a task with a
nonzero count — the correct discipline for a real lock holder), and
the tick-side PREEMPT-HEAL never matures because the leak flickers
(drain/re-leak resets the 200-consecutive-observation bar). Fix: at
the drain's yield point — lock-free by construction — a nonzero
preempt base is provably phantom; clear it, report (DRAIN-PC-HEAL),
yield for real. Same treatment in `wait_for_thread_group_death`.
Defense-in-depth: `__schedule`'s voluntary-park refusal now carries a
3 s escape hatch (PARK-HOLDOUT-HEALED) for the same phantom class.

### 8.3 The lost straggler: a mid-exit member whose wake never came (STRAGGLER-RESCUE)

Third face (the dominant one, 4/5 freezes): the gles-helper's
group-kill leaves a thread-group member parked on the leader's
dead-threads list in state=INTERRUPTIBLE with on_cpu=-1 — lost between
its ring leave and its terminal DEAD store, runnable by no one. The
reaper's drain re-parks it forever (observed: 598k–33.7M rounds; the
new DRAIN-STRAGGLER namer fingerprinted it: member RUNNING-on-no-cpu
→ later corrected to INTERRUPTIBLE-sleeping-on-no-cpu — TaskState
encoding: RUNNING=0, INTERRUPTIBLE=1). Fix: after 50k drain rounds a
member in that state is a lost wake — re-run it (set RUNNING,
wake_up_enqueue). Its kernel stack is intact mid-do_exit; it then
finishes its own exit (pid bookkeeping, DEAD, final switch) with all
invariants. Boots that hit the face now recover in-place:
3/3 taskdump-classified frozen boots became shell-up boots with one
STRAGGLER-RESCUE line each; zero panics.

### 8.4 What is still open

The **producer** of 8.3 (why the member's wake was lost — wake-refused
state race vs pick/heal re-link) and of 8.2 (the cross-task
SpinlockGuard add/drop engine) remain open hunts; the rescue/heal
machinery bounds their damage to sub-second and reports every firing
as evidence (pair with debug=leakhunt for acquire sites). The namer's
stack words (sp+0.., above the saved sp) name the park site on the
next occurrence. A rarer fourth face freezes the session past
check-accelerated in the keyring phase (1 of 11 post-fix boots; no
straggler/leak markers fired — needs its own taskdump capture).

**Verification (2026-11-07)**: 11 post-fix boots across three
batteries (5×160 s + 3×160 s + 3 taskdump), kernel main+this change,
image work/gnome-x86.img: shell up in 10/11 (was 0–1/5); zero
KERNPANIC; STRAGGLER-RESCUE fired in 3 of them and every firing
recovered the boot in place; the one freeze left is a different,
rarer face in the keyring phase (§8.4); dual-arch builds green;
riscv64 scripted boot rc=0; make test green.

## 9. Recommendations (updated)

1. Add a **desktop-boot gate** to the standing merge tiers: serial
   shows "Running GNOME Shell" and the session survives 60 s — same
   standing as the LTP scan.
2. Add the **byte-exact file-mmap verification probe** (fmv/coldfile)
   to the standing probes; it is the sensor class that would have
   caught the poison the day it landed.
3. Require perf landings that remove locks or batch I/O to state, in
   the merge message, how fault-around, brk-trim and multi-threaded
   munmap were re-verified.
4. The exit-path drain loops are now three-deep in "wait for another
   task to progress" shapes (group death, straggler drain, on_cpu
   wait) — each has bitten once. Any new one needs the same trio:
   lock-free yield point, bounded wait, always-on report.

---
*Evidence: run/ probe sources (mmap_witness, arena_churn, coldfile,
fmv, swaprt, mremap), boot serials under /tmp per session log, kernel
commits 5b4a4bc7, 10705570, f8ffce15, 484f2144; park-face serials
/tmp/fz-td*.log, /tmp/fz-cen1.log, /tmp/tri-g*.log, /tmp/res-g*.log
(2026-11-07).*
