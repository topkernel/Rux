Lane: LTP r27 — census + gap-filling (x86_64+KVM)
Base: main@12c5e326 — the ltp-r21 merge landed ALL r22-r26 fixes
(close-parity retire_complete, ppoll sigmask, SysV IPC ns/limits,
sysfs(2), ETXTBSY, O_DIRECT flush, execveat, idle lying-slot heal,
timers ns deadline, poll RLIMIT gate, SA_RESTORER, WCONTINUED,
futex physkey page-in). Work autonomously; never ask.
Worktree = THIS. test/rootfs-x86_64-ltp.img is symlinked to the
central store — boot with -snapshot + file.locking=off only.
History of prior sessions (r20-r26 + inherited lane notes):
TASK-history.md.bak (4163 lines).

## Mission
Baseline: r26 census 1067/1869 (results/results-r26.txt on this
branch's history; delta files alongside). Target: beat 1067.

### A. CRASH family (28) — isolate every one (r26 classes)
cpuctl_def_task01-04 + cpuctl_test01-04 (8, cgroup cpu controller
face), sched_tc0/tc1/tc6 + sched_driver + pm_get_sched_values +
trace_sched (sched conformance set), rt_sigaction01 +
rt_sigprocmask01 (**still CRASH in r26 DESPITE the SA_RESTORER fix
being on the scan tree — isolate and decode; the fix either misses a
subtest shape or the crash is a different face**), acct02_helper
(BSD process acct), frag, genexp_log/genpower/gentrigo (float
class), shmat1, vma05_vdso, prot_hsymlinks, eject_check_tray.
Isolation rig: single-test boots 2x; capture the panic/abort shape;
symbolize with addr2line. Kernel fixes follow reference semantics
(NEVER the word "Linux" in commits — write "the reference kernel").

### B. FAIL families with real kernel work (from r26)
- name_to_handle_at + open_by_handle_at (4): exportable file handles.
- ptrace06 TIMEOUT + ptrace FAILs (3): the VA-translate fix landed;
  find the remaining faces.
- mincore (3), madvise (2), mprotect/mremap shapes under mmap FAILs.
- sysctl (3): /proc/sys shapes.
- pidns (3) + clone (3): namespace/flag gaps.
- mount (3): mount(2) edge semantics.
- pty (2 FAIL + 4 TCONF): ptmx/TIOCGPTN family.
- setsockopt (2 FAIL + 7 TCONF): which options; implement the cheap
  sock-opt acks with reference semantics.
- io_uring (2), getrusage (2), nftw (2 — probably ftw semantics),
  testsf_s/testsf_c (4).
- mmapstress 8 = adjudicated NOT-OURS host parity (verdicts-r25);
  ftest 4 = stress lane owns (fix landing on main separately —
  re-verify once main moves past 12c5e326); mmap1/mmap3/shm_test =
  documented budget/racy classes. Do NOT re-litigate.

### C. TCONF quick wins (503 total; ranked by cheapness)
- pidfd_open/pidfd_send_signal family (4): cheap fd-for-pid.
- move_pages (11): non-NUMA reference semantics (single node).
- autogroup, memfd_secret, open_tree/move_mount/fspick trio.
- add_key/keyctl/request_key (18): check what LTP actually asserts;
  reference errno shapes beat blanket ENOSYS if the tests assert
  behavior, else leave for a real keyring subsystem round.
- af_alg (7): assess. fanotify (23)/hugemmap (31)/quotactl (9)/
  userns (8)/bpf (7)/ksm (7): effort estimates into TASK.md for the
  coordinator; don't sink the round.
- CONFIG_BSD_PROCESS_ACCT: check how tst_kconfig reads it — if it's
  a /proc/config export, that's one file.

### D. Census
Full 1869 scan (negmemo=1, --append flow, rescue/splice discipline
from r26 — wedge rows rerun in isolation boots 2x before counting;
run/ltpscan-x86_64 + run/r25-drive.sh + run/r24-pipeline.sh are the
rigs, archived on this branch). results/results-r27.txt +
delta-r27-vs-r26.txt. Push origin/ltp-r27.

## Discipline (binding)
- One QEMU at a time (check ps; other lanes live); kill only yours;
  pkill FORBIDDEN. KVM for x86_64. -smp 2 -m 1G for scans (census
  boots used 2G — check the r26 driver log for the exact shape).
- Known flakes (rerun 2x before counting): accept02 getrusage04
  kill02 genfmod rename/statfs families + float_exp_log.
- Kernel changes: small commits, English, reference-parity comments.
  Gates per change: affected family 2x green in isolation. Round
  close: x3 builds rc=0 (x86_64/riscv64/aarch64; aarch64 rewrites
  dtb_blob.S — git checkout after), make test green (riscv64,
  lock-aware wait on test/rootfs.img — run/r26-maketest-wait.sh),
  Tier-2 pbench if hot paths touched (sched/fs/ipc/mm/timer).
- NEVER write to /home/william/workspace/Rux. Main-repo merges are
  the coordinator's.
- Throughput-bound faces (timerfd_settime02, msgstress01): the fdt
  conditional-refcount fast path is the real fix — note, don't hack.
Final message: r27 number + delta table + 2x rerun evidence for every
new-failure verdict + fixes list + results path.

## SESSION 7 (r27) LOG — branch ltp-r27 @ 12c5e326+ (2026-11-04/05)

### Kernel fixes landed (6 commits, all 2x-green in isolation)
1. **83fa2937 pidfd subsystem** — pidfd_open/pidfd_send_signal/pidfd_getfd,
   waitid(P_PIDFD) incl. the nonblocking EAGAIN contract, poll-exit
   readiness (global pidfd queue woken in do_exit), /proc/<pid> dir fds
   as pidfds with PINNED-instance identity (pid reuse resolves ESRCH),
   /proc/sys/kernel/ns_last_pid rw (CRIU-style cursor moves).
   Family: 8 PASS + 1 WARN (pidfd_send_signal03 TWARN: /proc ino
   aliasing across pid reuse — instance-coded inos deferred).
2. **09dce204 name_to_handle_at / open_by_handle_at** — 12-byte
   [gen][ino] ext4 handles, export iget from disk, EOVERFLOW size
   probe with the reference's copy order (mount id + size out before
   EOVERFLOW), AT_SYMLINK_FOLLOW=0x400 fixed, symlink handles ELOOP,
   CAP_DAC_READ_SEARCH gate, ESTALE for cross-fs mount fds. 4/4 PASS.
3. **7f70866e x86_64 syscall-hole parity** — native 156 (_sysctl) no
   longer falls through to the generic table's getsid (ESRCH); ENOSYS
   like the reference's hole. sysctl01/03/04 FAIL -> TCONF
   (host-verified identical verdict).
4. **1994215f mincore/mlock residency** — mincore's Sv39-hardcoded
   walk replaced with the portable PageTableWalker (x86 read garbage
   before: every page non-resident); file-backed no-PTE pages consult
   the PAGE CACHE (reference __mincore_unmapped_range); private reads
   of file holes instantiate cached zero folios (reference folio
   semantics); mlock populate faults through handle_mm_fault with
   WRITE faults for anon/shm (Rss; PROT_WRITE-only mappings) and READ
   for file spans. mincore02/03/04 + mlock05 PASS 2x.
5. **f9643a85 pty devpts parity** — slave nodes 0620, pairs born
   locked (TTY_PTY_LOCK; TIOCSPTLCK real), slave-write-after-master-
   hangup returns EIO (never SIGPIPE). pty01 all 5 blocks PASS.
6. **6e7b911c RUSAGE_CHILDREN** — cmaxrss/exit_maxrss zero-init in the
   raw Task initializers (recycled-page garbage), fold moved to REAP
   time (release_task; includes the child's own children totals).
   getrusage03 6/7 (last face: SIGCHLD-SIG_IGN auto-reap, open).

### Section A (CRASH families) — all 28 adjudicated (verdicts-r27.txt)
- rt_sigaction01/rt_sigprocmask01: NOT-OURS re-confirmed with FRESH
  host runs (rc=139 both) — musl never fills oact.sa_restorer, LTP
  harvests stack junk as the restorer; the "SA_RESTORER fix missed a
  face" theory is dead: the landing was correct, the crash is the
  test's own junk restorer.
- 20 helper/usage/environment artifacts (prot_hsymlinks needs useradd,
  sched_tc0/1/6 need env vars, cpuctl/memctl/trace_sched/frag/gen*/
  acct02_helper/eject_check_tray/vma05_vdso/pm_get_sched_values/
  sched_driver/testsf_* = bare-invocation helpers).
- REAL gap kept: cgroup_core01/02 (cgroup2 subsystem; same bucket as
  clone303 CLONE_INTO_CGROUP and the controller faces).
- clone08: NOT-OURS by a NEW decode — musl 1.2.5's clone() wrapper
  rejects CLONE_THREAD/SETTLS/CHILD_CLEARTID with EINVAL IN LIBC
  (src/linux/clone.c badflags); raw syscall succeeds on our kernel
  (probe ltp_r27_clone_argshape.c archived in probes.tar.gz). Host
  parity: rc=2, same TBROK.

### Gates
x3 builds rc=0 (x86_64 final, riscv64, aarch64 + dtb_blob.S restored).
Census + make test + Tier-2 pbench (mm fault path + fs touched): see
below/committed logs.

### r27 census: <TO-FILL> /1869
