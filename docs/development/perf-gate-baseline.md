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
| tick rate per CPU | 99.74–99.98 /s | ≥99.5 sustained |
| kernel panics / new OOM classes | 0 | 0 |
| total syscall volume (780s boot) | ~136M (pa38) | ±10% |
| clock_gettime avg | 3–4 µs | ±20% or better |
| nanosleep avg | ~221 ms post-tick-fix | ±20% or better |

## Tier 2 — pbench P1-P7 medians (main-class kernel, usbless image)

Banked Rux guest run: pb1 (pa31-class kernel, DONE t=378s, 25/25 lines).
Compare medians only (p99 is host-noise-dominated on a loaded host).
Banked Linux reference column (same image, OHOS 6.6.11 kernel, med-of-4):
perf-report.md §8.5. Rux medians must stay within ±20% of the pb1 column
per metric or improve.

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
