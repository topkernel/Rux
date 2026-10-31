# perf-mm lane findings — 2026-10-31

## FINAL A/B board (same machine, same battery, TCG q35/-cpu max/SMP2/2G)

`test/pbench_boot.py` on the reference bzImage (refsmoke run), our
strong-profile kernel at main 1e2bef44 (base run), and our strong
kernel with this lane's two fixes (after run, commits d2fac01a +
6cd3c9b8; provenance run/pmm-fix-built-from.txt; lines in
run/pbench-pmm-{refsmoke,base,after}-lines.txt):

| bench | base med | after med | ref med | base/ref | after/ref |
|---|---|---|---|---|---|
| p3c fault file (AVG, us) | 16.871 | **1.039** | 0.580 | 29.1x | **1.8x** |
| p4_fork_exit (med) | 507.0 | **447.3** | 346.9 | 1.46x | **1.29x** |
| p4_fork_exec (med) | 1621.1 | **1543.3** | 841.7 | 1.93x | 1.83x |
| p6_write_fsync (med) | 20592 | **14131** | 16583 | 1.24x | **0.85x (ahead)** |
| p6_append (med) | 89.3 | 93.3 | 9.4 | 9.5x | 9.9x |

p1/p2/p3a/p3b/p5a unchanged (ahead or parity; full lines in the run/
files). The p6_append +4.5% med is the populate's one page-cache
insert per 4 KB append write — the explicit trade for the 16x p3c win;
the row's remaining 10x is the ext4 append/journal path (fs lane).

Probe rows (test/pmmprobe.c, PMMP lines) across the lane's boots:

| row | base | after (clean build) |
|---|---|---|
| mm1 cold file fault avg (us/page) | 14.107 | 1.002 |
| mm2 warm file fault avg | 0.942 | 0.836 |
| mm5 fork syscall med | 172-209 | 176.9 (ref: 217.1) |
| mm6 fork+exit+wait med | 500-553 | 463.9 (ref: 346.9) |
| mm7 vfork round trip med | 202-233 | 158.2 |
| mm8 fork+exec+wait med | 1596-1677 | 1532.9 (ref pbench row: 841.7) |

---


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

## F2: fork+exit gap decomposition — fork at parity; exit work = 190us of the 347us; futex scan was 64us of it (FIXED)

Decomposition (mm5/mm6/mm7/mm8 rows, 400 iterations each):

| shape | Rux med | ref med |
|---|---|---|
| MM5 fork syscall alone (child parks 200ms) | 172-209 | 217 |
| MM6 fork+exit+wait round trip | 500-553 | 346.9 |
| MM7 vfork round trip (no mm copy, no mm teardown) | 202-233 | — |
| MM8 fork+exec+wait | 1596-1677 | 841.7 (pbench row) |

Kernel-side attribution (EXITC TSC buckets, 10 MHz clock, steady-state
windows during the mm6 loop; run/pmm-diag2-serial.log):

| exit-path piece | us/exit |
|---|---|
| MmStruct::drop total (mm_drop bracket) | 80 |
|   — VMA tree release under vma_write | 14 |
|   — synchronize_rcu | 1 (RCU_GEN frozen: cheap, confirms the ledger) |
|   — free_user_page_tables walk (in this context) | ~65 |
| exit "rest" block (mm-drop -> ZOMBIE) | 92 |
|   — futex_cleanup 512-bucket scan | 64 |
|   — fdtable drop | 20 |
|   — reparent | 2 |
| notify (deferred, in idle context) | 14 |
| reap (release_task, incl. its own synchronize_rcu) | 17 |

Sum of bracketed work ~ 190us; the roundtrip is 523us, so ~330us is
scheduling/wake latency (child's final context switch to idle, deferred
notify, parent wake + reschedule) — the sched lane's domain, not mm.

### F2a (FIXED): futex_cleanup scanned all 512 buckets on EVERY exit

`futex_cleanup` took `HASH_HEADS[bucket].lock_irqsave()` for all 512
buckets and walked every chain on every task exit — 64us under TCG for
tasks that never parked on a futex (the overwhelmingly common case; the
sleeper-side return unlinks the waiter itself).

Fix: `Task::futex_wait_enqueued` residency flag — set at the single
enqueue site (futex_wait_timeout's chain link, under the bucket lock),
cleared by every unlink/free path (remove_waiter both branches, the
woken-sleeper slot free, and the cleanup scan itself). futex_cleanup
returns after one load when the flag is clear. Numbers: diag3 boot
(below).

### F2b (root cause of the residual scans): the flag read HEAP GARBAGE on fork-allocated tasks

The diag4 counters (ffp/fscan) showed fork+exec children still
full-scanning 64/64 while vfork children skipped. The flag had NO
setter for those tasks: `alloc_task_slot` builds the Task via
`new_task_at` — a FIELD-BY-FIELD ptr::write init list over a recycled
heap page (the sc130 comment in task.rs documents this exact trap for
the last_fails rings) — and my at-construction `AtomicBool::new(false)`
only runs in `Task::new` (first construction). Fork tasks read the
stale page bit; whether a given task scanned depended on its page's
history (mm7's mixed ffp=52/fscan=12 vs mm8's 64/64).

Fix: init `futex_wait_enqueued` in `new_task_at`'s init list (commit
6cd3c9b8). Lesson for every future Task field: add it to BOTH
constructors.

## F3: exec adds ~1.1ms on our side (mm8: fork+exec+wait 1627 vs mm6 500)

Structural audit notes (exit/notify chain, for the sched lane):
- Parent notification is DEFERRED to after the dying task's final
  context switch (defer_exit_notify -> processed in __schedule's tail
  from the incoming/idle context, with parent-CPU steering). The
  ~330us scheduling-latency residual of the round trip lives in this
  chain (switch-to-idle, notify, parent reschedule) — fork syscall and
  all bracketed kernel work sum to ~370us of the 523us roundtrip.
- copy_page_table_cow EXONERATED (fork syscall at parity; the walk is
  chunked with per-leaf-table PTE sections; pte_install_log and the
  FUT/PT_LEDGER forensics are switch-gated).
- The teardown walk (~65us for a ~25-page mm) is a mix of per-PTE
  descriptor work (rmap_alt_remove, page_remove_rmap, put_page_site,
  free_pages via pcp) and per-table ledger checks — no single dominant
  line item; diffuse. Left as-is this lane.

## F4: exec cost (secondary)

Reference fork+exec 841.7 vs fork+exit 346.9 -> exec adds ~495us.
Ours: 1596-1677 vs 500-553 -> exec adds ~1100us. The exec path
(do_execve_elf) maps segments eagerly, then walks per page to tighten
permissions (4-level walk per page), reads the ELF + phdrs, copies
argv/envp, and the exec'd image re-faults its pages on first run.
~2.2x the reference's exec cost — fs+mm mix; not attacked this lane
(the execcost DFX switch exists for the follow-up).

## Code-level audit (charter items not driven by a measurable A/B row)

- **TLB shootdown**: x86_64 flush is fire-and-forget IPI broadcast +
  local CR3 reload (fork end, munmap end). No synchronous wait (the
  reference waits since its 4.x rework) — our shape is the FAST one;
  note the narrow freed-frame race window it accepts is documented at
  the call sites. COW-fault resolution uses per-page invlpg + IPI
  (mm_ops 2494/2540). No change.
- **munmap**: per-page TWO root-level walks (PageTableWalker::walk +
  clear_pte) + rmap/put/free, ONE full flush + one IPI per batch —
  sound shape; the double walk per page is a known ~2x zap tax vs the
  reference's table-driven descent, but munmap is not in the battery
  and the fork/exec rows that matter are at/near parity. Filed, not
  changed.
- **copy_page_table_cow**: per-PTE 2 atomic RMWs (get_page +
  inc_mapcount) + parent-PTE rewrite, leaf-table chunks, full flush at
  end. MEASURED at parity via mm5 (fork syscall 172-209 vs ref 217).
- **VMA tree**: BTreeMap keyed by start; O(log n) find/add — same
  class as the reference. mprotect's collect+sort under vma_write
  remains the known wart (sibling lane F3, pending there).
- **Demand-fill vs fault-around**: anon write faults on VMAs >= 1MB
  get a 16-page forward window (x86_64 AND aarch64 — the aarch64 port
  exists, header-documented, `faaround=0` opt-out; the charter's
  "aarch64 deferred" note is resolved). File read/exec faults get the
  16-page cache-direct window on both arches. Effective per-page anon
  cost measured AHEAD of the reference (2.1 vs 8.1 us avg).
- **Allocator family (small_alloc/buddy/slab)**: not reachable from
  the x86_64 A/B battery; the standing perf-gate census (REST share,
  magazine discipline, small_alloc class-lock sections) owns those
  numbers on riscv64 — no new evidence here; no change.
- **deferred_free deadline-stall and buddy merge bugs**: already fixed
  on this branch's base (a90b76fc, bab50e52 — see
  run/review-findings-mm.md).

## Non-mm rows noted for the owning lanes (not acted on here)

- p1_null 2.2x (syscall entry path).
- p7 sleep1ms overshoot 72x (16.6us ref vs 1192us ours — timer/tick
  domain).
- p6 rows (stat/openclose/read_cached/append) 3-9x behind — fs domain.

## Instrument

- test/pmmprobe.c — the PMMP decomposition probe (cold/warm file fault,
  anon fault, fork syscall vs round trip vs vfork vs fork+exec). Boot
  with: pbench_boot.py --probe-bin work/pmmprobe --bzimage <k>.
