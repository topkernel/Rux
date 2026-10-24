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

## Classification state (as of 2026-10-25 05:15)

KNOWN classes (do not page):
- Image-side OH noise: usb_host DDK crash lines, SAMgr 15/30/89s cost
  family, RS userspace read-lock parks (excluded per hunt ledger).
- IRQ-OFF-WINDOW known-noise: 103-771ms class at rip ffffffff80573d96
  (hunt ledger canonical family). Classifier WARNs only > 2000ms.
- PREEMPT-LEAK self-observation: the LEAK tripwire in scheduler_tick ran
  FROM the timer hardirq, so the observed task always carried the tick's
  own HARDIRQ_OFFSET — pc==0x10000 lines (pid 0 in wfi, or any task
  current across the window) were probe self-observation, 170-620
  lines/round. FIXED 2026-10-25 (commit follows): print gate now requires
  non-self bits (base != 0 or hardirq nest > 1). Any PREEMPT-LEAK line
  after the fix is a real held-count candidate.

OPEN items (tracked, need targeted repro):
- PARKED-LEAK on OH boots: fires on ~47% of OH boots across lanes
  (107/230 samgr-callback serials, ~30 lines/boot, pc=1 exactly,
  state=0/1, tcpu=1 mostly). Never seen on riscv64 members. Probe
  invariant dates from the no-kernel-preemption era ("a parked task can
  never legally hold a nonzero count") — with IRQ-exit kernel preemption
  a legal producer may exist (preempted-with-guard via explicit
  schedule()). Classifier: park=N in context; WARN only on non-OH members.
- memstorm pid=1 tick-monopoly: 3 hits r1, pid 1 current on cpu1 across
  218-820 consecutive ticks with pc=0x10001 (real PREEMPT_DISABLE bit at
  the printing tick), rip=enable_irq (cpu.rs:63 — syscall-entry path or
  schedule_dead park). Needs repro with memwatch + taskdump to separate
  "compute monopoly while preemptible" (benign) from "held guard while
  monopolizing" (real). PREEMPT-HEAL did NOT fire (no 10s+ base holds).

## Round log

- 2026-10-25 04:51/05:06 — fleet launched (8 members; first batch 04:51
  after fixing a boot_cmd eval quoting loss that qemu-misread `rw` as a
  disk; 12 garbage no-serial rounds rotated to *.bootfail-quoting).
  Round-1 serials in progress; first classifications due ~05:21/05:36.
- 2026-10-25 04:48 — chains self-advanced r88→r89 / load-r35→r36, both
  alive, capture continuity preserved (frozen kernel + GUARD armed).
- 2026-10-25 ~05:12 — probe-gated kernel (7fcff66a md5) smoke-booted
  (fsfuzz init, 0 PREEMPT-LEAK in 100s where old kernel printed
  dozens) and rolled into images/fleet-rv64-rux atomically; picked up
  by members from their next round.

## Resume (next session)

1. `./run/fleet/stress-fleet.sh status` — one sweep (members, chains,
   flagged rounds).
2. Chains: if DEAD, see hunt ledger resume notes; do NOT restart live
   ones. Members auto-continue round numbering via rounds.log lines.
3. Relaunch dead members with `stress-fleet.sh start <member>`.
4. Monkey beats: if serials show churn noise but no MONKEY-BEAT lines,
   apply /tmp fix — script echo goes to hilog, not console; v2 writes
   beats to /dev/console (apply at a round boundary via debugfs, image
   is snapshot-booted so base edits are safe between rounds only).
