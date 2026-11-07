# LTP TIMEOUT-class audit (2026-11-07)

**Trigger**: user directive — audit every TIMEOUT in the current LTP
census and resolve them first (P0-class rule: a TIMEOUT is a
"cannot-run" verdict, prioritized over optimization work).

**Source**: r27 census (`rux-agents/ltp-r27/results/results-r27.txt`,
main@e23d3c3f, x86_64+KVM, 1,869 list). Verdict distribution:
1,084 PASS / 497 TCONF / 228 FAIL / 28 CRASH / 17 EXECFAIL /
**13 TIMEOUT + 1 WEDGE**.

**Method**: every row re-run one-test-per-boot on current main
(`scripts/test/ltpscan/scan-x86_64.py --chunk 1`; in-guest runner
gives each test a 30 s budget, then pid-1 SIGTERMs it and reports
rc=143), with the LTP 20240524 sources open beside the serials.

## Disposition (all 14 rows)

| Test | Class on main | Verdict | Evidence |
|---|---|---|---|
| ptrace06 | TIMEOUT | **FIXED** → PASS (f558181b) | tracer blocked in its first wait4: a TRACEME'd vfork+exec child never took the exec SIGTRAP stop. Fix queues the trap at exec commit; do_signal's traced arm converts it to the tracer-reported stop. Also EIO errno mapping for bad PEEK/POKE/REGS/SIGINFO parameters |
| shmat1 | CRASH 5/5 | **test defect on x86_64** | the SIGSEGV recovery block is `#ifdef __i386__`; the 64-bit handler body is `fprintf("caught signal %d -- exiting"); exit(-1)`. The test's designed shm-detach race (reader touching `map_address` across shmdt) fires within ~200 iterations on our timing; all 5 crashes fault at exactly `map_address` (kernel USEGV-FOR forensics). Kernel side verified correct by probes: thread-directed fault delivery, SA_SIGINFO handler replacement, siglongjmp recovery all work (`shmsigprobe`). Also runs `alarm(exec_time*3600)` = 24 h by design |
| openfile | TIMEOUT | **known lost-wake family** | main parks in futex (join) after "Creating Reading Threads"; the reader threads are gone. STRAGGLER-RESCUE fired in-boot and recovered one lost mid-exit thread (member INTERRUPTIBLE, on_cpu=-1) — the drain unblocked, but the join's wake was already lost. Owned by the lost-wake producer hunt (TraceRing::push lead) |
| ht_affinity | TIMEOUT | slow-progress, not hung | zero TFAIL; ~1 affinity mask per 30 s budget (loop = cpu_count masks × child spawn + sleep(1)-based HT sync, twice). Needs a timing dig; not a wedge |
| mmap1 | FAIL(int) | slow-by-design | 274,152 verify iterations PASS inside the 30 s budget, then the LTP-internal timeout TBROKs it (the census r26 verdict already documents this budget/racy class) |
| cgroup_fj_proc | TIMEOUT | by-design helper | `do {...} while (test_switch)` — runs until its parent suite signals it |
| cgroup_regression_fork_processes | TIMEOUT | by-design helper | `while (1)` fork loop |
| cpuctl_fj_cpu-hog | TIMEOUT | by-design helper | `sigsuspend` waiting for the parent's SIGUSR1 |
| fork_exec_loop | TIMEOUT | by-design helper | `while (1)` fork+exec loop (freezer suite load generator) |
| memcg_test_2 | TIMEOUT | by-design helper | `while (1)` |
| memcg_test_4 | TIMEOUT | by-design helper | `while (1) sleep(1)` |
| pids_task2 | TIMEOUT | by-design helper | `pause()` |
| pthserv | TIMEOUT | by-design helper | `for (;;)` accept loop (server until killed) |
| timed_forkbomb | TIMEOUT | by-design helper | reads its bomb program from stdin (the parent pipes it); standalone blocks on scanf |
| mmap1 (WEDGE row) | — | same as above | the r27 WEDGE row is the same test under chunked load |

**Net kernel defects found in the TIMEOUT class: 2** (the ptrace
exec-trap + errno mapping — both fixed in f558181b; ptrace family
01/02/03/05/06/11 all PASS one-per-boot after the change).
**Test/list defects: 11** (9 by-design helpers + shmat1 + mmap1
budget). **1 carried by the open producer hunt** (openfile).

## Follow-ups

1. **Census hygiene**: the 9 by-design helpers should carry a
   BY-DESIGN marker in future censuses (runner-side list or verdict
   post-processing) so they stop reading as kernel TIMEOUT
   regressions.
2. **r28 census**: will pick up ptrace06 PASS and the reclassifications.
3. **ht_affinity**: per-step timing dig (affinity syscall + signal
   handshake cost).
4. **openfile**: rides the lost-wake producer hunt; the rescue
   machinery bounds the drain side.
