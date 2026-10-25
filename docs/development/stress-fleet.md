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
