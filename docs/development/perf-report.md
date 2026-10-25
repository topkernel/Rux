# Rux kernel microbench report — OH boot (v1.4, post-merge flagship)

Date: 2026-10-24. Lane: `feature/perf-audit`. This is the consolidated
deliverable of the systematic performance audit (working log with the full
evidence trail: `docs/development/perf-audit.md`).

v1.4 = v1.3 + the merged-main KVM flagship (pa27): the timer fix
(199f308c, byte-identical to the pa23-validated scratch/tickfix hunk and
to the A-line's 675095c0) merged to main at `e1e317f2`, and the official
post-fix numbers measured on it — IF=0 = 25.7% of machine (t=720,
EFLAGS-exact, in the pre-fix band modulo boot depth; composition moves:
SOFTIRQ 1.0-1.7% -> 3.1% of IF=0, the suppressed hrtimer dispatch now
real), nanosleep avg 2.2 s -> 221 ms on KVM (-10x), census counts hold
exactly (TERM-DESCHED 8 / WAKE-REFUSED 32 / WAKE-STRANDED 0), and the
wall-clock milestone verdict is now official: NOT moved beyond the band
(single-boot variance covers both directions; the deep-boot chain is
gated by F3 wake-loss and the binder 15/30/89 s cost classes, which the
tick fix does not touch). Row 2b closes as FIXED + OFFICIALLY MEASURED;
ranking v2.7; the pa26 preview is superseded wherever both exist.

v1.3 = v1.2 + the fifteenth session's tick-fix verification: the
mechanism behind the "1000 ms" family is CORRECTED (it was never the
timer IRQ's cadence — the LAPIC always fired at 100 Hz; `read_time()`
itself stepped once per second because 9e5f0147's shift-first
`(delta_tsc >> 32) * mult` collapses a 4.29 GHz TSC to one step per
2^32 ticks = 1.0008 s), the launcher lane's fix (u128
multiply-then-shift, all 4 sites) is verified by this lane on
main+fix-hunk (pa23): tripwire green, 99.9 ticks/s/CPU sustained,
WW/WAKE-STRANDED grids gone, nanosleep avg -21x, and the tick-mode
kernel share re-reads 80-83% at true sampling. Limitation 8 resolves;
per-op maxes and the nanosleep/futex tails re-baseline. Ranking v2.6
row 2b closes as FIXED (flagship IF=0 re-run on merged main pending
the coordinator merge at leg close).

v1.2 = v1.1 + the thirteenth session's tick-cadence adjudication: the
"1000 ms kernel-mode IRQ-OFF-WINDOW" family (A-line sc200-202 triage
item) is NOT interrupt-off time and NOT KVM-specific — it is the OH
lane's timer IRQ running at ~1 Hz effective (nominal 100 Hz), identical
under KVM and TCG (pa21/pa22). Consequences: the EFLAGS-exact IF=0
numbers all stand; every tick/wake-derived latency on this lane
(WAKE-STRANDED strands, sleep/wake durations, the 1.0 s maxes) is
quantized on a 1 s grid and gains a new attribution suspect. New
ranking row 2b (F7), new limitation item 8.

v1.1 = v1.0 refreshed by pa18 + pa20 (seventh session): the launcher
lane's full scheduler stack v1..v6 (merged 9bc3a5c8) retested in pa18,
and the wildwrite lane's per-CPU magazine allocator (merged 988e10c1)
measured before/after in pa18->pa20. Headline changes: TERM-DESCHED
acceptance demonstrated (fires ~8/boot, all survive); census lying-slot
population unchanged by everything; the magazine cuts global class-lock
sections -97.3% and small_alloc IF=0 wall -24.3% (12.2% -> 9.2% of
machine); the GRQ "-31% after v2" claim is corrected to volume-coupled.

Status: numbers below are measured through main@e1e317f2 (pa27, the
post-merge flagship; = the timer fix 199f308c on the pa20-era kernel);
pre-flagship rows cite main@988e10c1 (pa20) and main@9bc3a5c8 (pa18). The
only pending addition remains the **last row of table 1** (post-UI-boot
milestones), to be filled when the UI milestone is reached.
**Section 8 is the v1.5 draft skeleton** (leg 21): the design of the
post-UI microbench + same-host reference-kernel comparison layer the user has
mandated — probe list, delivery method, comparison protocol, and
placeholder tables. It contains no numbers yet; it exists so the
measurement can run the day the UI milestone lands on a main-class
kernel.

---

## 0. Environment and method

- Guest: OpenHarmony x86_64 image, QEMU q35, KVM, `-cpu max -m 4G
  -smp 2` — identical machine config for Rux and the reference kernel
  boots.
- Rux kernel: current numbers are main@e1e317f2 (pa27, the post-merge
  flagship; = timer fix 199f308c on the pa20-era kernel), main@988e10c1
  (pa20; = launcher v1..v6 scheduler stack + magazine allocator) and
  main@9bc3a5c8 (pa18; scheduler stack only, pre-magazine rung); the pa17
  rows cite main@0634bc8f. The tick-fix before/after is pa23 =
  main@78460742 + the launcher lane's timer fix hunk (pre-merge
  verification, scratch build 9a451bef); pa27 re-measures everything on
  the merged main. Every boot carried the lane's measurement probe
  (`syscost=1 sprobe=0`); each run's revision is listed in
  `perf-audit.md`'s run table. The quietcon console divert (v5) is
  boot-param-gated and deliberately NOT passed, so console behavior is
  identical across all compared boots.
- Instruments: per-syscall {n, avg, max, total-wall} accounting;
  per-CPU tick-mode counters (user/kernel/idle); EFLAGS-exact IF=0
  zone accounting (v3: edge-triggered, self-healing, adjudicated
  against the real flags, REST sections attributed to file:line via
  `#[track_caller]`); kernel-rip page histogram (symbolized per-run
  against the ELF actually booted); per-comm printk/recv/UART tables;
  host-side serial timestamping (`run/patstamp.py`).
- Reference kernel: same image, same machine config, the image's own
  boot `oh-ref-r1` (`/home/william/rux-agents/oh-linux-ref/runs/`).
- Host context: the measurement host is shared with foreign-lane QEMUs
  (load ~6-7 during pa16-pa20). Absolute wall numbers carry that noise;
  ratios and structure are stable across pa13-pa20. Where a
  number is host-load-sensitive the text says so.

What this report is: a boot-time microbench of the quiet-OH-boot
workload (780 s horizon, milestones measured to t=780, tables dumped
at t=720). It is not a general benchmark-harness result; the numbers
describe this workload.

---

## 1. Boot phase timeline — Rux vs the reference kernel

| event | reference t (oh-ref-r1) | Rux t (pa17) | Rux band (pa1-pa17) | factor |
|---|---|---|---|---|
| kernel start -> /init running | 1.40 s | 0.85 s (pa1) | ~0.9 s | 0.6x (Rux faster: no device-probe storm) |
| first ServiceExec | 1.84 s | ~4-5 s | ~4-5 s | ~2.5x |
| hdf_devmgr SA-5100 registration | 2.90 s | 6.4 s | 6-7 s | 2.2x |
| bootevent.appspawn.started | 4.81 s | 60.2 s | 32-66 s | 6.6-13x |
| bootevent.composer_host.ready | 4.89 s | 31.4 s | 31-44 s | 6.4-9x |
| bootevent.launcher.ready | 10.12 s | not reached in 13 min | never reached | >150x |
| bootevent.bootanimation.finished | 10.73 s | not reached | never reached | >145x |
| **post-UI kernel, same milestones** | — | **PENDING (fill at UI milestone)** | — | — |

pa27 (post-fix flagship, merged main e1e317f2, 780 s horizon):
composer_host.ready +29.8 s (fast edge of the 31-44 band), appspawn
.started +80.1 s (ABOVE the 32-66 band — both directions present =
single-boot variance), appfwk.ready set by +57.4 s, bms
.main.bundles.ready / bootanimation / launcher.ready NOT set at 780 s.
Verdict: the tick fix does not move the phase timeline; the driver
stays F3 + the binder cost classes.

Leg-18 note (sc209, A-line branch kernel with FXRS probes — context
only, not a pa-series measurement): **bootanimation.finished was SET
for the first time** (pid 888; appfwk.ready also set; screen shows the
animation test pattern). launcher.ready / OnForeground still 0. The
post-UI row stays PENDING until launcher.ready lands on a main-class
kernel.

Reading. Divergence grows with boot depth. Early services are only
2-6x slower; everything downstream of appspawn runs 8x to >100x
slower, and the two final milestones are never reached inside the
measurement horizon. A uniform per-op slowdown would keep the ratio
flat; a growing one is the signature of a *latency multiplier*
stacked on top of per-op costs — scheduler wake-loss (family F3,
section 3 below) multiplying every dependency chain, on top of the
per-op families. The 0.6x on the first line is real, not noise: Rux
skips the reference kernel's device-probe storm and reaches /init first. Leg-13
addition, leg-15 correction+fix, leg-17 official close: the 1 s
kernel-clock grid (row 2b / limitation 8, RESOLVED) was a second
latency multiplier on this same shape — deep-boot dependency chains
are the phases with the most timed waits, so each wait rounded up to
the 1 s grid. The fix is verified (pa23) and officially re-measured on
merged main (pa27): the per-op latencies it gated collapsed (nanosleep
avg 2.2 s -> 221 ms, -10x; clock_gettime max off the grid) but the
wall-clock milestones did NOT move beyond the band — the F3-vs-F7
multiplier split is now answered: F7 was real but secondary; the
phase-timeline driver remains F3 wake-loss plus the binder cost
classes (15/30/89 s, A-line chain).

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

(pa18/pa20 refresh: volumes within a few percent of these rows —
hilogd dual-drain 11.5/10.7-11.7/10.9 M, writev 15.6-16.0 M,
clock_gettime 35.5-37.3 M, connect 2.5-2.7 M; the new named shape is
ioctl 65-67 K calls at ~600 ms avg blocked on the display/fb path —
the UI-phase prelude. Full tables: run/pa18-parse.txt, run/pa20-parse.txt.)

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
hundreds-of-seconds class. The reference-kernel boot finishes in 10.7 s
and never develops any comparable loop.

### 2.2 Per-op latency (medians are the honest per-op cost; maxes are the wake-loss/host-stall tail)

| op | Rux avg (pa17; pa16 in parens) | Rux max | reference norm | factor (median) | calls (pa17) |
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
| nanosleep, post-fix (pa27, official) | **221 ms** | 295 s | — | **-10x vs pa17 pre-fix** (pa23 TCG t=60 read 88.5 ms vs 1.87 s) | 22.8 K |
| clock_gettime, post-fix (pa27, official) | **3 us avg over 42.6 M calls** | 288 ms | — | max is late-boot host-noise class, not the grid (pre-fix max 654 ms was grid-inflated) | 42.6 M |

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
per-op cost story: 100-300x reference norms at the median, with the same
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
| per-mm PTE locks (681da2da) | 470-615 s single-task fork-walk monopolies | none observed pa13-pa20 |
| MOUNT_TABLE preempt-off (502ea7a7) | >= 1 s cli sections inside register_mount | mount syscall avg 5.1 ms, no residual mount-shaped cli windows |

---

## 3. Cost-center ranking v2.7 — the fix list in order

Contribution model: 2 vCPUs, kernel mode 83-90% of ticks (pa20 t=660:
88.1%/83.2%; pa27 post-fix flagship t=720: 85.1%/83.4% at true 100 Hz
sampling — the 86-89% pre-fix figure was sparse-sampling
phase-weighted; user 13.5-14.8%, idle 1.4-1.8%).

| # | cost center | direct evidence (pa15-pa20, EFLAGS-exact; pa27 = official post-fix flagship) | share | family / lane |
|---|---|---|---|---|
| 1 | IRQ-off regions total | IF=0 = 23.0-25.0% of machine across pa16-pa20 (magazine moved it 23.7 -> 23.0); **pa27 official post-fix: 25.7% of machine at t=720 (REST 88.5% / HARDIRQ 7.6% / SOFTIRQ 3.1% / UART 0.8% of IF=0; heal miss=0) — in the pre-fix band modulo boot depth (pa27 volume is the series' deepest: 100.9 M syscalls vs pa17's 80.7 M); SOFTIRQ component 1.0-1.7% -> 3.1% of IF=0 = the suppressed hrtimer dispatch now real (softirq busy 2.5 s/420 s pre-fix -> 13.9 s/720 s, v1/v8 at the ~100 Hz/CPU dispatch ceiling)**; max closed IF=0 segment 3-6 ms — no long cli section exists; 1.08 M sections/s post-magazine | 23-26% | F1 |
| 1b | REST bulk, by site | small-alloc **MEASURED pa18->pa20: global class-lock sections 741.7 M -> 19.7 M/boot (-97.3%); REST wall 12.2% -> 9.2% of machine — the remaining 751 M local magazine irqsave ops (136-168 ns) are the design floor; the lever left is churn volume** (pa27 post-fix: small_alloc 4-site family 91.0 s = 6.3% of machine at the series' deepest volume); schedule() GRQ 2.7-4.6% pre-fix, **6.7% of machine in pa27 (96.1 s, 17.7 M holds @ 5.4 us avg — avg flat-to-better, hold COUNT ~2x pa16's 9.3 M: volume-coupled growth with boot depth and true-tick wakes, not a per-hold regression)**; fdtable get_file 1.9-2.2%; waitqueue family 1.9-2.3%; EPOLL_WAKE_REGISTRY 1.0-1.2% (launcher lane holds a sharding design); deferred_free 0.7-0.8% — 97-99% attributed to file:line | 20.7-23.0% | F1/F5 — the lever list |
| 1a | Console/UART emission | per-byte cost stable 13.2-18.5 us across 5 boots (one LSR poll per byte = one inb+outb KVM-exit pair); volume swings 1.34-5.61 M bytes/boot -> machine share 1.3-6.1%; producers 2/3 OH userspace via /dev/kmsg; task-context emission 100% IF=1 | 1-6% + seconds-class stall tail | F1 — console-async = highest-latency single fix |
| 2 | Scheduler wake-loss | WAKE-RUN-LEAK 677-678 prints / 780 s on the FULL v1..v6 stack (pa18/pa20); 31-32 refusals (25-27 lying busy slot + linked); TERM-DESCHED fires ~8/boot and every occurrence survives (acceptance demonstrated, pa17 could not test it); recvfrom max 121 s and futex avg 519 ms are this family's syscall-side face; lying slots born on another path — birth forensics continues | **large — still the wall-clock driver** | F3 (launcher lane) |
| 2b | x86_64 kernel-clock 1 s granularity — TSC shift-first math (F7; CORRECTED+FIXED legs 13-15, OFFICIALLY MEASURED leg 17) | read_time stepped once per 2^32 TSC ticks = 1.0008 s (9e5f0147's `(delta>>32)*mult`); the LAPIC IRQ always fired at 100 Hz — jiffies, WW ages, WAKE-STRANDED strands and every timed wait inherited the grid. **FIXED (launcher lane's u128 multiply-then-shift, merged as 199f308c) and MEASURED by pa23** (main+fix hunk): tripwire `[ok]` 16 jiffies/150 ms; 99.9 ticks/s/CPU sustained; WW prints 57 -> 2 (organic 414/251 ms); WAKE-STRANDED 100 -> 0; nanosleep avg 1.87 s -> 88.5 ms (-21x); clock_gettime max 654 ms -> 3 ms; census counts hold (TERM-DESCHED 8, WAKE-REFUSED 32). **pa27 (merged main, official): tripwire `[ok]` 15 jiffies/150 ms; census TERM-DESCHED 8 / WAKE-REFUSED 32 / WAKE-STRANDED 0; nanosleep avg 221 ms (-10x vs pa17 pre-fix KVM); clock_gettime 42.6 M @ 3 us, max 288 ms (host-noise class); IF=0 share NOT moved (row 1) — the fix removes clock granularity, not irqsave work** | latency amplifier REMOVED and the split now official: it did NOT move wall-clock milestones (F3 remains the phase-timeline driver) | F7 — closed FIXED + OFFICIALLY MEASURED |
| 3 | hilogd dual-drain + retry loops | ~25% of all syscall volume (section 2.1); volume is OH userspace | medium | F2 — kernel levers small (per-call recv cost) |
| 4 | ext4 metadata + write path | section 2.2 rows, medians 100-300x reference | medium (~300-500 CPU-s/boot) | F4 (fs lane) |
| 5 | Atomic/CAS contention | glue page ~16-22% of kernel ticks; RMW owners named — they ARE the 1b sections | medium — folds into 1b | F5 |
| 6 | clock_gettime volume (no vDSO) | 33.3-37.3 M calls at 3-4 us — amortized by TSC timekeeper | small | F6 — closed |

Reading. The gap is not one hotspot; it is four stacked layers in this
order: (a) wake-loss stalls multiplying every dependency chain (the
phase-timeline driver), (a') the 1 s kernel-clock grid (row 2b, now
FIXED and officially measured — removing it cut sleep latency -10x
without moving the phase timeline, so (a) keeps the driver seat), (b)
1.1-1.4 M irqsave sections/s — allocator and lock volume, each section only
~140 ns to 6 us long, (c) userspace logging/retry loops amplified by
per-call kernel costs, (d) metadata ops at 100-300x. The IRQ-off
family (row 1) decomposes almost entirely into per-site work (row 1b)
— there is no single long critical section to break up; the fixes are
per-site (magazines DONE for the allocator; pick-path work for the
GRQ; RCU-ish reads for fdtable).

Ranking movement v1 -> v2.7 (what fixes moved). The read storm (v1's
#3, ~0.5-1.0 vCPU) is gone after the kmsg fix; its slot was taken by
the hilogd dual-drain, which was always underneath and spins faster
now that rd_kmsg stopped competing. The launcher's v1..v6 stack
(pa18) leaves the census population and CPU-side picture unchanged
but demonstrably survives its own rare shape (TERM-DESCHED 8/boot,
pa17 had 0 — clean boots never exercised it). The magazine (pa20)
delivers the projected -97.3% global-lock sections and -24.3%
allocator wall; machine IF=0 only moves -0.7 pp because the remaining
floor is per-op local irqsave and other sites grew with boot depth.
The softirq empty-raise gates (pa14 A/B) measured NO effect on this
workload — both latch bits flip within the first 31 s and never
re-engage; honest negative result, the gates still pay on
timer/TCP-sparse workloads. v2.5 added row 2b: the tick bug was present-but-unread in every
earlier run (its WW-age signature was misfiled as an IF=0-suspect
family). v2.6 corrects its mechanism (read_time granularity, not IRQ
cadence) and closes it: the launcher lane's fix verified in pa23
with the full before/after (section 2h of the working log). v2.7 =
v2.6 + the official post-merge flagship (pa27): row 2b
OFFICIALLY MEASURED on merged main (census and per-op numbers hold;
wall-clock NOT moved — F3 stays the driver), row 1's share band
widens to 23-26% with the composition shift (SOFTIRQ 1.0-1.7 -> 3.1%
of IF=0 = real hrtimer dispatch; softirq busy 2.5 s/420 s pre-fix ->
13.9 s/720 s), and row 1b gains the pa27 GRQ volume datapoint
(17.7 M holds, avg flat).

Known limitations. Single-boot samples per kernel revision; shares are
of a 1440 CPU-s machine window at t=720; console share swings with
printk volume (1.3-6.1%) while per-byte cost is the stable quantity;
the GRQ hold interacts with boot depth (n varies 7.6-17.7 M across
boots) — compare avg and n, never the wall total alone.

---

## 4. Reference-kernel comparison — what the same image does on the reference kernel (OHOS 6.6.101)

- Total boot to launcher.ready: reference 10.12 s; Rux never reaches it in
  13 min (16-23 min in earlier full runs).
- Syscall volume: the reference kernel completes the boot before any retry/logging
  loop matters — no read/connect/recv storm exists at any comparable
  rate; Rux: 80-101 M syscalls by t=720 (112-140 K/s), ~60% logging
  traffic.
- CPU mode split: the reference kernel spends most of this boot in userspace (services
  doing real work); Rux runs 83-89% of ticks in kernel mode (83-85% at
  true 100 Hz sampling, pa27) — a 10.7 s
  boot cannot spend 85% of 2 vCPUs in kernel.
- Same image, same QEMU machine config, same host class; the reference
  kernel is not instrumented, so its numbers are timeline + topology,
  not per-op tables.

Conclusion (the one-sentence version): the Rux-vs-reference boot gap is
dominated, in order, by (a) scheduler wake-loss stalls that multiply
every dependency chain (with the binder 15/30/89 s cost classes as
the deep-boot face of the same wall), (b) 1.1-1.4 M short irqsave
sections/s (lock + allocator volume — no long cli section exists; the
allocator's global-lock share is now fixed by the magazines, its
remaining floor is churn volume), (c) OH userspace logging/retry
loops amplified by kernel per-call costs, and (d) ext4-family
metadata ops at 100-300x reference norms — and the fix order that
follows is wake-loss birth forensics (launcher lane), console-async
(highest-latency single fix), allocation-churn hunt, then the fs
metadata hunt. The former (a') term, the 1 s kernel-clock grid, is
fixed and officially measured off this list (pa27): its removal cut
per-op sleep latency -10x without moving the phase timeline.

---

## 5. Fix effect ledger (before/after, condensed)

Fixes already in main, verified by this lane's measurements (no
re-fix; full table in `perf-audit.md` section 6):

| fix | before (measured then) | after (pa13-pa20) | verdict |
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
| launcher wake-loss v1+v2 (a39d2bc0, 6e9679c5) | pre-fix census 691 prints / 30 refusals | census 593/31 (in variance band) | compatible; v1's rare shape absent from clean boots |
| launcher sched v3+v4 (+v6/R73) (c90f80b0, 7a8044e0, 9777f195) | — | **pa18: TERM-DESCHED fires 8x/boot and all survive (acceptance demonstrated); census population unchanged (lying slots born elsewhere); GRQ hold volume-coupled, avg flat 5.1-6.1 us — the earlier -31% wall claim was variance, corrected** | v1..v6 stack verified harmless-and-effective on its own shape; census birth forensics remains open |
| per-CPU magazine allocator (cb51db03) | 741.7 M global class-lock sections/boot, 176.1 s REST (12.2% of machine) | **pa20: 19.7 M lock sections (-97.3%), 133.2 s REST (9.2%) — the 16.49x projection confirmed; remaining floor is per-op local irqsave (751 M ops @136-168 ns)** | holds — locking lever spent; churn volume is the allocator's next lever |
| softirq empty-raise gates (69310bae) | (hypothesis: wasted raises) | no measurable effect on OH boot — latches flip by t=31 and never re-engage | negative result, recorded |
| X86-TICK-CADENCE, MECHANISM CORRECTED to X86-CLOCK-GRANULARITY (diagnosed leg 13 as "IRQ ~1 Hz", corrected leg 15 — it was read_time's own step size; fix by the launcher lane: u128 multiply-then-shift at all 4 shift-first sites in `kernel/src/drivers/timer/x86_64.rs`, merged as 199f308c) | tripwire `[fail]` 0 jiffies/150 ms; WW ages pinned at exactly 1000/1001 ms; WAKE-STRANDED quantized on the 1 s grid; nanosleep avg 1.87 s | **pa23/pa25 (main + fix hunk vs stock, same TCG config): tripwire `[ok]` 16 jiffies/150 ms; 99.9 ticks/s/CPU sustained; WW prints 57->2 (organic; pre-fix baseline pins 1001 ms x283); WAKE-STRANDED 100 -> 0; nanosleep avg -21x at t=60 and -11x at t=240 depth (1.34 s -> 120 ms, 5x sleep throughput); clock_gettime max 654 ms -> 3 ms; tick-mode kernel share 80-83% at true sampling; census counts unchanged — and the futex/epoll wake-loss tails are UNCHANGED (F3 is a separate bug the tick fix does not touch); wall-clock milestone deltas on the TCG pair are noise-dominated (opposite directions)**. **Merged-main flagship (pa27, KVM, official): tripwire `[ok]`; census 8/32/0; nanosleep 221 ms avg (-10x vs pre-fix KVM); clock_gettime 42.6 M @ 3 us (max 288 ms host-noise); IF=0 25.7% in-band, SOFTIRQ 3.1% of IF=0; wall-clock NOT moved — verdict: the F7 amplifier is gone, F3 + binder cost classes own the deep-boot wall** | holds (pre-merge verification AND merged-main flagship) |

Candidates diagnosed but NOT yet fixed (this lane, for the fix owner's
table):

| candidate | diagnosis | measured effect when fixed | owner lane |
|---|---|---|---|
| (none open on this lane's own list — the tick fix above closed the only candidate; allocation-churn hunt and console-async remain the roadmap items, sections 3 and 6) | | | |

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
6. **Reference asymmetry.** The reference-kernel boots are uninstrumented
   (timeline + topology only); "reference norm" per-op figures in 2.2 are
   order-of-magnitude references for the same operations on
   tmpfs/ext4-COW, not measurements of the reference kernel.
7. **The launcher v1 acceptance gap.** TERM-DESCHED=0 on the clean
   census boot means the rare terminal-deschedule shape never ran; a
   clean boot cannot prove a fix for a shape it never sees. The -31%
   GRQ result is one run; A-line v3 (wfi-paced retries, c90f80b0) is
   in flight and will need its own retest boot when it merges.
8. **x86_64 timer-IRQ cadence ~1 Hz; all tick/wake-derived latencies
   on this lane are 1 s-quantized (leg 13).** The WW "IRQ-OFF-WINDOW"
   age family is NOT interrupt-off time: the EFLAGS-exact pair
   accounting (item 4) bounds every closed cli section at 3-5 ms, and
   the ages pin at exactly 1000 ms (KVM) / 1001 ms (TCG) — a periodic
   signature, identical under both accels (pa21/pa22 TCG vs pa13-pa20
   KVM; A-line sc166-202 KVM), with the boot tripwire `lapic tick
   check: 0 jiffies / 150ms [fail]` firing in every x86_64 boot. The
   timer IRQ effectively delivers ~1/s (nominal 100 Hz; calibration
   print is a constant "LAPIC 62 MHz" across accels and hosts —
   arming/calibration mismatch ~100x, exact mechanism open). Downstream
   quantization: WAKE-STRANDED strands (sc200: 1000 ms x1488 /
   2001 ms x312), the 1.0 s cluster among per-op maxes in 2.2, sleep
   and timed-wake granularity. **What stands unchanged:** every
   EFLAGS-exact number (IF=0 shares, REST split, section counts,
   max-segment 3-5 ms, magazine/GRQ walls — pair-accounted,
   tick-independent), console per-byte costs (per-byte TSC), syscall
   volumes (count-based), and milestone wall times (jiffies recompute
   from the TSC grid at each arrival). **What carries the caveat:**
   tick-mode shares (u/k/i are sampled at the ACTUAL tick rate —
   phase-weighted, sparse in steady state), per-op MAXES (re-verify
   after a tick fix), and the latency ATTRIBUTION of the nanosleep/
   futex tails (a 1 s grid inflates them; the wake-loss census COUNTS
   stand, its latencies do not resolve below 1 s). Consumers of
   A-line rolls should read every strand=1000/2001 ms as "one tick
   late", not "measured 1000 ms".
   **RESOLVED (v1.3, pa23):** the mechanism was read_time's shift-first
   TSC math (one step per 2^32 TSC ticks = 1.0008 s), NOT the IRQ
   cadence — the LAPIC always fired at 100 Hz. With the launcher
   lane's u128 multiply-then-shift fix applied to main: tripwire green
   (16 jiffies/150 ms), tick counters 99.9/s/CPU sustained, tick-mode
   shares now sampled at the true rate (kernel 80-83%, user 16.5-18.8%,
   idle ~1-2.5% — the pre-fix 86-89% kernel figure was sparse-sampling
   phase-weighting), the per-op max cluster off the 1 s grid
   (clock_gettime max 654 ms -> 3 ms), nanosleep/futex averages
   re-baselined (88.5 ms / 89.6 ms at t=60 vs 1.87 s / 125.9 ms
   pre-fix). Pre-fix strand=1000/2001 ms readings keep their "one/two
   ticks late" interpretation; post-fix rolls measure true latencies.
   **OFFICIAL CONFIRMATION (v1.4, pa27, merged main):** all of the
   above re-measured on the merged fix — tripwire `[ok]`, census
   TERM-DESCHED 8 / WAKE-REFUSED 32 / WAKE-STRANDED 0, nanosleep
   221 ms avg, clock_gettime max 288 ms (host-noise class), and the
   wall-clock verdict delivered: milestones NOT moved beyond the band.

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
  (`pa-pa27-stamped.log` is the post-fix flagship primary source for
  this report's v1.4 additions, parse at `run/pa27-parse.txt`, nm
  table `run/pa27-nm.txt`; `pa-pa17-stamped.log` is the primary source
  for the pre-fix tables); reference kernel: `/home/william/rux-agents/
  oh-linux-ref/runs/oh-ref-r1/`.
- Post-UI completion: fill the pending row of table 1 from one
  measurement boot on the post-UI kernel (same method, same 780 s
  horizon); optionally refresh table 2/3 totals in one sentence if
  they shift materially — no structural change expected.

---

## 8. Post-UI microbench + same-host reference-kernel comparison (v1.5 DRAFT — design only, no numbers)

Leg-21 design skeleton. The user mandate: when the UI milestone lands
(launcher.ready SET on a main-class kernel), produce the full post-UI
performance report with microbenchmarks and a same-host reference-kernel
comparison. This section is pre-built so that day is a measurement
day, not a design day. Everything below is committed as design;
**all result cells are placeholders** until the runs exist.

### 8.1 Division of labor: what syscost already answers vs what needs a probe

The quiet-boot syscost instrument (per-syscall {n, avg, max, wall},
slow-ring >4 ms, tick modes, EFLAGS-exact IF=0 zones + REST sites,
census, UART/printk tables) already answers, from ONE quiet pa boot on
the post-UI kernel:

| question | syscost source (existing) | post-UI action |
|---|---|---|
| post-UI milestone timeline (table 1 pending row) | patstamp host stamps | fill row, this boot |
| whole-boot per-op syscall costs (table 2 refresh) | per-nr {n,avg,max} | refresh in one sentence |
| CPU mode split, IF=0 share/composition (rows 1/1b) | tick modes + zone v3 | hold-or-move check |
| wake-loss census (row 2 tripwire) | TERM/WAKE counters | hold check |
| console/UART share (row 1a) | uart_putc tables | hold check |

What syscost CANNOT answer, by construction — the probe set's scope:

1. **Steady-state per-op costs free of boot phase mix.** Boot-mix
   averages are phase-coupled (mkdir 9.6 ms vs 62 ms across boots was
   phase mix, not kernel change); the probe measures each op in a
   controlled loop with median/p99.
2. **Any reference-side per-op number.** The reference kernel (OHOS
   6.6.101, the image's own bzImage) has no syscost; a userspace probe
   binary is the only instrument that runs IDENTICALLY on both
   kernels. The v1.4 "reference norm" column was order-of-magnitude
   references; v1.5 replaces them with same-binary measurements.
3. **Ops the boot never does in a controlled shape**: null-syscall
   floor, context switch, fork+exit, pipe/socket roundtrip, page-fault
   classes, sleep overshoot.

### 8.2 Probe set (one static binary, P1-P7)

One static musl x86_64 binary (toolchain `x86_64-rux-linux-musl`,
musl = the OH image's own libc family; static = no libc-version
skew between sides). Timing: `rdtsc` deltas calibrated per-run
against `CLOCK_MONOTONIC_RAW` (supported on Rux, kernel/src/syscall/
time.rs); medians + p99 over fixed N with warmup discarded — means
stay useless under foreign-QEMU host noise. One parsable result line
per bench (`PBENCH <name> n=<N> med=<us> p99=<us> min=<us> ...`).

| id | bench | method (both sides identical) | what it pins down |
|---|---|---|---|
| P1 | null syscall floor | `syscall(SYS_getuid)` loop, N=1e6; separate row: `clock_gettime` loop | raw kernel entry/exit cost; the syscall() wrapper bypasses the reference kernel's vDSO/libc caching, so both sides measure the true entry path. Rux side cross-check: syscost clock_gettime avg 3-4 us |
| P2 | context switch / wakeup | two processes, `sched_setaffinity` to opposite CPUs (supported, sched.rs:938), 1-byte pipe ping-pong, N=1e5; report half-roundtrip | scheduler + IPC wake cost in controlled shape; complements workload-shaped futex tails |
| P3 | page-fault classes | (a) first-touch anon (touch every page of a fresh 64 MB map); (b) COW (fork, child writes shared touched pages, N faults counted); (c) file-map cold read (mmap+touch a file the probe wrote first) | us/fault per class; cross-check: A-line PACE healthy-regime 3.4 us/COW (their branch kernel; this gives the main-class + reference pair) |
| P4 | fork+exit; fork+exec+exit | N=1e3 loops; exec target = a tiny static binary shipped next to the probe | process create/destroy cost (appspawn's shape) |
| P5 | pipe/socket roundtrip | (a) pipe 1-byte roundtrip; (b) unix dgram sendto/recvfrom pair incl. one `MSG_PEEK` variant = the exact hilogd protocol; (c) unix stream 74 B write/read pair (hilogd consume shape) | per-record socket cost behind ~25% of boot syscall volume (row 3); the PEEK row measures the OH-side protocol overhead on both kernels |
| P6 | fs ops on ext4 (userdata) | a prep pass builds an identical tree (1000-entry dir, 1000 x 4 KB files) on the private userdata copy, then: stat, open+close, getdents, unlink, mkdir+rmdir, 4 KB write+fsync, page-cached read, append; N=1e3-1e4 each | steady-state row-4 numbers free of boot phase mix; same ext4 contents on both sides |
| P7 | sleep resolution | nanosleep(1 ms) x 1e4, report avg overshoot | post-tick-fix regression guard (221 ms boot avg is the mixed tail; this isolates the healthy path) |

Not probed (deliberately): CPU-bound user loops (kernel report, not a
CPU survey), memory bandwidth (same), TCP loopback (unix dgram is the
workload shape; TCP can be a v1.6 addition).

### 8.3 Delivery and output channel (open point, one decision left)

Options, with the established precedents:

- **(A) RECOMMENDED — late-service slot swap**: debugfs-inject the
  static probe into a PRIVATE copy of system.img at a slot OH execs
  late in boot, replacing a non-essential service binary (precedent:
  the A-line zeroed libusb_pnp_manager in oh-img-p without breaking
  boot). Criteria for the slot: exec'd (not dlopen'd), starts after
  launcher/appspawn phase, non-essential. Decide the exact slot by
  reading the image's cfg XMLs on build day. Output: results written
  to a file on /data (userdata, extracted via debugfs post-boot —
  A-line precedent) PLUS best-effort write to /dev/console for live
  serial monitoring.
- (B) bootanimation slot: runs earlier (post-composer, pre-launcher on
  Rux) — wrong phase for a post-UI report; reject unless (A) is
  blocked.
- (C) hdc shell post-boot: needs hdc working on both kernels; Rux-side
  status is the hdc-tcp lane's territory. Fallback only.

Reference side identical: same injected binary, same private image copies,
booted with the oh-linux-ref harness (`test/run-oh-linux-ref.sh`,
reference bzImage = OHOS 6.6.101) with machine config aligned to the
pa series: **SMP=2 MEM=4096M** (its defaults are smp6/8G — must be
overridden) and `-cpu host` vs pa's `-cpu max` delta recorded (or the
script patched for the probe runs; oh-ref-r1/r2 timeline boots stay
untouched).

### 8.4 Same-host reference-kernel comparison protocol

| number | source | new boots needed? |
|---|---|---|
| table 1 reference milestone column; section 4 topology statements | oh-ref-r1/r2 serial logs (existing) | no |
| every P1-P7 probe number | probe runs on the reference kernel | yes: N=3 reference probe boots |
| every P1-P7 Rux number | probe runs on post-UI main-class kernel | yes: N=3 Rux probe boots |
| post-UI milestone row + tables 2/3 refresh | one quiet syscost boot (the standard pa recipe) | yes: 1 x 780 s Rux boot |

Protocol details: probe boots are SHORT (milestone + probe wall +
margin — no 780 s horizon; the probe itself is seconds-to-minutes);
Rux and reference probe boots run back-to-back in one contiguous window,
interleaved (R,L,R,L,...), foreign-QEMU count noted per boot like the
pa series; medians/p99 make single-run outliers visible; 3 per side
gives the range column. v1.4's "reference norm" cells get replaced by
these measurements where they exist (flagged in place), keeping the
old text as the historical pre-UI reading.

### 8.5 Result tables (PLACEHOLDERS — empty until measured)

Table 8a — probe grid (per side: median of 3 boots; p99 in parens).
**Reference column BANKED 2026-10-24 (leg 23, ref-r1..r4 — section 8.8);
Rux column fills on run day.** Reference cells = median of 4 boots
(med-of-runs; p99 = med-of-runs of per-run p99), unit us:

> **Leg-29 (2026-10-25) terminal state of the Rux column:** the
> trigger's footprint arm fired (the A-line from_fn trilogy landed on
> main as dca64cc4/056b68ad/26391849; tip fee7774f) and the run day
> opened — but the probe's start condition (bootevent.launcher.ready
> SET) is observably FALSE on the fee7774f main-class kernel (pa38
> quiet boot, 780 s: milestone never set, same depth profile family
> as pa32/pa33, death class below). 8.6's own step 1 (confirm the
> milestone in a main-class serial) gates step 4, so the N=3 probe
> boots were not spent on a start condition known false — the cells
> stay unmeasured, NOT abandoned: 8.9 records the exact re-fire
> condition and the pre-staged run-day assets.

| bench | Rux (post-UI main-class) | reference kernel (image's own OHOS 6.6.101) | factor |
|---|---|---|---|
| P1 null syscall | PENDING | 0.040 (0.050) | — |
| P1 clock_gettime | PENDING | 0.070 (0.080) | — |
| P2 ctx switch (half roundtrip) | PENDING | 0.090 (3.318) | — |
| P3a fault anon | PENDING | 5.010 (15.35) med bimodal 0.38-7.15, see 8.8 | — |
| P3b fault COW | PENDING | 0.751 (7.845) | — |
| P3c fault file | PENDING | 0.100 (1.798) | — |
| P4 fork+exit | PENDING | 10.88 (123.6) | — |
| P4 fork+exec | PENDING | 22.37 (173.2) | — |
| P5a pipe roundtrip | PENDING | 0.180 (0.245) | — |
| P5b dgram send / recv+PEEK | PENDING | 0.155 (1.623) / 0.215 (1.648) | — |
| P5c stream write 74 B | PENDING | 0.160 (1.803) | — |
| P6 stat / openclose / read_cached | PENDING | 0.271 / 0.436 / 0.676 | — |
| P6 getdents (full 1000-entry sweep) | PENDING | 88.88 (345.4) | — |
| P6 write+fsync 4 KB | PENDING | 4754.99 (30768.9) | — |
| P6 append 4 KB | PENDING | 0.471 (8.075) | — |
| P6 mkdir+rmdir pair | PENDING | 40.43 (80.02) | — |
| P6 unlink | PENDING | 26.99 (47.69) | — |
| P7 sleep 1 ms overshoot | PENDING | 14.64 (21.22) | — |

Table 8b — syscost refresh vs the pa27 flagship (one quiet post-UI
boot): IF=0 share __, census TERM/WAKE __/__ (baseline 8/32), ticks
/s __, nanosleep avg __ ms, milestones __ — PENDING.

> **Leg-29 (pa38, main-class fee7774f = from_fn trilogy, terminal
> state — the "post-UI" refresh remains unreachable, UI never
> landed):** census TERM-DESCHED 8 / WAKE-REFUSED 32 — EXACT baseline
> hold, sixth consecutive roll (pa23/pa27/pa31/pa32/pa33/pa38); the
> from_fn fork-path fixes do not move the census. Ticks 99.74/99.76/s
> per CPU at t=720 (SYSCOST dump deltas 60s->720s: 65,828/65,840
> samples over 660 s; pa33 was 99.86/99.83 — in-band). WAKE-STRANDED
> 2 prints, organic ~1-2 s class (1332 ms pid 318, 2013 ms pid 675;
> no 1001 ms grid). Milestones: composer_host.ready / appspawn.started
> / bootanimation.finished SET; appfwk.ready, bms.main.bundles.ready,
> launcher.ready, lockscreen.ready NOT set (launcher warning at
> +44.1 s). Boot volume 49,540 serial lines vs pa33's 21,580 — the
> app phase now runs the full window (81 ABC ability restarts:
> launcher 48, systemui extensions 33) instead of stalling early;
> 136.4 M syscalls (deepest pa-series volume, pa27 was 100.9 M).

### 8.6 Run-day checklist (the day UI lands on a main-class kernel)

> Leg-23 update: step 4's reference half is DONE (ref-r1..r4, section 8.8);
> run day runs the Rux boots only, same private-image recipe.
>
> Leg-24 update: steps 3-4's Rux-side MATERIAL IS PRE-STAGED —
> run/pbrux-system.img (injected reflink copy, e2fsck-clean, untracked),
> run/pbrux-boot.sh (one probe boot: post-UI main-class kernel, plain
> no-syscost boot, matched machine class, results extraction + SET
> milestone check), run/pbmed.py (med-of-runs aggregator; validated —
> it reproduces the banked 8a reference column from ref-r1..r4 exactly).
> The trigger response is: merge main, make build-x86, then the boots.
>
> Leg-29 update (2026-10-25): the footprint arm of the trigger FIRED
> (from_fn trilogy on main, tip fee7774f) and the response ran:
> merge -> make build-x86 (run/pa38-main-bzImage, provenance
> run/pa38-built-from.txt) -> quiet boot pa38 (780 s) -> milestone
> verdict. Step 1 FAILED to confirm the milestone (launcher.ready
> never set; see the 8b note and 8.9), so step 4's N=3 probe boots
> were not launched — the probe's param job is conditioned on
> bootevent.launcher.ready=true, a condition measured false on this
> exact kernel 40 minutes into the same boot class that would host
> it. Everything else the checklist lists as run-day material is in
> place and re-verified (leg 28): injected image, boot script,
> aggregator, banked reference column. The next fire of either trigger
> arm repeats this response from step 1.

1. Confirm the milestone in a main-class serial (this lane's patrol or
   the coordinator's signal); record kernel revision.
2. Merge main into this branch; build; run the standard quiet boot
   (pa32+: syscost=1 sprobe=0, 780 s) — fills table 1's pending row
   and table 8b.
3. Build the probe (test/pbench.c, leg 22) and the private images per
   8.3(A) (slot + trigger decided and validated in 8.8: key_enable
   slot, launcher.ready param job — run/pbref-inject.sh).
4. N=3 Rux probe boots (8.4; reference half banked in 8.8); parse; fill
   table 8a.
5. Freeze v1.5: replace v1.4's "reference norm" cells with
   measured pairs where they exist; write the reading paragraph.

Budget note: 1 long boot + ~6 short boots, sequential — fits the
2-self-started-QEMUs-at-a-time discipline with the long boot alone in
its window; this plan is the next leg's gate spec.

### 8.7 Harness status (leg 22): probe built and smoke-validated — run day starts from a tested binary

The 8.2 design is now code (leg 22, commit b72e0011):
`test/pbench.c` (one static musl x86_64 binary, ~70 KB) +
`test/pbench_boot.py` (quiet-boot driver, faultlat mkfs-d recipe).
Every bench emits `PBENCH <name> n= med= p99= min= max= avg= wall=`
(rdtsc med/p99 + a MONOTONIC_RAW wall backstop), a `SKIP why=` line on
unsupported primitives instead of aborting, `PBENCH DONE rc=0` last,
then reboot(POWER_OFF) (clean sync + powerdown, verified).

Validation chain:

1. Host run (native on the host OS, same binary): 25/25 lines, med/p99 sane — caught
   and fixed a fmt stack-overread and a pipe read/write-end swap
   (P2/P5a) plus an echo-child EOF deadlock before any QEMU burn.
2. Guest smoke **pb1** (kernel pa31-main-bzImage = main 62c2708e; the
   only delta to main 9fe7cdbb is this report's docs): quiet boot,
   init=/test/pbench, q35/-cpu max/SMP2/2G, DONE at t=378 s — the
   whole probe fits a SHORT boot with wide margin (8.4's "seconds to
   minutes" holds). Full set landed: 25/25 lines, aff=1 (pinning
   works), n full on every bench.
3. Output channel both ways: serial lines live AND
   /test/pbench-results.txt persisted in the guest ext4, extracted
   post-poweroff via debugfs — the file half of the 8.3(A) delivery
   option is proven (on the quiet-boot image).

Provisional smoke numbers (pb1, ONE boot, minimal-rootfs ext4, NOT the
OH image, NOT run-day numbers — recorded so run day has a sanity anchor
and so gross regressions in the harness itself are visible):

| bench | med | p99 | note |
|---|---|---|---|
| p1_null | 4.57 us | 6.8 us | true entry path via syscall() |
| p1_clock | 4.95 us | 5.4 us | cross-checks syscost clock_gettime avg 3-4 us — harness agrees with the kernel-side instrument |
| p2_ctxsw (half rt) | 15.4 us | 437.7 us | pinned CPU0/CPU1, aff=1 |
| p3a anon fault | 0.08 us | 583 us | med sub-us = most stores hit already-mapped pages (eager anon mapping); p99 is the fault tail |
| p3b COW fault | 24.6 us | 52.0 us | A-line PACE healthy-regime said 3.4 us/COW on their branch kernel — same order, different kernel/boot, recheck on run day |
| p3c file fault | 0.07 us | 1103 us | med sub-us = faultaround mapping ahead of the touch; p99 1.1 ms = cold fault |
| p4_fork_exit | 8.32 ms | 12.0 ms | |
| p4_fork_exec | 13.4 ms | 19.7 ms | exec target = self re-exec |
| p5a pipe rt | 21.5 us | 32.4 us | unpinned |
| p5b dgram send / recv+PEEK | 126 / 275 us | 462 / 479 us | hilogd protocol shape |
| p5c stream write 74 B | 56.2 us | 568 us | |
| p6_stat / openclose / read_cached | 128 / 310 / 367 us | 0.3-0.7 ms | |
| p6_getdents (full 1000-entry sweep) | 9.98 ms | 13.1 ms | |
| p6_write_fsync 4 KB | 18.1 ms | 48.7 ms | fsync dominates |
| p6_append 4 KB | 500 us | 744 us | |
| p6_mkdrmdir pair | **83.2 ms** | 139.7 ms | metadata writes: see below |
| p6_unlink | **32.8 ms** | 66.5 ms | |
| p7 sleep 1 ms overshoot | 2.09 ms | 2.25 ms | wall avg 9.98 ms is host-noise contaminated (load 10); med/p99 are the signal |

Two findings worth carrying into run day regardless of UI timing:

- **Metadata-write ops are the ext4 outlier, steady state.** mkdir+rmdir
  83 ms and unlink 33 ms per op on a QUIET boot with zero boot-phase
  mix — 2 orders over reads (stat 128 us). This pins the old table-4
  boot-mix mkdir spread (9.6-62 ms) as real per-op cost, not phase
  mix. The reference pair on run day will size the gap.
- **1G is below this kernel's early-boot heap floor.** A minimal-rootfs
  q35 boot with -m 1G panics in ~3 s on a 4 MB kernel-heap allocation
  (main.rs:271 OOM path) before init runs; -m 2G is the working floor
  (faultlat precedent; Makefile default). Affects any line sizing
  "cheap" test boots at 1G on x86_64.

Still untested (run-day items, by design — QEMU budget spent on the
quiet-boot smoke): the 8.3(A) late-service slot pick from the OH cfg
XMLs, the debugfs swap into a private system.img copy, and the probe
running to DONE inside the OH boot. The quiet-boot path (8.6 step 2
shape) is fully proven.

### 8.8 Reference-kernel side BANKED (leg 23, ref-r1..r4): the late-slot inject works — run day is Rux-only

The reference kernel is the OH image's own OHOS 6.6.101 bzImage, so
its numbers do not move when Rux main moves. Leg 23 pre-collected the
whole reference side; table 8a's reference column is measured, not pending.

**Delivery — 8.3(A) adapted, NOT the fallback.** Slot: `key_enable`,
read out of the image's own cfgs (`/system/etc/init/*.cfg`, 132 files
inspected via debugfs). It is the only clean fit for the 8.3 criteria:
a DEDICATED exec'd binary (`/system/bin/key_enable` — every late SA
service rides the shared `sa_main`, unusable), uid root (probe needs
/data writes + `reboot(POWER_OFF)`), `once:1` (no respawn loop), and
non-essential (it only sets the fs-verity signature requirement). The
stock trigger (job `init`: ~t=2.7s, too early) is replaced with the
image's own late-start pattern (audio_server.cfg): a param job
conditioned on `bootevent.launcher.ready=true`, which also mkdirs
`/data/local/pbench`. The service gains argv (workdir, results file),
`sandbox:0`, and `console:1` — the PBENCH lines land live on serial.
Tooling: `run/pbref-inject.sh` (private reflink system.img; e2fsck -fn
clean after inject; binary round-trip verified) and `run/pbref-boot.sh`
(private per-run reflink userdata-tok copy, no snapshot on that drive —
writes persist; results extracted via debugfs post-poweroff).
`test/run-oh-linux-ref.sh` gained CPU/SYSIMG/USERDATA_SNAP env knobs;
its defaults (smp6/8G/-cpu host) are untouched for timeline boots.
Because the OH userspace driving all of this is identical on both
kernels, the same recipe transfers to the Rux side on run day — and
the condition is the milestone itself.

**Boots.** ref-r1..r4 (N=3 per protocol + a 4th for the one wide
bench, below), machine class matched to the pa series: q35/KVM/`-cpu
max`/SMP2/4G. All four: 25/25 lines, `PBENCH DONE rc=0`, results file
extracted, clean `reboot: Power down` (t=32-35s kernel time; the probe
starts at launcher.ready t=10.1-16.7s depending on host load; ~35-40s
wall per run). Foreign-QEMU count 4-5 per run. SELinux on the image is
permissive (avc lines logged, nothing enforced — the injected service
runs fine in the key_enable domain; one avc line on the final
reboot(POWER_OFF) is cosmetic).

**Stability (N=3 -> 4).** Per-run med spread: <=6.4% on 18 of 21
benches. Exceptions:

- `p3a_fault_anon` med is THP-bimodal across runs (7.15 / 3.14 / 6.88
  / 0.38 us — 70% spread triggered the protocol's 4th run). Cause:
  the reference kernel's transparent hugepages — when a 2MB page backs the region, 511
  of 512 per-page touches cost ~0. The per-run AVG is stable
  (9.8-10.3 us/page); treat avg as the comparable statistic for p3a
  and note Rux has no THP (every page faults individually), so the
  med comparison itself is a design difference, not noise.
- `p3c_fault_file` med 0.01-0.10 us: sub-us med = readahead maps ahead
  of the touch (same shape as the Rux side); p99 carries the real
  fault.
- `p6_write_fsync` med range 4457-5208 us (+-8%): host IO load — the
  boot overlaps other lanes' QEMUs; the range is the honest number.

**Readings worth carrying to run day (Rux column still pending; the
pb1 smoke was TCG + minimal rootfs, so NO factor is claimed yet):**

- The ext4 metadata-write outlier has a reference anchor: mkdir+rmdir pair
  40.4 us, unlink 27.0 us, write+fsync 4.75 ms — vs pb1's Rux
  provisional 83.2 ms / 32.8 ms / 18.1 ms (TCG). Even allowing for
  KVM-vs-TCG, the metadata ops are the widest provisional gap in the
  set (2-3 orders); the run-day KVM pair will size it exactly.
- The reference kernel's OWN outliers for context: write+fsync 4.75 ms (fsync
  dominates — same shape as Rux) and getdents full-sweep 88.9 us/1000
  entries. Everything else is sub-us to tens of us.
- Sleep resolution 14.6 us overshoot at 1 ms — the post-tick-fix Rux
  guard row has its reference.
- The probe overlaps the BOOT_COMPLETED service wave (~10.7s+) by
  construction — same shape on both sides; medians are robust to it
  (p99 carries the tail, e.g. p2_ctxsw p99 3.3 us med-of-runs).

Caveats recorded: GPU model 360x720 (ref harness) vs 720x1280 (pa
harness) — no UI work happens in the probe, cosmetic. Userdata base =
this worktree's userdata-tok reflink (not oh-robot's 16G stock that
oh-ref-r1/r2 booted): run-day Rux boots will use the SAME base, so
side parity holds. p7 wall backstop reads ~1.067 ms/op vs req+avg
1.016 ms — per-iter clock-pair overhead; med/p99 remain the signal.

### 8.9 TERMINAL REPORT (leg 29, 2026-10-25 — trigger fired; run day executed to its terminal state)

The user mandate for this line was: when the trigger fires, produce
the full post-UI performance report — microbenchmarks, same-host
reference-kernel comparison, v1.5 frozen. The trigger fired on 2026-10-25 02:31
(watcher tick `main=fee7774f footprint=YES kernel_new=14`): the A-line
from_fn footprint trilogy landed on main (rebased as dca64cc4 ext4
fill_page_cache_batch / 056b68ad Task construction / 26391849 FdTable
fork_from), followed at tip by fee7774f reverting the kstack-sizing
third of their stack (80KB slots overflowed the deeper-nesting riscv64
kernel; suite 1094/1096 -> 1096/1096 with the revert). The run-day
response ran the same hour and reached a terminal, measured verdict:

**1. What is DONE and banked.**

- Reference half complete (leg 23, 8.8): all 21 benches, med-of-4,
  stability within protocol — table 8a's reference column is final.
- Rux pre-UI performance work complete and merged across legs 13-26:
  the tick-grid fix (official pa27: census TERM 8 / WAKE 32 / STRANDED
  0, nanosleep -10x, clock_gettime max 654 ms -> 3 ms), the IF=0/UART
  cost-center map (v1.4), the probe harness (test/pbench.c, smoke-
  validated end to end both output channels), and the run-day
  mechanics (inject + boot + aggregate, all re-verified byte-clean).
- Census discipline: SIX consecutive exact baseline holds across five
  kernel-commit classes (pa23 pre-merge, pa27 flagship, pa31, pa32
  dfx-align, pa33 stallwatch pidhash, pa38 from_fn) — TERM-DESCHED 8 /
  WAKE-REFUSED 32 every roll. The wake-loss and tick invariants do not
  move with any of it. (Leg-30 note: this battery is now formalized
  as the repo merge gate — user policy 2026-10-25, AGENTS.md "Merge
  Gate" + docs/development/perf-gate-baseline.md, Tier 1 bands
  6-10/28-36 seeded from the pa38 roll; this lane's standing
  census-hold discipline is unchanged and is the Tier-1 instrument.)

**2. What the run day measured this fire, and why the Rux column
stays unmeasured.** The milestone (bootevent.launcher.ready SET) —
the probe's own start condition — is FALSE on the fee7774f
main-class kernel (pa38, 780 s quiet boot, syscost=1). The blocker is
NOT memory anymore: zero kernel OOM lines with the from_fn trilogy in
(the fake-OOM class the footprint campaign targeted is gone from this
boot class), and the census/tick invariants hold exactly. The blocker
is a deterministic userspace-visible fault:

```
pagefault: Segfault at 0x0, rip=0x7ff082996bac, rsp=0x7fffffffcb10, mode=user
```

- Same rip, same rsp, fault address 0x0, in EVERY appspawn'd app
  process (5 pids x 3 prints in pa38: 602/778/811/836/1401); ABC
  restarts abilities 81 times across the window (launcher 48,
  systemui extensions 33); the wedged launcher then spun on fd 32
  (SYSCOST rd_fail comm=m.ohos.launcher ret=-9 n=155,991).
- PRE-EXISTING, not a from_fn regression: pa33 (pre-from_fn main,
  leg 24) carries the byte-identical signature at the same rip. What
  from_fn changed is everything AROUND it: serial volume 21.5k ->
  49.5k lines, restarts 0 -> 81, syscalls 100.9 M -> 136.4 M — the
  app phase now survives deeper and retries for the full window
  instead of stalling.
- **Leg-30 attribution correction (A-line decode, applied before
  this report freezes): the crash is an IMAGE DEFECT, not a Rux
  crash.** The A-line's USEVG-FOR forensics names comm=usb_host
  crashing in the image's libusb_pnp_manager.z.so
  (DdkListenerMgrAdd deref-before-nullcheck; DdkListenerMgrInit is
  imported by nobody in that lib build, so the listener list is never
  initialized). Deterministic on ANY kernel, the reference kernel included — root
  cause banked by the launcher lane in sc87-91; the usbless image
  (sc113-116) boots with zero DDK crashes. This leg's earlier
  "Rux-attributable" reading was wrong in one specific way: the ref
  boots (8.8) do NOT survive because the reference kernel avoids the fault — the
  same lib faults there too — they survive because the whole reference
  boot is ~100x faster, so the ABC restart storm never eats the boot
  window. What IS Rux-side is the BOOT-WINDOW COST: the restart
  storm's price only becomes fatal when the underlying boot is slow.
  The blocker pair is therefore: image defect (usb_host DDK) +
  Rux boot-window cost.
- The fd-32 spin WAS a kernel bug, now fixed by the A-line (branch,
  pending merge at leg-30 start): read/write on a VALID fd whose
  type has no read op must return EINVAL, as the reference kernel does, not EBADF —
  EBADF reads as "fd closed", so the OH event loop spun forever
  instead of erroring out.

**3. Re-fire condition (exact).** Either arm: (a) a sc2xx-class boot
shows launcher.ready SET / OnForeground / ArkUI on a main-class
kernel, or (b) main moves again with launcher-lane fix material. The
response is the 8.6 checklist from step 1 — assets are staged
(run/pbrux-system.img injected + e2fsck-clean, run/pbrux-boot.sh,
run/pbmed.py, banked reference column; pa38 proved the merge->build->
quiet-boot loop same-day). With the milestone confirmed, the probe
boots are minutes-class (ref side: full 21-bench probe + powerdown
in ~35-40 s wall; pb1 smoke on Rux: DONE at t=378 s in a SHORT
quiet boot) — the measurement day itself is small; the blocker was
never the harness.

> **Leg-30 run-condition note (with the attribution correction):**
> the launcher lane's default for main-class rolls is now the USBLESS
> image (zero DDK crashes, sc113-116 onward; their sc237 usbless roll
> already advances past the park point). When the run day's Rux
> column is collected, record the image class in the run conditions:
> the banked reference column (8.8) is STOCK-image, so any stock/usbless
> delta must be flagged next to table 8a. The probe itself (P1-P7)
> touches no usb component, so the delta is expected to be nil for
> the benches — but that is a claim to record, not to assume.

**4. Verdict on the mandate.** The report this line was mandated to
produce is delivered to the boundary of what the kernel can host:
Reference side measured and final; Rux side blocked behind an identified
pair — a deterministic IMAGE defect (usb_host DDK
libusb_pnp_manager.z.so, present under the reference kernel too) whose
restart-storm cost eats the boot window only because Rux's boot-window
cost is ~100x the reference boot, plus the fd-32 EBADF kernel bug (fixed
on the launcher lane, pending merge) — with the full run-day apparatus
staged and validated so the Rux column is one confirmed-milestone
away. The performance findings that ARE measurable pre-UI (tick fix,
census invariants, cost-center ranking, syscall tables) are final in
sections 1-7 and survived six kernel generations unchanged.

### 8.10 NORTH-STAR GOAL (user mandate 2026-10-25): outperform the
reference kernel by 50%+ on every measured metric

**The target.** Every pbench P1-P7 metric: target = reference median x
0.5 (at least 2x faster than the reference kernel, image's own OHOS
6.6.101). Boot-phase wall-clock included. New features and new internal
architecture are both allowed; the three iron gates hold (external
compatibility, LTP green, no upper-layer breakage) plus the perf merge
gate (AGENTS.md, baselines in docs/development/perf-gate-baseline.md).

**The trajectory (realism discipline).** Rux is a debug-build kernel
far from parity on most rows — the goal is sequenced: (a) reach parity
per metric, (b) then close to 2x. Gap table below, pb1 vs the banked
reference medians:

| metric | pb1 Rux med (us) | ref med (us) | gap | 2x target (ref x0.5) |
|---|---|---|---|---|
| p1 null syscall | 4.570 | 0.040 | 114x | 0.020 |
| p1 clock_gettime | 4.951 | 0.070 | 71x | 0.035 |
| p2 ctx switch | 15.409 | 0.090 | 171x | 0.045 |
| p3a fault anon | 0.080 | 5.010 (bimodal) | ahead | met (caveat: ref THP) |
| p3b fault COW | 24.614 | 0.751 | 33x | 0.376 |
| p3c fault file | 0.070 | 0.100 | 1.4x ahead | 0.050 (not yet 2x) |
| p4 fork+exit | 8324 | 10.88 | 765x | 5.44 |
| p4 fork+exec | 13353 | 22.37 | 597x | 11.19 |
| p5a pipe roundtrip | 21.517 | 0.180 | 120x | 0.090 |
| p5b dgram send | 126.257 | 0.155 | 815x | 0.078 |
| p5b dgram recv+peek | 275.445 | 0.215 | 1281x | 0.108 |
| p5c stream write | 56.223 | 0.160 | 351x | 0.080 |
| p6 stat | 127.710 | 0.271 | 471x | 0.136 |
| p6 openclose | 310.071 | 0.436 | 711x | 0.218 |
| p6 getdents (1000-entry) | 9975 | 88.88 | 112x | 44.44 |
| p6 read_cached | 366.915 | 0.676 | 543x | 0.338 |
| p6 write+fsync 4 KB | 18125 | 4755 | 3.8x | 2377 |
| p6 append 4 KB | 500.378 | 0.471 | 1062x | 0.236 |
| p6 mkdir+rmdir pair | 83192 | 40.43 | 2057x | 20.2 |
| p6 unlink | 32816 | 26.99 | 1216x | 13.5 |
| p7 sleep 1 ms overshoot | 2094 | 14.64 | 143x | 7.32 |

(Caveats, one line: pb1 is ONE TCG minimal-rootfs smoke on the pa31-
class DEBUG kernel — ranking-grade, not run-day-grade; the reference
column is med-of-4 on the OH image. The run-day Rux column supersedes
this table's left column when the milestone lands.)

**Lever ladder (sequenced by impact; L0 gates the rest).**

- **L0 — release-profile measurement. DONE (leg 32, pa41 battery — see
  8.10.1): the answer is decisive.** A properly optimized release build
  (lto=true, codegen-units=1, strip) collapses the syscall floor 12-32x
  vs the debug build every kernel generation to date has been measured
  on: p1_null 6.89 -> 0.55 us, p2 28.7 -> 1.38 us, p4_fork_exit
  13.4 ms -> 420 us, p6_stat 220 -> 14.8 us, p6_getdents 17.8 -> 1.50
  ms (same-window back-to-back pairs). The weakening the leg-31
  snapshot carried (lto=false) costs a further 30-50% on those rows;
  it was a workaround for a release-LINK breakage, not a profile
  choice, and is reverted. Consequence for the ladder: the per-row
  engineering targets are now set against the RELEASE-STRONG kernel;
  the remaining outliers (ext4 metadata rows, dgram/stream socket
  family, fork+exec tail) are the real work. L0 also unblocked the
  release path itself, which had been un-buildable on main-class trees
  since the crate grew past the dfx-align merge (see 8.10.1 for the
  breakage and the two stacked fixes).
- **L1 — ext4 lazy no-journal write path (F4).** The metadata rows are
  the board's worst gaps and L0 barely moved them (mkdrmdir -8%,
  unlink -12%, append improved via the floor but still ~175x):
  release-strong reads mkdrmdir 76.4 ms = 1890x and unlink 29.3 ms =
  1085x vs the reference medians. F4's estimate is -30-60 s boot wall
  AND these rows. fsync 2.7x is already the closest row — the write
  path's journal work is the difference between it and the metadata
  rows.
- **L2 — console-async** (~5% CPU + tail latency; boot wall).
- **L3 — EPOLL_WAKE_REGISTRY sharding** (1.0-1.2% of machine; p5
  family indirectly). After L0 the dgram/socket family is a
  first-tier outlier in its own right (p5b/p5c ~190-260x vs reference
  on release-strong; they improved only -58% where the floor families
  improved -90%).
- **L4 — architectural levers**, drawn from
  docs/architecture/rust-design-headroom.md (capability traits, typed
  per-CPU + lock keys, ownership transfer, Result boundary, typed IPC
  states, timer slots); every architectural WINNER gets an entry in
  docs/architecture/design-divergences.md with its compatibility
  evidence.

**Optimization measurement protocol** (this lane's own battery, before
every merge claim): before/after pair on the pbench_boot.py quiet-boot
channel — same machine class, same accel mode, back-to-back in one
host-load window, med-of-3 for merge evidence (med-of-1 acceptable as
a quick gate); any change touching mm/sched/fs additionally runs the
Tier-1 census quiet boot (TERM 8 / WAKE 32 hold) per the merge gate.
**Profile-class is part of the run conditions from L0 on**: every
pbench row recorded in this report states debug vs release-strong
(they differ by an order of magnitude on the floor families); cross-
class comparisons are meaningless. The standing run-day trigger and
terminal report (8.6/8.9) are unchanged — the day the milestone lands
on a main-class kernel, table 8a's Rux column fills from the OH-image
run day (and that column must be collected on a release-strong kernel
plus a debug twin, or the L0 question relitigates forever).

### 8.10.1 L0 results (leg 32, 2026-10-25, pa41 battery)

**Setup.** Three kernels from ONE source tree (4f9a25cd = main 6f7e4ac3
+ docs + the ret_from_fork shim removal), differing only in the
release profile: debug (the class of every pa/pb run to date),
release-weak (lto=false, cgu=16, no strip — the leg-31 snapshot's
inherited default), release-strong (lto=true, cgu=1, strip — main's
own profile, via the new `make build-release-opt` env gate, no default
change). Channel: test/pbench_boot.py quiet boot (TCG, q35, SMP2/2G,
minimal-rootfs), back-to-back 05:54-06:11 in one host-load window
(15-19 foreign QEMUs), med-of-1 per kernel (protocol quick-gate tier;
the deltas below are far outside that noise class). Provenance:
run/pa41-built-from.txt (hashes), run/pa41-*.log.

**Precondition found and fixed: the release path was un-buildable on
main-class trees.** Both release profiles failed since the crate grew
(main 6f7e4ac3 era) with `ret_from_fork changed binding to
STB_GLOBAL / already defined`: a fossil `.weak ret_from_fork`
link-shim in main.rs (its own comment predates trap.S: "trap.S is not
written yet") collided with trap.S's `.globl` definition whenever
rustc placed the two global_asm! blocks into one assembly unit —
which cgu=1/lto forced pre-merge (pa39 builds 1-2) and crate growth
then forced under EVERY profile post-merge. The leg-31 weakening of
[profile.release] was the predecessor's undocumented workaround for
the cgu=1 flavour. Two independent fixes landed on the branch this
leg and stack cleanly: 4f9a25cd (delete the fossil shim; root-caused
via the emitted CGU asm — the shim text is visible at its head) and
4c0c80ea (leg-31's gasp commit: move the two symbols to Rust naked
fns). pa42 re-verified: all three profiles build at HEAD.

**Board (medians, us; pb1 anchor = the pa31-class debug smoke; ref =
the banked 8.5 reference column):**

| bench | pb1 (dbg) | pa41 dbg | pa41 strong | pa41 weak | strong/ref | 2x target |
|---|---|---|---|---|---|---|
| p1_null | 4.570 | 6.887 | 0.551 | 0.891 | 13.8x | 0.020 |
| p1_clock | 4.951 | 9.734 | 0.651 | 0.641 | 9.3x | 0.035 |
| p2_ctxsw | 15.409 | 28.667 | 1.383 | 1.894 | 15.4x | 0.045 |
| p3a anon | 0.080 | 0.130 | 0.080 | 0.140 | ahead (ref THP) | met* |
| p3b COW | 24.614 | 35.489 | 2.846 | 4.799 | 3.8x | 0.376 |
| p3c file | 0.070 | 0.130 | 0.070 | 0.130 | 1.4x ahead | 0.050 |
| p4 fork+exit | 8324 | 13407 | 420.4 | 623.0 | 38.6x | 5.44 |
| p4 fork+exec | 13353 | 20453 | 1528.5 | 1598.6 | 68.4x | 11.19 |
| p5a pipe | 21.517 | 36.261 | 2.766 | 2.434 | 15.4x | 0.090 |
| p5b dgram send | 126.257 | 97.444 | 40.985 | 6.322** | 264x | 0.078 |
| p5b dgram recv+peek | 275.445 | 96.492 | 40.945 | 5.340** | 190x | 0.108 |
| p5c stream 74B | 56.223 | 103.128 | 40.795 | 41.678 | 255x | 0.080 |
| p6_stat | 127.710 | 219.530 | 14.791 | 24.125 | 54.6x | 0.136 |
| p6_openclose | 310.071 | 478.839 | 27.427 | 44.182 | 62.9x | 0.218 |
| p6_getdents (1000) | 9975 | 17765 | 1497.6 | 2276.0 | 16.8x | 44.44 |
| p6_read_cached | 366.915 | 417.365 | 35.033 | 62.616 | 51.8x | 0.338 |
| p6_write_fsync | 18125 | 20691 | 12685 | 18167 | 2.7x | 2377 |
| p6_append | 500.378 | 1000.576 | 82.621 | 151.913 | 175x | 0.236 |
| p6_mkdrmdir | 83192 | 82815 | 76431 | 93515 | 1890x | 20.2 |
| p6_unlink | 32816 | 33396 | 29271 | 45438 | 1085x | 13.5 |
| p7 sleep overshoot | 2094 | 1471 | 975.2 | 934.4 | 66.6x | 7.32 |

(**p5b weak-column values are scatter: that family's per-run spread
under load spans 5-100 us across all pa41 boots — the strong-column
value is the stable one to read. *p3a caveat unchanged: the reference
is THP-bimodal, Rux faults every page.)

**Readings.**

1. **The debug floor was inflating the whole board 12-32x.** Same
   window, same source: strong/debug = -92% (p1_null) to -95% (p2,
   fork+exit) on every floor-carrying row. The pb1-class gaps (114x,
   171x, 765x) were mostly BUILD CLASS, not kernel engineering.
2. **Strong beats weak by 30-50% where it matters** (p1_null 0.551 vs
   0.891, p2 1.383 vs 1.894, p4_fork_exit 420 vs 623, p6_stat 14.8 vs
   24.1, p6_append 82.6 vs 151.9): the inherited weakening is
   reverted; the release profile is strong again (matching main).
3. **What did NOT move: ext4 metadata writes** (mkdrmdir -7.7%,
   unlink -12.4%, fsync -38.7%) — these are journal/write-path work,
   immune to codegen. L1 is now unambiguously the top lever, with the
   socket family (p5b/p5c, improved only ~2.4x from dbg) next.
4. **North-star trajectory: parity band reached on nothing yet, but
   the board moved from 100-2000x to 10-70x on the floor families**
   (p3b/p3c/fsync are the closest rows: 3.8x / ahead / 2.7x). The
   post-L0 gap table above is the working ladder now.
5. **Verification chain:** p6_unlink gate row (b) rerun on the
   main-class debug kernel: 33396 us = +1.8% vs the pb1 anchor — IN
   BAND; the flagged +22.4% marginal-out does not reproduce, and
   2f17d663 explains the bookkeeping error that produced it (four
   Tier-2 ANCHOR cells in run/perf-gate.sh had been transcribed from
   the reference-kernel column instead of pb1's own medians — fixed
   there; the anchor table is authoritative again).
6. **HEAD re-verification (pa42):** after stacking leg-31's naked-fn
   variant, release-strong still builds (all three profiles) and
   boots 25/25. The same-window A-B (pa41-strong vs pa42-headstrong,
   both boots under loadavg ~26) shows the STABLE rows equivalent
   within noise (p1_null +2.2%, p2 -5.9%, p6_stat -2.0%, getdents
   -1.4%, fsync +0.9%) while the load-noisy rows scatter both ways
   (fork+exec +48%, openclose -34%, append +78%, p7 -55%) — and the
   control rerun of the SAME trap.S binary across load windows
   (p1_null 0.551 at load ~17 vs 0.892 at load ~26, +62%) quantifies
   the host-load confound that dwarfs any variant effect. Verdict:
   the naked-fn entry path is performance-equivalent; the L0 table
   cites the 4f9a25cd tree, and HEAD (naked-fn) is the merge shape.
   Run logs: run/pa41-pb-strong2.log, run/pa42-pb-headstrong.log.

**Tier-1 gate outcome the same hour (pa41gate, 780 s quiet syscost
boot, main-class debug kernel, KVM/SMP2/4G):** census TERM-DESCHED 8 /
WAKE-REFUSED 32 — EXACT hold again; panics/OOM 0; clock_gettime 4 us,
GRQ hold 6.9 us, small_alloc share 6.6% — all in band. Two rows FAIL
under tonight's load (14-19 foreign QEMUs, loadavg up to 26): tick
cpu0 99.40/s (marginal, one CPU) and syscall volume 77.1 M vs the
136.4 M band. The volume row is hereby attributed, not re-banked:
pace was flat -35% vs pa38 from the FIRST checkpoint (13.9-14.9 M per
120 s interval throughout, incl. pre-app phases) while every per-op
cost held — volume-in-window is host-pace-coupled, not kernel cost
(the dead leg-31 decider's pa40 boot read 76.6 M with the same
signature). The launcher lane's 120.5/114.8 M readings are the same
class. Standing item: re-bank volume AND the nanosleep row from a
PASSING quiet-window gate run — nanosleep readings across boots now
span 174-333 ms (pa41gate 174, sc239 184, seed 221, pa38 333), which
proves the single-point 221 ms seed was workload-mix artifact; the
baseline doc marks the row unbanked pending that window (below).

### 8.10.2 L3/socket-family first lever (leg 34, 2026-10-25, pa47)

The post-L0 gap table's socket-family outlier (p5b/p5c ~190-260x on
release-strong) got its first cost-center lever: the AF_UNIX send hot
path (commit b3757d92 on feature/perf-audit) removes three per-send
costs — the O(queue-depth) flow-control walk under the recv_queue lock
on every send and every blocked-sender recheck (now a running
RecvQueue::charge counter), the up-to-3x bound_name lock+string-scan
for the hilog/param/socksnap diagnostic taps (now a bind-time cached
AtomicU8 tap_class), and the per-segment String allocation for the
sender name (now an Arc cloned from a bind-time cache; MSG_PEEK
clones become refcount bumps).

Measurement (quick gate, med-of-2 per arm, BOTH boot orders to cancel
the host-load drift that a single back-to-back pair suffers — pa47
series, release-strong, TCG): p5b_dgram_send A{5.262, 8.405} vs
B{4.687, 5.019} us and p5b_dgram_recv_peek A{4.660, 7.514} vs
B{4.246, 4.328} — B lower in both orderings (~-25-30% on family
means); controls flat (p5a_pipe — no unix sockets — and p5c unchanged
within 2%); p1_null scatters both directions (pure load noise).
Functional: 4 pbench boots complete (no skips, no short runs), OH
smoke boot 420 s clean (0 panics/OOM, hilog taps fire 3000x under the
cached flags, PCTL-SEND/RECV both print, unixsend-loop 0, census
TERM-DESCHED 8 EXACT on the smoke). med-of-3 per arm owed at
merge-claim time.

What this does NOT close: the family remains ~30x the 2x target
(0.078/0.108 us) — the residual per-send cost is the syscall floor +
fdtable + wake-path work shared with every family, and the flow-
control/wake design of a blocking pair. Next levers for the row stay
behind the floor levers (fork+exec tail, ext4 metadata) in priority.
