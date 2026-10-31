# perf-ipc lane findings — 2026-10-31

Worktree: /home/william/rux-agents/perf-ipc (branch perf-ipc, off
origin/main 1e2bef44). Rig: pbench_boot.py battery, q35, TCG
thread=multi, smp2/2G, -cpu max — the SAME battery booted on the
reference bzImage (/home/william/workspace/oh-robot/out/x86_64_virt/
packages/phone/images/bzImage) for the true A/B column.

## The headline numbers (medians, us)

strong profile (release-opt LTO), baseline = origin/main same rig:

| row            | base  | fixed | gain   | reference | vs ref   |
|----------------|-------|-------|--------|-----------|----------|
| p5b_dgram_send | 39.80 | 1.94  | 20.5x  | 2.39      | AHEAD    |
| p5b_recv_peek  | 39.78 | 3.82  | 10.4x  | 2.66      | 1.44x    |
| p5c_stream     | 35.50 | 2.01  | 17.6x  | 2.50      | AHEAD    |
| p5c2_send      | 35.45 | 1.96  | 18.1x  | 1.91      | 1.03x    |
| p5c3_wrecv     | 2.50  | 1.94  | 1.3x   | 1.91      | 1.02x    |
| p5f_dwait      | 35.35 | 1.94  | 18.2x  | 1.91      | 1.02x    |
| p5e            | 2.35  | 1.86  | 1.3x   | 1.90      | AHEAD    |
| p5d_rt_read    | 4.17  | 3.84  | 1.09x  | 4.31      | AHEAD    |
| p5d_rt_recv    | 4.68  | 4.10  | 1.14x  | 3.78      | 1.08x    |
| p5a_pipe       | 2.08  | 2.28  | noise  | 2.88      | AHEAD    |
| p2_ctxsw       | 1.13  | 1.23  | noise  | 1.87      | AHEAD    |

debug profile (same rig, one boot each end): p5b 321.8 -> 23.8 (13.5x),
p5c 276.7 -> 27.7 (10.0x), p5f 276.4 -> 25.5 (10.8x), full battery
DONE rc=0 both profiles, zero panics.

The charter target ("10x-class on the lagging metrics") is met on the
p5b/p5c/p5f family in BOTH profiles; the reference kernel column is now
matched or beaten on the unix-socket family in the strong profile.

## Root cause chain (evidence-backed)

1. WHY the socket rows were 35-40 us: the pbench p2 section pins pid 1
   to cpu0 and never restores the mask, so every later fork inherits
   cpus_allowed={0}; the p5b/p5c sender+receiver pairs run pinned to
   ONE cpu while cpu1 idles (ipcdisc3 proved no other precursor — the
   same shapes unpinned were fast on both kernels).

2. On that same-cpu pair, every send's wake_up_process resched'd the
   wakee's previous cpu (= the sender's cpu, busy) UNCONDITIONALLY:
   the sender preempted at trap-exit per record, the receiver drained
   one 74-byte record and re-slept — a 275 us/record dance (ipcdisc2
   sched trace: wake 19us + preempt-switch 56us + receiver cycle 131us
   + sender cycle 86us). The reference kernel batches the receiver and
   sustains 2.4 us/record on the same pinned shape.

3. Why a granularity-gated preempt could not fix it: the vruntime
   credit is structurally skewed in a ping-pong pair. Per-switch charge
   probe (CHG): the SENDER takes +10 ms vruntime jumps at requeue
   clamps (enqueue floor = min_vruntime − SCHED_LATENCY_NS) while the
   receiver gets sleeper-credit drops; the receiver sits a few ms of
   vruntime "behind" the sender forever, so check_cfs_preempt's 700 us
   gate passed on EVERY wake (WPRE probe: delta steady at 2-6 ms).

## Fixes (commits on perf-ipc)

- 977adb51 sched: (a) wakes dispatch to an IDLE cpu within the wakee's
  affinity when one exists (claim_idle_cpu_for; the GRQ keeps the IPI
  advisory — blocker handoffs still happen in place), (b) the idle loop
  honors NEED_RESCHED before halting (closes the IPI-before-halt
  stranded-task window the idle dispatch newly exercised), (c) with no
  idle cpu, the busy-cpu preempt is gated on the MEASURED wait: a wakee
  that last ran within one SCHED_LATENCY_NS window is a batch peer and
  is left to the slice/blocker machinery; a wakee that waited a full
  window keeps the immediate preempt. RT/DL wake paths unchanged.
- e1e7d8f9 unix: single-copy send (owned-Vec segment enqueue), write(2)
  fast path, no-zero-fill staging for sendto/recvfrom/writev-gather.
- 236dec39 + revert eab076ec pipe: the O_ASYNC registry lock-free
  empty check was measured, then WITHDRAWN — see the layout note.
- 3f24ac59 test(probes): ipcdisc/ipcdisc2/ipcdisc3 in probes.tar.gz.

## p1_null layout variance (documented, not chased)

p1_null on this rig swings 0.47-0.65 us across semantically adjacent
strong builds: origin/main 0.481/0.481; sched-only 0.471; sched+unix
0.481; +pipe 0.631/0.641; pipe-reverted (SOURCE IDENTICAL to the
sched+unix tree) 0.651 with a different bzImage md5 — incremental LTO
reuses different object sets and re-rolls code layout, and under
TCG/MTTCG that is worth ±0.17 us on the null syscall. The socket-family
results are layout-stable (2.0 us across every variant). p1_clock moved
0.671 -> 0.752 (+12%, inside the gate's +-20% band). Verdict columns
for the merge gate should re-anchor p1_null on its own builds; treat
+-0.2 us on p1_null as build noise on this rig unless it survives a
same-source rebuild.

## What is still behind the reference (filed, not fixed here)

- p1_null 0.63 vs 0.24: the syscall entry itself (arch lane).
- p5b_recv_peek 3.82 vs 2.66: the hilogd peek+consume shape = two full
  syscalls per record; each syscall carries the p1 tax.
- The vruntime accounting skew (sender +10ms requeue clamps, sleeper
  credit drops) is the deeper CFS defect that made the wakeup gate
  useless — the sleep-gate is a robust workaround, but a fair.rs fix
  (min_vruntime semantics / place-on-wake) would restore true
  vruntime-gated wakeup preemption. Evidence: CHG probe logs in the
  session record; WPRE deltas 2-6 ms steady.
- binder SLOW-TXN family: not re-measured this session (the wake-path
  fix removes the per-record preempt class that fed the 18/boot
  residue's queue-latency component; the image-side 15/30/89 s family
  is userspace-owned). Suggest a slowtxn re-bank on the next OH boot.

## Gates

make test (riscv64, in-guest suite): 1204/1204 green after the fixes.
Builds: x86_64, riscv64, aarch64 rc=0. riscv64 smoke boots (unit-suite
init and shell init) green, zero real panics. Perf-gate census/pbench
re-bank belongs to the coordinator's merge gate (sched/ is a guarded
domain: wake→pick deltas and the census bands need a re-anchor pass on
this branch).
