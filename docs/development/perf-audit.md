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
| `pa-pa9` | + positive-read census (pa10 adds fail census) | syscost=1 sprobe=0 | 330 s | ATTRIBUTES the flood: hilogd.rd_kmsg reads /proc/kmsg, 196 data reads then idle while flood counter runs — so the flood reads return 0 |
| `pa-pa10` | + failing-read census | syscost=1 sprobe=0 | 300 s | zero rd_fail rows all boot → flood reads are EOF; pa4-8 "not EOF" was census-table saturation (12/12 slots taken by earlier comms) |
| `pa-pa11` | + /proc/kmsg stream (WQ block) + kmsg write fix | syscost=1 sprobe=0 | 330 s (wedged) | ABBA WAKE: WQ-held wake vs GRQ-held scheduler prints → both CPUs spin; kmsg write passthrough VERIFIED working ([kmsg] markers live) |
| `pa-pa12` | + /proc/kmsg stream (timed-poll block) | syscost=1 sprobe=0 | 330 s | verification run for the fix |
| `pa-pa13` | same kernel as pa12 (= HEAD f94c9f19) | syscost=1 sprobe=0 | 780 s | post-F2 ranking re-measure, pa3 horizon; read rate -113x, hilogd.server now #1 flood; ranking v2 below |
| `pa-pa14` | pa13 + launcher's softirq v1/v8 empty-raise gates (69310bae timer hunks, applied to a scratch build) | syscost=1 sprobe=0 | 780 s | F1 gate A/B: NO measurable effect on OH boot — marginal raise rates identical to ungated from t=120 (199.2-199.7/s); latch flips at ~9 s (v8) / ~31 s (v1); host shared with 3 foreign QEMUs (2x oh-display, 1x samgr-callback probe) from mid-run |
| `pa-pa15` | main@f242b7be + pa15 probe (IF=0 zone hooks + UART per-byte + per-comm printk + recv census; hooks in run/pa15-probe.patch, bzImage run/pa15-probe-bzImage) | syscost=1 sprobe=0 | 782 s | F1 deep-dive + hilogd dual-drain profile. cpu1 parked by a KERNPANIC at t=418 (see incident) -> t>=480 windows are cpu0-only. 2 smoke runs first (pa15smoke/2/3): smoke1 caught the on()-before-cmdline cache-off bug, smoke2 caught a u64 multiply overflow in the dump |

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
- pa13 (post-F2) does not change this picture: 316 IRQ-OFF-WINDOW
  prints over 780 s (0.44/s vs pa3's 0.49/s — same order), and the #1
  rip page is still the cli-recovery cluster (below).

### 2a. pa13: the post-F2 landscape (read storm gone)

pa13 = same kernel as pa12, run to the 780 s horizon for apples-to-apples
against pa3 (pre-fix). Host was quiet this time (load ~0.5, no foreign
QEMU), so absolute rates are cleaner than pa1/pa3/pa12.

Tick modes, last dump (t=720): cpu0 user 14.9% kernel 84.2% idle 0.9%;
cpu1 14.9% / 83.3% / 1.8%. (pa3 t=900: 12.2-12.4% / 86.9% / ~0.8%.)
Kernel share down ~3 pp, userspace up ~2.5 pp — the CPU the read storm
burned moved partially to userspace, partially to idle.

Syscall totals: 105.9M calls / 720 s = 147K/s vs pa3 188.7M / 900 s =
210K/s (**-30% syscall rate**; pa12's -16% at 330 s was the early-phase
slice). read: 591,713 total (0.82K/s) vs pa3 83.96M (93.3K/s) =
**-113x**. hilogd.rd_kmsg flood count 19,459 vs 83.46M (**-4300x**).

New flood leader (F2 remainder, confirmed): `hilogd.server` dual drain —
13.26M recvmsg **and** 13.26M recvfrom by t=720 (one of each per loop
iteration, 18.4K/s each; pa3: 11.62M each by t=900 = 12.9K/s — the
drain spins FASTER now that rd_kmsg no longer competes). Then
sysevent_source 672K, mmi_service 477K, connect-retry storm 3.26M
(pa3: 4.38M). clock_gettime 43.6M and writev 19.8M unchanged (not
flood-family: no per-call amplification, just volume).

pa13 kernel-rip pages (nm-verified against pa13's own ELF):

| page (pa13) | share | contents (nm) |
|---|---|---|
| 0xffffffff802cc000 | **26%** | `fs::buffer` FileChunk/FileBuffer read/write/resize + `cpu::enable_irq`/`disable_irq`/`restore_irq`/`save_and_disable_irq` + `inb`/`outb` — the same cli-recovery cluster as pa1/pa3's #1 page (layout shifted by the kmsg code); `__do_softirq` itself now lives on 0x806fe000, which is *below top-20* (<0.5%) |
| 0xffffffff8046a000 | 16% | generic `atomic_load`/`atomic_store` monomorphizations (RootFSSuperBlock, Task, u8-u64, SuperBlock/Ext4FileSystem/VfsMount/SysfsSuperBlock words) — the F5 atomic-glue page (pa3 equivalent 19%) |
| 0xffffffff80742000 | 5% | `log::__private_api::GlobalLogger::enabled` — log-filter checking per record |
| 0xffffffff80740000 | 4% | `alloc::string::String` fmt Write |
| 0xffffffff80632000 | 3% | `fs::inode::inode_hash` |
| others (rt_sigprocmask page 2%, TwoWaySearcher 2%, flood_account 1%...) | ~15% | long tail |

Reading: the F1 ticks-behind-cli page is still #1 at 26% (pa1 32%, pa3
20%; composition stable — re-enable sites + UART cli + now sharing the
page with FileBuffer). The parser's bare-symbol label for the page
(`Ext4DirEntry::from_bytes +0x520`) is the page-base symbol only; the
nm walk above is the authoritative content. The softirq *loop body*
dropping below top-20 while v1/v8 raise rates stay ~194/s matches the
established model: the cost is the dispatch discipline and where queued
ticks land, not handler time.

Boot milestones (single-run, high variance — do not over-read): pa13
composer_host.ready 44.3 s, appspawn.started 65.9 s (pa3: 33.5/36.1,
pa12: 36.0/43.8, pa1: 40.3/31.9). Early milestones stay in the same
2x band pre/post fix; the F2 fix's wall-clock claim rests on the
syscall/CPU structure above, not on these.

### 2b. pa15: F1 decomposed — UART per-byte cost, console producers, and the hilogd.server drain decoded

pa15 = main@f242b7be plus a measurement probe (fourth session). What it
adds over pa13/pa14: direct, stateless per-event numbers instead of
tick-sampling inferences. Four instruments:

1. **UART per-byte cost** (hook in `console.rs putc`, x86_64): every
   console byte reports its whole-call wall time (raw TSC), LSR poll
   count (one `inb` = one KVM VM exit), and whether it ran with the
   probe's IF=0 model set. Stateless per call — these numbers are
   exact regardless of the zone-accounting caveat below.
2. **Per-comm printk volume** (hook at `write_to_ring_buffer` +
   `emit_to_console`): records, ring bytes, console bytes, and
   console-bytes-under-IF=0 per comm; IRQ-context prints bucket to a
   `<irq>` pseudo-comm.
3. **recv-family fd census** (hook in the syscall wrapper, nrs
   47/45): per (comm, nr) slot that follows the current fd — flags,
   O_NONBLOCK, ret-shape (data/EAGAIN/0/errno), bytes, ops-table
   pointer (nm-symbolized host-side), and for AF_UNIX the socket
   kind/bound-name/peer.
4. **IF=0 zone accounting** (hooks in cpu.rs enable/disable, irq_enter/
   irq_exit, `__do_softirq`, putc): cumulative wall time with IF=0,
   split by zone — REST (explicit cli/irqsave sections), HARDIRQ
   (delivery->irq_exit frame), SOFTIRQ (dispatch loop), UART (putc).
   PROTOTYPE CAVEAT, stated up front: a task-context putc runs with
   IF=1 and can be migrated by the scheduler between its zone push and
   pop (the inb/outb VM exits are exactly where a resched IPI lands);
   a leaked stack entry then mis-attributes that CPU's zones and can
   hold a segment "open" across IF=1 time. Cross-checks caught this
   (uart-zone time 20x the summed per-putc times; REST nseg
   nonsensical at 32G). **Zone splits and the if0 totals are UPPER
   BOUNDS only.** The v2 probe (run/pa15-probe.patch as committed:
   zone pushes gated on if0_now for both putc and the ksoftirqd-side
   softirq entry) fixes that class — smoke4 (pa15smoke4, 180 s)
   validates cpu0 (uart zone 148 ms against 541 ms summed if0-putc
   time, consistent order), but cpu1 STILL inflates (uart segments
   averaging 10x the per-putc average, ~2 s stall maxes soaking
   zones) — a second leak class stays open, so zone numbers remain
   upper bounds until it is found. The stateless instruments above
   carry the findings.

**Finding 1 — the console path costs 16-18 us per byte and ~5% of the
machine, and its stall tail is seconds-class.** Final t=721 tables:

| table | bytes | total | avg/byte | max/byte | polls>1 |
|---|---|---|---|---|---|
| putc with IF=0 | 282,194 | 4.57 s | 16.2 us | 3 ms | 0 |
| putc with IF=1 | 4,061,151 | 74.19 s | 18.3 us | **7,085 ms** | 0 |

Every single byte polls LSR exactly once (multi=0) — the cost is the
`inb`+`outb` KVM-exit pair (~8-9 us per exit on this loaded host),
plus a host-scheduling tail: healthy-path bytes have stalled up to
7.1 s inside that one exit pair (2^34/2^35-cycle histogram outliers;
smoke runs saw 2.9-4.0 s versions of the same). 4.34 M console bytes
per boot at 11.5 printk records/s — total UART emission 78.8 s of CPU
per 782 s boot = **~5% of the 2-vCPU machine**, of which only ~0.3%
of machine (4.6 s) executes with IF=0. The per-byte latency tail, not
the IF share, is the F1 multiplier: an emitter parked seconds in a
putc while holding the printk/console lock is exactly the b140 wedge
shape and the 1 s-class TIMER-SOFTIRQ-STALL / nanosleep-lateness
feed (pa15: 363 IRQ-OFF-WINDOW prints at age≈1000 ms, same order as
pa13's 316).

**Finding 2 — who prints: OH userspace via /dev/kmsg dominates the
console; kernel IRQ-context prints are the only IF=0 emitters.** Top
console-byte producers, t=721 (8,962 records, 733 KB console bytes
total, loglevel=4 filtered exactly 1 record all boot):

| comm | records | console bytes | console bytes under IF=0 |
|---|---|---|---|
| init | 3,829 | 236,902 | 0 |
| `<irq>` (kernel IRQ-context) | 2,518 | 211,135 | 211,135 |
| OS_FFRT_2_0 (FFRT worker) | 958 | 138,613 | 0 |
| storage_daemon | 967 | 77,423 | 0 |
| storage_manager | 491 | 53,868 | 0 |

All task-context console emission runs with IF=1 (every con_if0B=0);
the only IF=0 console bytes are the kernel's own IRQ-context prints
(WW/sprobe family — including the SYSCOST dumps themselves, ~52 KB —
self-observation noted). Console-async or a console-loglevel drop
reclaims the ~5% CPU and removes the seconds-class stall tail; the
IF=0 portion (0.3% of machine) is secondary. This also sizes the
`[kmsg]` passthrough decision from pa9-12: userspace kmsg writes are
now the majority of console volume.

**Finding 3 — the zone split (upper bound) says the 26% cli-recovery
page is mostly REST.** t=420 (both CPUs alive), upper-bound IF=0 wall
= 237.4 s / 840 s CPU-s = 28.3% — same order as pa13's independent
26% tick-page estimate, so the model is right at the total level. The
split ordering is REST (irqsave sections: spinlocks, FileBuffer,
atomic RMW regions) > HARDIRQ frames > SOFTIRQ dispatch > UART —
i.e., the launcher line's irq-exit dispatch fix (roadmap 2b) targets
the hard+soft slice (single-digit % of CPU, upper bound ~5-8%), while
the bulk of IF=0 time is short irqsave critical sections scattered
across the kernel (60% of the upper bound). WW's own counters bound
the softirq story precisely: handler-pass time (WW_SOFTIRQ_TICKS) is
2.5 s per 420 s (0.6%), 203,849 passes (485/s against a 200/s two-CPU
tick rate) — the loop machinery is NOT spinning out its 2 ms budget
(~1.7 passes/invocation, ~95 us/invocation upper bound). The
launcher's in-flight dead-task-holding-HARDIRQ-count conviction
(sc186/sc189, parked in wfi) is a *lost-CPU* shape, not an IF=0
shape: wfi executes with IF=1, so it cannot inflate the cli-recovery
page — it would surface as kernel ticks with rip on the wfi page
(pa13 has no unidentified multi-percent page; pa15's own PREEMPT-LEAK
rips symbolize to `cpu::inb+0x10` (UART THRE poll inside an IRQ
frame), a log-crate iterator, and one park inside the pa15 probe's
own `irq_note_on` — the probe widens the classic sti-window and can
act as a park site for the pre-existing leak; the count anomaly
itself is the launcher lane's sc69 disassembly-proven pre-existing
bug).

**Finding 4 — hilogd.server dual-drain decoded: a PEEK+consume pair
paced by real traffic, not a spin.** Final t=721 census:

| syscall | calls | fd | target | flags | shape |
|---|---|---|---|---|---|
| recvfrom (45) | 9,477,284 | 12 | unix:dgram bound=/dev/unix/socket/hilogInput, UNIX_SOCKET_OPS, blocking | MSG_PEEK (0x2) | 100% data, exactly 2.0 B/call, 0 EAGAIN |
| recvmsg (47) | 9,477,284 | 12 | same fd | 0x0 | 100% data, 74 B/call avg, 702 MB total, 0 EAGAIN |

Perfectly 1:1 (both counters identical to the digit). The loop is:
blocking `recvfrom(MSG_PEEK)` parks on the socket wait queue until the
next hilog record arrives (avg 84 us/call at t=420 — exactly the
12.2 K records/s arrival period; max 59 s during a stall), returns a
2-byte peek, then `recvmsg` consumes the record (5 us avg). Evidence
for the ipc lane: (a) the "flood" is real log volume — 702 MB of
records per boot through one socket, fed by the writev storm (19.8 M
calls) of 100+ services; (b) each record pays TWO full socket recv
paths where one recvmsg suffices — the PEEK is pure OH-side overhead
(9.5 M syscalls, 19 MB copied); (c) 12.2 K blocking wake+peek pairs
per second ride the socket wait-queue wake path continuously, which
is the same wake machinery the W3 census watches; (d) no kernel
bug — per-call costs (5/84 us) are the lever, plus whatever the
launcher lane's wake-loss fix does to the tail (recvfrom max 59 s =
a WAKE-RUN-LEAK-class stall, not a kernel recv cost).

**pa15 incident — cpu1 parked by KERNPANIC at t=418.**
`trap: KERNPANIC pfault badaddr=0x00007ff090411000 rip=handle_timer_tick+0x526`
— the WW tick-age probe's stack walk read a garbage (user-range)
table base inside the timer tick handler, right after printing an
IRQ-OFF-WINDOW whose recovery rip was already in user space
(pid=0x42a rip=0x7ff0007c7a69). Not pa15 code (trap.rs is untouched
by the probe); the pattern is forensics reading corrupted task/regs
state — wildwrite-lane territory. Effect on pa15: cpu1's tick and
probe counters freeze from t=480 on; all t>=480 windows above are
cpu0-only. W3 census at 780 s: 26 refusals (pa13 baseline: 23),
WAKE-RUN-LEAK prints 406 (pa13: 702 — lower because half the machine
was parked), PREEMPT-LEAK 5.

**pa15 probe bugs found and fixed in the committed code** (both by
smoke runs before the real boot): (1) `syscost::on()` cached its
"off" verdict the first time it was called before cmdline init — the
pa15 zone hooks call it from the first boot-time irqsave spinlock,
which killed the whole probe (now: no caching until the cmdline
exists); (2) a `cyc * 1e9` u64 overflow in the UART dump line
(divide-first now).


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

## 4. Ranking table v2.1 (cost centers -> boot-time contribution)

v2.1 = v2 refined by pa15's direct measurements (fourth session). The
v2 rows keep their pa13 numbers; the pa15 column adds the direct
evidence. Contribution model unchanged: 2 vCPUs; kernel mode ~84% of
ticks (pa15 2-CPU phase t<=420).

| # | cost center | measured (pa13) | pa15 direct evidence | gap contribution | family |
|---|---|---|---|---|---|
| 1 | Interrupt-disabled regions (cli-recovery page) | 26% of kernel ticks; 316 IRQ-OFF-WINDOW/780 s | upper-bound IF=0 28.3% (t=420, 2 CPU); split ordering REST > HARDIRQ > SOFTIRQ > UART; softirq handler bodies only 0.6% CPU, ~1.7 passes/invocation (no budget spin) | **large** — but the levers split: REST bulk = irqsave-section audit; hard/soft = irq-exit discipline; UART = console async | F1 |
| 1a | Console/UART emission (sub-item of F1, now sized) | outb/inb in recovery rips; cli page shares inb/outb | 4.34 M bytes/boot, 16-18 us/byte (inb+outb exit pair), 78.8 s CPU = **~5% of machine**, stall tail to 7.1 s/byte; producers: init 237 KB, `<irq>` 211 KB (all IF=0), FFRT worker 139 KB, storage 131 KB; task-context emission is 100% IF=1 | medium CPU + **large latency multiplier** (seconds-class emitter stalls holding printk lock) | F1 |
| 2 | Scheduler wake-loss (WAKE-RUN-LEAK; lying busy slots + linked-starving) | 702 leak prints/780 s; census 16 lying + 7 linked | 26 refusals/406 prints at 780 s with cpu1 KERNPANIC-parked from t=418; recvfrom(hilogd) max stall 59 s = a wake-tail victim; launcher's dead-task-holds-HARDIRQ-count-in-wfi conviction folded in (lost-CPU shape, not IF=0) | **large** — unchanged; still the wall-clock driver | F3 |
| 3 | hilogd.server socket dual-drain + sysevent_source + connect-retry | 26.5 M calls by t=720 = ~25% of ALL syscalls | DECODED: blocking recvfrom(MSG_PEEK, 2 B) + recvmsg(74 B) 1:1 on fd=12 unix:dgram /dev/unix/socket/hilogInput; 702 MB real log traffic/boot; 0 EAGAIN (paced by traffic, not a spin); PEEK = 9.5 M pure-overhead syscalls; 12.2 K wake-queue parks/s | medium — volume is OH userspace; kernel levers = per-call recv cost + the wake-loss tail | F2 |
| 4 | ext4 metadata + write path (open/stat/getdents/fsync/pwrite/mkdir) | open 235 us, newfstatat 206 us, getdents 643 us, fsync avg 16 ms | (untouched this session) | medium (~300-500 CPU-s/boot at this horizon) | F4 |
| 5 | Atomic/CAS contention (atomic glue page + fetch_update family) | 16% of kernel ticks | (untouched; part of the REST if0 bulk) | medium | F5 |
| 6 | Log-filter + fmt glue | ~11% of kernel ticks combined | printk filtered_by_loglevel=1 all boot — the filter passes everything OH emits; fmt cost is real volume | small-medium | F2/F5 |
| 7 | Allocation churn | ~5% kernel ticks | (untouched) | small | F5 |
| 8 | Mount registry scan | mount not in pa13 top-40 by wall | (untouched) | small | F4 |
| 9 | clock_gettime volume (no vDSO) | 43.6 M calls <1 us | (untouched) | small CPU, medium latency-shape | F6 |


What moved v1 -> v2: the read storm (#3 in v1 at ~0.5-1.0 vCPU) is gone
(read -113x, total syscalls -30%); its slot is taken by the hilogd.server
dual-drain, which was always there underneath (11.6M by t=900 in pa3)
and now spins faster. F1/F3 are unchanged — as predicted, the kmsg fix
was CPU-side relief, not a wall-clock lever by itself.

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
  **RESOLVED (pa9-pa12, second session): the hilogd.rd_kmsg read flood
  is an EOF spin on /proc/kmsg.** pa9's positive-read census caught the
  fd: rd_kmsg opens /proc/kmsg (NOT /dev/kmsg — that is why pa4's
  /dev/kmsg counters stayed 0), drains the procfs open-time SNAPSHOT
  in 196 reads (~200 KB), and every subsequent read returns 0; pa10's
  failing-read census showed zero errno returns all boot, so the flood
  reads are EOFs — the pa4-8 "not EOF" conclusion was EOF-census table
  saturation (12/12 slots claimed by earlier comms), not absence.
  Linux /proc/kmsg is a BLOCKING record-at-a-time stream; OH's reader
  paces on the blocking semantics. Fixed as a Linux-faithful stream
  (single-reader EBUSY, one record per read, shared read_seq cursor,
  timed-poll block — see the pa11 ABBA note below): rd_kmsg collapsed
  from 60-92K reads/s to ~20/s (one syscall per delivered record),
  system-wide read() count at t=300 fell from 20.4M to 228K (-90x),
  total syscalls -16%, kernel tick share 88.0% -> 86.0%. The same
  commit fixes the /dev/kmsg write passthrough (userspace writes were
  dropped by printk()'s pre-ring loglevel filter — OH boot markers
  never reached any hilog consumer; now always ring-recorded,
  console-gated; 772 [kmsg] markers observed in pa12).
  REMAINING in F2 (pa13-confirmed): **hilogd.server dual drain is the
  new #1 flood** — 13.26M recvmsg + 13.26M recvfrom by t=720 (18.4K/s
  each, ~25% of all syscalls; spins faster than pre-fix 12.9K/s because
  rd_kmsg stopped competing), plus sysevent_source 672K, mmi_service
  477K, and the connect-retry storm 3.26M — OH userspace logging
  volume/retry behavior; route to the launcher line with the pa13
  numbers (kernel-side lever is small unless recvmsg/recvfrom per-call
  cost is attacked).
- **F3 Scheduler wake-loss — CONFIRMED as the dominant *wall-clock*
  family.** 448 WAKE-RUN-LEAK episodes (kicked >=2x, never picked),
  median 252 s, p90 467 s, max 520 s. The W3-RUNWAKE-REFUSED census
  splits them: 20 measured refusals with `curr_on=true on_cpu=true`
  (a lying busy slot — the task switched out but the CPU slot/on_cpu
  bookkeeping never cleared, so wake_up refuses forever) and 6 with
  `linked=true` (queued but never picked — pick-path starvation).
  pa13 refresh (post-F2, pre-fix baseline for the launcher lane's
  wake-loss fix): 702 leak prints over 780 s, census 23 refusals split
  16 lying-slot / 7 linked — same ~70/30 shape; W3 output format is
  grep-stable for co-run with the launcher's birth forensics.
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
| /proc/kmsg blocking stream + /dev/kmsg write ring fix (perf-audit d931c345 + cfc39329, second session) | rd_kmsg EOF spin 60-92K read/s, ~15-25% of a vCPU; boot markers dropped pre-ring | pa12: rd_kmsg ~20 reads/s, 772 [kmsg] markers. pa13 (780 s): read() total -113x (93.3K/s -> 0.82K/s), total syscalls -30%, kernel tick share 86.9% -> 84.2/83.3%, user share +2.5 pp | holds |

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
   STATUS: launcher lane has a wake-loss fix in flight; pa13 recorded
   the post-F2 pre-fix census baseline (23 refusals = 16 lying / 7
   linked; 702 leak prints / 780 s) — re-measure W3 numbers on their
   fix when it lands. pa15 (same census, cpu1 KERNPANIC-parked from
   t=418): 26 refusals / 406 prints / 5 PREEMPT-LEAK — baseline shape
   unchanged; their fix is the gate for the retest.
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
   STATUS (pa14 A/B, final numbers): the launcher lane implemented (a)
   as empty-raise gates (69310bae: v1 on TCP-table high-water, v8 on a
   TIMER_EVER_ARMED latch). Measured on the OH boot the gates have NO
   measurable effect: from t=120 s on, the marginal raise rate is
   199.2-199.7/s for BOTH vectors in BOTH runs (gated pa14 and ungated
   pa13 — identical); cumulative differences at t=720 (v1 -2461, v8
   +2010 raises) are inside early-boot run-to-run variance (pa14's
   first-60 s softirq runs were 22% higher than pa13's on host-load
   grounds alone). Reason: OH arms its first timer within seconds
   (v8 latch flips) and creates its first TCP socket by ~t=30 s (v1
   high-water flips — DHCP/netmanager); both latches are monotonic, so
   the gates never engage again. The intra-run v8-vs-v1 split does
   locate the latch moments (v8 latches ~t=9 s, v1 ~t=31 s, from the
   v8-v1 = 4471 offset). Conclusion: on THIS workload the F1 lever is
   (b), the irq-exit dispatch discipline; gate (a) still pays off on
   timer/TCP-sparse workloads (minimal rootfs boots). Tick/page
   agreement: kernel share 84.6/83.0% (pa14, 3 foreign QEMUs on host)
   vs 84.2/83.3% (pa13, quiet); cli-recovery page 25% vs 26%, same
   nm-verified symbol cluster (FileBuffer + enable_irq/restore_irq +
   inb/outb, +0x20 patch shift).
   PA15 REFINEMENT (fourth session): the F1 lever set splits three
   ways. (i) Console async / console-loglevel: reclaims ~5% of the
   machine's CPU (78.8 s of UART emission per 782 s boot at 16-18
   us/byte) and removes the seconds-class per-byte stall tail (max
   7.1 s in a single inb/outb exit pair) that parks emitters holding
   the printk lock — the highest-latency-value single fix. Producers
   are 2/3 OH userspace via /dev/kmsg (init 237 KB, FFRT worker
   139 KB, storage daemons 131 KB); task-context emission is 100%
   IF=1, so this is CPU+latency, not IF=0. (ii) irq-exit dispatch
   discipline: the hard+soft IF=0 slice (upper bound ~5-8% of CPU);
   the softirq loop is NOT budget-spinning (0.6% handler time, ~1.7
   passes/invocation) — the slice is hardirq-frame + dispatch wall,
   and the launcher's dead-task-with-HARDIRQ-count conviction is the
   extreme case (lost CPU, not IF=0). (iii) The REST bulk (~60% of
   the IF=0 upper bound): short irqsave critical sections across the
   kernel (spinlocks, FileBuffer, atomic RMW regions) — needs its own
   section-level audit (biggest single IF=0 bucket, least glamorous).
   PA15 CAVEAT: zone-split numbers are upper bounds (probe migration
   bug, see 2b); fix is known, one line.
2b. **hilogd.server drain — evidence package delivered to the
   ipc lane (pa15 census).** The dual drain is a blocking
   recvfrom(MSG_PEEK, 2 B) + recvmsg(74 B avg) pair on fd=12
   (unix:dgram, /dev/unix/socket/hilogInput), exactly 1:1, 0 EAGAIN,
   paced by 12.2 K real log records/s (702 MB/boot — the writev storm
   feeds it). Kernel levers: per-call recv cost, and the wake tail
   (recvfrom max 59 s = wake-loss family). OH-side lever (their
   code): drop the PEEK — 9.5 M syscalls/boot of pure overhead.
3. **Kill the hilogd.rd_kmsg spin (printk/dfx).** ~~92.6K read()/s
   sustained for the whole boot, ~15-25% of a vCPU. Attribution
   narrowed to "positive-returning reads on a non-kmsg fd" (pa4-8);
   the remaining step is one fdtable dump of that thread mid-flood
   (KILLDUMP prints fd tables; or a one-line fd-path latch in the
   read fast paths).~~ **DONE (second session, pa9-pa12): the spin was
   an EOF loop on /proc/kmsg's open-time snapshot; /proc/kmsg is now a
   Linux-faithful blocking stream (see F2 above). Also fixed the
   /dev/kmsg write passthrough.** NEW CROSS-LANE FINDING from the
   first draft (pa11): parking a reader on KMSG_READ_WQ deadlocks
   against scheduler-internal printk under GRQ (WAKE/W3 print family)
   because wake_up_all holds the WQ lock across wake_up_process
   (R12-1). The waitqueue invariant "no GRQ-held path takes a
   waitqueue lock" is violated by scheduler diagnostics prints —
   scheduler-lane owners should either gate those prints or expect
   this hazard for any new printk-woken waitqueue.
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
  kmsg read counters + EOF/EAGAIN read censuses (fd paths), and
  (second session) the positive-read census (comm-keyed fd latch that
  follows the current open-file-description by `file_id`, recording
  fd/ops/path/bytes; ops pointers symbolize host-side via nm) and the
  failing-read census (comm, errno, fd-resolution state).
  (fourth session, pa15) the recv-family fd census (per-(comm,nr)
  slot following the current fd: flags, O_NONBLOCK, ret-shape,
  bytes, ops pointer, AF_UNIX kind/bound/peer; fed by a wrapper hook
  in syscall dispatch for native nr 47/45, x86_64-gated), the UART
  per-byte accumulators (`uart_putc_note`: n/cyc/max/polls + log2
  histograms, split by IF=0/IF=1), the per-comm printk table
  (`note_printk_ring`/`note_printk_con`/`note_printk_filtered`,
  `<irq>` pseudo-comm for interrupt-context prints), and the IF=0
  zone machinery (`irq_note_on/off`, `zone_push/pop`,
  if0/cum/nseg/seg_max per CPU per zone — callers live in the pa15
  probe patch until the zone-stack migration bug is fixed, see 2b).
  Also: `on()` no longer caches its verdict before the cmdline exists
  (boot-time callers would otherwise latch the probe off forever).
  60 s dumps from the timer softirq. Both arches; zero cost when off.
  Diagnostic caveats learned the hard way: the EOF/EAGAIN censuses are
  ONCE-PER-COMM tables (12 slots) — they saturate with early comms and
  silently DROP later spinners (this produced the wrong "not EOF"
  conclusion in pa4-8; the positive/fail censuses closed it); census
  adds ~5 us to each read that returns 0/EAGAIN while syscost=1;
  flood-table per-op numbers for read come from the census-free binary;
  the IF=0 zone split is an UPPER BOUND (migration stack leak —
  fix: skip the UART zone push for IF=1 putcs).
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

pa14 (softirq-gate A/B) provenance: the gate hunks were extracted from
the launcher lane's 69310bae (`git show 69310bae -- kernel/src/timer.rs
kernel/src/drivers/timer/x86_64.rs | git apply`), built once, the bzImage
saved as `run/pa14-gate-bzImage`, and the patch reverted before commit —
timer/sched files stay launcher-lane territory. Symbolize each run's
pages against the ELF the run actually booted (pa13 vs pa14 layouts
differ by the patch).

pa15 (F1 deep-dive) provenance, same pattern: the IF=0-zone/UART/
printk hooks live in `run/pa15-probe.patch` (cpu.rs, preempt.rs,
softirq.rs, console.rs, printk.rs — all other lanes' territory), the
built kernel is `run/pa15-probe-bzImage`, and the patch was reverted
before commit. The stateless instruments (recv census via the syscall
wrapper, UART per-byte, printk per-comm tables) plus their syscost.rs
state ARE committed and active in any syscost=1 boot. To rebuild the
probe kernel: `git apply run/pa15-probe.patch && PLATFORM=x86_64 make
build`; revert with `git checkout -- kernel/src/arch/x86_64/cpu.rs
kernel/src/console.rs kernel/src/interrupt/preempt.rs
kernel/src/interrupt/softirq.rs kernel/src/printk.rs`.
