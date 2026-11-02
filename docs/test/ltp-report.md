# LTP compatibility report

Two rigs, two baselines — they are different axes (architecture AND
accelerator), never mixed in a trend line:

| Rig | Baseline | Date |
|---|---|---|
| riscv64 · TCG (minimal rootfs) | **1028 / 1869** (r20) | 2026-10-31 |
| x86_64 · KVM (Ubuntu rootfs — the standing rig per the 2026-11-01 directive) | **1030 / 1869** (r22, main 819a1be2 + this round's two fixes) | 2026-11-02 |

## riscv64 · TCG trend

35 → 524 → 659 → 677 → 602 → 721 → 771 → 673 → 801 → 795 → 945 →
1005 (r15) → 1018 (r19) → **1028 (r20)**. Scan records: the LTP lane's
`results/` directories; the r20 losses were adjudicated host-starvation
flakes (0 real regressions), with 4 from-FAIL gains verified 2x.

## x86_64 · KVM trend

1010 (r21, first scan on this rig) → **1030 (r22)**. r22 ran on main
819a1be2 plus this round's two fixes (below); the r21 bug-list head
(coredump-path panic) and the blocking-socket hang family were already
fixed on main by the r21-bugs lane — both re-verified 2x in isolation
this round (kill11/waitpid01/abort01 and recv01/recvfrom01/send01/
sendto01/connect01/accept4_01 all rc=0, zero kernel faults).

### r22 (2026-11-02): the chunk@700 wedge and the two regressions it exposed

- The scan's first pass lost a whole 100-case chunk (nftw01 first) to a
  deterministic boot panic: a task executing kernel code returned
  through zeroed memory to rip=0 with rsp ~19KB below its kernel stack
  (`PANIC: Kernel stack overflow`, 3/3 reproducible in isolation).
  Bisect dd94e441..819a1be2: first bad 5542a7b6 (perf(fs) lock-free
  walk/stat/open fast paths). Root cause class: the x86_64 80KB packed
  kstack slot's margin was measured on OH loads only (62,512B max);
  the LTP walk/open chains nest deeper, spill frames past the slot
  into adjacent slab/heap pages and zero them — the fee7774f failure
  family ("wild-jump panics", nice=-120 corruption), which also
  resurfaced in isolation as getpriority01's -120. Fix: x86_64 kstack
  back to 128KB (Kernel.toml), packed slab re-fit at 2×128KB per 256KB
  block (zero waste). After the fix the chunk reran 100/100 reported,
  zero panics; results match r21 class-for-class (one old POISON
  resolved to TCONF).
- A second bisect on the stat-family failures (stat01/fstat02(_64)
  st_size=0 after write, inotify01) convicted 0b30585e (ext4 delayed
  allocation): `refresh_inode_cache` overwrote the sb-boxed inode copy
  with the lagging DISK-fresh state (chmod → stat served the pre-write
  size). Fix: the dirty-tail EOF watermark (the same one the icache
  revival guard uses) now holds the published size in
  `refresh_inode_cache` and the link-path twin. stat01/stat01_64/
  fstat02/fstat02_64/inotify01 flipped back to PASS (verified 2x).
- Net vs r21: +30 gains (all six r21 bug families green, the compat
  fixes, ftest03/04, epoll_wait05, recvmsg01, hackbench, sendmsg02,
  cve-2017-17052, epoll-ltp), -10 real losses of which 5 were recovered
  by this round's fixes; remaining open items below.

### r22 open items (filed, not this round's fixes)

- mmap02/diotest2/diotest5: O_DIRECT reads and mmap fills after
  buffered (delayed-allocation) writes still serve the lagging disk
  state — the delalloc read-coherence follow-on (flush-before-DIO-read
  and mmap-fill-through-dirty-frames).
- getpriority01: getpriority(PRIO_USER,0) returns -120 — a task in the
  hash with pid=0 comm=ksoftirqd/1 and static_prio=0 (all constructors
  init 120; survives the 128KB fix; probe evidence in
  run/compat-r3-notes.md).
- creat07 (FAIL→TBROK-timeout), mmap18 (FAIL→TIMEOUT): isolation
  reruns still time out; separate hangs, not adjudicated yet.
- oom01 parks as POISON in-flight at a stall (OOM-path stall under the
  oom01 stress; r21 passed it).
- getrusage04: documented host-load flake family, unchanged.

## Compat-lane fix log (2026-11-01, all isolation-verified 2x)

- xattr errno contract (`strncpy_from_user` double-negation → +14 to
  userspace): setxattr01, fsetxattr01, fremovexattr02 flipped.
- sched affinity default mask (online-CPU mask, not all-MAX_CPUS):
  getcpu01 flipped.
- UTS default hostname aligned (`(none)`).

Adjudicated NOT kernel gaps: gethostname02 (musl truncation shape),
rmdir02/readlink03 (LTP cleanup shape; ELOOP budget probe-verified),
futex_wait05 (TCG timing). Filed for later: per-thread cputime clocks,
userspace RVV gating (sstatus.VS), trailing-slash+NOFOLLOW ENOTDIR,
name_to_handle_at, char-dev open-by-devno, fork-ENOMEM at 400+
children, clone08 context EINVAL.
