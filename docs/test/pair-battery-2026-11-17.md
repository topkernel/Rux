# Pair battery 2026-11-17 — same-image same-binary comparison vs the reference kernel

Status: data recorded 2026-11-17 (main @ the 11-17 debug build). This file is the
canonical record of the first full two-sided pair battery and the working
optimization queue derived from it. Every gap below carries its mechanism and a
concrete fix shape; the queue is worked one item at a time.

## 1. Protocol

- **Rig**: QEMU q35, `-accel kvm -cpu max`, smp2/2G, virtio-blk, `snapshot=on`
  — identical for both arms; the only delta between arms is the `-kernel`
  bzImage.
- **Rux arm**: `target/x86_64-unknown-none/debug/bzImage` (main, 2026-11-17).
- **Reference arm**: the host build (7.0.0-38-generic) booted as the guest
  kernel in the identical rig — the same reference-column convention as
  lmbench-report.md.
- **Same image**: one ext4 copy (`run/bench-pair.img`, reflinked from the qa
  vehicle), injected once with the same static binaries for both arms: fio
  3.36, cyclictest/hackbench/pi_stress (rt-tests v2.11), the flush-wall probe,
  the qsrun cmds runner.
- **Same cmds**: one cmds file, 19 items, executed in sequence per arm
  (`run/bench-pair.cmds`); results captured through the qsrun `RES|` protocol.
- **Host context**: one foreign QEMU from another lane ran during the window;
  both arms faced the same ambient load. Cyclictest max rows carry
  final-window spikes that correlate with it — avg/miss-rate rows are the
  meaningful comparison.

## 2. fio — storage I/O

| Job | Rux | Reference | Ratio |
|---|---|---|---|
| 1M sequential write 48M (direct) | 93.9 MiB/s · clat 10.6 ms | 5.2 GiB/s · clat 171 µs | ref 57× |
| 1M sequential read 48M (direct) | 40.8 MiB/s · clat 24.5 ms | 9.4 GiB/s · clat 90 µs | ref 226× |
| 4k random read 40M (direct, 20 s) | 69.7 MiB/s · 17.8k IOPS · clat 22.7 µs | 163.2 MiB/s · 41.8k IOPS · clat 22.7 µs | ref 2.34× (latency at parity) |
| 4k random rw 40M (direct, 15 s) | 30.9/30.9 MiB/s · clat 74.7/50.3 µs | 72.7/72.7 MiB/s · clat 23.9/25.3 µs | ref 2.35× |
| 4k random write 8M + fsync=1 | 1.0 MiB/s · 260.6 IOPS · fsync 3.82 ms | 7.9 MiB/s · 2017.7 IOPS · fsync 489 µs | ref 7.7× (IOPS) |
| 1M buffered write 48M | 9.4 GiB/s · clat 79.9 µs | 9.4 GiB/s · clat 89.5 µs | parity |
| Flush-wall probe (128 MB, total wall clock) | 4.896 s (fsync 3.523 · msync 0.572) | 1.223 s (fsync 1.142 · msync 0.022) | ref 4.0× |

Engines without numbers are unavailable: libaio (io_setup ENOSYS — UAPI not
implemented), io_uring (the enter contract crashes user space — ring
semantics incomplete). posixaio runs (round-5b readings on the qa-suites
ledger).

### Mechanism (from the data shape + the code)

1. **4k random read: identical latency (22.7 µs both sides), 2.34× IOPS gap**
   — the per-IOP cost is the same; the difference is concurrency depth
   (in-flight requests). The reference runs deeper queues; Rux serializes the
   direct path (the EXT4_BIG_LOCK covers the whole miss/direct path).
2. **1M sequential: per-request cost dominates.** Rux's O_DIRECT leg serves
   through the buffer cache **one 4 KiB block at a time, synchronously**
   (`ext4/file.rs` DIO branch → `bio::bread`; the in-code note already records
   the shape: "64 KiB pread chunks of a 128 MiB file paid 32768 lookups"). One
   1M I/O = 256 sequential 4 KiB round trips: write 10.6 ms ≈ 256 × 41 µs,
   read 24.5 ms ≈ 256 × 96 µs — both match the measured 4k direct latencies.
   The reference submits the full 1M request once (write 171 µs, read 90 µs).
3. **fsync row**: one journal commit costs ~3.82 ms on Rux vs 489 µs on the
   reference.
4. **Buffered write at parity** — the page-cache-absorbed path is not the
   bottleneck; everything above is the direct/synchronous path.

## 3. rt-tests — real-time latency and scheduling stress

| Item | Rux | Reference | Notes |
|---|---|---|---|
| cyclictest SCHED_OTHER (missed/avg/max µs) | 59/30000 · avg 107 · max 29061 (final-window spike) | 31/30000 · avg 63 · max 1417 | |
| cyclictest SCHED_FIFO p98 (missed/avg/max µs) | 56/30000 · avg 104 · max 27889 (final-window spike) | 25/30000 · avg 12 · max 887 | |
| hackbench socketpair -g 20 (s) | 1.536 | 0.330 | ref 4.7× |
| hackbench pipe -p 20 -g 4 (s) | 0.163 | 0.017 | ref 9.6× |
| hackbench thread -T 20 -g 4 (s) | 0.287 | 0.042 | ref 6.8× |
| pi_stress -g 2 -D 20 (inversions) | — (PI-mutex UAPI init fails: ENOTSUP) | >4.84M (~240k/s) | |

### Mechanism

1. **The FIFO tell: Rux's SCHED_FIFO 98 (avg 104 µs) does not beat its own
   SCHED_OTHER (avg 107 µs)**, while the reference improves 63 → 12 µs. The
   gap is not priority arbitration — it is the wakeup delivery path. The
   reference's hrtimer expires in hardirq context and preempts immediately;
   Rux's timer expiry delivers in **softirq context** (the SoftirqCtx
   delivery guard) and kernel preemption defaults off (`kpreempt=0`), so the
   freshly woken RT task runs at the current context's next scheduling point.
2. **Sleep overshoot ~16 µs** (pbench p7_sleep1ms, the 14.6 µs class) — a
   small contributor.
3. **IF=0 long segments** (the 6–9 ms gate class) delay the timer fire itself
   occasionally; min 47 µs shows the best path is far faster than the mean —
   the mean is raised by intermittent segments. The 27–29 ms final-window
   max spikes correlate with the ambient foreign QEMU + this class; the
   controlled isolated round (r3) reached FIFO max 396 µs.
4. **hackbench**: the message-passing forms stress wake/schedule round trips —
   consistent with the wakeup-path cost above.
5. **pi_stress**: the kernel-side FUTEX_LOCK_PI exists, but glibc's
   PTHREAD_PRIO_INHERIT mutex chain (robust registration + PI semantics) is
   not traversed yet — a registered UAPI gap.

## 4. Optimization queue (worked one item at a time; each item cites its row)

1. **DIO batching** — route the O_DIRECT leg through `bio::bread_async_multi`
   (the batching exists for the page-cache fill) and/or coalesce to large
   requests before submit. Row: 1M sequential read/write. Acceptance anchor:
   the 1M direct rows within a stated multiple of the reference column.
2. **Direct-path concurrency** — relieve the whole-path serialization on the
   direct leg (per-inode/per-range locking instead of the global big lock;
   deeper virtio queue exposure). Row: 4k random read (2.34× at equal
   latency).
3. **Journal-commit cost** — one commit 3.82 ms → sub-ms class (the D-8
   direct-write commit's per-commit work). Rows: fsync=1, flush-wall fsync.
4. **RT wakeup delivery** — move timer expiry delivery out of the
   softirq-pinned context into the hardirq-preemptible path (the reference
   hrtimer shape) so a woken RT task preempts immediately; evaluate the
   `kpreempt` default with this in place. Rows: cyclictest FIFO avg (104 →
   the reference 12 µs class is the acceptance anchor), hackbench forms.
5. **NOHZ on all architectures** — x86_64 is wired; the other twins keep the
   grid glue. Rows: the latency rows under load.
6. **IF=0 long-segment compression** — the 6–9 ms REST segments delay timer
   fires; continue the phantom-bits direction. Rows: cyclictest max; the
   final-window spike class.
7. **PI-mutex UAPI chain** — robust registration + the LOCK_PI semantics
   glibc expects; acceptance anchor: pi_stress completes inversions.
8. **msync/sync writeback phase** — the mapped-dirty writeback path (13.9× on
   the msync phase). Row: the flush wall.
9. **io_uring enter contract** — ring semantics to the point where fio's
   io_uring engine runs; libaio io_setup/io_submit family (UAPI).
10. **pbench dual-kernel column** — the P1–P7 battery is queued on both
    kernels (same image); the table lands in this file when the run
    completes.

## 6. pbench dual-kernel column (2026-11-17, sysmm-2x lane)

Infrastructure: `test/pbench_boot.py --ref-kernel` (stages
`test/pbench_ref_init.c` — mounts proc/sys/devtmpfs, wires the console,
execs the same static musl probe — and boots the distro kernel on the
packed rootfs). Rig: q35/KVM/`-cpu max`, smp8/2G, same probe binary
and image shape both arms, Rux = main 9df618643 release.

| Row | Rux | Reference | Ratio |
|---|---|---|---|
| p1_null | 0.050 µs | 0.050 µs | 1.00x |
| p1_clock | 0.060 | 0.070 | **0.86x ahead** |
| p2_ctxsw | 0.115 | 0.075 | 1.53x |
| p3a_fault_anon | 0.010 | 0.260 | **0.04x ahead** |
| p3b_fault_cow | 0.601 | 0.721 | **0.83x ahead** |
| p3c_fault_file | 0.010 | 0.020 | **0.50x ahead** |
| p4_fork_exit | 91.968 | 13.826 | **6.65x behind** |
| p4_fork_exec | 114.409 | 26.580 | 4.30x behind |
| p5a_pipe | 0.150 | 0.150 | 1.00x |
| p5b_dgram_send | 0.160 | 0.210 | **0.76x ahead** |
| p5b_dgram_recv_peek | 0.370 | 0.230 | 1.61x |
| p5c/c2/c3 stream | 0.170 | 0.180-0.190 | ~0.9x ahead |
| p5d_rt_read / rt_recv | 0.301 / 0.340 | 0.321 / 0.311 | 0.94x / 1.09x |
| p5e_child_spin | 19.646 | 66.635 | **0.29x ahead** |
| p6_stat | 0.491 | 0.220 | 2.23x |
| p6_openclose | 0.942 | 0.371 | 2.54x |
| p6_read_cached | 1.232 | 0.551 | 2.24x |
| p6_append | 0.641 | 0.571 | 1.12x |

Readings that update the queue:

- The memory-fault family (p3a/p3b/p3c) is AHEAD — 25x, 1.2x, 2x
  respectively. The historical "COW +58%" gap is retired: it was
  anchored on a debug-profile kernel before the lazy-mm_cpumask
  shootdown work landed.
- The p4 fork/exec rows are the real MM gap (6.65x/4.30x at the
  fragmented-parent shape; the lmbench lat_proc shape with a small
  parent reads 0.80x AHEAD). Decomposition + first fix (VMA-bounded
  walks) in `run/SYSMM-2X.md` (sysmm-2x lane); remaining levers:
  munmap table freeing, per-table alloc cost, child wake-to-run
  amplitude (bimodal 51-69 vs 90-107 µs — the wakeup-delivery family
  of queue item 4).
- p6 stat/openclose/read confirm the open-path queue (standing
  lmbench rows); the sysmm-2x registry gates landed −11% on
  lat_syscall open.

## 5. Artifacts

- Battery driver + cmds: qa-suites worktree `run/bench-pair.sh`,
  `run/bench-pair.cmds`; raw serials `run/bench-pair-1117/`.
- Site presentation: the performance section carries the same tables
  (performance/fio.html, performance/rttests.html,
  performance/microbench.html).
- Gate context: docs/development/perf-gate-baseline.md (the guarded domains).
