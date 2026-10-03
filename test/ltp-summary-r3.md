# LTP third-round scan (r3) — Rux kernel post-fix rescan
- **Kernel (r3)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll` (worktree ltp-r5, clean rebuild of main; cumulative fix series: r2 syscall/signal/mm/fs fixes, fs batch (dentry cache, ext4 namei, VFS path, file syscalls), proc/signal/mm batch (fork namespace, signal, mmap/mprotect, poll))
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed
- **Total tests**: 1869 (same 1869-entry list as r1/r2: scenario tests from runtest files + helper binaries)

## Overall buckets (r3)
| bucket | count | share |
|---|---|---|
| PASS | 659 | 35.3% |
| WARN | 1 | 0.1% |
| FAIL | 592 | 31.7% |
| TCONF | 438 | 23.4% |
| CRASH | 27 | 1.4% |
| TIMEOUT | 128 | 6.8% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 14 | 0.7% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 comparison
| bucket | r1 | r2 | r3 | delta r2→r3 |
|---|---|---|---|---|
| PASS | 35 | 524 | 659 | +135 |
| WARN | 138 | 5 | 1 | -4 |
| FAIL | 226 | 698 | 592 | -106 |
| TCONF | 62 | 353 | 438 | +85 |
| CRASH | 0 | 16 | 27 | +11 |
| TIMEOUT | 32 | 131 | 128 | -3 |
| EXECFAIL | 0 | 17 | 10 | -7 |
| WEDGE | 63 | 125 | 14 | -111 |
| NOTRUN | 1313 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r2 -> r3)
```text
r2 class   -> r3 class   count
FAIL       -> PASS         121
WEDGE      -> FAIL         102
FAIL       -> TCONF         85
TIMEOUT    -> PASS          19
WEDGE      -> TIMEOUT       11
PASS       -> FAIL           8
FAIL       -> TIMEOUT        8
FAIL       -> CRASH          8
WEDGE      -> PASS           6
PASS       -> TIMEOUT        6
TIMEOUT    -> FAIL           5
TIMEOUT    -> WEDGE          4
FAIL       -> WEDGE          3
EXECFAIL   -> TIMEOUT        3
WARN       -> PASS           3
TIMEOUT    -> TCONF          2
EXECFAIL   -> FAIL           2
TCONF      -> PASS           2
PASS       -> WEDGE          1
WARN       -> FAIL           1
TCONF      -> FAIL           1
TIMEOUT    -> CRASH          1
EXECFAIL   -> TCONF          1
EXECFAIL   -> CRASH          1
PASS       -> CRASH          1
```
- **Improved vs r2** (now PASS/WARN, previously worse): **151** tests
- **Regressed vs r2** (previously PASS/WARN, now FAIL-or-worse/TCONF): **17** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 (+624 vs r1, +135 vs r2).**

## By scenario/subsystem (r3)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      15      23       0       0       0       0       0
controllers               10       0       0      10       0       0       0       0       0       0
crashme                    3       0       0       0       1       0       2       0       0       0
crypto                    10       0       0       2       8       0       0       0       0       0
cve                       21       1       0       2      16       0       2       0       0       0
dio                        6       4       0       1       0       0       1       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       0       0       0       0       0       1       0       0       0
fs                        22       8       0       2       1       0      11       0       0       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      17       0      98      17      22      38      10       0       0
hugetlb                   48       0       0       1      47       0       0       0       0       0
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
mm                        57      15       0      12      19       1       7       0       3       0
net.ipv6_lib               6       1       0       2       2       0       1       0       0       0
nptl                       1       0       0       0       0       0       1       0       0       0
pty                        8       2       0       4       2       0       0       0       0       0
sched                     10       0       0       1       3       3       1       0       2       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     598       1     434     258       1      55       0       9       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r3)
```text
  [451] FAIL     (no TFAIL line)   e.g. accept02,access01,access04,acct01,adjtimex02,adjtimex03
  [158] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_prog01,bpf_prog02
  [115] TIMEOUT  (no TFAIL line)   e.g. clock_settime03,creat07,epoll-ltp,execve02,execve04,execve05
  [ 29] TCONF    (from serial-c15.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FUT: REPEAT teardown of root ppn=ADDR    e.g. hugefork02,hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09
  [ 17] FAIL     tst_device.c:N: TBROK: Failed to acquire device
Summary:
passed   N
failed   N
broken   N
skipped  N
warnings N   e.g. fdatasync03,fremovexattr01,fremovexattr02,fsconfig01,fsconfig02,fanotify01
  [ 15] CRASH    (no TFAIL line)   e.g. shm_test,trace_sched,eject_check_tray,frag,genexp_log,genpower
  [ 13] WEDGE    (no TFAIL line)   e.g. bind05,creat05,fpathconf01,kill10,open06,sendmsg02
  [ 13] TCONF    tst_kconfig.c:N: TINFO: Parsing kernel config '/proc/config.gz'
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
   e.g. clock_gettime03,fcntl39_64,io_destroy02,io_getevents01,io_setup02,keyctl09
```
