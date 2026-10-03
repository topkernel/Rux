# LTP fourth-round scan (r4) — Rux kernel post-fix rescan
- **Kernel (r4)**: `971543b fix(mm): heap leak defense + meminfo observability — close_cloexec fix, /proc/meminfo heap fields` (worktree ltp-r6, clean rebuild of main; cumulative fix series: r2 syscall/signal/mm/fs fixes, fs batch (dentry cache, ext4 namei, VFS path, file syscalls), proc/signal/mm batch (fork namespace, signal, mmap/mprotect, poll), virtio-gpu framebuffer COW fix, net/timer/sched batch (17 files), heap leak defense + meminfo observability)
- **Kernel (r3 baseline)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll`
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed
- **Total tests**: 1869 (same 1869-entry list as r1/r2/r3: scenario tests from runtest files + helper binaries)

## Overall buckets (r4)
| bucket | count | share |
|---|---|---|
| PASS | 677 | 36.2% |
| WARN | 1 | 0.1% |
| FAIL | 542 | 29.0% |
| TCONF | 443 | 23.7% |
| CRASH | 31 | 1.7% |
| TIMEOUT | 153 | 8.2% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 12 | 0.6% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 -> r4 comparison
| bucket | r1 | r2 | r3 | r4 | delta r3→r4 |
|---|---|---|---|---|---|
| PASS | 35 | 524 | 659 | 677 | +18 |
| WARN | 138 | 5 | 1 | 1 | 0 |
| FAIL | 226 | 698 | 592 | 542 | -50 |
| TCONF | 62 | 353 | 438 | 443 | +5 |
| CRASH | 0 | 16 | 27 | 31 | +4 |
| TIMEOUT | 32 | 131 | 128 | 153 | +25 |
| EXECFAIL | 0 | 17 | 10 | 10 | 0 |
| WEDGE | 63 | 125 | 14 | 12 | -2 |
| NOTRUN | 1313 | 0 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r3 -> r4)
```text
r3 class   -> r4 class   count
FAIL       -> PASS          30
FAIL       -> TIMEOUT       19
PASS       -> TIMEOUT       12
FAIL       -> TCONF         10
TIMEOUT    -> PASS           9
PASS       -> FAIL           6
TCONF      -> TIMEOUT        4
TIMEOUT    -> FAIL           4
PASS       -> WEDGE          3
WEDGE      -> TIMEOUT        3
WEDGE      -> PASS           2
PASS       -> CRASH          2
TCONF      -> CRASH          2
FAIL       -> WEDGE          1
FAIL       -> CRASH          1
WEDGE      -> FAIL           1
TIMEOUT    -> WEDGE          1
CRASH      -> TIMEOUT        1
WEDGE      -> TCONF          1
```
- **Improved vs r3** (now PASS/WARN, previously worse): **41** tests
- **Regressed vs r3** (previously PASS/WARN, now FAIL-or-worse/TCONF): **23** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 -> r4 677 (+642 vs r1, +18 vs r3).**

## By scenario/subsystem (r4)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      15      23       0       0       0       0       0
controllers               10       0       0      10       0       0       0       0       0       0
crashme                    3       0       0       0       1       0       2       0       0       0
crypto                    10       0       0       1       9       0       0       0       0       0
cve                       21       1       0       1      17       0       2       0       0       0
dio                        6       4       0       1       0       0       1       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       0       0       0       0       0       1       0       0       0
fs                        22       8       0       1       1       0      12       0       0       0
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
mm                        57      14       0      12      20       0       8       0       3       0
net.ipv6_lib               6       1       0       2       2       0       1       0       0       0
nptl                       1       0       0       0       0       0       1       0       0       0
pty                        8       2       0       2       4       0       0       0       0       0
sched                     10       0       0       1       3       3       2       0       1       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     617       1     390     257       6      77       0       8       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r4)
```text
  [415] FAIL     (no TFAIL line)   e.g. access01,access04,acct01,bind04,capget02,capset02
  [160] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_map01,bpf_prog01
  [136] TIMEOUT  (no TFAIL line)   e.g. accept02,clock_nanosleep02,clock_nanosleep04,leapsec01,clone302,close01
  [ 26] TCONF    (from serial-c15.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FUT: REPEAT teardown of root ppn=ADDR    e.g. hugemmap07,hugemmap08,hugemmap09,hugemmap10,hugemmap11,hugemmap12
  [ 19] CRASH    (no TFAIL line)   e.g. sendfile08,sendfile08_64,sendfile09,sendfile09_64,sendmsg01,trace_sched
  [ 14] FAIL     tst_device.c:N: TBROK: Failed to acquire device
Summary:
passed   N
failed   N
broken   N
skipped  N
warnings N   e.g. fgetxattr01,fanotify01,fanotify05,fanotify21,preadv03,preadv03_64
  [ 14] TCONF    FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N:   e.g. ftruncate04,io_destroy02,msgget05,shmget06,ksm03,ksm07
  [ 10] TCONF    FUT: REPEAT teardown of root ppn=ADDR (ring[N])
```
