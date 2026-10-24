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
