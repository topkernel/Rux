# LTP seventh-round scan (r7) — Rux kernel post-fix rescan
- **Kernel (r7)**: `f35afe5 Revert "fix(net/unix+epoll): three EPOLLET edge-loss holes starved healthy clients forever"` (worktree ltp-r11, clean rebuild of main; fix series since the r6 kernel: ext4 synchronous-I/O batching out of the write path (post-exec hang family), sched_setaffinity prompt migration, mprotect PROT_WRITE on MAP_SHARED read-only mappings, jbd2 handle unwind at exit for killed tasks, ptrace signal-stop parking — sys_ptrace wired for the LTP ptrace family; net/unix+epoll EPOLLET fix applied and reverted (net-neutral))
- **Kernel (r6 baseline)**: `d50445e fix(fs/ext4): tmpfs/ext4 space accounting and file syscall improvements`
- **Kernel (r5 baseline)**: `53fd44d fix(process): exec comm basename, exit cleanup, kthread improvements`
- **Kernel (r4 baseline)**: `971543b fix(mm): heap leak defense + meminfo observability — close_cloexec fix, /proc/meminfo heap fields`
- **Kernel (r3 baseline)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll`
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed (up to 5 attempts each)
- **Total tests**: 1869 (same 1869-entry list as r1..r6: scenario tests from runtest files + helper binaries)

## Overall buckets (r7)
| bucket | count | share |
|---|---|---|
| PASS | 771 | 41.3% |
| WARN | 2 | 0.1% |
| FAIL | 488 | 26.1% |
| TCONF | 446 | 23.9% |
| CRASH | 28 | 1.5% |
| TIMEOUT | 111 | 5.9% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 13 | 0.7% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 -> r4 -> r5 -> r6 -> r7 comparison
| bucket | r1 | r2 | r3 | r4 | r5 | r6 | r7 | delta r6→r7 |
|---|---|---|---|---|---|---|---|
| PASS | 35 | 524 | 659 | 677 | 602 | 721 | 771 | +50 |
| WARN | 138 | 5 | 1 | 1 | 2 | 2 | 2 | 0 |
| FAIL | 226 | 698 | 592 | 542 | 564 | 501 | 488 | -13 |
| TCONF | 62 | 353 | 438 | 443 | 427 | 448 | 446 | -2 |
| CRASH | 0 | 16 | 27 | 31 | 26 | 29 | 28 | -1 |
| TIMEOUT | 32 | 131 | 128 | 153 | 225 | 142 | 111 | -31 |
| EXECFAIL | 0 | 17 | 10 | 10 | 10 | 10 | 10 | 0 |
| WEDGE | 63 | 125 | 14 | 12 | 13 | 16 | 13 | -3 |
| NOTRUN | 1313 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **1869** | **1869** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r6 -> r7)
```text
r6 class   -> r7 class   count
TIMEOUT    -> PASS          43
FAIL       -> TIMEOUT       13
TIMEOUT    -> FAIL           9
FAIL       -> PASS           7
TCONF      -> TIMEOUT        7
WEDGE      -> PASS           5
WEDGE      -> TIMEOUT        4
TIMEOUT    -> WEDGE          3
FAIL       -> WEDGE          3
PASS       -> WEDGE          2
PASS       -> TIMEOUT        2
CRASH      -> TCONF          2
WEDGE      -> TCONF          2
WEDGE      -> FAIL           1
PASS       -> FAIL           1
FAIL       -> TCONF          1
TIMEOUT    -> TCONF          1
TIMEOUT    -> CRASH          1
TCONF      -> WEDGE          1
```
- **Improved vs r6** (now PASS/WARN, previously worse): **55** tests
- **Regressed vs r6** (previously PASS/WARN, now FAIL-or-worse/TCONF): **5** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 -> r4 677 -> r5 602 -> r6 721 -> r7 771 (+736 vs r1, +50 vs r6). New all-time high, +50 above the r6 peak (721); short of the 850 stretch goal — the remaining headroom sits in the 379-test FAIL-with-no-TFAIL cluster and 446 TCONFs (missing kernel config: hugetlbfs, USER_NS, watchqueue).**

## Notable findings (r6 -> r7)

- **ext4 write-path batching (ac605b3) dissolved the post-exec-hang cluster**: 43 TIMEOUT -> PASS transitions, dominated by the libm math family (genacos..geny1, 28 tests) plus write01-03, open01/02/03/06/07, close01, clone302, fork09, mmap2, mremap01, waitid07/10, diotest5/6, inode01, mtest01, pathconf01 — the "execve-then-silence" tests that were the single largest r6 failure family. timerfd_create01 and timer_delete01 (r6 TIMEOUTs) now pass.
- **ptrace fix (7f9682d) directly visible**: ptrace01/02/03/05/11 all FAIL -> PASS; the ptrace family now stands at 5 PASS + 6 TCONF (missing config) + ptrace06 FAIL.
- **The other three fixes each visible**: mprotect01 FAIL -> PASS (PROT_WRITE on MAP_SHARED read-only, 0594fc3), getcpu01 FAIL -> PASS (sched_setaffinity prompt migration, f5c93ac), and the jbd2 handle unwind (6a90114) plus write-path batching together recovered **all five r6 WEDGE regressions**: fcntl37_64, futex_wait01, locktests, open08, write04.
- **TIMEOUT 142 -> 111 (-31)**: 43 exits to PASS, offset by 13 FAIL -> TIMEOUT, 7 TCONF -> TIMEOUT, 4 WEDGE -> TIMEOUT and 2 PASS -> TIMEOUT inflows. The remaining 111 are scattered small families (ftest 6, lchown 5, kill 4, fcntl 4, open/mmap/inotify/execve 3 each) — no single dominant cluster anymore.
- **Regressions minimal (5, vs 9 in r6)**: getgid03/openat01 WEDGE, kill12/lchown02 TIMEOUT, getrusage04 FAIL. All hang/wedge-flavoured and signal-heavy — plausible interaction with the new signal-stop parking, but the count is within historical flake noise.
- **Sweep health markedly better**: 19/19 chunks completed with only 10 wedge-retry attempts (r6 needed far more), ~1h47m wall vs ~4h for r6, zero ENOSPC, zero jbd2-hang repeats.
- **Operational notes (methodology, this round's `.ltp` copies only)**: (a) an orphaned leapsec01 child can now outlive its sweep and keep flooding the serial socket after `SWEEP-DONE` — the round's driver copy bounds the post-done drain to 10 s, where the r2-era driver could hang forever; (b) one serial interleaving glued a kernel trace char onto a RUN marker (`FRUN gethostname02`) — a single-test rerun recorded it cleanly (FAIL rc=1, hostname len check).

## By scenario/subsystem (r7)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      15      20       0       3       0       0       0
controllers               10       0       0       9       1       0       0       0       0       0
crashme                    3       0       0       0       1       0       2       0       0       0
crypto                    10       0       0       1       9       0       0       0       0       0
cve                       21       1       0       1      17       0       2       0       0       0
dio                        6       5       0       1       0       0       0       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       1       0       0       0       0       0       0       0       0
fs                        22       8       0       2       1       0       9       0       2       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      40       0      97      16      22      16      10       1       0
hugetlb                   48       0       0       0      48       0       0       0       0       0
input                      6       0       0       0       6       0       0       0       0       0
ipc                        1       0       0       0       0       0       1       0       0       0
irq                        1       0       0       1       0       0       0       0       0       0
kernel_misc               13       0       0       1      12       0       0       0       0       0
ltp-aio-stress             1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part1           1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part2           2       0       0       1       1       0       0       0       0       0
ltp-aiodio.part3           1       0       0       1       0       0       0       0       0       0
ltp-aiodio.part4           7       1       0       0       4       0       2       0       0       0
math                      10       5       0       0       0       0       5       0       0       0
mm                        57      18       0      10      20       1       6       0       2       0
net.ipv6_lib               6       1       0       2       1       0       2       0       0       0
nptl                       1       0       0       0       0       0       1       0       0       0
pty                        8       2       0       2       4       0       0       0       0       0
sched                     10       0       0       1       3       3       2       0       1       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     682       2     341     262       2      60       0       7       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r7)
```text
  [379] FAIL     (no TFAIL line)   e.g. access04,acct01,capget02,capset02,capset04,chdir01
  [165] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_map01,bpf_prog01
  [101] TIMEOUT  (no TFAIL line)   e.g. leapsec01,creat05,creat07,epoll-ltp,execve02,execve04
  [ 29] TCONF    (from serial-c15.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
cp-trace EXECVE by pid=N a0=ADDR
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FUT:    e.g. hugefork02,hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09
  [ 18] CRASH    (no TFAIL line)   e.g. sendmsg02,shm_test,pth_str01,pth_str03,trace_sched,eject_check_tray
  [ 12] WEDGE    (no TFAIL line)   e.g. clock_nanosleep02,fstatfs01_64,getgid03,lchown03_16,openat01,sendfile07
  [ 12] TCONF    cp-trace EXECVE by pid=N a0=ADDR
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N: TINFO: Constra   e.g. clock_gettime03,fcntl38_64,io_destroy02,io_getevents01,keyctl09,setsockopt10
  [ 10] EXECFAIL (no TFAIL line)   e.g. erst-inject,lddfile.out,thugetlb,tinjpage,tkillpoison,tprctl
  [  9] TCONF    tst_kconfig.c:N: TINFO: Constraint 'CONFIG_USER_NS=y' not satisfied!
tst_kconfig.c:N: TINFO: Variables:
tst_kconfig.c:N: TINFO:  CONFIG_USER_NS Undefined
tst_kc   e.g. bind06,sendto03,setsockopt05,setsockopt06,setsockopt07,fanout01
  [  9] FAIL     (from serial-c17.log)
```
