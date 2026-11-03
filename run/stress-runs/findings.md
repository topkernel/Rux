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
