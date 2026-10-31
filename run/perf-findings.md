# perf-mm lane findings — 2026-10-31

Method: true A/B per charter — `test/pbench_boot.py` boots the SAME
pbench P1-P7 battery on the reference bzImage
(/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage,
6.6.101) and on our strong-profile bzImage, same machine (q35, TCG
thread=multi, -cpu max, SMP2, 2G), same probe binary, back-to-back
windows. Host load 4-8 during these runs (not quiet-protocol grade;
gaps smaller than ~1.3x are not verdict-grade at that load — the big
rows below are far outside noise).

## The headline correction

The charter's numbers (p3a p99 581us bimodal; fork+exit 9ms; fork+exec
13ms) predate two merged fixes (RCU QS on user-return c312944d, pcp
pagesets 45ccb66d) and were debug-profile numbers. On current main,
strong profile, the true A/B board is:

| bench | Rux med (us) | ref med (us) | gap |
|---|---|---|---|
| p1_null | 0.601 | 0.270 | 2.2x |
| p1_clock | 0.761 | 0.861 | ahead |
| p2_ctxsw | 1.347 | 2.044 | ahead |
| p3a anon fault | 0.080 (avg 2.12) | 5.711 (avg 8.07) | ahead |
| p3b COW fault | 2.954 | 7.915 | ahead |
| p3c file fault | 0.070 (avg 16.87) | 0.080 (avg 0.58) | avg 29x |
| p4_fork_exit | 507.0 | 346.9 | 1.46x |
| p4_fork_exec | 1621.1 | 841.7 | 1.93x |
| p7 sleep1ms | 1192.2 | 16.6 | 72x (timer domain, not mm) |

Full lines: run/pbench-pmm-base-lines.txt (Rux main 1e2bef44 strong),
run/pbench-pmm-refsmoke-lines.txt (reference, same machine).

p3a bimodality explained: med 0.08us = the 15/16 stores that hit pages
the anon fault-around already mapped (16-page forward window, VMA >= 1MB
write faults); the tail (~52us p99 on strong) is the fault-around batch
itself (16 allocs + 64KB zeroing + 16 maps). Per-page effective cost
(avg 2.12 vs ref 8.07) is AHEAD of the reference — the reference faults
every page individually (no anon fault-around; its THP is bimodal under
this config). No action needed on p3a.

## F1 (FIXED): write path never populated the page cache — p3c cold faults 14.3x slower than warm

Decomposition probe (test/pmmprobe.c, PMMP rows; run/pmm-decomp* logs):

| shape | Rux avg us/page | ref avg us/page |
|---|---|---|
| MM1 cold file fault (write+fsync, mmap, touch) | 14.107 | 0.692 |
| MM2 warm (pread first, mmap, touch) | 0.942 | 0.635 |

Our ext4 write path writes through the buffer cache and only UPDATES
existing page-cache frames (writeback_update_range); it never inserts.
The reference kernel's buffered write populates ITS page cache, so its
"cold" faults are cache hits. Every written-then-mapped file (the
dynamic linker's every lib) paid the populate path per fault window:
filemap_pin_cache_page miss -> ext4_fault_populate_cache ->
EXT4_BIG_LOCK + fill_page_cache_batch + RA state.

Fix: `PageCache::writeback_populate_range` (fs/page_cache.rs) — after a
successful write, update existing frames (unchanged semantics) AND
insert the fully-written page run as clean cache frames via
insert_batch (one wlock + buddy allocations for the whole run; partial
head/tail pages stay to the fault-time populate). Write-core call site
switched (fs/ext4/file.rs).

Numbers (same boot class, pmm-pop vs pmm-base/decomp runs):
- MM1 cold file fault avg: 14.107 -> 0.985 us/page (14.3x), now == warm
  (0.977) and == the reference cold shape (0.692, 1.4x residual).
- Warm path unchanged (0.94 -> 0.91-0.98).

## F2: fork+exit gap is ENTIRELY in exit+reap; fork syscall at parity

Decomposition (mm5/mm6, 400 iterations):

| shape | Rux med | ref med |
|---|---|---|
| MM5 fork syscall alone (child parks 200ms) | 171.6-209.0 | 217.1 |
| MM6 fork+exit+wait round trip | 500-553 | 346.9 |
| exit+reap (derived) | ~330-344 | ~130 |

The COW page-table copy — the charter's copy_page_table_cow suspect —
is NOT the bottleneck; the fork syscall is at parity. The excess is on
the child-exit/parent-reap side. mm7 (vfork round trip: no mm copy, no
mm teardown) will split machinery vs mm-teardown — measurement in
flight (pmm-decomp3).

Structural observations on the exit path (audit):
- Parent notification is DEFERRED to after the dying task's final
  context switch (defer_exit_notify -> processed in __schedule's tail
  from the incoming task's context). The parent's wake therefore waits
  for a full switch-to-idle + notify + wake chain; the reference wakes
  the parent inside exit_notify (before its final schedule).
- MmStruct::drop runs synchronously on the child's exit: shm walk,
  VMA-tree release under vma_write, synchronize_rcu (cheap: RCU_GEN is
  frozen absent call_rcu users), then free_user_page_tables with
  per-leaf-table lock chunks + cond_resched between chunks (the
  cond_resched boundaries are correct for the launcher wall, but each
  boundary can donate the CPU to the parent's wake -> scheduling
  latency inside the round trip).
- The task/kstack/pid frees on reap (free_task_slot, free_kernel_stack,
  pid free) and the TLB-shape work (fork does flush_tlb_all +
  fire-and-forget IPI broadcast; exit needs none).

## F3: exec adds ~1.1ms on our side (mm8: fork+exec+wait 1627 vs mm6 500)

Reference fork+exec 841.7 vs fork+exit 346.9 -> exec adds ~495us.
Ours: 1627 vs 500 -> exec adds ~1127us. The exec path
(do_execve_elf) maps segments eagerly, then walks per page to tighten
permissions (4-level walk per page), reads the ELF + phdrs, copies
argv/envp, and the exec'd image re-faults its pages on first run.
~2.3x the reference's exec cost — secondary to F2 but same family.

## Non-mm rows noted for the owning lanes (not acted on here)

- p1_null 2.2x (syscall entry path).
- p7 sleep1ms overshoot 72x (16.6us ref vs 1192us ours — timer/tick
  domain).
- p6 rows (stat/openclose/read_cached/append) 3-9x behind — fs domain.

## Instrument

- test/pmmprobe.c — the PMMP decomposition probe (cold/warm file fault,
  anon fault, fork syscall vs round trip vs vfork vs fork+exec). Boot
  with: pbench_boot.py --probe-bin work/pmmprobe --bzimage <k>.
