# Compatibility review — findings ledger (review-compat lane, 2026-10-31/11-01)

Scope: make stock software behave right, driven by the freshest LTP failure
data. Baseline data: riscv64 full scan r20 (1,028/1,869, kernel af36f8ce,
files under /home/william/rux-agents/ltp-r21/results/) + the r21 x86_64
first-scan bug list. This worktree's HEAD (4a757d6e) is 372 commits past the
r20 kernel, so every candidate was re-triaged in isolation on HEAD BEFORE
any fix: one LTP case per boot, `test/ltpscan/scan.py --chunk 1` against a
private reflink copy of the shared LTP rootfs (run/rootfs-ltp-compat.img).

## 1. Triage on HEAD (27 cases, one per boot) — run/triage-baseline.txt

Already fixed between r20 and HEAD (no action): syslog12 (syslog negative
length EINVAL), rt_sigaction01 (RT-signal act parse), execve02/03/05 (r20
CRASHes were host-starved EXT4-BIGLOCK flakes per iso20 verdicts anyway).

Still failing on HEAD (25): xattr errno family (setxattr01, fsetxattr01,
fremovexattr02), fgetxattr02 (char-dev open ENXIO), sched affinity family
(getcpu01, nice05), gethostname02, settimeofday02, sigrelse01 (signal()
EINVAL at SIGRT 34), sigtimedwait01 + sigwaitinfo01 (timeout), futex
family (cmp_requeue01 now fork()-ENOMEM past ~N children, cmp_requeue02,
wait05 oversleep, wait_bitset01 hang), epoll family (wait02, wait06,
pwait03), clone08 (CLONE_THREAD EINVAL), clone303, rmdir02 + readlink03
(ENOENT→ENOTDIR on the 43-hop ring), pipe2_04 (F_SETPIPE_SZ EINVAL),
open11, semctl06.

## 2. Fixes landed (commit → cases flipped red→green, isolation 2x)

### F1. xattr NAME read double-negated its errno — 5252505e
`xattr_read_name` did `Err(e) => return Err(-(e as i64))` on
strncpy_from_user's result, but strncpy_from_user already returns a
NEGATIVE errno. Every setxattr/fsetxattr/lsetxattr/getxattr/lgetxattr/
removexattr/lremovexattr/f*-variant (and the new *xattrat family) that
failed reading the NAME pointer returned **+14** to userspace with errno
untouched. LTP: setxattr01/fsetxattr01 printed
"should fail with EFAULT: SUCCESS (0)"; fremovexattr02 printed
"fremovexattr(2) returned 14".
Flipped (pass A, one boot each): setxattr01 9/9, fsetxattr01 9/9,
fremovexattr02 3/3, plus regressions checked: getxattr01 PASS,
removexattr01 PASS.

### F2. Default task affinity covered never-online CPUs — a9cbdac2
`Task::new`/`new_task_at` initialized cpus_allowed to !0 (all 32 bits);
sched_getaffinity masked that to MAX_CPUS=4 bits, so a -smp 2 boot
advertised CPUs 0-3 while sched_setaffinity (correctly) rejects masks
that name never-online CPUs with EINVAL. The kernel's own contract was
self-inconsistent; LTP getcpu01/nice05 take the highest advertised CPU
and hand it back to sched_setaffinity → TBROK EINVAL.
Fix: default mask = (1 << num_started_cpus()) - 1 at task creation, plus
`refresh_default_affinity_to_online()` after SMP bring-up (all three
arches) widening the boot-time tasks (boot init pid 1 + early kthreads,
none carrying user affinity yet) — the reference kernel's cpu-online
refresh analogue. Serial evidence: "affinity: 6 of 6 task(s) default
mask -> 0x3" on -smp 2.
Flipped: getcpu01 ("getcpu() returned proper cpuid:1" — was TBROK
sched_setaffinity EINVAL). Regression checks: sched_setaffinity01 PASS.
nice05 now gets PAST affinity and hits a different gap (see §3-6).

### F3. Default hostname "(none)" — b5427e87
UtsNamespace::new zeroed the hostname; the reference kernel's
init_uts_ns defaults BOTH nodename and domainname to "(none)".
Note: gethostname02 does NOT flip with this (see §3-2) — the init
namespace is seeded with hostname "rux" anyway; the commit aligns the
never-set default with the reference.

Gates: riscv64 build rc=0; x86_64 build rc=0; `make test` suite verdict
green (in-tree units, private reflink rootfs — the shared rootfs.img was
flock-contended by a concurrent lane).

## 3. Adjudicated NOT-kernel-gap (do not burn cycles here)

1. **gethostname02** — musl's gethostname TRUNCATES and returns 0 when
   len < strlen(nodename)+1 (toolchain/musl-1.2.5 gethostname.c); it can
   never return ENAMETOOLONG. The LTP image is a static-musl build, so
   this test fails on the reference kernel too under this libc. Flips
   only with a glibc-built LTP.
2. **rmdir02 / readlink03** — the ELOOP errno contract itself is CORRECT
   (probe-measured: 40 follows max, matches MAXSYMLINKS; parent-walk
   arithmetic verified for unlinkat/readlinkat/newfstatat). The observed
   ENOTDIR tails are the LTP framework's own cleanup rmobj() walking the
   43-hop ring (unlink of "…/loopdir/" with trailing-slash semantics,
   see §4-3), after the test already aborted after subtest 1 for
   framework-side reasons (no TFAIL line for the ELOOP case is ever
   printed). The tests' abort shape needs the LTP new-API results-IPC
   understood first; not a one-line kernel divergence.
3. **futex_wait05 "slept too long"** — timing-precision class under TCG
   (median 10.13 ms for a 10 ms sleep, max 94 ms under host load);
   belongs to the timer/scheduling precision lanes, not a contract bug.
4. **lgetxattr01 TCONF** ("no xattr support in fs") — the case runs on
   tmpfs; TCONF either way, not a regression.

## 4. Filed items (feature gaps / follow-ups, not fixed here)

1. **Per-thread/per-process CPU-time clocks** — clock_gettime with
   pthread_getcpuclockid() clockids returns EINVAL (nice05.c:129 TBROK
   "clock_gettime(INVALID/UNKNOWN CLOCK) failed: EINVAL" AFTER the
   affinity fix unblocked its setup). Needs clockid decoding
   ((~tid<<3)|2 thread / (~pid<<3)|0 process) + cputime accounting
   exposure. Affects nice05, likely getrusage03 cousins.
2. **RVV for userspace** — gcc-15/Ubuntu 25.x static glibc binaries
   contain unconditional RVV instructions (probe faulted on 0xcd817057,
   OP-V vsetvli-class). Kernel never sets sstatus.VS, so every vector
   instruction traps as SIGILL; QEMU cpu flags in the standard flows
   don't enable v either. Any newer static binary will die at startup —
   needs a coordinated decision (sstatus.VS + HWCAP 'v' + cpu flags).
   Evidence: run/probe-serial-miniprobe.log (SIGILL-FORENSIC epc page
   dump; binary bytes at epc decode to OP-V).
3. **Trailing-slash + NOFOLLOW divergence** — path_lookup treats
   "path/" ending at a symlink (LOOKUP_NOFOLLOW) as ENOTDIR
   (vfs.rs:2141 check fires because the final symlink is NOT followed),
   where the reference kernel follows the link for the trailing slash
   and reports the target's type. Found via probe ring
   (readlinkat("…/loopdir/") → -20). Low frequency but a real contract
   divergence; fix belongs in the walk's terminal-component handling.
4. **name_to_handle_at / open_by_handle_at** (4 cases: 01/02 both) —
   EOPNOTSUPP; file handles are a feature (export_operations-style),
   filed not fixed.
5. **fgetxattr02** — TBROK open(mntpoint/fgetxattr02chr) ENXIO: opening
   a freshly-mknod'ed char device (dev 1,x) finds no driver. Needs
   char-device open-by-devno routing (null/full/zero majors).
6. **futex_cmp_requeue01** — r20 timeout/403-fails became fork() ENOMEM
   on HEAD: the test forks 403 waiters; fork starts failing partway.
   Suspect pid/threads accounting or the clone heap-reserve gate
   (clone-gate ENOMEM print did NOT fire — needs a focused look at
   pid_max/RLIMIT_NPROC accounting under many children).
7. **clone08 CLONE_THREAD EINVAL** — my syscall-only probe creates
   CLONE_THREAD|CLONE_SIGHAND|CLONE_VM(±FS|FILES|SETTID) threads
   successfully, so the EINVAL is context-dependent (LTP runs the clone
   from a forked child of the framework). Needs an strace-style
   reproduction of the exact failing call.
8. **sigrelse01 (signal 34 EINVAL)** + **sigtimedwait01/sigwaitinfo01
   (timeout)** — probe shows rt_sigtimedwait itself works (returns 10
   with si_signo=10) with valid pointers; the LTP hangs/errno come from
   the tests' own fork/handshake shapes. Needs per-subtest triage.
9. **epoll_wait02/06, epoll_pwait03** — timing-threshold and hang
   classes; epoll_wait06's 2 TFAILs are event-data contract details;
   needs the subtest tails (not captured by the 300-char result tails).

## 5. Probe assets (reusable)

- run/probe/compatprobe.c — syscall-only (no libc) probe: symlink ring
  arithmetic (unlinkat/readlinkat/newfstatat errno per hop count, n=38
  ..46), clone flag variants, rt_sigtimedwait wake, sched_[gs]etaffinity.
  Build: riscv64-linux-gnu-gcc -nostdlib -static -march=rv64imafd.
  NOTE: gcc-15 glibc-static probes SIGILL (§4-2) — keep them libc-free
  and initialize gp (`la gp, __global_pointer$`) or statics addressed
  via gp decode to garbage pointers.
- run/probe-inject.sh + run/probe-boot.sh — debugfs-inject a probe into
  run/rootfs-ltp-compat.img as /test/linux-ltp/testcases/bin/<name> and
  boot it through the ltpscan runner.
- run/triage-run.sh, run/verify-run.sh — chunk=1 isolation drivers
  (scan.py); verify-run.sh pins --kernel run/rux-fixed-snapshot so
  concurrent `make test`/PLATFORM rebuilds cannot swap the binary
  mid-scan (that poisoned one verify round: the unit-test kernel booted
  instead and wedged the scan — see run/verify-a.log).
