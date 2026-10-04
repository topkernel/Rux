# LTP eighth-round scan (r8) — Rux kernel post-fix rescan
- **Kernel (r8)**: `64e0f2e test(unixstress): re-resolve conns by fd in the poll sweep` (worktree ltp-r12, clean rebuild of main; fix series since the r7 kernel: ext4 read performance — batched async page-cache fills for sequential reads, coalesced multi-block reads and batched page-cache inserts, read(2)/write(2) fast paths for pipes and ext4, saturated group free-block accounting, plus a 10MB kernel boot-stack mapping budget; epoll ET wait edge-consumption fix; net/unix — connect notifies the listener's epoll watchers, recv notifies the peer when draining frees send room, zero-length STREAM sendmsg with ancillary data no longer fakes EOF, a closed peer wakes blocked writers with EPIPE)
- **Kernel (r7 baseline)**: `f35afe5 Revert "fix(net/unix+epoll): three EPOLLET edge-loss holes starved healthy clients forever"`
- **Kernel (r6 baseline)**: `d50445e fix(fs/ext4): tmpfs/ext4 space accounting and file syscall improvements`
- **Kernel (r5 baseline)**: `53fd44d fix(process): exec comm basename, exit cleanup, kthread improvements`
- **Kernel (r4 baseline)**: `971543b fix(mm): heap leak defense + meminfo observability — close_cloexec fix, /proc/meminfo heap fields`
- **Kernel (r3 baseline)**: `edd782c fix(proc/signal/mm): LTP process fixes — fork namespace, signal, mmap/mprotect, poll`
- **Kernel (r2 baseline)**: `155b71d fix(syscall): LTP syscall-suite fixes — memory, time, process, signal, fs`
- **Kernel (r1 baseline)**: `b13cdd9 fix(mm): map_kernel_region superpage path called deleted map_kernel_pmd`
- **LTP**: 20240524, riscv64 musl static binaries, from `userspace/linux-ltp`
- **Environment**: QEMU virt, TCG single-thread, rv64, 2G RAM, 4 vCPUs; rootfs ext4 (4K blocks, no metadata_csum); PID-1 sweep runner, 30s wall-clock timeout per test, wedged chunks auto-resumed (up to 5 attempts each)
- **Total tests**: 1869 (same 1869-entry list as r1..r7: scenario tests from runtest files + helper binaries)

## Overall buckets (r8)
| bucket | count | share |
|---|---|---|
| PASS | 673 | 36.0% |
| WARN | 2 | 0.1% |
| FAIL | 589 | 31.5% |
| TCONF | 434 | 23.2% |
| CRASH | 36 | 1.9% |
| TIMEOUT | 110 | 5.9% |
| EXECFAIL | 10 | 0.5% |
| WEDGE | 15 | 0.8% |
| NOTRUN | 0 | 0.0% |

**Executed**: 1869/1869 (100.0%). "PASS" = exit 0; "WARN" = rc 4 (TWARN only); "FAIL" = rc&1 or rc&2 (TFAIL/TBROK); "TCONF" = unsupported config; "CRASH" = test killed by signal; "TIMEOUT" = killed at 30s; "WEDGE" = kernel died mid-test; "NOTRUN" = never reached.

## r1 -> r2 -> r3 -> r4 -> r5 -> r6 -> r7 -> r8 comparison
| bucket | r1 | r2 | r3 | r4 | r5 | r6 | r7 | r8 | delta r7→r8 |
|---|---|---|---|---|---|---|---|---|
| PASS | 35 | 524 | 659 | 677 | 602 | 721 | 771 | 673 | -98 |
| WARN | 138 | 5 | 1 | 1 | 2 | 2 | 2 | 2 | 0 |
| FAIL | 226 | 698 | 592 | 542 | 564 | 501 | 488 | 589 | +101 |
| TCONF | 62 | 353 | 438 | 443 | 427 | 448 | 446 | 434 | -12 |
| CRASH | 0 | 16 | 27 | 31 | 26 | 29 | 28 | 36 | +8 |
| TIMEOUT | 32 | 131 | 128 | 153 | 225 | 142 | 111 | 110 | -1 |
| EXECFAIL | 0 | 17 | 10 | 10 | 10 | 10 | 10 | 10 | 0 |
| WEDGE | 63 | 125 | 14 | 12 | 13 | 16 | 13 | 15 | +2 |
| NOTRUN | 1313 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| **executed** | **556** | **1869** | **1869** | **1869** | **1869** | **1869** | **1869** | **1869** | **+0** |
| total | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 1869 | 0 |

### Transition matrix (tests whose class changed, r7 -> r8)
```text
r7 class   -> r8 class   count
PASS       -> FAIL          80
TIMEOUT    -> FAIL          22
PASS       -> TIMEOUT       17
TCONF      -> FAIL          13
FAIL       -> TIMEOUT       12
TIMEOUT    -> TCONF          6
FAIL       -> WEDGE          5
WEDGE      -> FAIL           4
WEDGE      -> TIMEOUT        3
TIMEOUT    -> PASS           3
PASS       -> CRASH          3
TCONF      -> CRASH          3
PASS       -> WEDGE          3
WEDGE      -> PASS           2
TIMEOUT    -> WEDGE          2
TCONF      -> WEDGE          2
FAIL       -> PASS           1
WARN       -> FAIL           1
TCONF      -> TIMEOUT        1
TIMEOUT    -> CRASH          1
FAIL       -> CRASH          1
PASS       -> WARN           1
WEDGE      -> TCONF          1
```
- **Improved vs r7** (now PASS/WARN, previously worse): **6** tests
- **Regressed vs r7** (previously PASS/WARN, now FAIL-or-worse/TCONF): **104** tests

**PASS trajectory: r1 35 -> r2 524 -> r3 659 -> r4 677 -> r5 602 -> r6 721 -> r7 771 -> r8 673 (+638 vs r1, -98 vs r7). First regression round of the series — the ext4 read-performance series landed a large new failure family (see findings); the r7 peak stands at 771.**

## Notable findings (r7 -> r8)

- **Regression round — the first since r1**: PASS 771 -> 673 (-98), FAIL 488 -> 589 (+101). The ext4-read-perf series bought a ~2x faster sweep but introduced three new failure families; the unix/epoll fix series was net-neutral-to-negative on WEDGE/CRASH.
- **mkdtemp-ENXIO storm, bisected to `c8f5a34` (ext4: batched async page-cache fills for sequential reads)**: `tst_tmpdir: mkdtemp(/tmp/LTP_*) failed: ENXIO (6)` went from 2 occurrences in all of r7's logs to 214 in r8, across 14 of 19 chunks, TBROK-ing ~86 tests at setup (the bulk of the 80 PASS->FAIL plus 22 TIMEOUT->FAIL transitions: chdir04, chmod01/03/07, chown01, dup01/03, lstat*, rename*, mknod*, statfs*, ...). **Minimal repro (2-test fresh boot)**: `access04` (falls back to /dev/loop0 + mkfs.ext2, write fails EIO, LOOP_CLR_FD fails EINVAL) followed by `acct01` whose very first mkdtemp in /tmp returns ENXIO. Bisect across the five fs/ext4 commits: 1447b31 clean, c8f5a34 reproduces — every test after a loop-format attempt hits the storm within the chunk. The kernel's mkdir path has no ENXIO of its own (the only -6 producers are the loop/virtio block request paths for unbound/not-ready devices), so this is block-layer state/errno corruption leaking into ext4 metadata operations after loop-backed writes.
- **Kernel heap exhaustion (ALLOCTHROW) CRASH family grew 1 -> 10**: sendfile08/08_64/09/09_64, sendmsg01/02/03, sendmmsg01/02 die on `ALLOCTHROW size=1 ... -> SIGKILL current task (task-context ENOMEM)` late in chunks c9/c15 — 1-byte allocations failing means the kernel heap is exhausted within a single 100-test chunk; the new batched read machinery is the prime leak suspect.
- **Read-path data corruption signatures**: read02/readv02/writev02 FAIL with buffer mismatches, mmapstress04 "unexpected value in map", diotest5 bufcmp mismatch, writetest/read_all broken — consistent with the `BufferHead::new_uninit` staging buffers (uninitialized until DMA fills them) being observable on failed/partial async fills.
- **The r6->r7 post-exec-hang family partially regressed**: 17 PASS->TIMEOUT (libm genacos/genasin/genceil/gencos/genfmod/genfrexp + close01/02, clone302, fpathconf01, mremap01, pipe13, write02, capset03/04, futex_wake03, mallocstress, clock_nanosleep04) — the family r7 had recovered wholesale via ext4 write batching is back in part.
- **New soft-lockup WEDGEs (5)**: capset03/04 (CPU#0 stuck 10s), mallocstress, mmapstress01 (CPU#3 stuck 37s), pth_str02.
- **All five r7 regressions recovered** (getgid03, openat01, kill12, lchown02, getrusage04 -> PASS; genlog TIMEOUT->PASS as well) — the only 6 improvements this round; flake noise from r7 is fully gone, the damage is all new-code damage.
- **Sweep performance roughly 2x faster than r7** despite the regressions: clean chunks ran 111-457s vs r7's 283-1291s (c4 264s, c10 111s, c12 201s) — the ext4 read-path work does deliver its throughput goal; total wall ~1h45m with 2 concurrent QEMU, zero host ENOSPC, 6 of 19 chunks needed a wedge/qemadied retry (c14 took 5 attempts; c3's extras were the missing-RUN fixup and the getrandom02 single-test rerun).
- **Operational notes (methodology, this round's `.ltp` copy only)**: (a) the harness killed both background sweep orchestrators mid-round (~55 min in); the persisted chunk logs + resumable sweep.py relaunch lost nothing (chunks 0-13 were already complete); (b) the r7 driver's bounded post-SWEEP-DONE drain (10s) was carried over and worked — no orphaned-serial hangs; (c) one serial interleaving again glued a trace line onto a RUN marker (getrandom02, same artifact class as r7's gethostname02) — a single-test rerun recorded it cleanly (PASS).

## By scenario/subsystem (r8)
```
subsystem              total    PASS    WARN    FAIL   TCONF   CRASH TIMEOUT EXECFAI   WEDGE  NOTRUN
can                        2       0       0       0       2       0       0       0       0       0
capability                 1       0       0       0       1       0       0       0       0       0
containers                45       7       0      15      23       0       0       0       0       0
controllers               10       0       0       9       1       0       0       0       0       0
crashme                    3       0       0       0       1       0       2       0       0       0
crypto                    10       0       0       1       9       0       0       0       0       0
cve                       21       1       0       1      16       0       2       0       1       0
dio                        6       2       0       3       0       0       1       0       0       0
dma_thread_diotest         1       0       0       0       1       0       0       0       0       0
fcntl-locktests            1       1       0       0       0       0       0       0       0       0
fs                        22       6       0       4       1       0       8       0       3       0
fs_perms_simple            1       0       0       1       0       0       0       0       0       0
helpers                  202      35       0      97      17      22      21      10       0       0
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
math                      10       4       0       0       0       0       5       0       1       0
mm                        57      12       1      18      20       1       4       0       1       0
net.ipv6_lib               6       1       0       2       2       0       1       0       0       0
nptl                       1       0       0       0       0       0       1       0       0       0
pty                        8       2       0       2       4       0       0       0       0       0
sched                     10       0       0       1       3       3       2       0       1       0
scsi_debug.part1           1       0       0       1       0       0       0       0       0       0
smack                      1       0       0       0       1       0       0       0       0       0
syscalls                1356     601       1     430     246      10      60       0       8       0
tracing                    1       0       0       0       1       0       0       0       0       0
uevent                     3       0       0       0       3       0       0       0       0       0
watchqueue                 9       0       0       0       9       0       0       0       0       0

```

## Top failure clusters (top 20, r8)
```text
  [451] FAIL     (no TFAIL line)   e.g. access04,acct01,capget02,capset02,chdir04,chmod01
  [151] TCONF    (no TFAIL line)   e.g. add_key01,add_key02,add_key03,add_key04,bpf_map01,bpf_prog01
  [100] TIMEOUT  (no TFAIL line)   e.g. capset03,capset04,chdir01,clock_nanosleep02,clock_nanosleep04,leapsec01
  [ 29] TCONF    (from serial-c15.log)
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
cp-trace EXECVE by pid=N a0=ADDR
tst_hugepage.c:N: TCONF: hugetlbfs is not supported
FUT:    e.g. hugefork02,hugemmap05,hugemmap06,hugemmap07,hugemmap08,hugemmap09
  [ 26] CRASH    (no TFAIL line)   e.g. sendfile08,sendfile08_64,sendfile09,sendfile09_64,sendmsg01,sendmsg02
  [ 22] TCONF    cp-trace EXECVE by pid=N a0=ADDR
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
FUT: REPEAT teardown of root ppn=ADDR (ring[N])
tst_kconfig.c:N: TINFO: Constra   e.g. fcntl39_64,io_destroy02,io_getevents01,io_setup02,keyctl09,msgget05
  [ 14] WEDGE    (no TFAIL line)   e.g. ppoll01,prctl04,prctl07,prctl09,sendto01,splice03
  [ 10] TCONF    tst_kconfig.c:N: TINFO: Parsing kernel config '/proc/config.gz'
cp-trace EXECVE by pid=N a0=ADDR
cp-trace EXECVE by pid=N a0=ADDR
tst_kconfig.c:N: TINFO: Constr   e.g. acct02,clock_gettime03,msgget04,shmget05,sysinfo03,timerfd04
  [ 10] EXECFAIL (no TFAIL line)   e.g. erst-inject,lddfile.out,thugetlb,tinjpage,tkillpoison,tprctl
  [  9] FAIL     (from serial-c17.log)
```
