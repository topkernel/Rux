# Merge-Gate Performance Baseline (user policy 2026-10-25)

Last-accepted baseline for the merge gate defined in AGENTS.md. Updated only
by a merge that passed its own gate tier. Numbers below are seeded from the
perf-audit lane's accepted measurements on main@257698a5 (pa38 battery,
pb1/pa-series, census streaks) — refresh the whole table at once when a new
main lands, do not cherry-pick rows.

## Tier 1 — quiet syscost boot (KVM, q35, smp2/4G, syscost=1, 780s window)

| metric | baseline | band |
|---|---|---|
| TERM-DESCHED census | 8 | 6–10 |
| WAKE-REFUSED census | 32 | 28–36 |
| WAKE-STRANDED | 0–7 organic (~1–1.5s) | no 1001ms-grid class |
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

## Guarded hot-path domains (user mandate: allocation/reclaim, scheduling, filesystem)

| domain | metric | baseline (accepted) | band |
|---|---|---|---|
| alloc/reclaim | small_alloc global class-lock sections/boot | 19.7M (pa20, post-magazine) | ≤25M |
| alloc/reclaim | small_alloc REST wall (share of machine) | 133.2s ≈ 9.2% | ≤10% |
| alloc/reclaim | reclaim slowpath on standard boots | drct=0 class | no emergence |
| scheduling | GRQ average hold | 5.1–6.1 µs flat | ±20% |
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
