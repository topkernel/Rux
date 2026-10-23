# Rux systematic performance audit (OH boot): measurement, ranking, root causes

Date: 2026-10-23/24. Lane: `feature/perf-audit` (branch off main `0727d295`).
Question: is the 16-23 min OpenHarmony boot (vs 10.7 s for the same image on
the reference Linux kernel, `runs/oh-ref-r1`) caused by a few systemic
root causes or a long tail of individual slow points? If few: which, how
much, and in what order should they be fixed?

Method: measure the whole quiet-OH boot on current main with new
kernel-side instrumentation (`syscost=1`, committed on this branch) plus
host-side serial timestamping; compare phases against the reference
Linux timeline; rank cost centers; group into families; propose a fix
roadmap routed to the existing lanes.

Runs (this worktree, `run/`):

| run | kernel | boot args | wall | notes |
|---|---|---|---|---|
| `pa-pa1` | main@0727d295 + syscost | syscost=1 | 1560 s (killed; alive) | 66,910 serial lines; full diagnostics on |
| `pa-pa2` | + softirq-vec counters | syscost=1 | ~360 s (killed) | dump lines interleaved by WW prints; superseded by pa3 |
| `pa-pa3` | + flood attribution | syscost=1 sprobe=0 | 780 s (killed) | flood/process attribution, softirq vecs |
| `pa-pa4` | + kmsg read counters | syscost=1 sprobe=0 | ~180 s (killed) | proves the read flood never touches /dev/kmsg |
| `pa-pa5/6/7/8` | + EOF/EAGAIN read censuses | syscost=1 sprobe=0 | 60-120 s each (killed) | fd-path census hunts (see F2) |

Host context for all runs: KVM q35 `-cpu max -m 4G -smp 2`, shared with
2-3 foreign-lane QEMUs (load avg ~5.5). Absolute numbers carry that
noise; ratios and structure are stable across pa1/pa3.

## Verdict in one paragraph

The "one fix at a time" pattern was real but it has converged into a
small number of systems-level families. Measured on current main, the
boot spends **~85% of its CPU time in kernel mode** (Linux reference
spends most of the same boot in userspace), and that kernel time is
dominated by (1) **interrupt-disabled regions** — a quarter to a third
of all kernel-mode ticks land in the page holding `__do_softirq` +
`enable_irq`/`restore_irq`, which is the signature of ticks queuing
behind long IF=0 windows (softirq dispatch at irq-exit runs with IRQs
off, and every `irqsave` critical section attributes its queued ticks
to this page's re-enable site); (2) a **userspace syscall flood riding
on kernel per-call costs** — 560M syscalls in 25 min (~370K/s on 2
vCPUs), 60% of it logging traffic (`hilogd.server` 13K socket
receives/s, `hilogd.rd_kmsg` 94K reads/s spinning), amplified by retry
loops; (3) **scheduler wake-loss** — 448 wake-without-pick events per
boot with median 252 s age, which converts 10x-slower operations into
minutes-long service stalls (the actual 16-23 min driver); and (4) the
**ext4/metadata per-op family** — open/stat/getdents at 100-200x Linux,
fsync/fdatasync at 20-90x. The dominant single lever is not any
algorithmic hotspot: it is interrupt-off time + wake-loss, both of which
multiply everything else. Fixing the known in-flight items (mount index,
anon fault-around on the launcher lane) will not change this picture.

## 1. Phase timeline vs the reference

Reference (`oh-ref-r1`, KVM, kernel timestamps):

| event | Linux t | Rux pa1 t (host stamps) | ratio |
|---|---|---|---|
| kernel start -> /init running | 1.40 s | 0.85 s | 0.6x (Rux faster: no device probe storm) |
| first ServiceExec | 1.84 s | ~4-5 s | ~2.5x |
| hdf_devmgr SA-5100 registration | 2.90 s | ~6.4 s (SAREG at 6.4 s) | 2.2x |
| `bootevent.appspawn.started` | 4.81 s | 31.9 s | 6.6x |
| `bootevent.composer_host.ready` | 4.89 s | 40.3 s | 8.2x |
| `bootevent.launcher.ready` | 10.12 s | not reached in 26 min | >150x |
| `bootevent.bootanimation.finished` | 10.73 s | not reached | >145x |

Divergence grows with boot depth: the first services are only 2-6x
slower; everything downstream of appspawn (bundle queries, sandbox
mounts, SA loads) runs 8x to >100x slower. That shape is the signature
of a *latency multiplier* (see family F3), not a fixed per-op cost.

pa1 survived the full 26 min without the log-stop wedge (66,910 lines,
still emitting at kill). The 16-23 min wedge of sc175-180 did not
reproduce this time; hilog backpressure remains the known terminal
event when it does (SC-SESSIONS, session R).

## 2. Where the CPU goes (tick-mode accounting, syscost)

Per-dump samples (pa1 CPU0 and pa3 both CPUs; the two CPUs track within
a few % — pa1's late dumps that showed only c0 were print interleaving,
not a frozen vCPU):

| t | user | kernel | idle |
|---|---|---|---|
| pa1 60 s | 6.9% | 81.7% | 11.4% |
| pa1 900 s | 12.6% | 86.0% | 1.2% |
| pa3 780 s | c0 12.2% / c1 12.1% | c0 87.0% / c1 86.8% | c0 0.8% / c1 1.1% |

The kernel is CPU-saturated in kernel mode for the whole boot. In the
reference, the same image spends most CPU in userspace (services doing
real work); a 10.7 s boot cannot spend 85% of 2 vCPUs in kernel.

Kernel-rip page histogram (stable across pa1/pa3 despite different
binary layouts — pages verified by symbolizing each run against its own
ELF):

| page (pa1) | share of kernel ticks | contents (nm-verified) |
|---|---|---|
| 0xffffffff80659000 | **32%** | `__do_softirq` + softirq helpers + `enable_irq`/`disable_irq`/`restore_irq` + monowatch |
| 0xffffffff80458000 | 9% | `Atomic::fetch_update` instantiations: virtio queue CAS, `PageCache` put/flush RMWs |
| 0xffffffff80728000 | 4% | `alloc::string` machinery (path copies, fmt) |
| 0xffffffff80459000 | 4% | `atomic_store` glue |
| 0xffffffff806b5000 | 3% | `Vec<...>::drop` glue (allocation churn) |
| 0xffffffff805e9000 + 0x805e5000 + others | ~8% | atomic CAS/load/store + `RawSpinlock::lock` |
| 0xffffffff8021e000 | 2% | `TwoWaySearcher` (string scanning) |

The 32% page needs careful reading (pa3 finding): the softirq *handler*
time is measured separately (`WW_SOFTIRQ_TICKS`) at only ~0.4% CPU
(912 ms over 420 s). The ticks that land in this page are ticks that
**could not fire earlier**: the timer interrupt was masked while the CPU
was inside an IF=0 region, and it lands in the first window where IF
returns to 1 — which is `enable_irq`/`restore_irq` on the *same page*
as `__do_softirq`. So the 32% is not "the softirq loop burns CPU by
itself"; it is **a quarter to a third of kernel-mode wall time executes
with interrupts disabled**, centered on (a) softirq dispatch at irq-exit
(the whole `__do_softirq` loop runs at IF=0 inherited from the IRQ
frame) and (b) every `irqsave` critical section long enough to be
sampled. Consistent evidence:

- 395 printed IRQ-OFF-WINDOW events in pa1 (print gate = 1 per 2 s per
  CPU; near saturation => a qualifying window every ~4 s), 388 of them
  >= 1 s, median 1000 ms, max 2001 ms.
- Their symbolized recovery rips: 143x `enable_irq+6`, 16x `outb`, 8x
  `inb` (UART TX with IF=0), `memcmp` (string scan inside a cli
  section), `atomic_load` (spinlock polling acquired under irqsave).
- Timer consequences: `nanosleep` average 2.6-3.0 s (max 295 s) per
  call across the boot — sleep timers fire seconds late because ticks
  queue behind cli windows; every FFRT timer wheel in OH inherits this.

## 3. Syscall cost table (pa1, cumulative t=1500s)

560M calls, 146 kinds. Hot by count (the flood):

| nr | name | calls | rate | avg | total wall | attribution (pa3) |
|---|---|---|---|---|---|---|
| 0 | read | 322M | 215K/s | ~1.6 us | 516 s | **hilogd.rd_kmsg = 94K/s of them**; rest spread |
| 228 | clock_gettime | 114M | 76K/s | <1 us | 58 s | (vDSO absent; still a syscall each) |
| 20 | writev | 44.6M | 30K/s | 14 us | 655 s | logging writes, many comm |
| 47 | recvmsg | 30.6M | 20K/s | 4 us | 129 s | **hilogd.server = 395K in 60 s** + sysevent_source |
| 45 | recvfrom | 29.8M | 20K/s | 54 us | 1613 s | **hilogd.server** (same socket drain) + mmi_service |
| 42 | connect | 7.4M | 5K/s | 65 us | 487 s | socket reconnect storm (dead-peer retries) |
| 281 | epoll_pwait | 2.05M | 1.4K/s | 13 ms | 26,848 s | event loops (blocked; normal shape) |
| 202 | futex | 1.7M | 1.1K/s | 118 ms | 201,347 s | FFRT pools + binder sync (blocked) |

Hot by per-op latency (pa1 averages; Linux norms are order-of-magnitude
references for the same operation on tmpfs/ext4-COW):

| op | Rux avg | max | Linux norm | factor | calls/boot | total |
|---|---|---|---|---|---|---|
| fsync | 90.5 ms | 7.0 s | 1-5 ms | 20-90x | 199 | 18 s |
| fdatasync | 59.8 ms | 3.0 s | 0.5-2 ms | 30-100x | 1,640 | 98 s |
| bind (sock) | 97.6 ms | 2.0 s | ~10 us | 10^4x | 41 | 4 s |
| unlink | 57.6 ms | 2.0 s | ~50 us | 10^3x | 330 | 19 s |
| ftruncate | 53.5 ms | 6.0 s | ~20 us | 10^3x | 673 | 36 s |
| execve | 51.9 ms | 3.0 s | 2-5 ms | 10-25x | 270 | 14 s |
| pwrite64 | 18.3 ms | 6.0 s | 50 us-1 ms | 20-300x | 14,127 | 258 s |
| fork/clone | 27 ms | 4.0 s | 0.1-1 ms | 30-200x | 703 | 19 s |
| mkdir | 2.9 ms (28.9 ms in pa1 t=660 phase) | 3.0 s | ~50 us (journaled) | 60-600x | 51,469 | 149 s |
| mount | 5.1 ms | 1.0 s | ~100 us | 50x | 12,145 | 62 s |
| chmod/chown | 7-8 ms | 3.0 s | ~20 us | 400x | 5,219 | 42 s |
| getdents64 | 0.5 ms | 2.0 s | 2-4 us | 100-250x | 34,951 | 17.5 s |
| pread64 | 1.75 ms | 3.0 s | 5-20 us | 100-350x | 32,634 | 57 s |
| open | 271 us | 6.0 s | 1-2 us | 100-200x | 467,657 | 127 s |
| newfstatat | 277 us | 3.0 s | ~1 us | 200x+ | 179,119 | 50 s |
| munmap | 163 us | 1.0 s | ~5 us | 30x | 1.09M | 179 s |

Notes:
- Wall-time averages include blocking; maxes show where scheduling
  delays land (6 s pwrite, 7 s fsync). The per-op *medians* for
  metadata ops are the 100-200x story; the maxes are family F3.
- `connect` at 7.4M calls is a *userspace* retry storm against dead
  peers — each retry pays a full kernel socket path.
- `mount` at 5.1 ms avg on main (which still has the linear registry
  scan — the launcher lane's index fix `8cd4f2c0` is not merged into
  main yet) is already far below the 1.7-1.9 s *per sandbox phase*
  telemetry, because a phase is dozens of mounts plus other work.

## 4. Ranking table (cost centers -> boot-time contribution)

Contribution model: 2 vCPUs x 1500 s = 3000 CPU-s observed; kernel mode
~85% = ~2550 CPU-s. "Gap contribution" = my estimate of how much of the
16-23 min -> 10 s gap this center explains directly.

| # | cost center | per-op | calls/boot | share of kernel CPU / wall | gap contribution | family |
|---|---|---|---|---|---|---|
| 1 | Interrupt-disabled regions (softirq-at-irq-exit loop + irqsave sections + UART TX cli) | 388 events >=1 s; ticks-behind-cli 25-32% of kernel ticks | continuous | ~25-32% of kernel-mode CPU; causes all timer lateness | **large** — multiplies everything (timers, sleeps, watchdogs) | F1 |
| 2 | Scheduler wake-loss (WAKE-RUN-LEAK: lying busy slots + linked-but-starving) | median 252 s age per episode | 448 episodes/boot | latency, not CPU | **large** — the direct maker of the 16-23 min (minutes-long service stalls serialize the dependency chain) | F3 |
| 3 | Logging syscall flood (hilogd.server socket drain + hilogd.rd_kmsg spin + writev) | 4-54 us per call | ~130M calls (read+recv*2+writev) | ~0.5-1.0 vCPU sustained | medium-large — burns a vCPU half, saturates softirq/wake paths | F2 |
| 4 | ext4 metadata + write path (open/stat/getdents/fsync/pwrite/mkdir) | 100-300x Linux | ~700K calls | ~400-600 CPU-s | medium | F4 |
| 5 | Softirq dispatch overhead proper (v1 tcp_timer + v8 hrtimer raised every tick per CPU = 400 entries/s; v4 block 98/s) | loop machinery | 125K entries/420 s | small (handlers 0.4% CPU) | small once F1's cli accounting is separated | F1 |
| 6 | Page-cache/virtio CAS contention (fetch_update family) | - | - | 9-15% of kernel ticks | medium (I/O completion path) | F5 |
| 7 | Allocation churn (Vec drop + grow + small_alloc) | - | - | ~5% kernel ticks | small-medium | F5 |
| 8 | Mount registry scan (linear, unindexed on main) | 5.1 ms/mount whole-syscall | 12,145 | 62 s | small on main (fix already on launcher lane) | F4 |
| 9 | IPC global epoch / WORLD lock (binder) | not directly measured this run | 598 SAREG + ~100K txns | folded into #1/#6 | small-medium (needs a binder-stat run to split out) | F5 |

## 5. Root-cause families (adjudicating the hypothesis)

The brief's candidate list was: global single-lock family; O(n) scan
family; per-page/per-byte family; IRQ-off-window family; TCG/KVM
specific amplification. Measured result on current main:

- **F1 IRQ-off windows — CONFIRMED, now the #1 CPU-side family.** But
  the composition changed from the historical picture (MOUNT_TABLE
  irqsave, PTE_MODIFY_LOCK, printk THRE are all fixed): what remains is
  the softirq-at-irq-exit discipline itself plus residual cli sections
  in the UART TX path (`outb`/`inb` with IF=0) and string-scan/atomic
  regions. The tick-sampling artifact (all queued ticks attribute to
  the re-enable page) explains why it shows as 32% "softirq page".
  pa3 softirq vec counts add the composition detail: v1 (tcp_timer)
  and v8 (hrtimer) are raised on *every* timer tick of *every* CPU
  (199/s each — v1 for a usually-empty TCP table), v4 (block) 98/s
  early then ~0 once I/O settles; total loop entries 298/s; handler
  bodies only 0.4% CPU (WW_SOFTIRQ_TICKS 1369 ms over 780 s) — the
  cost is the dispatch discipline and its cli window, not the handlers.
- **F2 Flood amplification loop — CONFIRMED (new in the ranking).**
  Slow answers -> userspace retries -> per-call kernel cost x volume.
  hilogd.rd_kmsg alone: 72.2M reads over 780 s (92.6K/s sustained for
  the whole boot, ~15-25% of a vCPU). hilogd.server: 395K socket
  receives in the first 60 s alone. connect-retry storm: 7.4M calls.
  None of these exist at that rate on the Linux reference boot (which
  finishes before such loops matter). This is the "systemic"
  multiplier that makes every per-op cost count double.
  Attribution status of the hilogd.rd_kmsg read flood (pa4-pa8 hunt):
  the reads do NOT go to /dev/kmsg (its read counters stay 0 for the
  whole boot), do not return EOF (fd-path census), do not return
  EAGAIN (census) — they return data, ~92K times/s, from some other
  fd. Remaining step: one taskdump of that thread's fdtable while the
  flood is live (KILLDUMP already prints fd tables). Side finding
  from the same hunt: userspace writes to /dev/kmsg go through a
  `pr_info!` passthrough that loglevel 4 drops BEFORE the ring —
  OH's boot markers written this way never reach any hilog consumer
  (correctness bug, trivial fix: route the passthrough at KERN_INFO
  into the ring unconditionally, console-gated).
- **F3 Scheduler wake-loss — CONFIRMED as the dominant *wall-clock*
  family.** 448 WAKE-RUN-LEAK episodes (kicked >=2x, never picked),
  median 252 s, p90 467 s, max 520 s. The W3-RUNWAKE-REFUSED census
  splits them: 20 measured refusals with `curr_on=true on_cpu=true`
  (a lying busy slot — the task switched out but the CPU slot/on_cpu
  bookkeeping never cleared, so wake_up refuses forever) and 6 with
  `linked=true` (queued but never picked — pick-path starvation).
  This family is why per-service phases take minutes: one leaked
  server thread stalls a whole dependency chain until something heals
  it. It is also *caused* by F1 in part (a wake issued while the
  target CPU is cli-bound gets dropped/raced), forming a loop.
- **F4 ext4/metadata per-op family — CONFIRMED, #1 in per-op factor
  (100-300x), medium in total.** open 271 us, newfstatat 277 us,
  getdents 501 us, fsync 90 ms. Combined ~600 CPU-s/boot. The mount
  registry scan (O(n)) subcase is real but already fixed on the
  launcher lane; the deeper cost is the name-lookup/journal path.
- **F5 Global-lock/atomic contention — CONFIRMED as a component, not
  the top.** 9-15% of kernel ticks in `fetch_update` CAS loops
  (virtio completion + PageCache RMW), plus `RawSpinlock::lock`
  pages ~2%. The binder WORLD/epoch lock did not show as a hot page
  this run (needs a binder-focused run to quantify separately).
- **F6 TCG/KVM-specific — NOT a factor for the OH lane** (all
  measurement and the reference are KVM; the vDSO-absence cost is
  already amortized by the TSC timekeeper fix — 114M clock_gettime at
  <1 us each).

The per-page/per-byte family, historically the biggest (1.5 s/file
fault), is verified fixed (see section 6).

## 6. Already-fixed items: effect verification (no re-fix)

| fix (commit) | before (measured then) | now (pa1/pa3) | verdict |
|---|---|---|---|
| UART THRE budget 1s->250us (29a49444) | sc166: 2300 IRQ-off + 2503 softirq-storm + 2380 WAKE-STRANDED lines fed by probe prints | TIMER-SOFTIRQ-STALL 7/boot; WAKE-STRANDED 744/boot (sprobe prints, not THRE-bound) | holds |
| ksoftirqd wake at dispatch skip (aa742f18) | tick_age=0ms dispatch starvation for seconds | 1 borderline line/boot | holds |
| per-mm PTE locks (681da2da) | 470-615 s single-task fork-walk monopolies | none in rip table; fork avg 27 ms still high but not monopolistic | holds |
| buddy bitmap marks (a80974df) | order-18 = 262K iterations, terminal 15-min freeze | small_alloc pages ~1% of ticks | holds |
| fault-around x2 + filemap cache mapping (6782020e, 1b742d80/798b4cb3, dc096f14) | 802 s single .so load (1.5 s/file fault); 1727 OOM faults (sc176) | 0 OOM lines in pa1/pa3; pread avg 1.75 ms; SA chain completes; deepest boots ever | holds |
| TSC timekeeper default (753c834b, 6529f06a, 9d595efa) | clock_gettime = HPET MMIO VM exit; launcher stime 1690-1766 ticks in hpet page | 114M calls at <1 us; no hpet page in top-20 | holds |
| MOUNT_TABLE preempt-off (502ea7a7) | >=1 s cli sections in register_mount | mount avg 5.1 ms whole-syscall; residual cli windows now UART/memcmp-shaped | holds |
| unix truesize (5157c305) | writev parked whole 3 s budgets (183 stime ticks) | writev avg 14 us | holds |
| sticky one-shot wake re-issue (385c9793) | consumed wake tokens, tasks parked for minutes | REKICK now re-issues; the *residual* WAKE-RUN-LEAK (448/boot) is the next member of this family | partially — family continues |

## 7. Fix roadmap (ROI order, with lane routing)

1. **Wake-loss heal + pick adjudication (sched).** The W3 census gives
   the two exact shapes: `curr_on&&on_cpu` lying slots (need a
   pick-side stale-ownership sweep, same spirit as the R85
   stale-claim heal e0902a27) and `linked` starvation (pick-path
   geometry). Expected effect: removes minutes-long stalls — the
   largest wall-clock term. Lane: scheduler is currently unowned;
   route to main session for assignment (wildwrite lane owns adjacent
   sched forensics). First step is cheap: extend the W3 print to dump
   the lying slot's owner timeline.
2. **Shrink irq-exit softirq cli time (interrupt/timer).** Two
   independent pieces: (a) stop raising the tcp_timer softirq (v1)
   on every tick when the TCP table is empty (199 raises/s/CPU for
   nothing — one-line gate in the tick path); (b) run the softirq
   loop with IRQs on after the first iteration (Linux
   local_bh_enable discipline) or move dispatch to ksoftirqd when
   the loop exceeds one handler pass. Expected effect: recovers a
   large part of the 25-32% cli-bound kernel time and all of the
   timer-lateness amplification (nanosleep 2.6 s -> ms-class).
   Lane: interrupt/softirq is shared-stable; small diffs, main
   session coordinates.
3. **Kill the hilogd.rd_kmsg spin (printk/dfx).** 92.6K read()/s
   sustained for the whole boot, ~15-25% of a vCPU. Attribution
   narrowed to "positive-returning reads on a non-kmsg fd" (pa4-8);
   the remaining step is one fdtable dump of that thread mid-flood
   (KILLDUMP prints fd tables; or a one-line fd-path latch in the
   read fast paths). Also fix the /dev/kmsg write passthrough
   (loglevel drop swallows OH's boot markers — one-line fix).
   Lane: this perf-audit line can take both (no conflicts; the
   printk.rs kmsg region is not contended by other lanes right now).
4. **ext4 metadata path profiling (fs lane).** open/stat at 100-200x
   is the largest pure per-op factor. Likely dcache/name-lookup
   shape (String allocations visible at 4-6% of ticks) rather than
   disk (KVM virtio is fast). Needs its own instrumented hunt; the
   launcher lane's mount-index experience is the template.
   Lane: fs is launcher-lane-adjacent; route with #4 of their queue.
5. **Page-cache/virtio fetch_update contention (block layer).** 9-15%
   of kernel ticks in CAS retry loops around virtio queue state and
   PageCache counters. Batching or per-CPU counters. Lane: virtio
   completion path is historically bug-dense — main session must
   assign deliberately (lock-ordering doc applies).
6. **In-flight (launcher lane, do not duplicate):** mount-registry
   index (8cd4f2c0, verified flat 12-25K cyc vs 345K-13.5M linear),
   anon fault-around, futex pool 8192 — all on `feature/samgr-callback`,
   unmerged into main at audit time.
7. **Not worth a lane right now:** allocation churn (~5%), String
   glue (~6%) — they fall out of #4/#5's callers; printk UART
   emission (known printk-wedge lane territory, small at quiet boot).

## 8. Tools landed (this branch)

- `kernel/src/dfx/syscost.rs` — cmdline `syscost=1` (own key, no
  dfx=switches.rs bit): per-syscall {n, wall, max}, >4 ms slow ring
  with comm, per-CPU tick modes (user/kernel/idle), 256-slot kernel-rip
  page histogram, softirq vec counts, flood-nr per-comm attribution,
  kmsg read counters + EOF/EAGAIN read censuses (fd paths).
  60 s dumps from the timer softirq. Both arches; zero cost when off.
  Diagnostic caveat: the read censuses add ~5 us to each read that
  returns 0/EAGAIN while syscost=1 (fd re-resolution); flood-table
  per-op numbers for read come from the pa3 binary (census-free).
- `kernel/src/printk.rs` — kmsg read counters (records/EAGAIN/sleeps/
  first-reader flags) for the spin hunt.
- `run/pa-boot.sh` — quiet-OH measurement boot (snapshot shared disks,
  private reflink userdata) + host-side serial timestamping.
- `run/patstamp.py` — host-side serial-line timestamper.
- `run/pa-parse.py` — offline parser (decodes native x86_64 nrs via
  the map extracted from x86_compat.rs; symbolizes rip pages via nm).

Known measurement caveats (in the data, not hidden):
- Syscall wall includes blocking; sums across tasks exceed CPU time by
  design. The tick modes and rip histogram are the CPU-side truth.
- Page-granular rip attribution + the cli-window bias (all queued ticks
  land at the re-enable site): interpret "hot page" as "time ended
  with IF returning to 1 here" when the page contains enable_irq.
- Diverging syscall paths (exit_group) skip the exit hook: those calls
  are undercounted.
- Dump lines can interleave with concurrent WW prints at char level
  (pa2 lost whole lines this way; pa3 ran `sprobe=0`).
- pa1/pa3 ran on a host shared with 2-3 foreign QEMUs; per-second
  rates carry that noise, structure does not.

## 9. Reproducing

```bash
cd /home/william/rux-agents/perf-audit
PLATFORM=x86_64 make build          # bzImage with syscost
./run/pa-boot.sh pa4                # 26-min quiet boot, syscost=1
./run/pa-boot.sh pa5 ... APPEND_EXTRA="syscost=1 sprobe=0"  # quiet variant
python3 run/pa-parse.py run/pa-pa4-stamped.log --nm <(nm target/x86_64-unknown-none/debug/rux | sort)
```
