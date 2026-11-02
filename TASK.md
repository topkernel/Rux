# stress-port lane: port industry stress tests, find stability bugs

## Charter (user directive 2026-11-02)
"Supplement stress test cases — stability is still a problem. If the
industry has stress tests for the reference kernel, port them directly
and run them." PRIMARY vehicle: x86_64 + KVM (platform priority).
Goal: panics, wedges, leaks, and scheduler starvation exposed by
sustained load — then kernel-side fixes.

## What to port, in order
1. stress-ng (the industry standard, ~300 stressors). Build a static
   x86_64 subset against the toolchain already used for probes (see
   test/ build patterns; musl static preferred). Inject via the LTP
   vehicle in /home/william/rux-images/testimages/ (boot rig there
   already runs x86_64+KVM). Start with the classes our bug ledger
   says are weak:
   - memory: malloc/mmap churn (allocator class-lock family),
     mmapfixed, bigheap (zone exhaustion vs the fake-OOM history)
   - fork/exec churn (p4 regression history; exit-path RCU grace)
   - filesystem metadata churn (mkdir/rmdir/unlink storms — the OH
     chmod-storm shape; bio write-plan under load)
   - IPC: pipe/socket storms, SysV sem/msg/shm
   - sched: many-thread yield/wakeup storms (phantom preempt family,
     WAKE-STRANDED census)
   - timer/clock: settime/interval storms (tick discipline)
2. LTP robustness subset already in the tree (fork12, mmapstress,
   mmap-corruption, sem/msg stress) — run long-loop, not just once.
3. (later) syscall-fuzzer class (trinity-like) once 1-2 are clean.

## Method
- One stressor class per boot; serial console is the log.
- PASS = zero panics/wedges over the budget; FAIL = named bug entry in
  run/findings.md + minimal reproducer.
- Every kernel fix: x3 builds rc=0 + make test green + stressor rerun
  clean before push (origin/stress-port).
- Use the in-tree DFX under load: dfx=allocwatch (class-lock holder
  watch), syscost census bands, PREEMPT-HEAL counters — stability runs
  double as Tier-1-under-load evidence for the perf gate.
- Host discipline: at most 2 own QEMUs; -snapshot + file.locking=off;
  kill only your PIDs; pkill forbidden (pgrep -f self-matches: verify
  by exact cmdline).

## Session log (append per session)

### 2026-11-02, round 1 — rs suite ported, 2 kernel bug families fixed, 2 stressors re-run clean

**Ported** (stress-ng stressor logic as standalone static musl probes,
per charter method; sources archived in userspace/tests/probes.tar.gz
per the 394c16ce policy, host driver test/stressng/run-class-x86.sh):
14 classes — malloc/mmap/bigheap (memory churn), fork/exec, fsmeta
(metadata storm with read-back verify), pipe/sock/sem/msg/shm (IPC),
futex/sched (wakeup storms), timer (create/settime/delete + CLOCK_
MONOTONIC discipline + nanosleep latency), plus --spawn passthrough
for minimal reproducers. Vehicle: the x86_64+KVM LTP rootfs (copy of
the permanent-store image in run/, rs/rsrun/renameco injected via
debugfs); PID-1 rsrun runs cmds.txt lines with per-class budgets.

**Round-1 campaign** (baseline 819a1be2, one family per boot,
dfx=allocwatch,taskdump,watchdog + syscost=1, logs in
run/stress-runs/): ipc / sched / timer / memory PASS (0 panics, 0
wedges, 0 probe errors); fsmeta FAIL (SP-2); forker survived but lit
up the SP-1 storm. Verdicts + census numbers: run/stress-runs/
findings.md.

**SP-1 (fixed, verified)**: PREEMPT-HEAL false-fires stripped live
guard bits of fresh fork/exec tasks (15,577 heals + 169 underflows per
120 s of fork churn). Two producers: (1) stale_200's j==0 arm treated
never-stamped fields as instantly stale — fixed with arm-on-first-
observation (sched.rs); (2) new_task_at did not zero the four
ti_pc_*_zero_jif stamps on recycled Task pages — same class as R68a
(task.rs). Verification boot (fork 8 + exec 4, full 100 s budgets):
**0 heals, 0 underflows, 0 panics** (was 15,577/169/0).

**SP-2 (fixed, verified)**: ext4 setattr corrupted freshly-written
files — the success tail ran page-cache invalidate_inode for EVERY
attr flavor (chmod dropped dirty unflushed pages) and metadata-only
setattrs wrote back the disk-read inode's stale i_size (0 for pending
writeback). renameco minimal reproducer isolated chmod (3/300 fails,
reads returned EOF; rename alone clean). Fix: invalidate gated to
ATTR_SIZE, metadata setattrs carry the live VFS size (ext4/mod.rs).
Verification boot: renameco 0/300 on every sequence; fsmeta 4-worker
class full 120 s rc=0 (was: all 4 workers corrupt on cycle 1).

**Driver bug found+fixed in-flight**: rsrun killed classes at an
absolute 60 s (RES rc=137 truncation in the first campaign's memory/
forker boots); now parses --timeout per class.

**Tier-1-under-load evidence** (syscost, memory boot): IF=0 cpu0
17.6 s/60 s (rest 17.2 s, hard 408 ms, uart 3 ms), top rest-site
sched.rs:2568 (37.4M ops avg 834 ns); no ALLOCTHROW / DEADLOCK /
KERNPANIC classes in any boot. Follow-up hunts queued: IRQ-OFF-WINDOW
idle-wake prints 65-142 ms under load (pre-existing, guarded domain
interrupts); WAKE-STRANDED 118 s face under memory churn (SP-1
family; re-check on fixed kernel in round 2).

**Round 2 queue**: dirstorm-style shared-tree churn + fsync pressure;
pipe/socketpair under CPU contention (2 vCPU); LTP robustness subset
long-loop (fork12, mmapstress, mmap-corruption); brk-churn probe via
glibc-static (musl pins brk); make test + full LTP spot-scan on the
fixed kernel to guard the setattr/preempt changes.


### 2026-11-02, round 2 — SP-3 fixed, IRQ-OFF-WINDOW adjudicated artifact, residual family censused, brk covered

**Sync**: merged origin/main aa43f52e (round-1 SP-1/SP-2 content landed
there); branch descends from main. Round-2 commits: 126e99b9 (kernel),
ec9a0079 (probes).

**Queue item 1 (SP-2 deep soak) + SP-3**: new rs classes dirstorm
(shared-tree legal races, strict fd-pinned verify) and dirsync (16 KB
write + fdatasync/fsync + fchmod + rename + dir-fsync storm). fsdeep
boot: dirstorm 6x150s PASS (2.02M ops, 0 corrupt); fsmeta 8x180s PASS
(1.45M ops — SP-2 area holds at 2x round-1 intensity); dirsync caught
**SP-3: fsync(dirfd) on ext4 returned EINVAL** (sys_fsync's blanket
!is_regular_file check; the reference kernel's ext4_dir_operations
carries .fsync, and our own namei defers entry-block durability to
fsync(dirfd)). Fixed (is_directory + ext4 ops -> ext4_sync_file);
rerun clean: dirsync 615,856 ops rc=0, dirstorm 2.55M ops rc=0 with
strict dir-fsync checks. make test green after fix.

**Queue item 2 (IRQ-OFF-WINDOW idle-wake)**: adjudicated MEASUREMENT
ARTIFACT, not an interrupts-domain violation. Evidence: periodic LAPIC
tick + idle halts with IF=1 (gap = halt-exit delivery latency);
user-frame gap lines prove steal-magnitude delays with IF=1; syscost's
cli/sti zone accounting measured organic REST maxima <=2 ms in the same
boots (the only >7 ms segments were ONE host-stall window quantized on
both CPUs: 2619 ms REST @ file.rs:581 cpu1 + 2620 ms UART cpu0, fired
once, never again). Probe re-tagged: IRQ-DELAY-IDLE / IRQ-DELAY-STEAL
for artifact frames (kernel change 126e99b9), verified live. No
interrupts fix needed; guarded-domain band holds organically.

**Queue item 3 (WAKE-STRANDED re-census, SP-1-fixed kernel)**: the
residual family STANDS and is deterministic. memory2 boot: malloc rc=1
(worker 308 wedge, needed SIGKILL — campaign's first wedge), 1x
WAKE-STRANDED 100,469 ms/22,262 visits, 21 heals, 3 underflows. Chain:
rs malloc parent (pid 307) carried preempt_count 0x10004 for 200.7 s ->
wakeups refused -> heal -> induced underflow at a page-allocator lock
drop (.sdata near mm::pglist::NODE_DATA). leakhunt rerun reproduced
everything (wedge + 100.3 s strand + 200.8 s leak) and captured
false-fire ORIGIN sites mm/zone.rs:567 + mm/mm_struct.rs:958 with
oage_jif=1 (live acquires healed instantly) — steal-window false-fire
producer: the ti_pc_* stamps refresh only on the task's own CPU tick,
so a task parked mid-guard through a >2 s host steal window resumes
"stale-stamped + bits present" and is healed live. Numbers are for the
OH lane (same family); steal-aware exemption for the 200-tick bar is
the argued fix direction.

**Queue item 4 (LTP robustness long-loops)**: rsrun gained an
`exec <budget> <path> [args]` line form (fixed a stale-tail argv bug
found in-flight: NULL terminator placed before two token shifts — LTP
new-API binaries rejected the duplicated path arg; host-verified).
ltplong sweep (49 entries: mmapstress01-10 x3, mmap-corruption01 x3,
msgstress01 x3, msg_comm, sem_nstest, fork_procs, fork_exec_loop,
growfiles): mmapstress01 x3 + mmap-corruption01 x3 rc=0. Adjudicated
NOT bugs (host reference identical): mmapstress02/03/05/08 brk-ENOMEM
(1990s static binaries; their brk roundup fails on the modern
reference kernel too). Usage-shape (my cmds lacked args):
mmapstress06/09/10 — rerun with canonical args in ltplong3. REAL
suspect family: fork_procs/fork_exec_loop/growfiles/msg_comm/sem_nstest
+ 1 of 3 msgstress01 -> rc=254 with EMPTY output = rsrun's waitpid
ECHILD path: exec'd glibc-static children die silently before
userspace output, sometimes (msgstress01 mixed 2xTBROK+1x254), under
fork-heavy tests. fork_procs passes on the host reference (rc=0).
ltplong3 rerun (8 entries incl. 2 of the same binaries): 0 recurrences
— the family is INTERMITTENT (~1 boot in 2 today); mmapstress09/10
PASS with canonical args, fork_procs -n 100 x2 + sem_nstest + growfiles
all rc=0. Functional datum for the LTP lane: msg_comm TFAIL "messages
leak between namespaces".

**Queue item 5 (brk via glibc)**: brkchurn.c (glibc-static, host gcc
— host/guest both x86_64): PASS. 864,320 raw sbrk waves (212 GB) +
114.9M glibc malloc blocks (1.15 TB), enomem=0, verify 0 mismatch.
The sys_brk paths musl physically cannot reach: covered clean.

**Gates**: x86_64 build rc=0 (final 22:23), riscv64 build rc=0 (clean
config), make test GREEN (1096-suite verdict green), SP-3 stressor
rerun clean. Pushed origin/stress-port.

**Round-3 queue**: (1) the rc=254 silent-child-death family (minimal
repro: fork+exec loop of a glibc-static hello + waitpid audit; suspect
interaction with the strand/leak family — same boots); (2) heal
bar steal-awareness (200-tick bar vs >2 s host steal windows — design
per the census above, coordinate with OH lane); (3) ltplong long-horizon
loops (hours) once the 254 family is fixed; (4) mmapstress07 with
tmpfile args; (5) doubled direct-console interleave on heal prints
(two CPUs, no console lock) — cosmetic but garbles the heal lines.

### 2026-11-03, round 3 — heal-bar root-caused and rebuilt (observation
### counting), steal question decided, execaudit landed, 4x soaks PASS

**Sync**: merged origin/main 12acd3f6 (binder tcomplete deferral, ext4
published-size, x86_64 128KB stacks, LTP r22) — merge 3383cf79. Round-3
commits: ac0ff7f6 (kernel heal bar), f527ac16 (execaudit probe) + this
session's docs/driver commit.

**Q1 (the "200.7 s 0x10004 leak") — ROOT-CAUSED: not a missed put.**
Evidence chain: the first heal's leakhunt ORIGIN showed oage_jif=1 with
held_jiffies=0x4e6c — an outermost 0->1 acquire ONE jiffy before the
"200 s hold" heal (a solid base=4 hold cannot rewrite the o-record;
inner 4->5->4 pairs never touch it) — the base was churning live
guards. held_jiffies is LEAK_WIN window AGE, not hold time. Three bar
defects produced every observed face:
(a) absence-stamp AGE accrued while off-CPU (stamps refresh only on
    the task's own CPU tick) — the 100 s strand victim resumed with a
    200+-tick-old bar and its first tick healed the resumed syscall's
    LIVE guards -> spinlock.rs:0x1b8 underflow at the page-allocator
    zone lock;
(b) jiffies is timebase-DERIVED, so >2 s host steal windows inflate
    stamp age with zero ticks delivered;
(c) the pb3 release-side re-arm stored the SUPERSEDED ti_pc_zero_jif
    field (zero readers in the tree) — the bar never re-armed on legal
    1->0 drains, and lock-hot tasks formed 200-present-sample streaks
    (held_jiffies=0 storm, oage_jif=1 on every line).
Plus (d): heals never re-armed anything, so post-heal presence was
judged against the pre-heal stale bar — the storm amplifier.
**Fix (ac0ff7f6)**: the bar now counts CONSECUTIVE-PRESENT OBSERVATIONS
(ti_pc_{pirq,sirq,hirq}_present_run): bumped only by the task's own-CPU
tick when the field is present; reset on absent samples, legal drains
(release-side now writes the field the gate reads), every external
zeroing (all heal paths + underflow clamps), and task creation. No
tick, no increment — off-CPU time and host steal cannot inflate it.
Verification (leakhunt boots): memory-fix1 + memory-fix2 = 0 heals / 0
underflows / 0 strands / 0 PARKED-LEAK (round-2 same-boot: 21/3/1 + a
storm); forker-v4 (fork 8 + exec 4) = 0 heals / 0 underflows (SP-1
families stay dead). HEAL lines now print prun=/srun=/hrun=.

**Q2 (steal-aware exemption) — DECIDED, no exemption needed**: the new
bar's unit is the OBSERVATION (a delivered tick that sampled the field
present). Host steal delivers no guest ticks -> contributes zero by
construction. An exemption on the old bar would have masked defect
(c): the storm was churn-caused (oage_jif=1), not steal-caused.

**NEW parkwalk census (dfx=leakhunt)**: PARKWALK-CENSUS walks/obs/
parkednz printed every 1000 walks. fix1/fix2: walks=10,000,
obs=82,629, parkednz=0 — the walk is healthy and NO task parked with a
nonzero count in either boot. This answers round-2's "why did
PARKED-HEAL miss the 100 s strand": the victim parked CLEAN; the
0x10004 face arose at RESUME from the stale bar. The strand itself
(R85 orphaned-mark/lost-park) remains the OH lane's family — its
wake-side face no longer corrupts counts on this kernel.

**Q3 (rc=254 silent-child-death)**: execaudit.c probe landed
(userspace/tests/probes.tar.gz; driver injects it): fork storm +
immediate exec of a glibc-static target (brkchurn) + strict waitpid
audit (ECHILD/wrong-pid/signaled/127/nonzero), sibling fork-pressure,
rsrun's WNOHANG discipline via -w, EA lines to /dev/console. In-guest
boot: 560 audited execs (280 per discipline), events=0, echild=0 — the
family did NOT fire this boot (matches its ~1-in-2 intermittency;
next session: concurrent burst + LTP-shaped targets + multi-boot
sweep). Probe is the standing reproducer now.

**Q4 (soaks)**: dirstorm 12x150 (4x round-2 intensity) PASS — 2,139,796
ops, 0 RS-ERR; fsmeta 8x180 PASS — 1,375,896 ops; brkchurn pressure
rerun (two driver lessons: everything after --spawn is the probe argv;
the spawn class keeps rs's default 60 s budget unless --timeout
precedes --spawn) — ~90 s of glibc brk churn under leakhunt:
sbrk_ops 226k+ / 55+ GB, malloc_ops 29M+ / 290+ GB, enomem=0,
0 heal-family events. 0 heal-family events in every soak boot.

**NEW mm finding (SP-5 candidate, memory lane's guarded domain)**:
memory-fix1 boot — malloc class 0 ops for 100 s (round-2 boots: 4.45M);
zone genuinely exhausted (used=524213/524252), 2 workers self-SIGKILL
on pagefault OOM, parent sat in direct reclaim with
in_direct_reclaim=<pid> gate_exit_releases=0 across 4+ OOM-FORENSIC
dumps WHILE a later dump showed zone_free=33462 pages returned. Heal
side not involved (round-2 malloc windows had 0 heals too — first heal
fired only after the class). fix2 boot completed malloc normally:
INTERMITTENT, same cadence class as the fake-OOM history. Shape:
reclaim's exit gate does not release (or holder never re-checks) once
memory returns. Reproducer: the memory class boot itself.

**Gates**: x86_64 build rc=0 (post-fix, twice), riscv64 build rc=0
(clean config), make test GREEN (ALL TESTS PASSED, rc=0; suite QEMU
killed after verdict per lock discipline), stressor reruns clean.
Pushed origin/stress-port.

**Round-4 queue**: (1) SP-5 reclaim-gate livelock — coordinate with
the mm owner; the stress vehicle reproduces it; add a gate-age census
to name the holder's wait site; (2) execaudit multi-boot sweep
(concurrent burst, LTP binaries as targets) to catch the intermittent
ECHILD; (3) ltplong long-horizon loops (hours) on the fixed kernel;
(4) mmapstress07 with tmpfile args; (5) SPLIT o-record divergence
under fork churn (forker-v4: 24 budgeted lines, sched.rs:2580/6041) —
input for the stale-tp booking hunt (omis), zero wedges alongside.
