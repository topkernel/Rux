# LTP Pass-Rate Tracking (living document)

Goal: 100% of the 1,869-test guest-side list. One line per sweep; a merge
is only "verified" after a sweep shows no regression. Historical context
(previous machine): PASS trend 35 → 524 → 659 → 677 → 602 → 721 → 771 →
673 → 801 → 795 (r1–r10).

| # | Date | Commit | PASS | FAIL | TBROK | TCONF | TIMEOUT | Δ PASS | Notes |
|---|------|--------|------|------|-------|-------|---------|--------|-------|
| r1–r10 | ≤10-20 | (previous machine) | 35→795 | | | | | | trend: 35, 524, 659, 677, 602, 721, 771, 673, 801, 795 |
| r14 | 10-25 | leg-3 baseline | 945 | | | 522 | 42 | +150 | CRASH=49; pre-do_exit-fix TIMEOUT family still present |
| r15 | 10-26 | 743ea3ca | 1005 | 243 | (in FAIL) | 524 | 22 | +60 | CRASH=33 EXECFAIL=32 POISON=10; OH-lane do_exit fix converted TIMEOUT 42→22, CRASH 49→33. Artifact: `.ltp/results-r15.txt` (verified: rows sum to 1869). All 32 EXECFAIL are rc=127 image gaps (missing test binaries, e.g. memcontrol01-04, k-tree-gen) — not kernel bugs. Top FAIL families after the closed-list filter (musl sbrk stubs / clone08 LTP-side / send02 wall-clock): mmapstress 5 (usage-arg artifacts + musl brk), prctl 4, mmap 4, epoll_wait 3, madvise 3, mount 3, pidns 3, ftest 3, timer_settime 2×(6 sub-fails), setrlimit 2. 10 POISON rows parked at serial stalls — rerun in isolation (flake rule) before trusting. |
