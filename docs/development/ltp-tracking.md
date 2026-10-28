# LTP Pass-Rate Tracking (living document)

Goal: 100% of the 1,869-test guest-side list. One line per sweep; a merge
is only "verified" after a sweep shows no regression. Historical context
(previous machine): PASS trend 35 → 524 → 659 → 677 → 602 → 721 → 771 →
673 → 801 → 795 (r1–r10).

| # | Date | Commit | PASS | FAIL | TBROK | TCONF | TIMEOUT | Δ PASS | Notes |
|---|------|--------|------|------|-------|-------|---------|--------|-------|
| — | — | (baseline pending: LTP building, rootfs rebuild next) | | | | | | | |
| r1–r10 | ≤10-20 | (previous machine) | 35→795 | | | | | | trend: 35, 524, 659, 677, 602, 721, 771, 673, 801, 795 |
| r14 | 10-25 | leg-3 baseline | 945 | | | 522 | 42 | +150 | CRASH=49; pre-do_exit-fix TIMEOUT family still present |
| r15 | 10-26 | 743ea3ca | 1005 | 243 | (in FAIL) | 524 | 22 | +60 | CRASH=33 EXECFAIL=32 POISON=10; OH-lane do_exit fix converted TIMEOUT 42→22, CRASH 49→33. Artifact: `.ltp/results-r15.txt` (verified: rows sum to 1869). All 32 EXECFAIL are rc=127 image gaps (missing test binaries, e.g. memcontrol01-04, k-tree-gen) — not kernel bugs. Top FAIL families after the closed-list filter (musl sbrk stubs / clone08 LTP-side / send02 wall-clock): mmapstress 5 (usage-arg artifacts + musl brk), prctl 4, mmap 4, epoll_wait 3, madvise 3, mount 3, pidns 3, ftest 3, timer_settime 2×(6 sub-fails), setrlimit 2. 10 POISON rows parked at serial stalls — rerun in isolation (flake rule) before trusting. |
| r16 | 10-26 | leg-7 fix batch (3eedbce3+2a3e75c3+b93d888b class, build 13:12) | 1016 | 236 | (in FAIL) | 525 | 21 | +11 | CRASH=30 EXECFAIL=32 POISON=9. vs r15: +17 gained (the leg-7 flips timer_settime01/02, madvise02, prctl02, epoll_wait05/07, setrlimit01/03, open13 + POISON conversions newuname01/writev01/fcntl14 + main-merge conversions copy_file_range01, fsx-linux, getrusage04, llseek01, mem02), −6 lost: clock_gettime04 and ftest04 FAIL, 4 POISON stalls (getpid01, setregid03, waitid02, waitid11). Artifact: `.ltp/results-r16.txt`. |
| r17 | 10-27 | dfd8e674 (merge origin/main into the lane branch; includes a2dbc5cf fdtable close_range fix) | 1022 | 234 | (in FAIL) | 522 | 22 | +6 | CRASH=31 EXECFAIL=32 POISON=6. vs r16: +10 gained (close_range01/02, ftest04 back, kallsyms, prctl03, prctl08 + the 4 r16 POISON rows recovered), −4 lost: creat05 TIMEOUT, fork09 FAIL, fcntl34+shmctl05 POISON. Net r15→r17: +22/−5; the −5 (clock_gettime04, creat05, fcntl34, fork09, shmctl05) got strict 2x isolation reruns on the r17 kernel — see the leg-8 note. Artifact: `.ltp/results-r17.txt`. |
| r18 | 10-28 | ce5ab2d0 (5f578c3a zero-serve completion fix + origin/main 584c1105; scan kernel built clean at 07:15 — later chunks rode the e26f56f7 exthunt-instrument rebuild, dormant/OFF by default) | 1021 | 231 | (in FAIL) | 524 | 23 | −1 | CRASH=30 EXECFAIL=32 POISON=8. vs r17: +4 gained (clock_gettime04, float_iperb, ftest06, semop04), −5 lost. Strict 2x isolation on the −5: ftest04 2/2 FAIL and ftest05 2/2 FAIL = REAL standing failures (the deterministic zzprobe4 family — the r17 PASSes were the lucky side; not branch regressions); futex_wake02, mkdir09, mmapstress01 2/2 PASS = scan-host flakes. Effective: +4/−2 real. Artifact: `.ltp/results-r18.txt`, verdict logs `.ltp/diag/iso18-*.log`. |

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
