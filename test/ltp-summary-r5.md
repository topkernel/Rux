# LTP fifth-round scan (r5) — Rux kernel post-fix rescan
- **Kernel (r5)**: `53fd44d fix(process): exec comm basename, exit cleanup, kthread improvements` (worktree ltp-r7, clean rebuild of main; cumulative fix series: r2 syscall/signal/mm/fs fixes, fs batch (dentry cache, ext4 namei, VFS path, file syscalls), proc/signal/mm batch (fork namespace, signal, mmap/mprotect, poll), virtio-gpu framebuffer COW fix, net/timer/sched batch (17 files), heap leak defense + meminfo observability, pipe/socket/mmap + regression/ioctl/mount batch (28 files: page-fault COW, ext4 extents, loop device, devfs/dev_t, mlock), exec comm basename + exit cleanup + kthread improvements)
- **Kernel (r4 baseline)**: `971543b fix(mm): heap leak defense + meminfo observability — close_cloexec fix, /proc/meminfo heap fields`
- **Kernel (r3 baseline)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll`
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed
- **Total tests**: 1869 (same 1869-entry list as r1/r2/r3/r4: scenario tests from runtest files + helper binaries)

## Overall buckets (r5)
| bucket | count | share |
|---|---|---|
| PASS | 602 | 32.2% |
| WARN | 2 | 0.1% |
| FAIL | 564 | 30.2% |
| TCONF | 427 | 22.8% |
| CRASH | 26 | 1.4% |
| TIMEOUT | 225 | 12.0% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 13 | 0.7% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 -> r4 -> r5 comparison
| bucket | r1 | r2 | r3 | r4 | r5 | delta r4→r5 |
|---|---|---|---|---|---|---|
| PASS | 35 | 524 | 659 | 677 | 602 | -75 |
| WARN | 138 | 5 | 1 | 1 | 2 | +1 |
| FAIL | 226 | 698 | 592 | 542 | 564 | +22 |
| TCONF | 62 | 353 | 438 | 443 | 427 | -16 |
| CRASH | 0 | 16 | 27 | 31 | 26 | -5 |
| TIMEOUT | 32 | 131 | 128 | 153 | 225 | +72 |
| EXECFAIL | 0 | 17 | 10 | 10 | 10 | 0 |
| WEDGE | 63 | 125 | 14 | 12 | 13 | +1 |
| NOTRUN | 1313 | 0 | 0 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r4 -> r5)
```text
r4 class   -> r5 class   count
PASS       -> FAIL          59
FAIL       -> TIMEOUT       42
PASS       -> TIMEOUT       41
FAIL       -> PASS          17
TCONF      -> TIMEOUT       16
TIMEOUT    -> FAIL          14
TIMEOUT    -> PASS           9
TIMEOUT    -> WEDGE          6
TCONF      -> FAIL           5
PASS       -> WEDGE          4
WEDGE      -> TIMEOUT        4
WEDGE      -> PASS           3
WEDGE      -> FAIL           3
CRASH      -> FAIL           3
TIMEOUT    -> TCONF          2
CRASH      -> TCONF          2
FAIL       -> WEDGE          1
FAIL       -> TCONF          1
FAIL       -> WARN           1
```
- **Improved vs r4** (now PASS/WARN, previously worse): **30** tests
- **Regressed vs r4** (previously PASS/WARN, now FAIL-or-worse/TCONF): **104** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 -> r4 677 -> r5 602 (+567 vs r1, -75 vs r4). First net PASS decline since the r1 baseline.**

## Notable findings (r4 -> r5 regression analysis)

- **tmpfs `/tmp` exhaustion (dominant new failure mode)**: 93 tests hit `tst_tmpdir: mkdtemp(/tmp/LTP_*) failed: ENOSPC` (45 of them were PASS/WARN in r4: the msgctl/msgget/msgrcv/msgsnd, msync, read*, pwrite*, fstat/ftruncate, open* families). Once /tmp fills mid-chunk it never drains and every subsequent tmpdir-using test TBROKs (e.g. chunk 8: all tmpdir tests from pselect03 onward). Standalone re-verify of read03 on a fresh boot PASSES, so this is cumulative guest state, not a per-test breakage — pointing at tmpfs page accounting / reclaim / exit-time file cleanup. Candidate changes: the mlock/mmap (`syscall/memory.rs`, +341 lines) and exit-cleanup (`process/exit.rs`) parts of 6294c69/53fd44d.
- **Early-exec hangs (TIMEOUT inflation)**: 162 of the 225 TIMEOUTs hang immediately after execve (tail = `cp-trace EXECVE` then 30s silence; +72 TIMEOUTs vs r4, 41 of them ex-PASS: timer_delete*/timer_getoverrun*/timer_settime*/timerfd*, madvise*, open/openat*, close02, pause01, pty family). In several cases a leaked process from an earlier timed-out test (e.g. leapsec01's adjtimex loop — 336 spam lines in chunk 0's log alone) is still running and printing while the current test hangs; correlation observed, causation not established.
- **Genuine wins from the round-5 fix batch remain**: 30 tests improved, incl. clock_nanosleep04, clone02, crash01/crash02, fallocate01, futex_wake01, lseek07, mkdir02/04/05, mkdirat01, mknod02/08, mknodat01, mlock05, mmap10, mtest01, munlock02, sched_setparam03, socket01, unlink08, unlinkat01, unshare02, vhangup02, waitpid06/07/08.

## By scenario/subsystem (r5)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      14      23       0       1       0       0       0
controllers               10       0       0      10       0       0       0       0       0       0
crashme                    3       2       0       0       1       0       0       0       0       0
crypto                    10       0       0       1       9       0       0       0       0       0
cve                       21       1       0       1      17       0       2       0       0       0
dio                        6       1       0       1       0       0       4       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       0       0       0       0       0       0       0       1       0
fs                        22       6       0       1       1       0      12       0       2       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      17       0      98      17      22      38      10       0       0
hugetlb                   48       0       0       0      48       0       0       0       0       0
input                      6       0       0       0       6       0       0       0       0       0
ipc                        1       0       0       0       0       0       1       0       0       0
irq                        1       0       0       1       0       0       0       0       0       0
kernel_misc               13       0       0       3      10       0       0       0       0       0
ltp-aio-stress             1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part1           1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part2           2       0       0       1       1       0       0       0       0       0
ltp-aiodio.part3           1       0       0       1       0       0       0       0       0       0
ltp-aiodio.part4           7       1       0       0       4       0       2       0       0       0
math                      10       5       0       0       0       0       5       0       0       0
mm                        57      12       0      20      20       0       5       0       0       0
net.ipv6_lib               6       1       0       2       2       0       1       0       0       0
nptl                       1       0       0       0       0       0       0       0       1       0
pty                        8       0       0       0       0       0       8       0       0       0
sched                     10       0       0       1       1       3       3       0       2       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     549       2     407     247       1     143       0       7       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r5)
```text
  [427] FAIL     (no TFAIL line)   e.g. access01,access04,acct01,capget02,capset02,capset04
  [205] TIMEOUT  (no TFAIL line)   e.g. accept02,chdir01,clock_nanosleep02,clock_nanosleep03,leapsec01,clone302
  [152] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_map01,bpf_prog01
  [ 29] TCONF    (from serial-c15-r1.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
cp-trace EXECVE by pid=N a0=ADDR
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FU   e.g. hugefallocate02,hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09
  [ 22] TCONF    cp-trace EXECVE by pid=N a0=ADDR
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N: TINFO: Constra   e.g. fcntl38_64,fcntl39,fcntl39_64,ftruncate04_64,setsockopt10,shmget06
  [ 16] CRASH    (no TFAIL line)   e.g. pth_str01,pth_str03,trace_sched,eject_check_tray,frag,genexp_log
  [ 12] WEDGE    (no TFAIL line)   e.g. fcntl36_64,getdents02,getegid01_16,sendmsg02,splice08,write04
  [ 11] FAIL     tst_device.c:N: TBROK: Failed to acquire device
Summary:
passed   N
failed   N
broken   N
skipped  N
```
