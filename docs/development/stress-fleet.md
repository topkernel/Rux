# Stress-Fleet Ledger

Standing stress lane (s2, wildwrite-hunt worktree). Mission: high-load +
randomized monkey testing across a concurrent QEMU fleet, catching every
anomaly class — wild writes (armed capture on the chains), panics/hangs,
leaks, fake-OOM recurrences, scheduler pathologies, perf-under-load drift
(vs `docs/development/perf-gate-baseline.md`), data corruption.

## Harness

- `run/fleet/stress-fleet.sh start [members...]` — launch all (or listed);
  `stop` sets stop files (chains untouched); `status` runs one sweep.
- `run/fleet/fleet-member.sh <member> <kind> [rounds] [first]` — per-member
  round driver (kinds: `ltp` `memstorm` `fsfuzz` `monkey`). Rounds are
  SECS=1800s, serial in `run/fleet/logs/<member>-r<N>-serial.log` (last 3
  kept), classification line appended to `<member>-rounds.log`. Wedge rule:
  no serial growth >= 300s → kill, mark WEDGE, keep tail.
- `run/fleet/fleet-classify-round.sh` — CRITICAL > WARN > WEDGE > clean.
- `run/fleet/fleet-sweep.sh` (daemon `fleet-sweeprd.sh`, 30 min) — central
  sweep; chain liveness check included.
- Host budget (user mandate 2026-10-25): fleet size ADAPTIVE; hard floor
  >= 10 GB free host memory at all times — check `free` before every
  launch/ramp step, shed members if threatened. QEMU launches niced +10.
  Shared images `-snapshot`/`snapshot=on` + `file.locking=off` only;
  writable images are private reflink copies (`images/fleet-*.img`).
- NOTE: /tmp is tmpfs (30G) — `snapshot=on` overlays live in RAM and count
  against the memory floor.

## Roster (round 1, 2026-10-25 ~04:51-05:06)

2 chains (adopted, frozen hunt kernel, self-advancing:
`run/ww-chain2.sh` / `run/ww-loadchain.sh` — do NOT restart, see
`docs/development/wildwrite-hunt.md`) + 8 members: ltp-a/b/c/d (riscv64
TCG smp2/2G, random-sweepstart LTP sweeps), monkey-a/b (x86_64 KVM q35 4G,
usbless OH image + randomized churn: sdc cycles, hilog flood/rotate,
paced appspawn kill-restart, bm/hidumper/param walks, sync), memstorm
(riscv64, init=/test/memstorm: mmap/munmap/MAP_FIXED/mprotect/fork
storms, dfx=periodic,memwatch,watchdog), fsfuzz (riscv64, init=/test/fsfuzz:
random-syscall fuzzer + file/pipe/socket/madvise/fork sequences).

## Round log (cont.)

- 2026-10-25 06:40 — OVERFLOW FIX (commit 93a18d53): fsfuzz r172 (v4)
  panicked 'attempt to add with overflow' at syscall/memory.rs:2683 —
  madvise page-round of length=-1. All seven user-length round-ups in
  syscall/memory.rs (madvise/mprotect/msync/mremap/mincore/mlock/
  munlock) now checked (EINVAL on wrap, reference-kernel end-wrap
  semantics). Verified: v5 runs the deterministic seed-7 stream 150 s+
  with zero panics (v4 died <60 s). v5 rolled to the fleet 06:39.
- 2026-10-25 06:42 — ltpm member added: LTP sweep on the CURRENT MAIN
  kernel (de41d5ab, built in detached worktree
  /home/william/rux-agents/wwh-main-kernel — control arm for
  worktree-vs-main drift; pre-merge main still carries the csrrw-time
  bug, mount-heavy sweeps are EXPECTED to panic it — that is the
  demonstration for the merge, not a new anomaly).
- Fleet: 2 chains (r91/load-r38) + 9 members = 11 QEMUs. Host floor
  >= 10G respected throughout (avail 30G at last check).

## Classification state

See "Classification state (updated ...)" below — the single source of
truth is the latest-dated block; older adjudications live in the round
log.

## ROOT-CAUSED (2026-10-25 06:30): the SIGILL family — kernel csrrw-time bug + toolchain zcb binaries; zcb adjudication REVERSED

Two intertwined SIGILL classes produced the round-2/3 crash storm; both
are now closed:

1. KERNEL BUG (fixed, verified): read_counter() read the time CSR with
   the WRITE-flavored form `csrrw a0, time, zero`. csrrw with a non-x0
   rs1 attempts a write, and `time` is a read-only CSR, so the entire
   instruction is illegal-instruction-prone whenever the platform
   enforces read-only writes. Repro: init=/test/fsfuzz-mountonly
   (mount(2) hammer loop; sys_mount's mntslow phase probes are the only
   kernel caller) -> PANIC "illegal instruction in kernel mode" with
   stval = the instruction word 0xc0101573, within ~100 s on EVERY
   kernel build tried (v1 predecessor build and v3 alike; smp1/2,
   thread single/multi, with/without zcb — my probe edits exonerated).
   Fix: `csrr {}, time` (csrrs rd, csr, x0 — the canonical pure read,
   identical semantics to rdtime; no write attempt). Verification:
   v4 kernel survived 330 s / 3491 beats of the mountonly repro where
   v1/v3 died < 100 s. Why it looked flaky: sys_mount is the only
   caller, so only mount-heavy workloads reach it; low mount-rate runs
   (memstorm: no mounts at all; fsfuzz r1: mounts at ~1/60 syscall
   density for 30 min) survive by luck of never hitting the gated state.
2. NOT A KERNEL BUG: user-mode SIGILLs at deterministic per-binary
   offsets (fsfuzz 0x10704 = `c.mul`, etc.) — the musl cross toolchain
   emits Zcb instructions into the static probes. With zcb in the CPU
   string they execute; without it they are illegal. The "SIGILL-FORENSIC
   garbage" was the binary's own valid zcb code (the forensic dump's
   epc-16 window is offset by 4 vs its label — bytes matched the file at
   the corrected offset).
3. The round-1 "QEMU zcb flakiness" adjudication was WRONG (short 70 s
   test cells + misread forensic offsets) and is REVERSED: zcb=true is
   REQUIRED for these toolchain-built user probes and is restored in
   fleet-member.sh. The repo-standard zbb/zba/zbs-only strings used by
   other lanes remain fine for their glibc/toybox workloads — but any
   binary built with run/fleet/bin/riscv64-musl-gcc needs zcb.

Fleet state after fix: v4 kernel rolled; all 8 members relaunched
(ltp-a r12 ... fsfuzz r172 continuing the numbering).

## Round log

- 2026-10-25 06:01 — full relaunch after three harness/infra fixes (round
  numbering continues: ltp-a r7 ...). (a) QEMU CPU string: zcb=true
  WITHDRAWN — QEMU 10.2.1 TCG nondeterministically raises
  illegal-instruction on legal guest instructions (fleet evidence:
  byte-identical kernel panics at sys_mount's `csrrw a0,time,zero`
  within seconds on some boots, runs 30 min clean on others; without zcb
  zero panics across all repro matrix cells; at the trap mcounteren.TM=1
  — not counter gating; repo convention `rv64,zbb=true,zba=true,zbs=true`
  restored). v=true also withdrawn (not in repo convention; alone it did
  not reproduce, kept out to match the 29 existing drivers). (b) monkey
  members now boot with dfx=memwatch,periodic — monkey-b r1 caught the
  fake-OOM class (see OPEN items). (c) stress-monkey.sh beats now write
  to /dev/console (echo went to hilog, never to serial). Root-caused in
  round 1: fsfuzz r2-r6 + ltp-a/c/d r2-r4 wedge-looped on the zcb panic.
- 2026-10-25 ~05:30 — kernel probe fixes (commit 6ea6064d + parkwalk):
  PREEMPT-LEAK print gated on non-self bits; parkwalk parked-check
  double-sampled (was pc-then-parked: a task releasing a guard and
  legally parking between the two loads printed as PARKED-WILDWRITE
  "proof" — armed-capture ground truth never corroborated one across
  88+ chain rounds; the shape fired every round at pc=1 exactly).
  Frozen chain kernel keeps printing the old shapes (capture continuity:
  do not rebase the chains); adjudicated KNOWN there.
- 2026-10-25 04:51/05:06 — fleet launched (8 members; first batch 04:51
  after fixing a boot_cmd eval quoting loss that qemu-misread `rw` as a
  disk; 12 garbage no-serial rounds rotated to *.bootfail-quoting).
- 2026-10-25 04:48 — chains self-advanced r88→r89 / load-r35→r36, both
  alive, capture continuity preserved (frozen kernel + GUARD armed).

## Classification state (updated 2026-10-25 06:05)

KNOWN classes (do not page):
- Image-side OH noise: usb_host DDK crash lines, SAMgr 15/30/89s cost
  family, RS userspace read-lock parks (excluded per hunt ledger).
- IRQ-OFF-WINDOW known-noise: 103-771ms class at rip ffffffff80573d96
  (hunt ledger canonical family). Classifier WARNs only > 2000ms.
- PREEMPT-LEAK self-observation (pc==0x10000, probe-owns-tick shape):
  fixed in fleet kernel v2+; frozen chains still print it — ignore there.
- PARKED-WILDWRITE / PARKED-LEAK on the FROZEN chain kernel: probe
  sampling race (see Round log 05:30); armed capture is the ground
  truth — 0 real hits in 88+ rounds. Fleet kernels v3+ have the
  double-check fix.
- (REVERSED 06:30 — see ROOT-CAUSED block: zcb is required for the
  musl-built probes; the "flakiness" was the kernel csrrw-time bug,
  now fixed.)
- fsfuzz-r1 494s timer starvation (timer_irq=3): consistent with
  documented TCG host-contention vCPU starvation (kernel R57 comment:
  3-10 IRQs/s under load; here 3 total over 494s at load avg 25 with 16
  QEMUs). Reclassify as kernel bug only if it reproduces on the new
  fleet config at moderate host load.

OPEN items:
- FAKE-OOM on OH monkey churn (mandate-named target): monkey-b r1
  (4G guest, appspawn kill-restart + hilog churn) hit `pagefault: Out
  of memory` and the oom-killer KILLED BOOT-CRITICAL SERVICES (ueventd
  score 573, watchdog_service 449) with OOMFX-RAW zone_free=10 of
  zone_managed=736006 and NO accounting corruption (dblfree=0,
  inverse=0, orphan=85). If cache were reclaimable the allocator should
  reclaim before killing; expected behavior (reference kernel + POSIX
  OOM semantics): reclaim page cache / drop slab first; killing
  ueventd on a 4G guest under app churn is the fake-OOM signature.
  NEXT: instrumented rounds (dfx=memwatch,periodic now on) to capture
  used-vs-reclaimable at the OOM point; then root-cause the reclaim
  path (alloc domain, guarded — see AGENTS.md merge gate + perf
  baseline).
- ltp-a r1 soft lockup 10s CPU#0 (timer_irq=962, ticks flowing): brief
  kernel monopolization during sweep; watch for recurrence at low host
  load before spending a repro cycle.

## Resume (next session)

1. `./run/fleet/stress-fleet.sh status` — one sweep (members, chains,
   flagged rounds).
2. Chains: if DEAD, see hunt ledger resume notes; do NOT restart live
   ones. Members auto-continue round numbering via rounds.log lines.
3. Relaunch dead members with `stress-fleet.sh start <member>`.
4. Monkey OOM: check `logs/monkey-*-r*-serial.log` for OOMFX-RAW + MEM
   lines; ledger OPEN item has the repro shape (appspawn killall storm
   case 3 + hilog floods on 4G guest).

- 2026-10-25 06:47 — ltpm CONTROL RESULT: main@de41d5ab panicked at
  06:4x with the exact csrrw-time signature (badaddr=0xc0101573, the
  read_counter instruction word) after 155 sweep results — while the
  same sweep on the fixed worktree kernel (ltp-a/b/c/d r12+ on v4,
  then v5) ran clean. Merge-gate evidence: main carries the
  read_counter bug fixed by 52abd2ea. ltpm continues rotating (each
  panic round auto-restarts per the member driver).

## Session 3 central sweep (2026-10-25 ~10:00-10:30, after the 06:55 quota window)

3 h of unattended fleet data adjudicated. Evidence pack:
`run/fleet/evidence/2026-10-25-sweep3-evidence.txt`.

Verdicts:

- **monkey-b r8 KERNPANIC — KNOWN, already fixed on main.** Panic:
  `pfault badaddr=0x13ff0 rip=...8075085e` immediately after
  `KILLDUMP: tid 3735928559 fdtab:` — 3735928559 = 0xdeadbeef, the
  free_task_slot pid poison. This is the kill_dump_tgid unpinned-task
  race (sc244 shape): the fake-OOM SIGKILL dump walked a victim thread
  that exited concurrently. Fixes d14d9403 + e79dc757 (originally
  samgr-callback 4dee194f/66858395) are IN current main; the fleet x86
  kernel (v1, 04:26) predates them. Independent third observation of
  that bug (sc242/sc244/monkey-b-r8). No action here beyond evidence
  preservation; goes away with the kernel refresh.
- **ltpm r1-r8 — KNOWN control result.** Every panic is
  `illegal instruction ... badaddr=0xc0101573` (the read_counter
  csrrw-time instruction word) — main still carries it at 47d96961;
  merge-gate evidence for 52abd2ea, not an anomaly. Its
  PREEMPT-LEAK=2000+/round counts are the un-gated main kernel printing
  the old shapes (known there). r6 WEDGE: post-panic quiet serial,
  watchdog silence — same class.
- **PREEMPT-LEAK pc=0x10001 on riscv members — NEW family, root-caused
  this session (see below), producer split into two classes.**
- **PARKED-LEAK on riscv members (pc=0x10000 x3626, 0x100/0x101/0x102
  x573, 0x1 x8 in current serials) — the fleet-ledger promotion rule
  fired: the class had "never been seen on NON-OH members"; it is now
  seen there. Same family as the PREEMPT-LEAK splits below.**
- **WATCHDOG=NNN rounds (ltp-c r14: 201, ltp-d r14: 228, monkey-b r4:
  323, ltp-a r18-r20: 745 lines) — ALL one signature:
  `DEADLOCK: spinlock stuck cpu=N lock=0xffffffff8080c840` =
  PROCESS_TREE_LOCK, ra=RawSpinlock::lock backoff. Guest keeps
  progressing through the episodes (sweep results continue), so this is
  extreme contention, not a deadlock. Amplifier chain: leaked-count
  holders strand (WAKE-STRANDED h0 pc=0x10001 run=176 s,
  strand_max=125 s) and everyone else piles on the global tree lock
  under fork storms. NOT load-only noise (ticks flow, timer_irq counts
  high) — treated as downstream of the count-leak family + a fairness
  question on PROCESS_TREE_LOCK sections; re-check after the count fix.
- **BUG: soft lockup 10 s (ltp-a r1, ltp-b r16/r17, ltp-c r18,
  timer_irq=257k-265k flowing)** — same downstream family: a
  non-preemptible task monopolizes a CPU for 10+ s. Call trace
  fragment: current=0xffffffff802c5ae2.
- **IRQ-OFF-WINDOW**: all samples in the 103-1677 ms known families;
  nothing over 2000 ms. memstorm/monkey HEAP-GROW lines: the OH heap
  grows to ~320 MB/round and resets on reboot — the fake-OOM OPEN item's
  instrumentation, still accumulating; no new anomaly class.
- **fsfuzz r172-r179: clean** (post-93a18d53 EINVAL fix, zero panics,
  beats continuous).
- **Chains (s2 ledger cross-ref): MAIN r89-r95 + LOAD r36-r42 all
  CLEAN** — 0 WW-HIT / 0 watchpoint hits / 0 heals / 0 panics / 0
  badaddr; WW-ARMED every round EXCEPT r89, whose gdb log shows a
  connection timeout ("could not connect") — one blind round for the
  armed capture, serial-side evidence still clean. LOAD r36-r42 added
  407-664 sdc exec cycles each (~3758 cycles).

### NEW family root-cause split (count migration), and the fix landed

The PREEMPT-UNDERFLOW / PARKED-LEAK / PREEMPT-LEAK(0x10001) trio is one
phenomenon — preempt-count bits migrating between tasks — with TWO
producers, split by fleet data:

1. **Self-inflicted by PREEMPT-HEAL (FIXED this session).** The heal
   runs INSIDE the timer IRQ bracket, so the observed task always
   carries the tick's own in-flight HARDIRQ_OFFSET; the old whole-word
   `ti_preempt_count_zero()` deleted it, and the irq_exit sub then
   landed on a zeroed count — one guaranteed
   `PREEMPT-UNDERFLOW sub=0x10000 count_was=0` per heal firing.
   Correlation: pid-1 HEAL 31x vs UNDERFLOW 40x, pid-0x683 HEAL 2x vs
   UNDERFLOW 5x. Fix: `ti_preempt_count_clear_base()` — clear only
   PREEMPT+SOFTIRQ bits, preserve HARDIRQ/NMI so the pending subs pair.
   PARKED-HEAL keeps the full zero (a task current on no CPU has no
   in-flight nesting by definition). Verification A/B in flight.
2. **tp switch inside an IRQ bracket (the deep class — HUNTING).**
   Direct observations: 12x `IRQ-BRACKET-LEAK timer cpu=1
   entry_tp=task(1530) exit_tp=idle-task` in ltp-a r20 — a context
   switch happens between irq_enter and irq_exit, so the exit sub books
   on the switched-in task (idle/0x5fa underflows; 0x5fa shows 91
   underflows vs only 1 heal — heal cannot explain it) and the
   switched-out task parks with +0x10000 (the PARKED-LEAK pc=0x10000
   stream; the parked task later resumes non-preemptible, monopolizes,
   strands wakers — the schedule_dead_never_returns comment at
   sched.rs:1740 documents this exact end-state on the OH side). The
   in-tick switch site is not yet identified: handle_timer_interrupt's
   own schedule gate is preemptibility-checked and cannot fire with the
   hardirq bit held. NEXT: instrument the bracket print with the
   interrupted-frame ra/pc to name the switch path; keep the frozen
   chains' armed capture as ground truth.

### Session-3 refresh plan (post-verification rollout)

Branch merged with main (d5c642cb, main@47d96961 + 37 lane commits);
the merged tree carries main's binder reclaim, slab work, KILLDUMP pin
fixes, panic= parameter — plus this lane's parkwalk double-sample,
csrr read, madvise EINVAL round-ups, PREEMPT-HEAL base-clear fix, and
the IRQ-BRACKET-LEAK discriminator. Images frozen:
`images/fleet-rv64-v6-rux` (pre-merge + heal fix, verification A/B
kernel), `images/fleet-rv64-v7-merged-rux`, `images/fleet-x86-v7-merged-bzImage`.
Rollout order after v6 verification passes:
1. ltp-a relaunch with KIMG=v7 (the refresh member, one full round
   clean before wider rollout).
2. ltp-c/ltp-d/memstorm/fsfuzz: stop-file + relaunch with KIMG=v7.
3. Swap images/fleet-rv64-rux := v7 and images/fleet-x86-bzImage :=
   v7-x86, then restart monkey-a/b so the x86 surface gets the KILLDUMP
   pin fixes (monkey-b-r8's panic class).
4. ltpm stays stopped: its control result is banked (csrrw panics on
   main@47d96961 — verified again r7/r8); relaunch only after main
   takes the csrr read fix, then as the main-class arm.

### Heal-fix verification result (10:30-11:00)

A/B across three kernels, same discriminators (HEAL count vs
UNDERFLOW sub=0x10000 count on the healed pids):

- v5 (fleet-rv64-rux): memstorm-r11 — HEAL(pid=1) 14x,
  UNDERFLOW(pid=1, sub=0x10000) 14x. Exact 1:1: every heal destroyed
  the in-flight tick hardirq bit and the irq_exit sub underflowed.
- v6 (pre-merge + fix only, isolated): memstorm-surface verify boot —
  HEAL 5x, sub=0x10000 underflow 0x.
- v7 (merged with main + fix): ltp-a r22 storm — HEAL(pid=0x78a) 22x,
  sub=0x10000 underflow 0x. The 22 underflows present are sub=0x1 at
  spinlock.rs guard-drop on the healed pid — the heal's DOCUMENTED
  "later unpairing sub clamps and reports" path (a 10 s-held spinlock
  section healed; the hold itself is the pathology — the hunt's next
  lever, now named at task granularity).

Verdict: PASS. The self-inflicted count-loss class is gone on both the
isolated and merged kernels; the hardirq nesting now survives heals and
pairs at irq_exit. LTP-surface verify boots wedge at oom01 (~2/2
attempts, same v5 wedge family — see evidence addendum), so the
memstorm surface carries the isolated gate.

Fleet refresh: rollout-v7.sh (detached) gates on ltp-a r22's
panic-free verdict, then rolls v7 to ltp-b/c/d + memstorm + fsfuzz
(KIMG), swaps the default rv64/x86 images, restarts monkey-a/b on the
v7 x86 kernel (carries the KILLDUMP pin fixes).

### Heal-fix final gate (11:15) — PASS, ready-for-merge

Single-variable A/B on the identical surface and pid (memstorm,
pid=1): v5 memstorm-r11 HEAL 14 / sub=0x10000 UNDERFLOW 14 (every heal
lost the in-flight hardirq bit). v6 (same tree + fix only) verify run:
HEAL 18 / sub=0x10000 UNDERFLOW 0 — 18 heals, zero hardirq losses; the
18 sub=0x1 reports are the documented guard-unpair path (healed 10 s
spinlock holds, drop-site named). v7 (merged) confirms under the LTP
storm (22/0). Fix commit bf26d2d1 on this branch; the healed-pid
hardirq nesting now survives and pairs at irq_exit. Guarded-domain
check: only the two heal sites touch count semantics; probe prints,
parkwalk, spinlock guards, irq brackets all unchanged (the
IRQ-BRACKET-LEAK counter remains the hunt instrument for the remaining
producer class).

## Session 3 close-out (2026-10-25 11:15)

Fleet state at close (v7 rollout in progress via detached
run/fleet/rollout-v7.sh — members cycle as their rounds end):
- chains ww-s2 (r96 in flight) + wwload (r43 in flight): adopted,
  frozen kernel, UNTOUCHED — do not restart; 0 armed hits through r95.
- ltp-a: v7 (merged) since r22; r22 storm-evidence round (474k lines,
  PREEMPT-LEAK 4658, no panics) — the storm itself is the next hunt's
  data.
- ltp-b/c/d, memstorm, fsfuzz: cycling onto v7 (KIMG override).
- monkey-a/b: restart onto v7-x86 (KILLDUMP pin fixes land on the x86
  surface — monkey-b-r8's panic class).
- ltpm: STOPPED by design (control result banked: main panics with the
  csrrw signature under LTP sweeps; relaunch only after main takes the
  csrr read fix 52abd2ea-class, then as the main-class arm).
- Host floor: free-vs-available interpretation logged (verify-launch
  scripts accept free>=10G OR available>=12G; page-cache pressure from
  other lanes' QEMUs kept free at 1-9G with available 26-32G all
  session).

Next-session queue (priority order):
1. v7 rollout completion check (rollout-v7.log) + first full v7 rounds
   verdicts on all members.
2. In-bracket tp-switch hunt: discriminator live fleet-wide on v7 —
   grep 'IRQ-BRACKET-LEAK' in member serials; `when=in` = the producer
   ran between irq_enter/irq_exit (count-migration source),
   `when=pre` = stale tp formed earlier. epc_ctx names the interrupted
   site. 12 occurrences on v5 (ltp-a r20) to re-correlate.
3. The 10 s-held spinlock sections the heal now names per-task
   (sub=0x1 reports name the drop site at spinlock.rs): quantify which
   lock/section (PROCESS_TREE_LOCK hypothesis from the v5 stuck
   episodes) and whether v7's merged tree changed the rate.
4. oom01-wedge family: v7 carries the KILLDUMP pin fixes — compare
   wedge rate (v5: roughly half of ltp-b/c/d rounds) on v7.
5. Fake-OOM open item: instrumentation (dfx=memwatch,periodic) keeps
   accumulating on monkey rounds; root-cause per ledger OPEN items.

## Session 4 (2026-10-25 15:00-...) — sweep of the 11:42-14:48 unattended window

### Chains: the first-ever WW-HITs landed at 10:50/10:51 — adjudicated benign

Both armed-capture chains self-stopped (by design) within a minute of
each other: ww-s2 r96 + wwload r43, byte-identical rip
(core::sync::atomic::atomic_compare_exchange+921), watchpoint 1 =
hpet::heal_report::LAST, first-fire shape (LAST==0, jiffies CAS).
Root cause: HEAL_LAST is the rate-gate static SHARED by all four
heal_report callers, including the documented-benign "clock corrected
backward" jitter clamp. r96 PROVEN benign by its own serial (line
58631, 268ms backward step immediately after a TIMER-LATE-FIRE 252ms —
the host-contention family); r43 same class (~10:20 host event during
session-3's build+sweep; print lost to serial buffering at the stall).
The wild-write engine remains uncaught: 0 real hits in 96+43 armed
rounds. Fix: chain stop-checks (ww-chain2.sh / ww-loadchain.sh) now
self-adjudicate HEAL_LAST-only hits via the serial line; chains
relaunched r97 / load-r45. Full evidence:
run/fleet/evidence/2026-10-25-heallast-hit-evidence.txt.

### Unattended-window fleet verdicts (rounds through ~14:48)

- oom01-wedge family: UNCHANGED on v7 — the KILLDUMP pin fixes did
  not fix it. Every round that reaches sweep position ~837-847 wedges
  (ltp-b r22-25, ltp-c r22/25, ltp-d r22-26/28, ltp-a r24/26/30);
  signature: sweep SIGKILLs oom01 main, children (pid 4702/4704 class)
  survive holding memory, guest stops progressing. This is the
  fake-OOM/open-reclaim class with a DETERMINISTIC 100% repro —
  priority target for the reclaim hunt (same family as the GNOME
  blocker: allocator reports OOM while reclaimable memory exists).
- Soft-lockup class (BUG: soft lockup 10s, OOPS counter in the
  classifier): ltp-a r29/r30, ltp-b r27, ltp-c r27 — the known
  non-preemptible-monopolization downstream family, timer_irq flowing.
- memstorm v7 (r15-r21): PREEMPT-LEAK pc=0x10001 steady ~390/round,
  784/786 observed at rip=enable_irq (cpu.rs:76) — one dominant
  observation site. UNDERFLOW all sub=0x1 (documented guard-unpair);
  ZERO sub=0x10000 — the heal fix holds under storm on v7. NOTE:
  IRQ-BRACKET-LEAK never fires on v7 across millions of storm lines
  while the 0x10001 rate is unchanged: the timer/soft/ext bracket tp
  switch is NOT the producer; the missing instrument is a bracket
  around SOFTIRQ PROCESSING itself (do_softirq at irq_exit). That is
  the next discriminator for the count-migration hunt.
- monkey v7-x86 (r14-r19, 12 round-hours): ZERO OOM/OOMFX/KILLDUMP —
  the fake-OOM class has not recurred on v7; PREEMPT-LEAK dropped
  31-54/round (v1-x86) to 6-14/round. HEAP-GROW instrumentation
  continues (resets on reboot).
- fsfuzz v7 r184-189: clean. r180-182 gap unadjudicated (rotation
  ate the serials; classification lines absent = clean-class).
- ltpm: stays STOPPED by design (main still lacks the csrr read fix
  at ddb3ed01 — verified by grep; relaunch when main takes it).
- execsig member: 1365 rounds on v7, ALL with the SIGILL (the
  classifier just never counted it — now it does, USIGILL=). See
  next block: the anomaly was NOT corruption at all.

### The "exec text-page corruption" REVERSED: missing V-extension support; fixed

The execsig binary's faulting page was byte-exact file content —
objdump decodes the "garbage" at text+0x1b8 as a valid RVV sequence
(vsetvli + a vectorized compare loop; the musl toolchain
auto-vectorizes ordinary loops). The kernel never wrote sstatus.VS:
with VS=Off every vector instruction (and every vector-CSR access,
vlenb included) traps illegal-instruction, so RVV binaries SIGILL on
V-capable harts. That dissolves session-3's evidence class: the
v6pre 3300/3300 child-exec SIGILLs, the ~0.5% small-binary sweep
hits, and the "PID-1 exec clean" control (that probe just had no
vectorized loop). The kernel-side suspects (read_exec_file
short-read, partial-fill reader) are exonerated; the SHORTREAD line
is normal EOF on the last 12384-byte window.

Fix (commit 8e54f35e): lazy VS enable + per-task vstate context
switch mirroring the FS discipline — capability primed from the DTB
riscv,isa string (misa is NOT S-mode readable; a misa probe traps —
caught on the first verify boot), vector-CSRs-are-VS-gated ordering
(enable before the vlenb probe; VLEN!=128 harts keep VS=Off + notice),
fork inherits, execve resets, switch save/restore Dirty-gated, raw
.word encodings verified against the toolchain assembler. The
deterministic repro (SIGILL on every one of 1365+ rounds) now runs
the full read+child-exec walk and powers down cleanly
(EXECSIG-CHILD sum=4868770075288); minimal-rootfs smoke unchanged.

Harness lesson (twice bitten): NEVER stage a fleet kernel image while
`make test` is rebuilding the same target — the first v8 image was a
test-suite kernel (the member "wedge" was the in-kernel suite running
under TCG). Stage images only from a fresh `make build`.

### Session 4 close-out (16:10)

- V-extension fix VERIFIED three ways: (1) deterministic repro — the
  execsig binary that SIGILL'd on 1365 consecutive v7 rounds now runs
  its full read+child-exec walk and powers down (EXECSIG-CHILD
  sum=4868770075288, zero SIGILL); (2) in-kernel suite on the merged
  tree: make test 1096/1096 PASS, 0 FAIL; (3) minimal-rootfs smoke
  reaches the mrsh prompt unchanged. The first fleet-rv64-v8-rux
  staging was a contaminated test-suite kernel (cp raced the make-test
  rebuild — the member "wedge" was the in-kernel suite running under
  TCG); re-staged from a fresh plain build (md5 37f0a336...) and
  re-verified with the member's exact append line.
- Fleet refresh: execsig member on fleet-rv64-v8-rux (main ddb3ed01 +
  lane fixes + V support) — rounds r1367+ clean, ~11s each, regression
  canary via the new USIGILL classifier counter. memstorm rolled to
  v8 at r25 as the heavy-switch-path soak (the V diff touches the
  switch path for every task; memstorm is the highest switch-rate
  surface). Rest of the fleet stays on v7 until the v8 soak reads
  clean.
- Branch: merged origin/main ddb3ed01 (kallsyms full table + trace
  events — addr2line symbolization used live this session; the trace
  events are the next tool for the oom01 reclaim hunt).

Fleet map at close:
- chains ww-s2 (r98 in flight) + wwload (r46 in flight): frozen hunt
  kernel, armed, patched HEAL_LAST self-adjudication.
- execsig: v8 (merged main-class + V) — the main-class refresh member.
- memstorm: v8 since r25 (soak in flight).
- ltp-a/b/c/d, fsfuzz: v7. monkey-a/b: v7-x86. ltpm: stopped (banked;
  relaunch when main takes the csrr read fix).

Next-session queue (priority order):
1. v8 soak verdicts: memstorm r25+ panics / sub=0x10000 (must stay 0)
   / PREEMPT-LEAK rate vs v7; execsig USIGILL must stay 0. If clean:
   roll v8 to the ltp members + monkeys, and propose the branch merge
   (V fix + chain adjudication + classifier) to main.
2. oom01-wedge: the deterministic fake-OOM-family repro (100% at sweep
   position ~837-847, children hold memory after SIGKILL of the test
   main, no reclaim progress). Instrument with dfx=memwatch + the new
   trace events; root-cause the reclaim path. Same class as the GNOME
   blocker (OOM at reclaimable state).
3. SOFTIRQ-bracket discriminator for the pc=0x10001 count-migration
   producer (timer/soft/ext brackets are silent on v7 while the leak
   rate is unchanged; instrument the do_softirq processing bracket
   with the same entry_tp/exit_tp shape).
4. Chains continue; HEAL_LAST-only hits now self-adjudicate (expect
   ~1 per 50-80 boot-hours under host load).

## Session 5 (2026-10-25 16:43) — cut by user pause

Pause snapshot 73f984a6 only: fleet state froze mid-round (memstorm
r26 in flight on v8, monkey r23s, chains r99/r47 ended normally). No
ledger block was written by that session; the queue above IS its
handoff. The pause protocol killed all drivers + QEMUs (fleet COLD).

## Session 6 (2026-10-25 21:36-...) — cold restart, main merge, v9 instruments

### Restart + merge

- Fleet relaunched from ledger recipes: chains ww-s2 r100 / wwload
  r48 (both ended normally at pause, no hits), members auto-continued
  (ltp-a r34..., memstorm r26 on v8 via new KIMG wiring, execsig r1586
  on v8, rest on v7). Host floor respected (26-48G avail all session;
  load peaked 24 with two lanes' make-test TCG boots concurrently).
- HARNESS LESSON (chain mislaunch, fixed same session): `cd worktree
  && setsid A & setsid B &` groups the cd into job A only — B ran from
  the MAIN repo checkout, booted the load chain WITHOUT the private
  ww31-system.img (qemu_drive skips missing files) = degraded-profile
  round. Killed (own processes only), relaunched with ABSOLUTE script
  paths. Frozen kernel md5 identical across the two checkouts
  (bf779126...), so no capture-continuity damage; r48 restarted clean
  in the worktree. Always launch chains with absolute paths.
- main merged (a5af759c): semantic no-op for kernel/ — main's only
  new commit is the V-extension cherry-pick of this lane's 8e54f35e;
  the merged tree == branch tree. MAIN STILL CARRIES csrrw-time in
  read_counter (verified origin/main:kernel/src/arch/riscv64/cpu.rs) —
  the csrr fix (52abd2ea) and madvise EINVAL (93a18d53) and the heal
  base-clear (bf26d2d1) remain un-merged main-side.

### v9: the session-5 queue's instruments (commit 477648cd)

v9 = main-class a5af759c + lane fixes + (a) SOFTIRQ-BRACKET-LEAK (the
prescribed do_softirq processing-window tp bracket, first_pending
names the vector class), (b) pctick= on PREEMPT-LEAK lines (per-CPU
snapshot of the interrupted task's own count BEFORE irq_enter books
the tick offset — splits leaked-PREEMPT-bit vs stranded-hardirq-bit
producers). Staged from a fresh plain make build (md5 25d94392...;
no make-test race this time). Classifier counts SOFTIRQ-BRACKET= and
IRQ-BRACKET= now.

### Verdicts (first v9/v8 data)

- memstorm v8 soak (r25+r26 full rounds): PREEMPT-LEAK 350/350 (v7
  band 364-395), ZERO panics, ZERO sub=0x10000, beats 62/73 — the
  switch path with V state in it shows NO regression. execsig v8
  1700+ rounds USIGILL=0 (regression canary clean).
- MAIN-CLASS CONTROL PASS: fresh build of origin/main a5af759c
  (images/fleet-rv64-main-rux, md5 6d776535) runs the execsig repro
  clean — EXECSIG-CHILD sum=4868770075288 (exact expected checksum),
  zero SIGILL, clean powerdown. Main's V cherry-pick lost nothing.
- THE 0x10001 PREEMPT-LEAK FAMILY ON MEMSTORM IS A SAMPLING ARTIFACT,
  not a leak: v9 verify boot (300 s) — 39 lines, ALL pctick=0x1,
  rip symbolizes to SigPending::remove (the SIGCHLD dequeue loop;
  SigQueue.inner is a Spinlock<VecDeque> whose lock books the +1);
  ZERO heals and ZERO underflows (a true strand would heal at 10 s and
  print the sub=0x1 pair — the ltp-a r22 shape). pid 1's dequeue loop
  duty-cycles inside the sigqueue spinlock at ~100% at tick-sample
  granularity. The REAL pathology underneath is the dequeue-loop
  burn (soft-lockup/monopolization family). CONSEQUENCE: the
  ~365/round metric was a user-loop behavior number, not kernel
  count-migration evidence; switch-path safety rests on the
  sub=0x10000=0 + panic-free + beat-stable legs (all hold).
- oom01-wedge hypothesis (pctick data pending from v9 ltp-c rounds):
  the wedge tail's surviving children show cpu0 pc=0x100 / cpu1 pc=2
  then SERIAL SILENCE (no 1 Hz timer-softirq heartbeats = softirq
  dispatch stopped on BOTH CPUs). Hypothesis: a genuinely STRANDED
  SOFTIRQ_OFFSET makes a task permanently non-preemptible; irq_exit's
  in_softirq() skip defers softirq dispatch to ksoftirqd, which can
  never preempt the non-preemptible task → timer softirq starves →
  silence → wedge. v9 ltp-c (deterministic repro at sweep ~837-847)
  will print pctick= at the wedge: sustained pctick>=0x100 confirms;
  pctick=0 at every sample means the WW3 pc=0x100 was the heartbeat's
  own window and the producer is the pc=2 double-preempt class.
  Full evidence: run/fleet/evidence/2026-10-25-s6-pctick-evidence.txt.

### v9 rollout (staged, per session-3 protocol)

Gate: 300 s memstorm-surface verify boot (14 beats, 0 panics) PASS →
rollout-v9.sh rolled v9 to memstorm (r27+) and ltp-c (r33+, the
oom01-repro surface). Default images NOT swapped yet; ltp-a/b/d +
fsfuzz + monkeys roll after ltp-c's first full v9 round reads clean
(x86 image staged: fleet-x86-v9-bzImage, md5 71652c65).

## Session 7 (2026-10-26 00:40-...) — oom01-wedge ROOT-CAUSED; v10 rolled; v11 gated

### Fleet arrival + main merge

- s6's fleet survived the quota window intact: 10 members + 2 chains
  (ww-s2 r105, wwload r52 advancing) + sweeprd all adopted. Host floor
  OK all session (free 3-10G / avail 21-33G under load 20-27; the
  page-cache reading of the rule per session-3 close-out).
- main merged (4842a252, b332d08a): ALL lane kernel fixes now on main
  (csrr 7248c3ae, heal base-clear 1e9f7c01, madvise 66bd3736, both
  bracket discriminators 93e1b667/4a8d97e5) + the softirq PER-VECTOR
  2ms budget fix (7d6e1449), pstore-lite, tracepoint! events, ABI fix
  wave. Kernel delta branch-vs-main is now just the two sched probes
  (PREEMPT-LEAK print gate, parkwalk double-sample).
- ltpm relaunched r9 on PURE main d16571e8 (first ltpm rounds ever on
  a csrr-fixed main): r9 sweepRES=837 OOM=8, wedge=NO, no csrrw panic
  (control arm live and clean).

### THE oom01-wEDGE ROOT CAUSE (evidence: run/fleet/evidence/2026-10-26-s7-oom01wedge-rootcause.txt)

The v9 wedge tail re-read with the full serials: every wedge (ltp-a
r40/r41, ltp-b r38/r39, ltp-d r36, ltp-c r40) shows the oom-killed
group LEADER stranded at pc=0x100 (in_softirq, non-preemptible) in
25/25 consecutive 1 Hz samples; its SIGKILL never delivers, threads
4271/4272 die (oom_kill_process Step-2 mm-sharer kills) holding mm
refcounts, memory never frees, allocator re-OOMs forever. ticks=[0,0]
both CPUs; Timer/Hrtimer softirqs keep dispatching on the other hart
(so the s6 "softirq dispatch stopped" sub-hypothesis was wrong). All
bracket discriminators 0 — the window's EPILOGUE never runs, so the
exit-side print cannot fire by construction. pctick= on pre-wedge
PREEMPT-LEAK lines: all pctick=own-count (0x1/0x3) — the s6
"sampling artifact" adjudication confirmed; no stranded-hardirq-bit
producer exists on this surface.

Live capture: the oom01-wedge-gdb driver's trigger reworked
(persistent-pc=256 + SWEEP-RES-stall; attempt-1 lessons: pid 0/1
duty-cycle in softirq context every heartbeat, single LTP tests run
>30s between RES lines). The first v10 LTP boot BLEW PAST oom01 with
ZERO OOM events (no wedge!) but hit a ~100 s pc=0x100 strand at
position ~640 under a BLOCK-softirq storm (vec[4] ~2000/s) — the
new ww-strand-dump.sh attached gdb DURING the strand and named it:

    hart0: RawRwSpinlock::write (rwlock.rs:94) <- SPINNING
           <- GlobalPteLock::lock_irqsave <- PteLock::lock_irqsave
           <- handle_cow_fault (mm_ops.rs:2162)
    hart1: Page::next_free <- Zone::remove_from_free_list (allocator)

DEFECT: handle_cow_fault held pte_lock_for(mm).lock_irqsave across
the FULL alloc_pages(GFP_USER) slowpath — local IRQs off + PTE epoch
held while reclaim/OOM-wait/free-list churn ran, starving every
global PTE writer and same-mm section for the slowpath's whole
duration. Fix (994cbc60, riscv64 + x86_64 twins): speculative
pre-lock alloc + SpecPage Drop-guard (Linux wp_page_copy discipline;
every no-copy outcome already returns Retry/Resolved, the guard frees
the page; pre-lock OOM returns OutOfMemory with IRQs on, no lock).

### v10 roll + verdicts

v10 = merged main-class e1b794ef (verify boot 14 beats/0 panics).
rollout-v10 staged: ltp-a (r43+) + ltp-c (r41+), then memstorm (r35+)
+ fsfuzz + execsig + default rv64 swap. ltp-b/ltp-d stay v9 as the
budget-fix A/B controls. Rollout BUG (fixed for posterity): the
canary gate grepped "round N:" off the rounds.log's trailing
"serial:" preserve line -> LTPA_NEXT=1 -> matched the historical r1
line — the wide roll ran on the de-facto evidence (verify boot +
ltpm-on-main + the repro's own 1200-test v10 LTP run) instead; the
x86 verify in the script also hand-rolled a broken argv (no initrd/
root -> "no init program loadable") — completed manually with the
exact member argv.
- ltp-a r43 (v10): sweepRES=837, OOM=10, wedge=NO + 1 KNOWN
  soft-lockup. ltpm r9 (main): 837/OOM=8/no wedge. BUT ltp-b r41
  (v9 CONTROL): 845/OOM=11/no wedge TOO — first v9 survival at the
  wedge position. The wedge is load-timing dependent; the budget
  fix's credit needs n>1 per arm. The 100 s strand class (pid 1100
  on v10, avg_run 283ms, stranded 15171) is the still-open residual.
- memstorm r35 (v10): beats=61, PREEMPT-LEAK=352 (v7-v9 band
  348-395) — main-class soak clean.

### v11 (v10 + COW pre-lock fix) GATED OUT on the memstorm surface

v11 verify boots (300 s AND 600 s): memstorm crawls at ~0.8
beats/min vs v10's ~2.7 (beats 48->80 in ~7 min; guest alive, 0
OOM/refusals/corruption). smp1 control: 3.5 beats/min — normal.
Adjudication: NOT memory corruption — removing the PTE-epoch
serialization un-masked a scheduler fairness starvation (fork-storm
children monopolize both CPUs; pid1 runnable-not-running, the W3
wake-refusal family). v11 stays UNROLLED; next session: v11 canary
on ltp-c only (LTP surfaces are not fork-storm shaped), fairness
question joins the W3 hunt.

### Sweep verdicts (KNOWN vs NEW)

- monkey-a r32 KERNPANIC (v9-x86): pfault badaddr=0x13ff0 rip=
  0xffffffff8078be9e immediately after "KILLDUMP: tid ... fdtab" —
  the monkey-b-r8/sc244 KILLDUMP class RECURRED on v9-x86 (which
  carries the d14d9403+e79dc757 pin fixes): the fdtab-walk lifetime
  corner is still open (not the task pin). Serial preserved
  (monkey-a-r32-serial.log); every round reaching HEAP-GROW >500MB
  ends in this panic class (r8/r20/r24/r26 serials rotated away
  pre-fix; the fleet-member CRITICAL keep-tail fix (3MB, keep-*
  names) now preserves them). OPEN: root-cause the fdtab walk race;
  the v10-x86 roll (pstore equipped) captures the panic record
  across the reboot boundary.
- Harness: CRITICAL-round serial keep-tails; ww-strand-dump.sh (live
  strand gdb attach); oom01-wedge-gdb trigger hardening.
- Chains: ww-s2 r105→r106, wwload r52→r53 clean, 0 WW-HITs.

### Fleet map at session-7 close

- chains ww-s2 (r106+) + wwload (r53+): frozen hunt kernel, armed.
- v10 (main-class): ltp-a, ltp-c, memstorm, fsfuzz, execsig + the
  DEFAULT rv64 image. ltp-b, ltp-d: v9 controls. ltpm: pure main
  d16571e8. monkeys: v9-x86 pending the manual x86 verify-v10x2 gate.
- v11 (COW fix): staged, gated out, awaiting an ltp-c canary next
  session.

### Next-session queue

1. v11 on ltp-c (one member): does the oom01 surface change? If
   clean, roll v11 wide EXCEPT memstorm; memstorm waits on the W3
   fairness companion.
2. Wedge A/B statistics: ltp-b/d (v9) vs ltp-a/c (v10) wedge rates
   at position ~837 over several rounds before crediting the budget
   fix; the 100 s BLOCK-storm strand class is the residual target
   (the COW fix addresses its root).
3. KILLDUMP fdtab-walk race (monkey-a r32 evidence + keep-tails +
   pstore on v10-x86 once monkeys roll).
4. oom01 OOM=8-11 still fires at 837 on main-class: is the OOM
   kill+recovery now CORRECT (test completes) or still fake-OOM
   (reclaimable existed)? Read the OOM-FORENSIC zone_free vs
   reclaimable in the keep-tails.
5. Fix rollout-v10.sh's next_round() (grep "round .*: R" lines, not
   tail -1) before the next staged roll reuses it.

## Session 8 (2026-10-26 07:15-...) — first attempt died at ~43min (provider captcha); second attempt adopted the fleet intact

### Adoption + merges

- All 10 members + 2 chains + sweeper survived the provider death
  untouched (adopted, not restarted). dirtyscan (the session-8-opening
  member, e357376b) was already running its first rounds.
- Predecessor's in-flight work landed before the death and was adopted:
  KILLDUMP census stack-freed ROOT CAUSE (fb643799 — free_kernel_stack
  zeroes kernel_stack_bottom but the pinned Task stays visible to
  for_each_task; kernel_stack_top() = 0+0x14000; pt_regs read at
  0x13ff0 panics the boot from inside a printer; fix =
  kernel_stack_top_checked() guards on every diagnostic reader of a
  foreign stack), v11-canary launch, v12-x86 staging.
- main merged: 94c670da (DFX batch-5 klogd/Oops/pstore + F4 route-1 +
  D-6 capability-checked FileOps builder) — conflict only in the
  GENERATED kernel/src/config.rs (stale platform flavors of an identical
  Kernel.toml; build.rs regenerates, took ours).

### v13 roll to the monkeys (KILLDUMP-fix validation)

- The predecessor's rollout-v12-monkeys gate FAILED spuriously: the
  verify grepped "stress-monkey beat" but the probe prints MONKEY-BEAT;
  the 300 s v12 verify actually had 171 beats / 0 panics. Pattern fixed;
  v13 = merged main 94c670da + fb643799 built both arches; 300 s verify
  gate PASSED (164 beats, 0 panics); monkeys relaunched on v13-x86
  07:41 (r43+). First v13 rounds: monkey-a r43 WARN HEAP-GROW=115MB,
  monkey-b r43 WATCHDOG=1 (swap_alloc_slot spinlock trip — pre-existing
  noise, monkey-b r4 had WATCHDOG=323 on v10-x86), both panic-free but
  below the >500MB heap-growth trigger that reliably produced the
  KILLDUMP panic on v9/v10-x86 — verdict round still pending at this
  writing.

### THE DIRTY-PAGE-RECYCLER AUDIT (OH leg-22 handoff) — TWO REAL BUGS FIXED

Audit scope: every path that installs a user page (demand faults both
arches, anon fault-around both arches, fork COW copy, exec pre-map
allocators, munmap/MAP_FIXED zap, exit/exec teardown, vmscan swap-out).
Fault paths are zero-clean; fork/munmap/teardown locking is sound (the
recycled-frame families there were fixed in prior sessions). Two holes
found and fixed:

1. x86_64 single-page FILE fault, negative read_at (590368ba):
   will_full_read skipped the pre-zero; the two re-zero arms were
   `bytes_read == 0` and `bytes_read > 0 && < PAGE` — a NEGATIVE errno
   from File::read_at (EIO under I/O churn, ESPIPE, EBADF) fell through
   BOTH, mapping recycled-buddy content (previous owner's bytes) into
   user space. Exactly the leg-22 bssslack2 evidence class (foreign
   strings + live pointers in a mapping nobody wrote). riscv64 clean
   (fill_file_page fixed this shape long ago; its single-page path
   always pre-zeroes); the x86 fault-around batch was also clean (uses
   fill_file_page) — only the x86 single-page path kept the hole.
2. vmscan swap-out pending-mark ordering (f87b0f29): the single-mapping
   path installed the swap PTE and only THEN marked the slot
   write-in-flight; a fault in the [installed, marked] window swapped
   the slot in immediately and read the slot's PREVIOUS device content
   (another page's bytes) into a fresh anon page. Fix: mark BEFORE
   install; the no-write arms clear the pre-set mark. Relevant to every
   swap-active surface (the LTP members boot with swap active — r51
   census "swap active=true"; monkey-x86 swap_alloc_slot contention
   shows swap live there too).

Both fixes are in v14 (staged, md5 x86 ba2a234d); rollout-v14-monkeys.sh
is a SCHEDULED roll: it waits for the v13 KILLDUMP-verdict round
(HEAP-GROW>500MB panic-free on both monkeys, or a panic, or a 90-min
deadline), runs its own 300 s verify gate, then swaps the default x86
image. dirtyscan stays on the v10-class default this session (its hunt
targets the anon zero-fill surface; a mid-hunt kernel change adds a
variable — next session's call).

### Budget-fix A/B verdict: NO wedge-prevention credit

Post-roll windows (n=21/arm): v10 arm (ltp-a r43-53 + ltp-c r41-50) 9
wedges; v9 arm (ltp-b r41-51 + ltp-d r40-49) 12 wedges; ltpm (pure
main) 7/19. Difference is within noise; BOTH arms wedge. The softirq
per-vector budget fix does not prevent the oom01-wedge class.

### v11 (COW pre-lock fix) canary verdict: wedge SURVIVES the COW fix

- ltp-c r51 (v11): swept to 845 with wedge=NO — first v10-class member
  to cross the historical wedge position... but r52 wedged at 643 in
  the SAME pc=256 oom01 class (pid 2712 stranded in_softirq at
  pc=0x100; sweep's SIGKILL to oom01 never delivers). Canary n=2: the
  COW fix does NOT kill the wedge. Combined with the A/B above, the
  wedge survives BOTH candidate fixes — the surviving mechanism is the
  W3 family itself (non-preemptible pc=0x100 strand + undeliverable
  SIGKILL). Coordination note posted to run/CC-COLLAB.txt (W3 lane
  gets the reproducible producer + our strand probes).
- v11 r51 OOM-FORENSIC answer (session-7 queue #4): at the OOM point
  memory is genuinely full (free=0, anon=1613MB of 2G, dirty=0) and
  the kill+recovery is CORRECT (test crossed 845, no wedge in r51).
  NEW NEXT-SESSION LEAD: ~204MB of clean, unmapped inactive-file cache
  (lru[2]=43764+8611) sat unevicted while the allocator OOM'd — either
  the file-LRU and the page-cache eviction index drifted, or those
  pages are silently pinned. Evidence: ltp-c-r51-serial.log OOM-FORENSIC
  #2 + MEMLEDGER census. A shrink-eviction-reason counter would decide.

### Sweep verdicts

- No new fleet-wide anomaly classes this session. dirtyscan r1/r2
  clean (2160+ cycles, 6M pages scanned, 0 DIRTYHIT/GUARDHIT, 2300+
  churn forks) — the hunt continues armed.
- Chains ww-s2 r112, wwload r61 advancing, 0 WW-HITs.
- ltp-c r51 soft-lockup (CPU#1 10s) = the known pc=256 residual class;
  the round recovered.

### Fleet map (session-8 mid/close)

- ltp-c: v11 canary (r53+). ltp-a + memstorm + execsig: v10 (default
  rv64 image = v10). ltp-b + ltp-d: v9 controls. ltpm: pure main
  d16571e8. monkeys: v13-x86 → v14-x86 via the scheduled roll.
  dirtyscan: v10-class default (KIMG hook available for a pinned
  kernel). Chains ww-s2/wwload: frozen hunt kernel.

### Next-session queue

1. KILLDUMP verdict: read the v13/v14 monkey rounds — any >500MB
   HEAP-GROW round panic-free on both = fix confirmed; a KERNPANIC at
   the old rip = fix incomplete (pstore on v13/v14 survives reboots).
2. The 204MB-unevicted-clean-cache question (r51 OOM-FORENSIC).
3. W3 wedge hunt (with the OH lane): producer reproducible on LTP at
   ~600-850; both budget and COW fixes exonerated.
4. dirtyscan: consider KIMG=v14-rv64 + a swap-activating variant (the
   swap-pending fix's negative test needs swap pressure).

## Session 9 (2026-10-26 11:27-...) — cold restart, VS-SIGILL kernel fix, modchurn member, the 204MB question ADJUDICATED

### Cold start + refresh to main-class

- Host fresh at open: 54G free, load 0.3, 0 QEMUs (pause protocol had
  taken everything, chains included). main still 94c670da (already the
  branch merge-base; nothing to merge).
- Order: ltpm main-kernel rebuild first (wwh-main-kernel detached
  worktree -> 94c670da, md5 4b4baade), v14-x86 verify boot (the
  dead-at-launch scheduled roll's gate run manually: 210 beats / 0
  panics -> PASS), default x86 := v14, chains relaunched with
  numbering continued (ww-s2 r115+, wwload r64+), then all members.
  Full fleet by ~12:05: 12 members + 2 chains + sweeper = 15 QEMUs
  (13-14 steady), floor 27G free at the tightest; other lanes came up
  through the session (free dipped to 1G with avail 32G once — the
  logged page-cache reading; no launches happened below that).
- LAUNCH MISTAKE (fixed): the wwload chain's compound launch
  backgrounded the cd with the ww-s2 half, so ww-loadchain.sh ran from
  the MAIN repo dir; r64's artifacts landed in workspace/Rux/run.
  Round was valid (armed, storming, 0 WW-HIT); stop file set, chain
  ended cleanly after r64, relaunched from the worktree (r65+).
- budget-fix A/B verdict: carried from session 8 (no wedge credit;
  both arms wedge) — nothing further this session.

### NEW KERNEL BUG FIXED: V-first-use SIGILL under enabled FS (v15)

- Repro surfaced by the NEW modchurn member's very first boot (see
  below): pid 1 (dynamic binary, ld-musl mapped, lib dlopen'd fine)
  SIGILLed at its first auto-vectorized vsetvli (0x0c007657) with
  forensic fs=0x2000 — FS already Initial, on a v=true hart.
- Root cause: the V-extension first-use lazy-enable branch lived
  INSIDE the `if fs == 0` decode block of the riscv64
  illegal-instruction handler. Any task whose FP was enabled first
  (the dynamic-loader path enables FS before main; any printf/libm)
  never reached the VS handler and died on its first RVV instruction.
  The s4-era fix looked complete because the execsig-era repros hit
  their first vector instruction with FS still Off. ABI divergence vs
  Linux (vectorized user binaries + prior FP use = SIGILL).
- Fix b174383e: branch hoisted to handler top level (conditions
  unchanged; VS and FS are independent sstatus fields). Verified: the
  same modchurn boot on v15 = 10 beats, phases live, 0 SIGILL; both
  arches build rc=0. v15 rolled fleet-wide as the rv64 default.
  Evidence: run/fleet/evidence/2026-10-26-s9-vsfirstuse-sigill-rootcause.txt;
  CC-COLLAB note warns other lanes until it reaches main.

### modchurn member (the OH leg-22 bssslack2 hunt — session-9 top item)

- Probe: dynamic binary + churned .so whose ctor writes only page 0
  of a large bss; every dlopen scans the untouched interior + the
  in-page tail past memsz (the exact OH bssslack shape); donor-tag
  fork waves feed the recycler identifiable frames; a 24MiB
  pattern-region swap roundtrip under hog pressure (the swap-pending
  fix's standing negative test); MADV_DONTNEED re-zero check. Rootfs =
  reflink of dirtyscan's + debugfs injection (sf-musl interp name
  installed alongside the plain one). Markers MODDIRTYHIT/SWAPHIT ->
  CRITICAL (SWAPHIT counter added to fleet-classify).
- r1 lesson (probe bug, fixed same session): the slack scan derived
  its range from /proc/self/maps, which coalesces the module bss with
  the adjacent malloc arena — 70 "hits" at the exact array-end
  boundary with stable allocator pointers = neighbor reads, not
  recycle evidence. v2 (cb48c1ce) gives the bss arrays mid-page ends
  (0x40000-0x51c / 0x10000-0x2a4, the OH layout) and scans only the
  containing page's tail. The untouched-INTERIOR surface was clean
  throughout r1 — and stays clean through r4+ (dh=0) with 200+ donor
  waves feeding tagged frames.
- r1 also WEDGED in the known W3 class (see below); hogs tuned
  4x175MiB -> 3x128MiB. r4+ complete swap roundtrips clean: pattern
  bytes verified byte-exact after swap-out + swap-in (sh=0, srts>=1
  per 3 cycles) — the swap-pending fix holds under this pressure.

### W3 producer + verdicts on the session-8 queue

- modchurn r1's 4x175MiB hog phase froze BOTH harts (ticks=[0,0],
  pid 562 pc=256, "BUG: soft lockup CPU#1 stuck for 199s", timer late
  201535ms) — the machine-wide strand class reproduced ON DEMAND in
  ~25 min. Producer handed to the W3 lane via CC-COLLAB (modchurn
  r1 wedgetail preserved).
- KILLDUMP fdtab-walk verdict: monkey-a r45 (v13-x86) printed the
  FULL KILLDUMP census including the fdtab listing TWICE for a killed
  launcher (tgid 2297) — the exact r32 death site — with NO KERNPANIC;
  the round continued 107K lines and timed out normally (its OOPS=1
  was the known 10-12s soft-lockup class). Fix fb643799 holds on the
  observed firing path; the formal >500MB HEAP-GROW panic-free
  criterion has not re-met (r43-46 max ~190MB), so monkeys stay on
  v14-x86 accumulating — pstore persists any recurrence across
  reboots now.
- v11-canary fairness question (fork-storm starvation): memstorm on
  the v14/v15 default (which CARRIES the COW pre-lock fix) runs 2.1
  beats/min (r49, 72 beats/34min) — inside the v10 band, NOT the 0.8
  crawl. The v11-era starvation does not reproduce on v14-class;
  something between v11 and v14 (main's DFX batch-5 wave) or load
  timing resolved it. The COW fix is now fleet-wide with no memstorm
  regression.

### THE 204MB-UNEVICTED-CACHE QUESTION — ADJUDICATED (v16 counter)

- v16 (618a2048) adds a file-LRU clean/dirty breakdown walk to the
  OOM-FORENSIC printer (dirty/writeback/referenced/unevictable/locked
  per file LRU; unlocked best-effort walk, 300k bound, self-loop
  guard) — "measuring is part of the change".
- First v16 firing: ltp-b r58 OOM-FORENSIC #1+#2 — file-lru[2]
  walked=34313-37569 with dirty=0 writeback=0 locked=0 unevictable=0;
  file-lru[3] walked=7180-9412 same. ~175MB of CLEAN, unmapped,
  unlocked, non-unevictable file cache sat on the LRUs at the OOM
  (zone free=0). NOT writeback starvation, NOT pinning, NOT an LRU/
  pagecache index drift (walked == lru_sizes).
- THE NEW LEAD the counter surfaced: 88-93% of INACTIVE-file pages
  carry the Referenced flag (34859/37569 in #2) while ACTIVE-file
  shows referenced=0 — the inverse of a healthy LRU. If the reclaim
  scan honors Referenced as "hot", the inactive list is 90%
  unskippable and direct reclaim finds nothing -> OOM with 175MB
  clean cache one list away. Candidates: mark_page_accessed setting
  Referenced without activation (every file read + readahead), the
  scan never clearing Referenced on the inactive list, or promotion
  never firing. Evidence: run/fleet/evidence/
  ltp-b-r58-v16-oomforensic-serial.log + the lines above.

### Fleet map at session-9 close

- chains: ww-s2 r116+, wwload r65+ (worktree-rooted, frozen hunt
  kernel, armed, 0 WW-HITs).
- rv64 default = v16 (5ac7f2dc: main-class + KILLDUMP census fix +
  both dirty-page fixes + VS-first-use fix + file-LRU counter):
  ltp-a/b/c/d, memstorm, fsfuzz, execsig, dirtyscan, modchurn.
- ltpm: pure main 94c670da (4b4baade). monkeys: v14-x86 (ba2a234d).
- Known classes live: the ~837 OOM-kill (correct kill+recovery), the
  10-12s soft-lockup residual, W3 wedges in the 573-850 zone (ltp-b
  r57 PREEMPT-LEAK=3834 census storm there), ltpm PREEMPT-LEAK
  ~2000/round vs branch ~10-30 (watch, not adjudicated).

### Next-session queue

1. THE Referenced-flag lifecycle on the file LRUs (the 93%-referenced
   inactive list): read vmscan's skip-if-referenced logic +
   mark_page_accessed/promotion paths; the 204MB question's successor.
2. modchurn accumulation: bssslack2 negative/positive over many
   rounds (interior + in-page tail), SWAPHIT watch. If a real hit
   lands, the donor tag names the frame's previous owner.
3. KILLDUMP: the >500MB HEAP-GROW panic-free round on v14-x86
   (pstore makes any recurrence survive reboots).
4. W3 hunt support on request (modchurn 4x175MiB producer argv).
5. VS-first-use fix cherry-pick to main (b174383e) — warn lanes until
   merged (CC-COLLAB note stands).

## Session 10 (2026-10-27 11:29-...) — sf2: fleet home moves to wwh-main-kernel, batteries re-armed on main 57e44a90

### Re-anchor and merge state

- Worktree /home/william/rux-agents/wwh-main-kernel, new branch
  stress-fleet-2. Predecessor HEAD 94c670da turned out to be a pure
  ancestor of origin/main 57e44a90 (42 commits ahead), so the branch
  start == main; nothing to merge.
- Fix inventory vs main (all under cherry-pick hashes): c831df2e (x86
  file-fault negative read_at re-zero), 3db98d39 (swap-out pending
  precedes PTE install), 1350dc00 (V first-use hoist), 66bd3736
  (page-round checked arithmetic), bd4f331a (megapage demotion for 4K
  maps), 81fa78af (ext4 ENTRY_MUTEX + split refusal), 7d448132
  (reparent PF_EXITING), 2968ad13 (COW pre-lock, oom01 wedge), and the
  NEW dd9b5b9b faultlat merge (page cache at RAM scale, true LRU +
  active demotion, batch reclaim) — the successor to session-9's
  Referenced-flag lead and a brand-new reclaim engine to hunt.
  NOT yet in main: scoped-publication bio draining (c3e5a314, OH line),
  KILLDUMP census fix, OOM-FORENSIC file-LRU counter.
- dfx instruments confirmed on main: wwcensus (census behind a token —
  ordinary boots stay serial-clean per b7870bfd/6ed45aba), txnhunt,
  treehunt, pathstash, pstore, ksyms.
- Fleet home moves HERE from the wildwrite-hunt worktree (this lane's
  standing home per the mission; the old worktree keeps its artifacts
  untouched). Rootfs images full-copied (ext4 has no reflink; 8.4G),
  fleet kernels staged fresh: fleet-rv64-rux := main 57e44a90 riscv64
  build md5 0f35b05b (forced-rebuild-verified byte-identical),
  fleet-x86-bzImage := x86_64 twin md5 67e527db. Round numbering
  restarts at 1 in this home (r1-r16 of dirstorm were bring-up, see
  below). 150 s LTP smoke on the new kernel: 425 TPASS, 0 panics.

### dirstorm member (NEW) — the ext4 ENTRY_MUTEX + dir-split retest

- test/dirstorm.c: 4 workers, shared + private dirs, mkdir/rmdir/
  rename/unlink/creat/link/symlink mix, 190-byte-name split-grow
  phases (64 entries grown + purged per phase — large rec_len at
  directory-block ends, the leg-29A panic geometry), paced dir fsync,
  per-file pattern verification (self-read on create + random re-read).
- Bring-up defects (probe-side, NOT kernel evidence): (a) Ubuntu
  libgcc soft-fp members pulled via musl printf carry czero.nez/
  czero.eqz — SIGILL at insn 0x0f5bfbb3 until zicond=true joined the
  fleet CPU string (same class as the zcb precedent; commit 65de8492);
  (b) my worker dirs were never mkdir'd (ENOENT flood). r1-r16 were
  these; r17+ is the first clean surface (odd=0, ~240 files/worker/
  beat, split counters climbing, 0 DIRSTORM-CORRUPT / KERNPANIC).
- Also fixed mid-bring-up: a copy-paste slip put init=/test/dirstorm
  into the dirtyscan case — dirtyscan r1 booted to a shell; corrected,
  r2+ runs the real probe.

### Wave 1 (host budget: 7 agents share the host; floor held >=10G free)

dirstorm r17+ (ENTRY_MUTEX surface), dirtyscan r2+ (the wwh
dirty-page-recycler hunt on the new true-LRU/demotion reclaim code),
fsfuzz r1+ (broad syscall+fs fuzz). Staggered launches per the
resource discipline; ramp gated on a 10 min hold + floor check.
