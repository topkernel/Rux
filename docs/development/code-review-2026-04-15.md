# Rux Kernel Comprehensive Code Review — 2026-04-15

> Scope: All production code (176 .rs + 3 .S + 1 linker script + build files)
> Reference: Linux 6.19 (`/home/william/Rux/refer/linux/`)
> Each finding tagged `[BUG]` or `[DESIGN]` — reviewer decides whether to fix

---

## Overview

| Subsystem | Batch | Files | Lines | C | H | M | L | I | Total |
|-----------|-------|-------|-------|---|---|---|---|---|-------|
| Arch/Boot | 1 | 26 | ~9,800 | 0 | 2 | 4 | 10 | 6 | 22 |
| Kernel Core | 2 | 12 | ~6,340 | 0 | 4 | 6 | 11 | 4 | 25 |
| Memory Mgmt | 3 | 25 | ~9,546 | 0 | 5 | 8 | 7 | 3 | 23 |
| Scheduler | 4 | 8 | ~3,682 | 0 | 2 | 5 | 9 | 6 | 22 |
| Process Mgmt | 5 | 9 | ~4,797 | 1 | 0 | 6 | 8 | 4 | 19 |
| Sync Primitives | 6 | 8 | ~2,610 | 0 | 3 | 4 | 10 | 5 | 22+ |
| FS Core | 7 | 22 | ~7,500 | 0 | 1 | 6 | 9 | 6 | 22 |
| Ext4+JBD2 | 8 | 18 | ~6,600 | 0 | 6 | 16 | 15 | 4 | 41 |
| ProcFS | 9 | 11 | ~2,234 | 0 | 11 | 14 | 6 | 4 | 35 |
| Networking | 10 | 16 | ~7,213 | 1 | 11 | 16 | 10 | 5 | 43 |
| Syscalls | 11 | 11 | ~12,879 | 0 | 6 | 14 | 12 | 5 | 37 |
| IPC | 12 | 6 | ~3,317 | 0 | 7 | 8 | 9 | 4 | 28 |
| Interrupts | 13 | 8 | ~1,683 | 0 | 1 | 7 | 9 | 8 | 25 |
| Drivers | 14 | 28 | ~8,500 | 0 | 8 | 10 | 9 | 4 | 31 |
| Security/DFX/IO_uring | 15 | 13 | ~2,480 | 0 | 3 | 5 | 10 | 5 | 23 |
| Build Files | 16 | 10 | ~2,685 | 0 | 0 | 4 | 9 | 9 | 22 |
| **Total** | | **~237** | **~96,863** | **2** | **70** | **133** | **143** | **77** | **425** |

### Severity Definitions

- **Critical (C)**: Data corruption, kernel panic, security vulnerability
- **High (H)**: ABI incompatibility, race condition, incorrect userspace-visible behavior
- **Medium (M)**: Edge case bug, missing error handling, non-standard behavior
- **Low (L)**: Style issue, missing validation, minor inefficiency
- **Info (I)**: Design difference (may be intentional)

---

## Batch 1: Architecture/Boot (26 files, ~9,800 lines)

### [H] [BUG] F1-01: SUM bit leak on context switch
**File**: `arch/riscv64/mm/context.rs`
**Description**: `csrs` only sets the SUM bit; switching from a SUM=1 task to a SUM=0 task leaves SUM enabled. The SUM bit should be restored per-task or cleared on context switch.
**Linux**: Stores SUM in thread_info and restores it via `switch_to()`.

### [H] [BUG] F1-02: Heap-allocated PtRegs in fork never freed
**File**: `arch/riscv64/process.rs`
**Description**: Fork allocates PtRegs on the heap but never frees it. Inconsistent with stack-based `current_task_pt_regs()`.
**Linux**: Stores pt_regs on the kernel stack, not the heap.

### [M] [BUG] F1-03: COW race — parent PTE modified without PTE-level locking during fork
**File**: `arch/riscv64/mm/mm_ops.rs`
**Description**: During fork, parent PTE is read and modified without holding PTE lock, racing with concurrent page faults.
**Linux**: Uses `ptep_get_and_clear()` / `ptep_set_wrprotect()` under PTL.

### [M] [BUG] F1-04: strncpy_from_user lacks exception table entries
**File**: `arch/riscv64/uaccess.rs`
**Description**: User memory reads have no exception table entries. Page fault during read causes unrecoverable kernel panic.
**Linux**: Uses `__get_user()` with `extable` entries for fault-safe user access.

### [M] [BUG] F1-05: Premature COW with refcount=1, violating INV-COW-2
**File**: `arch/riscv64/mm/page_fault.rs`
**Description**: COW flag set even when refcount=1 (exclusive page), causing unnecessary copy-on-write overhead.
**Linux**: Only sets COW when page is shared (refcount > 1).

### [M] [BUG] F1-06: Box::leak in smp_call_function permanently leaks heap allocation
**File**: `arch/riscv64/smp/ipi.rs`
**Description**: Every `smp_call_function` call permanently leaks a `Box<IpiMessage>`.
**Linux**: Uses per-CPU call_single_data structures from a pool, no heap allocation.

---

## Batch 2: Kernel Core (12 files, ~6,340 lines)

### [H] [BUG] F2-01: UContext field order mismatch with Linux RISC-V UAPI **[FIXED]**
**File**: `signal.rs:553`
**Description**: Field order doesn't match musl expectations. Linux: `uc_flags, uc_link, uc_stack, uc_sigmask, padding, uc_mcontext`. Rux: `uc_sigmask, uc_flags, uc_link, uc_stack, uc_mcontext`. Breaks musl sigreturn.
**Linux**: Strict UAPI layout in `arch/riscv/include/uapi/asm/ucontext.h`.
**Fix**: Reordered UContext fields and SignalStack (ss_sp, ss_flags, ss_size) to match Linux UAPI. Applied 2026-04-16.

### [H] [BUG] F2-02: SigContext register array layout incompatible with musl **[FIXED]**
**File**: `signal.rs:533`
**Description**: Linux stores 32 registers as `pc, ra, sp, ...` (32 entries). Rux stores 31 registers (excluding x0) with `pc` in separate field, array starting from `ra`. Incompatible with musl.
**Linux**: `struct sigcontext` has `gregs[32]` starting from `pc`.
**Fix**: Changed to `sc_regs: [u64; 32]` with pc at [0], x1-x31 at [1..32]. Updated save/restore code. Applied 2026-04-16.

### [H] [BUG] F2-03: SS_DISABLE/SS_ONSTACK values swapped **[FIXED]**
**File**: `signal.rs:618-624`
**Description**: Rux: `SS_DISABLE=1, SS_ONSTACK=2`. Linux: `SS_ONSTACK=1, SS_DISABLE=2`. Breaks `sigaltstack()`.
**Linux**: `include/uapi/asm-generic/signal.h`.
**Fix**: Swapped to `SS_ONSTACK=1, SS_DISABLE=2`. Applied 2026-04-16.

### [H] [BUG] F2-04: Timer lock order inversion deadlock
**File**: `timer.rs:87-94 vs 147-151`
**Description**: `add_timer` locks TIMERS then ACTIONS; `del_timer` locks ACTIONS then TIMERS. Classic ABBA deadlock.
**Linux**: Uses `hrtimer` with per-CPU bases and proper lock ordering.

---

## Batch 3: Memory Management (25 files, ~9,546 lines)

### [H] [BUG] F3-01: PTE PPN mask incorrect — corrupts target PFN during page migration
**File**: `compact.rs:394`
**Description**: PPN bitmask uses `0x00FFFFFFFFFFFFFF` (40-bit) instead of correct Sv39 44-bit mask (bits [53:10]).
**Linux**: Uses `pfn << _PAGE_PFN_SHIFT` with proper pgtable macros.

### [H] [BUG] F3-02: Comment claims refcount is 0 after try_to_unmap, but it's not
**File**: `compact.rs:313`
**Description**: `try_to_unmap()` decrements `_mapcount`, not `_refcount`. Calling `free_pages()` with non-zero refcount violates INV-REF-2.
**Linux**: Uses `page_ref_unfreeze(page, 1)` to set refcount to known frozen state.

### [H] [BUG] F3-03: free_pages() only updates leader page descriptor
**File**: `page_alloc.rs:141-177`
**Description**: `alloc_pages()` sets `refcount=1` for ALL pages in block, but `free_pages()` only clears leader's `refcount`. Non-leader pages retain `refcount=1`.
**Linux**: Only leader page used for buddy operations.

### [H] [BUG] F3-04: Buddy allocator remove_from_free_list only handles head removal
**File**: `page_alloc.rs:406-424`
**Description**: Only handles case where `pfn` is list head. Non-head nodes not properly removed.
**Linux**: Uses proper doubly-linked lists with prev/next pointers.

### [H] [BUG] F3-05: Execute-only mapping mapped to Perm::None, blocks code execution
**File**: `vma.rs:147`
**Description**: `(false, false, true)` (execute-only) mapped to `Perm::None`, blocking all access. Sv39 supports execute-only pages (X=1, R=0).
**Linux**: Respects execute-only permission when hardware supports it.

### [M] [BUG] F3-06: PER_CPU_PAGES hardcoded 4 elements, MAX_CPUS configurable
**File**: `pcp.rs:232-237`
**Description**: Array hardcoded to 4 elements but MAX_CPUS comes from config. OOB access if MAX_CPUS > 4.

### [M] [BUG] F3-07: Loop underflow when objects_per_slab == 0
**File**: `slab.rs:286`
**Description**: `for i in 0..self.objects_per_slab - 1` underflows to `0..usize::MAX` when `objects_per_slab` is 0, causing infinite loop.
**Linux**: SLUB checks objects count and fails creation when 0.

---

## Batch 4: Scheduler (8 files, ~3,682 lines)

### [H] [BUG] F4-01: calc_delta_fair 64-bit overflow
**File**: `fair.rs:266`
**Description**: `(delta_exec * inv_weight) >> 32` can overflow u64 when delta_exec is large.
**Linux**: Uses `__calc_delta` with 96-bit intermediate via `mul_u64_u32_shr`.

### [H] [BUG] F4-02: calc_delta_fair missing NICE_0_LOAD factor
**File**: `fair.rs:253-266`
**Description**: Formula computes `delta_exec / weight` instead of `delta_exec * 1024 / weight`. All non-nice-0 tasks get incorrect vruntime.
**Linux**: `calc_delta_fair` calls `__calc_delta(delta, NICE_0_LOAD, &se->load)`.

### [M] [BUG] F4-03: Dequeue of prev always targets CFS queue
**File**: `sched.rs:669-671`
**Description**: When prev is not RUNNING, code calls `cfs_rq.dequeue(prev)` unconditionally. RT/DL tasks never get dequeued from their actual queue.
**Linux**: Calls `prev->sched_class->dequeue_task(rq, prev, flags)`.

### [M] [BUG] F4-04: Preempted RT/DL tasks re-enqueued without prior dequeue
**File**: `sched.rs:674-676`
**Description**: RT/DL tasks get enqueued twice — original position (never removed) + re-enqueue.

### [M] [BUG] F4-05: update_curr only for CFS in __schedule
**File**: `sched.rs:658-665`
**Description**: DL runtime accounting not updated in __schedule, causing stale exec_start and incorrect throttling.
**Linux**: Calls `update_curr_common(rq)` which updates all classes.

---

## Batch 5: Process Management (9 files, ~4,797 lines)

### [Critical] [DESIGN] F5-01: ZOMBIE/DEAD state bit values swapped vs Linux **[FIXED]**
**File**: `task.rs:166,170`
**Description**: Rux: ZOMBIE=0x10, DEAD=0x20. Linux: EXIT_DEAD=0x10, EXIT_ZOMBIE=0x20. Internal only (never exposed to ABI).
**Fix**: Swapped ZOMBIE=0x20, DEAD=0x10 to match Linux convention. Applied 2026-04-15.

### [M] [BUG] F5-02: exec does not reset signal handlers
**File**: `exec.rs`
**Description**: execve should reset all non-SIG_IGN handlers to SIG_DFL (POSIX requirement). Rux preserves old handlers across exec.
**Linux**: Calls `flush_signal_handlers(me, 0)` in `begin_new_exec()`.

### [M] [BUG] F5-03: exec does not reset sigaltstack
**File**: `exec.rs`
**Description**: Signal alternate stack not cleared during exec.
**Linux**: Clears `sas_ss_sp/sas_ss_size` in `begin_new_exec()`.

### [M] [BUG] F5-04: exec does not clear pending signals
**File**: `exec.rs`
**Description**: Pending signals from pre-exec program can be delivered to post-exec program.
**Linux**: Flushes pending signals during exec.

### [M] [BUG] F5-05: Missing clear_child_tid futex wake (breaks pthread_join)
**File**: `exit.rs`
**Description**: do_exit never touches clear_child_tid or performs futex wake.
**Linux**: `mm_release()` writes 0 to clear_child_tid and calls `do_futex(FUTEX_WAKE)`.

### [M] [BUG] F5-06: Missing clone flag validation
**File**: `fork.rs`
**Description**: CLONE_THREAD requires CLONE_SIGHAND, CLONE_SIGHAND requires CLONE_VM. Rux doesn't validate.

### [M] [BUG] F5-07: new_idle_at overwrites idle thread entry point
**File**: `task.rs:808-840`
**Description**: thread field initialized twice: first with correct ra=cpu_idle_loop, then overwritten with defaults (ra=0).

---

## Batch 6: Synchronization Primitives (8 files, ~2,610 lines)

### [H] [BUG] F6-01: Non-atomic check-then-decrement in semaphore down() + lost-wakeup
**File**: `semaphore.rs:79-125`
**Description**: Race between load() and fetch_sub() in retry loop. Waitqueue add + set-state not atomic; concurrent up() misses waiter.
**Linux**: Uses spinlock protecting count + wait list together.

### [H] [BUG] F6-02: Lost-wakeup — add to waitqueue then set state without lock
**File**: `semaphore.rs:109-120`
**Description**: Concurrent up() can find task RUNNING, mark woken, but task then sets UNINTERRUPTIBLE and sleeps forever.
**Linux**: Holds sem->lock across __set_current_state() + unlock + schedule.

### [H] [BUG] F6-03: Futex hash bucket lock held while scanning waiter pool with IRQs disabled
**File**: `futex.rs:254-268`
**Description**: Holding hash bucket lock during alloc_waiter() scan blocks timer IRQs for extended time.
**Linux**: Per-hash-bucket spinlock with plist for waiters; no separate waiter pool lock.

### [M] [BUG] F6-04: Lost-wakeup in condvar wait()
**File**: `condvar.rs:95-128`
**Description**: Mutex unlocked before waitqueue add + state set. Concurrent signal() can miss waiter.
**Linux**: Uses prepare_to_wait()/finish_wait() which atomically adds and sets state.

### [M] [BUG] F6-05: val2=0 breaks FUTEX_CMP_REQUEUE (pthread_cond_broadcast)
**File**: `futex.rs:486-495`
**Description**: sys_futex_handler hardcodes val2=0, so REQUEUE always requeues 0 waiters.
**Linux**: Passes val2 as nr_requeue for REQUEUE ops.

### [M] [BUG] F6-06: synchronize_rcu() busy-spins, deadlocks on UP
**File**: `rcu.rs:169-194`
**Description**: On UP, if called from only running task, no other task can produce quiescent state.
**Linux**: tiny RCU synchronize_rcu() just increments gp_seq and returns on UP.

---

## Batch 7: Filesystem Core (22 files, ~7,500 lines)

### [H] [BUG] F7-01: DevNo encoding uses (major<<32)|minor instead of Linux's (major<<20)|minor **[FIXED]**
**File**: `fs/dev_t.rs`
**Description**: Device number encoding incompatible with Linux ABI. Linux uses `(major << 20) | minor` (12-bit major + 20-bit minor), Rux uses `(major << 32) | minor`.
**Linux**: `include/uapi/linux/kdev_t.h`: `MKDEV(major, minor) = ((major) << 20) | (minor)`.
**Fix**: Changed to `MKDEV = (major << 20) | minor` with 12-bit major + 20-bit minor. Applied 2026-04-16.

### [M] [BUG] F7-02: Devfs inode uses bare Arc pointer — potential use-after-free
**File**: `fs/devfs/mod.rs:421`
**Description**: `inode.private_data` stores raw pointer from `Arc::as_ptr()`. If DevfsEntry is dropped, pointer dangles.

### [M] [BUG] F7-03: Pipe buffer wrap-around handling incorrect
**File**: `fs/pipe.rs`
**Description**: When write position nears buffer end, ring handling is incorrect.

### [M] [BUG] F7-04: Block cache TOCTOU initialization race
**File**: `fs/buffer.rs`
**Description**: Check-then-initialize of block cache without lock protection can cause duplicate initialization.

---

## Batch 8: Ext4 + JBD2 (18 files, ~6,600 lines)

### [H] [BUG] F8-01: Ext4InodeOnDisk struct layout mismatches Linux ext4_inode
**File**: `fs/ext4/inode.rs:15-74`
**Description**: Field layout after `i_block[15]` is wrong. `osd2` union and `i_size_high` at incorrect offsets. Files > 4GB will have corrupted sizes.
**Linux**: `fs/ext4/ext4.h:804-863`.

### [H] [BUG] F8-02: s_log_frag_size should be s_log_cluster_size
**File**: `fs/ext4/superblock.rs:29`
**Description**: Incorrect field name, affects large filesystems using clusters.

### [H] [BUG] F8-03: Superblock blocks_count read as 32-bit only
**File**: `fs/ext4/mod.rs:163`
**Description**: When INCOMPAT_64BIT is set, should use 64-bit value from `s_blocks_count_lo + s_blocks_count_hi`.
**Linux**: `fs/ext4/ext4.h:3372-3375` (`ext4_blocks_count()` helper).

### [H] [BUG] F8-04: Ext4Inode.from_disk truncates size to 32 bits
**File**: `fs/ext4/inode.rs:143`
**Description**: Only uses 32-bit `i_size`. Linux uses `i_size_lo | (i_size_high << 32)`. Files > 4GB report wrong size.

### [H] [BUG] F8-05: write_inode stores high 32 bits in wrong field
**File**: `fs/ext4/inode.rs:436`
**Description**: High 32 bits stored in `i_dir_acl` (wrong offset) instead of Linux's `i_size_high`.
**Linux**: `fs/ext4/inode.c` — ext4_do_update_inode().

### [H] [BUG] F8-06: Ext4GroupDesc only stores 32-bit descriptors (no 64-bit support)
**File**: `fs/ext4/superblock.rs:234-259`
**Description**: Missing 64-bit extension fields. Block groups beyond 2^32 blocks will fail.
**Linux**: `fs/ext4/ext4.h:403-424`.

### [M] [BUG] F8-07: find_entry_space does not handle deleted entries
**File**: `fs/ext4/namei.rs:487-510`
**Description**: Does not check for inode==0 unused entries. Space from deleted entries may not be reused.

### [M] [BUG] F8-08: get_name() uses from_utf8_unchecked — panic on corrupt filesystem
**File**: `fs/ext4/dir.rs:67-71`
**Description**: On corrupt filesystem, directory entry names could contain non-UTF-8 bytes.

### [M] [BUG] F8-09: Ext4 file write does not support external extent tree nodes
**File**: `fs/ext4/file.rs:391-484`
**Description**: Only handles inline extents (max 4 entries ≈ 512MB). Writes fail beyond that.
**Linux**: `ext4_ext_insert_extent()` promotes to external nodes.

### [M] [BUG] F8-10: Extent truncation uses physical block numbers instead of logical
**File**: `fs/ext4/mod.rs:1426-1438`
**Description**: Comparison uses physical block numbers, may free wrong blocks.
**Linux**: `fs/ext4/extents.c` — ext4_ext_remove_space().

### [M] [BUG] F8-11: is_dir_empty only checks first block
**File**: `fs/ext4/namei.rs:1360-1414`
**Description**: Entries beyond first block not detected. rmdir may succeed on non-empty directories.
**Linux**: `ext4_empty_dir()` iterates all directory blocks.

### [M] [BUG] F8-12: JBD2 tag flags written in native-endian but read as big-endian
**File**: `fs/jbd2/commit.rs:149; recovery.rs:249-264`
**Description**: LAST_TAG detection fails during recovery.
**Linux**: All journal structures use `__be32`/`__be16`.

### [M] [BUG] F8-13: JBD2 blocknr written in native-endian but read as big-endian
**File**: `fs/jbd2/commit.rs:147; recovery.rs:263`
**Description**: Block numbers byte-swapped during recovery, replaying to wrong blocks.
**Linux**: `include/linux/jbd2.h` — `__be32 t_blocknr`.

### [M] [BUG] F8-14: Journal free space decremented twice in commit
**File**: `fs/jbd2/commit.rs:283`
**Description**: Transaction's outstanding_credits already decremented j_free at handle start, commit decrements again. Journal appears full prematurely.

### [M] [BUG] F8-15: Global journal handle unsafe with interrupts
**File**: `fs/ext4/namei.rs:36-52`
**Description**: `CURRENT_JOURNAL_HANDLE` is a global AtomicUsize. Comment says "IRQs must be disabled" but callers don't disable them.
**Linux**: Uses per-task journal_info pointer.

### [M] [BUG] F8-16: read_inode group index can underflow for ino == 0
**File**: `fs/ext4/inode.rs:316; fs/ext4/mod.rs:232`
**Description**: `(ino - 1) / inodes_per_group` underflows to `u32::MAX` when ino==0.

---

## Batch 9: ProcFS (11 files, ~2,234 lines)

### [H] [BUG] F9-01: /proc/[pid]/stat format severely incomplete — breaks ps/top
**File**: `fs/procfs/pid.rs:161-191`
**Description**: Only outputs ~20 of the required 52 fields. Many hardcoded to zero, field count doesn't match Linux.
**Linux**: `fs/proc/array.c` `do_task_stat()` outputs all 52 fields.

### [H] [BUG] F9-02: /proc/[pid]/status format incomplete with wrong field order
**File**: `fs/procfs/pid.rs:84-135`
**Description**: Missing many fields Linux provides. State always shows "R (running)".
**Linux**: `fs/proc/array.c` `proc_pid_status()`.

### [H] [BUG] F9-03: /proc/self symlink uses absolute path instead of relative
**File**: `fs/procfs/self_proc.rs:16-21`
**Description**: Returns `/proc/[pid]` instead of just `[pid]`. Linux returns numeric PID string.
**Linux**: `fs/proc/self.c:25` — `sprintf(name, "%u", tgid)`.

### [H] [BUG] F9-04: /proc/version format does not match Linux
**File**: `fs/procfs/version.rs:13-29`
**Description**: Rux uses multi-line custom format. Linux uses single-line: `<sysname> version <release> (<compile_by>@<compile_host>) (<compiler>) <version>`.
**Linux**: `init/version.c:35-38`.

### [H] [BUG] F9-05: /proc/[pid]/maps device number hardcoded to 00:00
**File**: `fs/procfs/pid.rs:351-359`
**Description**: Even file-backed mappings show `00:00`. Should show `MAJOR(dev):MINOR(dev)`.
**Linux**: `fs/proc/task_mmu.c:442-460`.

### [H] [BUG] F9-06: /proc/meminfo Active/Inactive values wrong
**File**: `fs/procfs/meminfo.rs:54-55`
**Description**: Active set to mem_used_kb, Inactive hardcoded to 0.
**Linux**: `fs/proc/meminfo.c:66-73`.

### [H] [BUG] F9-07: /proc/meminfo CommitLimit calculation wrong
**File**: `fs/procfs/meminfo.rs:89`
**Description**: Calculated as `mem_total_kb / 2`, missing SwapTotal portion.
**Linux**: `mm/util.c:875-887`.

### [H] [BUG] F9-08: /proc/loadavg always reports zero load
**File**: `fs/procfs/loadavg.rs:68-72`
**Description**: `get_load_avg()` always returns (0.0, 0.0, 0.0).
**Linux**: `fs/proc/loadavg.c:14-27`.

### [H] [BUG] F9-09: /proc/[pid]/exe assumes binary is in /bin/
**File**: `fs/procfs/pid.rs:194-211`
**Description**: Always prefixes `/bin/`. Should return actual executable path.
**Linux**: Gets path from `task->mm->exe_file->f_path`.

### [H] [BUG] F9-10: /proc/[pid]/cmdline only outputs executable name, not full argv
**File**: `fs/procfs/pid.rs:137-158`
**Description**: Doesn't return full argv. Should return null-separated strings.
**Linux**: Reads from `mm->arg_start` to `mm->arg_end`.

### [H] [BUG] F9-11: lookup() cannot resolve PID directories — returns None
**File**: `fs/procfs/mod.rs:410-417`
**Description**: Returns None when encountering a PID directory component.

### [M] [BUG] F9-12: /proc/[pid]/status state always "R (running)"
**File**: `fs/procfs/pid.rs:111`
**Description**: No attempt to read actual task state. Linux has 9 distinct states.

### [M] [BUG] F9-13: /proc/mounts is hardcoded rather than querying actual mount state
**File**: `fs/procfs/mounts.rs:16-35`
**Description**: Returns static hardcoded list, doesn't reflect actual VFS mount table.

---

## Batch 10: Networking (16 files, ~7,213 lines)

### [Critical] [BUG] F10-01: TcpHdr has wrong memory layout — flags and window fields swapped on wire **[FIXED]**
**File**: `net/tcp.rs:53-72`
**Description**: `repr(C)` alignment inserts padding between `dof_res` and `flags_win`. All TCP flag checks read the window field instead of the actual flags byte. Entire TCP subsystem non-functional — three-way handshake cannot work.
**Linux**: `struct tcphdr` uses C bitfields for precise wire layout control.
**Fix**: Split `flags_win: u16` into `flags: u8, window: u16` and update all accessor methods. Applied 2026-04-15.

### [H] [BUG] F10-02: TCP checksum never computed in transmitted packets
**File**: `net/tcp.rs:1847-1878`
**Description**: In `tcp_build_packet`, checksum set to 0 and never computed. RFC 793 mandates TCP checksum.
**Linux**: `tcp_v4_send_check()` computes checksum for every outgoing segment.

### [H] [BUG] F10-03: UDP skb_put_data called twice — packet data duplicated
**File**: `net/udp.rs:481-512`
**Description**: Data put into skb in `udp_send`/`udp_sendto`, then `udp_build_packet` puts it again.

### [H] [BUG] F10-04: IP tot_len not validated against actual skb data length
**File**: `net/ipv4/mod.rs:261-304`
**Description**: Malicious or corrupted packets can claim larger tot_len than actual data.
**Linux**: `ip_rcv()` calls `skb_trim(skb, ntohs(iph->tot_len))`.

### [H] [BUG] F10-05: transmit_to_device frees skb without sending when virtio device present
**File**: `net/ethernet.rs:299-307`
**Description**: When virtio device detected, calls `skb.free()` and returns success without transmitting.
**Linux**: Calls `dev_queue_xmit(skb)`.

### [H] [BUG] F10-06: handle_syn_recv sets remote_ip to 0
**File**: `net/tcp.rs:823`
**Description**: Destroys remote IP set by caller. Subsequent sends addressed to 0.0.0.0.
**Linux**: Copies peer address from listening socket.

### [H] [BUG] F10-07: Socket file private_data stores bare Arc pointer — use-after-free
**File**: `net/socket.rs:535-536`
**Description**: Arc stored in both SOCKET_TABLE and private_data, TOCTOU race between check and increment.
**Linux**: Uses `sock_hold()`/`sock_put()` for reference counting.

### [M] [BUG] F10-08: TCP OOO queue only matches exact rcv_nxt
**File**: `net/tcp.rs:923-934`
**Description**: Doesn't handle partial overlaps. RFC 793 and Linux handle partial overlaps.

### [M] [BUG] F10-09: TCP recv only works in ESTABLISHED state
**File**: `net/tcp.rs:1008-1029`
**Description**: Cannot read buffered data in CLOSE_WAIT state. RFC 793 allows reading after FIN.
**Linux**: Allows reading in FIN_WAIT2 and CLOSE_WAIT.

### [M] [BUG] F10-10: UDP socket table never reuses freed slots
**File**: `net/udp.rs:165-174`
**Description**: Once count reaches UDP_SOCKET_TABLE_SIZE, all subsequent allocations fail.
**Linux**: Uses hash tables.

### [M] [BUG] F10-11: ARP cache access not protected by any lock
**File**: `net/arp.rs:263-312`
**Description**: Timer interrupt and softirq handlers can preempt syscall code, creating real concurrency on single-core.

### [M] [BUG] F10-12: Ethernet send always broadcasts
**File**: `net/ethernet.rs:256`
**Description**: `ethernet_send()` hardcodes `dest_mac = ETH_BROADCAST`. Doesn't resolve destination MAC via ARP.
**Linux**: Uses `arp_find()`/`neigh_resolve_output()`.

### [M] [BUG] F10-13: Loopback device drops all transmitted packets
**File**: `drivers/net/loopback.rs:43-61`
**Description**: `loopback_xmit()` immediately frees skb without delivering to receive path.
**Linux**: `loopback_xmit()` calls `netif_rx()`.

---

## Batch 11: Syscalls (11 files, ~12,879 lines)

### [H] [BUG] F11-01: Syscall dispatch NR 121/122/123 mismatched with Linux ABI **[FIXED]**
**File**: `syscall/dispatch.rs:173-175`
**Description**: 121 maps to sched_getaffinity (should be sched_getparam). All sched affinity queries and sched_getparam calls fail.
**Linux**: `include/uapi/asm-generic/unistd.h` lines 331-344.
**Fix**: Corrected mapping: 121=getparam, 122=setaffinity, 123=getaffinity. Applied 2026-04-16.

### [H] [BUG] F11-02: sys_close errno conversion truncates negative error codes **[FIXED]**
**File**: `syscall/file.rs:145`
**Description**: `e as u32 as u64` zero-extends instead of sign-extending. Userspace interprets error as success.
**Fix**: Changed to `(e as i64) as u64` for proper sign extension. Applied 2026-04-16.

### [H] [BUG] F11-03: sys_mmap silently forces PROT_READ|PROT_WRITE for MAP_ANONYMOUS **[FIXED]**
**File**: `syscall/memory.rs:208-211`
**Description**: Violates POSIX. PROT_NONE anonymous mappings should be valid.
**Linux**: `mm/mmap.c` — do_mmap() respects exact prot flags.
**Fix**: Removed PROT_READ|PROT_WRITE override for MAP_ANONYMOUS. Applied 2026-04-16.

### [H] [BUG] F11-04: sys_mmap error returns are positive errno values **[FALSE POSITIVE]**
**File**: `syscall/memory.rs:294-299`
**Description**: Returns 12 (ENOMEM) instead of -12. Userspace interprets as success.
**Note**: mmap_error constants are already negative i64, so `as u64` correctly sign-extends. Not a bug.

### [H] [BUG] F11-05: sys_munmap error returns are positive errno values **[FALSE POSITIVE]**
**File**: `syscall/memory.rs:294-299`
**Description**: Returns 12 (ENOMEM) instead of -12. Userspace interprets as success.
**Note**: mmap_error constants are already negative i64. Not a bug.

### [H] [BUG] F11-06: sys_mmap EINVAL error return is positive **[FALSE POSITIVE]**
**File**: `syscall/memory.rs:157-158`
**Description**: Returns 22 (EINVAL) instead of -22.
**Note**: mmap_error::EINVAL is -22i64, so `as u64` produces correct negative return. Not a bug.
**File**: `syscall/memory.rs:157-158`
**Description**: Returns 22 (EINVAL) instead of -22.

### [M] [BUG] F11-07: sys_rt_sigsuspend race condition between signal check and sleep
**File**: `syscall/signal.rs:391-403`
**Description**: Signal could arrive between checking pending and calling sleep(), task misses it.

### [M] [BUG] F11-08: sys_fstatat ignores flags argument (AT_SYMLINK_NOFOLLOW)
**File**: `syscall/file.rs:193-233`
**Description**: lstat always follows symlinks.

### [M] [BUG] F11-09: sys_chdir leaks a file descriptor on every call
**File**: `syscall/file.rs:615-616`
**Description**: Opens directory to verify it exists but never closes the fd.

### [M] [BUG] F11-10: sys_mremap does not copy data when moving mapping
**File**: `syscall/memory.rs:850-865`
**Description**: Allocates new mapping and unmaps old one without copying data.
**Linux**: `mm/mremap.c` — move_vma copies pages before unmapping.

### [M] [BUG] F11-11: sys_mknodat uses O_CREAT|O_TRUNC — truncates existing files
**File**: `syscall/file.rs:1435`
**Description**: Linux mknodat returns EEXIST instead of truncating.

### [M] [BUG] F11-12: sys_mprotect does not update VMA permissions
**File**: `syscall/memory.rs:501-599`
**Description**: Modifies page table entries directly but doesn't update VMA metadata. fork COW may restore old permissions.
**Linux**: `mm/mprotect.c` — mprotect_fixup updates vma->vm_page_prot and vm_flags.

### [M] [BUG] F11-13: sys_nanosleep truncates sub-millisecond sleep to zero
**File**: `syscall/time.rs:165-169`
**Description**: 500us nanosleep immediately returns. Linux guarantees at least one jiffy.
**Linux**: `kernel/time/hrtimer.c`.

---

## Batch 12: IPC (6 files, ~3,317 lines)

### [H] [BUG] F12-01: IPC_INFO msginfo struct layout completely wrong
**File**: `ipc/sysv_msg.rs:239-264`
**Description**: Writes 128 bytes of u64 fields. Linux `struct msginfo` uses `int` fields = 30 bytes.
**Linux**: `include/uapi/linux/msg.h`.

### [H] [BUG] F12-02: IPC_INFO seminfo struct layout completely wrong
**File**: `ipc/sysv_sem.rs:365-424`
**Description**: Same as F12-01. 128 bytes u64 vs 40 bytes int.
**Linux**: `include/uapi/linux/sem.h`.

### [H] [BUG] F12-03: IPC_SET reads msg_qbytes from wrong offset
**File**: `ipc/sysv_msg.rs:231`
**Description**: Offset 72 is `__msg_cbytes`, not `msg_qbytes` (offset 88).

### [H] [BUG] F12-04: GETVAL treats unused arg as pointer — always returns EFAULT
**File**: `ipc/sysv_sem.rs:230-253`
**Description**: Linux GETVAL simply returns semval as syscall return value.

### [H] [BUG] F12-05: POSIX MQ priority ordering is inverted
**File**: `ipc/posix_mq.rs:391-393`
**Description**: Insertion maintains ascending order, `remove(0)` returns lowest priority. POSIX requires highest priority first.
**Linux**: `ipc/mqueue.c` stores with highest priority first.

### [H] [BUG] F12-06: GETNCNT always returns 0 (ncnt never incremented)
**File**: `ipc/sysv_sem.rs:342-346`
**Description**: `ncnt` initialized to 0 and never modified anywhere.

### [H] [BUG] F12-07: GETZCNT returns wrong value
**File**: `ipc/sysv_sem.rs:348-364`
**Description**: Returns `1 if val == 0 else 0`. Should return count of processes waiting for semaphore to become zero.

### [M] [BUG] F12-08: IPC_SET does not update uid/gid
**File**: `ipc/sysv_msg.rs, sysv_sem.rs, sysv_shm.rs`
**Description**: Only updates mode. Linux `ipc_update_perm()` also updates uid and gid.

### [M] [BUG] F12-09: shm_detach_vma race between nattch check and slot free
**File**: `ipc/sysv_shm.rs:647-661`
**Description**: Drops spinlock before calling remove(), another thread could attach in between.

---

## Batch 13: Interrupts (8 files, ~1,683 lines)

### [H] [BUG] F13-01: Double EOI in external interrupt path
**File**: `interrupt/irqdesc.rs:347-369`
**Description**: `handle_fasteoi_irq` calls `chip.irq_eoi` (PLIC complete), then `trap.rs:303` calls `plic::complete()` again.
**Linux**: EOI happens exactly once inside the flow handler.

### [M] [BUG] F13-02: handle_fasteoi_irq missing desc->lock and state checks
**File**: `interrupt/irqdesc.rs:347-369`
**Description**: No locking, no disabled check, no ONESHOT masking.
**Linux**: chip.c:736-773 performs all state checks under lock.

### [M] [BUG] F13-03: handle_irq_event reads action chain lock-free — data race with free_irq
**File**: `interrupt/irqdesc.rs:321-343`
**Description**: If IRQ fires while free_irq is removing handler, may traverse partially-unlinked chain.

### [M] [BUG] F13-04: free_irq does not mask IRQ in hardware or synchronize
**File**: `interrupt/irqdesc.rs:262-316`
**Description**: No masking, no synchronization, no chip callbacks. If interrupt in-flight, handler references freed memory.
**Linux**: manage.c:1896-1901 calls `irq_shutdown(desc)` + `__synchronize_irq(desc)`.

---

## Batch 14: Drivers (28 files, ~8,500 lines)

### [H] [BUG] F14-01: VirtIO Block capacity read uses non-volatile access
**File**: `drivers/virtio/mod.rs:266-267`
**Description**: MMIO read uses plain dereference instead of `read_volatile`. Compiler could optimize out or reorder.

### [H] [BUG] F14-02: VirtIO Block read path response descriptor missing VIRTQ_DESC_F_WRITE
**File**: `drivers/virtio/mod.rs:451-457`
**Description**: flags=0 instead of 2. Device may not write response buffer correctly.
**Linux**: Always sets response as writable scatterlist element.

### [H] [BUG] F14-03: VirtIO GPU send_command hardcodes descriptors 0 and 1
**File**: `drivers/gpu/virtio_gpu.rs:624-674`
**Description**: Bypasses allocator, only one GPU command can be in-flight at a time.

### [H] [BUG] F14-04: VirtIO Net TX uses virtual addresses instead of physical for DMA
**File**: `drivers/net/virtio_net.rs:385-398`
**Description**: All other VirtIO drivers correctly use `virt_to_phys()`. DMA reads/writes to wrong physical addresses.
**Linux**: Uses `dma_map_single()`.

### [H] [BUG] F14-05: VirtIO Net RX buffer leak
**File**: `drivers/net/virtio_net.rs:431-504`
**Description**: Descriptor allocator keeps incrementing, rx_buffers Vec grows unboundedly.

### [H] [BUG] F14-06: PLIC enable_interrupt has TOCTOU race on enable register
**File**: `drivers/intc/plic.rs:102-116`
**Description**: Read-modify-write not atomic. Other harts could modify same word concurrently on multi-hart systems.
**Linux**: Uses `raw_spin_lock_irqsave()` or atomic `__set_bit()`.

### [H] [BUG] F14-07: VirtIO Net MMIO register offsets all wrong
**File**: `drivers/net/virtio_net.rs:149`
**Description**: STATUS at 0x50 instead of 0x70. Driver writes status values to wrong registers.
**Linux**: `include/uapi/linux/virtio_mmio.h`.

### [M] [BUG] F14-08: EVIOCGPROP ioctl value conflicts with EVIOCGID
**File**: `drivers/input/evdev.rs:31`
**Description**: Defined as `0x80004502`, should be `0x80004509`.
**Linux**: `include/uapi/linux/input.h`.

### [M] [BUG] F14-09: EVIOCGBIT extracts event type from wrong position in cmd
**File**: `drivers/input/evdev.rs:313`
**Description**: Uses `>> 8` to extract, should use `& 0xFF` then subtract 0x20.

### [M] [BUG] F14-10: PCI set_command writes 32-bit to 16-bit COMMAND register
**File**: `drivers/pci/mod.rs:371-373`
**Description**: Overwrites STATUS register. Should use 16-bit write.
**Linux**: Uses `pci_write_config_word()`.

### [M] [BUG] F14-11: VirtIO Input dereferences used ring without volatile read
**File**: `drivers/input/virtio_input.rs:381-383`
**Description**: Device-written fields via DMA must use volatile access.

---

## Batch 15: Security/DFX/IO_uring (13 files, ~2,480 lines)

### [H] [BUG] F15-01: io_uring read/write file position TOCTOU race
**File**: `io_uring/mod.rs:546-556`
**Description**: Concurrent operations between `get_pos()` + `do_read()` + `set_pos()` can change position. do_read may also internally advance pos, causing double advancement.
**Linux**: Uses `kiocb` with `ki_pos`; pread doesn't change f_pos.

### [H] [BUG] F15-02: Signal permission check incomplete — missing UID cross-checks
**File**: `security/mod.rs:59-75`
**Description**: Missing `cred.euid == target.suid` comparison. Linux has 4 comparisons, Rux only 3.
**Linux**: `kernel/signal.c:kill_ok_by_cred()`.

### [H] [BUG] F15-03: io_uring CLOSE can close the ring fd itself
**File**: `io_uring/mod.rs:643-649`
**Description**: No validation that fd is not the io_uring ring fd. Closing ring fd causes use-after-free.
**Linux**: Uses separate fixed-file table and validates fd.

---

## Batch 16: Build Files (10 files, ~2,685 lines)

### [M] [DESIGN] F16-01: Kernel.toml [performance] settings have no effect
**File**: `kernel/build.rs:136-199`
**Description**: opt_level/lto/codegen_units parsed and emitted as env vars but never consumed. Actual optimization comes from Cargo.toml profiles.

### [M] [DESIGN] F16-02: enable_aarch64 = true contradicts project status
**File**: `Kernel.toml:18`
**Description**: Project docs state aarch64 was removed, but config still enables it.

### [M] [DESIGN] F16-03: debug_log feature has no effect
**File**: `kernel/Cargo.toml:27`
**Description**: Defined and passed but no source file uses `#[cfg(feature = "debug_log")]`.

### [L] [BUG] F16-04: build.rs default platform falls back to aarch64
**File**: `kernel/build.rs:134`
**Description**: Should default to riscv64.

---

## Prioritized Fix Recommendations

### Critical (Fix Immediately)

1. ~~**F10-01**: TcpHdr layout wrong~~ **[FIXED]** — restructured to `dof_res: u8, flags: u8, window: u16`
2. ~~**F5-01**: ZOMBIE/DEAD state bit values swapped~~ **[FIXED]** — swapped to match Linux convention

### High (Fix Soon)

**ABI Compatibility (affects userspace compatibility)**:
3. ~~**F2-01/F2-02**: UContext/SigContext layout incompatible with musl~~ **[FIXED]**
4. ~~**F2-03**: SS_DISABLE/SS_ONSTACK values swapped~~ **[FIXED]**
5. ~~**F7-01**: DevNo encoding incompatible with Linux ABI~~ **[FIXED]**
6. ~~**F11-01**: Syscall numbers 121/122/123 mapped incorrectly~~ **[FIXED]**
7. **F11-02**: ~~sys_close errno sign extension~~ **[FIXED]** | **F11-03**: ~~mmap PROT override~~ **[FIXED]** | F11-04~06: **[FALSE POSITIVE]**

**Data Integrity (potential data corruption)**:
8. **F8-01~06**: Ext4 on-disk struct layout mismatches (>4GB file corruption)
9. **F8-12/13**: JBD2 tag field endianness mismatch (recovery writes wrong blocks)
10. **F3-01**: PTE PPN mask incorrect

**Networking**:
11. **F10-02**: TCP checksum never computed
12. **F10-05**: virtio_net silently drops outgoing packets when device present
13. **F10-06**: TCP remote_ip set to 0

**Drivers**:
14. **F14-04**: VirtIO Net DMA uses virtual addresses
15. **F14-02**: VirtIO Block response descriptor missing WRITE flag
16. **F14-07**: VirtIO Net MMIO register offsets wrong

**Synchronization/Races**:
17. **F4-01/02**: vruntime calculation incorrect (missing NICE_0_LOAD factor + overflow)
18. **F6-01/02**: Semaphore non-atomic operation and lost-wakeup
19. **F1-01**: SUM bit leak

### Medium (Fix by Subsystem)

Distribute across corresponding development phases:
- Process Mgmt: F5-02~07 (exec signal reset, clear_child_tid)
- Memory Mgmt: F3-03~07 (buddy allocator, slab)
- Scheduler: F4-03~05 (RT/DL queue management)
- Networking: F10-08~13 (OOO, recv states, ARP locking)
- Syscalls: F11-07~13 (sigsuspend, mremap, mprotect)
- IPC: F12-08~09 (IPC_SET permissions, shmdt race)
- ProcFS: F9-12~13 (state tracking, mounts hardcoded)
- Interrupts: F13-02~04 (locking, synchronization, state checks)
