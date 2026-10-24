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

## Classification state

See "Classification state (updated ...)" below — the single source of
truth is the latest-dated block; older adjudications live in the round
log.

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
- QEMU zcb flakiness (withdrawn): illegal-instruction on legal
  instructions, QEMU 10.2.1 TCG + `-cpu ...,zcb=true`. Do not re-add.
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
