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

---

## 0. Environment and method

- Guest: OpenHarmony x86_64 image, QEMU q35, KVM, `-cpu max -m 4G
  -smp 2` — identical machine config for Rux and the Linux reference
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
- Linux reference: same image, same machine config, reference kernel
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

pa27 (post-fix flagship, merged main e1e317f2, 780 s horizon):
composer_host.ready +29.8 s (fast edge of the 31-44 band), appspawn
.started +80.1 s (ABOVE the 32-66 band — both directions present =
single-boot variance), appfwk.ready set by +57.4 s, bms
.main.bundles.ready / bootanimation / launcher.ready NOT set at 780 s.
Verdict: the tick fix does not move the phase timeline; the driver
stays F3 + the binder cost classes.

Reading. Divergence grows with boot depth. Early services are only
2-6x slower; everything downstream of appspawn runs 8x to >100x
slower, and the two final milestones are never reached inside the
measurement horizon. A uniform per-op slowdown would keep the ratio
flat; a growing one is the signature of a *latency multiplier*
stacked on top of per-op costs — scheduler wake-loss (family F3,
section 3 below) multiplying every dependency chain, on top of the
per-op families. The 0.6x on the first line is real, not noise: Rux
skips the Linux device-probe storm and reaches /init first. Leg-13
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
| 4 | ext4 metadata + write path | section 2.2 rows, medians 100-300x Linux | medium (~300-500 CPU-s/boot) | F4 (fs lane) |
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

## 4. Linux comparison — what the same image does on Linux

- Total boot to launcher.ready: Linux 10.12 s; Rux never reaches it in
  13 min (16-23 min in earlier full runs).
- Syscall volume: Linux completes the boot before any retry/logging
  loop matters — no read/connect/recv storm exists at any comparable
  rate; Rux: 80-101 M syscalls by t=720 (112-140 K/s), ~60% logging
  traffic.
- CPU mode split: Linux spends most of this boot in userspace (services
  doing real work); Rux runs 83-89% of ticks in kernel mode (83-85% at
  true 100 Hz sampling, pa27) — a 10.7 s
  boot cannot spend 85% of 2 vCPUs in kernel.
- Same image, same QEMU machine config, same host class; the reference
  kernel is not instrumented, so its numbers are timeline + topology,
  not per-op tables.

Conclusion (the one-sentence version): the Rux-vs-Linux boot gap is
dominated, in order, by (a) scheduler wake-loss stalls that multiply
every dependency chain (with the binder 15/30/89 s cost classes as
the deep-boot face of the same wall), (b) 1.1-1.4 M short irqsave
sections/s (lock + allocator volume — no long cli section exists; the
allocator's global-lock share is now fixed by the magazines, its
remaining floor is churn volume), (c) OH userspace logging/retry
loops amplified by kernel per-call costs, and (d) ext4-family
metadata ops at 100-300x Linux norms — and the fix order that
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
6. **Reference asymmetry.** The Linux reference boots are uninstrumented
   (timeline + topology only); "Linux norm" per-op figures in 2.2 are
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
  for the pre-fix tables); Linux reference: `/home/william/rux-agents/
  oh-linux-ref/runs/oh-ref-r1/`.
- Post-UI completion: fill the pending row of table 1 from one
  measurement boot on the post-UI kernel (same method, same 780 s
  horizon); optionally refresh table 2/3 totals in one sentence if
  they shift materially — no structural change expected.
