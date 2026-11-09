# Review round 4 — syscall / ipc / net / io_uring (agent r4-syscall)

Scope: kernel/src/syscall (36,860), kernel/src/ipc (16,665), kernel/src/net (20,218),
kernel/src/io_uring (1,202). Full-file coverage; findings appended incrementally.
Reference: /home/william/refer/linux/. Dedup: review-coverage.md defects table,
docs/archive/code-review-2026-09-12.md, prior-round fixed items.

---

Findings
========

### [Medium] process/task.rs:1403 + syscall/time.rs:1242 — POSIX timers and itimers are per-Task (per-thread), Linux has them per-process — ✅ FIXED (`9bf06ed7`)
`posix_timers: Spinlock<Vec<PosixTimerState>>`, `itimer_real/virt/prof` live directly on `Task`. Linux
keeps itimers and POSIX interval timers in the shared `signal_struct` (CLONE_THREAD shares them).
Trigger: `timer_create` in thread A, `timer_settime`/`timer_delete` from thread B returns EINVAL (timer
handle unknown in B's list); `alarm()` armed in one thread is invisible to `alarm(0)`/`getitimer` in a
sibling; ITIMER_VIRTUAL counts only the arming thread's `sum_exec_runtime` (Linux: process utime).
glibc's timer_create/timer_settime contract is process-wide. Fix direction: move the timer vector and
itimer fields into the shared signal struct (whatever CLONE_THREAD aliases today) and look up timers
through the thread-group leader.

### [Medium] syscall/time.rs:1454 + timer.rs wheel action — POSIX timer signal carries no siginfo; overruns never accumulate — ✅ FIXED (`0843bb2a`)
Wall-clock POSIX timers arm the wheel with a bare `signo` (`add_timer_with_action_hres(..., timer.sigev_signo, ...)`);
expiry delivers a plain `send_signal(pid, signo)`. Linux posix-timers deliver `SI_TIMER` siginfo with
`si_value = sigev_value.sival_ptr` and `si_overrun`; a `SA_SIGINFO` handler cannot tell which timer
fired (sigqueue-based timer users, and glibc's `timer_getoverrun` consumers, break). Also
`overrun_count` is only seeded by the late-TIMER_ABSTIME arm in `sys_timer_settime`; the periodic
expiry path never increments it, so `timer_getoverrun` after a missed delivery reads 0.
Fix direction: extend the wheel action to carry (timer handle, sival) and enqueue a siginfo-carrying
signal; count expirations between deliveries on the wheel/tick path.

### [Low] syscall/time.rs:1454 — SIGEV_NONE wall-clock timers still deliver their signal — ✅ FIXED (`0843bb2a`)
`sys_timer_settime` arms `sigev_signo` unconditionally for CLOCK_REALTIME/MONOTONIC/BOOTTIME/TAI; the
CPU-time tick path (`sched/sched.rs:6936`) checks `sigev_notify != SIGEV_NONE`, the wheel action path
does not. Linux: SIGEV_NONE = timer runs, never notifies (timer_gettime still counts down).
Trigger: `timer_create(CLOCK_MONOTONIC, &(SIGEV_NONE timer), ...)` + `timer_settime` → signal arrives.
Fix: keep the wheel arm for the deadline but pass signo=0 when sigev_notify == SIGEV_NONE.

### [Medium] syscall/signal.rs:250-331 — rt_sigaction copies oldact out BEFORE reading act; Linux reads act first, swaps, then copies oldact out — ✅ FIXED (`3df6acb6`)
Ordering: Rux writes `oldact` to user, then copies `act` in, then installs. Linux `sys_rt_sigaction`
does `copy_from_user(act)` → `do_sigaction` (install) → `copy_to_user(oact)`. Divergences: (a) Rux with
a faulting `act` pointer writes a modified `oldact` first (user-visible side effect on a failed call);
Linux touches nothing. (b) Rux with a faulting `oldact` pointer leaves the new action NOT installed;
Linux installs it and returns EFAULT. Trigger: `sigaction(SIGUSR1, bad_ptr, &old)` /
`sigaction(SIGUSR1, &new, bad_ptr)`. Fix: copy act in first, install, then copy oldact out
(match kernel/signal.c sys_rt_sigaction).

### [Low] syscall/signal.rs:486 — rt_sigpending returns `pending & blocked`; Linux returns the raw pending union — ✅ FIXED (`bcbd69f7`)
`do_sigpending` (kernel/signal.c) ORs private and shared pending with no blocked-mask intersection;
`sigdelsetmask(SIGKILL|SIGSTOP)` only. Rux masks out unblocked pending signals, so a raised-and-
undelivered signal (task still in kernel on the delivery check) reads as not-pending. Trigger: raise a
signal to a task busy in a long syscall, sigpending() from another thread. Fix: report the full
`pending` set.

### [Low] syscall/time.rs:414-445 — nanosleep remainder quantized to whole milliseconds, rounded UP — ✅ FIXED (`bcbd69f7`)
`remaining_ns` is converted via `remaining_msecs = (ns + 999_999)/1_000_000` and back to sec/nsec, so
`rem` can exceed the actual remaining time by up to 1 ms — and exceed the original REQUEST when the
request is < 1 ms or interrupted early (POSIX: rem <= req; LTP clock_nanosleep02 monotonicity checks
tolerate some slack). Fix: copy `remaining_ns` split into sec/nsec directly (drop the msec round-trip).

### [Low] syscall/dispatch.rs:274-278 — x86_64 native NR 34 (a table hole on real Linux) implemented as pause()
On the x86_64 table 34 is unused (glibc musl never issue it; Linux returns ENOSYS). Rux sleeps until a
signal. A program probing ENOSYS behavior of nr 34 hangs instead. (The sibling legacy arms — 7 poll,
23 select, 33 dup2, 37 alarm, 139 sysfs, 213/232 epoll — are all real x86_64 numbers and correct.)
Fix: return ENOSYS for 34 like Linux, or keep the shim documented as intentional.

### [Low] syscall/time.rs:1952-1956 — get_robust_list always reports head=0 and ignores put_user failures — ✅ FIXED (`bcbd69f7`)
Linux returns the address the task registered via set_robust_list. Rux stores `0` (set_robust_list
apparently discards the pointer — see process.rs) so no round-trip is possible; and the two `put_user`
results are discarded, so an in-range-but-unmapped pointer silently "succeeds" instead of EFAULT.
Fix: store the registered head in the task and report it; propagate put_user failure.

### [Low] syscall/time.rs:555-561 — clock_settime checks CAP_SYS_TIME before the clock-id validity — ✅ FIXED (`bcbd69f7`)
`clock_settime(CLOCK_MONOTONIC)` without the cap returns EPERM; Linux's posix-clock dispatch fails
EINVAL (no clock_set op) before any permission check. Trigger: unprivileged
`clock_settime(CLOCK_MONOTONIC, ...)` → Rux EPERM, Linux EINVAL. Fix: validate clk_id first
(only CLOCK_REALTIME is settable here).

### [Info] syscall/sched.rs:1167-1226 — sched_getaffinity len handling
Accepts any size >= 1 and returns min(kernel_bytes, len); Linux requires len*8 >= nr_cpu_ids and len
aligned to 8, returning EINVAL otherwise and `len` on success. Small ABI divergence; glibc always
passes a full-size mask.

### [Info] syscall/sched.rs:741-929 — sched_setattr accepts then ignores SCHED_FLAG_* bits (0x7f mask)
SCHED_FLAG_RESET_ON_FORK (0x01) and KEEP_PARAMS (0x10) are validated as "known" but not implemented
(setscheduler's RESET_ON_FORK via policy-bit is implemented; the setattr-flags route is not).
sched_getattr reports util_min=0/util_max=0xFFFFFFFF (Linux: 0/1024 defaults).


### [High] syscall/io.rs:848-1128 — readv/writev consume RE-FETCHED iovec entries after whole-vector validation (TOCTOU)
`validate_iov_entries` walks the user array (access_ok per entry), but every consumer loop then re-reads
each `Iovec` from user memory and transfers to/from the freshly-read `iov_base` with no re-validation:
- writev stream path: `read_user::<Iovec>` at io.rs:1023 then `write_file_core(fd, &file, base, len)` (which
  trusts "validated by caller").
- writev AF_UNIX dgram gather: THREE separate fetches (validate :850, reserve :933, gather :952) — the
  copy at :972 uses the third fetch's base.
- readv: fetch at :1092 then `read_file_core`.
A racing thread (same process, CLONE_VM) can swap `iov_base` to a kernel-range pointer after validation:
the copy then reads/writes kernel memory (info leak / kernel memory corruption) — the exact defect class
fixed for recvmsg in commit 32fd359f (single kernel snapshot). Fix: snapshot the whole iovec array into a
kernel Vec once (like `check_iov_array` computes size for) and consume the snapshot in every loop.

### [High] syscall/misc.rs:2273-2312 — epoll_wait's dead-interest purge keys on the fd NUMBER, not the open file description
The close-path purge (`epoll_purge_closed_fd` in misc.rs:4474, gated by a Weak strong-count since the
appspawn fork fix) correctly survives dup/fork. But `sys_epoll_wait`'s scan independently purges any
entry whose `fdtable.get_file(entry.fd)` no longer resolves to the registered `file_id`
(misc.rs:2302-2312, "dead_fds"). Trigger: `epoll_ctl(ADD, fd=5)` then `dup2(5, 9)` then `close(5)` —
fd 5's slot is empty, so the next `epoll_wait` silently removes the interest even though the open file
description is still alive at fd 9. Linux removes an epitem only when the file description's last
reference drops (eventpoll_release), so the watch keeps firing through any dup. Fix: keep a Weak to the
File in the entry and purge on description-liveness, not fd-number resolution.

### [Medium] syscall/misc.rs:2731-2765 — epoll_pwait restores the signal mask on EINTR immediately (not deferred) — ✅ FIXED (`834f2fca`)
ppoll/pselect6 use `WaitScopedMask::defer_restore` so the interrupting signal — unblocked by the wait
mask — is actually delivered at return-to-user before the old mask is reinstated. epoll_pwait (and its
epoll_pwait2 sibling in file.rs — verify) does a plain save/restore around sys_epoll_wait: on EINTR the
mask is restored BEFORE delivery, so the pending signal is re-blocked and typically never delivered;
the caller sees EINTR with no handler run (glibc's epoll_pwait contract broken; LTP epoll_pwait01's
signal-race case is timing-lucky). Fix: use the same WaitScopedMask deferral.

### [Medium] syscall/misc.rs:4260-4455 — getrandom(2) is a time-seeded ChaCha PRNG; never blocks; GRND_* ignored
`ChaCha20Crng::new` seeds from CLINT time + jiffies + a boot counter + a stack address; `next_bytes`
nonces each block with the timer counter only. On a VM the seed is fully predictable to anyone who can
bound boot time (no interrupt/device entropy is mixed in, no reseeding). Linux getrandom blocks until
the CRNG is seeded; GRND_RANDOM/GRND_NONBLOCK/GRND_INSECURE semantics (block vs EAGAIN) are accepted
but ignored. Anything deriving secrets (tls keys, ASLR canary via AT_RANDOM if this feeds it) is
guessable. Fix: mix device/IRQ timing entropy into a pool with a seeded flag, block until ready
(GRND_INSECURE excepted).

### [Medium] syscall/misc.rs:591-731 — poll(2) ignores copy_from_user/copy_to_user results for the pollfd array — ✅ FIXED (`f4cbbd70`)
Both copies (in at :663, out at :717) discard the residual count. An in-range-but-unmapped array is
zero-filled on the way in (fds read as 0 → stdin is polled!) and the revents store silently vanishes on
the way out; Linux returns EFAULT (fault_in_pages_readable + copyout checks). Trigger: `poll(mmap-trimmed
page, 1, 0)`. Fix: propagate both copy results to -EFAULT (partial-out may keep the delivered count).

### [Low] syscall/misc.rs:2053-2057, 2141-2145 — epoll_ctl ADD/MOD ignore the event copy_from_user result — ✅ FIXED (`bcbd69f7`)
A faulting event pointer (access_ok-passed, unmapped) registers `events=0,data=0` (a watch that never
fires) instead of EFAULT. Fix: check the residual like the other paths here do.

### [Low] syscall/misc.rs:240/258-267 — always-on hunt prints on hot paths
- `epoll_notify_file` prints "epollnotify#..." via pr_warn for the first 40 notifies of every boot
  (then every 10th up to 400) with no debug switch — pure residue.
- `sys_timerfd_create` prints 60 TFDC lines per boot unconditionally.
- dispatch.rs's `EFAULTTRAC` (60 lines) and the `println!` on unknown syscalls are likewise ungated.
Fix: move behind the debug= switches catalog like the rest of the hunt taps.

### [Low] syscall/io.rs:438-501 — sys_write's fd-trace budget gate is dead code (512 as u8 == 0)
`if used < WTRACE_PID_MAX as u8` with `WTRACE_PID_MAX = 512` truncates to 0 — the branch never runs, so
the intended 512-line write trace never fires (harmless in production, but the diagnostic is broken and
the surrounding atomic bookkeeping still executes per call). Fix: compare in usize or store u16.

### [Info] syscall/misc.rs — timerfd precision is jiffy-quantized
timerfd_settime arms the jiffy wheel only (10 ms grid, msecs_to_jiffies().max(1)); the POSIX-timer path
has hres deadlines but timerfd does not. Sub-10ms timerfd users get 0-10ms error, and timerfd_gettime's
it_value is ms-rounded. Design gap, not a bug per se.

### [Info] syscall/io.rs:1213-1418 — sys_ioctl re-resolves the fd up to 10 times per call
The EBADF gate fetches `get_file_fd_safe`, then the O_PATH check, fb, DRM, prime, binder, access_token,
evdev, pty, VT, loop, watchdog, ashmem, tokenid blocks each fetch again (Arc clone + drop pair each).
One fetch at the top passed by reference would remove ~10 atomic inc/dec pairs per ioctl.

### [Medium] syscall/memory.rs:4522-4533 — get_mempolicy nodemask write exceeds the access_ok-validated length (Sept M6 still present) — ✅ FIXED (`4f76fab0`)
`access_ok(nodemask_ptr, (maxnode + 7) / 8)` validates ceil(maxnode/8) BYTES, then the loop writes
`nwords = ceil(maxnode/64)` usize WORDS (8 bytes each) of all-ones. For maxnode=65: 9 bytes validated,
16 written — a 7-byte overwrite of adjacent user memory past the caller's buffer (Linux masks the final
word to maxnode bits and copies min(maxnode, ...)). The Sept review recorded this as M6-fixed; the
current code still has the mismatch. Fix: write ceil(maxnode/8) bytes with the last word masked.

### [Low] syscall/memory.rs:4454-4472 — mlock2 rejects an unaligned address with EINVAL — ✅ FIXED (`bcbd69f7`)
Linux mlock2 rounds addr down / len up exactly like mlock (mlock2(man): "addr is rounded down"); Rux's
mlock2 gate `addr % PAGE_SIZE != 0 -> EINVAL` makes an unaligned mlock2 fail where Linux succeeds.
Fix: reuse mlock_impl's rounding, drop the alignment gate.

### [Low] syscall/memory.rs:4478-4551 — mbind/set_mempolicy accept every mode and flag silently
Linux validates mode (MPOL_DEFAULT..MPOL_LOCAL, else EINVAL), rejects MPOL_DEFAULT with a non-empty
nodemask, and limits flags to MPOL_MF_STRICT|MOVE|MOVE_ALL (+MPOL_F_NUMA in newer kernels); unknown
bits are EINVAL (flag-validation completeness). Rux only access_oks the nodemask and returns 0 for
anything. Trigger: `mbind(..., mode=99, flags=0xffff)` → Linux EINVAL, Rux 0.

### [Medium] net/socket.rs:631-680 — IPv6 TCP connect() ignores nonblocking and never waits for the handshake — ✅ FIXED (`00a52820`)
`Socket::connect` (v4) has the full machinery: EISCONN/EALREADY guards, loopback drain, blocking wait
to ESTABLISHED/RST/ETIMEDOUT, and EINPROGRESS for O_NONBLOCK. `connect6` sets Connecting, calls
`tcp_connect6` once, and then either marks the socket Connected on ret==0 (success reported while the
SYN may still be in flight — the "fake connection" shape the v4 side was fixed for) or drops to
Unconnected and returns the raw error. The `nonblock` argument the syscall layer computes is not even
passed. Trigger: nonblocking or blocking `connect()` on an AF_INET6 SOCK_STREAM socket. Fix: mirror
the v4 flow (wait rounds + EINPROGRESS + pending-error surfacing).

### [Low] syscall/network.rs:4033/4255 — sendmmsg/recvmmsg on AF_PACKET/AF_INET-raw sockets return ENOTSOCK — ✅ FIXED (`9bf06ed7`)
The batch entry points special-case AF_UNIX/AF_NETLINK through the per-message engines, then fall to
`socket_file_of_checked`, which only resolves the INET socket ops table — a raw/packet fd yields
ENOTSOCK although single sendmsg/recvmsg on the same fd work. Route raw fds through the per-message
syscalls like unix/netlink.

### [Low] syscall/network.rs:2022-2183 — SO_RCVTIMEO/SO_SNDTIMEO setsockopt accepts optlen 8 — ✅ FIXED (`9bf06ed7`)
Linux requires optlen >= sizeof(struct timeval) (16 on LP64) and returns EINVAL for 8; Rux copies 8
bytes and silently reads tv_usec as 0, arming a seconds-only timeout. (Also: the unix-socket twin
`read_tv` at :1979 accepts optlen 8 the same way.)

### [Info] syscall/network.rs:4281 — recvmmsg timeout conversion hardcodes 10ms jiffies — ✅ FIXED (`9bf06ed7`)
`deadline = jiffies + (us / 10_000).max(1)` assumes HZ=100; use `drivers::timer::HZ` like the other
deadline conversions or the timeout mis-scales if HZ ever changes.

### [Medium] syscall/process.rs:709-788 — execve argv/envp: 8KB per-string cap with silent truncation; unreadable strings silently drop the rest — ✅ FIXED (`8952fd96`)
`copy_argv_from_user`/`copy_envp_from_user`:
1. `strncpy_from_user(arg_ptr, MAX_ARG_STRLEN=8192, &mut buf)` TRUNCATES a longer string at 8 KB —
   `bash -c "<10KB script>"`, long java/python command lines exec with a silently shortened argument.
   Linux's limit is MAX_ARG_STRLEN = 128 KB and exceeding it is E2BIG, never truncation.
2. `Err(_) => break` on a faulted string read: exec proceeds with the arguments read SO FAR instead of
   EFAULT (Linux: count() faults → EFAULT). A mid-array bad pointer silently rewrites the program's
   command line.
Also 2048-entry array cap vs Linux's (2MB/8)-ish; minor. Fix: cap strings at 128 KB returning E2BIG,
and surface read faults as EFAULT.

### [Medium] syscall/process.rs:6838-6857 — clone3 CLONE_PIDFD never writes the pidfd out — ✅ FIXED (`5b93ce8e`)
The CLONE_PIDFD arm only probes the user word's PTE writability; the actual fd is never stored
("writing the actual fd stays future work"). Callers (systemd, libuv, Go runtime via clone3) then read
an uninitialized variable and use it as a pidfd — EBADF at best, wrong-process signal at worst.
Fix: alloc a pidfd in the parent and put_user it (the pidfd infrastructure already exists —
pidfd_open builds exactly this file).

### [Low] syscall/process.rs:863-884 — do_execve joins relative paths against the GLOBAL cwd — ✅ FIXED (`7006cc43`)
Every path syscall moved to `get_cwd_user()` (root-relative) so path_lookup's root prefixing applies
exactly once (file.rs resolve_user_path comment); do_execve still uses `get_cwd()`. Under chroot,
execve("bin/sh") resolves to <jail>/<global-cwd>/bin/sh. Fix: reuse resolve_user_path for the execve
wrapper too.

### [Low] syscall/process.rs:2607-2618 + 4041-4076 — getgroups and sysinfo swallow copy-out failures — ✅ FIXED (`bcbd69f7`)
getgroups calls copy_to_user without access_ok and ignores the residual (bad list pointer = success);
sysinfo ignores every field's put_user result. Linux returns EFAULT. Fix: check the copy results like
the neighboring syscalls do.

### [Low] syscall/process.rs:7124-7132 — faccessat2 ignores its flags argument — ✅ FIXED (`bcbd69f7`)
Delegates to faccessat with flags dropped: AT_SYMLINK_NOFOLLOW follows the symlink (Linux lstats the
link itself), and flag validation (unknown bits → EINVAL) is missing. Fix: validate flags and honor
NOFOLLOW in the lookup.

### [Low] syscall/file.rs:196-782 — openat/open does not validate unknown O_ flag bits — ✅ FIXED (`bcbd69f7`)
Linux build_open_flags rejects undefined flag bits with EINVAL (and O_ACCMODE==3); openat2 in this
tree does exactly that (file.rs:3979-4001) but the openat path passes any garbage flags through to
file_open. Trigger: `openat(..., flags=0x40000000)` succeeds instead of EINVAL. Fix: share openat2's
O_VALID_FLAGS check.

### [Low] syscall/file.rs:6289-6310 — fchmodat2 AT_EMPTY_PATH is a no-op stub — ✅ FIXED (`bcbd69f7`)
The `if flags & AT_EMPTY_PATH != 0 && args[1] != 0` block has an empty body; fchmodat2(fd, "",
mode, AT_EMPTY_PATH) falls through to sys_fchmodat → resolve_user_path("") → ENOENT, where Linux
(since 6.14) chmods the fd's file. Fix: route the empty-path form to fchmod_file_core like
fchownat does.

### [Low] syscall/file.rs:5674 — name_to_handle_at ignores one copy_to_user result — ✅ FIXED (`bcbd69f7`)
The handle_type store at handle_ptr+4 discards its residual; a fault there reports success with a
half-written handle. The neighboring payload copy is checked. Fix: check both.

### [Medium] ipc/sysv_shm.rs:1147-1178 — shmdt accepts an INTERIOR address and unmaps past the VMA end — ✅ FIXED (`4f76fab0`)
`vma_mgr.find(VirtAddr::new(shmaddr))` returns the VMA CONTAINING shmaddr, then the syscall munmaps
`[shmaddr, shmaddr + vma_size)` — for an interior address the range overruns the shm VMA's end by
(shmaddr - vma.start) into whatever mapping follows, destroying it. Linux shmdt requires the address to
equal the attach address exactly (ipc/shm.c: `vma->vm_start == addr`), else EINVAL. Trigger: `shmdt(base
+ PAGE_SIZE)` (LTP shmat01 probes wrong addresses). Sept M12 recorded this ("shmdt accepts any offset");
still live. Fix: `if vma.start() != shmaddr { EINVAL }`.

### [Low] ipc/sysv_sem.rs:1546-1549 — sem_undo_exit lacks the [0, SEMVMX] clamp — ✅ FIXED (`4f76fab0`)
Exit-time undo does a raw `fetch_sub(entry.adjustment)`: with a concurrent SETVAL/other-undo the result
can go negative (or above SEMVMX), and every later P on that semaphore blocks forever. Linux exit_sem
clamps `semval = clamp(semval, 0, SEMVMX)` after the subtraction (ipc/sem.c). Sept M5 recorded this;
still live. Fix: clamp after fetch_sub and wake waiters (wake is present).

### [Low] ipc/util.rs:491-504 — ipc permission check ignores supplementary groups
The group arm compares only `cred.egid == perm.gid`; a caller made member of the object's group via a
supplementary gid (setgroups) is denied (EACCES) where Linux's `in_group_p` grants. LTP as根 mostly
masks it. Fix: also test `cred.groups.contains(&perm.gid)`.

### [Low] ipc/sysv_sem.rs + sysv_msg.rs + sysv_shm.rs — IPC_STAT/SEM_STAT/INFO copy_to_user results ignored
Every `*ctl` stat path access_oks the buffer then discards copy_to_user's residual — an in-range but
unmapped page (PROT_NONE) yields "success" with nothing written (Linux: EFAULT). GETALL/SETVAL check
theirs; the struct paths do not. Fix: propagate the residual like GETALL does.

### [Info] ipc/posix_mq.rs — MQ descriptors live in a side namespace (documented)
mq fds are PID-keyed slots at 512-575: fork does not inherit, dup2/fcntl/poll cannot see them, and a
process whose fdtable grows past 512 gets regular fds colliding with MQ fds (mq syscalls resolve the MQ,
everything else the file — one number, two identities). O_NONBLOCK is also per-QUEUE
(apply_open_nonblock) rather than per-open-description, so a second open flips the first fd's blocking
mode. Known TODO (F12-34 comment); recorded because OH image processes with >512 fds will trip it.

### [High] ipc/binder.rs:9584-9611 + 9758 — BR staging buffer is 512 bytes with a SILENT drop; read_size > 512 lets the command stream corrupt
binder_thread_read stages into `staged_buf: [u8; 512]`; `Staged::extend` drops bytes silently when the
buffer is full, while the drain loop's only bound is the USER read_size (`staged.len() + 76 >
read_size`). With the default large read buffers (libbinder/OH pass read_size of tens of KB) a burst of
small work items (Work::Node ref transitions — a client dropping many proxies queues one Node item per
ref; several ReturnErrors; tcompletes) drives staged past 508 bytes: a command header (4B) fits while
its ptr/cookie payload (8+8B) does not — userspace then parses the next command's bytes as the cookie,
desynchronizing the whole BR stream. Linux stages directly into the user buffer bounded by read_size.
Fix: bound the stop_full check by `min(read_size, staged_buf.len())` (or make the buffer read_size-
bounded).

### [High] ipc/binder.rs:6721-6800 — handle-0 ref commands: ACQUIRE works by-node, RELEASE/DECREFS never match → permanent ref/node leak
BC_INCREFS/BC_ACQUIRE on handle 0 has a special arm that finds-or-creates the context-manager ref BY
NODE (created with desc = next_desc, not 0). The decrement path (BC_RELEASE/BC_DECREFS) has NO such
arm: it looks up `r.desc == desc(0)` only, always misses ("ref command on invalid handle 0"), and
skips. Every BpBinder(0) acquire/release cycle therefore leaks one strong ref on the mgr node and one
duplicate-ish ref entry; internal_strong_refs on the manager node grows monotonically and the
BINDER_WORK_NODE stream reports a ref state that never returns to zero. Fix: route handle-0 decrements
through the same by-node lookup (and ideally give the mgr ref desc 0 like Linux).

### [Medium] ipc/binder.rs:713-963 — silent-txn retirement and stale-buffer reclaim drop/rewrite live transactions
sweep_silent_txns retires any undelivered transaction older than 30 s (oneway) / 60 s (sync), failing
sync senders with BR_DEAD_REPLY and DROPPING oneways; sweep_stale_async frees receiver-side buffers
delivered >30 s ago even though the receiver may still be reading the parcel (its data can then be
reused under it). The Linux binder never fails a queued transaction on age. A legitimately slow server
(a service blocked 60 s in storage I/O) returns to find its queued calls failed and its buffers
recycled. Deliberate (OH leak mitigation, leg-17/leg-19) but a protocol divergence worth tracking as a
compat risk — a Linux workload with minute-scale IPC latency breaks. Fix direction: bound the grace by
receiver-thread liveness rather than wall age, or make it a mount/cmdline opt-in.

### [Medium] ipc/binder.rs:9392-9416 — binder nodes are never released while the owner lives — ✅ FIXED (`0843bb2a`)
translate_object creates a node per (ptr, reg_tgid) in the owner's `nodes` list; when the last external
ref drops (Work::Node → BR_RELEASE/BR_DECREFS read with zero external refs), nothing removes the node
— Linux releases the node (binder_put_node → deleted from proc->nodes) once internal+local refs and
refs are all zero. Rux keeps every node until the owner exits. A long-running service that registers
many temporary callback binders (OH pattern) accumulates nodes unboundedly. Fix: on the Work::Node
read-side transition to !weak && external==0, remove the node from owner nodes (keep the mgr node).

### [Low] ipc/binder.rs:1535-1537 — handle-0 transactions park up to 8 s waiting for a context manager
wait_for_context_manager (documented deliberate divergence): Linux answers BR_DEAD_REPLY immediately
when no manager is installed; Rux blocks the sending thread up to 8 s (250 ms slices). A Linux client
probing manager absence hangs 8 s per attempt instead of failing fast. Recorded as a compat
divergence; keep or bound lower.

### [Low] ipc/binder.rs:6797-6800 — invalid-handle BC ref commands are skipped silently
Linux queues BR_ERROR (binder_user_error path) for ref commands on an invalid handle; Rux logs
pr_info and continues — userspace cannot detect its own protocol bug. Fix: stage Work::ReturnError
(BR_ERROR, EINVAL) like the reference.

### [Medium] net/udp.rs:938-941 — SO_BROADCAST gate treats every destination ending in .255 as broadcast — ✅ FIXED (`f4cbbd70`)
`!socket.broadcast && (dest_ip == 0xFFFF_FFFF || (dest_ip & 0xFF) == 0xFF) → EACCES`. Linux requires
SO_BROADCAST only when the route classifies the destination as broadcast (limited broadcast or the
local subnet's directed broadcast). Any legitimate UNICAST host whose last octet is 255 (8.8.1.255,
1.2.3.255, ...) is refused EACCES unless the caller set SO_BROADCAST. Trigger: `sendto` a UDP peer
at x.x.x.255. Fix: gate on 255.255.255.255 plus the interface-derived subnet broadcast only.

### [Medium] io_uring/mod.rs:928-943 — CQE publish lacks a release barrier between the CQE store and the cq_tail store — ✅ FIXED (`f4cbbd70`)
io_uring_post_cqe writes the CQE, then cq_tail, then a fence(Release) AFTER both stores. On the
weakly-ordered RISC-V memory model (and under compiler reordering) the tail store can become visible
before the CQE store: userspace polling cq_tail sees a new entry and reads a stale/garbage CQE.
The reference (io_cqring_fill_event → io_cqring_ev_posted) uses smp_store_release on tail (or
smp_mb after). Fix: fence(Release) BEFORE the tail store (or an atomic release store), same for the
SQE ring's head write-back in submit_sqes:679.

### [Medium] io_uring/mod.rs:597-618 — ring mmap installs PTEs without PTE_MODIFY_LOCK — ✅ FIXED (`0843bb2a`)
binder_mmap_handler (and sys_mmap's device paths) take PTE_MODIFY_LOCK around every map_page and
tear down any racing demand-fault leaf — the NEW2 class (eager PTE install racing fork's COW walk).
io_uring_mmap_handler runs its map loop unlocked, only holding the VMA it just added. A fork (or a
CLONE_VM thread faulting through the half-built VMA) racing the loop diverges refcounts / leaves
dangling child PTEs exactly like the fixed binder cases. Fix: same pte_lock section as binder.rs:2662.

### [Low] io_uring/mod.rs:713-739 — READ/WRITE at an explicit offset implemented as set_pos/restore
io_uring_op_read with off != -1 saves file.get_pos(), set_pos(off), reads, restores — the exact
racy pattern removed from sys_pread64 (position corrupted under CLONE_FILES sharers; the syscall
layer uses position-invariant read_at). Two ring submits on one shared fd interleave positions.
Fix: route through read_at_slice/write_at_slice like syscall/io.rs does.

### [Low] io_uring/mod.rs:963-986 — GETEVENTS wait spins 1000 yields then invents ETIMEDOUT
wait_for_cqes loops yield_cpu() 1000 times and returns -ETIMEDOUT — Linux blocks interruptibly
until the completions exist (no timeout, EINTR on signal). A pure-wait enter (to_submit=0) with
min_complete above the ready count returns a bogus ETIMEDOUT instead of blocking. Fix: wait-queue
round (prepare_to_wait + recheck) like the socket waits.

### [Low] net/udp.rs:1518-1528 — always-on "udp-in" debug prints (marked "TEMP probe drop before merge")
udp_rcv prints the first 40 datagrams of every boot unconditionally ("lmbench-fix TEMP probe").
Merges with the misc.rs always-on taps finding. Fix: remove or gate.

### [Info] net/tcp.rs:3096-3098 + socket.rs:2532 — per-slot linear scans
tcp_accept/tcp_listen_has_pending_child/tcp_readable etc. scan the whole TCP table under the table
lock per call, and socket_close → SOCKET_TABLE.free scans the VFS socket Vec. Fine at the current
64/256 slot sizes; the design limit to note before any table growth.

### [Info] net/ipv6/mod.rs:454-501 — ipv6_rcv dispatches without verifying version/hop-limit or addressing
No version-nibble check (ethertype implies v6, benign), no hop-limit == 0 drop, and no destination
local/multicast filter — any v6 frame the NIC accepts is dispatched by port. Acceptable for the
single-homed QEMU NIC; noting as a design limit vs ipv6_rcv_core's checks.

### [Info] net/tcp.rs:3174,3343 — dead `wire_hdr` locals
handle_tcp_packet/handle_tcp_packet6 build a `wire_hdr` slice that is never used (option parsing
went through tcp_option_slice). Harmless residue.

### [Info] net/udp.rs:180-186 — receive budget is a fixed 128 KB, ignoring SO_RCVBUF
UDP_RCVBUF_BUDGET is a constant; setsockopt(SO_RCVBUF) is stored and reported but the enqueue gate
never consults it. A socket that raised its buffer still drops beyond 128 KB; one that lowered it
still queues up to 128 KB (Linux charges sk_rmem_alloc against sk_rcvbuf). Functional gap, low
impact at current traffic levels.

---

## Totals

| Severity | Count |
|---|---:|
| Critical | 0 |
| High | 6 |
| Medium | 15 |
| Low | 17 |
| Info | 9 |
| **Total** | **47** |

High: readv/writev iovec re-fetch TOCTOU; epoll_wait dead-interest purge on fd number;
binder BR staging 512-byte silent truncation; binder handle-0 RELEASE/DECREFS never match
(ref/node leak); execve argv 8 KB truncation + silent arg drop on fault; clone3 CLONE_PIDFD
never writes the fd.

## Three worst

1. **binder.rs:9584 read-path BR staging buffer** — 512-byte stack buffer with silently
   dropping `extend`, while the loop's bound is the USER's read_size. Any large read buffer
   plus a burst of small work items (a client releasing many proxies queues one Node item per
   ref) desynchronizes the BR command stream mid-payload — every binder process on the system
   misparses its reads. This is the highest-blast-radius defect found.
2. **io.rs readv/writev TOCTOU** — the exact double-fetch fixed for recvmsg (32fd359f) still
   lives in the two hottest vector syscalls: validate-then-refetch lets a racing CLONE_VM
   thread redirect the kernel's copies to unvalidated addresses (kernel memory disclosure /
   corruption).
3. **binder.rs:6721 handle-0 ref accounting** — every BpBinder(0) acquire/release cycle
   permanently leaks one strong ref on the context-manager node and one ref-table entry;
   long-running OH services monotonically inflate the manager node's refcount and the
   BINDER_WORK_NODE stream never settles to zero.

## Coverage appendix (all files read end to end unless noted)

### kernel/src/syscall (36,860 lines) — COMPLETE
mod.rs (errno table, FdSet), dispatch.rs (incl. all hunt hooks), x86_compat.rs (table checked
against x86_64 unistd_64.h facts: 34 hole, 37 alarm, 139 sysfs, 213/232 epoll, 78 getdents→64-bit
divergence noted as Info), signal.rs, sched.rs, time.rs (timers/itimers/adjtimex/robust_list),
io.rs (read/write/pread family, splice/tee/vmsplice/sendfile, ioctl dispatch incl. tty/FIONREAD),
misc.rs (poll/ppoll/pselect/select cores, epoll engine + wake registry + purge, eventfd/timerfd/
signalfd backends, getrandom CRNG), memory.rs (brk/mmap/munmap/mprotect/msync/mremap/madvise/
mincore/mlock*/mempolicy stubs + shm/fb/drm/io_uring/binder mmap arms + trimq), network.rs
(socket/bind/connect/accept/send*/recv*/mmsg/get/setsockopt), file.rs (openat fast paths, stat
family, xattr family incl. *xattrat, mount/umount/pivot, name_to_handle_at/open_by_handle_at,
copy_file_range, preadv2/pwritev2, renameat2, fchmodat2), process.rs (clone/clone3/execve ladder,
wait4/waitid, kill family, creds, pidfd family, seccomp gate, reboot, hwprobe).

### kernel/src/ipc (16,665 lines) — COMPLETE
sysv_sem.rs, sysv_msg.rs, sysv_shm.rs, posix_mq.rs, util.rs (IpcIds), access_token.rs,
binder.rs (10,425 lines, full: alloc engine, tx/rx loops, fixup/translate incl. FD/FDA/PTR,
death notifications, thread/proc reap, sweeps, stall audit, poll path, OH extensions).

### kernel/src/net (20,218 lines) — COMPLETE
tcp.rs (5,057, full state machine + table + timer arms), tcp_timer.rs, udp.rs, socket.rs,
unix.rs (core engines read line-level: send/recv/stream-user-read/connect/accept/shutdown/
close/setsockopt; the always-on hilog/socksnap tap blocks skimmed as instrumentation),
netlink.rs (rtnl exec, portid table, uevent broadcast; if_ioctl skimmed), raw.rs (packet/icmp
taps, send/recv, bind), ipv4/{mod,route,checksum,defrag}.rs, ipv6/{mod,icmpv6}.rs, ethernet.rs,
arp.rs, buffer.rs, mod.rs.

### kernel/src/io_uring (1,202 lines) — COMPLETE
mod.rs: setup/enter/register, mmap handler, SQE dispatch (NOP/READ/WRITE/FSYNC/CLOSE/FADVISE),
CQE posting, eventfd registration, lifecycle/pin discipline.

Dedup honored: review-coverage.md defect table and the Sept-2026 review items were skipped
except where the current tree still contains the defect (get_mempolicy nodemask OOB write,
shmdt interior address, sem_undo clamp — each marked "still live" in the finding).
