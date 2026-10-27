# LTP Pass-Rate Tracking (living document)

Goal: 100% of the 1,869-test guest-side list. One line per sweep; a merge
is only "verified" after a sweep shows no regression. Historical context
(previous machine): PASS trend 35 → 524 → 659 → 677 → 602 → 721 → 771 →
673 → 801 → 795 (r1–r10).

| # | Date | Commit | PASS | FAIL | TBROK | TCONF | TIMEOUT | Δ PASS | Notes |
|---|------|--------|------|------|-------|-------|---------|--------|-------|
| — | — | (baseline pending: LTP building, rootfs rebuild next) | | | | | | | |

## Subset runs (merge-gate evidence)

- 2026-10-25 pa48 (leg 35, perf-audit branch @ d3cc1cb0-class debug,
  kernel = main ddb3ed01 + b3757d92 socket fast path): the 32
  socket-family rows of the fleet sweep list (indices preserved), one
  boot, SWEEP DONE, 0 panics. 30/32 states == the fleet's latest
  main-class result; recv01 (CRASH vs FAIL) and recvmsg03 (TCONF vs
  FAIL) are states the fleet's own main-class history already shows.
  No NEW failure class — closes the socket-fast-path merge gate
  (evidence: run/pa-pa48sock-r1-serial.log, driver
  run/pa48-ltpsock.sh, list run/pa48-sweep-socket-subset.list).
