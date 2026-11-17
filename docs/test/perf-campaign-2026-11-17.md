# Performance campaign 2026-11-17 — the 2×-over-Linux push

Status: merged into main through `cb36c271e`. This file is the campaign's
evidence record; the per-lane details live in each lane's `run/` findings
doc (paths listed at the bottom). The reference column is the same-rig
guest kernel (`7.0.0-38`), same image, same benchmark binaries — a
guest-to-guest comparison, not host-native.

## 1. Why this campaign existed

The first two-sided battery (`pair-battery-2026-11-17.md`) showed the
storage paths 10–226× behind the reference kernel. The microbenchmark
rows (lmbench) read 1.09–1.19× because they measure per-operation
syscall latency, not sustained I/O — the gap lived in the block layer
and I/O pipeline, invisible to microbenchmarks.

The target: **at least 2× faster than the reference kernel** on the
headline rows.

## 2. Where the campaign landed

| Row | Campaign start | Now | Reference | Now vs reference |
|---|---|---|---|---|
| 1M sequential read 48M (direct) | 40.8 MiB/s (226× behind) | 4711 MiB/s | 9600 MiB/s | 0.60–0.63× |
| 1M sequential write 48M (direct) | 93.9 MiB/s (57× behind) | 1583 MiB/s | 4019 MiB/s | 0.39× |
| 4K random read 40M (direct) | 17.8k IOPS (2.34× behind) | 33.4k IOPS | 43.7k IOPS | 0.77× |
| 1 MiB buffered write 48M | 6000 MiB/s | 8000 MiB/s | 9600 MiB/s | 0.83× |
| getdents 1000-entry dir | 1374 µs (19× behind) | 37 µs | 72 µs | **1.95× ahead** |
| p6_stat | 0.491 µs (2.23× behind) | 0.281 µs | 0.220 µs | 1.28× |
| p6_openclose | 0.902 µs (2.43× behind) | 0.711 µs | 0.371 µs | 1.92× |
| p6_read_cached | 1.202 µs (2.18× behind) | 1.012 µs | 0.551 µs | 1.84× |
| p4_fork_exit (fragmented parent) | 68.6 µs (5.31× behind) | 39.0 µs | 12.9 µs | 3.02× |
| p4_fork_exec (fragmented parent) | 77.2 µs (3.00× behind) | 44.3 µs | 25.7 µs | 1.72× |
| p1_null (syscall entry floor) | — | 0.050 µs | 0.050 µs | parity |
| rt_wakesys | — | 3.3 µs | parity | parity |
| t_abs (absolute timer) | — | 50.7 µs | 72 µs | ahead |

The 226×/57× class moved to the 0.4–0.6× class; the 19× getdents class
moved to ahead of the reference. The 2× target is **not** reached on the
sustained-I/O rows yet — the remaining distance is concentrated in the
journal-commit path (4K write+fsync 0.30×), the per-op submit cost of
the 1M direct rows, and the descriptor-construction path.

## 3. What landed (each with its own A/B)

1. **Batched O_DIRECT + extent-run resolution** (fio-2x, `5606622b8`):
   the direct reader resolves the request into physically-consecutive
   runs and submits each run as ONE multi-block request; the write
   tail updates page-cache frames in place instead of inserting dirty
   duplicates. Killed the "256 sequential 4 KiB round trips per 1 MiB
   op" shape.
2. **32768-block extent decode fix** (`73f0ee509`) — a data-correctness
   bug found by the campaign's own md5sum check: contiguous runs ≥128 MiB
   served hole zeros. Also kicked the DIO read window (the virtqueue was
   never kicked on a quiet boot).
3. **Batched journal write-through** (fio-2x) — one commit's per-buffer
   sequential sync collapsed; fsync row halved against main.
4. **MSI-X completions + inline hardirq walker** (dio-conc,
   `f051ef80e`): the per-op cost was INTx delivery (39.5 µs) plus a
   shared dispatcher that polled EVERY registered virtio function's ISR
   per IRQ (16.3 µs — a hypervisor round trip each). Now: MSI-X with
   the completion walker inline in the hardirq vector (the reference
   vring shape), bounded try-lock deferral preserved.
5. **Whole-MiB staging + `insert_dirty_batch`** (dio-conc): one lock
   pass per 64 pages for the buffered write path.
6. **Incremental readdir** (getdents, `4b9f8ba3f`): a typed
   `INodeOps::readdir_fill` op packs `linux_dirent64` records straight
   into the caller's buffer one directory block at a time — the
   whole-directory `Vec` materialization (≈3000 allocations per sweep)
   is structurally gone, and so is the 4 MiB whole-dir allocation class.
   `f_pos` becomes the ext4 byte offset; `d_off` the next entry's
   offset (telldir/seekdir round-trip through lseek).
7. **Path-walk costs** (stat-2x, `78b17d528`): `has_mount()` early-out
   in `follow_mount`; per-inode open counters replacing the global
   `INODE_OPENS` table (Linux parity); an adaptive lock-free child
   memo (self-describing seqlock table sized next_pow2(2×children),
   clamped [128, 2048]) replacing the 4-slot inline memo that
   thrashed to ~0% hit rate in large directories.
8. **Empty-table fast-path gates + per-CPU file ids** (sysmm-2x,
   `9b7a9aef3`): every regular-file open paid a lease-table lock +
   scan and every close four more global lock pairs, all with empty
   tables; live-entry mirrors collapse each to one acquire.
9. **munmap frees emptied leaf tables** (fork-frag, `746678591`):
   `unmap_pages` cleared PTEs but never freed emptied tables — 36 linked
   leaf tables against 61 present PTEs, every fork allocating 36 child
   tables. Freed behind a per-CPU TLB-flush epoch (the `tlb_remove_table`
   shape, zero added IPIs); the table allocator takes the pcp per-CPU
   pageset fast path.
10. **Hardirq timer expiry + PI-futex reference shape** (rt-2x,
    `e0ff2a5b8`/`4256a7cc7`): hres timers expire at the top of the
    timer IRQ (the reference `hrtimer_interrupt` shape); PI futex
    gains the reference `rt_mutex_setprio` behaviour (the owner
    converts to FIFO at the waiter's priority; the boost transfers on
    ownership handoff; a lost-wake race in the post-park unlink
    closed). `pi_stress -g 1` passes 132k inversions.

## 4. Methodology notes worth keeping

- **debug-vs-release matters**: the first battery compared a debug
  kernel against a release reference. On release, `rt_wakesys` is
  3.3 µs (parity) — the debug build inflated it.
- **The "concurrency depth" hypothesis was wrong**: the batched DIO
  reader takes no global big-lock, and numjobs=4/8 scale linearly on
  our side. The 4K random gap was pure per-op cost, and the phase
  probe put it in interrupt delivery, not locking.
- **One experiment measured net-negative and was reverted with the
  record kept**: deferred mm-drop via a kthread (the wake IPI plus an
  RCU-grace poll per queued mm cost more than it saved).
- **`st_dev` / `getdents` anchor drift**: the pbench anchors were
  banked on different days; several "regressions" were anchor drift
  (mkdrmdir reads ~260 µs on both arms today; the 216.7 anchor is from
  2026-11-03). A per-row re-bank table belongs to the fs lane.

## 5. Open items (the distance to 2×)

1. **Journal-commit cost** — one commit ~1.9 ms vs the reference's
   489 µs; the D-8 direct-write commit does per-buffer sequential
   sync. Row: 4K write+fsync (0.30×), fsync row.
2. **Per-op submit cost of the 1M direct rows** — 1583 MiB/s vs 4019.
3. **Descriptor construction** — openclose's close-side grace is
   ~250–330 ns and semantically load-bearing; `File::new` slimming
   and an embedded `f_ra` are the named levers.
4. **Timer slack for RT tasks** — the reference exempts RT tasks from
   timer slack; our relative sleeps overshoot by ~21–41 µs.
5. **Pinned-RT slot corruption** — `cpu_state.current` stops tracking
   the running task, so both wake-dispatch paths refuse to poke it
   (pi_stress -g 2 wedges; reproduced with zero PI involvement).
6. **libaio / io_uring** — both unavailable (ENOSYS / ring-semantics
   crash); they need the DIO batching first to matter.

## 6. Lane artifacts

- `~/rux-agents/fio-2x/run/FIO-2X.md`
- `~/rux-agents/getdents/run/GETDENTS.md`
- `~/rux-agents/dio-conc/run/DIO-CONC.md`
- `~/rux-agents/stat-2x/run/STAT-2X.md`
- `~/rux-agents/sysmm-2x/run/SYSMM-2X.md`
- `~/rux-agents/fork-frag/run/FORK-FRAG.md`
- `~/rux-agents/rt-2x/run/RT-2X.md`

Baseline table: `docs/test/pair-battery-2026-11-17.md`.
