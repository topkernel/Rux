# stress-port round-1 findings (2026-11-02)

Vehicle: x86_64+KVM q35, 2G/2cpu, LTP rootfs image + /test/stress-ng
(rs suite, this lane). DFX: dfx=allocwatch,taskdump,watchdog (+leakhunt
on the memory-leakhunt boot) + syscost=1. One class family per boot,
serial console log per boot in run/stress-runs/.

## Verdict table

Baseline kernel 819a1be2 (round-1 campaign):

| boot | classes | verdict |
|------|---------|--------|
| memory | malloc 4x100 / mmap 4x100 / bigheap 2x100 | PASS on panics/wedges; classes truncated at 60 s by an rsrun driver bug (fixed; budgets restored in v3 boots) |
| memory-leakhunt | same + dfx=leakhunt | PASS; 12 heals + 2 underflows (SP-1) |
| forker | fork 8 / exec 4 | PASS on panics; 15,577 heals + 169 underflows (SP-1 storm) |
| fsmeta | fsmeta 4 | **FAIL** — read-after-chmod content corruption (SP-2) |
| ipc | pipe/sock/sem/msg/shm 4x90 | **PASS** (0 panics, 0 RS-ERR; 5 heals + 3 underflows) |
| sched | futex 8 / sched 8 | **PASS** (0 panics; W2-WAKE-REFUSED census lines in band) |
| timer | timer 4 | **PASS** (clock monotonic never regressed; nanosleep negative average = EINTR from the class's own 1 ms timer storm — probe artifact, not a finding) |

Fixed-kernel verification (SP-1 fix1 only, task-stamp init not yet in):

| boot | delta |
|------|-------|
| forker-v2 | heals 15,577 -> 4,476, underflows 169 -> 112 (fork class alone): fresh-task false-fires gone, recycled-stamp family remains |

Census under load (syscost=1, representative, memory boot):
- IF=0 cpu0: tot=17.6s/60s rest=17.2s hard=408ms soft=21ms uart=3ms
  heal=224n miss=0n; cpu1: tot=14.6s rest=14.2s — kernel share ~27%
  with 4 malloc workers (TCG-era baseline share 83-86% interim;
  KVM numbers not directly comparable; rest-site top:
  sched.rs:2568 n=37.4M avg=834ns).
- No ALLOCTHROW / DEADLOCK / KERNPANIC classes in any boot.
- WAKE-STRANDED census fired under memory (118 s strand, pc=65537 =
  stranded hardirq bit + 1) and forker (2.6 s) — SP-1 family faces;
  re-check after the fix boots.

## SP-1 PREEMPT-HEAL false-fire on fresh fork/exec tasks (live-guard strip)

- Class: scheduler-integrity / wedge-risk amplifier
- Signature: `PREEMPT-HEAL ... was=0x10001..3 held_jiffies=0..1` then
  `PREEMPT-UNDERFLOW ... at sync/spinlock.rs:0x1b8` (SpinlockGuard
  drop) / `rwlock.rs:0x22d`.
- Two producers, both fixed:
  1. stale_200's `j == 0` arm treated never-stamped fields as
     instantly stale; fork children are born with zero stamps
     (15.5k heals/120 s under rs --fork 8). Fix: arm-on-first-
     observation in the tick stamp block (sched.rs).
  2. new_task_at did not re-initialize the four ti_pc_*_zero_jif
     stamps — recycled Task pages carried the PREVIOUS task's stamps
     (4,114 heals on fresh children persisted after fix 1). Fix:
     stamps zeroed in new_task_at (same R68a class as hung_task_since).
- ORIGIN (dfx=leakhunt): acquire sites mm_struct.rs:113 (PteLock
  spin) and :958 (vma_write), oage_jif=1 — heal fired one jiffy after
  a fresh live acquire.
- Status: FIXED (commits 70c61aad, c65fdccd); forker-v3 verification
  boot pending at write time.

## SP-2 ext4 setattr drops dirty pages + writes stale i_size on chmod

- Class: CORRUPTION (data loss under metadata churn)
- Signature: `RS-ERR fsmeta CORRUPT ...` (all 4 workers, first cycle);
  renameco: `chmod 3/300 fails, read returns n=0 (EOF)`; `rename 0/300`
  — isolated to chmod, not rename.
- Root cause (ext4_setattr):
  1. The page-cache `invalidate_inode` in the success tail ran for
     EVERY setattr flavor — chmod/chown/utime/atime dropped the
     inode's dirty unflushed pages. The reference kernel keeps the
     page cache across metadata-only attribute changes.
  2. Metadata-only setattrs wrote the disk-read Ext4Inode back with
     its STALE size (0 for files with pending writeback), persisting
     i_size=0.
- Sequence: write(64B) [page cache dirty, VFS size 64, disk size 0]
  -> chmod -> setattr writes i_size=0 + invalidates page cache ->
  read -> EOF.
- Fix: metadata-only setattr carries the live VFS size; the
  invalidate is gated to ATTR_SIZE (commit c65fdccd).
- Reproducer: test/stressng/renameco.c (op-matrix; run via
  `rs --spawn /test/stress-ng/renameco 300 /rc`); fsmeta-v3
  verification boot pending at write time.

## Notes

- rsrun's wall-clock guard killed classes at an absolute 60 s
  (RES rc=137) in the first campaign — driver bug, fixed to parse
  --timeout per class (f4beaab6).
- bigheap: brk grow is impossible for musl-static binaries (TLS
  layout pins the brk region — verified on the reference kernel
  host too); the class falls back to malloc/realloc churn with an
  RS-NOTE. brk churn coverage needs a glibc-static or custom-alloc
  probe — follow-up.

# stress-port round-2 findings (2026-11-02)

Vehicle unchanged (x86_64+KVM q35, 2G/2cpu, lane image run/rootfs-stress.img,
dfx=allocwatch,taskdump,watchdog + syscost=1, one family per boot). Kernel =
main aa43f52e + this lane's round-2 commits (probe re-tag, SP-3 fix).

## Verdict table (round 2)

| boot | classes | verdict |
|------|---------|--------|
| fsdeep | dirstorm 6x150 / dirsync 4x150 / fsmeta 8x180 | dirstorm PASS (2,024,574 ops, 0 RS-ERR, 0 corrupt); dirsync **FAIL** — fsync(dirfd)=EINVAL (SP-3); fsmeta-high PASS (1,447,788 ops, 0 corrupt — SP-2 area holds at 2x intensity) |
| dirsync-fix | dirsync 4x150 / dirstorm 6x150 (strict dir-fsync checks) | see below |
| memory2 | malloc 4x100 / mmap 4x100 / bigheap 2x100 | see below |
| brkchurn | glibc-static brk churn 240 s | see below |
| ltplong | LTP robustness long-loops (44 entries) | see below |

## Q2: IRQ-OFF-WINDOW idle-wake prints — MEASUREMENT ARTIFACT (not a violation)

The round-1 follow-up (65-142 ms idle-wake prints) is adjudicated artifact,
four independent lines:

1. The LAPIC tick is PERIODIC (no tickless idle — timer/x86_64.rs vector
   0xE0), so ticks keep falling due every 10 ms even while halted; the idle
   loop halts with IF=1 (cpu_idle_loop restores IRQs before wfi). A >100 ms
   tick gap on an idle frame therefore cannot be an in-kernel IF=0 window:
   the interrupt was due but not DELIVERED (halt-exit latency).
2. Round-1 logs contain user-frame gap lines (113 ms): user mode runs IF=1
   by definition — proof that steal-magnitude delivery delays occur with
   interrupts enabled, in the same boots.
3. syscost's edge-triggered cli/sti zone accounting (real IF ground truth,
   per-site longest CLOSED segment) measured organic REST maxima of
   <=2 ms across 88 sites in the fsdeep boot while the probe printed
   101-461 ms gaps. No kernel cli section of the printed magnitude exists.
4. The only >7 ms closed segments anywhere: ONE 2619 ms REST segment
   (cpu1, syscall/file.rs:581 — the ENOENT-log irqsave that prints under
   the lock) and ONE 2620 ms UART segment (cpu0) — the SAME early-boot
   host-stall window quantized on both CPUs (fired once, never again;
   organic uart cost on this boot was 3 ms-class). Host steal, not kernel.

Kernel change (probe honesty, not semantics): the tag now carries the
adjudication — idle frames print `WW: IRQ-DELAY-IDLE`, user frames
`WW: IRQ-DELAY-STEAL`, and only kernel frames keep `WW: IRQ-OFF-WINDOW`
(still a triage hint; corroborate with syscost zones). Verified live in
the fsdeep boot: 4 IRQ-DELAY-IDLE lines, 1 kernel-frame IRQ-OFF-WINDOW.

Guarded-domain reading: organic max closed cli segments stayed in the
3-7 ms class band (<=2 ms measured); no new IRQ-OFF print class beyond
the split artifact tags. NOT real; no interrupts-domain fix needed.

## SP-3 fsync(dirfd) returns EINVAL on ext4 — FIXED

- Class: ABI/compat + durability (fs hot path)
- Signature: `RS-ERR dirsync dir-fsync errno=22` on EVERY worker's first
  directory fsync (dirsync class, first run); dirstorm masked it by not
  checking fsync's return (probe bug, also fixed).
- Root cause: sys_fsync's blanket `!inode.mode.is_regular_file() ->
  EINVAL` refused directory fds. The reference kernel's
  ext4_dir_operations carries .fsync — fsync(dirfd) on ext4 succeeds and
  persists the directory's blocks; the durable-rename pattern
  (fsync(file); rename; fsync(dirfd)) is core ABI, and this tree's own
  ext4 namei write sites defer entry-block durability to fsync(dirfd)
  (namei.rs comments) — the FS layer waited for a syscall that could
  never succeed.
- Fix: allow `is_directory() && ext4-ops` inodes through to
  ext4_sync_file (syscall/file.rs). ext4_sync_file is directory-safe:
  it walks the inode's data blocks (directory blocks), syncs the
  inode-table block, commits. Non-ext4 dirs keep EINVAL (reference
  parity for fs without a dir fsync op).
- Verification: dirsync-fix boot (below).

## Q3: WAKE-STRANDED / preempt-guard residual family under memory churn (SP-1-fixed kernel)

memory2 boot (malloc 4x100 / mmap 4x100 / bigheap 2x100, full budgets,
dfx=allocwatch,taskdump,watchdog + syscost=1):

- malloc RES rc=1: `RS-ERR worker 308 needed SIGKILL (wedge)` — the
  class's first WEDGE of the campaign (round-1 memory boots: 0).
- Census on the fixed kernel: 21 PREEMPT-HEAL / 3 PREEMPT-UNDERFLOW /
  1 WAKE-STRANDED (100,469 ms, visits=22,262). Round-1 baseline memory
  boots: 118 s strand face + 12 heals + 2 underflows (leakhunt boot).
- The strand is NOT a CPU hog: at pick time both CPUs ran healthy
  3-5 ms activations (h0=311/3ms/pc=1, h1=313/5ms/pc=4). The task sat
  registered-nowhere through 22k wake-path visits — the same
  lost-park/stranded shape the OH lane hunts.
- Chain, in order: pid 0x133 (decimal 307 = the rs malloc parent)
  carried preempt_count 0x10004 (HARDIRQ + 4) for 200.7 s
  (held_jiffies=0x4e66) -> wakeups refused (100 s strand) -> heal
  stripped the bits -> its NEXT SpinlockGuard drop underflow-clamped
  (spinlock.rs:0x1b8, lock in .sdata right after mm::pglist::NODE_DATA
  — the page-allocator path lock) -> 2 more underflows on other tasks
  (0x13c, 0x13d). Underflow-after-heal = heal-induced, secondary.
- NEW false-fire attribution: 17 of 21 heals fired at held_jiffies=0
  with was=0x10001/0x10004 on two LIVE mmap workers (pids 0x13c/0x13d).
  Mechanism: the ti_pc_*_zero_jif stamps refresh only on the task's
  OWN CPU tick; a task parked mid-guard through a host steal window >
  2 s (measured to exist: Boot A's simultaneous 2619/2620 ms segments
  on both CPUs) resumes with "old stamp + bits present" and is healed
  LIVE — the guard's paired decrement then underflows. The SP-1
  arm-on-first-observation fix removed the fresh-fork producers; the
  steal-window producer remains.
- Also observed: doubled direct-console lines ("PPRREEEEMMPPTT...")
  — two CPUs printing the heal lines concurrently without a console
  lock (known R34 shape, cosmetic, but garbles these specific lines).
- ACTION: memory-hunt2 boot (dfx=leakhunt) targets the 200 s leak's
  ORIGIN acquire site; the steal-window false-fire argues the 200-tick
  bar needs a steal-aware exemption (do not heal a task whose CPU was
  descheduled through the window). OH lane hunts the strand family;
  numbers above are the residual census it asked for.

## Q5: brk-churn via glibc-static — PASS (first real sys_brk coverage)

brkchurn boot (glibc-static probe, 240 s: raw sbrk waves 64-512 KB in
one child + glibc malloc/free trim-threshold churn in another):

- BRK-DONE sbrk_ops=864,320 (212 GB cumulative grown+shrunk+verified),
  malloc_ops=114,918,907 (1.15 TB), enomem=0, bug=0, rc=0.
- The glibc-static binary itself is a compatibility datum: the
  reference-kernel-shaped ELF loads and runs as PID-1 child with no
  issue. The sys_brk grow/shrink paths (which musl-static binaries
  physically cannot reach — round-1 bigheap finding) took ~864k
  round trips with zero failures and zero pattern mismatches.
- Probes: rs --spawn vehicle extended (rsrun exec form); source
  brkchurn.c archived in userspace/tests/probes.tar.gz.

## Q4: LTP robustness long-loops — running (detached, 4 h budget)

ltplong.cmds: mmapstress01-10 x3, mmap-corruption01 x3, msgstress01
x3, msg_comm x3, sem_nstest x3, fork_procs x2, fork_exec_loop x2,
growfiles x3 (49 entries, 300 s budget each via the rsrun exec form).
Interim: mmapstress01 rc=0 x3; mmapstress02 rc=1 "errno = 12;
couldn't round up brk" (brk ENOMEM under the test's rounding loop —
adjudicate against the round-1 LTP baseline; the new brkchurn PASS
says the plain brk path is healthy, so this is likely the test's
aggressive roundup shape, not a regression). Verdict when the boot
completes; next session reads run/stress-runs/ltplong.log.

Driver bug found+fixed in-flight: rsrun's exec form passed a
stale-tail argv (the NULL terminator was placed before two token
shifts — argv gained duplicated tail args; LTP's new API saw the
binary path as an argument: "Unexpected argument(s)"). Fixed
(re-terminate at the shifted position, argv[0]=basename for ltpscan
parity); host-verified argv exact; ltplong relaunched on the fixed
rsrun.

## Q4 final adjudication (ltplong + ltplong3 boots)

- PASS rc=0: mmapstress01 x3, mmap-corruption01 x3, mmapstress09 -p 20
  -t 0.2, mmapstress10 -p 20 -t 0.2, fork_procs -n 100 x2, sem_nstest,
  growfiles (ltplong3, with canonical args).
- NOT a bug (host reference kernel identical failure): mmapstress02/
  03/05/08/06 "couldn't round up brk" errno=12 — the 1990s-era static
  binaries' brk roundup fails on the modern reference kernel too
  (verified: mmapstress02 on the host reference = same TFAIL line).
- Usage-shape fixed in ltplong3 by adding canonical runtest args
  (mmapstress09/10 now pass).
- REAL suspect family (SP-4 candidate, INTERMITTENT): in the ltplong
  sweep, fork_procs (bare), fork_exec_loop x2, growfiles x3, msg_comm
  x2, sem_nstest x3 and 1 of 3 msgstress01 runs returned rc=254 with
  EMPTY output = rsrun's waitpid-ECHILD path — exec'd glibc-static
  children dying silently before any userspace output. In ltplong3
  (8 entries, incl. 2 of the same binaries): 0 recurrences. Intermittent
  ~1 boot in 2 today; bare fork_procs passes on the host reference.
  msgstress01's mixed 2xTBROK(EINVAL msgsnd)+1x254 in one boot is the
  cleanest evidence the death is a race, not the binary. Suspect:
  same fork/exec + strand/wake family as the malloc-boot wedge.
- Functional datum for the LTP lane: msg_comm TFAIL "messages leak
  between namespaces" (rc=3; needs root, so in-guest only).

# stress-port round-3 findings (2026-11-02)

Kernel = main-side merge 3383cf79 + this lane's ac0ff7f6 (heal-bar fix).
Vehicle unchanged (x86_64+KVM q35, 2G/2cpu, lane image, one family per
boot). DFX: allocwatch,taskdump,watchdog (+leakhunt where noted).

## Q1: the "200.7 s preempt_count 0x10004 leak" — ROOT-CAUSED, not a
## missed put; it was a three-defect false-fire chain in the heal bar

Reconstruction (memory2 + memory-hunt2 + fix1/fix2 boots):

1. held_jiffies is WINDOW AGE (first nonzero observation of the task
   on that CPU's LEAK_WIN slot), NOT continuous hold. The "200.7 s"
   never was a 200-second count hold.
2. The first heal's own leakhunt ORIGIN record showed oage_jif=1 with
   held=0x4e6c — an outermost 0->1 acquire ONE jiffy before the heal.
   With a genuine solid base=4 hold, inner pairs (4->5->4) cannot
   rewrite the o-record; oage would be ~20067. The base was churning
   through live guards at heal time. NOT a leak.
3. Three bar defects made the tick-side PREEMPT-HEAL fire on those
   live guards:
   a. Absence-stamp AGE accrued for free while the task was off-CPU
      (only the task's own CPU tick refreshes its stamps). The strand
      victim parked through the 100.4 s pick-skip strand with frozen
      stamps; its first post-resume tick sampled the resumed
      syscall's live guards (base 1-4) against a 200+ tick-old bar
      and healed them -> the paired SpinlockGuard drop underflowed at
      spinlock.rs:0x1b8 (the page-allocator zone lock: mm/zone.rs:567
      is exactly that guard's acquire).
   b. jiffies is timebase-DERIVED (drivers/timer/x86_64.rs: jiffies =
      floor(elapsed/period)), so a >2 s host steal window inflates
      stamp age by 200+ ticks with ZERO ticks delivered — round-2's
      steal-window suspicion is real in kind.
   c. The pb3 release-side re-arm stored the SUPERSEDED ti_pc_zero_jif
      field (zero readers anywhere in the tree), so the per-field bar
      the heal gates on NEVER re-armed on legal 1->0 drains. Lock-hot
      tasks (malloc/reclaim churn) cross zero for microseconds between
      guard pairs; the 100 Hz sampler keeps missing those gaps and
      200-present-sample streaks form — the held_jiffies=0 heal storm
      (17-30 fires per episode, oage_jif=1 on every line).
4. The heal itself then never re-armed anything: post-heal presence
   was judged against the pre-heal (still stale) bar -> the storm.
   Each storm fire stripped a LIVE guard's bit; every subsequent
   unpaired drop clamped at zero (the "induced underflows") — pure
   poison, zero diagnostic value.

FIX (ac0ff7f6): the heal bar now counts CONSECUTIVE-PRESENT
OBSERVATIONS (ti_pc_{pirq,sirq,hirq}_present_run), bumped only by the
task's own-CPU tick when the field is present, reset on absent
samples, on legal drains (release-side, now writing the field the
gate actually reads), on every external zeroing (heal paths, underflow
clamps), and zeroed at task creation. No tick, no increment: off-CPU
time and host steal cannot inflate the bar. Parked phantoms stay
covered by the parkwalk's 4 s PARKED-HEAL (no tick needed); on-CPU
monopolists still reach 200 observations in 2 s.

Verification (fixed kernel, leakhunt on):
- memory-fix1 + memory-fix2 boots (same repro classes as round 2):
  PREEMPT-HEAL 0, PREEMPT-UNDERFLOW 0, WAKE-STRANDED 0, PARKED-LEAK 0,
  W2/LP lines 0. Round-2 same-boot census: 21 heals / 3 underflows /
  1 strand(100.4 s) / heal storm.
- PARKWALK-CENSUS (new, dfx=leakhunt): walks 10,000 obs ~82,600
  parkednz=0 per boot — the walk is NOT blind; no task ever parked
  with a nonzero count in these boots. This settles the round-2
  "why did PARKED-HEAL miss the 100 s strand" question: the victim
  parked CLEAN (count 0); the 0x10004 face arose at RESUME from the
  stale bar. The strand itself (R85 orphaned-mark/lost-park family)
  remains the OH lane's bug — on the fixed kernel its wake-side face
  no longer corrupts counts.
- forker-v4 (fork 8 + exec 4, leakhunt): PREEMPT-HEAL 0,
  PREEMPT-UNDERFLOW 0 — SP-1 families stay dead at round-1-fixed
  levels, no regression from the new bar.

## Q2: steal-aware staleness exemption — DECIDED, implemented as
## observation counting (no exemption needed)

The round-2 proposal was "do not heal a task whose CPU was
descheduled through the window". The data says something cleaner: the
bar's unit was wrong, not its threshold. Guest jiffies derive from the
timebase, so steal inflates jiffy-deltas without delivering ticks; the
old bar therefore counted TIME the CPU did not spend observing. The
new bar counts OBSERVATIONS (delivered ticks that sampled the field
present); host steal and off-CPU gaps contribute zero by construction.
A steal-aware exemption on top of the old bar would have masked
defect (c) (the wrong-field re-arm) — the storm was NOT steal-caused;
oage_jif=1 on every storm line proves live churn. No exemption logic
added anywhere.

## NEW finding (mm domain, handed to the record): direct-reclaim gate
## livelock under full-zone exhaustion — SP-5 candidate

memory-fix1 boot: malloc class made ZERO progress (ops=0 for 100 s;
round-2 boots completed 4.45M ops). Census: zone genuinely exhausted
(used=524213/524252, free_cnt=0), two workers self-SIGKILL on OOM
("pagefault: Out of memory"), and the parent (malloc holder 306) sat
in direct reclaim with in_direct_reclaim=306 gate_exit_releases=0
across 4+ OOM-FORENSIC dumps — while a later dump showed
zone_free=33462 pages returned but the holder still gated. The heal
side is NOT involved (0 heals in the round-2 malloc windows too — the
first heal in hunt2 fired only at line 647, after the malloc class
finished at line 343; variance between boots is the mm family, not a
heal regression). Shape: reclaim's exit gate does not release (or the
holder never re-checks) once memory returns. Reproducer = the memory
class boot itself (malloc 4x100 --vm-bytes 256M on the 2G image);
owner: mm lane (guarded domain memory allocation/reclaim). fix2 boot
completed malloc normally (227k ops @t10) — INTERMITTENT, same
~1-in-2 cadence class as the fake-OOM history.

## forker-v4 ACQUIRE-PAIR-SPLIT lines (first observation of the
## forker+leakhunt combo; benign, recorded for the preempt hunt)

24 budgeted lines (cap 40): the o-record names a different task than
the 1->0 releaser under fork-8 churn (Task pages recycle through a
small address set; slot succession, not aliasing — Fibonacci hashing).
osites: sched.rs:2580 and sched.rs:6041 (scheduler-internal
preempt_disable sites). Zero heals/underflows alongside. Not a
stability failure; useful input to the stale-tp booking hunt (omis).

## Round-3 verification addendum

- execaudit in-guest (execaudit boot): 560 audited execs of
  glibc-static brkchurn (280 blocking + 280 WNOHANG discipline), 2
  sibling forkers each — events=0, echild=0, badstatus=0, wrongpid=0.
  rc=143 on both invocations is rsrun's budget TERM (sequential rounds
  cost ~8 s; budget was 260 s) — not a probe failure. The rc=254
  family stays intermittent; the standing reproducer is now in the
  vehicle (`exec <budget> /test/stress-ng/execaudit -r N -b B -f F
  [-t target] [-w]`).
- Soaks on the fixed kernel: dirstorm 12x150 (4x round-2 intensity)
  PASS 2,139,796 ops rc=0; fsmeta 8x180 PASS 1,375,896 ops rc=0;
  brkpressure (glibc brk churn + leakhunt) ~90 s: 226k+ sbrk ops /
  55+ GB, 29M+ malloc ops / 290+ GB, enomem=0, bug=0.
- Heal-family census across ALL round-3 stress boots (fix1, fix2,
  forker-v4, fsdeep4x, execaudit, brkpressure): PREEMPT-HEAL 0,
  PREEMPT-UNDERFLOW 0, WAKE-STRANDED 0, PARKED-LEAK 0.
- make test (riscv64 suite, post-fix kernel): ALL TESTS PASSED
  (MAKE-TEST-RC=0) — including the sgid_mode group that the round-3c
  heal policy once wedged.

## Round 4 (2026-11-03, new main a409deaa = origin/main d2b2672a + SP-5 census)

Baseline re-verify on the merged main (negmemo + vm-op fast paths +
chroot fast base landed): fsmeta 8x180 PASS rc=0 (fsmeta-r4base boot,
~1.34M ops by t=180; SP-2 area still clean under the new fs path).

## Negmemo validation (queue item 4) — boot 1 PASS

negstorm class (new probe, committed 63e80291): 8 workers x 180 s,
cmdline negmemo=1 fsw=1, shared parents + per-worker unique names,
stat-ENOENT-prime -> create/link/rename-in/mkdir/symlink -> strict
verify (STALENEG = stale negative hit), teardown ghost check, common
24-name churn. VERDICT: PASS rc=0 — 1.9M ops, neghit=274,763,
negstale=479,832 (memo heavily exercised + invalidated), ZERO
STALENEG / VERIFY-OPEN / GHOST / TAG / PRIME events. d2b2672a's
invalidation holds under this oracle on boot 1. (Sweep continues —
more boots below.)

## SP-5 (queue item 1) — sweep boot 1: the face reproduces, phase named

malloc 4x256M boot (sp5-r4-boot1): RES rc=1 with the round-3 face —
worker 307 WEDGED (parent SIGKILL), 4.07M ops first. OOM-FORENSIC
caught the gate held: in_direct_reclaim=310 across BOTH gate-bearing
dumps with **phase=5 (SWAPW), rounds=0, freed=0, deep=0, waiters=1,
exhausted=4, selffight=0** — the holder is INSIDE the first
try_to_free_pages round, in reclaim_anonymous_pages ->
swap_write_page (synchronous blkdev_write -> virtio park), NOT
self-fighting (selffight=0: no nested alloc_pages from the holder's
reclaim I/O) and NOT progressing (rounds=0 across dumps). Meanwhile
zone_free moved 0 -> 19,966 between the two dump contexts (other
tasks' deaths returned memory) — matching round 3's "memory returned,
holder still in reclaim".

Census mystery: ZERO GATE-CENSUS lines in boot 1 despite the 1 Hz
heartbeat census being unconditional and holder present across dumps
— either (a) gate CHURN (short release+reacquire holds, age never
reaches the 200 j bar; the OOM snapshots just always catch pid 310
holding because it lives in the slowpath), or (b) the 1 Hz heartbeat
slot starved under the OOM storm. Discriminating instrumentation
committed: GATE_ACQS cumulative acquire counter + taken_age on the
OOM-FORENSIC gate line + GATE-LONGHOLD release-audit line (prints at
RELEASE from process context — immune to heartbeat starvation).
Boots 3+ of the sweep run this kernel.

## SP-5 root-cause correction (sweep boots 1-6 + discriminator)

The discriminator fields killed the "holder stuck for minutes"
reading: taken_age=0-1j, acqs=1-2 TOTAL, longholds=0 at every dump —
the gate is acquired once or twice in the whole cascade, every hold is
sub-second (no GATE-LONGHOLD line ever printed), and the phase=5 SWAPW
snapshots catch a FRESH round. Round-3's "in_direct_reclaim across 4+
dumps" was the same snapshot artifact: dumps are rate-limited to the
cascade's first seconds, and in_direct_reclaim is held most of that
wall time because every fault goes direct-reclaim.

The REAL damage: (a) rs's "worker N needed SIGKILL (wedge)" line is
NOT a 15 s wedge — reap_all's escalation prints "unreapable, SIGKILLed"
(a different line) which NEVER fires; the SIGKILL deaths are the
KERNEL's pagefault fake-OOM self-kills. (b) The kill chain: waiters
exhaust the 64-attempt gate-wait (exhausted=4..8 vs acqs=1-2) while
the holder's first round is in flight -> alloc_pages returns 0 ->
pagefault OOM strikes -> worker dies by SIGKILL while the zone is
about to refill (dump #2 site: "alloc_pages(order=0) failed with
free_cnt>0" — free 15,963 pages minutes later; failure instant and
dump instant differ).

Committed (b5a6dc3d): reference-parity progress-reset gate wait
(RECLAIM_PROGRESS from balance_pgdat), fatal-signal bails (waiter +
holder rounds + deep drain), PF_MEMALLOC-parity holder-nested skip,
swap-write latency census in the OOM dump. Fixed-kernel boots 5-6:
same class rc=1 (kills persist) — kill-chain breaker still open;
swap-write latency is the measured next step (boot sp5-swapcensus).

## execaudit multi-boot sweep (queue item 2) — no recurrence

5 variants in one boot (burst 16 x 80 rounds x both disciplines on
brkchurn; growfiles + fork_procs LTP targets with anyexitok):
3,520 audited execs, events=0, echild=0, wrongpid=0, badstatus=0.
Including round 3's boot: 4,080 audited execs across 2 boots, zero
events. The rc=254 silent-child-death family did NOT fire; it stays
rare-intermittent (round-2 cadence: ~1 boot in 2 — today 0/2). The
standing reproducer (execaudit in the vehicle) is the right tool;
next round should sweep more boots with the SP-5 fix in place (the
family shared boots with the strand/leak + fake-OOM history).

## Negmemo validation boot 2 — PASS (16 workers, 2 vCPU = tighter interleaving)

negstorm 16x180: rc=0, neghit=200,402, negstale=483,127, zero oracle
events. Combined with boot 1: ~1,055k neghits + ~963k stale
normalizations across both shapes, ZERO stale-negative deliveres.
d2b2672a's negative memo invalidation is holding under the full
create/link/rename-in/mkdir/symlink oracle with cap-eviction churn.
(NOTE: negmemo requires cmdline negmemo=1 — it is default-OFF; the
stress vehicle now carries that in the negstorm boots.)

## SP-5: the clock discriminator settled it — TIMER IRQs STOP on both
## CPUs during the malloc-exhaustion ramp (timers/interrupts domain)

Clock boot (boot-sp5-clock): "OOM-FORENSIC: clock tb=6173251 irq0=46
irq1=50" at dump #2 and "tb=7051524 irq0=46 irq1=50" at dump #3 —
the TSC/timebase ADVANCES between dumps while the per-CPU timer-IRQ
counters are frozen at 46/50 (~0.5 s into the boot, right as the
malloc class's fault storm ramps). jiffies froze at 45 because its
increment runs only from the timer IRQ (increment_jiffies_from_timebase
is called in timer_interrupt_handler; nominal from tb would be ~70).

Consequences (all observed): the 1 Hz heartbeat slot dies (GATE-CENSUS,
WW, fuxstuck, binder audits — every "silent" census during the SP-5
cascade), the fault-OOM kill windows never elapse (strikes-only kills
— CAS(last==now) always succeeds, strike table is count-based and
still works), syscost's jiffies cadence dies, watchdog heartbeats die.

NOT reproduced by: fork-only storm (t=60s syscost fired, tick alive),
mmap churn (same), negstorm 16w (PC2 census advanced to t=270s),
fsmeta/timer classes (t=180s syscost). The death is specific to the
zone-to-zero exhaustion ramp — the trigger lives in the
reclaim/OOM-slowpath machinery (kswapd launch, first direct reclaim,
or an LAPIC arm/mask interaction on that path).

Handoff for the timers/interrupts owner (guarded domain): instrument
the LAPIC state (timer LVT mask bit, TMICT, IRR/ISR for vector 0xE0)
plus the last re-arm site stamp, sampled when WW_TICK_IRQ[c] is frozen
>5 s while tb advances. The one-shot re-arm chain is
trap.rs:1546 -> set_next_trigger -> apic::timer_arm_oneshot (counts
clamped [1, u32::MAX]; a reg_write of TMICT=0 or an LVT mask from any
other path would exactly produce this signature).

NOTE for SP-5 verdicts: every jiffies-derived silence during the
cascade (zero GATE-CENSUS/GATE-LONGHOLD context) is explained by the
dead tick, NOT by short holds alone; the reference-parity reclaim-wait
fixes (b5a6dc3d) remain correct on their own semantics.

## ltplong4 (queue item 3) — FULL PASS

One boot, sequential budgets: mmapstress07 x2 (canonical tmpfile args
`<file> 4096 1 0` — TPASS both; the round-2 usage gap closed),
mmapstress01, mmap-corruption01, fork_procs -n 200, growfiles,
mmapstress09 -p 40 -t 0.4 — all rc=0/TPASS, no panics, no wedges.

## Round-4 gates

- Builds: x86_64 rc=0 (several), riscv64 rc=0 (clean config), aarch64
  rc=0 (dtb_blob.S restored after the build, per policy).
- make test (riscv64 suite): verdict GREEN (wrapper rc=124 after the
  kernel halt = the normal pattern); suite QEMU killed by exact
  rootfs.img cmdline match.
- Tier-1 note for the mm change (b5a6dc3d + census commits): the
  changed code is exhaustion-slowpath-only (unreachable on standard
  boots); always-on delta = one relaxed fetch_add per reclaim round
  (balance_pgdat) + dump-only prints. All round-4 stress boots carried
  syscost=1: no new IRQ-OFF / panic / OOM classes outside the known
  exhaustion family.
- Tier-2 pbench (r4sp5, debug profile, DONE t=240s, 25/25 rows):
  p1_null / p1_clock / p3b_cow / p6_write_fsync / p6_mkdrmdir in band
  vs the pb1 anchor; p2_ctxsw -27%, p6_append -84%, p6_unlink -98% —
  all out-of-band rows are on the FAST side, consistent with main's
  own vm-op fast paths + negmemo + chroot fast base (which postdate
  the pb1 anchor), not with this round's slowpath-only change. Zero
  regressions; anchor re-bank belongs to the perf lane.
- Pushed origin/stress-port 65123b48.

# stress-port round-6 findings (2026-11-03, verification round)

Kernel = merge 1a560c8c: origin/main e264a64b (carries the round-5
dead-tick fix as d3700224 + r23 LTP wave + gap-fs lock-free walk
3f377d13 + code-review-perf) + this lane's r5 instruments + SP-6 fix
(this round). Vehicle unchanged (x86_64+KVM q35, 2G/2cpu, lane image,
one family per boot). DFX: allocwatch,taskdump,watchdog + syscost=1
(leakhunt where noted). Logs: run/stress-runs/*-r6*.{out,log} +
tickreg-fix2/negstorm*/forker-lh.

## Mission 1 — dead-tick fix verified end-to-end (2 boots, deterministic repro)

- tickreg-fix1 (round-5 fix kernel) and tickreg-fix2 (this round's
  merged kernel), malloc 4x256M exhaustion cascade, dfx=leakhunt:
  OOM-FORENSIC clock lines MONOTONIC and ADVANCING on both CPUs
  (fix1: irq 58/61 -> 97/109; fix2: 69/74 -> 117/100). Pre-fix round-4
  signature (tb advances, counters frozen at 46/50) is gone.
- TICKTRAP verdicts: all "counting" (LAPIC live, arms advancing,
  armop=1, tmcct>0); ZERO irr-stuck / lvt-masked / apic-disabled.
- Census lines FLOW under load: SYSCOST t=...s lines to window end,
  PARKWALK-CENSUS every 1000 walks, GATE-CENSUS would print (0 lines =
  no >=2 s gate holds; consistent with r4/r5 sub-second-hold findings).
- Regression driver committed (test/stressng/tick-regression-x86.sh,
  dcdee4e9): PASS on both fix boots. NOTE its parser originally
  mis-grouped columns (bare "[0-9]+" split also matched the digits in
  the irq0/irq1 FIELD NAMES) — fixed; r4 pre-fix logs SKIP (the clock
  instrument postdates them), the pre-fix evidence lives in the
  8884c2b3 commit message (TICKTRAP IF=false irr7=0x29 tmcct=0).

## Mission 1b — SP-5 re-adjudicated with living clocks: TRANSFORMED, closes as a timers-domain kill (fixed)

Post-fix kill timeline (tickreg-fix2, the deterministic SP-5 shape):
dumps #1-#3 at TRUE exhaustion (free_cnt=0..2, zone_free=1-2); two
faulting tasks strike out and die at free_cnt=32 (the designed
3-strikes-after-3-consumed-kill-windows discipline, clocks alive so
the 1 s windows elapse properly); ONE score-based oom-killer kill
("oom-killer: Killed process 308 (rs) score 63954"); the class then
runs its FULL 100 s budget — 5.42M ops, RS-BEAT every 10 s to t=100,
RS-ALL-DONE. The pre-fix fake-OOM chain (workers dying en masse by
strike-only kills while the zone refilled, class truncating at ~4M ops
with RES rc=1 "wedge" mislabels) is GONE. Zero strike-ONLY kills.
The same shape held in the memory-family boot (malloc 4.30M ops rc=1
with one score-based kill at exhaustion; mmap rc=0; bigheap
malloc-mode rc=1 at exhaustion). Residual for the mm owner (unchanged
from r4/r5, now with live clocks): dump #37 shows an
alloc_pages(order=0) failure site with free_cnt=8108/zone 53k — the
failure-instant vs dump-instant skew (or broken-free-list class);
swap reclaim still produced zero completed writes at every dump.

## Mission 2 — family re-baseline post-fix (one boot per family)

| family | boot | verdict |
|---|---|---|
| memory | memory (r6) | PASS on panics/wedges; ticks alive through the cascade (51/56 -> 100/89); kills only at true exhaustion (see SP-5); malloc 4.30M rc=1, mmap 1.14M rc=0, bigheap-malloc-mode 16 ops rc=1 (musl brk limit -> malloc fallback at zone exhaustion; brk coverage itself = brkchurn PASS r2/r3) |
| sched | sched (r6) | PASS — futex 377,847 rc=0, sched 17.3M rc=0; 16 W2-WAKE-REFUSED lines (futex-storm in-band class), 0 strands |
| timer | timer (r6) | PASS — 1.72M ops rc=0, 0 panics |
| ipc | ipc (r6) | PASS — pipe 33.2M / sock 19.9M / sem 285.9M / msg 58.6M / shm 61.6k ops, all rc=0, 0 RS-ERR |
| fsmeta | fsmeta-v3 (r6) | PASS — renameco chmod/link/symlink 300/300 fail=0 each (SP-2 area), fsmeta 799,332 ops rc=0 |
| dirstorm | dirstorm (r6) | PASS — 1,315,133 ops rc=0 |
| negstorm | negstorm (r6) | **CRASH — SP-6 below**; after the fix: 8w 1.63M ops rc=0 (neghit 156,575 / negstale 275,007, 0 oracle events) and 16w 1.57M ops rc=0 (neghit 121,608 / negstale 293,508, 0 oracle events) |
| execaudit | execaudit (r6) | PASS — 5 variants one boot: 4,800 audited execs, events=0, echild=0, wrongpid=0, badstatus=0 (rc=254 family: no recurrence; campaign total incl. r3/r4 now 8,880 execs / 0 events) |
| forker | forker + forker-lh (r6) | PASS — fork 475,892 / exec 178,290 rc=0 (default DFX); leakhunt boot fork 447,552 / exec 169,274 rc=0 with PREEMPT-HEAL 0 / UNDERFLOW 0 / WAKE-STRANDED 0 (SP-1 stays dead with the fix in); ACQUIRE-PAIR-SPLIT 40 (cap) — the known benign slot-succession class |

Kernel-provenance note: the campaign's first six boots (memory ..
negstorm-crash) ran the pre-SP-6 kernel; execaudit/forker boots and
every verification boot after SP-6 ran the fixed kernel (bzImage
rebuilt mid-campaign; the crash family is negmemo-gated and
default-off, so the execaudit/forker verdicts are unaffected).

## SP-6 (NEW this round, FIXED + VERIFIED) — stale negmemo hit hands a NULL dentry to the dcache-hit deref

- Class: CORRUPTION-adjacent crash (kernel NULL deref) — fs walk,
  negmemo-gated (cmdline negmemo=1, default OFF).
- Face: r6 negstorm boot 1 + repro (deterministic, first stale hit):
  `KERNPANIC pfault badaddr=0x98` in path_lookup -> Dentry::
  is_negative -> Atomic<bool>::load; two tasks faulted at the same
  rip (second = "re-entered during dump"); VFS-MUTATION-STALL
  holder=311 held=18001j with 6 waiters followed (the panic froze the
  holder); ops froze at 65. Symbolized chain: file_open ->
  path_lookup -> is_negative; RDI=0x98 = &NULL.negative.
- Root cause: the gap-fs lane's lock-free walk translation (3f377d13,
  2026-11-03 08:04, on main AFTER the round-4 negmemo validation
  boots) rewrote the 21st-successor stale-memo arm. The pre-gap-fs
  code returned `None` (the Option arm selector) on a stale hit ->
  the fs re-lookup arm ran ("detach and re-resolve"). The new code
  returned `core::ptr::null()` as the match VALUE for child_raw,
  which flowed straight into the unconditional dcache-hit deref
  (`let c: &Dentry = unsafe { &*child_raw }` at vfs.rs:2272) — first
  stale hit = kernel panic. negmemo is default-off, which is why
  standard boots, LTP, and the gap-fs lane's own validation never
  saw it; the negstorm oracle (negmemo=1) walks into it within
  seconds.
- Fix (this round, kernel/src/fs/vfs.rs): the stale case now sets
  `found = None` BEFORE the match (if let Some(p) = found ... stale ->
  remove_child + found = None), restoring exactly the pre-gap-fs
  fall-through-to-fs-re-lookup semantics. No behavior invented.
- Verification: negstorm 8w + 16w on the fixed kernel (numbers in the
  table): 568,699 stale normalizations processed through the fixed
  path, zero panics, zero oracle events. Also forker-lh boot (walk
  heavy, negmemo off) clean.
- DISCIPLINE NOTE for the gap-fs owner (pair 2 entry point): any
  `Option<*const T>` match whose arms produce raw VALUES must keep
  the miss path as an ARM SELECTOR, never as a sentinel value;
  lookup_child_fast (dentry.rs:813) is the clean counter-example.

## Watch items (recorded, not chased)

- PARKED-LEAK 1 line in forker-lh (pid 0x657c pc=1 state=0 tcpu=1;
  PARKWALK-CENSUS parkednz 0 -> 1 over 5,000 walks / 80,744 obs,
  stayed 1): a single runnable-not-running task sampled with
  preempt_count=1, self-resolved, zero heals/underflows/strands all
  boot. Rounds 3-5 leakhunt boots: 0. Possibly the first face of the
  fix widening park/sampling interleavings (IRQs now on across
  switches). OH lane's strand hunt owns the family; re-census next
  round before adjudicating.
- riscv64 make-test (r6, post-merge pre-SP-6 + post-SP-6 suite runs
  both verdict-green): ONE "WW: TIMER-SOFTIRQ-STALL cpu=2 age=1053ms
  tick_age=1054ms pid=0" at the very end of the suite = the known
  post-verdict halt-time family (idle context, kernel halted; same
  class as the halt-drain cosmetic in r4 gates). Not a live class.
- p2 anchor drift: pb1's 15.409 µs debug anchor is now ~30% slower
  than reality on main-derived kernels (r4sp5 11.245 -> r6 10.687/
  10.091); the perf lane should re-bank the Tier-2 anchor.

## Mission 3 — sched-domain perf evidence with the fix

- pbench P2 (context-switch half-rt; TCG smp2 mem2G, debug profile,
  same rig as r4sp5): pre-fix r4sp5 med 11.245 µs; post-fix r6 two
  reps 10.687 / 10.091 µs (DONE rc=0 both, zero panics, foreign_qemus
  2-4). The fix does NOT regress context-switch cost (min-of-2 is
  ~10% FASTER than pre-fix; single-run caveat on both sides). Both
  sit ~27-31% fast of the stale pb1 anchor (15.409) — the same
  fast-side drift round 4 attributed to main's own fast paths; not
  this fix. Other rows in family with r4sp5 (p3a 0.080, p5b
  20.7/41.7, p6 openclose 106.9, append 83.1; p6_unlink 861.6 vs
  751.7 = +15%, in the ±20% band; p3b 23.5, p5e 1640).

## Mission 3b — Tier-1 quiet census boot on the OH image with the fix

pa-boot r6fix (q35 KVM smp2 4G, syscost=1 dfx=wwcensus, 780 s window,
tok109 userdata lineage, debug kernel with SP-6; serial
run/pa-r6fix-serial.log / -stamped.log):
- census TERM-DESCHED engaged = 8 (band 8 +- 2 — dead center),
  WAKE-REFUSED = 32 (band 32 +- 4 — dead center).
- WAKE-STRANDED = 6, all organic shapes: three 1.0-1.7 s strands
  (visits 2.6k-8.3k, healthy ms-class activations) + pid 305 at
  25.9 s / 48.2 s (inside the baseline's 32-107 s organic envelope);
  NO 1001 ms-grid class.
- tick rate sustained to window end: c0 5,981 ticks in the last 60 s
  window = 99.7/s; c1 5,997 = 99.95/s (bar >= 99.5/s).
- 0 kernel panics, 0 OOM lines across the boot.
The sched guarded domain holds with the dead-tick fix in: census
bands, tick rate, and P2 all in shape on the same kernel.

## Round-6 gates

- x3 builds rc=0: x86_64 (pre- and post-SP-6), riscv64 (clean config,
  post-SP-6), aarch64 (dtb_blob.S restored after the build).
- make test (riscv64 suite) GREEN twice this round: post-merge
  pre-SP-6 and post-SP-6 (ALL TESTS PASSED; suite QEMU killed by
  exact-match discipline; one halt-time TIMER-SOFTIRQ-STALL line at
  the very end of the pre-SP-6 run = the known post-verdict halt
  family, see watch items).
- Pushed origin/stress-port (dcdee4e9 driver, 87c51458 SP-6 fix +
  findings, session-close commit with TASK.md).

# Round 7 (2026-11-03) — SP-7: why swap never completes, NAMED and FIXED;
# PARKED-LEAK becomes a family; P2 anchor re-banked to KVM

Sync: merge d734ed49 = origin/main 91de1d04 (r6 close round-trip; the
vfs.rs conflict was the known SP-6 double-fix convergence, comment-only,
main's wording kept + r6 validation note). Kernel commits: fe6279c9
(instrument; also carries the vmscan rotation-direction hunk — it was
edited before the split, noted here for the merge reviewer), 5c95f612
(fix), e4d70ee6 (perf-gate re-bank).

## Mission 1 — the mm-owner residuals: swap starvation decoded, fixed, verified

Instrument (fe6279c9): per-page skip-reason census in
reclaim_anonymous_pages — every filter branch and terminal path counted
(notanon/notsb/unmapped/rc-buckets/rsvlock/rotref/slotnone/toctou/
unmap0/partial/werr/putnz/freed + want-vs-got), printed by OOM-FORENSIC
next to the swapwrite line + a self-gated 1 Hz delta heartbeat (silent
without anon pressure, 300-line budget) + the reference-shaped
vmscan:mm_vmscan_lru_shrink_inactive tracepoint (unified trace table,
id 31; flags field omitted on TRACE_ARGS capacity; anon arm).

BASELINE (exhaustion class = memory-fix2 cmds, instrument-only kernel,
sp7-instr2-memory.log): lru[0]=245,515 anon pages, swap active.
calls=11 invocations the WHOLE boot; want=got=3,589 examined (1.5% of
the pile); rotref=1,642 (46%); freed=wok=1,240; notanon=236; EVERY
error bucket zero (werr=putnz=partial=toctou=rc*=slotnone=0). The swap
pipeline completes 100% of admitted pages — locking, accounting, and
zone wiring are all CLEAN. The skip mass is one bucket: rotref.

WHY (three producers, all reference divergences):
1. The ALLOCATOR plants Referenced on fresh pages (zone.rs block-leader
   set + pcp.rs to_in_use). The reference's prep_new_page hands out
   flag-clear pages; PG_referenced exists only after
   mark_page_accessed(). NOTHING in Rux legitimately flags anon pages
   (lru_note_refs has no callers; no accessed-bit propagation) — every
   flag was false-hot.
2. vmscan's referenced rotation moved pages to the TAIL — the COLLECT
   end (lru_collect_cold walks tail-first). The code's own comment said
   "MRU end"; the reference protects referenced pages at the YOUNG end.
3. page_add_anon_lru linked new pages at the TAIL — the same collect
   end. The reference (lru_cache_add → list_add at head) keeps the tail
   the oldest end. Under the fault storm the scan window was
   permanently full of freshly-faulted, falsely-flagged pages while the
   old pages never reached it. The file side already had the correct
   shape (page_add_file_lru_young → link_head_locked, policy doc).

FIX (5c95f612 + the rotation hunk in fe6279c9): fresh pages flag-clear
(zone.rs + pcp.rs); compact.rs PRESERVES src's Referenced on migration
(folio_migrate_flags parity) instead of re-planting; anon insertion at
the HEAD; rotation to the HEAD.

VERIFIED (sp7-fix1-memory.log, same cmds/rig): rotref 1,642 -> 0;
freed/examined 1,240/3,589 (35%) -> 2,163/2,629 (82%); +75% swap
writes; third rep in the park-census boot 2,990/3,346 (89%). Zero
panics/wedges-new; malloc/mmap/bigheap op rates +2-5% (5.63M/1.27M/
2,222 by t=100); kills remain TRUE-exhaustion class (dumps free_cnt=0
plus post-kill refilling moments 50-56k; demand 2.9 GB anon vs 960 MB
zone). make test 1297/1297 on the fixed kernel.

RESIDUALS for the mm owner (second-order, 7-13% combined): unmap0
(107-181/boot: try_to_unmap finds no PTE for aged pages — rmap is 4
slots then single-value degraded, page_get_mappings is a placeholder);
unmapped (95-111: mapcount<=bias while still LRU-linked — exit-path
races); notanon (52-190: non-anon members on the anon list).

The alloc-fail-with-free>0 skew (dump #37 class): evidence-ordering,
not a free-list bug — the dump's zone reads race the refill between
the alloc-fail decision and the print (free_cnt moves 8108 -> 7248
between two reads INSIDE one dump; walkable tracks free at each read).
The waiter loop retries zone.alloc_pages every iteration with
RECLAIM_PROGRESS resets (should_reclaim_retry parity) — no missed
retry found.

## Mission 2 — PARKED-LEAK: second sample = FAMILY (benign so far)

Memory-class re-census (dfx=leakhunt,allocwatch,fuxstuck + syscost,
sp7-parkcensus-memory.log): ZERO lines, 11,000 walks / 102,820 obs,
parkednz=0 the whole boot. Forker-class (same dfx set,
sp7-forker.log): SECOND sample — PARKED-WILDWRITE + PARKED-LEAK pid
29860 pc=1 state=0x20 tcpu=0; LEAK-ORIGIN site sched.rs:2641 (the GRQ
lock_irqsave in the context switch), hsite trap.rs:1113 (IRQ vector
dispatch), hage/cnt/age all ~0 (count went 0 -> 1 BETWEEN walks on a
parked task); ACQUIRE-PAIR-SPLIT releaser != recorded acquire
(cross-task booking). Zero heals/underflows/strands; fork+exec both
rc=0; RS-ALL-DONE. VERDICT: family = 2 boots x 1 self-resolved line,
fork/exec churn class, no downstream damage. The pair-split at the GRQ
lock + IRQ dispatch is the lying-slot/cross-task family the strand
hunt owns — this is its sharpest datum (a booked acquire released by a
DIFFERENT task pointer). Not chased here.

## Mission 3 — TIMER-SOFTIRQ-STALL at halt-time: cosmetic, mechanism named

Not reproduced this round (make-test serial ends clean at "All tests
passed! Halting..."; r6 saw it in 1 of 2 runs). Code reading bounds it:
the unit-test halt path (main.rs) drains pending softirqs DIRECTLY then
loops wfi forever — the WW watchdog inside that drain invocation
blind-scans every CPU and can print a stall for a CPU whose tick
legitimately stopped during the halt quiescence (r6 line: age 1053 ms
~= tick_age 1054 ms, pid=0 idle, kernel-text rip). NOT a lost wakeup:
post-verdict nothing awaits a wakeup and the halt never schedules again
by design. No timers-work connection; stays a watch item only if it
ever fires PRE-verdict (never observed).

## Mission 4 — Tier-2 pbench anchor re-banked to the KVM debug class

The pb1 TCG debug column was ~30% stale on p2 (r4sp5 11.245, r6
10.687/10.091 vs the 15.409 cell) — and the r7 re-measure showed the
TCG class cannot anchor a gate row at all: the SAME kernel read p2
28.151 µs at host load 5-6 (r6's quieter windows: 10.1-10.7). KVM is
10x tighter: three full boards on the round-7 kernel (debug bzImage,
q35 smp2 2G, quiet, foreign_qemus<=2, DONE rc=0 each) — p2
1.237/1.262/1.242 µs (±1%). Re-banked the WHOLE gated column (median
of 3): p1_null 0.300, p1_clock 0.371, p2_ctxsw 1.242, p3b_cow 8.075,
p6_write_fsync 8442.6, p6_mkdrmdir 216.7, p6_unlink 54.6, p6_append
5.19 µs; tier2 now boots pbench --accel kvm (PB_ACCEL override
retains TCG). Provenance + retired-TCG note in
docs/development/perf-gate-baseline.md. Cross-class comparison
(TCG run vs KVM cells) stays meaningless.

## Mission 5 — family sweep on the fixed kernel

memory x2 (sp7-fix1, sp7-parkcensus): RS-ALL-DONE both; worker
SIGKILL-wedges only at TRUE exhaustion (same class as r6); census
bands in shape (IF=0 ~1 s/60 s per CPU under exhaustion; heal counts
~2-3 samples; allocwatch sections max 48 CYCLES, over10ms=0);
VSCAN rotref=0 in every boot. forker: fork+exec rc=0, zero
heals/underflows/strands. fsmeta (sp7-fsmeta-v3.log): renameco 300 iters plain=0 chmod=0
rename=0 (SP-2 area holds), fsmeta 4 workers full budget rc=0,
RS-ALL-DONE, zero panics. Gates: x3 builds rc=0 (x86_64 post-fix, riscv64 clean
config, aarch64 with dtb_blob.S restored), make test GREEN attempt 1
(1297/1297).

## Round 8 — Mission 1: the ACQUIRE-PAIR-SPLIT / PARKED-WILDWRITE family
## ROOT-CAUSED (both faces benign; census gaps closed; no lying-slot
## wild write exists on this tree)

The r7 datum (PARKED-WILDWRITE + PARKED-LEAK pid 29860 pc=1 site=
sched.rs:2641 hsite=trap.rs:1113, releaser != recorded) decomposes
into TWO distinct mechanisms, both now proven:

1. PARKED-WILDWRITE/PARKED-LEAK = the parkwalk census sampling a
   voluntary-parking task inside its OWN __schedule, in the window
   [per-CPU slot publish -> SWITCHING_OUT store]. Facts (code):
   the census classifies "parked" as "named by no sched slot and no
   SWITCHING_OUT marker"; __schedule publishes cpu_state.current=next
   (sched.rs) BEFORE storing the R63 SWITCHING_OUT marker (which sat
   behind the schedstat block); the parking task legitimately holds
   its OWN GRQ guard through that whole window (acquire at
   sched.rs:2641 = the recorded site — the task's own scheduler lock;
   hsite=1113 is just its most recent tick stamp). The count returns
   to 0 at unlock_irqretain microseconds later (before the asm
   switch) — hence "self-resolved, all ages 0, pc=0 at report". The
   wildwrite predicate (parked two consecutive walks + 0->1 between)
   is satisfied when walk N sampled an earlier full park and walk N+1
   landed in the gap of a later park (wake->run->re-park between
   walks is invisible to the two-walk predicate). The report-time
   tp=-1 (no arch tp names the victim) does NOT falsify this: the
   report runs after the walk's bucket-lock latency, long past the
   us-scale window.
2. ACQUIRE-PAIR-SPLIT (40/boot cap under fork churn, ALL
   osite=sched.rs:2641, including symmetric releaser/recorded pairs
   swapped across cpu0/cpu1) = leakorigin slot-table cross-talk:
   two LIVE tasks hashing to the same 2048-entry Fibonacci slot
   (birthday collisions are certain at ~2700 concurrent tasks), both
   paying __schedule's GRQ outermost pair — the dominant outermost
   acquire under churn (every context switch) with a ~5-6 us armed
   window (the GRQ hold class) — their pairs interleaving across the
   2 CPUs so each release observes the other's still-armed o-record.
   A GENUINE cross-task release of this pair is impossible on this
   tree: preempt booking resolves through the arch tp register, which
   cannot change between the 2641 acquire and the unlock release (the
   window is IF=0; the asm switch runs strictly after the release).

FIXES (this round, minimal):
- sched.rs: SWITCHING_OUT is now stored BEFORE the slot publish
  (strictly widens the R63 protection; still inside the same GRQ
  hold; closer to the reference's prev->on_cpu which covers the whole
  switch). The [slot flip -> marker] gap no longer exists.
- parkwalk census: Visit now records tp_named (any arch tp register
  naming the task AT SAMPLE TIME); PARKED-WILDWRITE/PARKED-LEAK print
  tpexec=; the 4 s PARKED-HEAL never fires on a tp-named (executing)
  task.
- leakorigin: ACQUIRE-PAIR-SPLIT prints rpid/rpc/rown (recorded
  task's pid, CURRENT base count, ownership: slot/marker) so every
  line self-adjudicates: cross-talk => rpc=0 (pair completed) or
  freed-descriptor rpid (0xdeadbeef = TASK_POISON, slot succession);
  a real lying-slot orphan => rown=0 with rpc=1 persisting.

VERIFICATION (forker-lh boot, fixed kernel, dfx=leakhunt,allocwatch):
fork 492,292 ops rc=0 + exec 178,724 ops rc=0, RS-ALL-DONE;
PARKED-WILDWRITE 0 (r7: 1), PARKED-LEAK 0 (r7: 1), heals/strands/
underflows 0. ACQUIRE-PAIR-SPLIT 20 lines: every live recorded task
rpc=0 (its own pair completed); one line rpid=0xdeadbeef (recorded
task freed before print — slot succession). ZERO lines with a live
rpc=1 orphan. VERDICT: the "lying-slot wild-write family" as a
cross-task preempt-count write does not exist on this tree; the r7
datum family is closed.

## Round 8 — Mission 2: rmap residual buckets ADJUDICATED (one real
## divergence found + fixed; the rest are correct teardown behavior)

Battery: memory2 class (malloc 4x256M + mmap 4 + bigheap 2x384M,
100 s budgets each) on the r8 kernel, vs the r7 sp7-parkcensus
baseline (same class shape). Final census: calls=12 want=2868
got=2868 freed=2546 (88.8% — r7 band 82-89%), rotref=0, werr=0,
zero panics/deadlocks; class verdicts identical to r7 (malloc rc=1 =
true-exhaustion worker kill; bigheap errno=12 mode switch; mmap rc=0).

1. unmap0 (r7: 119-161/boot) — WAS A REAL MISSED-RMAP-WALK DIVERGENCE,
   now FIXED. Producer: private file VMAs host anonymous COW pages
   (the fault paths' FileBacked write-fault arm flags the copy
   Anonymous+SwapBacked and links it on the anon LRU — do_cow_fault
   parity; the reference swaps these via page_add_new_anon_rmap), but
   try_to_unmap_inner's VMA filter only admitted VmaType::Anonymous —
   every private-COW page (an exec'd binary's written .data/.bss)
   passed all vmscan filters, got a slot, found no admissible VMA at
   its recorded address, unmap0'd, freed the slot, and was re-skipped
   on every later scan: permanently unswappable dead weight at the
   LRU's cold end. Fix: admit FileBacked VMAs in the walk's filter
   (the PTE ppn==target match stays the authority; shared file VMAs
   never host Anonymous-flagged frames; shmem pages never reach this
   walk from vmscan). After: unmap0 5/boot (−96%), and the new
   u0(live/dead/ovf) split reads 0/5/0 — the entire residue is the
   dead face (mapcount already drained when the arm runs = a
   teardown raced the filter→walk window; benign transient). ZERO
   live faces (missed walks) and ZERO RmapOverflow faces remain in
   this workload.
2. unmapped (r7: 104-111; r8: 226) — CORRECT BEHAVIOR (sampling of
   in-flight teardown), not a leak. Two producers: (a) exit-path
   last-unmap (page_remove_rmap decs to −1, clears flags, then
   removes from LRU — the unlocked filter can read in that window);
   (b) the swap-out success path keeps the page LRU-linked THROUGH
   the swap device write (dec_mapcount at PTE-replace ... write (ms
   class) ... clear flags ... page_remove_lru — the no-swap-cache
   shape, review 4.12). (b) grew because the fix lets ~2.5k more
   pages complete swap-out per boot; the share (226/2868 ≈ 8%) is
   the in-flight overlap, and every such page completes its teardown
   and leaves the list. The reference hides this class behind
   PG_swapcache (page leaves the LRU at isolation, before the
   write) — a swap-cache refactor is architectural, out of scope,
   and NOT needed for correctness.
3. notanon (r8: 91 ≈ r7's 83) — same teardown-sampling class (the
   clear_flag(Anonymous) → page_remove_lru window, plus the exit
   path). Benign; self-correcting.

VERDICT per the round-8 question: unmap0 was a missed rmap walk (now
fixed); unmapped and notanon are correct aging/teardown behavior.
The degraded 4-slot rmap (RmapOverflow) and page_get_mappings
placeholder never fired in this workload (u0ovf=0) — the
architectural rmap limit stays a known constraint, not an observed
face here.

## Round 8 — Mission 3: KVM p2 anchor VERIFIED at the strong class

Full board on the r8 kernel (debug bzImage, --accel kvm, q35 smp2 2G,
P2's CPU-affinity pin class — aff=1 held), booted UNDER host load
4.3-4.7 with 2 foreign QEMUs (OH 27 + LTP r24 lanes), tag r8sp8-load,
PBENCH DONE rc=0:
  p1_null 0.300 (0% vs anchor) | p1_clock 0.371 (0%) |
  p2_ctxsw 1.267 (+2.0%) | p3b_cow 8.366 (+3.6%) |
  p6_write_fsync 9661.2 (+14.4%) | p6_mkdrmdir 217.7 (+0.5%) |
  p6_unlink 54.6 (0%) | p6_append 5.069 (-2.3%)
p2 at +2.0% under load+pin where TCG read 28.151 (the load-hostage
class) closes the r7 re-bank's open question: the KVM anchor HOLDS at
the strong class. The strong-class p2 column re-banks to
1.267 loaded / 1.242 quiet (perf-gate-baseline.md updated). The
fsync row's +14.4% is the I/O row's load face (quiet class already
spread +/-11%; max tail 51 ms tracks host I/O scheduling) — no
mechanism in this round's changes touches that path (rmap filter is
reclaim-only, the sched change is one store inside the GRQ hold);
this board doubles as the Tier-2 row for the round-8 sched change —
all rows inside +/-20%.

## Round 8 — Mission 4: ltplong long-horizon PASS (the round-5 leftover
## re-run on the r8 kernel)

ltplong5 (single pass, 8 entries, budgets 600-900): mmapstress01 /
mmap-corruption01 / mmapstress09 -p 40 -t 0.4 / mmapstress07 /
mmapstress10 -p 40 -t 0.4 / fork_procs -n 500 / growfiles ALL rc=0;
msgstress01 rc=2 (msgsnd EINVAL — the r2/r3-adjudicated intermittent
family, known-flake list; not re-adjudicated here). ltplong6 (mm trio +
fork_procs x3 consecutive loops in ONE boot): 12/12 rc=0, RS-ALL-DONE,
zero panics/wedges, leakhunt+allocwatch armed throughout (0 wildwrite,
0 parked-leak, alloc sections max 0x43e cycles, over10ms=0). The
rc=254 silent-child-death family: no recurrence (20 execs of
glibc-static LTP binaries all reported results).

## Round 9 — Mission 1+2: the r8 rmap fix's stress echo (3-kernel table, same recipe, same host load)

Recipe: memory2 class (malloc 4x256M / mmap 4 / bigheap 2x384M, 100s
budgets), x86_64+KVM q35 smp2 2G, dfx=allocwatch,taskdump,watchdog +
syscost=1, OH A/B QEMU co-running (load parity across all three boots).

| kernel (tree)              | unmap0 final | u0 split (live/dead/ovf) | kills | class verdicts |
|----------------------------|--------------|--------------------------|-------|----------------|
| abe5df57 (r7 close, PRE-fix)| 107-143      | (no split — pre-SP-8)    | 2     | malloc rc=1, bigheap rc=1 |
| d1184d2d (r8 kernel)        | 68-72        | 2/70/0                   | 0     | malloc rc=1 (wedge SIGKILL), bigheap rc=1 |
| 54be3925 (r9 = r8 + main 8d81c5da RCU-fd + ams46 tap) | 51 / 85 | 1/50/0 and 4/81/0 | 2+2 | malloc rc=1 x2, bigheap 137/1 |

VERDICTS:
1. The FileBacked-COW admission's benefit HOLDS under true-exhaustion
   load: unmap0 halves (107-143 -> 51-85) at matched kill volume.
2. NO new unmap/swap error class from the fix or from main's merge:
   werr=0, partial=0, putnz=0, rotref=0 on every boot; rc buckets zero.
3. The residue is dead-face dominated (u0dead 50-81) + a small live
   face (u0live 1-4/boot). The u0live events cluster at the OOM-kill
   teardown waves (boot 2: flat 0 through five censuses, then 4 in the
   final kill window) — transient stale-slot shape, no monotone
   per-round growth => NOT permanently-unswappable dead weight. The
   r8 close's "residue is 100% the dead face" was measured on a boot
   with no kill cascade; under kills a 1-4-event live face appears.
4. malloc rc=1 ("worker needed SIGKILL") reproduces on ALL THREE
   kernels under today's load — including the pre-fix r7-close tree.
   It is the load shape (true-exhaustion OOM kills + slow teardown
   under swapout), NOT a regression. OOM discipline held every time:
   kills only at free_cnt=0 dumps (the SP-5/SP-7 design).
5. r8's "unmap0 <=5/boot" class is confirmed as a NO-KILL-BOOT figure;
   the comparable kill-boot class is 51-85 (pre-fix: 107-143+).

## Round 9 — kill10 hunt (coordinator handoff, LTP r24): face-1 census + instruments

- FACE 1 (park/lost-wake subset) CAUGHT LIVE, r9f boot (guest-side
  k10loop loop, no serial input): loop 1 HUNG forever, loops 2-8 ALL
  rc=0 (TPASS) — 1-in-8 intermittence on the current tree (kill10 fix
  a5dfbe33 included). During the hang: ticks alive (TICKARM-RING
  heartbeat advancing, seq 0xd91 -> 0x9141 over the window), both CPUs
  alive, zero WAKE-REFUSED/SLOT-LIE prints — the silent-park class.
- FACE 2 instrument LANDED: TICK_ARM_RING (commit ec141f0c) — last 16
  timer events per CPU in plain memory, phys published at boot
  (TICKARM-RING line), QEMU-monitor xp dump verified end-to-end on a
  live machine (decodes ARM->SERVICE pairs; ra symbolizes to
  timer_arm_oneshot / trap_handler). The freeze face now leaves a
  readable corpse; NMI non-resumption (LTP k10-rux-nmi1) repeated.
- Discriminating instrument LANDED: HRES-OVERDUE (commit 469a7be8) —
  wheel-side overdue scan at the 1 Hz softirq heartbeat; r9g+ boots
  adjudicate the two branches (stuck-in-wheel vs delivery-loss).
- Rig lesson (host-side): the emulated 16550 RX FIFO drops bytes on
  long serial-fed command lines (syntax-error face) AND serial RX can
  die mid-delivery while ticks continue (observed r9d/r9e: input stops
  ~15s in, ring heartbeat alive) — the hunt now runs fully guest-side
  (k10loop init binary), no serial input dependency. The RX-loss shape
  itself is logged for the interrupts domain (same delivery-loss
  family as the freeze's irr-stuck capture).
- fdreapd (main 8d81c5da) trips the hung-task detector once per quiet
  boot (ppid=0 kernel thread, parked in its reaper wait) — cosmetic
  watchdog noise, worth a look from the fs owner.

## Round 9 — kill10 face-1 DECODE (the park subset root shape)

r9f/r9g/r9h/r9i/r9j/r9k boots (k10loop guest-side, d8 protocol trace
console-direct on r9k): the hang is FIRST-RUN-AFTER-BOOT DETERMINISTIC
(5/5 boots: loop 1 hangs; loops 2-8 ALL rc=0 — 22 passing loops). This
also explains the LTP r24 anomaly (census FAIL vs isolation PASS): the
census always runs kill10 first-in-boot; isolation reruns are 2nd+ runs.

Protocol state at the hang (r9k ordered trace):
- Manager 310: FULL round — children "Child start signaling", SIGUSR2
  storm, reply_to_child x10, "All children reported", USR2-done to
  master (the master's "pausing for done (1/2)" = 310's report).
- Manager 311: confirmed ready, entered its main pause loop, counter
  stayed 0/10 — its TEN children NEVER received the round's group
  USR1 (zero start-signaling prints from 311's children). The master
  printed BOTH group kills ("test_loop, SIGUSR1 -> -310/-311").
- Master: waits 1/2 forever, alarm(1) wakes flowing (ticks alive).

Exonerated this round: wheel-side timer loss (HRES-OVERDUE silent all
boots; wheel carries live alarm/wake entries that keep cycling); the
group-kill primitive in steady state (k10pgid: 150 rounds x 10
children, 0 dropped); ticks/LAPIC (TICKARM-RING heartbeat advances).

Remaining suspect (successor entry): SIGNAL DELIVERY loss into ONE
process group (311's family) around fork boundaries — two candidate
losses observed in that family (the fork_pgrps ALRM to 311 landed
concurrently with 311's first pause print and 311 only recovered via
its own alarm(1) failsafe; the round's group-USR1 never landed at
all). Discriminating instrument: a signal send/pend/deliver tracepoint
(unified trace framework, pid-filterable) + a k10pgid variant that
fires the group kill DURING child forking (the steady-state variant is
clean). KILLTRACE currently covers KILL/TERM/SEGV/HUP only.

Rig notes: 16550 RX FIFO drops long serial-fed lines (dribble fixed);
serial RX itself can die mid-delivery with ticks alive (logged for the
interrupts domain); kill10 d8 output to a shared file is stale-buffer
garbage — console-direct (line-buffered) is the readable channel.

## Round 10 (2026-11-04) — ftest03 decoded (budget-blown convoy, NOT a
## data race; mutex optimistic-spin producer); rlimits irqsave fix (tick
## self-deadlock = a real both-CPU freeze class); OH wedge-rate A/B table

1. ftest03 (LTP r24 handoff, adjudicated REAL but re-classified):
   - A/B reproduced 4/4 deterministic (tip FAIL 2/2, 1bde7490-mutex
     PASS 2/2, ltpscan 30s-budget isolation, private stress rootfs).
   - The children do NOT fail data checks: ZERO "bad verify" lines on
     any failing boot. The failure chain is 30s-budget TERM ->
     ftest03 parent term() fan-out -> still-running children take the
     TBROK-unlink exit (nonzero) -> "Test{N} failed, expected 0 exit".
     With a 120s budget the REAL binary PASSES on the tip kernel.
   - Producer RE-ATTRIBUTED: 1bde7490's mutex.rs ALREADY carries the
     da166982 recursion-arm gates; the swap reverted the optimistic-
     spin rework (4x64 -> 100x64 + need_resched, merged e264a64b from
     code-review-perf). That rework dropped the reference's
     holder-on-CPU guard (mutex_spin_on_owner / owner->on_cpu): under
     the fs biglock convoy (5-6 tasks, SMP2, ms sections through
     jbd2) spinners burned full budgets against descheduled holders,
     starving the holder. ft03probe (clone with HEART timestamps):
     14s (old spin) -> 95s (rework) -> 13s (rework + holder-on-CPU
     guard, committed 1784be54). Real ftest03 under budget: PASS 2/2.
   - WAKE-HEAL-SKIP floods and the virtio "out-of-order completion"
     witness fire on PASSING boots too — concomitant, not causal.
   - pbench gate class (KVM smp2 2G, vs SP-7 anchor): p2_ctxsw 1.292
     (anchor 1.242, +4.0%), p1_null 0.300 (0%), p3b 8.055 (-0.2%),
     p6_append 5.44 (+4.8%), p6_unlink 48.9 (-10.5%); A-vs-C same-rig
     p2 delta +1.2% — ctx-switch win preserved, all rows in band.
2. rlimits tick self-deadlock (committed 3b352cf6): scheduler_tick
   reads current's RLIMIT_CPU via lock_irqsave; sys_poll's NOFILE
   bound and __send_signal's SIGPENDING check took the same per-task
   spinlock PLAIN — a self-targeted holder interrupted by its own
   tick spins forever at IF=0. Evidence: the OH A/B joint-run
   DEADLOCK-spin class — pw-5 (byte-identical rebuild): cpu1 >1400s
   at Task::rlimit+0x89 on two per-task locks (0x...ab2f4b8 /
   0x...a3db4b8), cpu0 joining at the end; cg-1's 761-print storm is
   the same lock-offset class. This is a REAL both-CPU interrupt
   freeze family (NMI-only survivors) — kill10 face-2's shape, though
   the k10 workload itself never printed DEADLOCK (its signals are
   non-RT, no poll), so face-2 proper remains open with the ring.
3. OH wedge-rate A/B (5v5 interleaved, same recipe/host):
   - cg26b: 4/5 full ladder (bundles.ready 91-94s, create 60-63s,
     startUser 91-94s); 1/5 wedge (cg-1 DEADLOCK-spin storm from 41s).
     All 5 carry >=1 full-roll TXN strand (maxexec 1462-1486s) — the
     pre-existing wedge family persists but does not block the ladder.
   - pollwake (491ebc09): 0/5 ladder; 3/5 frozen pre-bootanimation
     (pw-2/4: single FFRT park at ~5-6s anywoken=0 run=0; pw-5: that
     park PLUS the terminal both-CPU rlimit spin); pw-1/3 reached
     ba_ready ~11s but no bundles.
   - VERDICT: pollwake does NOT reduce the wedge rate — it is
     categorically worse under this recipe (0/5 vs 4/5 ladder; 3/5 vs
     0/5 pre-ba freeze). The FFRT/samgr park family is unchanged
     between kernels; do not merge pollwake on wedge-reduction
     grounds. (Note: the pw-5 freeze mechanism (rlimit) is fixed on
     our branch; the OH-side kernel predates the fix.)
4. kill10 r10a re-hunt (post both fixes): first-run hang (face 1,
   r9-decoded signal-loss) still present; loops 2-7 rc=0; ZERO freeze
   dumps, ZERO DEADLOCK prints — no new class from today's changes.
5. r9 freeze-capture re-adjudication: ALL r9 "FREEZE-WATCH" stalls
   (r9a-e) were face-1 PARKS, not interrupt freezes — the TICKARM-RING
   seq advanced ~200 events/s across every stall window (ticks alive
   on both CPUs) and the dumped CPU sat HALT=1 (idle) with IF=1 at a
   stable idle-halt RIP. True face-2 (both-CPU interrupt freeze) did
   not occur in any of the 11 r9 hunt boots; the ring + HRES-OVERDUE
   instruments remain armed for the next recurrence.

## Round 12 (2026-11-04) — pick-gen ledger complete; R49 pick hypothesis
## REFUTED for lat_udp (pickgen=retired 6/6, LP-OUTCOME woken=1); strand
## re-pointed: silent wake (recv + alarm) + malloc-corruption marker

Kernel: c6d74e01 (ledger wiring) + f5a78f93 (origin/main r26 merge) +
6ebf6859 (LP-OUTCOME). Gates: x3 builds rc=0, make test 1306/1306 2x
(r12-maketest{,2}.log — the 3df3fe17 "wake_up running task (false)"
row PASS), renameco 300-iter clean (plain=0 chmod=0 rename=0 renchmod=0
link=0 symlink=0, zero panics; post-run parked-rs HUNG-CTX lines are
the known cosmetic). Pushed 74f2a977..6ebf6859.

1. PICK-GEN LEDGER (r11 completion, mission 1): publish at the tag-1
   slot write; retire at every materialization arm (post-context_switch,
   schedule_tail newborn, repick fast path) + supersede (publish+retire
   in one breath) at all six heal-side rewrites (R56c/d, steal, abort-
   republish, idle-heal, R65 guard). Conviction hook at the LP
   observation: pickgen=LOST needs tag-1 + TASK==slot + unretired >=2
   jiffies. PICKGEN_LOST census static per CPU.
2. LIVE VERDICT on the 4/4-class lmbench lat_udp strand (release-opt
   KVM smp2, run/lmb-rig, r12b/r12c/r12d boots; strand reproduced 2/3):
   - EVERY lying-slot observation shows pickgen=retired (6/6: r12b
     pid310 cpu0; r12c pid308 cpu1+cpu0 x2, pid310 cpu1; r12d pid310
     cpu1). ZERO convictions. The R49 "pick published but never
     switched in" cascade is REFUTED as this family's producer on the
     current tree; the r10 "SLOT LIE is the stuck invariant" reading is
     corrected — the slot names a task whose pick genuinely ran.
   - LP-WAKE-OUTCOME (6ebf6859): the wake that prints why=c/LP
     SUCCEEDS — enqueue landed, woken=1 (r12d live capture; that boot
     completed the benchmark cleanly despite the LP observation). The
     why=c/why=s refusals are mid-park protocol diagnostics
     (WAKE-SKIP-ORIGIN osite=sched.rs GRQ-acquire / wait.rs prepare,
     oage_jif=0), NOT dropped wakes. W2-ENQUEUE-HEAL-REFUSED never
     fired on any strand boot.
3. THE STRAND, re-anatomized (both stranding boots agree):
   - lat_udp ping child (benchmp child of the client, pid 310) parks in
     recv AFTER a datagram was delivered to its socket ("udp-in ->
     delivered fd=1" is the LAST packet event) and never wakes; its own
     alarm(15) SIGALRM watchdog (lat_udp.c doit) NEVER fires — two
     independent wake channels (socket data + timer) lost with ZERO
     scheduler-side evidence. Note the whole WHS/LP instrument family
     gates on pc!=0: the stranding victim's wake is SILENT by
     construction. Next-round leads: (a) delivery-side wake to an
     empty/stale wait-queue entry, (b) the unlocked not-sleeping drop
     at Task::wake_up_impl (task.rs ~3250 — prints nothing), (c)
     socket_recv_ready's view missing the queued datagram (the
     connected-UDP filter path — udp-in prints "connected_filters_on").
   - The ping child dies at TERM in musl malloc get_meta (#GP,
     rip=0x417e8b in lat_udp, non-canonical chunk-meta pointer, 2/2
     stranding boots) — a userspace heap-corruption marker (kernel
     over-write suspect near user buffers in the udp/alloc paths).
     Separate family; needs its own hunt.
   - The +0x100 SOFTIRQ pc orphan (SeqLock::write, seqlock.rs:137, the
     R21-N5 loopback-stats bracket) rides the victims (BIT-ORPHAN
     prints, sage 1) — known healable class, unchanged this round.
4. A/B re-run coordination (mission 3): OH 32's post-rlimits-fix rolls
   double as the cg26b leg (their TASK.md lb386+ series; verdict was
   0/5-vs-4/5 pre-fix). The pollwake leg is skipped this round: r10
   adjudicated the FFRT/samgr park family UNCHANGED between kernels
   (not rlimit-caused), so a single pollwake roll adds no decision
   power; re-visit if OH's post-fix rolls show the park family gone.
5. Sweep cadence: renameco 300-iteration clean on the final tree (gates
   paragraph above); ltplong long-horizon loops remain pending from r10
   (compaction priority — carry to r13).
