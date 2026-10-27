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

### Session 10 continued — fsfuzz chase: one kernel bug fixed, one probe defect exposed

- fsfuzz rounds on main froze at 231 lines deterministically. gdb +
  dfx=periodic taskdump on a repro: pid 1 INTERRUPTIBLE-parked in
  epoll_pwait(0, 0x1000, ...) with the machine otherwise healthy (both
  CPUs idle, kthreads parked, RWFRZ alive).
- printf-per-call trace variant decoded the deterministic seed-7 stream:
  close(0) -> epoll_create1 reuses fd 0 (valid EMPTY epoll) -> close(1)
  blinds the probe's own stdout -> epoll_pwait on fd 0 with an
  unbounded timeout draw (Linux blocks identically). Every historical
  "clean" 7.5k-line fsfuzz round in the old home was wwcensus
  heartbeats over this parked machine — the member has contributed ~5 s
  of fuzzing per round since the v4 era. NOT a kernel defect.
- Probe hardening: all marker prints through a saved fd dup'd ABOVE the
  fuzz fd pool (F_DUPFD 900; a fd-3 dup variant wedged the same way),
  epoll_pwait timeout draws clamped to <=200 ms, private rootfs image.
- THE REAL CATCH hiding behind the park: once the stream could run
  past it, seq_madvise(0x7ffffffff000, huge) PANICKED the kernel —
  'attempt to add with overflow' at syscall/memory.rs:2813
  (`addr + length_aligned`, unchecked; the 66bd3736 page-round fix
  checked the length only). Fixed with checked_add -> EINVAL (msync
  twin ec87f42b; mlock/munlock/mremask verified already guarded).
  eex4 trace boot = the panic capture; eex5 on the fix = full stream,
  0 panics. Commit ec68f275. Tier-1/2 gate owed before main merge.
- ltp-a r1 CRITICAL adjudicated: known classes (10-12 s soft-lockup
  residual CPU#1, PREEMPT-UNDERFLOW count-migration family, LTP OOM
  output strings). memstorm r1 clean (87 beats, heap flat). dirtyscan
  r3 clean. modchurn r1 clean (dh=0 through 36 loads/84 donors).
- Main merged: 0cd29a75 (scoped-publication draining + DFX batch
  edf22c33); dual-arch rebuilt (a46464fb/654ca5df), 150 s LTP smoke
  397 TPASS 0 panics; members pick the new kernel on round rollover.
- OPEN: the fixed fsfuzz member still reaches a deep park (~255 lines,
  ~1 beat in, past all prior points); member-argv repro with gdbstub
  (fsfuzz-repro3) running for a last_syscall read on the frozen guest.

### RETRACTION NOTE (session 10, standing): historical fsfuzz rounds carry NO coverage evidence

Every fsfuzz round classified clean/WARN before 2026-10-27 (both fleet
homes, v4-era through v16-era kernels) was a machine parked ~5 s into
the round: the deterministic seed-7 stream closed fd 0, let
epoll_create1 reuse fd 0, closed fd 1 (blinding stdout), and blocked
in epoll_pwait on the valid-but-empty epoll with an unbounded timeout
draw. The 7.5k "active" lines per round were wwcensus heartbeats over
the parked boot. Do not cite those rounds as evidence that the fuzz
surface was exercised; the first actual coverage from this member
begins with the saved-fd/clamped-timeout probe (test/fsfuzz.c, this
session). Probe-design rule recorded: fuzz fd pools must never contain
the marker-print channel's fd (the F_DUPFD-from-900 escape hatch), and
blocking-wait syscalls in a raw pool need bounded timeout draws.

## Session 11 (2026-10-27 17:57-...) — cold restart: fleet on main 488528dc, deep-park CLOSED (blocking-call), ec68f275 gates, heap-exhaustion catch

### Restart and image decision (rebuild-vs-restore)

- The 13:48 quota kill left the staged fleet images uncommitted at the
  0cd29a75-era build (a46464fb/654ca5df, md5-verified against the
  session-10 notes). Main had since gained D-6 slice 2 (VFS FileOps
  capability builder), route-4 getdents cache, and wait/misc deltas, so
  RESTORE was rejected: merged origin/main 488528dc as fe590453
  (branch kernel delta vs that main = ec68f275's syscall/memory.rs
  alone), dual-arch rebuilt rc=0, staged f75a24b2 (rv64) / 8383bb3e
  (x86), committed ba1bfbce with the ltp-a-r3 wedge tail kept and the
  retracted fsfuzz-r1 artifacts dropped. 150 s LTP smoke: 426 TPASS,
  0 panics.
- The fleet member processes survived the agent kill (setsid) and ran
  until ~14:49 (r85 for fsfuzz) — the 16:46 resume never needed to
  adopt, only relaunch. Wave 1 up: ltp-a r5+, dirstorm r22+,
  dirtyscan r7+, fsfuzz r85+, memstorm r5+, modchurn r4+ + sweeper.
- ltp-a r3/r4 CRITICALs adjudicated: known classes only (oom01
  10-12 s soft-lockup, LTP OOM output strings, park=1 noise). r3 tail
  re-confirms the session-9 open lead: file-lru[2] (inactive) is 93%
  referenced (40243/43394) at the OOM point — the Referenced-flag
  vmscan question stays open.

### Deep-park adjudication: CLOSED as blocking-call (no kernel defect)

- The gdbstub chase's answer was already on disk: repro2's dfx=taskdump
  on the frozen guest (line 631) shows
  `pid=1 state=INTERRUPTIBLE(sleep) comm=fsfuzz lastsys=22(0x0,0x1000)`
  — syscall 22 = epoll_pwait, a0=0 (epfd 0), a1=0x1000 (maxevents):
  the SAME valid-but-empty-epoll park signature as the root-caused
  probe defect (the fd-3 marker-dup variant still landed the marker
  channel within the fuzz pool's reach).
- Negative control: with the F_DUPFD-900 + clamped-timeout probe, the
  park never recurs — r8-r85 (~78 rounds pre-kill) plus r85-r99 on the
  488528dc kernel, every round a full 55-beat stream, rc=0. A kernel
  lost-wake on a bounded epoll_pwait would survive the probe fix; it
  does not. BLOCKING-CALL, probe-defect class, closed.

### NEW CATCH: kernel-heap exhaustion on EVERY fixed-probe fsfuzz round (r8-r108+)

- Not a one-off. repro3 (13:24, kernel a46464fb) first showed the
  chain; the live member then reproduced it EVERY round on the new
  kernel (f75a24b2): r106/r107 ALLOCTHROW size=65537 align=1,
  r108 size=2048, heap size=131072K free=0-64K, small-alloc live
  objects only ~10k (a few MB) => the 128MB kernel heap is consumed
  by RAW allocations; pid 1 SIGKILLed by the designed task-context
  defense; ~most rounds then halt quietly (qemu exits rc=0 — that is
  why rounds "ended" at 55 beats), occasionally a second ALLOCTHROW
  in timer-irq context panics (repro3, r108).
- HARNESS GAP FIXED (fleet-classify-round.sh): neither "Kernel panic -
  not syncing:" nor "PANIC: Allocation error" matched the CRITICAL
  patterns, so ~100 exhausting rounds r8-r107 read as WARN
  (FSFUZZ-ODD). Now ALLOCTHROW=n and both panic phrasings are
  CRITICAL; r106-r108 re-verify as CRITICAL.
- Mechanism (narrowing): the stream holds fds until EMFILE (the ODD
  storm, errno=24) — that caps pid 1 at ~1024 objects, which cannot
  alone fill 128MB. The failing allocation size=65537 align=1 is the
  read/write staging-buffer size class (io.rs "staging below costs an
  alloc + 2 copies per op") — lead hypothesis: a raw read/write/pipe
  transfer with a garbage user pointer EFAULTs mid-staging and the
  ~64KB staging buffer leaks per call; ~2000 such calls = 128MB.
  Candidate audit: kernel/src/syscall/io.rs staging paths + pipe
  copy-in/copy-out EFAULT returns. sys_pipe2's EMFILE paths were
  audited clean (Drop-safe, alloc_fd does not reserve slots).
- OPEN: memwatch-instrumented fsfuzz boot for the growth curve;
  staging-path leak audit; decide heap-exhaustion policy for irq
  context (panic-by-design vs degrade).
- heapwatch boot DONE (evidence/
  2026-10-27-s11-heapexhaustion-fsfuzz.txt): clone-gate ENOMEM storm
  from free~2.9MB, then ALLOCTHROW 65537 -> SIGKILL pid 1 -> second
  throw (8192) -> klogd (2688) -> down. Census at death: ~3194 LIVE
  raw allocations hold ~128MB (avg ~41KB each); small-alloc classes
  hold only a few MB. The staging buffer is the VICTIM not the leaker
  (RW_CHUNK staging is a properly-dropped local; 65537 is a len+1
  capacity from elsewhere — site not yet pinned; rawframes did not
  symbolize). NEXT: capture with dfx backtrace build; audit the
  41KB-class live set (fork children x stack+fdtable, held pipes'
  rings, length-derived buffers on raw-syscall paths).
- Related old-home reminder: monkey-b r20/r24/r26 (stale sweep file)
  show the >500MB HEAP-GROW + KERNPANIC class on the x86 monkeys —
  same exhaustion family at a different scale; resumes with pstore
  when the monkeys come back. Monkeys DEFERRED this session: host
  load 16-21 with 23-25 foreign QEMUs (all lanes gate-booting) —
  2x4G KVM members would breach the courtesy window; relaunch when
  load1 drops (the 10G floor was never threatened, avail 40G
  throughout).

### ec68f275 (madvise wrap -> EINVAL) merge-gate numbers

- Dual-arch build rc=0 (x86_64 + riscv64); make test 1118/1118 PASS;
  minimal-rootfs boot 0 panics (unit suite ran green as init, twice);
  150 s LTP smoke 426 TPASS / 0 panics.
- LTP madvise family vs the PRE-FIX fleet baseline (ltp-a r3/r4,
  57e44a90 kernel, same 1081-row list indices):
  madvise01 FAIL(2)->PASS, madvise02 FAIL(2)->FAIL(1) (remaining
  TFAILs are the advice rows Linux-LTP expects to fail), madvise06/08
  FAIL(2) unchanged (procfs gaps: drop_caches EACCES,
  coredump_filter ENOENT), msync01-03 PASS, msync04/madvise07/09/11/
  process_madvise01 TCONF (same as baseline). NO regressions; two
  improvements. Method note: /test/sweep reads /test/sweep.list
  (N|name|cat|cmd rows, indices preserved — pa48 convention), NOT
  /test/linux-ltp/chunk.txt (that is ltpscan's file; first attempt
  edited the wrong file and ran the full list).
- Tier 1 (sf2g1, 780 s, KVM, host at 25 QEMUs / load ~14-25): PASS
  panics=0, OOM=0, TERM-DESCHED=8, WAKE-REFUSED=32, clock_gettime
  4 us, nanosleep 191.6 ms, GRQ hold 6955 ns (4080-7320 band),
  small_alloc REST share 5.8% (<=10%). FAIL tick cpu1=99.45/s (the
  documented one-CPU marginal flake under foreign-QEMU load) and FAIL
  volume 82.1M vs 107.5M +-10% (load-gated row: pa41gate precedent
  is a flat -35% at 14-19 foreign QEMUs with per-op costs held; our
  per-op costs are all in band). Volume re-bank needs a load1<=9 /
  qemus<=8 window — owed when the host calms.
- Tier 2 (pbench, TCG, same loaded window): vs the pb1 anchor the
  first branch boot read p2_ctxsw +71%, p6_append +70%, p6_write_fsync
  +47%, p6_mkdrmdir +40% — but a pure-main 488528dc CONTROL boot under
  the same load also missed the anchor on the same rows, and the
  spread did not survive interleaving: a second pair (boots 3+4,
  adjacent in time) collapsed every pair-1 fs delta to noise
  (mkdrmdir +73.4% -> +1.4%, write_fsync +80.3% -> -1.1%, stat
  +42.0% -> -7.1%, append +32.0% -> +0.4%, ctxsw -12.6% -> -4.0%).
  Only p7_sleep1ms stays noisy (+119% on one boot) — pure host-jitter
  row. VERDICT: branch (main 488528dc + ec68f275) is performance-
  indistinguishable from pure main 488528dc within noise; ec68f275
  (madvise range validation only, no pbench row calls madvise) shows
  no measurable effect. Tier-2 PASS by paired A/B; anchor-relative
  FAILs are host-load inflation common to both kernels.

### Session-11 late adjudications

- modchurn r4 (OOPS=8) / r5 (OOPS=1): 10 s soft-lockup CPU#0 with
  TIMER-LATE-FIRE 26ms->400ms during the donor phase — fired exactly
  in the 18:00-19:10 host window at load 16-21 (all lanes
  gate-booting). Host-CPU starvation of a TCG vCPU, the documented
  flake class; r1-r3 clean at lower load. If a soft lockup recurs on
  modchurn at load1 < 10, treat as a kernel lead (rerun-in-isolation
  rule).
- fsfuzz r109+ rounds classify CRITICAL (ALLOCTHROW) under the fixed
  classifier — the member doubles as the standing repro for the heap
  exhaustion until that bug is fixed.

## Session 12 (2026-10-27 20:0x-23:1x) — succession restart; heap-exhaustion ROOT-CAUSED and FIXED (sys_pipe2 abort leak); PREEMPT-UNDERFLOW class adjudicated to the main delta

### Succession intake

- The 20:00 quota kill left 7 dirty files: TEMP-DIAG refcnt/dealloc
  counters in exit.rs/task.rs/sched.rs (reverted — the hunt was already
  concluded by the on-disk fwl boot logs), 4 stale wedge-log deletions
  (kept, housekeeping), test/forkwait_leak.c untracked (committed as
  the standing repro), and 5 fwl boot logs that carried the whole
  case. Nothing was lost mid-air; the predecessor's last boot
  (fwl-cnt, 3000 forks) was the decisive one.

### ROOT CAUSE of the s11 heap exhaustion (evidence/
2026-10-27-s12-pipe2-abort-leak-rootcause.txt)

- The 65537B site is the pipe ring (PipeBuffer::new: capacity+1).
  Rings are 128KB-aligned buddy blocks — each leaked ring pins 128KB,
  and the fsfuzz death census (785 live rings ~= 98.8MB) IS the
  exhausted 128MB heap.
- Mechanism: create_pipe() puts ALL Arc<Pipe> ownership into two raw
  private_data pointers, consumed only by the PIPE_OPS close op, which
  File::drop runs solely after close_fd committed (close_pending).
  sys_pipe2 allocated the pipe BEFORE any fd work; every error return
  before install (alloc_fd EMFILE x2 — the fsfuzz ODD storm, fdtable
  gone, install_fd failure) dropped the Arc<File>s without the op:
  both raw refs + ring leaked per call. Deterministic: fwl leg-2 shows
  live=100 a=100 f=0 on every pre-fix boot. Sibling creators were
  audited: epoll/signalfd/eventfd/timerfd reclaim on alloc_fd failure,
  unix_socketpair unwinds via close_file_fd — only sys_pipe2 had the
  gap (supersedes the s11 "audited clean" note, written before the
  ring site was isolated).
- Fix (de891155): abort_uninstalled_end()/abort_pipe_creation() in
  fs/pipe.rs run the close op on never-installed ends via Arc::get_mut;
  sys_pipe2 routes all four pre-install aborts through it; the
  install_fd(write) failure keeps its correct read-end unwind and
  aborts the write end. Data path and success path untouched.
- The Task(11064B)/SignalStruct(2088B) "live" fingerprint is a memwatch
  ACCOUNTING ARTIFACT, not a leak: packed sub-page blocks share
  PAGE_SITE marks, neighbor frees clear them, later frees drop
  attribution (frees counted 782 of 3000 while TSLOT-DEALLOC ran
  3000/3000, RLS-EXTRA-REF 0; page-aligned sites attribute exactly —
  stacks f=624 + 64 baseline = a=688 perfect arithmetic). forkwait_leak
  leg 1 stays as the standing task-leak watch.

### Fix verification (kernel 140acd4b; rebuilt same-source 914e2f4b after the A/B checkout dance)

- fwl-fix1: ring site live=0 a=100 in every MEMLIVE dump (was
  live=100 f=0), 3000 forks + 100 EMFILE pipes, 0 panics, FWL-PASS,
  clean powerdown.
- fsfuzz-fix1 (fixed probe, 1800s): 130816 beats, sustained EMFILE
  storms, ZERO ALLOCTHROW/panic — pre-fix f75a24b2 died EVERY round.
- Unit suite: 1125 PASS / 0 FAIL (rv-smoke boot of the same build);
  `make test` guest banner "ALL TESTS PASSED" (wrapper SIGTERMed in
  teardown post-banner, host load ~20).
- Regression boot: minimal rootfs to mrsh prompt, /proc mounted,
  0 panics. Dual-arch build rc=0 (rv64 140acd4b, x86 bzImage 16.2MB).

### NEW FINDING (not this branch's bug): PREEMPT-UNDERFLOW class from the origin/main delta

- ltp-fix2 (fixed kernel, sweepstart=1005) printed 329
  PREEMPT-UNDERFLOW (spinlock.rs:461 SpinlockGuard::drop, lock =
  PROCESS_TREE_LOCK, 2 distinct pids) around the waitid06/07 rows;
  fleet-kernel rounds r11/r12 (f75a24b2 = main 488528dc + madvise)
  show ZERO.
- A/B boot at exactly 51a7a6c7 (main delta, NO pipe fix): same class
  (320 prints), and the LTP failure set is byte-identical to the fixed
  kernel (25 files; TPASS 1131 vs 1127, TFAIL 81 vs 80). The class
  entered via the 51a7a6c7 merge of origin/main 979fde80 — kernel
  delta vs the fleet kernel: D-7 percpu/small_alloc rework (55d65114
  chain), virtio input IRQ delivery (6ae8c8fb), posix_mq deadline
  (7564083f), madvise wrap (70d614e7). Suspects by mechanism: the D-7
  per-CPU preempt-count carrier rework around PROCESS_TREE_LOCK
  guards in the ptrace-stop/continue path. Report handed to the
  coordinator/perf-audit lane; the print is clamp-to-0 (no observed
  corruption), and 19 of the 25 failing rows are standing failures on
  the fleet kernel too; the other 6 (setpgid03, setxattr01-03,
  tkill02, truncate03) were mid-run in r12 at session close — both my
  kernels fail them identically, so they ride with the main delta
  either way.
- FOR THE COORDINATOR: main 979fde80 (488528dc -> 979fde80) carries a
  preempt-count-underflow producer visible on the LTP waitid/ptrace
  rows. Not blocking this merge (identical with/without the pipe fix;
  clamp-to-0 diagnostic), but it should not ship to the fleet baseline
  untriaged.

### Gate numbers (this branch = 51a7a6c7 + de891155)

- Tier 1 (pa-boot sf2g2, KVM, 780s, host load ~17-20): PASS panics=0,
  OOM=0, TERM-DESCHED=8 (band 6-10), WAKE-REFUSED=32 (28-36),
  clock_gettime 4us, nanosleep 185.5ms, GRQ hold 6586ns (4080-7320),
  small_alloc REST 1.1% (<=10%). FAIL tick cpu0=97.07 (documented
  one-CPU marginal class under foreign-QEMU load), FAIL volume
  85.99M vs 107.5M +-10% (load-gated row; pa41gate precedent, per-op
  costs all held). Volume re-bank still owed a load1<=9/qemus<=8
  window (carried over from sf2g1).
- Tier 2 (pbench, TCG, interleaved A/B x2 vs the 51a7a6c7 control —
  the control differs from the fixed kernel ONLY by this change):
  p5a_pipe (the touched family) +2.3%/-0.9% — statistically
  identical. Every first-boot anchor miss collapses or reverses in
  pair 2 (p6_append +22.3% -> -2.0%, p6_openclose -29.3% -> -2.8%,
  p6_stat -28.2% -> -6.4%, p7_sleep1ms +67.9% -> -11.6% — the known
  jitter row). p2_ctxsw (+13.0/+12.2%) and p1_clock (+12.8/+38.8%)
  stayed positive across pairs; neither executes any code from this
  change (error-returns only), and a concurrent foreign pbench
  (pa72f) ran inside pair 2 — adjudicated host noise per the
  session-11 interleaving method. VERDICT: Tier-2 PASS (A/B).
- LTP (sweepstart=1005 tail rows, x2 rounds): failure set IDENTICAL
  with and without the fix (25 files), zero panics, heap-free ~105MB
  throughout (the 15 ALLOCTHROW lines per round are the standing
  garbage-size ENOMEM class; pre-fix rounds showed 21 — same class).

### Fleet posture

- Members continue on f75a24b2 through this session (the standing
  repro duty ended with the fix). Working-tree fleet-rv64-rux updated
  to the fixed kernel (914e2f4b-class build, same source as de891155)
  AFTER its gates passed; NOT committed (session rule: no big
  binaries) — next sessions' members boot the fix from the working
  tree; the coordinator's staged-image commit can pick it up at merge
  time. fsfuzz member becomes the leak-fix watch (expect CRITICAL
  count to drop to zero).
- Monkeys stay deferred (host load 17-21 all session; floor never
  threatened, avail >=40G). The vmscan Referenced-flag question
  (file-lru 93% referenced at OOM) stays queued from s11.

## Session 13 (2026-10-28 01:31-...) — roll onto main 0f806d8b; vmscan Referenced question ADJUDICATED (code-level); perf_event_open fuzz arm

### Succession intake

- Predecessor's fleet-onto-main leg died minutes in (quota window):
  the merge itself landed (6bb7f396 = origin/main 0f806d8b + docs/test
  deltas; kernel/ is a ZERO delta vs main — branch kernel == pure main
  0f806d8b), but the image roll, member relaunch, and ledger block did
  not. Fleet was COLD (no drivers, no sweeper, no stop files).
- Dual-arch build of the merged tree: rv64 md5 527656b294d44b8e39c0ecb
  283abd18b, x86 bzImage md5 29be66a7ef51b4f6f6aa3b540cadffc1 (staged
  working-tree, never committed — big-binary rule).
- QEMU hold honored 01:31→(chain exit or ~03:43): the perf lane's
  detached gate chain (perf-gate-leg36b-chain.sh) needs total-QEMU <=1
  windows. ALL launches (smokes included) deferred; prep done first.

### THE vmscan file-lru 93%-referenced-at-OOM question — code-level adjudication

The queued candidates were (a) mark_page_accessed setting Referenced
without activation, (b) the scan never clearing Referenced on the
inactive list, (c) promotion never firing. Code reading of the current
engine (mm/lru.rs, mm/vmscan.rs, fs/page_cache.rs, mm/page_alloc.rs):

- (b) is FALSE: the eviction walk's second-chance arm
  (page_cache.rs:1210-1215) clears Referenced and ACTIVATES the page
  onto the young end of LRU_ACTIVE_FILE; demotion
  (lru_demote_active_file_oldest, page_cache.rs:1232) also clears it.
  The flag does cycle.
- (a) is TRUE and is the EXPLANATION of the census number — by design.
  All four setters are access paths (get/get_phys/get-range-hits/
  pin_by_key): every cache HIT sets PG_referenced in place with one
  bit op, no LRU move, no activation. (No insertion path sets it.)
  Linux's mark_page_accessed activates an inactive page on its SECOND
  hit; Rux defers graduation entirely to the reclaim walk. Under any
  working-set sweep (LTP re-exec'ing the same binaries — every exec
  maps the same pages), the inactive list accumulates the bit on
  ~every page between walk sweeps → 88-93% at any instant census.
  This is the expected state of the policy, not a leaked bit. The
  hit-path avoidance of lru_lock is a deliberate divergence (record in
  docs/architecture/design-divergences.md if kept).
- THE REAL DEFECT the census points at is reclaim DEPTH before OOM,
  not Referenced semantics. Quantified chain:
  - evict_one scans at most MAX_SCAN=64 pages per call from the tail;
    with ~93% referenced, the budget is spent clearing+activating
    (graduating) pages before the first evictable page is found. When
    the oldest 128 (2 rounds incl. the demotion retry) are all
    referenced, evict_one returns FALSE with ZERO pages freed.
  - shrink() breaks on the first evict_one failure
    (page_cache.rs:990-992).
  - balance_pgdat stops as soon as 32 pages (SWAP_CLUSTER_MAX) are
    reclaimed per call — even at priority 12, the shallowest scan
    (size>>10 = ~42 pages of a 43k-page list).
  - The alloc slowpath runs <=8 try_to_free_pages rounds, breaking on
    freed==0 (page_alloc.rs:126-135).
  - NET: worst case ~8x32 = 256 pages (~1 MB) of reclaim effort
    precede the OOM decision — while the s8/s11 censuses measured
    175-204 MB of clean, unlocked, unevictable-flag-free file cache
    sitting on the LRUs one list away. The kill fires because the
    pre-OOM protocol is latency-bounded at ~1 MB of drain against
    ~200 MB of drainable pages — two orders of magnitude short.
- FIX DIRECTION (for the reclaim owner, not landed here): deepen the
  LAST pre-OOM round only — e.g. a final "drain clean file cache"
  pass with nr_to_reclaim proportional to the shortfall (or simply
  keep descending priority while ANY progress is made) instead of the
  flat 32-page stop. Alternatives: adaptive MAX_SCAN under pressure,
  or Linux-parity activate-on-second-hit (moves graduation cost to the
  hit path — the divergence-record trade). The r51 forensics (anon
  1613MB of 2G at kill) mean those OOMs are borderline-correct kills
  that should have been SLOW ALLOCATIONS instead.

### perf_event_open fuzz surface (mission watch)

- fsfuzz's raw pool drew syscall numbers 1..60 only — perf_event_open
  (nr 241, asm-generic; x86 298 maps to it) was NEVER touched. The
  fresh D-7-set implementation (kernel/src/perf/mod.rs) validates
  defensively (access_ok + size 64..1024, type/config/freq/sample_type
  gates, flag-mask EINVAL) — now it gets exercised.
- test/fsfuzz.c seq_perf arm (commit cf6cf99d): per call 3 chaos draws
  (garbage 128-byte attr blob with targeted size/type/config/period/
  flags pools, occasional garbage attr pointer for the EFAULT class)
  + 1 valid-shape SOFTWARE cpu/task-clock open so the lifecycle (ARMED
  up, read() record drain, close, ARMED down) churns; FSFUZZ-PERF
  census line every 256 opens; FSFUZZ-ODD perf on unexpected errno.
  Rootfs staged with the new probe (private image, debugfs injection).
- Classifier: PREEMPT-UNDERFLOW counter added (WARN at >=5/round) for
  the session-12 sighting watch; the print row carries pid= count_was=
  sub= at file:line lock=0x — keep tails on any WARN round.

### The roll (02:06-02:24, after the gate chain got its window)

- Gate chain (perf lane's perf-gate-leg36b-chain.sh) got its 60s quiet
  window 01:46:50, ran tiers 1+2, exited rc=0 at 02:05:31 — hold
  released early. 150s LTP smoke on 527656b2: 322 TPASS / 0 panics.
- Images staged (working tree, never committed): fleet-rv64-rux :=
  527656b2..., fleet-x86-bzImage := 29be66a7... Members relaunched:
  fsfuzz r263, ltp-a r16, memstorm r15, dirstorm r32, dirtyscan r17,
  modchurn r14 + sweeper; monkey-a/b r1 (KVM 4G, first monkey rounds
  in this home — host headroom allowed: load 13 at launch, avail
  38-42G, free 3-4G under the s3 page-cache reading all session).
- Pre-kill round adjudications (predecessor's last, old 140acd4b-class
  image): ltp-a r14/r15 CRITICAL = known classes (oom01 OOM at 837
  with kill+SIGDEATH delivered, W3 no-growth wedge after, standing
  ALLOCTHROW garbage-size ENOMEM 21-24/round, 10s soft-lockup
  residual; r15 wedgetail re-confirms file-lru[2] 85.7% referenced
  34867/40664, active referenced=0, dirty=0 — the vmscan census shape
  on the true-LRU engine). modchurn r13 OOPS=9 = soft lockup 109s
  CPU#1, the documented host-starvation flake class under the pre-kill
  load window (s11 precedent). dirstorm r31 PARKED-LEAK-RV=5 /
  dirtyscan r16 PARKED-LEAK-RV=2: non-OH parked-leak prints on the
  D-7-delta kernel — likely the count-migration family's parked face;
  watch continues on the new kernel. fsfuzz r254-r261 exhaustion
  deaths were pre-fix-kernel rounds (r262 on 140acd4b: 180 beats,
  ZERO ALLOCTHROW — the pipe fix holds); two `klogd: flush wrote 0/
  512 (ext4 write error)` lines during the r15 OOM crunch — buffered,
  bounded, watch for recurrence.

### NEW FINDING: perf ABI divergences (kernel-side, for the perf lane)

1. PERF_FLAG_* encoding: kernel/perf/mod.rs has FD_NO_GROUP=1<<1,
   PID_CGROUP=1<<4; the Linux uapi defines FD_NO_GROUP=1<<0,
   FD_OUTPUT=1<<1, PID_CGROUP=1<<2 (FD_CLOEXEC=1<<3 matches).
   Consequence: userspace passing Linux's FD_NO_GROUP(1) gets spurious
   EINVAL; Linux's FD_OUTPUT(2) is silently accepted as "no group".
   No corruption (neither feature implemented), error contract only.
2. read(perf_fd) BLOCKS until a record arrives (perf_read waits on the
   event's wait queue). Linux's read(perf_fd) returns the 8-byte
   counter value and never blocks (records go through mmap+poll). A
   task-scoped cpu/task-clock event read from the SAME task can never
   wake: the reader sleeps, its clock stops, no record is generated —
   deterministic self-deadlock for any same-task profiling reader
   (my first fuzz arm parked exactly this way; kernel exited the park
   only via the wedge kill). RECOMMEND: counter-value read semantics
   (Linux parity) or at minimum make read() honor O_NONBLOCK (it does
   — but callers cannot know blocking is the default divergence).

### fsfuzz perf arm bring-up (3 probe revisions, all probe-side)

- v1 parked (task-scoped blocking read, see above) — r263/r264 WEDGE
  215 lines/1 beat, the exact park signature. v2 fixed the park but
  crashed pid 1 with a STACK CANARY failure: perf read() drains up to
  COUNT bytes of records into the user buffer (unlike file/pipe reads
  which the kernel clamps by data availability), so a fuzz-length draw
  (1MB) with the 512-byte stack buffer smashed the canary (r265/r266
  rc=0 exits; symbolized to __stack_chk_fail_local in main). Both are
  probe-contract violations, not kernel defects — on Linux the same
  calls behave the same way.
- v3 (228f741d): O_NONBLOCK via F_SETFL, task-scoped fds never read,
  every perf read clamped to the buffer. Smoke: 172 beats/150s,
  FSFUZZ-PERF ok=1024 (the valid-shape open+read+close lifecycle runs
  ~7x/s), 0 segv/panic. FSFUZZ-ODD perf errno=24 = the EMFILE storm
  class (fdtable full mid-storm), known noise.
- Kernel-side validation of the chaos surface so far: 0 panics, 0
  ALLOCTHROW, no NEW class through ~30 min of full-rate fuzzing.

### PREEMPT-UNDERFLOW sightings (watch item, new kernel 0f806d8b)

- memstorm r15 (527656b2): 7 rows, ALL pid=1, sub=1,
  at kernel/src/sync/rwlock.rs:457 (bare preempt_enable from an
  rwlock guard drop — a DIFFERENT site than the s12 LTP sighting at
  spinlock.rs:461/PROCESS_TREE_LOCK). memstorm r14 (pre-merge-class
  kernel, same D-7 delta): 13 rows same shape. Class persists on
  0f806d8b at low volume on the fork/mmap-storm surface; perf lane
  owns the producer triage.

### PRODUCER NAMED (code-level, s13): the mutable current-slot — bookings split across two tasks

The perf lane's leg-47 refutation (pre-delta kernel shows the class
at the same rate; the s12 attribution withdrawn) handed the producer
hunt here with the pa73c leak-origin fingerprint (pid 593 stuck
RUNNING at pc=0x10000, heal/underflow 64/64). Code-level analysis:

- The stuck bit in pa73c is the HARDIRQ bit (offsets: PREEMPT=1,
  SOFTIRQ=0x100, HARDIRQ=0x10000) — the reported "acquire sites"
  (sched.rs do_wait/grq guards, wait.rs finish_wait, spinlock.rs) are
  the LEAKORIGIN shadow's last PREEMPT_OFFSET acquire = noise for a
  hardirq leak; they only name where the victim was spinning.
- irq_enter() is called ONLY from the three trap brackets, all
  instrumented with tp_entry != tp_exit discriminators — and pa73c
  shows ZERO bracket prints, ZERO SWITCH-COUNT-LEAK (the
  context_switch discipline-bypass tripwire), while bits still
  migrate. tp never changes during the leak.
- THE GAP: preempt_count_add/sub resolve through
  sched::current() = cpu_state[cpu].current — the MUTABLE SLOT —
  while every discriminator watches the tp register. The slot has
  writers that run while the hardware keeps executing a different
  task: (a) the wake-path orphan heal rewrites a REMOTE CPU's slot to
  idle (sched.rs curr_on loop, "heal the lying slots"); (b)
  republish_current_after_abort (arch switch-abort repair) — its own
  comment: "the task actually continuing here is prev"; (c) the
  slot-capture steal / harvest_relink machinery the R56 family
  documents. A bracket (or guard scope) opened before such a rewrite
  and closed after it books +offset on one task and -offset on the
  slot task — stranding one (pa73c's 0x10000-on-593; dirstorm's
  parked pc=1 workers) and underflowing the other (idle 0x5fa in s3;
  memstorm's pid-1 sub=1 at rwlock preempt_enable). One mechanism,
  both bit classes, both faces; tp unchanged explains every silent
  discriminator. __schedule's own R85 block ("tp != prev ... booking
  the physical truth: prev = tp") proves slot-vs-tp divergence is a
  recurring live event, not theory.
- INSTRUMENT LANDED (commit on this branch): all three trap brackets
  now compare the SLOT across the bracket (cs_entry/cs_exit pointers
  + pids in the print; when=slot = tp-unchanged-slot-changed = the
  producer signature). Canary kernel e80e92f0 rolled to a NEW ltp-b
  member (ltp-a stays the clean-kernel control); smoke 406 TPASS /
  0 panics. Watch: grep 'IRQ-BRACKET-LEAK.*when=slot' in ltp-b
  serials — a hit directly witnesses a slot rewrite inside a live
  bracket.
- FIX DIRECTION (for the sched owner, once captured): bookings should
  resolve against the hardware truth (tp) — or the slot rewrites must
  be bracketed so no add/sub can straddle them. Not changed here:
  semantics touch on the switch machinery needs the capture first.

### Monkey verdict (s8/s11 queue: KILLDUMP at >500MB HEAP-GROW)

- monkey-a r1 (29be66a7 x86): HEAP-GROW 619684KiB (605MB, past the
  historical >500MB panic trigger), heap 708MB/1GB, 1548 KILLDUMP
  census lines INCLUDING the exact old death-site shape (tgid 2074
  m.ohos.launcher fdtab walk) — and ZERO KERNPANIC; 795 beats, normal
  timeout. The fb643799-class census fix HOLDS under the heap-pressure
  trigger on this kernel. monkey-b r1: 188MB growth, clean (WARN for
  growth alone). Formal "both monkeys >500MB panic-free" still open
  (monkey-b has not crossed 500MB yet); monkeys keep accumulating.
  The heap GROWTH itself (OH image-side) remains its own standing
  item. OOPS=1 = the known 10s soft-lockup residual.

### Fleet map at session-13 close (03:1x)

- Default rv64 image = 527656b2 (main 0f806d8b): ltp-a (r17+),
  memstorm (r15+), fsfuzz (r268+ on probe v3), dirstorm (r32+),
  dirtyscan (r17+), modchurn (r14+). x86 = 29be66a7: monkey-a/b (r1+).
- ltp-b: NEW instrument canary on e80e92f0 (0f806d8b + the slot
  bracket discriminator), KIMG-pinned.
- Sweeper live. Host floor: free 3-10G / avail 33-42G under the s3
  page-cache reading all session; load peaked ~20 during member boot
  transients.
- Housekeeping: the 6bb7f396 merge had accidentally committed
  fleet-x86-bzImage (16MB); untracked (kept in working tree, 29be66a7)
  — the coordinator may want to squash that out of the merge at
  main-merge time.
- Chains (ww-s2/wwload): down since the s10 home move, not restarted
  (no mission item; old-home artifacts preserved untouched).

### NEW CATCH (kernel, main 0f806d8b): perf REGISTRY multi-CPU spin stall — allocations under spinlocks in softirq context

fsfuzz r268 — the FIRST FULL ROUND of the perf_event_open fuzz arm —
wedged at ~beat 43776 (~12 min in, right after FSFUZZ-PERF ok=4608):

    DEADLOCK: spinlock stuck cpu=1 lock=0xffffffff808808b8 (= rux::perf::REGISTRY) x67
    DEADLOCK: spinlock stuck cpu=0 lock=0xffffffd603f15218 (heap-range dynamic lock) x67
    ra=RawSpinlock::lock on both; 134 watchdog prints until timeout;
    beat stream stopped cold (serial "growth" = the watchdog printer).

Taskdump at the wedge: pid 1 (fuzzer) preempt=259 (SOFTIRQ|PREEMPT)
and the cpu1 idle task preempt=257 — BOTH CPUs inside timer-softirq
processing (softirq_tick runs on whatever task the tick interrupted).

Code defect (kernel/src/perf/mod.rs): softirq_tick holds
REGISTRY.lock_irqsave() across the whole event iteration and inside
that section (a) takes ev.inner.lock(), (b) calls to_wake.push() — a
VEC ALLOCATION UNDER REGISTRY; push_sample grows the ring VecDeque
inside ev.inner — ALLOCATION UNDER inner; register()'s reg.push()
allocates under REGISTRY on the open path. Under the fuzzer's
standing heap-pressure class (the ENOMEM/EMFILE storms; the 128MB
kernel heap crawls), every under-lock allocation enters the allocator
slowpath while the other CPU spins on REGISTRY/inner — a
watchdog-tripping multi-CPU spin stall, 67/67 alternation.

Fix direction (perf owner): no allocation inside any spinlock section
reached from softirq/irq context — to_wake as a fixed array (events
are few), ring capacity preallocated at open (RING_CAP becomes the
reserved capacity so push_sample never grows), registry pre-reserved
or a fixed table. Evidence:
run/fleet/evidence/2026-10-28-s13-perf-registry-deadlock-{rootnotes.txt,
fsfuzz-r268-serial.log}. The member doubles as the standing repro
(classify WATCHDOG= + lock=0xffffffff808808b8).

### Next-session queue

0. COORDINATOR DIRECTIVES (s13 close-out): (a) when the ltp-b canary
   captures when=slot, the fix (bookings resolve against tp, or
   bracket the slot rewrites) goes through the FULL sched-domain merge
   gates — this closes the longest-standing wedge family in the fleet
   ledger; (b) the perf REGISTRY spin-stall + the two perf ABI defects
   are routed to the DFX lane as MERGE-BLOCKING for the perf surface —
   keep the fsfuzz perf arm hot until their fixes land, then ONE clean
   round on the fixed kernel closes the catch.

1. ltp-b instrument: grep 'when=slot' — a capture names the producer
   directly; cross-read the harvest/abort/heal state around the
   timestamp. Zero prints over several rounds = the slot-rewrite
   window is NOT the producer; next candidate is a per-CPU count
   mirror divergence (extend leakorigin to shadow HARDIRQ acquires).
2. PREEMPT-UNDERFLOW volumes on ltp-a (clean kernel) vs ltp-b
   (instrument) — rate comparison; the memstorm rwlock face and
   dirstorm parked face continue as wide capture.
3. fsfuzz perf arm: the REGISTRY spin-stall catch goes to the perf
   owner (fix: no allocation under REGISTRY/inner in softirq paths);
   after the fix, resume first-full-day accumulation (PERF census,
   chaos classes); perf ABI divergences await the perf lane's call
   (flag-bit encoding + blocking read).
4. Monkeys: monkey-b past 500MB growth panic-free closes the formal
   criterion; heap-growth curve continues.
5. modchurn/dirstorm soft-lockup + PARKED-LEAK recurrence watch at
   load1 < 10 (kernel-lead test).

## Session 14 (2026-10-28 03:1x-...) — s13 producer theory REJECTED at code level; canary re-instrumented at booking time; msgstress01 strand burst dissected

Intake: floor at 6G free (15 QEMUs total) — shed by own PIDs per the
≥10G hard rule: both monkeys (4G x86 KVM each; monkey-a criterion
already closed at r1 605MB, monkey-b r2 partial ~294MB@25min captured
in-serial before the kill, zero panics), later ltp-a (r19 next) and
modchurn (r17 next) as other lanes' boxes re-grew. All member data
preserved; resume = relaunch fleet-member.sh with the next round
number.

### Harvest: ltp-b r1 (canary e80e92f0, base main 0f806d8b)

ONE WAKE-STRANDED burst, 390 lines at msgstress01 (sweep 668), plus
the first-ever PARKED-WILDWRITE firing. Adjudications:

1. **s13 slot-rewrite producer theory: REJECTED, code-level.** Zero
   IRQ-BRACKET-LEAK prints of ANY kind (slot/pre/in) during the burst.
   The null is structural, not luck: preempt bookings resolve through
   sched::current() which reads the ARCH tp REGISTER (sched.rs:5237),
   NOT cpu_state.current. The slot discriminators watched a path
   bookings never touch; no slot rewrite can split a bracket's
   +offset/-offset. Brackets stay as standing negatives.
2. **PARKED-WILDWRITE = probe false positive.** The paired PARKED-LEAK
   line shows state=0x0 (RUNNING): a task preempted mid-switch-out
   holds the GRQ guard while already slotless; the parkwalk's running
   classifier (slots only, predates R63's SWITCHING_OUT marker)
   misread it. Fix in 766d60a9 (also stops the 4s PARKED-HEAL from
   zeroing a live guard mid-switch-out — which would re-open
   preemption inside the spinlock section).
3. **The h-field pc census needed picker separation.** check_picked
   prints under the GRQ: the picking CPU's own incumbent always shows
   pc=1 LEGALLY. Splitting picker/other (script in evidence dir):
   other-CPU faces r1 = 52% pc=0, 27% pc=1, 13% pc=65537 — and r2
   (fresh kernel) shows pc=1/256/65536 incumbents some of which are
   LEGAL transient states (spinlock-held / softirq / mid-IRQ sampled
   at print time). The h-fields alone cannot convict an incumbent.
4. **Half the strands have a CLEAN other-CPU incumbent** rotating
   every 0-5ms — that half is NOT preempt-count leakage: it is the
   pick walk skipping a leftmost runnable victim (every strand line
   carries vr=0ms lead-lag=0ms; saturating_sub also reads 0 when the
   victim's vruntime is BELOW min_vruntime — a sleeper massively owed
   CPU). That is the R85 PICK-SKIP shape (why=1/2 legs), a SECOND
   family tangled with the count leaks.
5. Zero LEAK-ORIGIN / PARKED-LEAK / PREEMPT-HEAL / IRQ-PAIR prints
   through r2 partial (101 strands): leaked-count incumbents churn
   faster than every tripwire window (2s PREEMPT-LEAK, 4s parkwalk,
   10s tick-heal reset on each incumbent change) — the windows, not
   the instruments, are the gap.

### s14 instruments (766d60a9) + canary v2 (72e03151)

- leakorigin HARDIRQ shadow (hsite= names the trap bracket that booked
  a later-stranded 0x10000; irq_enter now #[track_caller]);
- IRQ-PAIR booking check: per-CPU token of the task irq_enter booked
  +0x10000 on (riscv S-mode hardirqs cannot nest per-cpu → token
  strictly outermost); irq_exit verifies the paired sub lands on the
  same task — split = tp changed inside the bracket, measured at
  booking time, no trap-boundary sampling hole;
- parkwalk mid-switch-out adjudication (SWITCHING_OUT);
- harness 42e6fe52: START/DFX env overrides on the ltp member kind
  (installed via atomic rename; running loops keep their old inode).

Canary round live: fleet-rv64-instr-rux md5 72e03151f484d99de14162a8b7dad1cc,
sweepstart=660 (replays the msg region), dfx=watchdog,taskdump,
leakhunt,wwcensus,schedtrace (schedtrace arms the PICK-SKIP-ALL why=
reports — the discriminator for adjudication 4; wwcensus arms the
PREEMPT-LEAK incumbent site reports).

Evidence: run/fleet/evidence/2026-10-28-s14-ltpb-r1-msgstress-strand/
(analysis.txt, burst excerpt, r2 partial without schedtrace).

### Cross-lane notes

- origin/main 0f806d8b..584c1105 carries NO perf fix yet (fsfuzz perf
  arm stays hot; catch closure queued behind the DFX lane's merge).
- main 584c1105 (leakhunt v3) adjudicates the parkwalk against "arch
  tp slots" — on riscv64 that helper reads cpu_state.current (tp is
  register-only), i.e. the same slots the walk scanned: x86-effective,
  riscv-null. Our SWITCHING_OUT extension is the riscv64 complement;
  edits do not overlap (they touch report_parked_site + wakewatch, we
  touched the shadow table, preempt.rs, the walk classifier) — merge
  is clean.

### Next-session queue (s14)

0. Harvest the live canary: PICK-SKIP-ALL why= census against the
   strand victims (family A verdict), LEAK-ORIGIN/IRQ-PAIR-SPLIT
   (family B verdict). If why=1/why=2 dominates with slot=0 swout=0,
   the R85 pick-side heal windows (50 jiffies) are too slow under
   msgstress churn — that tuning is the fix candidate, sched-domain
   gates apply.
1. ltp-a (r19+) and modchurn (r17+) resume when the floor relaxes;
   monkeys resume for monkey-b's formal 500MB criterion (r2 partial:
   294MB@25min panic-free, on pace).
2. perf catch closure: ONE clean fsfuzz round on the fixed kernel when
   the DFX lane's perf fixes land on main.
3. vmscan file-lru question stays queued (s13 item 4).
