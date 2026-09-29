# Rux Kernel Linux-Comparison Full-File Review Report (2026-09-23)

**Review goal**: all 278 source files in the repository (including 3 .S assembly files) reviewed file by file, function by function, data structure by data structure, line by line; semantic differences compared against Linux as the baseline; focus on POSIX/ABI compatibility (goal: statically compiled musl Linux software runs directly).

**Review conventions**: each finding's "initial verdict" is annotated with one of three classes —
- `Design inconsistency`: deliberate simplification/trade-off, self-consistent semantics (for user review to accept or not)
- `Suspected bug`: differs from Linux and causes incorrect behavior
- `Pending review`: cannot be adjudicated unilaterally

**Categories**: LINUX-DIFF (semantic difference) | ABI (POSIX/ABI) | BUG | OVERFLOW | RACE | TIMING | VISIBILITY | COMMENT | LICENSE | ARCH

**Statistics**: 278 files / 116,099 lines / ~1,590 function symbols / **446 findings** (18 P0-P1 theme groups, see the master review summary).

# Batch 3: kernel/src/process/ — Linux-comparison full-file review

Scope: task.rs(2840), fork.rs(502), exec.rs(787), exit.rs(809), wait.rs(356), pid.rs(160), pid_hash.rs(262), kthread.rs(231), mod.rs(62), 6,009 lines total. Corroborating cross-references: syscall/process.rs, syscall/signal.rs, signal.rs, sync/futex.rs, fs/elf.rs, config.rs.

## 3.1 fork.rs (clone semantics)

Function list: `CloneArgs` definition; `do_fork`; `copy_thread`; `do_clone`.

**Findings**
- [ABI][P1] fork.rs:238-257 + futex.rs:48-70 — Current state: the exit path does have `put_user(0)`+`futex_wake` (formally complete), but `FutexKey::matches` requires `pid` equality for private futexes, and the key's pid is taken from `current.pid()` (different per thread); Linux: the private futex key is the mm pointer (shared by threads of the same CLONE_VM). Impact: a join waiter's (another thread) wait key never matches the exiting thread's wake key → musl `pthread_join` sleeps forever; every private futex between threads of the same process (mutex contention/cond) likewise breaks. The heaviest ABI break of this batch (the sync batch should re-review the key definition).
- [ABI][P1] fork.rs:394-400 — CLONE_THREAD only does `set_tgid`, with no thread_group chain/group_leader/nr_threads, and sys_kill with a positive pid only targets one; Linux's kill(tgid) iterates the thread group. Impact: process-directed signals of multithreaded programs (SIGINT/SIGTERM) hit only one thread; combined with the missing exit_group (3.4), multithreaded musl programs are wholly unusable.
- [Semantics][High] fork.rs:150-157 — CLONE_CHILD_SETTID/CLONE_PARENT_SETTID written into the parent's address space after the mm copy (Linux in the child context); EFAULT ignored.
- [Semantics][Medium] fork.rs:188-201 — Illegal flag combinations return None → a uniform -ENOMEM (Linux -EINVAL); PID exhaustion gives ENOMEM instead of EAGAIN.
- [Semantics][Medium] The child task does not inherit comm, nice, policy, rt_priority, cpus_allowed, oom_score_adj, sigaltstack.
- [Semantics][Medium] fork.rs:255-267 — With CLONE_FILES and the parent having no fdtable, the child creates a new table + std fds, violating the sharing semantics (an edge case).
- [Semantics][Low] sigmask copied twice; CLONE_VFORK approximated with UNINTERRUPTIBLE + manual schedule; CLONE_PARENT unimplemented; the CSIGNAL low 8 bits ignored (happens to be equivalent since it's always SIGCHLD).
- [Correctness][Low] fork.rs:226 `add_child` before copy_thread: TASK_NEW guards against concurrent wake (positive), do_wait can observe a half-built child (not a zombie, harmless).

**Positives**: the up-front flag validation matches Linux; the failure path fully unwinds; the vfork double race fixed; the CLONE_SETTLS timing correct.

## 3.2 exec.rs (execve vs load_elf_binary)

Function list: `do_execve_elf` (including UserAddrSpaceGuard, auxv/stack construction, PTE tightening, interpreter loading, VMA registration).

**Findings**
- [ABI][High] exec.rs:170-172 + 629-644 + signal.rs:1096-1099 — The signal-return ra points to a 2-instruction trampoline **on the user stack**, with sa_restorer stored but unused; the stack PTE has no X and the stack VMA is RW only, page_fault rejects EXEC by VMA. Linux: rt_sigaction mandates SA_RESTORER, returning via the libc __restore_rt (in the text segment). Impact: musl signal handlers return by jumping to non-executable code on the stack → SIGSEGV; the W^X and "kernel trampoline" designs contradict (recommend honoring sa_restorer).
- [ABI][High] syscall/process.rs:110/141 — argv capped at 65 entries/1024B, envp 257/4096B, silently truncated beyond, non-UTF-8 dropped; Linux 131072 entries/32 pages per single string/RLIMIT_STACK/4 total, E2BIG beyond. Impact: shells/python with slightly larger env break.
- [ABI][High] exec.rs whole file — Multithreaded exec has no de_thread: sibling threads keep running the old image but the shared fdtable was already close_cloexec'd and the SignalStruct flushed. Impact: a threaded process's exec semantics undefined afterward.
- [Semantics][Medium] exec.rs:55-81 — The cloexec/handler/pending cleanup happens before the image loads successfully, with no rollback on failure (Linux commits only after the point of no return).
- [Semantics][Medium] exec.rs:574-590 — The auxv's 15 pairs are self-consistent but **missing AT_PLATFORM(15)**; AT_HWCAP=0 with no ISA probing; AT_CLKTCK=100 hardcoded, a second source against config's HZ.
- [Correctness][Medium] exec.rs:474-475 — Without PT_PHDR, AT_PHDR falls back to the stack copy (musl ld.so derives the bias as a stack address); Linux uses load_bias+e_phoff.
- [Security][Low] The interpreter base fixed at 0x3FBF000000, PIE bias 0, no ASLR.
- [Correctness][Low] PTE tightening for same-page straddling RX/RW segments is "last writer wins"; the phdr-table copy isn't self-protected (pub(crate) reuse is risky).
- [Design][Low] The whole file read in + all physical pages pre-allocated (no demand paging); itimer/posix_timers not reset across exec.

**Positives**: BSS zeroing, segment bounds validation, the RAII guard, tp=0 left for musl TLS reinitialization, the credential snapshot rollback, the vfork wakeup at the end of the success path.

## 3.3 exit.rs + wait semantics

Function list: `release_task`; `reparent_children_to_init`; `do_exit`; `do_wait`; `do_wait_nonblock`; `write_siginfo`; `do_waitid`; the W*/P_*/CLD_* constants.

- [ABI][P1] dispatch.rs:142-143 — exit(93) and exit_group(94) both mapped to sys_exit→do_exit: only the calling thread terminates, no thread-group kill (Linux zap_other_threads). After a multithreaded process's exit_group, the other threads survive.
- [Semantics][High] exit.rs:370-372/535-537 — wait4/waitpid's pid==0 (same pgid) and pid<-1 (pgid==-pid) are always treated as "any child"; Linux has the three-way semantics. Shell job control reaps the wrong process.
- [Semantics][High] exit.rs:752-754 — waitid+WNOHANG with no event returns -EAGAIN; Linux/POSIX return 0 and leave infop untouched.
- [Semantics][Medium] do_wait_nonblock ignores options (WUNTRACED ineffective); wait4 without WCONTINUED; unknown options don't report EINVAL.
- [Semantics][Medium] wait4's rusage entirely ignored; no RUSAGE_CHILDREN accumulation (bash time/make statistics are 0).
- [Semantics][Medium] Reparenting only finds init(1), never checking PR_SET_CHILD_SUBREAPER (prctl stores but never uses it); pdeath_signal stored but never sent.
- [Concurrency][Medium] exit.rs:437 vs 743 — wait4 uses stop_reported, waitid uses stop_signal()!=0; the two one-shot report mechanisms don't exclude each other (a single stop event double-reported).
- [Correctness][Low] No WCOREDUMP bit in the signal-death wstatus; a status write failure ignored (inconsistent with the nonblock version).
- [Cleanup][Low] exit.rs:64-74 — Freeing the heap PtRegs is legacy dead code of the old fork path (two copy_thread implementations coexisting is a hazard).

**Positives**: the wstatus bit layout matches the musl macros; the release_task ordering correct; the zombie reparenting sends a supplementary SIGCHLD + directly wakes init; do_waitid's filter-consistency re-check.

## 3.4 wait.rs (wait-queue infrastructure)

- [Semantics][Low] wake_up ignores the mode filter (backstopped by wake_up_process); nr_exclusive approximated.
- [Design][Low] Vec head-insert/retain is O(n) (Linux's doubly linked list is O(1)); an API trap of copying in by value on add.
- Positives: R12-1 wakeup inside the lock; prepare_to_wait sets state + enqueues under the same lock; both macros' re-check + dequeue discipline.

## 3.5 pid.rs

- [Semantics][Low] PID exhaustion gives ENOMEM (Linux EAGAIN); PID_MAX_LIMIT(4M) exported but unused; the cyclic scan + cursor matches Linux (positive).

## 3.6 pid_hash.rs

- [Concurrency][High] pid_hash_lookup returns a raw pointer after rcu_read_unlock — the 56 unpinned call sites can have the object freed by a concurrent reap during the post-unlock dereference → a UAF window (the pinned version exists but isn't universally adopted).
- [Concurrency][Medium] pid_hash_remove's *prev = next is a plain write (Linux rcu_assign_pointer).
- [Semantics][Low] LIFO order (procfs listing unstable); collect_all truncates at a fixed 64 entries.

## 3.7 kthread.rs

- [Resource][Medium] A kernel thread exiting on its own has nobody cleaning KTHREAD_MAP → entry leak + PID-reuse mismatch.
- [Semantics][Low] kthread_stop busy-wait polls (Linux completion); the comment says the name is stored but it isn't; kthread_bind only cpu<32.

## 3.8 task.rs (PCB)

Function list (grouped): the StackCache family; the TaskState family; SchedPolicy/TaskFlags/Cred; Task::new/new_idle_at/new_task_at; sleep/wake_up; the time-slice family; the scheduler-accessor family; the process-tree family (add_child/remove_child/for_each_child etc.); the address-space family; the vfork family; the thread_info group (ti_flags/preempt_count/ti_kernel_sp/ti_cpu/on_cpu etc. kernel-side); the kernel-stack family; misc (oom/fdtable/signal/exit_code/stop_signal/comm/pgid/pending/clear_child_tid/robust_list/brk/cwd/umask/exe_path etc.); the HZ constants, task_offsets, get_current_fdtable.

- [Cleanup][Low] In stack_cache_alloc, the second unsafe block after the return is unreachable dead code.
- [Comment][Low] "32KB" is actually 64KB; "O(log N)" is actually hash O(1).
- [Consistency][Low] new_task_at writes time_slice=HZ(100), Task::new uses TIME_SLICE_TICKS=10: an inconsistent dual source.
- [Concurrency][Medium] add_child's double-link tripwire only prints without preventing; the old parent's list is left dangling (no defensive effect).
- [Consistency][Low] new_idle_at writes an AtomicU64 directly via *mut u64; Task::new and new_task_at on dual tracks.
- [Security][Low] A new task is born with Cred::new_init() (FULL caps) relying on subsequent overwriting: a missed copy is an escalation — a fragile pattern.
- [Structure][Info] Missing exit_signal, thread_group/group_leader, real rlimits, namespaces, io_context — the groundwork gaps for multithreading/container semantics.

## 3.9 mod.rs

- [Semantics][Low] find_task_by_pid returns &'static mut (aliasing-UB risk, the kernel-wide established style); the O(log N) comment wrong.

## 3.10 Corroborating snapshot (syscall integration)

sys_clone's parameter order correct, errors uniformly ENOMEM; sys_set_tid_address semantics correct; sys_rt_sigaction per the RISC-V ABI (restorer@16) correctly round-trips sa_restorer but the delivery path doesn't use it (3.2); sys_wait4 drops options on WNOHANG, -11 hardcoded; sys_kill's permissions approximately in place but a positive pid has no thread-group propagation (3.1).

## Batch 3 statistics

| Level | Count | Items |
|---|---|---|
| P1 | 3 | The CLEARTID futex key break (join sleeps forever); no thread-group signal propagation; exit_group doesn't kill the thread group |
| High | 5 | sa_restorer ignored + the stack trampoline conflicting with W^X; argv/envp truncation; multithreaded exec without de_thread; waitpid pid==0/<-1 semantics wrong; waitid WNOHANG returning -EAGAIN |
| Medium | 11 | SETTID written into the parent's space; EINVAL/ENOMEM confusion; scheduling attributes not inherited; the CLONE_FILES edge; the pre-exec destruction; auxv missing AT_PLATFORM; the AT_PHDR fallback; WUNTRACED×WNOHANG; rusage ignored; subreaper/PDEATHSIG; double stop reporting; pid_hash non-atomic unlink; the kthread map leak; add_child without defense |
| Low | 14 | Dead code, comment drift, the time_slice dual source, WCOREDUMP, EFAULT inconsistency, PID EAGAIN, dual-track construction, cred full powers, the procfs 64 truncation, LIFO order, the O(n) queue, the API trap, no ASLR, last-writer-wins |

**Conclusion**: the fork/exec/wait main chain is usable for single-threaded static musl programs and aligns with Linux in many places; but the **three thread-ABI breaks** (the private futex key using tid, no thread-group signal propagation, exit_group being a no-op) and the **sa_restorer/W^X contradiction in signal return** are mandatory obstacles before running real musl dynamic programs.


# Batch 1: syscall layer Linux-comparison review report

Baseline: Linux riscv64 (the asm-generic syscall table / the musl ABI), reviewed file by file, function by function, in full. 11 files, ~372 functions/methods total.

**Overall architectural findings (affecting multiple files)**
- [ARCH][Suspected bug] trap.rs:135 globally sets SUM=1 via enable_external_interrupt() — Linux sets SUM only inside copy windows. Consequence: large amounts of "raw user-pointer access" code in the syscall layer work under SUM=1 but bypass the exception table: a bad pointer triggers a kernel-mode fault → panic instead of EFAULT.
- [LINUX-DIFF][Design inconsistency] The return convention is correct, but resolve_user_path/do_execve using u64 to pass negative errnos and similar sign hacks are fragile.

## 1. dispatch.rs
- [ABI][Suspected bug] dispatch.rs:296 — Number 240 maps sys_perf_event_open; in Linux asm-generic, **240=rt_tgsigqueueinfo, 241=perf_event_open**. Wrong number + rt_tgsigqueueinfo missing.
- [LINUX-DIFF][Design inconsistency] All the musl-startup-required numbers are present and correct in the table (except 240).
- [COMMENT][Pending review] Module attribution misplaced (rseq/fanotify in time.rs etc.).
- [COMMENT][Pending review] exit(93)/exit_group(94) share sys_exit.

## 2. mod.rs
- [COMMENT][Suspected bug] mod.rs:238-240 — Select=280/Pselect6=281/Eventfd=290 are all wrong (280=bpf, 281=execveat, 290=pkey_free); 249 should be Dup3; 156-159 values wrong. The enum is a contamination source.
- [LINUX-DIFF][Pending review] errno values checked error-free; missing ENOLCK(37)/EPROTO(71)/EOVERFLOW(75)/ECANCELED(125)/EOWNERDEAD(130)/ENOTRECOVERABLE(131) and other common ones.
- [LINUX-DIFF][Pending review] FdSet 1024 bits ✓ but FD_SETSIZE taken from config (silently inconsistent if ≠1024).

## 3. file.rs (100 functions, list omitted, see git history)
- [ABI][Suspected bug] file.rs:492 — /proc readlink with insufficient bufsiz returns ENAMETOOLONG; Linux truncates and returns the length (never ENAMETOOLONG).
- [ABI][Suspected bug] file.rs:1501-1509 — mknodat with ftype==0 returns EINVAL; Linux treats it as a regular file.
- [ABI][Suspected bug] file.rs:1998-2031 — futex2's FUTEX2_PRIVATE uses bit0; Linux = FUTEX_PRIVATE_FLAG = 0x80. The standard flag determination inverted.
- [BUG][Suspected bug] file.rs:2012-2030 — futex_wait(455) passes a relative timeout through to the absolute-semantics WAIT_BITSET; the mask ignored.
- [LINUX-DIFF][Design inconsistency] utimensat an empty shell (doesn't read times, doesn't update); renameat2 ignores flags (NOREPLACE overwrites); statx ignores flags/attributes.
- [BUG][Suspected bug] file.rs:1183-1214 — statfs doesn't check path existence (any path returns fake data).
- [BUG][Suspected bug] file.rs:1807-1846 — copy_file_range ignores the off pointers; returns 0 false success on first failure.
- [LINUX-DIFF][Pending review] linkat/unlinkat/fchmodat etc. flags not validated; openat2 size>24 should be E2BIG; faccessat uses euid (Linux without AT_EACCESS uses the real uid); fallocate false success; mount ignores parameters; fsync a stub.
- [RACE][Pending review] dirfd-relative paths based on a path-string snapshot (semantics drift after a rename).

## 4. io.rs
- [OVERFLOW][Suspected bug] io.rs:386/460/951/1013 — iovcnt without an IOV_MAX(1024) check and size_of×iovcnt can overflow.
- [BUG][Suspected bug] io.rs:1179-1212/1289/1326 — splice/sendfile raw-dereference the user off pointer; splice changes the fd position without restoring it.
- [RACE][Suspected bug] io.rs:184-227/885-927 — pread/pwrite simulated with get_pos/set_pos: a multithreaded shared fd position gets corrupted (Linux is atomic).
- [LINUX-DIFF][Pending review] readv/writev per-iov non-atomic; the ioctl 0x5400 family always 0 (FIONBIO false success, TCSETS lost); flock always succeeds; write to fd1/2 bypasses the file layer and writes MMIO directly.

## 5. memory.rs
- [BUG][Suspected bug] memory.rs:1330/1489/1501 — mincore/get_mempolicy raw-write user pointers.
- [ARCH][Suspected bug] memory.rs:29-131 — sys_brk has no upper-bound check on new_brk (USER_END): mapping a U page in the kernel region is an escalation surface.
- [LINUX-DIFF][Suspected bug] memory.rs:1160-1167 — MADV_DONTNEED/FREE don't free pages (glibc/jemalloc heap shrinking ineffective).
- [LINUX-DIFF][Pending review] munmap of an unmapped range gives EINVAL (Linux 0); PROT_EXEC mmap simplified to RWX (W^X defeated); fd>=1000 treated as framebuffer (a legal fd wrongly ENXIO); mremap restrictions; mprotect on unmapped returns 0; mincore overflow.

## 6. misc.rs
- [ABI][Suspected bug] misc.rs:348-356 — The pselect6 timeout parsed as timeval; **Linux uses timespec** — the magnitude 1000× too large.
- [LINUX-DIFF][Suspected bug] misc.rs:132-145 — poll(NULL,0,ms), a legal sleep idiom, rejected; nfds>1024 EINVAL (Linux unlimited).
- [ARCH][Suspected bug] misc.rs:1504-1538 — getrandom uses CLINT+LCG: not a CSPRNG (ssh/tls depend on it).
- [LINUX-DIFF][Pending review] epoll ERR/HUP masked out by the filter; epoll entries not deleted after close; sigmask entirely ignored; eventfd/timerfd blocking semantics missing (an EAGAIN busy-loop).

## 7. process.rs
- [ABI][Suspected bug] process.rs:30-32 — **clone's args[3]=tls, args[4]=child_tid; Linux riscv64 has a3=child_tidptr, a4=tls** — pthread creation is misaligned from the start.
- [ABI][Suspected bug] process.rs:1637-1641 — rt_sigqueueinfo parsed as 4 arguments; **Linux uses 3 (tgid,sig,uinfo)**.
- [BUG][Suspected bug] process.rs:2453 — getrusage writes 136B; Linux's rusage=144B.
- [BUG][Suspected bug] process.rs:2815-2838/2747 — close_range/riscv_hwprobe return count; Linux returns 0.
- [BUG][Suspected bug] signal.rs:546-549 — sys_tkill's Err double-negated and returned positive.
- [LINUX-DIFF][Design inconsistency] prlimit64 only recognizes NOFILE; setrlimit silently accepted; the execve pathname limited to 256B (PATH_MAX 4096).
- [LINUX-DIFF][Pending review] gettid returns pid; tgkill ignores tgid; getgroups EFAULT lost; setpgid an empty block; sysinfo raw-write + fake data; setuid clears caps.

## 8. sched.rs
- [LINUX-DIFF][Pending review] getpriority PRIO_PGRP/USER EINVAL; **renice raising priority without CAP_SYS_NICE**; sched_setaffinity doesn't store; sched_setattr doesn't truncate the read by size.

## 9. signal.rs
- [BUG][Suspected bug] signal.rs:332 — sigpending a raw write.
- [LINUX-DIFF][Pending review] rt_sigprocmask's alignment check (Linux doesn't require it); sa_mask strips KILL/STOP; sigaltstack raw read/write; restart_syscall always 0; signalfd4 ENOSYS.
- Positive: the SigActionUser 32B layout matches asm-generic; the sigsuspend mask handoff correct.

## 10. time.rs
- [ABI][Suspected bug] time.rs:553-601 — clock_nanosleep returns a negative errno; **Linux exceptionally returns a positive errno**.
- [LINUX-DIFF][Design inconsistency] clock_gettime REALTIME==MONOTONIC (no wall clock); CPUTIME always 0.
- [BUG][Suspected bug] time.rs:652/899 — The POSIX timer uses Vec::remove — deleting a middle timer misaligns ids.
- [LINUX-DIFF][Pending review] getitimer/setitimer always 0; nanosleep negative values not EINVAL.

## 11. network.rs
- [BUG][Suspected bug] network.rs:879-886 — sendmsg reads msg_name but discards it: **UDP sendmsg always sends a NULL destination**.
- [BUG][Suspected bug] network.rs:948 — recvmsg discards the source address.
- [BUG][Suspected bug] network.rs:387-397 — getsockname false success on a non-socket fd (getpeername correctly ENOTSOCK).
- [BUG][Suspected bug] network.rs:1085-1163 — sendmmsg/recvmmsg return 0 on first failure (in the Linux ABI 0=no messages).
- [LINUX-DIFF][Design inconsistency] setsockopt entirely ignored (**SO_RCVTIMEO ineffective → timeout network programs block forever**); getsockopt SO_ERROR always 0 (non-blocking connect probing broken).
- [LINUX-DIFF][Pending review] bind/connect ignore addrlen; AF_UNIX EAFNOSUPPORT; accept doesn't write addr; accept4 ignores CLOEXEC; MSG_PEEK/DONTWAIT missing; SHUT_RD a no-op.

## Batch 1 statistics
11 files/~372 functions/14,197 lines; **78 findings** (LINUX-DIFF 44/ABI 8/BUG 13/OVERFLOW 3/RACE 3/ARCH 3/COMMENT 12). Licenses present in 11/11.
**Top 10 fix priorities**: (1) clone a3/a4 swapped (2) rt_sigqueueinfo 3 args (3) FUTEX2_PRIVATE=0x80 (4) pselect6 timespec (5) number 240 misplaced (6) futex_wait relative timeout (7) sendmsg/recvmsg msg_name (8) getrandom LCG (9) tkill double negation (10) rusage 144B.

---

# Batch 2: arch/riscv64 (including all assembly) Linux-comparison review report

Baseline: Linux riscv64. Scope: 24 files ~10.2k lines, the three assembly files instruction by instruction. Format: [category][initial verdict] file:line — current state; Linux; impact.

## 2.1 trap.S (vs entry.S)
21 symbols reviewed (all of trap_entry's labels/macros/constants).
- [OK] The save set matches Linux's handle_exception (31 GPRs + sstatus/sepc/stval/scause); the sscratch protocol equivalent.
- [DESIGN][Medium] trap.S:234-236 — Kernel-mode traps don't switch to the IRQ stack (the 16KB×4 IRQ stacks actually idle); Linux switches to a per-CPU hardirq stack.
- [Defect][Low] .Learly_boot doesn't restore tp from sscratch (a defensive path); boot.S:389's .Lsecondary_hang comment contradicts the behavior.
- [RISK][Low] .Lrestore_and_exit writes sstatus before restoring the GPRs (relies on the PT_STATUS.SIE=0 premise); sc.d clearing LR is nonstandard.
- [CONS][Low] TASK_TI_* offsets hardcoded and maintained in duplicate (no asm-offsets generation).

## 2.2 uaccess.S / uaccess.rs
16 symbols reviewed.
- [OK] The SUM ritual matches old Linux __copy_user; the extable 16B entry used the same way.
- [SEC][Medium] uaccess.rs:230-243 — copy_from_user doesn't zero the tail buffer on failure (Linux memsets 0) — a kernel information-leak surface.
- [Defect][Low] strncpy_from_user wrongly EFAULTs on an empty string; the shift-copy tail word can over-read the source by 7B (false positives).
- [CONS][Low] The extable linearly scanned (Linux binary search); no MAX_RW_COUNT.

## 2.3 boot.S / smp.rs
18 symbols reviewed.
- [DESIGN][Low] Enabling the MMU uses the instruction-fetch-fault jump trick (Linux uses a fixmap trampoline); the fixed 8MB early-mapping window assumption.
- [DOC][Low] The 0xC7 comment wrongly includes the G bit (there is actually no G).
- [CONS][Low] Device PTEs don't use the SVPBMT IO bits (a real-hardware risk).
- [OK] The secondary-core SBI HSM protocol correct; tp preloaded with idle; the Acquire/Release ordering pairs correct.

## 2.4 context.rs / thread.rs
14 symbols reviewed.
- [PERF][High] context.rs:203-234 — switch_mm always ASID=0 + duplicated full sfence.vma; **asid.rs's full allocator suite implemented but with zero consumers (a dead mechanism)**; kernel-thread switches flush twice more (Linux lazy TLB).
- [SEC][Medium] thread.rs:163-184 — After execve, during FS=OFF, the FP registers persist across processes (fpu_init has no callers) — information leak + numeric pollution.
- [OK] __switch_to's save set/SUM clear ordering/on_cpu release ordering correct.

## 2.5 trap.rs / pt_regs.rs / process.rs
30+ symbols reviewed.
- [OK] **The pt_regs layout byte-for-byte identical to the Linux uapi** (0x00-0x118); the syscall restart semantics consistent.
- [SEC][Medium] trap.rs:124-142 — enable_external_interrupt sneaks in csrs SUM → the kernel's steady-state SUM=1 (defense in depth lost).
- [RISK][Low] Every kernel trap reads the instruction to detect WFI (a QEMU workaround); handle_syscall's compressed determination raw-reads the user epc without an extable.
- [DESIGN][Low] KERNPANIC's dead wfi doesn't stop the other CPUs (a locked-up deadlock); SR-PROBE diagnostics left in the production path.
- [Defect][Low] process.rs:87-158's arch copy_thread dead code carries hazards (the live path is the fork.rs version).

## 2.6 cpu.rs / mod.rs / ipi.rs / linker.ld
30+ symbols reviewed.
- [Defect][Medium] ipi.rs:85-101 — send_ipi_type's "already set, don't resend" loses an IPI race → smp_call_function deadlock (Linux sends unconditionally).
- [DESIGN][Low] cpu_id implemented in three places (mod.rs hardcodes 0x18, dual maintenance).
- [OK] save_and_disable_irq uses atomic csrrci; the linker layout self-consistent.

## 2.7 mm/
60+ symbols reviewed.
- [PERF][High] mmu_init.rs:412-462 — map_page does an unconditional full sfence.vma per page (2GB at boot = 1024 of them; Linux aggregates via mmu_gather).
- [DESIGN][Medium] mm_ops.rs:921 — PTE_MODIFY_LOCK, a global singleton, serializes all mm's PTE writes + a long interrupts-off window (Linux per-mm lock + per-PTL).
- [CONS][Medium] mm_ops.rs:669-695 — perm_to_flags(None)→V|R|A|D: **an mmap(PROT_NONE) anonymous page is actually readable**; mprotect's PROT_NONE however is correct — a semantic split within the same kernel.
- [OK] The Sv39 layout constants fully isomorphic to Linux; the COW-fork locking protocol correct; the page_fault skeleton corresponds to Linux.
- [RISK][Low] virt_to_phys returns as-is outside the linear region; asid.rs/TRAP_STACKS/__switch_mm_linear all dead code.

## Batch 2 statistics
24/24 files, 214 symbols; **36 findings** (defects 6/security 3/performance 2 High/design 4/consistency 5/risk 7/docs 1/dead code 4); 13 confirmed correct (pt_regs byte-for-byte identical etc.).
**Top risks**: (1) no ASID + full sfence per page (a double-High performance issue) (2) copy_from_user not zeroing (security) (3) FP persisting across processes (security) (4) SUM always on (5) the global PTE lock (6) ipi losing IPIs (deadlock).


# Batch 4: kernel/src/mm/ — Linux-comparison full-file review

Scope: 25 .rs files, ~10,618 lines. Method: reading each file through + grep cross-validation of key suspicions.

## 4.0 Statistics

| Category | Design inconsistency | Suspected bug | Pending review | Subtotal |
|---|---|---|---|---|
| LINUX-DIFF | 21 | 2 | 13 | 36 |
| BUG | 1 | 5 | 6 | 12 |
| RACE | 0 | 5 | 6 | 11 |
| TIMING | 1 | 1 | 4 | 6 |
| OVERFLOW | 0 | 0 | 3 | 3 |
| COMMENT | 0 | 1 | 14 | 15 |
| ARCH | 1 | 0 | 1 | 2 |
| **Total** | **24** | **14** | **47** | **85** |

**Three systemic threads**:
- S1 global &mut aliasing UB: pglist.rs's NODE_DATA (static mut), via first_online_node_mut(), takes a &'static mut on every alloc/free/lru/vmscan/kswapd — on 4 CPUs, multiple exclusive references alive concurrently (page_alloc.rs:42/135, lru.rs, vmscan.rs:89, kswapd.rs:103/152).
- S2 allocation has no slow path: on alloc_pages failure, only an async wake of kswapd + a single compact pass before returning 0; try_to_free_pages/is_memory_low/should_trigger_oom all have zero callers — Linux's __alloc_pages_slowpath chain of retry/watermark wait/direct reclaim/OOM is wholesale missing.
- S3 the per-CPU pageset not wired: the pcp.rs module has zero live callers; all allocations hit the zone's global lock directly.

## 4.1 vma.rs (829 lines, 40+ functions reviewed)
- [BUG][Suspected bug] vma.rs:627-660 — expand_downwards only checks the predecessor for overlap, not whether VMAs already exist within (new_start, vma_start): with mappings below the stack, expanding the stack directly overlaps them.
- [LINUX-DIFF][Design inconsistency] can_merge requires the offsets to be fully equal (Linux's anonymous case only requires contiguity); add merges on one side only (Linux chains three ways).
- [LINUX-DIFF][Design inconsistency] find_stack_vma is a linear scan + the expansion window only 1 page (Linux allows anything lower within rlimit + a guard gap).
- [COMMENT][Pending review] count AtomicU32 dead-code drift; SHARED/PRIVATE dual bits without validation; stale comments.

## 4.2 mm_struct.rs (794 lines, 60+ functions reviewed)
- [RACE][Suspected bug] alloc_asid check-then-set non-atomic (an ASID-pool leak).
- [RACE][Pending review] update_highest_vm_end non-atomic.
- [COMMENT][Pending review] mm_users_dec underflow only warns, can go negative.
- [LINUX-DIFF] OOM_SCORE_ADJ modeled as a flag bit (Linux a 10-bit field); Drop doesn't iterate the VMAs; statistics loose without snapshot consistency.

## 4.3 page.rs/pagemap.rs/allocator.rs
- [OVERFLOW][Pending review] ceil() overflows near MAX; new silently truncates.
- [LINUX-DIFF] VmaError::Overlap→AlreadyMapped (without FIXED, should relocate the address); the Perm enum has no correspondence to the PTE bits.

## 4.4 page_desc.rs (897 lines, 50+ functions reviewed)
- [BUG][Suspected bug] init_mem_map zeroes out-of-range pages with _mapcount 0 (the bias convention is equivalent to "already mapped once").
- [LINUX-DIFF] No compound-page metadata: a tail page's refcount=1 (Linux always 0, managed by the head) — putting a tail page triggers a wrong free.
- [RACE][Pending review] flags/refcount ordering conventions scattered, with no centralized documentation.

## 4.5 zone.rs (917 lines, 40+ functions reviewed)
- [BUG][Suspected bug] zone.rs:561-584 — **is_buddy_free doesn't check OnFreelist**: pages that are "refcount==0 but not on the list" get wrongly judged mergeable → an in-use PFN re-linked, double ownership (Linux's page_is_buddy always checks PageBuddy).
- [LINUX-DIFF] alloc_pages has no watermark/reservation admission; a single list per order with no MIGRATE types (anti-fragmentation missing).
- [TIMING] remove_from_free_list is O(n).
- [COMMENT][Suspected bug] An `if false` 30-line dead debug block; raw putchar embedded in free_pages.

## 4.6 pcp.rs (381 lines)
- [LINUX-DIFF][Suspected bug] The whole module has zero callers (main thread S3); once wired, PCP-resident pages fall into the is_buddy_free misjudgment surface.
- [RACE] this_cpu_pcp without preemption protection, &mut aliasing.

## 4.7 page_alloc.rs (664 lines)
- [RACE][Suspected bug] first_online_node_mut aliasing (S1); [BUG] ZoneMovable page frees silently leak.
- [LINUX-DIFF] No slow path (S2); [COMMENT] a second KERNEL_BUDDY with zero callers (three buddies coexisting).

## 4.8 buddy_allocator.rs (631 lines)
- [BUG][Pending review] CombinedAllocator decides the slab region by heap_end + a hardcoded 4MB (dual source).
- [COMMENT] Over-MAX-order large blocks pressed onto the list under-count the statistics by half; probe residue.

## 4.9 pglist.rs (381 lines)
- [RACE][Suspected bug] NODE_DATA static mut (the S1 root); add_zone's same-type second add overwrites + nr_zones double-incremented.

## 4.10 rmap.rs (373 lines)
- [LINUX-DIFF] try_to_unmap reverse-looks-up by a single index + scans all tasks (no anon_vma tree): MAP_FIXED across vaddrs misses deleting PTEs.
- [RACE][Suspected bug][ARCH] After unmapping, only a local hart sfence.vma (no remote shootdown) — a cross-process read/write window.

## 4.11 lru.rs (333 lines)
- [TIMING][Suspected bug] del/move_tail linear scans, O(n²) on the reclaim hot path (Linux O(1) doubly linked).
- [LINUX-DIFF] No active-list consumption, no PTE A-bit write-back — the two-list aging exists in name only.

## 4.12 vmscan.rs (319 lines)
- [LINUX-DIFF] reclaim_anonymous_pages linearly scans all page descriptors (not via the LRU); swap writes without a page lock (torn writes).

## 4.13 kswapd.rs (217 lines)
- [LINUX-DIFF] OOM triggered by kswapd failing 16 rounds (Linux triggers in the allocation slow path); no oom_lock serialization.

## 4.14 oom_kill.rs (287 lines)
- [LINUX-DIFF] badness uses total_vm (Linux rss+pgtables+swap): a process with a large mmap but small resident set gets wrongly killed.
- [TIMING] Only sends SIGKILL, no OOM reaper/reserved quota.

## 4.15 swap.rs (373 lines)
- [ARCH][Design inconsistency] A custom swap-entry encoding (self-consistent within the kernel, different from Linux's); no swap cache.
- [BUG][Pending review] Over-capacity prints "truncating" but actually disables; the swap area's overlap with the fs unchecked.

## 4.16 compact.rs (450 lines)
- [RACE][Suspected bug] migrate_page without migration-entry protection: page faults install the zero page during the window + remap overwrites (user writes lost + a zero-page leak).
- [RACE][Suspected bug] find_free_page's "steal any unowned page" conflicts with pcp/put_page ownership.
- [LINUX-DIFF] The permission walk hits on any task (the fallback 0xD7 grants W).

## 4.17 memblock.rs (637 lines)
- [BUG][Suspected bug] add merges only with the first adjacent region: when spanning two regions, overlapping entries (total inflated, double handover).
- [RACE] memblock_phys_alloc relies on boot ordering, lockless.

## 4.18 meminfo.rs (265 lines)
- [LINUX-DIFF] mem_available==mem_free; is_memory_low/should_trigger_oom zero callers.

## 4.19 slab.rs (722 lines)
- [BUG][Suspected bug] free doesn't validate obj_idx bounds; the kfree fallback doesn't verify cache ownership (hung on the wrong size list).
- [COMMENT][Suspected bug] On a free-list overrun it "marks full and returns that pointer" (should return null).
- [LINUX-DIFF] Free slabs never returned; no per-CPU freelist.

## 4.20 layout/vmemmap/hugepage.rs (800 lines)
- [LINUX-DIFF] Heap/slab/user_phys hardcoded ratios with a dual zone accounting (a 64MB user cap ceilings residency).
- [BUG][Pending review] init_vmemmap sets flags first then Err (falsely reporting initialization); pfn<start subtraction underflow.
- [LINUX-DIFF] Huge pages without a reserved pool; USER_HUGE defaults to including X.

## 4.22 musl focus conclusions
split exact/merge conservative/stack-expansion blind spot; the tail-page refcount convention inverted; RLIMIT_AS/DATA unenforced (ulimit -v ineffective, only MEMLOCK has one spot); the missing TLB shootdown is the biggest concurrency risk.

## 4.23 Conclusions
The structure corresponds completely, but three core runtime semantics are absent (the S2 slow path, S3 pcp, migration types); S1's static mut is the whole directory's soundness gap. The 14 suspected bugs take priority: is_buddy_free missing OnFreelist, expand_downwards overlap, the NODE_DATA alias, the compact migration window, memblock partial merging, the slab kfree validation.


# Batch 5: the fs layer (kernel/src/fs/) — Linux-comparison full-file review

Scope: 54 files/24,524 lines (including devfs/ext4/jbd2/procfs). Overall: the VFS/ext4 skeletons complete and musl static programs can run, but the ext4 write path has combinational defects that can cause data corruption, jbd2's crash consistency is substantively unachieved, and the VFS lacks the sticky bit and atomic creation.

## 5.1 VFS core
- [Semantics][High] vfs.rs:354 — Lexical .. folding + trailing-slash loss (open("regfile/") doesn't report ENOTDIR; at a mount root .. stays in place, opposite to Linux).
- [Security][High] No S_ISVTX sticky check — multiple users can delete each other's /tmp files.
- [Concurrency][High] O_CREAT's two steps without a parent-directory lock — SMP double-create, double inode.
- [Semantics][Medium] Symlink depth 8 (Linux 40); the io_poll ENOSYS stub; getdents64 returning Ok(0) when the first entry doesn't fit (Linux EINVAL); directory lseek ESPIPE (telldir broken); F_SETFL clearing the O_DIRECTORY bit; chroot not participating in path_lookup (no isolation); do_mount ignoring flags/source with three mount implementations coexisting; build_path truncating >64; icache eviction on collision leaving two Inodes for the same file.
- [Low] The dcache hash has zero calls; directory open doesn't check EISDIR; check_parent doesn't check MAY_EXEC; no ACL; elf.rs not rejecting p_filesz>p_memsz.

## 5.2 fd/pipes/char devices
- [Medium] dup2's close+install non-atomic; O_APPEND writes and pos updates unlocked (SMP interleaving); pipe capacity 16KB (Linux 64KB) without F_SETPIPE_SZ; PIPE_BUF atomicity broken.
- [Low] poll doesn't always set POLLHUP; /dev/null poll never ready; the pipe_read/write free functions dead code.

## 5.3 Block layer/page cache
- [Medium] Write-through cache (an immediate sync per BH_Dirty — aggregation lost, throughput degraded by orders of magnitude); the page-cache key ino:u32 truncated.
- [Low] The cache key lacks minor (multiple disks cross pages).

## 5.4 rootfs/devfs
- [Medium] Hardlink writes use Arc::make_mut COW (POSIX requires shared visibility); the path cache has no invalidation.
- [Low] chmod a no-op, timestamps 0, rename ancestors only one level; devfs lookup/readdir ino inconsistent.

## 5.5 ext4
- [Data][High] Sparse holes read disk block 0 (garbage data — the non-cache path); extent depth>0 unconditionally appended as a leaf (destroying the tree + a fake-full disk); unlink of a directory without EISDIR; mount not checking feature_incompat/ro_compat (bigalloc/metadata_csum all silent — real Linux judges them corrupt); directory entries not updating htree/checksum (files created by Rux invisible to real Linux).
- [Concurrency][High] EXT4_BIG_LOCK covers only namei; the write path/bitmap RMW unlocked — SMP bitmap lost updates, double allocation.
- [Medium] truncate doesn't handle depth>0; the rename cycle check wrong (can create .. loops); name_len u8 truncation unchecked; timestamps not epoch; two journal disciplines.
- [Low] The 172B fixed inode layout (s_inode_size=128 out of bounds); ext4_sync_file/fsync stubs.

## 5.6 jbd2
- [High] revoke/checkpoint entirely empty shells — crash consistency exists in name only; j_free inaccurate.
- [Medium] No tag/commit checksums and barriers (mutually incompatible with real Linux journals); stop spins busy-waiting.
- [Positive] recovery's two-pass scan/replay + the sequence-number window correct.

## 5.7 procfs
- [Correctness][High] cmdline/environ read other processes using the current task's address space (ps shows garbage).
- [Medium] environ without ptrace permission; /proc/self/fd relying on a syscall string special case (the VFS generic path fails); open("/proc/self/exe") opening an in-memory file whose content is the path text.
- [Low] 14+ stat fields 0; mounts hardcoding unimplemented file systems.

## Batch 5 statistics
High 11/Medium 27/Low 22/Info 8 = 68 items.
**Priorities**: (1) ext4 feature negotiation + htree/checksum (2) extent depth + sparse holes (3) unlink EISDIR/rename cycles (4) the jbd2 empty shells (5) sticky + the /proc cross-process read (6) ext4 writes unlocked.

---

# Batch 6: ipc + sync + security + io_uring — Linux-comparison full-file review

Scope: 18 files/~8,900 lines/~120 functions. Overall assessment: the most severe issues concentrate in the futex key semantics and io_uring mmap — **FutexKey using tid as the private key fundamentally conflicts with Linux's (mm,uaddr); it is the master gate deciding whether musl multithreaded programs can run at all**.

## 6.1 sync/futex.rs (871 lines)
- **[P1] futex.rs:48-70 — the private key = (uaddr, pid=tid)**: between threads of the same process, waiter/waker keys never match — musl pthread_mutex/cond/sem/join all lose wakeups and hang at random; the clear_child_tid wakeup mismatch makes join sleep forever. The shared key compares only the virtual address (mismatch as soon as shm attach addresses differ). Fix direction: the private key switched to address_space()/tgid; the shared key to physical frame + in-page offset.
- **[P2] the waiter's stale bucket_idx after requeue**: on a signal/timeout wake it goes to the old bucket to find its node and silently fails — a slot leak + spurious wakeups.
- [M] WAKE_OP degenerates to a plain wake; PI/robust list all ENOSYS (robust holder-death blocks forever); a bad timeout pointer → waiting forever (Linux EFAULT); no 4-byte alignment validation.
- [L] WAITER_POOL's 256 hard cap; exit.rs mistakenly passing FUTEX_PRIVATE_FLAG as internal flags.
- [Positive] re-read under the lock/slot placeholders/R12-2/IPC-C3/H4 consistent with Linux semantics.

## 6.2-6.6 SysV IPC + POSIX mq
- **[P2] semctl not waking waiters after SETVAL/SETALL** (the "release a semaphore with semctl" idiom hangs).
- **[P2] IPC_SET without an owner check** (three spots — anyone with write permission can seize ownership); RMID's owner determination missing the uid path + the cap number wrong.
- [P2] sysv_shmat's rollback path harbors a self-deadlock; mq notify firing on every enqueue (should be only empty→non-empty).
- [M] msgsnd/mq_send ignoring copy_from_user's return value (zero-filled corruption); MSG_COPY only the queue head; SHM_REMAP unimplemented + overlap destroying first then erroring; mq SIGEV_SIGNAL without siginfo, THREAD_ID unsupported; mq_unlink without permissions; the MQ fd table's 512-575 segment design debt.
- [L] EFBIG/EIDRM calibers; shm size wraparound; CLONE_SYSVSEM ignored.
- [Positive] ID encoding/seq wraparound/E2BIG re-insertion/lifecycle reference balancing/mq name and priority boundaries — the main body aligned.

## 6.7 the rest of sync
- **[P2] The RCU grace-period mechanism wholesale ineffective** (call_rcu executes immediately + synchronize_rcu actually a no-op — the existing callers survive via pin/poisoning stopgaps).
- [M] seqlock's try_write netting -255 counts (latent); the condvar's non-interruptible version contradicting with interruptible=true.
- [L] rwlock writer starvation; the semaphore IRQ fallback.
- [Positive] The spinlock variants map Linux precisely; the semaphore's register-first model consistent.

## 6.8 security
- [Positive] The 41 CAP numbers match the Linux UAPI; can_send_signal's four-tuple consistent.
- [M] The LSM hooks all placeholders (the real checks at the callers); no userns.

## 6.9 io_uring
- **[P1] Advertises SINGLE_MMAP but implemented as separate regions** — liburing's single mmap reads wrong ring pointers.
- **[P1] mmap with addr=NULL fixed at MMAP_START** — a second mmap necessarily overlaps and fails.
- [P2] Two spots double-negating errors (ENOMEM returned as fd=12).
- [M] enter(to_submit=0) always returns 0; a narrow op surface (NOP/READ/WRITE/FSYNC/CLOSE/FADVISE).
- [Positive] The lifecycle/references/validation defenses solid.

## Batch 6 statistics
P1 3/P2 7/M 21/L 14/Positives 12.
**Priorities**: (1) the futex private key switched to mm (2) io_uring dropping the SINGLE_MMAP claim + mmap going through find_free_area + fixing the double negation (3) the semctl wakeup + IPC_SET/RMID ownership (4) the mq notify condition.

---

# Batch 7: net + drivers — Linux-comparison full-file review

Scope: net/ 14 files + drivers/ 28 files, 42 files and 17,332 lines total.

## Key findings (net)
- [BUG][High] tcp.rs:2623 — **TCP RX doesn't validate the checksum** (bit flips enqueued as valid; UDP checked, TCP not).
- [BUG][High] tcp.rs:2724 — tcp_v4_err's four-tuple and the icmp-passed direction all inverted — ICMP fast-fail ineffective.
- [BUG][High] tcp.rs:1671 — The dup-ACK determination misplaced (ack==snd_una ignored) — **fast retransmit is a dead path**; loss recovery relies on RTO.
- [LINUX-DIFF][High] snd_wnd always 65535, never updated (ignoring the peer's advertised window, no zero-window probing).
- [LINUX-DIFF][High] **IP fragmentation missing in both directions** (inbound fragments scrambled, outbound over-MTU dropped); routing not connected to the data plane (no gateway concept, relying on slirp to answer) — the structural boundary of a "QEMU slirp-specific stack".
- [ABI][High] socket.rs:684 — **SOCK_NONBLOCK/SOCK_CLOEXEC type bits rejected** (musl's `SOCK_STREAM|SOCK_NONBLOCK` gets ESOCKTNOSUPPORT outright).
- [ABI][High] socket.rs:377 — **All sockets always non-blocking** (no blocking wait path) — musl blocking programs without their own retry loops read and get EAGAIN immediately.
- [LINUX-DIFF][High] UDP >1472B always fails (no fragmentation) and a length-truncation trap within the 65507 cap; an unbound sendto with source port 0 (no implicit bind).
- [RACE][High] ROUTE_TABLE static mut without a lock.
- [Medium] ~20 setsockopt options accepted then ignored (SO_RCVTIMEO ineffective); SO_ERROR always 0; SYN options not parsed (MSS always 1460, no timestamps → RTT samples inflated); the ICMP checksum unverified; arp hln/pln unchecked; the local IP hardcoded 10.0.2.15.
- [Low] The ephemeral order predictable; table full gives EIO (Linux EMFILE); the UDP checksum always sent as 0.

## Key findings (drivers)
- [TIMING][High] virtio_net xmit synchronously spins 10M+50M times inside the tx_queue with interrupts off (the chain-2 root cause remains); virtio-blk has the same 50M (outside the lock).
- [LINUX-DIFF][High] The PLIC enabled only on the boot hart — all external interrupts pile serialized onto hart0, no affinity.
- [TIMING][High] **jiffies incremented independently per hart — time runs 4× fast on 4 CPUs** (all jiffies-based TCP timeouts effectively cut to 1/4).
- [BUG][Medium] The notify address multiplied by 2 extra and not reading queue_notify_off (currently harmless since only queue0 is used); the PCI probe stride 0x1000 wrong (should be 0x8000) — slot≥4 all missed; virtio-input reading config from the wrong BAR (keyboard/mouse classification fails, everything treated as a keyboard); Flush false success (fsync without persistence); a write_block desc failure leaking; feature negotiation with three paths and three calibers.
- [LINUX-DIFF][Medium] No MSI-X (shared INTx + ISR level-falling); queue depth always 8; no CTRL_VQ/offload; evdev without poll, timestamps always 0 (libinput unusable); the FbFixScreeninfo layout misaligned.
- [Low] fence i,ir swapped (a weakly-ordered platform risk); the BAR window 256MB without bounds checks; SIOCGIFCONF missing.

## Batch 7 statistics
BUG 13/ABI 6/LINUX-DIFF 19/TIMING 4/RACE 4/VISIBILITY 2/OVERFLOW 2/COMMENT 8/ARCH 2 = **59 items** (High 12/Medium 21/Low 26).
**Priority re-checks**: tcp_v4_err's four-tuple, the TCP RX checksum, the dup-ACK determination, the input config BAR.

---

# Batch 8: sched + timer + interrupt + top level + dfx — Linux-comparison full-file review (the finale)

Scope: sched/ 8 files, 4,833 lines + timer.rs + interrupt/ 8 files + init/main/console/printk/print/config/dfx, ~10,700 lines.

## Key findings
- [Semantics][High] **SCHED_DEADLINE's CBS throttling wholly ineffective** (the budget refilled on every re-enqueue — a single DL task can monopolize 100% of all CPUs).
- [Semantics][High] **CFS has no wakeup preemption** (must wait for the next 10ms tick; check_preempt written but with no callers).
- [Concurrency][High] **tasklets without cross-CPU mutual exclusion** (RUN set non-CAS — two CPUs can run the same callback concurrently; currently latent with no users).
- [Semantics][High] **free_irq→request_irq reusing an IRQ line silently fails** (depth not reset; delivered interrupts never dispatched).
- [Semantics][High] **printk has no console output path** (only writes the ring buffer — kernel runtime errors invisible by default; syslog 6/7/8 empty semantics).
- [Semantics][High] **^C/^Z handled only on the read path** (key presses send no signal when nobody reads the tty — a runaway process can't be terminated with ^C).
- [Semantics][High] init.rs maps the whole RWX segment (W^X entirely absent); dfx softlockup's time conversion off by 10× + khungtaskd without periodic wakeup (the detectors exist in name only) + hung_task indexing by pid<256 (real pids ≥300 always skipped).
- [Medium] min_vruntime excluding curr + no sleep-compensation clamp; sched_yield a no-op for CFS; tick preemption judged by the vruntime difference rather than slice expiry; RT without throttling; the timer's single BTreeMap scanned O(n) per tick + period drift (not chasing the nominal expiry point); handlers invoked inside the action lock; the IN_PRINTK global flag losing CPU B's logs; cpu_id always 0.
- [Low] Contradictory comments in several places (rt.rs:19 opposite the implementation etc.); dead code (timer::init, overloaded, stop task, the SchedClass empty shell); find_idle_cpu always the lowest bit; panic doesn't stop the other CPUs; TASK_PAGE_OWNED aliasing 8192 pages.

## Batch 8 statistics
High 9/Medium 15/Low 18/Positives 7 = 42 items.
Positives: jiffies u64 without wraparound, the softirq/ksoftirqd/preempt_count bit layouts on target, syslog permissions/kmsg semantics, the taint table matching Linux, the tests gating correct.

---

# Master review summary (batches 1-8)

## Totals
| Batch | Scope | Findings |
|---|---|---|
| 1 | The syscall layer (11 files) | 78 |
| 2 | arch/riscv64 including 3 assembly files (24 files) | 36 |
| 3 | process (9 files) | 33 |
| 4 | mm (25 files) | 85 |
| 5 | fs (54 files) | 68 |
| 6 | ipc+sync+security+io_uring (18 files) | 45 |
| 7 | net+drivers (42 files) | 59 |
| 8 | sched+timer+interrupt+top level+dfx | 42 |
| **Total** | **278 files, 116K lines** | **446 items** |

## Cross-batch theme adjudication list (suggested for user review along these lines)

### Theme A: musl multithreaded programs wholly unusable (P0, 4 interlocking items)
1. The futex private key uses tid (batch 6 P1) — pthread_mutex/cond/join all randomly losing wakeups.
2. clone's a3/a4 parameters swapped (batch 1) — musl's direct syscall arguments misaligned.
3. CLONE_THREAD without a thread group (batch 3 P1) — kill(tgid) doesn't propagate.
4. exit_group doesn't kill the thread group (batch 3 P1).
**Review point**: this is a mix of "implementation bugs" (parameter swap/wrong key) and "design inconsistencies" (no thread-group model) — the former must be fixed; the latter needs a decision on whether to support threads.

### Theme B: signal ABI breaks (P0-P1)
5. sa_restorer ignored + the stack trampoline contradicting W^X (batch 3) — musl signal handlers SIGSEGV on return.
6. clock_nanosleep returning a negative errno (Linux's exceptional positive errno) (batch 1).
7. Number 240 misplaced + rt_tgsigqueueinfo's 3-argument layout (batch 1).

### Theme C: the socket behavior surface (P1)
8. All sockets always non-blocking (batch 7) + SOCK_NONBLOCK type bits rejected + setsockopt entirely ignored + SO_ERROR always 0.
9. TCP RX without checksums/dup-ACK dead path/four-tuple inverted/constant window (batch 7).
10. UDP >1472B failing/no implicit bind (batch 7).

### Theme D: file-system data corruption (P1)
11. ext4 feature negotiation missing + htree/checksum not maintained (batch 5) — corruption upon interop with real Linux.
12. extent depth>0 writes destroying + sparse holes reading garbage (batch 5).
13. jbd2 revoke/checkpoint empty shells (batch 5).
14. IPC: semctl not waking waiters/IPC_SET without an owner check (batch 6).

### Theme E: security class (P1-P2)
15. SUM always on (batches 1/2) — defense in depth lost + the raw-user-pointer panic surface.
16. FP registers persisting across processes (batch 2); copy_from_user not zeroing (batch 2); getrandom LCG (batch 1); brk without an upper bound (batch 1); the sticky bit missing (batch 5); the /proc cross-process read (batch 5).

### Theme F: performance/architecture (pending review)
17. switch_mm without ASID + full sfence per page (batch 2, double High); the global PTE_MODIFY_LOCK (batches 2/4); the NODE_DATA static mut alias (batch 4); no allocation slow path/dead pcp code (batch 4); xmit spinning with interrupts off (batch 7); jiffies 4× speed (batch 7); DL CBS broken/CFS without wakeup preemption (batch 8); printk without a console/^C only on the read path (batch 8).

### Theme G: io_uring (P1×2, batch 6)
18. The SINGLE_MMAP advertisement false + mmap's fixed address necessarily overlapping + errors double-negated.


## Batch progress
- [x] Batch 1: the syscall layer (the ABI baseline)
- [x] Batch 2: arch/riscv64 (including the 3 .S files)
- [x] Batch 3: process (fork/exec/wait/signal)
- [x] Batch 4: mm
- [x] Batch 5: fs (vfs/ext4/pipe)
- [x] Batch 6: ipc + sync
- [x] Batch 7: net + drivers
- [x] Batch 8: sched + timer + interrupt + the rest


## Fix implementation record (2026-09-23/24, all 8 waves landed)

8 commits (74ff805→05dc149) covering all 18 themes' fixable items. Three cases of "the review conclusion itself was wrong and corrected by testing":
1. The clone parameter order — batch 1's assertion that a3=child_tid/a4=tls was wrong; musl's clone.s proves a3=tls/a4=ctid (05dc149 corrected);
2. The svpbmt IO bits — QEMU's default CPU lacks this extension; the reserved bits invalidate the PTE (3c8ae4b reverted);
3. W6's NODE_DATA publication model broke the boot sequence (a6c3a9f reverted; W6 redone preserving the init order and completed).

### musl pthread live testing (graded probes test/pthread_min.c / pthread_staged.c)
- Single thread: create=0 → the thread body runs (TLS correct) → join returns the correct exit value ✓ (the create/TLS/clear_child_tid/futex-mm-key full chain)
- 2 threads plain: ✓
- 4 threads + mutex: after L0/L1 execute and exit, L2/L3 and join hang; the periodic snapshot shows the sole RUNNING task linked=1, queued yet long unscheduled, all others asleep. Known leftover (suspected scheduling/wakeup detail after a non-leader exits).


## 4-thread hang closed out (2026-09-24, e57ab8c)

Two root causes stacked, both confirmed by testing:
1. **sweep_dead_threads' misuse of filter+replace semantics**: retained members appearing in both the list and the free list — each exiting worker spins waiting for its own on_cpu on its own parked entry, burning all CPUs (the answer to the "queued but never picked" mystery: pick/affinity/counting all normal, there simply was no CPU free). The supply source of the same-family historical phantoms/double-execution ("alive printed twice"). Fix: the precise DEAD∧!on_cpu partition.
2. **The futex shared/private bucket split inconsistent**: musl's pthread_exit holds __thread_list_lock across SYS_exit, relying on cleartid's private-key wakeup to unlock, while the waiter parks with the shared key — the two sides land in different buckets, woken=0 while the neighboring bucket has 3 waiters (Linux solves the same interop with FLAG_IMMUTABLE). Fix: futex_hash buckets by uaddr only (matches() keeps the precise filtering).

musl live tests all green: 4-thread mutex+join 8/8+6/6, single-thread clean finish, cond broadcast + signal-before-wait 4/4+2/2, the composite soak all passing, the standard gates 4/4.

