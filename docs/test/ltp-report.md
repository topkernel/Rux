# LTP compatibility report

Two rigs, two baselines — they are different axes (architecture AND
accelerator), never mixed in a trend line:

| Rig | Baseline | Date |
|---|---|---|
| riscv64 · TCG (minimal rootfs) | **1028 / 1869** (r20) | 2026-10-31 |
| x86_64 · KVM (Ubuntu rootfs — the standing rig per the 2026-11-01 directive) | **1010 / 1869** (r21, first scan on this rig) | 2026-11-01 |

## riscv64 · TCG trend

35 → 524 → 659 → 677 → 602 → 721 → 771 → 673 → 801 → 795 → 945 →
1005 (r15) → 1018 (r19) → **1028 (r20)**. Scan records: the LTP lane's
`results/` directories; the r20 losses were adjudicated host-starvation
flakes (0 real regressions), with 4 from-FAIL gains verified 2x.

## x86_64 · KVM (r21, first baseline on the new rig)

- 72 rows re-triaged against HEAD at scan time: 43 true failures, 29
  newly-green confirmations, 4 load flakes (rerun clean).
- This week's kernel fixes flipping rows on this rig: the whole
  MPROTECT family, prlimit/getrlimit kernel-pointer EFAULT (SMAP +
  bootstrap-identity-map drop + exception-path ordering).
- Six new kernel bugs filed with symbolized stacks
  (`results/bugs-r21.txt`), head of the queue: a kernel-mode panic in
  the core-dump path (`do_signal → do_exit → do_coredump` memcpy
  fault, isolated 2x); also syslog() negative-length SUCCESS,
  process_vm_readv/writev cross-process misbehavior, rt_sigaction01
  signal-35 SIGILL, and a blocking-socket hang family (recv/send/
  connect/accept4).

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
