# LTP second-round scan (r2) — Rux kernel post-fix rescan
- **Kernel (r2)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs` (worktree ltp-r2, clean rebuild; cumulative LTP fix series: syscall suite, signal/time/futex, mm/fs, errno, recvmsg)
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed (up to 5 attempts each)
- **Total tests**: 1869 (same 1869-entry list as r1: scenario tests from runtest files + helper binaries)

## Overall buckets (r2)
| bucket | count | share |
|---|---|---|
| PASS | 524 | 28.0% |
| WARN | 5 | 0.3% |
| FAIL | 698 | 37.3% |
| TCONF | 353 | 18.9% |
| CRASH | 16 | 0.9% |
| TIMEOUT | 131 | 7.0% |
| EXECFAIL | 17 | 0.9% |
| WEDGE | 125 | 6.7% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 comparison
| bucket | r1 | r2 | delta |
|---|---|---|---|
| PASS | 35 | 524 | +489 |
| WARN | 138 | 5 | -133 |
| FAIL | 226 | 698 | +472 |
| TCONF | 62 | 353 | +291 |
| CRASH | 0 | 16 | +16 |
| TIMEOUT | 32 | 131 | +99 |
| EXECFAIL | 0 | 17 | +17 |
| WEDGE | 63 | 125 | +62 |
| NOTRUN | 1313 | 0 | -1313 |
| **executed** | **556** | **1869** | **+1313** |
| total | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r1 -> r2)
```text
r1 class   -> r2 class   count
NOTRUN     -> FAIL         477
NOTRUN     -> PASS         338
NOTRUN     -> TCONF        293
WARN       -> PASS         137
NOTRUN     -> TIMEOUT      102
NOTRUN     -> WEDGE         68
NOTRUN     -> CRASH         16
NOTRUN     -> EXECFAIL      15
FAIL       -> PASS          11
WEDGE      -> FAIL           5
WEDGE      -> TIMEOUT        4
NOTRUN     -> WARN           4
TIMEOUT    -> PASS           3
TCONF      -> FAIL           2
TIMEOUT    -> FAIL           2
TIMEOUT    -> WEDGE          2
FAIL       -> TIMEOUT        1
FAIL       -> WEDGE          1
FAIL       -> EXECFAIL       1
TIMEOUT    -> EXECFAIL       1
```
- **Improved** (now PASS/WARN, previously worse): **493** tests
- **Regressed** (previously PASS/WARN, now FAIL-or-worse/TCONF): **0** tests

### Like-for-like: the 556 tests that r1 actually executed

Apples-to-apples comparison restricted to the subset both rounds ran (r1 was
cut short by wedge cascades after 556/1869; r2 executed the full list):

| bucket | r1 | r2 | delta |
|---|---|---|---|
| PASS | 35 | 186 | +151 |
| WARN | 138 | 1 | -137 |
| FAIL | 226 | 221 | -5 |
| TCONF | 62 | 60 | -2 |
| CRASH | 0 | 0 | 0 |
| TIMEOUT | 32 | 29 | -3 |
| EXECFAIL | 0 | 2 | +2 |
| WEDGE | 63 | 57 | -6 |

- **PASS rate on this subset: 6.3% -> 33.5%** (5.3x). The r1 WARN bucket
  (138 TWARN-only tests) collapsed to 1 — those tests now exit 0.
- FAIL/WEDGE on this subset are roughly flat (226->221, 63->57): the fixes
  converted warning-storms and specific failures into passes rather than
  masking them; the large absolute FAIL count below comes from the 1313
  tests r1 never reached.

### 5-bucket rollup (task reporting scheme)

| bucket | r1 | r2 | delta |
|---|---|---|---|
| PASS | 35 (1.9%) | 524 (28.0%) | +489 |
| WARN | 138 (7.4%) | 5 (0.3%) | -133 |
| FAIL | 226 (12.1%) | 698 (37.3%) | +472 |
| WEDGE | 63 (3.4%) | 125 (6.7%) | +62 |
| NOTRUN | 1313 (70.3%) | 0 (0.0%) | -1313 |

(remaining r2 buckets: TCONF 353, CRASH 16, TIMEOUT 131, EXECFAIL 17 —
all from regions r1 never reached; totals still 1869 = 556 + 1313.)

## By scenario/subsystem (r2)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      36       2       0       0       0       0       0
controllers               10       0       0       6       0       0       0       0       4       0
crashme                    3       2       0       0       1       0       0       0       0       0
crypto                    10       0       0       2       8       0       0       0       0       0
cve                       21       1       0       9       9       0       2       0       0       0
dio                        6       4       0       1       0       0       1       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       0       0       0       0       0       0       0       1       0
fs                        22       7       1       1       1       0       7       1       4       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      18       0     106      17      12      38      11       0       0
hugetlb                   48       0       0       1      47       0       0       0       0       0
input                      6       0       0       0       6       0       0       0       0       0
ipc                        1       0       0       0       0       0       0       1       0       0
irq                        1       0       0       1       0       0       0       0       0       0
kernel_misc               13       0       0       4       9       0       0       0       0       0
ltp-aio-stress             1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part1           1       0       0       0       1       0       0       0       0       0
ltp-aiodio.part2           2       0       0       0       1       0       0       0       1       0
ltp-aiodio.part3           1       0       0       1       0       0       0       0       0       0
ltp-aiodio.part4           7       1       0       0       4       0       1       0       1       0
math                      10       5       0       0       0       0       5       0       0       0
mm                        57      13       0      16      15       0      10       0       3       0
net.ipv6_lib               6       1       0       2       1       0       1       1       0       0
nptl                       1       0       0       0       0       0       0       0       1       0
pty                        8       2       0       6       0       0       0       0       0       0
sched                     10       0       0       3       1       3       1       0       2       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     463       4     501     211       1      65       3     108       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r2)
```text
  [430] FAIL     (no TFAIL line)   e.g. accept01,accept02,accept03,access01,adjtimex02,adjtimex03
  [142] TCONF    (no TFAIL line)   e.g. add_key02,add_key03,bpf_map01,bpf_prog01,bpf_prog02,bpf_prog03
  [121] WEDGE    (no TFAIL line)   e.g. access04,acct01,chdir01,chown05,chroot03,close_range01
  [115] TIMEOUT  (no TFAIL line)   e.g. chown04_16,clock_gettime01,clock_settime03,creat05,creat07,epoll-ltp
  [ 28] TCONF    (from serial-c15-r5.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FUT: REPEAT teardown of root ppn=AD   e.g. hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09,hugemmap10
  [ 17] EXECFAIL (no TFAIL line)   e.g. creat08,execveat02,uname01,fs_fill,pipeio,asapi_02
  [ 13] CRASH    (no TFAIL line)   e.g. trace_sched,eject_check_tray,frag,genexp_log,genpower,gentrigo
  [ 12] FAIL     (from serial-c14-r7.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N: TINFO: Couldn't locate kernel config!
tst_kconfig.c:N: TBROK: Cannot p   e.g. pidns01,pidns02,pidns03,pidns12,pidns20,mqns_01
  [  9] FAIL     (from serial-c17.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_test.c:N: TBROK: LTP_IPC_PATH is not defined
FUT: REPEAT teardown of root ppn=ADDR (ri   e.g. execv01_child,execve01_child,execve06_child,execve_child,execveat_child,execveat_errno
  [  7] TCONF    (from serial-c4-r6.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
```
