# Merge-Gate Performance Baseline (user policy 2026-10-25)

Last-accepted baseline for the merge gate defined in AGENTS.md. Updated only
by a merge that passed its own gate tier. Numbers below are seeded from the
perf-audit lane's accepted measurements on main@257698a5 (pa38 battery,
pb1/pa-series, census streaks) — refresh the whole table at once when a new
main lands, do not cherry-pick rows.

> **Tier-1 vehicle restored (2026-10-31, perf-audit lane)**: the OH
> image set is back — 7 read-only seed images in the central store
> `/home/william/rux-images/oh/`, symlinked into `run/oh-img/` and
> booted snapshot=on + file.locking=off. The userdata LINEAGE is not
> the seed: the seed boot deterministically dies at ~49 s
> (storage_manager 4-strike critical crash → ExecReboot → deliberate
> sysrq panic), and a one-pass cooked copy parks in StorageDaemon
> HandleFileCrypt init_main_user — the tok lineage cannot be re-cooked
> on current main. The lineage was instead rescued from this lane's own
> history: pa-boot.sh always copied the userdata source fresh per tag,
> so `run/oh-userdata-tok109.img` (the state after the last accepted
> Tier-1 debug census boot, Oct 29) is the lineage asset. Full evidence:
> `run/evidence/2026-10-30-pa110-tier1-lineage.txt`. Tier-1 was re-banked
> from this lineage on main@82059759 (pa112tok/pa113tok below); the
> 2026-10-30 vehicle exception for the aarch64 merge is closed.

## Tier 1 — quiet syscost boot (KVM, q35, smp2/4G, syscost=1, 780s window)

| metric | baseline | band |
|---|---|---|
| TERM-DESCHED census | 8 | 6–10 |
| WAKE-REFUSED census | 32 | 28–36 |
| WAKE-STRANDED | 23–31 organic, 1–2 s family (pa109dbg/pa112tok/pa113tok), max 32–107 s | no 1001ms-grid class |
| tick rate per CPU | 99.74–99.98 /s | ≥99.5 sustained (quiet window; one-CPU marginal misses under heavy foreign-QEMU load are the documented flake class) |
| kernel panics / new OOM classes | 0 | 0 |
| total syscall volume (780s boot) | 126.0M (pa115debt1, 2026-11-01, main@afc41b8b) | ±10% IN A COMPARABLE HOST-LOAD WINDOW — re-banked 2026-11-01: 6394e8ee un-quantizes ms-class sleeps and the boot goes ~36% deeper in the same wall (pa113 92.4M → 126.0M, same-depth pace corroborated by debt3 8.0M@60s→18.1M@120s). Enforce only in a load-gated window (load1 ≤9, qemus ≤8); the superseded 107.5M/85–107M families and their load-coupling history are documented in the refresh sections below |
| clock_gettime avg | 3–4 µs | ±20% or better |
| nanosleep avg | 203 ms (pa50main, n=26,945); family 158–222 ms through 2026-11-01 (pa115debt1 read 158.4 ms, n=29.5k) | ≤244 ms (one-sided; 203 ms +20% top) |

## Tier 2 — pbench P1-P7 medians (main-class kernel, usbless image)

Banked Rux guest run: pb1 (pa31-class kernel, DONE t=378s, 25/25 lines).
Compare medians only (p99 is host-noise-dominated on a loaded host).
Banked reference-kernel column (same image, OHOS 6.6.101 kernel,
med-of-4): perf-report.md §8.5. Rux medians must stay within ±20% of
the pb1 column per metric or improve.

ANCHOR PROVENANCE (leg 32, commit 2f17d663): four cells of the Tier-2
anchor in run/perf-gate.sh had been transcribed from the REFERENCE
column (p6_write_fsync 4754.99, p6_mkdir_rmdir 40.43, p6_unlink 26.99,
p6_append 0.471) instead of pb1's own medians (18124.666, 83191.968,
32815.925, 500.378) — those rows failed by construction on every run
and produced the phantom "p6_unlink +22.4% marginal-out" bookkeeping
(leg 32 rerun: +1.8%, in band). The anchor now reads pb1 medians; keep
it that way — the reference column never belongs in a Rux gate anchor.

PROFILE-CLASS NOTE (leg 32, perf-report 8.10.1): Tier-2 rows are only
comparable within one build profile. The pb1 anchor column is a DEBUG
kernel; debug-class gate runs compare against it unchanged. A
release-strong kernel reads 10-30x lower on the floor families — its
gate anchor is the pa41-strong column (perf-report 8.10.1), and
cross-class comparison is meaningless.

P2_CTXSW ANCHOR PROVENANCE (leg 48, banked from the pa75q quiet tier
run, 2026-10-28 04:01, load1=9, rc=0, zero panics): the scheduling
domain's "context-switch half-rt" row is profile-bimodal and both of
its columns are now anchored with provenance. DEBUG class: pb1's own
median 15.409 µs (perf-report 8.10.1) — that is what run/perf-gate.sh's
anchor JSON carries (value 15.4) and it must never be used to judge a
strong kernel. RELEASE-STRONG class: pa41-strong 1.383 µs
(perf-report 8.10.1), re-banked quiet by pa75q at 1.373 µs on the
merged-main strong 0f806d8b — the quiet class is ~1.37-1.39 µs;
loaded windows read 1.9-2.0 µs (the pa57 A-arm 3.3 µs cluster is the
pre-6fdada6b wake-path class, superseded on main). Strong-class gate
runs judge p2_ctxsw against 1.38 µs ±20% (quiet window) or 2.0 µs
(loaded window), never against the 15.4 debug cell.

## SP-7 whole-column re-bank: Tier-2 pbench anchor moves to the KVM debug class (2026-11-03, stress-port round 7)

The pb1 TCG debug column had drifted ~30% stale on p2 (r4sp5 11.245,
r6 10.687/10.091 vs the 15.409 cell) and the r7 re-measure showed WHY
that class cannot anchor a gate row: TCG `thread=multi` p2 readings are
load-hostage — the SAME round-7 kernel read **28.151 µs at host load
5-6** where r6's quieter windows read 10.1-10.7. KVM is 10x tighter:
three full boards on the round-7 kernel (main 91de1d04 + the SP-7 mm
fix; debug bzImage, q35 smp2 2G, quiet windows, foreign_qemus<=2,
every rep `PBENCH DONE rc=0`) agree within ±1% on p2:

| rep | p2_ctxsw med (µs) |
|-----|-------------------|
| sp7pb-kvm1 | 1.237 |
| sp7pb-kvm2 | 1.262 |
| sp7pb-kvm3 | 1.242 |

**Re-banked Tier-2 anchor (run/perf-gate.sh ANCHOR, KVM debug class,
median of 3 boards):** p1_null 0.300, p1_clock 0.371, **p2_ctxsw
1.242**, p3b_cow 8.075, p6_write_fsync 8442.6 (worst spread +11%),
p6_mkdrmdir 216.7, p6_unlink 54.6, p6_append 5.19 µs. The Tier-2
invocation now boots pbench with `--accel kvm` (PB_ACCEL env
override retains the TCG escape hatch). The retired TCG columns stay
in this file's history for provenance; a TCG tier-2 run must NOT be
judged against these cells (cross-class comparison is meaningless —
same rule as debug-vs-strong). The debug-KVM p2 class (1.24) now sits
closest to the release-strong TCG-era class (1.37-1.39): expect the
strong-class KVM column to re-bank lower still when the perf lane
next measures it.

### Strong-class KVM verification (2026-11-03, stress-port round 8)

The re-banked KVM anchor's load-immunity is now VERIFIED at the
strong-workload class: one full board on the round-8 kernel (debug
bzImage, q35 smp2 2G, `--accel kvm`, CPU-affinity pin class — P2's
own aff=1 pin held — booted UNDER host load 4.3-4.7 with 2 foreign
QEMUs running, tag r8sp8-load, DONE rc=0) reads **p2_ctxsw 1.267 µs
= +2.0% of the 1.242 quiet anchor**, where TCG at comparable load
read 28.151 (the load-hostage class). Whole board vs the anchor
column: p1_null 0.300 (0%), p1_clock 0.371 (0%), p2 1.267 (+2.0%),
p3b_cow 8.366 (+3.6%), p6_write_fsync 9661.2 (+14.4% — the I/O row's
load face; the quiet class already spread ±11%), p6_mkdrmdir 217.7
(+0.5%), p6_unlink 54.6 (0%), p6_append 5.07 (−2.3%). All rows inside
±20%. The `p2_ctxsw strong` row's TCG-era cells (1.37-1.39 quiet /
1.9-2.0 loaded) are now historical: the KVM strong class anchors at
the SAME 1.24-1.27 band quiet and loaded — the strong-class KVM p2
column re-banks to **1.267 loaded / 1.242 quiet**.

## Whole-table refresh (2026-10-29, leg 52/53 — the 2d3df4a7 merge passed its own tier via the pa91 chain)

### Tier-2 debug-class board + append re-bank (2026-10-30, the aarch64 merge b238ebd1)

The merge's Tier-2 run (tag main-merge-aarch64, debug bzImage, DONE
t=366s, foreign_qemus=1) read the fs family far below the pb1 column
(p6_unlink 1094 vs 32816, p6_mkdrmdir 20248 vs 83192, p6_write_fsync
12432 vs 18125 — pre-existing improvements since pb1's era) and
**p6_append 637-649 µs vs the 500.4 anchor (+27%)**. A/B attribution
(pre-merge 9fec88fb bzImage, same harness, isolated rerun):
pre-merge append med = **655.5 µs** — identical within noise, post-merge
marginally faster. Verdict: anchor drift, not a regression; the merge
passes its tier, and the append cell re-banks here: **p6_append
(debug class) = 637-656 µs family**. The other fs-family cells keep
their pb1 anchors until a full debug-class re-bank board runs; the
drift direction is improvement on every one of them. That re-bank
board ran 2026-10-31 (pa112pb battery + the pa113pb quiet rerun —
see the 2026-10-31 whole-table refresh below): debug-class fs
families now read from there, and at load1 8-10 they sit +5-6%
(metadata cells) to +38% (write_fsync, load-coupled) over these
merge-board families.

Refreshed cells from the pa91 chain (census+REST boot on the leg-52
stack pre-CFS-fix; med-of-2 pbench boards at load1 13-15, strong class)
and the pa93 boards (branch e3c1ec35, pre-merge, the mkdir-flush rows).
Provenance labels matter: the CFS min_vruntime fix landed on main AFTER
pa91's boots — sched-sensitive cells (census, p2_ctxsw) re-bank on the
next main-class board (pa94 chain armed).

| metric | old baseline | refreshed | window/provenance |
|---|---|---|---|
| TERM / REFUSED census | 8 / 32 | 8 / 32 (pa91, unchanged) | syscost census boot rc=0 |
| tick rate per CPU | 99.74-99.98 | 99.50-100.00 (pa86/pa88/pa91) | quiet-relaxed windows |
| syscall volume | ~107.5M +-10% | 85-107M swing family (pa86 87.4M, pa88 96.6M, pa91 91.4M) — enforce only load-gated; the swing class is adjudicated, not a regression family | all three boots load1 9-15 |
| clock_gettime avg | 3-4 us | 3 us (pa91 n=38.4M) | syscost |
| nanosleep avg | 203 ms | 168-190 ms family (pa86 175, pa88 168, pa91 190) | +-20% of 203 stays the band |
| GRQ avg hold | 5.1-6.1 us | 6.3-7.0 us family (pa86 7.0, pa88 6.5, pa91 6.3) — inside the 4080-7320 gate band; the 5.1-6.1 text was the pre-leg-48 class | syscost |
| small_alloc REST share | 9.2% | 1.1-1.2% (pa86/pa88/pa91) | syscost |
| get_file REST site | (not a row) | file.rs:1032: 94.5M calls / 31.2s / 330ns (was :941 132.3M / 50.8s / 383ns pre-leg-52) — NEW guarded row (fdtable) | syscost, leg-52 A+B |
| p5b_dgram send/recv | 39-43 us loaded class | 4.3/3.8 us med-of-2 (pa91q; the class moved with lever A at the SAME load1 13-15 window) | strong, loaded |
| p2_ctxsw strong | 1.37-1.39 quiet / 1.9-2.0 loaded | KVM re-bank (r8): 1.267 loaded (load 4.3-4.7, aff pin, 2 foreign qemus) / 1.242 quiet — TCG-era cells historical | strong, loaded |
| p6_append J | 85-90 us | 82.7-104 us family (pa91q 82.7/84.2, pa93j 104.4) | strong, loaded |
| p6_mkdrmdir | 15.4-16.4 ms | 495-497 us (pa93, branch e3c1ec35 PRE-MERGE — adopts when the coordinator merges) | strong, loaded, both arms |
| p6_openclose | 27.4-29.4 | 28.8/28.9 (pa91q) | strong, loaded |
| p5a_pipe / p5c3 / p5e | 2.1-2.3 / 4.9 / 4.4-4.5 | 2.17-2.24 / 4.35 / 3.92-4.12 (pa91q) | strong, loaded |
| p6_stat / getdents | 15.2-16.1 / 599-615 | 15.7 / 570-570 (pa91q) | strong, loaded |

## Whole-table refresh (2026-10-31, main@82059759 — Tier-1 re-banked on the rescued tok109 lineage)

First Tier-1 battery since the image cleanup, closing the aarch64
merge's vehicle exception. Vehicle: pa-boot.sh 780 s KVM q35 smp2/4G,
syscost=1 dfx=wwcensus, kernel = main@82059759 x86_64 debug bzImage
(md5 c800ffe3710b07b8), userdata lineage = tok109 asset (see the
note above). Two boots, each with its own recorded load window
(run/pa112tok-window.log, run/pa113tok-window.log):

| metric | pa50main anchor (2026-10-25, load1=0/qemus=1) | pa112tok (load1 3.5–9.8, 2–5 qemus) | pa113tok (load1 3.5–10.1, up to 6 qemus) | verdict vs anchor |
|---|---|---|---|---|
| TERM / REFUSED census | 8 / 32 | 8 / 32 | 8 / 32 | exact, unchanged |
| WAKE-STRANDED | (no strand band then) | 23 organic, max 32.3 s | 31 organic, max 101 s | no 1001 ms grid class (one 1002 ms one-off in pa113; organic 1–2 s family dominates; pa109dbg read 28/max 107 s — the old "0–7 organic" text was stale) |
| tick rate per CPU | ≥99.5 class | 100.00 / 99.67 | 99.58 / 100.00 | in band, sustained |
| panics / new OOM | 0 / 0 | 0 / 0 | 0 / 0 | clean |
| syscall volume (t=720 s) | 107.5M | 97.5M | 92.4M | inside the adjudicated 85–107M swing family (pa86 87.4M … pa50main 107.5M); load-coupled, NOT a regression — pa113's window carried load1 ≤10.1 + 6 foreign QEMUs, and no quieter enforcement window existed on this host day |
| clock_gettime avg | 3 µs (n=45.5M) | 3 µs (n=40.9M) | 3 µs (n=38.8M) | same class |
| nanosleep avg | 203 ms (n=26.9k) | 198 ms (n=24.2k) | 188 ms (n=25.8k) | within ±20%, better direction |
| GRQ avg hold (sched.rs REST site) | no instrument then | 3451 ns (n=24.3M) | 5367 ns (n=18.8M) | pa112's quiet-window read is BELOW the 4080 ns floor; the rerun-once boot (pa113, loadier window) reads back inside 4080–7320 ns. Reading is load-coupled: same kernel+lineage+instrument spans 3.4–5.4 µs across the two windows. In-band at standard enforcement windows; the 3.4 µs quiet floor is the improvement side, not a regression. Gate band unchanged pending a genuinely quiet (pa50main-class) window. |
| small_alloc REST share | 9.2% (old) / 1.1–1.2% family | 0.9% (14.6 s) | 1.1% (17.4 s) | in family, ≤10% band |
| get_file REST site (file.rs:1032) | (pa91: 94.5M / 31.2 s / 330 ns) | 96.9M / 26.3 s / 271 ns | 92.4M / 26.2 s / 283 ns | improved vs pa91 (avg −15%, tot −16%) |

Row provenance: run/perf-gate-pa112tok-parse.txt, run/perf-gate-pa113tok-parse.txt, gate verdicts run/pa112tok-gate-parse.txt / run/pa113tok-gate-parse.txt. Both boots rc=0 at the 780 s horizon, 0 panics, 0 OOM, census 8/32 exact, strands organic.

vs the 2026-10-25 pa50main anchor, stated plainly: no enforced Tier-1
row regressed. Census is exact (8/32). Per-op rows are same-class
(clock 3 µs, nanosleep family 188–198 ms, anchor 203 ms). Volume reads
8–14% below the 107.5M quiet anchor but inside the adjudicated
load-coupled swing family — neither 2026-10-31 window was pa50main-quiet
(load1=0 does not exist on this host day; both windows peaked >9 with
multiple foreign QEMUs). The aarch64 merge's shared-file deltas (per-arch
constants, cfg-gated arms, failure-only prints) are consistent with the
Tier-1 rows being flat.

### Tier-2 debug-class fs-family battery (2026-10-31, pa112pb1/pa112pb2 + pa113pb rerun)

Same kernel as the Tier-1 rows (main@82059759 debug bzImage, md5
c800ffe3710b07b8), pbench_boot defaults (q35 TCG thread=multi, smp2,
2G, usbless minimal rootfs), two sequential boards plus the mandated
quiet-window rerun board. Load windows: board 1 at load1=8 with 10
foreign QEMUs, board 2 at load1=10 with 10-11, rerun gate-opened at
load1=5/qemus=7 (run/pa113pb-quiet.log). Comparison families: the
aarch64-merge boards (same commit 82059759, debug, run at
foreign_qemus=1; serial work/pbench-main-merge-aarch64{,-r2}-serial.log,
med-of-2). Medians only; p99 is host-noise-dominated (board 2's
mkdrmdir p99 hit 7.1 s mid-contention).

| cell | pa112pb1 (load1=8, 10 qemus) | pa112pb2 (load1=10, 10-11 qemus) | pa113pb rerun (gate-open 5/7, drift 9-10) | pa114pb (gate-open 5/5, close 5/3) | merge family (foreign_qemus=1) | pa114pb delta vs family top | verdict |
|---|---|---|---|---|---|---|---|
| p6_stat | 209.8 | 210.2 | 210.5 | 200.9 | 197.3–198.6 | +1.2% | in ±20% |
| p6_openclose | 527.2 | 525.6 | 525.1 | 512.9 | 502.8–503.3 | +2% | in ±20% |
| p6_getdents | 5507 | 5762 | 5608 | 5439 | 5300–5327 | +2% | in ±20% |
| p6_read_cached | 558.1 | 559.7 | 558.6 | 546.8 | 531.4–533.5 | +2.5% | in ±20% |
| p6_write_fsync | 17324 | 16838 | 15852 | 13920 | 12155–12432 | +12% | in ±20% at the quiet window — monotone window chain |
| p6_append | 714.5 | 702.3 | 723.0 | 657.2 | 637.0–649.2 | +1.2% | in ±20% |
| p6_mkdrmdir | 22097 | 27375 | 23523 | 22814 | 20248–20748 | +10% | in ±20% |
| p6_unlink | 1161 | 1228 | 1177 | 1141 | 1087–1094 | +4% | in ±20% |

Adjudication (final, four boards + the merge family): every fs cell
converges MONOTONICALLY toward the merge-board family as the window
quiets — write_fsync 17324/16838 (load1 8-10, 10-11 qemus) → 15852
(5→9, 7-9 qemus) → 13920 (load1 5, 5→3 foreign qemus) vs family
12155–12432 at foreign_qemus=1; append 714/702/723/657 vs 637–649;
stat 209.8–210.5 → 200.9 vs 197.3–198.6. Same commit (82059759,
debug class) on every board. Verdict: host-window coupling, NOT a
kernel regression — the quietest window available (3 foreign QEMUs by
close) lands all eight cells inside ±20% of the merge families
(write_fsync +12%, append +1%, everything else +1-10%). Practical
consequence for the gate: p6_write_fsync (and to a lesser degree
append/mkdrmdir) must be judged with a recorded load window — at
load1 8-10 with ~10 foreign QEMUs it reads +27-40% over the
merge-quiet family with the same kernel; the merge-class 12.2-12.4 ms
family is reachable only near foreign_qemus≈1, which did not occur on
this host day until the tail of the battery. All boards: PBENCH DONE
rc=0, zero panics.

## Guarded hot-path domains (user mandate: allocation/reclaim, scheduling, filesystem)

| domain | metric | baseline (accepted) | band |
|---|---|---|---|
| alloc/reclaim | small_alloc global class-lock sections/boot | 19.7M (pa20, post-magazine) | ≤25M |
| alloc/reclaim | small_alloc REST wall (share of machine) | 133.2s ≈ 9.2% | ≤10% |
| alloc/reclaim | reclaim slowpath on standard boots | drct=0 class | no emergence |
| scheduling | GRQ average hold | 2786 ns (pa115debt1, main@afc41b8b; pre-wedge debt3 read 1215 ns same family; was 3451–5367 ns on 82059759) | 1200–7320 ns (env GRQ_AVG_NS_MIN/MAX; floor re-banked 2026-11-01, improvement side) |
| scheduling | context-switch half-rt (pbench P2) | pb1 column | ±20% |
| scheduling | fork/exit + fork/exec+reap (pbench P4, debug class) | **REGRESSION OPEN on main@afc41b8b**: fork_exit 13.2 ms / fork_exec 46.4–46.6 ms (2 boards, 0.5% apart) vs the tight 9.4–10.0 / 13.9–14.6 ms family on 82059759 | recovery target = the 82059759 family; root-cause lead: wake-behind-incumbent tick quantization in the reap handoffs (p2f battery demonstrates the class); attribution tool: forkprof (95d3d5a5) |
| scheduling | futex wake→run behind an incumbent (p2f battery, KVM strong) | quiet 7.5 µs / khog 9.99 ms / uhog 10.0 ms (pa115p2f, kpreempt off; kpreempt=1 A/B reads the same 10 ms) | quiet ±20%; hog regimes are the known tick-quantized class — the canary row for the 977adb51 batch-peer gate + kpreempt re-land |
| scheduling | census TERM/REFUSED | 8 / 32 | 8±2 / 32±4 |
| filesystem | stat / mkdir+rmdir / unlink / write+fsync medians (pbench P6) | 2026-11-01 debug-class families: stat 71 µs, openclose 156 µs, read_cached 204 µs, append 77 µs, mkdrmdir 2.76 ms, unlink 808 µs, write_fsync 14.2 ms, getdents 5.6 ms | ±20% of the 2026-11-01 column or better |
| filesystem | fsw sync-write classes | first-phase (t<90s) only | no new class |
| filesystem | buffer-cache hit rate | 99.1% | ≥ baseline |
| interrupts | IF=0 share (official KVM, if0 v3 probe on main) | 20.4% (pa115debt1; was 25.1–27.6% on 82059759) | 20.4 ±3pp — tighten at the next clean pair |
| interrupts | max closed cli segments | REST zone 6–9 ms (pa115debt1; the 594–599 ms one-shots on 82059759 are gone); hardirq-frame one-shot 210 ms = the starvation-bound inline dispatch (watch row) | REST ≤10 ms |
| interrupts | SOFTIRQ share of IF=0 | 4.1% avg (c0 2.7 / c1 5.5) | 3.1 ±2pp (in band, top half) |
| IPC | pipe roundtrip (pbench P5) | 2026-11-01 debug-class families: p5a_pipe 18.3 µs, p5b_dgram 23–41 µs, p5c/f stream 24–27 µs, p5c3 25 µs, p5d 41–46 µs | ±20% of the 2026-11-01 column or better |
| IPC | unix writev/truesize avg | 24 µs class | ±20% |
| binder | SLOW-TXN classes | queue <= 10 ms everywhere; exec-side bands keyed by code: {1,64} 4.5-9 s, {7}->Init 13-140 s | leg-35 correction: the old "15/30/89 s family" was ONE code-7->Init exec class stretched by host load (pa47smoke light-load: tight 15.0-15.1 s cluster; load1 13-17 boots: same class 18.8-77.1 s, queue still <=10 ms). leg-36 correction: the band top 95 s was fit to loaded boots only and FAILED the accepted quiet baseline itself (pa50main, load1=0: one code-7 row at 133.6 s exec, queue 0 ms) — top widened to 140 s. Operative FAIL: queue >= 50 ms (a queueing class — what a broken reclaim would introduce), a total outside the code-keyed bands, or an unknown code (leg-36 open: ONE code-14 row, 40.7 s exec queue 0 ms, branch boot pa56branch at host load 30 — no prior log has code 14; adjudicate against the pending a5af759c pure-main census boot). Machine check: `run/pa46-slowtxn.sh <serial-log>` (validates PASS on pa40/pa41gate/pa47smoke/pa46main/pa46f4r2/pa50main, spanning the de41d5ab reclaim) |

General principle (user mandate): any subsystem that can affect system
performance gets gate-code guardianship — not measured yet means "add the
DFX tool", not "skip the check".

## Tier 3 — IF=0 share (allocator / IRQ-path changes)

The EFLAGS-pair probe (syscost if0 v3, rest/hard/soft/uart zones +
per-site REST histogram) is ON main since this arc — Tier-3 is now an
official measurement, not a proxy. Current accepted read (pa115debt1,
main@afc41b8b, KVM q35 smp2): IF=0 = 20.4% of machine (c0 20.8 / c1
20.0), heal miss = 0, REST-zone max closed cli segments 6–9 ms (≤10 ms
band; the 594–599 ms one-shot class on 82059759 is gone), SOFTIRQ
share of IF=0 4.1% avg. Historical anchor: pa27 read 25.7% pre
phantom-bits. The old interim proxy (tick-mode kernel share 83–86%)
is retired, and the tick-counter semantics changed on this arc
(hooks fire beyond jiffies) — that proxy row must not be re-derived
from this arc's counters without fixing the accounting first.

## Notes

- The pbench probe, syscost, census probes, and the quiet-boot methodology
  are all in-tree or in the perf-audit lane's run scripts — the gate is
  self-contained apart from the Tier-3 EFLAGS probe patch (to be upstreamed,
  gated off by default).
- Host load caveat: run gate boots when the host is not saturated; a boot
  under foreign QEMU load can shift volume rows — rerun once before calling
  a regression (same rule as the LTP flake policy).


### preempt-hunt + perf-sprint merge gate (2026-11-01, b96a2e48, tag preempt-hunt)

Ran under load (5 foreign QEMUs — wedge-fix + review lanes active; the
documented load-coupled caveat applies). Verdict split:
- **Tier 2 fs family: the perf sprint shows through — improvements, not
  regressions** (anchor = old pb1 column): append 83.2 vs 500.4 (6x
  faster), mkdrmdir 2829 vs 83192 (29x), unlink 851 vs 32816 (39x),
  p1_null 3.50 vs 4.57 (faster). These FAIL flags are the anchor being
  stale-slow; per the "±20% or improve" rule they PASS.
- **Tier 1 quiet rows: invalid under this load window** (clock_gettime
  27µs, nanosleep 930ms avg — pure host-load interference; census
  TERM 7 in band, WAKE-REFUSED 17 below band = the preempt-hunt fix
  working, fewer refused wakes). Rerun-once queued for a quiet window;
  the sched-domain verdict (guarded: sync/ spinlock change) rests on
  that rerun plus the lane's own OH-image numbers (wake-strands 233→31,
  SLOW-TXN median 5780→1970ms).

## Whole-table refresh (2026-11-01, main@afc41b8b — the 2026-11-01 sched/sync/softirq debt cleared)

Closes the Tier-2 debt of the 2026-11-01 merges (including the
preempt-hunt rerun queued above): preempt-hunt's bounded spin
(5f0537a8, then realigned on-tip by bd9c0881 spinlocks-never-sleep),
the heal stack rounds 1-3 (d2381bf9/264c61b2/8387c00b), phantom-bits
softirq dispatch gating + drain bounds + starvation bound
(1854ec4d/bef10f42), kpreempt default OFF (28b1afca), plus the
same-arc merges the board attributes mechanically: 6394e8ee (x86_64
LAPIC one-shot re-arm — hres sleeps stop waiting for the 10 ms grid),
977adb51 (idle-CPU dispatch + sleep-gated wakeup preemption,
perf-ipc), and the sync-realign SleepMutex conversions
(dd5f3731/a9825b45/ab397dfd).

Vehicle: same rig as the 2026-10-31 re-bank (pa-boot 780 s KVM q35
smp2/4G, syscost=1 dfx=wwcensus, tok109 lineage), kernel =
main@afc41b8b x86_64 debug bzImage md5 6fe271cbf910ec667bf8f2136e0bee83
(strong twin for the p2f rig: 974d519ef141e513e3a9223abd3d8939).
Evidence: run/pa115debt1-gate.txt, run/perf-gate-pa115debt1-parse.txt,
run/pa115debt1-window.log, run/pa115debt2-gate.txt,
run/pa115p2f-boot.log, run/pa115p2fk-boot.log.

### Tier-1 board (pa115debt1, load1 6.4–11.3, 3–5 foreign QEMUs, rc=0 at 780 s)

| metric | pa113tok anchor (main@82059759) | pa115debt1 (main@afc41b8b) | verdict |
|---|---|---|---|
| TERM / REFUSED census | 8 / 32 | 8 / 36 (band top; debt3 read 30) | in band. The intermediate-tree sub-floor reads (preempt-hunt 17, preempt-heal 12) did not survive the later merges — the tip reads in-band; band 8±2 / 32±4 HOLDS, no re-bank |
| WAKE-STRANDED | 23–31 organic, max 32–107 s | 5 organic (debt3: 12) | in band (fewer), no 1001 ms grid class |
| tick rate per CPU | 99.58–100.00 | counter advances 124–182/s class on this arc — the tick-accounting hooks now fire beyond jiffies; row passes ≥99.5 trivially | PASS + accounting-semantics note (needs an owner pass; not a perf change) |
| panics / new OOM | 0 / 0 | 0 / 0 | clean |
| syscall volume (t=720 s) | 92.4–97.5M (85–107M swing family) | 126.0M (+36% vs pa113) | **re-banked: 126M ±10% in a load-gated window.** Same-depth corroboration: pa115debt3 (same kernel, wedged later under load) paced 8.0M@60 s → 18.1M@120 s, the same ~10–11M/min regime debt1 ran from 24.8M@180 to 126.0M@720. Mechanism: 6394e8ee un-quantizes ms-class sleeps, userspace pacing loops run at true rate, the boot goes ~36% deeper in the same wall. Per-op avgs flat or better — throughput, not cost |
| clock_gettime avg | 3 µs (n=38.8M) | 3 µs (n=54.0M) | same class, deeper n — the preempt-heal SKIP rows from the quiet rerun are now measured |
| nanosleep avg | 188–198 ms | 158.4 ms (n=29.5k) | within the one-sided ≤244 ms band (improvement side); family note extends to 158 ms |
| GRQ avg hold (sched.rs REST site) | 3451–5367 ns | 2786 ns (n=6.3M, tot 17.6 s; pa113 tot was 100.9 s at n=18.8M) | **floor re-banked: gate band 1200–7320 ns.** avg −48%, charged total −82% vs pa113. Single clean sample (the debt3 rerun wedged, see below); debt3's pre-wedge read 1215 ns is the same family |
| small_alloc REST share | 0.9–1.1% | 0.4% (6.2 s); section count 6.0M vs 18.8M | improved, ≤10% band |
| get_file REST site | file.rs:1032: 92.4M / 26.2 s / 283 ns | file.rs:1110: 124.8M / 34.8 s / 278 ns | avg flat, n +35% (deeper boot); the old 13.8 ms-avg file.rs:1077 outlier site left the top table |

### Tier-3 / IRQ-domain rows (same boot; the if0 v3 probe is now ON main — official measurement, no proxy)

| metric | old band | pa115debt1 | verdict |
|---|---|---|---|
| IF=0 share of machine | 25.7 ±2pp (pa27) | **20.4%** (c0 20.8 / c1 20.0); pa112 read 25.1%, pa113 27.6% — a real ~5–7 pp drop | **re-banked: 20.4 ±3pp** (single clean sample — tighten at the next clean pair). Mechanism: phantom-bits defers softirq dispatch out of irq_exit IF=0 windows into schedulable ksoftirqd; bounded drains |
| SOFTIRQ share of IF=0 | 3.1 ±2pp | c0 2.7% / c1 5.5% (avg 4.1%) | in band |
| max closed cli segments (REST zone) | 3–7 ms class, ≤10 ms | **6 ms (c0) / 9 ms (c1)** — pa112/pa113 carried 594–599 ms one-shots | back inside the band; the 594–599 ms one-shot class on 82059759 is gone (preempt-hunt bounded spin + heal stack + softirq bounds) |
| hardirq-frame zone max | (not banded) | one-shot 210 ms (c1) | the bef10f42 20 ms starvation bound forces one inline dispatch whose bounded drain (64 packets/source) can still stretch the hardirq frame; bounded, one-shot, REST zone unaffected. Watch row, no band yet |
| heal counters | (new instrument class) | 64.5k/52.1k heals, 605 ms total, miss=0 (pa113: 154k/132k) | heal volume −58%, zero model-vs-reality misses |

### Tier-2 debug-class board (pa115debt2, q35 TCG thread=multi smp2/2G, DONE t=411 s rc=0, load1 9–11 / 3–5 foreign)

Probe provenance: test/pbench.c row implementations are byte-identical
to the pa114pb board's probe (the Nov 1 probe only ADDS the p2f battery
and a riscv timer reader — verified by diff) — the shifts below are
kernel behavior, not probe drift. The pa115debt4 rerun board (launched
load1 8.5, tail under 18–22, DONE t=258 s rc=0) reproduces every
family within 0.4–11% (p4 rows within 0.5%); its cells confirm the
re-banks below are stable across windows. TCG context for the p2f
battery: both debug boards read khog w2r 131–132 µs — the tick
quantization is a KVM-rig behavior (TCG's cooperative scheduling
preempts fine); the banked p2f row is the KVM-strong one per the
x86_64-first platform priority.

| cell | pa114pb (main@82059759) | pa115debt2 (main@afc41b8b) | delta | verdict |
|---|---|---|---|---|
| p1_null | 4.853 | 3.477 | −28% | improved (same-arc hot-path work, 4ce0cda5); re-bank family 3.5 µs |
| p1_clock | 5.124 | 4.188 | −18% | in ±20% |
| p2_ctxsw | 12.022 | 12.494 | +3.9% | **flat on the primary board — the scheduling-domain P2 row holds**; the debt4 rerun board read 15.150 (+21% vs debt2, +26% vs the 12.0–12.6 anchor family) — marginal-out sample under its load spike (board tail ran at load1 18–22); two-board family 12.5–15.2, WATCH row, not a regression call at these windows |
| p5a_pipe | 19.673 | 18.306 | −6.9% | flat (blocker handoffs untouched by the wake gate, as designed) |
| p5b_dgram_send / recv_peek | 318.6 / 318.3 | 23.1 / 40.9 | −93% / −87% | 977adb51 (author lane measured the same 321.8→23.8 class); **family re-bank: 23–41 µs** |
| p5c_stream_write / c2_send / f_dwait | 276.5–277.6 | 24.9–26.8 | −90–91% | same; **family re-bank: 24–27 µs** |
| p5c3_stream_wrecv | 32.7 | 25.0 | −24% | improved, in ±20% anyway |
| p6_stat | 200.9 | 71.1 | −65% | **family re-bank: 71 µs** (SleepMutex ext4/VFS conversions + same-arc) |
| p6_openclose | 512.9 | 156.3 | −70% | **family re-bank: 156 µs** |
| p6_read_cached | 546.8 | 203.7 | −63% | **family re-bank: 204 µs** |
| p6_getdents | 5438.8 | 5638.2 | +3.7% | flat, in ±20% |
| p6_write_fsync | 13919.9 | 14186.4 | +1.9% | flat, in ±20% |
| p6_append | 657.2 | 77.4 | −88% | **family re-bank: 77 µs** |
| p6_mkdrmdir | 22814.3 | 2757.3 | −88% | **family re-bank: 2.76 ms** |
| p6_unlink | 1141.4 | 808.0 | −29% | improved, in ±20% anyway |
| p7_sleep1ms (1 ms-sleep overshoot) | 1795.4 | 54.4 | −97% | 6394e8ee hres sleeps off the 10 ms grid; **family re-bank: 54 µs** |
| p4_fork_exit | 9.4–10.0 (tight 4-board family) | 13.2 | **+38%** | **CONFIRMED REGRESSION** — the pa115debt4 rerun board (launched at load1 8.5/4 qemus) read 13262 µs, within 0.5% of debt2's 13196: two boards agree vs a family that sat tight at 9.4–10.0 under load1 8–11. Root-cause lead: the p2f battery demonstrates a 10 ms wake-behind-incumbent quantization on this tip (below), and p4's reap handoffs are that shape; the perf-fork lane's forkprof (95d3d5a5) is the built attribution tool |
| p4_fork_exec | 13.9–14.6 (tight 4-board family) | 46.4 | **+231%** | **CONFIRMED REGRESSION** — rerun read 46620 µs (+0.4% vs debt2). +32 ms ≈ 3 ticks + the fork-side cost; same lead. This row BLOCKS a clean Tier-2 verdict for the arc — every other P1–P7 row is improved or flat |

### p2f futex wake→run battery (KVM, strong kernel 974d519e, kpreempt default OFF — the 28b1afca debt row)

| row (med, n=20000) | perf-sched anchors (6ab42522/28b1afca) | pa115p2f (kpreempt off) | pa115p2fk (kpreempt=1 A/B) |
|---|---|---|---|
| p2f_hogcal (pread4m) | KVM ~160 µs | 146.7 µs | 156.4 µs |
| p2f_futex_w2r (quiet) | 7.6 µs | **7.49 µs** | 7.61 µs |
| p2g_futex_khog_w2r | hook-off 163 µs / hook-on 9.5 µs | **9.99 ms** (b2lt10ms 10673 / b3ge10ms 9087 / b0lt100us 239) | **10.0 ms** (b2 17353 / b3 2567) |
| p2h_futex_uhog_w2r | — | **10.0 ms** (b2 19029 / b3 925) | 10.0 ms |

**Re-banked rows**: quiet futex wake→run 7.5 µs (unchanged class);
khog and uhog wake→run behind a running incumbent **~10 ms
(tick-quantized), both hog regimes, both kpreempt settings** — the
163 µs hook-off anchor is superseded. The A/B is the load-bearing
finding: arming kpreempt=1 on this tip does NOT restore the 9.5 µs
class, because 977adb51's sleep-gated wakeup preemption (batch-peer:
wakee ran within one latency window ⇒ no need_resched set) never
raises the flag the hook acts on, and a pure userspace incumbent has
no syscall exit to preempt at. The 28b1afca re-land condition "(2)
the p2f futex wake→run gains still hold" is therefore NOT satisfiable
by the hook alone on this tip — re-landing needs the batch-peer gate
and the hook addressed together. This is the honest cost side of the
9–13x p5 IPC wins and the +36% boot depth banked above; the p2f rows
are the canary for it.

### pa115debt3 (rerun boot) — load-wedge repro, not a Tier-1 sample

The rerun window collapsed (load1 9.4 → 25.7 while other lanes
booted). The boot wedged at t≈120 s: PREEMPT-LEAK storm, pid 138
current on cpu0 in `arch::x86_64::cpu::wfi` (rip ffffffff805ac6f5)
with pc=0x10000 (softirq field) held for 63k+ jiffies unhealed;
3715 leak prints vs 123 boot-wide heals; syscost dumps
(softirq-scheduled) stopped at t=120 s while jiffies kept advancing —
the softirq dispatch machinery stalled with the bit booked. The first
leak of the boot was pid=1 pc=0x10000 rip=ffffffff80626573 =
`fs::ext4::dir::Ext4DirEntry` (booked during ext4 dir ops, healed at
2 s). The same kernel ran a clean 780 s rc=0 in the quieter window
(pa115debt1), so the wedge is load-triggered/stochastic, not
deterministic. Evidence: run/pa115debt3-wedge-excerpt.txt (full
serial kept lane-local). Filed for the owning lane: the round-3
per-field absence-stamp heal (8387c00b) did not clear a
wfi-resident softirq-field phantom under vCPU starvation.
