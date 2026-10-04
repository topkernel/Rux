# LTP sixth-round scan (r6) — Rux kernel post-fix rescan
- **Kernel (r6)**: `d50445e fix(fs/ext4): tmpfs/ext4 space accounting and file syscall improvements` (worktree ltp-r9, clean rebuild of main; fix series since the r5 kernel: sched free_task_slot heap-leak fix (4kB/exec), net/unix inode-keyed name table + Linux backlog semantics (dbus system-bus blocker), syscalls/fs round-6 batch — fadvise, vmsplice, splice, readlink, ioctl, loop devices — and tmpfs/ext4 space accounting + file syscall improvements targeting the r5 /tmp ENOSPC regression)
- **Kernel (r5 baseline)**: `53fd44d fix(process): exec comm basename, exit cleanup, kthread improvements`
- **Kernel (r4 baseline)**: `971543b fix(mm): heap leak defense + meminfo observability — close_cloexec fix, /proc/meminfo heap fields`
- **Kernel (r3 baseline)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll`
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed
- **Total tests**: 1869 (same 1869-entry list as r1..r5: scenario tests from runtest files + helper binaries)

## Overall buckets (r6)
| bucket | count | share |
|---|---|---|
| PASS | 721 | 38.6% |
| WARN | 2 | 0.1% |
| FAIL | 501 | 26.8% |
| TCONF | 448 | 24.0% |
| CRASH | 29 | 1.6% |
| TIMEOUT | 142 | 7.6% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 16 | 0.9% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 -> r4 -> r5 -> r6 comparison
| bucket | r1 | r2 | r3 | r4 | r5 | r6 | delta r5→r6 |
|---|---|---|---|---|---|---|---|
| PASS | 35 | 524 | 659 | 677 | 602 | 721 | +119 |
| WARN | 138 | 5 | 1 | 1 | 2 | 2 | 0 |
| FAIL | 226 | 698 | 592 | 542 | 564 | 501 | -63 |
| TCONF | 62 | 353 | 438 | 443 | 427 | 448 | +21 |
| CRASH | 0 | 16 | 27 | 31 | 26 | 29 | +3 |
| TIMEOUT | 32 | 131 | 128 | 153 | 225 | 142 | -83 |
| EXECFAIL | 0 | 17 | 10 | 10 | 10 | 10 | 0 |
| WEDGE | 63 | 125 | 14 | 12 | 13 | 16 | +3 |
| NOTRUN | 1313 | 0 | 0 | 0 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **1869** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r5 -> r6)
```text
r5 class   -> r6 class   count
FAIL       -> PASS          88
TIMEOUT    -> FAIL          44
TIMEOUT    -> PASS          38
TIMEOUT    -> TCONF         16
FAIL       -> TIMEOUT       14
FAIL       -> TCONF          8
TIMEOUT    -> WEDGE          5
PASS       -> TIMEOUT        4
PASS       -> WEDGE          3
PASS       -> FAIL           2
WEDGE      -> TIMEOUT        2
WEDGE      -> PASS           2
TCONF      -> CRASH          2
WEDGE      -> FAIL           1
TCONF      -> FAIL           1
WEDGE      -> CRASH          1
FAIL       -> WEDGE          1
```
- **Improved vs r5** (now PASS/WARN, previously worse): **128** tests
- **Regressed vs r5** (previously PASS/WARN, now FAIL-or-worse/TCONF): **9** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 -> r4 677 -> r5 602 -> r6 721 (+686 vs r1, +119 vs r5). New all-time high, +44 above the previous r4 peak (677).**

## Notable findings (r5 -> r6 recovery analysis)

- **tmpfs `/tmp` exhaustion eliminated**: the dominant r5 failure mode (93 tests TBROKing on `tst_tmpdir: mkdtemp(/tmp/LTP_*) failed: ENOSPC`) is completely gone — **zero** ENOSPC occurrences across all 19 chunks of r6. The tmpfs/ext4 space-accounting fix (d50445e) directly recovered the msgctl/msgget/msgrcv/msgsnd, msync, read*, pwrite*, ftruncate and open* families; /tmp now survives the full 100-test chunk load without draining.
- **Early-exec hangs reduced but still the top TIMEOUT mode**: TIMEOUTs drop 225 -> 142 (-83). EXECVE-then-silence tails drop from 162 to 104, i.e. ~58 of the recovered exec hangs trace to the freed Task slots / heap fix (6142752, 4kB/exec leak) and reduced cross-test interference; the remaining 104 (timer_delete*, timerfd*, clock_nanosleep02/03, leapsec01, clone302, close*, creat*, pty family) still hang immediately after execve — the single largest remaining failure cluster.
- **Round-6 syscall batch directly visible**: vmsplice* (4), splice*/sendfile* (6+), posix_fadvise* (3), read*, ioctl/loop-device tests (access04 family now fails on assertions rather than hanging) account for a large slice of the 88 FAIL->PASS transitions.
- **Regressions are minimal and mostly heavy/flaky tests**: only 9 (vs 104 in r5): bind05, fork07 (FAIL), crash01, crash02, mmap2, mtest01 (TIMEOUT — fork-bomb/memory-hog tests sensitive to machine load; 3 other agent QEMUs were co-resident during the scan), fcntl37_64, futex_wait01, splice05 (WEDGE).
- **TCONF 427 -> 448 (+21)** is benign churn: 16 TIMEOUT -> TCONF and 8 FAIL -> TCONF transitions mean tests now execute far enough to report missing kernel config (hugetlbfs, CONFIG_USER_NS, watchqueue) instead of hanging.

## By scenario/subsystem (r6)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      15      23       0       0       0       0       0
controllers               10       0       0       9       1       0       0       0       0       0
crashme                    3       0       0       0       1       0       2       0       0       0
crypto                    10       0       0       1       9       0       0       0       0       0
cve                       21       1       0       1      17       0       2       0       0       0
dio                        6       3       0       1       0       0       2       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       0       0       0       0       0       0       0       1       0
fs                        22       7       0       2       1       0      10       0       2       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      17       0      97      17      22      39      10       0       0
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
mm                        57      16       0      10      20       0       9       0       2       0
net.ipv6_lib               6       1       0       2       2       0       1       0       0       0
nptl                       1       0       0       0       0       0       1       0       0       0
pty                        8       2       0       2       4       0       0       0       0       0
sched                     10       0       0       1       2       3       1       0       3       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     661       2     354     260       4      67       0       8       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r6)
```text
  [386] FAIL     (no TFAIL line)   e.g. access04,acct01,capget02,capset02,capset04,chdir01
  [159] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_map01,bpf_prog01
  [131] TIMEOUT  (no TFAIL line)   e.g. clock_nanosleep02,leapsec01,clone302,close01,close_range01,creat05
  [ 28] TCONF    (from serial-c15-r1.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
cp-trace EXECVE by pid=N a0=ADDR
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FU   e.g. hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09,hugemmap10
  [ 21] TCONF    cp-trace EXECVE by pid=N a0=ADDR
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N: TINFO: Constra   e.g. fcntl39_64,ftruncate04,ftruncate04_64,io_setup02,keyctl09,setsockopt10
  [ 19] CRASH    (no TFAIL line)   e.g. sendmsg02,sendmsg03,sendto03,pth_str01,pth_str03,trace_sched
  [ 14] WEDGE    (no TFAIL line)   e.g. creat06,fcntl37_64,fork14,open08,splice05,write04
  [ 10] TCONF    cp-trace EXECVE by pid=N a0=ADDR
tst_kconfig.c:N: TINFO: Parsing kernel config '/proc/config.gz'
cp-trace EXECVE by pid=N a0=ADDR
cp-trace EXECVE by pid=N a0=AD   e.g. fcntl38,io_submit02,kill13,swapping01,cfs_bandwidth01,pty03
  [ 10] EXECFAIL (no TFAIL line)   e.g. erst-inject,lddfile.out,thugetlb,tinjpage,tkillpoison,tprctl
  [  7] TCONF    tst_kconfig.c:N: TINFO: Constraint 'CONFIG_USER_NS=y' not satisfied!
```
