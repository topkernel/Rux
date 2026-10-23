# Rux kernel microbench report — OH boot (v1.0, pre-UI)

Date: 2026-10-24. Lane: `feature/perf-audit`. This is the consolidated
deliverable of the systematic performance audit (working log with the full
evidence trail: `docs/development/perf-audit.md`).

Status: every number below is measured and final for the pre-UI kernel.
The only pending addition is the **last row of table 1** (post-UI-boot
milestones), to be filled when the UI milestone is reached; nothing else
in this report needs to change at that point.

---

## 0. Environment and method

- Guest: OpenHarmony x86_64 image, QEMU q35, KVM, `-cpu max -m 4G
  -smp 2` — identical machine config for Rux and the Linux reference
  boots.
- Rux kernel: main@0634bc8f (pa17 measurement boot; the launcher lane's
  wake-loss fixes v1 a39d2bc0 + v2 6e9679c5 included) plus the lane's
  measurement probe (`syscost=1 sprobe=0`). Baseline comparisons cite
  earlier boots on their own kernel revisions (pa1/pa3 pre-fix,
  pa13/pa15/pa16 progressively fixed) — each run's revision is listed
  in `perf-audit.md`'s run table.
- Instruments: per-syscall {n, avg, max, total-wall} accounting;
  per-CPU tick-mode counters (user/kernel/idle); EFLAGS-exact IF=0
  zone accounting (v3: edge-triggered, self-healing, adjudicated
  against the real flags, REST sections attributed to file:line via
  `#[track_caller]`); kernel-rip page histogram (symbolized per-run
  against the ELF actually booted); per-comm printk/recv/UART tables;
  host-side serial timestamping (`run/patstamp.py`).
- Linux reference: same image, same machine config, reference kernel
  boot `oh-ref-r1` (`/home/william/rux-agents/oh-linux-ref/runs/`).
- Host context: the measurement host is shared with foreign-lane QEMUs
  (load ~6.7 during pa17). Absolute wall numbers carry that noise;
  ratios and structure are stable across pa13/pa15/pa16/pa17. Where a
  number is host-load-sensitive the text says so.

What this report is: a boot-time microbench of the quiet-OH-boot
workload (780 s horizon, milestones measured to t=780, tables dumped
at t=720). It is not a general benchmark-harness result; the numbers
describe this workload.

---

## 1. Boot phase timeline — Rux vs the Linux reference

| event | Linux t (oh-ref-r1) | Rux t (pa17) | Rux band (pa1-pa17) | factor |
|---|---|---|---|---|
| kernel start -> /init running | 1.40 s | 0.85 s (pa1) | ~0.9 s | 0.6x (Rux faster: no device-probe storm) |
| first ServiceExec | 1.84 s | ~4-5 s | ~4-5 s | ~2.5x |
| hdf_devmgr SA-5100 registration | 2.90 s | 6.4 s | 6-7 s | 2.2x |
| bootevent.appspawn.started | 4.81 s | 60.2 s | 32-66 s | 6.6-13x |
| bootevent.composer_host.ready | 4.89 s | 31.4 s | 31-44 s | 6.4-9x |
| bootevent.launcher.ready | 10.12 s | not reached in 13 min | never reached | >150x |
| bootevent.bootanimation.finished | 10.73 s | not reached | never reached | >145x |
| **post-UI kernel, same milestones** | — | **PENDING (fill at UI milestone)** | — | — |

Reading. Divergence grows with boot depth. Early services are only
2-6x slower; everything downstream of appspawn runs 8x to >100x
slower, and the two final milestones are never reached inside the
measurement horizon. A uniform per-op slowdown would keep the ratio
flat; a growing one is the signature of a *latency multiplier*
stacked on top of per-op costs — scheduler wake-loss (family F3,
section 3 below) multiplying every dependency chain, on top of the
per-op families. The 0.6x on the first line is real, not noise: Rux
skips the Linux device-probe storm and reaches /init first.

Fix effect visible here. The /proc/kmsg blocking-stream fix (pa9-pa12)
cut total syscall volume -30% and read() -113x, but milestones stayed
inside the same variance band (pa13: composer 44.3 s / appspawn
65.9 s, in-band) — CPU-side relief alone does not move the phase
timeline while the wake-loss multiplier is active. That is why the
ranking (section 3) puts wake-loss first for wall-clock.

Known limitations. Milestones are single-run samples per boot; the
band column shows the observed range across pa1/pa3/pa12/pa13/pa16/
pa17. Host load moves single milestones by up to ~2x inside that band;
the "never reached" verdicts are horizon-limited (13 min), and full
16-23 min boots from earlier sessions did reach the later phases.

---

## 2. Syscall volume and per-op latency (pa17, cumulative t=720 s)

### 2.1 Volume leaders — the flood

| syscall | calls | avg | max | total wall |
|---|---|---|---|---|
| clock_gettime | 33.3 M | 4 us | 1.0 s | 133 s |
| writev (logging) | 14.6 M | 24 us | 1.0 s | 357 s |
| recvfrom (hilogd PEEK) [a] | 10.08 M | 116 us | **393 s** | 1176 s |
| recvmsg (hilogd consume) | 10.29 M | 9 us | 1.0 s | 98 s |
| connect (dead-peer retry) | 2.47 M | 98 us | 1.0 s | 244 s |
| epoll_pwait (event loops) | 2.16 M | 11.6 ms | 180 s | 25,179 s (blocked) |

[a] **hilogd.server PEEK — OH-side overhead footnote.** The recvfrom
half of the hilogd.server dual-drain is a blocking
`recvfrom(MSG_PEEK)` returning exactly 2 bytes, followed 1:1 by a
`recvmsg` that consumes the record (74 B avg; 702 MB of records per
boot through one unix dgram socket, paced by ~12 K real records/s
from the writev storm of 100+ services). The PEEK call itself is
pure OpenHarmony userspace protocol overhead — one extra full socket
recv path per log record, ~10 M syscalls/boot — with no kernel bug
underneath (0 EAGAIN, per-call cost 116 us mostly parked on the
socket wait queue; the 393 s max is the wake-loss tail, family F3,
not a recv cost). Kernel-side lever: per-call recv cost. OH-side
lever (their code): drop the PEEK.

Reading. Total 80.7 M syscalls by t=720 (112 K/s on 2 vCPUs), ~60% of
it logging traffic. The volume leaders are OH userspace behavior
(retry loops, logging), amplified by kernel per-call costs; each row
is a "userspace volume x kernel unit cost" product. The blocked rows
(epoll_pwait, and recvfrom's max) are where the wake-loss tail lands:
healthy-path averages are microseconds, the maxes are
hundreds-of-seconds class. The Linux reference boot finishes in 10.7 s
and never develops any comparable loop.

### 2.2 Per-op latency (medians are the honest per-op cost; maxes are the wake-loss/host-stall tail)

| op | Rux avg (pa17; pa16 in parens) | Rux max | Linux norm | factor (median) | calls (pa17) |
|---|---|---|---|---|---|
| newfstatat | 294 us | 3.0 s | ~1 us | ~200x | 179 K |
| getdents64 | 1043 us (789) | 2.0 s | 2-4 us | 100-250x | 33.6 K |
| open | avg 1893 us, stall-inflated (pa16 avg 400 us) | 100 s | 1-2 us | 100-200x | 259 K (217 K) |
| unlink | 114.7 ms (117.2) [b] | 2.0 s | ~50 us | ~10^3x | 192 |
| mkdir | 9.6 ms @16.4 K calls (pa16: 62 ms @ 940 — phase mix differs) | 3.0 s | ~50 us | 60-600x | 16.4 K (940) |
| chmod | 11.1 ms (5.8) | 1.0 s | ~20 us | ~400x | 2.9 K |
| fsync | 44.8 ms (15.4) | 2.0 s | 1-5 ms | 20-90x | 67 |
| fdatasync | 39.6 ms | 2.0 s | 0.5-2 ms | 30-100x | 1.6 K |
| pwrite64 | 14.7 ms (25.8) | 2.0 s | 50 us-1 ms | 20-300x | 13.6 K |
| pread64 | 10.9 ms (5.2) | 125 s | 5-20 us | 100-350x | 19.2 K |
| munmap | 166 us | 1.0 s | ~5 us | ~30x | 707 K |
| futex (blocked waits) | 351 ms | 465 s | — | wake-loss tail | 284 K |
| nanosleep | 2.2 s | 295 s | — | tick-lateness tail | 5.5 K |

[b] **unlink — fs backlog item.** The unlink average (114.7 ms pa17 /
117.2 ms pa16 — stable across boots) belongs to family F4, the ext4
metadata + write-path family (roadmap item 4: ext4 metadata path
profiling, routed to the fs lane). It is a known, tracked backlog
item, not an unexplained anomaly: the whole metadata row set
(open/stat/getdents/unlink/fsync) is the same family, with the same
suspected dcache/name-lookup/journal shape; the fs lane's mount-index
fix is the template for the hunt. 192 calls/boot, so its total wall
contribution (22 s) is small even at 10^3x — the family matters in
aggregate (~300-500 CPU-s/boot), not through unlink alone.

Reading. The metadata rows (newfstatat through pread64) are the pure
per-op cost story: 100-300x Linux norms at the median, with the same
family signature across all of them. The last two rows are not per-op
costs at all — futex 351 ms and nanosleep 2.2 s averages are blocked
time inflated by the wake-loss and timer-lateness tails; they are the
same F3 multiplier seen from the syscall side. Wall averages include
blocking: an avg can be dominated by a few stalled calls (open's pa17
1893 us avg vs pa16's 400 us is a heavier stall tail that boot, not a
kernel regression — see limitations).

Before/after (fixes already in these numbers):

| fix | before | after (measured) |
|---|---|---|
| /proc/kmsg blocking stream + /dev/kmsg write ring (this lane) | rd_kmsg EOF spin 93.3K read/s, ~15-25% of a vCPU; boot markers dropped pre-ring | read() -113x (93.3K/s -> 0.82K/s), total syscalls -30%, 772 [kmsg] markers delivered |
| unix-segment truesize (5157c305) | writev parked whole 3 s budgets (183 stime ticks/message storm) | writev avg 24 us (pa17) |
| TSC timekeeper (753c834b..9e5f0147) | clock_gettime = HPET MMIO VM exit each call | 33.3 M calls at 4 us avg |
| anon fault-around + filemap cache (6782020e, 1b742d80/798b4cb3, dc096f14) | 802 s single .so load; 1727 OOM faults (sc176) | 0 OOM lines; pread64 completes at 10.9 ms avg incl. stall tail |
| per-mm PTE locks (681da2da) | 470-615 s single-task fork-walk monopolies | none observed pa13-pa17 |
| MOUNT_TABLE preempt-off (502ea7a7) | >= 1 s cli sections inside register_mount | mount syscall avg 5.1 ms, no residual mount-shaped cli windows |

---

## 3. Cost-center ranking v2.3 — the fix list in order

Contribution model: 2 vCPUs, kernel mode 86-89% of ticks (pa17 t=720:
89.4%/88.4%; user 9-10%; idle 1-3%).

| # | cost center | direct evidence (pa15-pa17, EFLAGS-exact) | share | family / lane |
|---|---|---|---|---|
| 1 | IRQ-off regions total | IF=0 = 23.4-25.0% of machine; split REST 88.8-92.0% / HARDIRQ 5.1-7.7% / SOFTIRQ 1.6-1.7% / UART 1.4-1.9%; **max closed IF=0 segment 4-5 ms — no long cli section exists; it is 1.26 M short sections/s** | 23-25% | F1 |
| 1b | REST bulk, by site | small-alloc locks 12.2% (~55% of REST, 732-740 M sections/boot; B lane, per-CPU freelist direction); schedule() GRQ 2.7% (**-31% wall after launcher v2: 56.6 -> 38.7 s**); fdtable get_file 1.9-2.0%; waitqueue family 1.8-2.2%; EPOLL_WAKE_REGISTRY 1.0-1.1%; deferred_free 0.7% — 97.3-99.3% attributed to file:line | 20.8-23.0% | F1/F5 — the lever list |
| 1a | Console/UART emission | per-byte cost stable 15.5-18.5 us across 3 boots (one LSR poll per byte = one inb+outb KVM-exit pair); volume swings 1.55-5.61 M bytes/boot -> machine share 1.7-6.1%; producers 2/3 OH userspace via /dev/kmsg; task-context emission 100% IF=1 | 2-6% + seconds-class stall tail | F1 — console-async = highest-latency single fix |
| 2 | Scheduler wake-loss | WAKE-RUN-LEAK 593 prints / 780 s; 31 refusals (22 lying busy slot + 7 linked, never picked); recvfrom max 393 s and futex avg 351 ms are this family's syscall-side face; launcher v1+v2 merged — census unchanged, their trigger shape absent this boot (TERM-DESCHED=0); lying slots born on another path, birth forensics continues | **large — still the wall-clock driver** | F3 (launcher lane) |
| 3 | hilogd dual-drain + retry loops | ~25% of all syscall volume (section 2.1); volume is OH userspace | medium | F2 — kernel levers small (per-call recv cost) |
| 4 | ext4 metadata + write path | section 2.2 rows, medians 100-300x Linux | medium (~300-500 CPU-s/boot) | F4 (fs lane) |
| 5 | Atomic/CAS contention | glue page ~16-20% of kernel ticks; RMW owners named — they ARE the 1b sections | medium — folds into 1b | F5 |
| 6 | clock_gettime volume (no vDSO) | 33.3 M calls at 4 us — amortized by TSC timekeeper | small | F6 — closed |

Reading. The gap is not one hotspot; it is four stacked layers in this
order: (a) wake-loss stalls multiplying every dependency chain (the
phase-timeline driver), (b) 1.26 M irqsave sections/s — allocator and
lock volume, each section only ~250 ns to 6 us long, (c) userspace
logging/retry loops amplified by per-call kernel costs, (d) metadata
ops at 100-300x. The IRQ-off family (row 1) decomposes almost entirely
into per-site work (row 1b) — there is no single long critical section
to break up; the fixes are per-site (per-CPU freelists for the
allocator, pick-path work for the GRQ, RCU-ish reads for fdtable).

Ranking movement v1 -> v2.3 (what fixes moved). The read storm (v1's
#3, ~0.5-1.0 vCPU) is gone after the kmsg fix; its slot was taken by
the hilogd dual-drain, which was always underneath and spins faster
now that rd_kmsg stopped competing. The GRQ site fell 3.9% -> 2.7%
when the launcher's v2 pick-ownership release merged (single-boot
evidence, direction consistent with design intent). The softirq
empty-raise gates (pa14 A/B) measured NO effect on this workload —
both latch bits flip within the first 31 s and never re-engage; honest
negative result, the gates still pay on timer/TCP-sparse workloads.

Known limitations. Single-boot samples per kernel revision; shares are
of a 1440 CPU-s machine window at t=720; console share swings with
printk volume (1.7-6.1%) while per-byte cost is the stable quantity;
the GRQ -31% is one run against one baseline (host-load interacts with
hold times — treat direction, not the exact percent).

---

## 4. Linux comparison — what the same image does on Linux

- Total boot to launcher.ready: Linux 10.12 s; Rux never reaches it in
  13 min (16-23 min in earlier full runs).
- Syscall volume: Linux completes the boot before any retry/logging
  loop matters — no read/connect/recv storm exists at any comparable
  rate; Rux: 80-84 M syscalls by t=720 (112-116 K/s), ~60% logging
  traffic.
- CPU mode split: Linux spends most of this boot in userspace (services
  doing real work); Rux runs 86-89% of ticks in kernel mode — a 10.7 s
  boot cannot spend 85% of 2 vCPUs in kernel.
- Same image, same QEMU machine config, same host class; the reference
  kernel is not instrumented, so its numbers are timeline + topology,
  not per-op tables.

Conclusion (the one-sentence version): the Rux-vs-Linux boot gap is
dominated, in order, by (a) scheduler wake-loss stalls that multiply
every dependency chain, (b) 1.26 M short irqsave sections/s
(allocator + lock volume — no long cli section exists), (c) OH
userspace logging/retry loops amplified by kernel per-call costs, and
(d) ext4-family metadata ops at 100-300x Linux norms — and the fix
order that follows is wake-loss birth forensics (launcher lane),
allocator per-CPU freelists (B lane), console-async (highest-latency
single fix), then the fs metadata hunt.

---

## 5. Fix effect ledger (before/after, condensed)

Fixes already in main, verified by this lane's measurements (no
re-fix; full table in `perf-audit.md` section 6):

| fix | before (measured then) | after (pa13-pa17) | verdict |
|---|---|---|---|
| /proc/kmsg stream + /dev/kmsg ring (d931c345, cfc39329) | 93.3K read/s EOF spin; markers dropped | -113x reads, syscalls -30%, markers live | holds |
| unix truesize (5157c305) | writev parks whole 3 s budgets | avg 24 us | holds |
| TSC timekeeper (753c834b..9e5f0147) | HPET MMIO exit per clock_gettime | 4 us avg over 33.3 M calls | holds |
| fault-around + filemap (6782020e +) | 802 s .so load, 1727 OOM faults | 0 OOM lines; deepest boots ever | holds |
| per-mm PTE locks (681da2da) | 470-615 s fork-walk monopolies | none | holds |
| buddy bitmap marks (a80974df) | order-18 262K-iteration terminal freezes | allocator bitmap gone from rip table | holds |
| MOUNT_TABLE preempt-off (502ea7a7) | >= 1 s cli in register_mount | mount avg 5.1 ms syscall | holds |
| UART THRE budget 250 us (29a49444) | softirq-stall/WAKE-STRANDED storms fed by prints | 7 stall lines/boot class | holds |
| sticky wake re-issue (385c9793) | consumed tokens, minutes-long parks | re-issues; residual 593/boot leak prints remain | partial — family continues |
| launcher wake-loss v1+v2 (a39d2bc0, 6e9679c5) | pre-fix census 691 prints / 30 refusals | census 593/31 (in variance band); **GRQ hold -31% (56.6 -> 38.7 s)** | compatible; v1's rare shape absent from clean boots — acceptance needs their reproducer; lying-slot birth path still open |
| softirq empty-raise gates (69310bae) | (hypothesis: wasted raises) | no measurable effect on OH boot — latches flip by t=31 and never re-engage | negative result, recorded |

---

## 6. Known limitations

1. **Single-boot samples.** Every Rux number is one boot per kernel
   revision (780 s horizon, tables at t=720). Bands are cross-run
   ranges, not confidence intervals. Host-load-sensitive quantities
   (per-second rates, maxes, milestone times) carry a foreign-QEMU
   load of ~5.5-7 on the shared host.
2. **Wall vs CPU.** Syscall averages include blocking; per-task wall
   sums exceed CPU time by design. The CPU-side truth is the tick
   modes + rip histogram + zone accounting; where they disagree with
   wall averages (open's 1893 us pa17 avg vs 400 us pa16), the stall
   tail is the explanation, not a kernel cost change.
3. **Instrument bias.** syscost adds ~5 us to each 0/EAGAIN read while
   enabled; the SYSCOST dumps themselves printed ~52 KB of console
   (self-observation, included in the console numbers); dump lines can
   interleave with concurrent prints at char level (runs use
   `sprobe=0` to minimize this).
4. **Attribution granularity.** Rip attribution is page-granular and
   cli-biased (ticks queued behind IF=0 attribute to the re-enable
   site) — the EFLAGS-exact zone accounting (v3, heal 0.2%/miss 0) is
   the ground truth for IF=0 shares; the wall-clock-based WW age lines
   were demoted to triage hints after cross-lane calibration (they
   conflate IF=0 with host vCPU steal and tickless gaps).
5. **Undercounting.** Diverging syscall paths (exit_group) skip the
   exit hook; early-census tables are once-per-comm with 12 slots
   (saturation cost the pa4-8 runs a wrong conclusion once — the
   committed censuses are per-fd-following and do not saturate).
6. **Reference asymmetry.** The Linux reference boots are uninstrumented
   (timeline + topology only); "Linux norm" per-op figures in 2.2 are
   order-of-magnitude references for the same operations on
   tmpfs/ext4-COW, not measurements of the reference kernel.
7. **The launcher v1 acceptance gap.** TERM-DESCHED=0 on the clean
   census boot means the rare terminal-deschedule shape never ran; a
   clean boot cannot prove a fix for a shape it never sees. The -31%
   GRQ result is one run; A-line v3 (wfi-paced retries, c90f80b0) is
   in flight and will need its own retest boot when it merges.

---

## 7. Provenance and reproduction

- Working log, evidence trail, and per-run kernel revisions:
  `docs/development/perf-audit.md` (sections 2a-2d carry the per-run
  readings; the run table at the top lists every boot's kernel and
  boot args).
- Tools (committed on this branch): `kernel/src/dfx/syscost.rs`,
  `run/pa-boot.sh` (quiet measurement boot), `run/patstamp.py`
  (serial timestamping), `run/pa-parse.py` (offline parser + rip
  symbolization). Reproduction recipe in perf-audit.md section 9.
- Raw logs: `run/pa-pa*-stamped.log` in the lane worktree
  (`pa-pa17-stamped.log` is the primary source for this report's
  tables); Linux reference: `/home/william/rux-agents/oh-linux-ref/
  runs/oh-ref-r1/`.
- Post-UI completion: fill the pending row of table 1 from one
  measurement boot on the post-UI kernel (same method, same 780 s
  horizon); optionally refresh table 2/3 totals in one sentence if
  they shift materially — no structural change expected.
