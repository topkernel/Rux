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
| total syscall volume (780s boot) | ~107.5M (pa50main, 2026-10-25) | ±10% IN A COMPARABLE HOST-LOAD WINDOW — re-banked 2026-10-25 from the pa50main quiet-window run (PURE main ddb3ed01, x86_64 debug, load1=0/qemus=1 at launch, all other enforced rows PASS); the old 136M seed was the pa38 boot at host load ~5.5 with the app phase running 81 ABC restarts (a deeper workload mix) — superseded. Pace-coupling stands: pa41gate (leg 32) proved a flat −35% pace deficit under 14–19 foreign QEMUs (77.1M, per-op costs held); pa46main at load1 11–17 read 93.2M. Enforce only in a load-gated window (load1 ≤9, qemus ≤8 — run/pa50-rebank.sh gate) |
| clock_gettime avg | 3–4 µs | ±20% or better |
| nanosleep avg | 203 ms (pa50main, n=26,945) | ≤244 ms (203 ms ±20%; enforced 2026-10-25 — observed cross-run range 174–222 ms across pa41gate/pa46main/pa46f4r2/pa27, workload-mix dependent but the mix is stable in this image class; the old ~221/333 ms seeds predate the tick-fix-era steady state) |

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
drift direction is improvement on every one of them.

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
| p2_ctxsw strong | 1.37-1.39 quiet / 1.9-2.0 loaded | 1.30-1.31 at load1 13-15 (pa91) — re-banks post-CFS | strong, loaded |
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

### Tier-2 debug-class fs-family battery (2026-10-31, pa112pb1/pa112pb2)

Same kernel as the Tier-1 rows (main@82059759 debug bzImage, md5
c800ffe3710b07b8), pbench_boot defaults (q35 TCG thread=multi, smp2,
2G, usbless minimal rootfs), two sequential boards. Load window:
board 1 at load1=8 with 10 foreign QEMUs on the host, board 2 at
load1=10 with 10-11 — the comparison families are the aarch64-merge
boards (same commit 82059759, debug, run at foreign_qemus=1; serial
work/pbench-main-merge-aarch64{,-r2}-serial.log, med-of-2). Medians
only; p99 is host-noise-dominated (board 2's mkdrmdir p99 hit 7.1 s
mid-contention).

| cell | pa112pb med-of-2 | merge-board quiet family | delta | verdict |
|---|---|---|---|---|
| p6_stat | 210.0 (209.8/210.2) | 197.3–198.6 | +6% | in ±20% |
| p6_openclose | 526.4 (527.2/525.6) | 502.8–503.3 | +5% | in ±20% |
| p6_getdents | 5634 (5507/5762) | 5300–5327 | +6% | in ±20% |
| p6_read_cached | 558.9 (558.1/559.7) | 531.4–533.5 | +5% | in ±20% |
| p6_write_fsync | 17081 (17324/16838) | 12155–12432 | +38% | OUT of band — rerun-once armed (run/pa113pb-quiet.sh, gate load1≤5/qemus≤4) |
| p6_append | 708.4 (714.5/702.3) | 637.0–649.2 | +9–11% over family top | marginal, in ±20% of top |
| p6_mkdrmdir | 24736 (22097/27375) | 20248–20748 | +19% vs top | at the ±20% edge, tail shows contention |
| p6_unlink | 1194 (1161/1228) | 1087–1094 | +9% | in ±20% |

Adjudication so far: the CPU-metadata cells (stat/openclose/
getdents/read_cached) sit at a uniform +5-6% — consistent, small,
inside band. The three I/O-heavy cells are elevated with extreme
tails, and the only variable between these boards and the comparison
family is host contention (10-11 foreign QEMUs vs 1). The write_fsync
row gets its mandated quiet-window rerun (pa113pb) before any
regression call; verdict appended when it lands. All boards: PBENCH
DONE rc=0, zero panics.

## Guarded hot-path domains (user mandate: allocation/reclaim, scheduling, filesystem)

| domain | metric | baseline (accepted) | band |
|---|---|---|---|
| alloc/reclaim | small_alloc global class-lock sections/boot | 19.7M (pa20, post-magazine) | ≤25M |
| alloc/reclaim | small_alloc REST wall (share of machine) | 133.2s ≈ 9.2% | ≤10% |
| alloc/reclaim | reclaim slowpath on standard boots | drct=0 class | no emergence |
| scheduling | GRQ average hold | 3.4–5.4 µs window-spanned on main@82059759 (pa112tok quiet-window 3451 ns, pa113tok loadier-window 5367 ns; older family 6.3–7.0 µs at load1 9–15) | 4080–7320 ns gate band (env GRQ_AVG_NS_MIN/MAX); quiet-window floor 3.4 µs is improvement-side, band re-bank pending a pa50main-class (load1≈0) window |
| scheduling | context-switch half-rt (pbench P2) | pb1 column | ±20% |
| scheduling | census TERM/REFUSED | 8 / 32 | 8±2 / 32±4 |
| filesystem | stat / mkdir+rmdir / unlink / write+fsync medians (pbench P6) | pb1 column | ±20% or better |
| filesystem | fsw sync-write classes | first-phase (t<90s) only | no new class |
| filesystem | buffer-cache hit rate | 99.1% | ≥ baseline |
| interrupts | IF=0 share (official KVM) | 25.7% | ±2pp |
| interrupts | max closed cli segments | 3–7 ms | ≤10 ms |
| interrupts | SOFTIRQ share of IF=0 | 3.1% | ±2pp |
| IPC | pipe roundtrip (pbench P5) | pb1 column | ±20% |
| IPC | unix writev/truesize avg | 24 µs class | ±20% |
| binder | SLOW-TXN classes | queue <= 10 ms everywhere; exec-side bands keyed by code: {1,64} 4.5-9 s, {7}->Init 13-140 s | leg-35 correction: the old "15/30/89 s family" was ONE code-7->Init exec class stretched by host load (pa47smoke light-load: tight 15.0-15.1 s cluster; load1 13-17 boots: same class 18.8-77.1 s, queue still <=10 ms). leg-36 correction: the band top 95 s was fit to loaded boots only and FAILED the accepted quiet baseline itself (pa50main, load1=0: one code-7 row at 133.6 s exec, queue 0 ms) — top widened to 140 s. Operative FAIL: queue >= 50 ms (a queueing class — what a broken reclaim would introduce), a total outside the code-keyed bands, or an unknown code (leg-36 open: ONE code-14 row, 40.7 s exec queue 0 ms, branch boot pa56branch at host load 30 — no prior log has code 14; adjudicate against the pending a5af759c pure-main census boot). Machine check: `run/pa46-slowtxn.sh <serial-log>` (validates PASS on pa40/pa41gate/pa47smoke/pa46main/pa46f4r2/pa50main, spanning the de41d5ab reclaim) |

General principle (user mandate): any subsystem that can affect system
performance gets gate-code guardianship — not measured yet means "add the
DFX tool", not "skip the check".

## Tier 3 — IF=0 share (allocator / IRQ-path changes)

Official post-tick-fix KVM flagship (pa27): IF=0 = 25.7% of machine, heal
miss = 0, max closed cli segments 3–7 ms. Interim proxy until the EFLAGS
probe patch lands on main: syscost tick-mode kernel share 83–86%.

## Notes

- The pbench probe, syscost, census probes, and the quiet-boot methodology
  are all in-tree or in the perf-audit lane's run scripts — the gate is
  self-contained apart from the Tier-3 EFLAGS probe patch (to be upstreamed,
  gated off by default).
- Host load caveat: run gate boots when the host is not saturated; a boot
  under foreign QEMU load can shift volume rows — rerun once before calling
  a regression (same rule as the LTP flake policy).
