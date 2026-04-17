# Rux Kernel 代码检视 — 2026-04-17（第二轮）

> Scope: All production code (276 files, 106,346 lines)
> Reference: Linux 6.19 (`/home/william/Rux/refer/linux/`)
> Previous review: `docs/development/code-review-2026-04-15.md` (425 findings, all fixed/deferred)
> Focus: POSIX/ABI compatibility, Linux behavioral equivalence, musl binary compatibility

## Severity Definitions

| Level | Meaning |
|-------|---------|
| **Critical** | Data loss, security vulnerability, or boot failure |
| **High** | Incorrect behavior visible to userspace, POSIX/ABI violation |
| **Medium** | Logic error with limited blast radius, missing edge case |
| **Low** | Cosmetic, style, minor inefficiency |
| **Info** | Observation, no action required |

## Category Definitions

| Tag | Meaning |
|-----|---------|
| `[BUG]` | Implementation defect — should be fixed |
| `[DESIGN]` | Intentional design difference from Linux — reviewer decides |
| `[POSIX]` | POSIX standard violation |
| `[ABI]` | Linux ABI incompatibility — musl binaries may break |

---

## Batch 1: Arch/Boot (26 files, ~9,800 lines) — 26/26 reviewed

### [High] [BUG] F01-01: `copy_page_table_cow` modifies parent PTEs without holding page table lock
**File**: `arch/riscv64/mm/mm_ops.rs:1062-1066`
**Description**: Fork COW path modifies parent PTEs (W→COW downgrade) without any page table lock. Concurrent page fault on another CPU could race with the PTE modification.
**Linux**: Uses `ptep_set_wrprotect()` atomically under PTL, plus `get_page()` for refcount.
**Impact**: Memory corruption on real SMP hardware during concurrent fork + page fault.

### [Medium] [BUG] F01-02: boot.S KERNEL_MAP field order mismatches Rust `KernelMapping` layout
**File**: `arch/riscv64/boot.S:449-457` vs `arch/riscv64/mm/memory_layout.rs:155-185`
**Description**: Assembly data section has fields in order: `virt_addr, phys_addr, size, virt_offset...` but Rust struct expects `virt_addr, virt_offset, phys_addr, size...`. Fields at offsets 8-24 are swapped. Currently harmless because only `va_pa_offset` and `va_kernel_pa_offset` (which do match) are used.
**Impact**: Future code reading `phys_addr`, `size`, or `virt_offset` will get wrong values.

### [Medium] [BUG] F01-03: `current_task_pt_regs()` assumes stack-based pt_regs, but fork uses heap-allocated
**File**: `arch/riscv64/trap.rs:23-42`
**Description**: Calculates pt_regs from stack top, but fork allocates child pt_regs on the heap. For newly forked children, `thread.sp` points to heap pt_regs, not stack-based. Any code calling `current_task_pt_regs()` on a forked child before its first trap return will read garbage.
**Linux**: pt_regs always at kernel stack top. `copy_thread()` uses `task_pt_regs(p)`. No heap allocation.

### [Medium] [BUG] F01-04: `sstatus.SUM` bit enabled globally for all tasks via `enable_external_interrupt()`
**File**: `arch/riscv64/trap.rs:125-142`
**Description**: Unconditionally sets SUM bit in sstatus for secondary CPUs, permanently enabling Supervisor User Memory access for all contexts. Should only be enabled during uaccess routines.
**Linux**: Clears SUM at trap entry, enables only inside uaccess routines.

### [Medium] [BUG] F01-05: IPI CSD_QUEUES uses `static mut` with raw pointer list operations
**File**: `arch/riscv64/ipi.rs:180-199`
**Description**: `CSD_QUEUES` is `static mut` with manual circular linked list insertion via raw pointers. Fragile and susceptible to list corruption on errors.

### [Medium] [DESIGN] F01-06: `copy_thread` heap-allocates pt_regs — deviates from Linux stack convention
**File**: `arch/riscv64/process.rs:91-100`
**Description**: Uses `alloc::alloc::alloc` for child pt_regs. Linux always places pt_regs at kernel stack top. Tools expecting stack-based pt_regs (ptrace, kexec) would break.
**Impact**: Different lifecycle semantics. Potential memory leak if fork_pt_regs pointer is overwritten.

### [Low] F01-07: boot.S early page tables use RWX permissions for all kernel pages
**File**: `arch/riscv64/boot.S:132`
**Description**: All kernel memory mapped with `V|R|W|X|G|A|D` — no permission separation.
**Linux**: Permanent kernel maps use RX for .text, R for .rodata, RW for .data.

### [Low] F01-08: trap.S LR/SC reservation clear comment misleading
**File**: `arch/riscv64/trap.S:625`
**Description**: Comment says "sp + PT_EPC" but code uses `sp + PT_SIZE`. Functionally correct (any valid address works for reservation clear) but misleading.

### [Info] F01-09: `early_pmd_dev` extern symbol doesn't match `early_pmd_io` assembly symbol
**File**: `arch/riscv64/mm/mmu_init.rs:35`
**Description**: Rust declares `early_pmd_dev` but boot.S defines `early_pmd_io`. Dead code — never used.

---

## Batch 2: Kernel Core + Signal (12 files, ~3,500 lines) — 12/12 reviewed

### [Critical] [ABI] F02-01: SignalFrame uc pointer off by 4 bytes due to alignment padding **[FIXED]**
**File**: `signal.rs:938-939`
**Description**: In `setup_frame()`, `regs.a2 = frame_addr + 32 + size_of::<SigInfo>()` computes uc at offset 52. But `SignalFrame` has 4 bytes of alignment padding after `info` (SigInfo=20 bytes, UContext needs 8-byte alignment), so `uc` is actually at offset 56. Handler receives wrong pointer to `UContext`.
**Linux**: Uses C pointer arithmetic which naturally handles alignment.
**Impact**: Signal handlers with `SA_SIGINFO` read corrupted `ucontext` — `uc_flags` contains garbage.

### [High] [ABI] F02-02: SigAction sa_flags is u32 but Linux ABI is unsigned long (8 bytes on RV64)
**File**: `signal.rs:94-96`
**Description**: `SigFlags(u32)` makes `sa_flags` 4 bytes. Linux defines `sa_flags` as `unsigned long` = 8 bytes on RV64. When writing `oldact` to userspace, 4 bytes of uninitialized padding accompany the flags. musl reads back 8 bytes and sees garbage in upper 32 bits.
**Linux**: `struct sigaction` has all fields 8 bytes on RV64.
**Impact**: musl programs querying `sigaction()` see corrupted `sa_flags`.

### [High] [ABI] F02-03: Signal enum missing signals 23-31 (SIGURG through SIGSYS)
**File**: `signal.rs:27-72`
**Description**: `Signal` enum only defines signals 1-22. Signals 23-31 (SIGURG, SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF, SIGWINCH, SIGIO, SIGPWR, SIGSYS) are missing. `handle_default_signal()` has no case for them — they fall through to default ignore instead of terminating the process.
**Linux**: `include/uapi/asm-generic/signal.h` defines all 1-31.
**Impact**: POSIX signals 23-31 have wrong default behavior (ignored instead of terminate/stop).

### [Medium] [ABI] F02-04: SigInfo struct is 20 bytes but Linux siginfo_t is 128 bytes
**File**: `signal.rs:496-509`
**Description**: Rux `SigInfo` has `{si_signo, si_code, si_pid, si_uid, si_status}` = 20 bytes. Missing `si_errno` field. Linux `siginfo_t` is 128 bytes. musl expects 128 bytes — handler reads past actual data.
**Impact**: SA_SIGINFO handlers reading past first two fields get garbage or fault.

### [Medium] [ABI] F02-05: SigContext missing floating-point state
**File**: `signal.rs:558-565`
**Description**: Rux `SigContext` has only integer registers (264 bytes). Linux `struct sigcontext` includes FPU state (~784 bytes total). FP registers not saved/restored across signal handlers.
**Linux**: `struct sigcontext` includes `sc_fpregs` union for FPU state.
**Impact**: Any program using floating-point (including printf %f) gets corrupted FP registers after signal handler.

### [Medium] [POSIX] F02-06: SA_RESTART logic incorrect — applies to saved PC, not syscall restart
**File**: `signal.rs:893-898`
**Description**: Checks `SA_RESTART` and rewinds PC by 4. Linux's mechanism decides restart based on internal return codes (`-ERESTARTSYS`), then checks `SA_RESTART` to convert restart to `-EINTR`.
**Impact**: Syscalls interrupted by signals behave differently than Linux.

### [Medium] [POSIX] F02-07: SIGTTOU/SIGTTIN default action is "ignore" but POSIX says "stop"
**File**: `signal.rs:1059`
**Description**: SIGTTIN(21) and SIGTTOU(22) handled as "ignore" in same case as SIGCHLD. POSIX default is "stop" (same as SIGSTOP).
**Impact**: Background terminal I/O won't stop the process as expected.

### [Medium] [BUG] F02-08: Persistent log timestamp conversion incorrect
**File**: `printk.rs:1120`
**Description**: `timestamp / 1000` but TIMER_FREQ=10MHz, so correct conversion to microseconds is `timestamp / 10`. Timestamps are off by 100x. (Persistent logging currently disabled.)

### [Medium] [BUG] F02-09: is_root_readonly() returns inverted result
**File**: `cmdline.rs:528-531`
**Description**: Returns `!has_param("ro")` — returns `true` when root is NOT readonly. Function unused outside tests.

### [Low] F02-10: printk cpu_id hardcoded to 0
**File**: `printk.rs:253`
**Description**: All log records show `cpu(0)` regardless of which CPU generated the message.

### [Low] F02-11: UART write_reg uses `options(nomem)` for MMIO write
**File**: `console.rs:246-252`
**Description**: `nomem` tells compiler the asm doesn't read/write memory, but UART MMIO IS a memory-mapped write. Should not use `nomem` for MMIO.

### [Low] F02-12: Dual BTreeMaps for timer + action adds overhead
**File**: `timer.rs:51-54`
**Description**: Two separate `Spinlock<BTreeMap>` keyed by same timer ID. Single struct combining both would reduce lock acquisitions.

### [Info] F02-13: main.rs mount code path duplicated between PCI and MMIO
**File**: `main.rs:498-548`
**Description**: Nearly identical 25-line blocks for PCI and MMIO virtio block device mount. Should extract helper.

---

## Batch 3: Memory Management (25 files, ~9,500 lines) — 25/25 reviewed

### [Critical] [BUG] F03-01: `get_zeroed_page` writes to physical address, not virtual **[FIXED]**
**File**: `mm/page_alloc.rs:126-133`
**Description**: `get_zeroed_page` calls `alloc_page()` returning a physical address, then does `core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE)` — writing to the physical address directly. After MMU is enabled, physical addresses are not directly writable. Must convert to virtual (linear-mapped) address first.
**Linux**: `get_zeroed_page()` uses virtual addresses via `__get_free_pages`.
**Impact**: Zeroing silently fails or corrupts memory after MMU init.

### [High] [BUG] F03-02: `free_pages` only resets leader page refcount for high-order blocks
**File**: `mm/page_alloc.rs:148-156`
**Description**: `free_pages()` sets `refcount(0)` and clears `Referenced` only on the leader page (pfn), but `alloc_pages()` sets refcount=1 on ALL pages in the block. Remaining pages retain `refcount=1`, causing refcount corruption if individually freed later.
**Linux**: `__free_pages()` manages all pages' refcounts correctly through buddy merge protocol.
**Impact**: Refcount corruption → use-after-free or double-free on high-order free + individual re-free.

### [High] [BUG] F03-03: Standalone `BuddyAllocator::remove_from_free_list` only handles head removal
**File**: `mm/page_alloc.rs:406-424`
**Description**: If target PFN is not the list head, block is not removed but `free_counts` is still decremented — corrupted free list and inaccurate counts.
**Linux**: Linux buddy allocator maintains proper doubly-linked lists; removal always handles non-head cases.
**Impact**: Buddy merging broken in standalone BuddyAllocator path.

### [High] [BUG] F03-04: `mm_users_dec` / `mm_count_dec` can underflow without detection
**File**: `mm/mm_struct.rs:577-578,595-596`
**Description**: Both use `fetch_sub(1)` unconditionally. If counter is already 0, it wraps to -1 (AtomicI32), violating refcount invariant. No underflow guard unlike `Page::put_page()`.
**Linux**: `mmput()` / `mmdrop()` use `atomic_dec_and_test()`.
**Impact**: Premature page table free → use-after-free of entire address space.

### [Medium] [BUG] F03-05: `migrate_page` may over-count destination mapcount
**File**: `mm/compact.rs:273-319`
**Description**: `remap_page` calls `dst.add_mapcount()` for every task with a matching VMA, even if the page was only mapped once. This may over-increment mapcount.
**Linux**: `migrate_pages()` copies mapcount exactly from source to destination.
**Impact**: Inflated mapcount prevents page reclamation.

### [Medium] [BUG] F03-06: `remap_page` scans page tables without holding mmap_lock for write
**File**: `mm/compact.rs:335-415`
**Description**: Walks each task's page table and modifies PTEs while only holding VMA read lock. Another thread could `munmap` the same range concurrently — TOCTOU race.
**Linux**: `migrate_pages()` holds `mmap_lock` for write during migration.
**Impact**: PTE writes into freed page tables → memory corruption.

### [Medium] [BUG] F03-07: `try_to_unmap` drops VMA lock before page table walk
**File**: `mm/rmap.rs:248-308`
**Description**: Acquires `vma_read()` to check if any VMA matches, then drops it before walking the page table. Between VMA check and PTE modification, the VMA could be removed — use-after-free of page table page.
**Linux**: `try_to_unmap()` holds `mmap_lock` for entire duration.
**Impact**: Use-after-free of page table pages during reclaim.

### [Medium] [BUG] F03-08: Swap I/O passes physical addresses without `phys_to_virt`
**File**: `mm/swap.rs:264-271,286-293`
**Description**: `swap_read_page` / `swap_write_page` convert physical address directly to `&mut [u8]` slice. After MMU is enabled, physical addresses cannot be dereferenced directly.
**Linux**: Uses `kmap()` / `kunmap()` to get virtual addresses.
**Impact**: Swap I/O faults or writes to wrong addresses after MMU init.

### [Medium] [DESIGN] F03-09: `VmaManager::add` does not attempt VMA merge with adjacent VMAs
**File**: `mm/vma.rs:412-445`
**Description**: When adding a new VMA, only checks for overlap — never merges adjacent VMAs with same flags. VMA count grows without bound (e.g., `brk` increments), causing O(n) lookups.
**Linux**: `vma_merge()` called after every `mmap`.
**Impact**: Performance degradation over time. POSIX-correct but suboptimal.

### [Medium] [BUG] F03-10: SlabAllocator init writes through `&self` → UB
**File**: `mm/slab.rs:523-531`
**Description**: Creates `*mut SlabAllocator` from `&SLAB_ALLOCATOR` and writes to it. Violates Rust aliasing model.
**Linux**: Uses proper locking and initialization ordering.
**Impact**: Potential miscompilation. Low probability with current compilers.

### [Medium] [BUG] F03-11: `alloc_pages` high-order compaction re-borrows zone via raw pointer
**File**: `mm/page_alloc.rs:82-104`
**Description**: Casts `&mut Zone` to `*mut Zone` then re-creates `&mut` from raw pointer to work around aliasing. Still UB if any other reference exists.
**Linux**: Uses `zonelist` iteration with proper locking.
**Impact**: Potential UB. Works in practice with current compilers.

### [Medium] [BUG] F03-12: `lru_del_page` may deadlock if called with lru_lock already held
**File**: `mm/lru.rs:70-142`
**Description**: `lru_del_page` calls `node.lru_lock.lock()` unconditionally. If called from `lru_move_to_tail` which is called from contexts already holding the lock, Spinlock deadlock.
**Linux**: LRU operations check whether lock is already held.
**Impact**: Potential deadlock in LRU operations.

### [Medium] [BUG] F03-13: MemBlock `add_reserved` overcounts overlaps in `total_size`
**File**: `mm/memblock.rs:186-217`
**Description**: When merging overlapping reserved regions, `total_size` is not adjusted — only region size updated. `available_memory()` may underestimate.
**Linux**: Carefully adjusts `total_size` for overlaps during merge.
**Impact**: Slightly inaccurate available memory reporting.

### [Medium] [BUG] F03-14: `try_to_unmap` and `try_to_unmap_with_swap` duplicate 95% code
**File**: `mm/rmap.rs:212-415`
**Description**: Two functions share ~95% identical code. Bugs fixed in one may be missed in the other.
**Linux**: Uses callback-based walk sharing core logic.
**Impact**: Maintenance hazard.

### [Low] [DESIGN] F03-15: `meminfo.pcp_pages` hardcoded to 4 CPUs
**File**: `mm/meminfo.rs:55`
**Description**: `pcp_pages: [usize; 4]` — stats lost for CPUs >3 if MAX_CPUS > 4.
**Impact**: Incomplete diagnostics on >4 CPU systems.

### [Low] [DESIGN] F03-16: `layout.rs` user_phys hardcoded to 25%/64MB
**File**: `mm/layout.rs:105-106`
**Description**: User physical memory = 25% of remaining, max 64MB. On 2GB systems, only 64MB for user pages.
**Linux**: Dynamic zone-based allocation.
**Impact**: May limit user physical memory unnecessarily.

### [Low] [BUG] F03-17: `CombinedAllocator::dealloc` hardcodes 4MB slab region
**File**: `mm/buddy_allocator.rs:533`
**Description**: If slab region size changes in config, slab frees fall through to buddy → double-free.
**Impact**: Dead code (not used as global_allocator), but broken if activated.

### [Low] [BUG] F03-18: `set_order` overwrites entire `private` field
**File**: `mm/page_desc.rs:519-521`
**Description**: `set_order` stores into entire `private` field, overwriting upper bits. Getter masks with `0xFF`. Asymmetry is a maintenance trap.
**Impact**: Currently safe since buddy allocator exclusively owns `private`.

### [Low] [BUG] F03-19: `page_desc_stats` reads without consistency guarantees
**File**: `mm/page_desc.rs:756-798`
**Description**: Scans all page descriptors without locks. Individual reads are atomic but values may be inconsistent across fields.
**Impact**: Diagnostic inaccuracy only.

### [Low] [DESIGN] F03-20: `heap_size_to_order` latent overflow for non-power-of-2 heap
**File**: `mm/buddy_allocator.rs:246-263`
**Description**: For non-power-of-2 page counts, initial block may be larger than actual heap, potentially overrunning heap region. Safe with current config.
**Impact**: Latent bug if KERNEL_HEAP_SIZE changes to non-power-of-2.

### [Low] [BUG] F03-21: kswapd may do one extra unnecessary wake loop
**File**: `mm/kswapd.rs:67-85`
**Description**: Clears `KSWAPD_WAKE` before checking watermarks. Allocation between clear and sleep → lost wakeup. Worst case: one extra iteration.
**Impact**: Minor performance issue.

### [Info] F03-22: `add_reserved` does not merge newly-adjacent regions after first merge
**File**: `mm/memblock.rs:186-217`
**Description**: After merging with one region, returns immediately. If new region bridges two previously separate regions, both should merge into one.
**Impact**: Slightly more regions than necessary. No correctness issue.

---

## Batch 4: Scheduler (8 files, ~3,700 lines) — 8/8 reviewed

### [Critical] [BUG] F04-01: RT enqueue lacks on_rq guard — duplicate enqueue corrupts list **[FIXED]**
**File**: `sched/rt.rs:120-164`
**Description**: `RtRunQueue::enqueue()` does not check `rt_entity.on_rq` before inserting. If a task already on the runqueue is enqueued again (e.g., RR tick → enqueue, then `__schedule()` → enqueue again), the same `rt_run_list` node is added twice, corrupting the linked list. CFS enqueue correctly checks `is_on_rq()` but RT does not.
**Linux**: `__enqueue_rt_entity()` prevents double-queue via `on_rq` flag and `plist` deduplication.
**Impact**: List corruption → infinite loops in `pick_next` or use-after-free. System hang under SCHED_RR workloads.

### [Critical] [BUG] F04-02: DL dequeue matches by (ptr, deadline) but deadline changes underfoot **[FIXED]**
**File**: `sched/deadline.rs:126-163`
**Description**: `DlRunQueue::dequeue()` searches by matching both task pointer AND current deadline. If `update_deadline()` changes the deadline between enqueue and dequeue, the linear scan fails to find the entry. Task leaks in BTreeMap, `dl_nr_running` permanently inflated.
**Linux**: Uses `rbtree` with `RB_CLEAR_NODE` on dequeue, keyed by entity not mutable deadline.
**Impact**: DL tasks leak from runqueue, cannot be scheduled, admission control poisoned.

### [High] [POSIX] F04-03: SCHED_RR time_slice unit mismatch — 10x longer than specified
**File**: `sched/rt.rs:329-330`, `sched/sched.rs:1014-1023`
**Description**: `time_slice` initialized to `RR_TIMESLICE_MS = 100` (milliseconds). `scheduler_tick()` decrements by 1 per tick. With `KERNEL_HZ=100`, each tick = 10ms, so actual timeslice = 100 × 10ms = 1000ms (1 second), not the intended 100ms.
**Linux**: `RR_TIMESLICE = (100 * HZ / 1000)` = 10 jiffies = 100ms with HZ=100.
**Impact**: SCHED_RR tasks get 10x longer timeslices than POSIX specifies. Round-robin semantics violated.

### [High] [BUG] F04-04: SCHED_IDLE tasks never preempted by tick
**File**: `sched/sched.rs:1037-1039`
**Description**: When `policy == SchedPolicy::Idle`, tick handler does nothing. CFS preemption check only runs for `Normal | Batch`. A SCHED_IDLE task monopolizes the CPU indefinitely.
**Linux**: `task_tick_fair()` runs for all CFS-class tasks including SCHED_IDLE.
**Impact**: SCHED_IDLE task can starve all other CFS tasks.

### [Medium] [BUG] F04-05: sched_slice integer division truncates unfairly for low-weight tasks
**File**: `sched/fair.rs:721`
**Description**: `sched_period / load_weight * se.load.weight` — integer division truncates to 0 when `sched_period < load_weight`. Always falls back to `SCHED_MIN_GRANULARITY_NS`, losing proportional distribution.
**Linux**: Uses `__calc_delta()` with `(period * weight * inv_weight) >> 32` to avoid truncation.

### [Medium] [BUG] F04-06: DL pick_next_cpu hardcoded 64-entry stack buffer
**File**: `sched/deadline.rs:212`
**Description**: Fixed-size stack array of 64 entries. If more than 64 DL tasks have CPU affinity restrictions, overflow path silently skips tasks. In practice DL task counts are small.

### [Medium] [BUG] F04-07: DL replenish_runtime does not advance deadline — CBS incomplete
**File**: `sched/deadline.rs:345-349`
**Description**: Only refills runtime budget. CBS requires deadline also postponed to `now + period` when replenished. Throttled tasks may retain stale deadlines.
**Linux**: `replenish_dl_entity()` advances both deadline and runtime with multi-period catching-up.

### [Medium] [BUG] F04-08: nr_running decremented even when dequeue fails
**File**: `sched/sched.rs:927-949`
**Description**: `dequeue_task()` always attempts `fetch_update` to decrement `nr_running` even if class-specific dequeue returned `false`. `checked_sub(1)` prevents underflow but count becomes inaccurate.
**Impact**: Idle fast-path may incorrectly skip scheduling when tasks are runnable.

### [Medium] [BUG] F04-09: RR tick re-enqueues task without setting RUNNING state
**File**: `sched/sched.rs:1019-1020`
**Description**: RR tick handler enqueues current task when timeslice expires, but doesn't set state to RUNNING. If another thread set state to INTERRUPTIBLE between tick and enqueue, a sleeping task goes on the runqueue.

### [Low] [BUG] F04-10: for_each_task does not iterate global runqueue tasks
**File**: `sched/sched.rs:1071-1089`
**Description**: Only iterates per-CPU `current` and `idle`. Sleeping/runnable-but-not-running tasks on global RQ are missed. Affects `/proc`, signal broadcast, OOM killer.
**Impact**: Incomplete task enumeration.

### [Low] [BUG] F04-11: Double reschedule IPI in enqueue_task + wake_up
**File**: `sched/sched.rs:853-863`, `process/task.rs:1265-1268`
**Description**: Both `enqueue_task()` and `Task::wake_up()` call `resched_cpu()` for the same target CPU.
**Impact**: Minor performance waste from redundant IPI.

### [Low] F04-12: Fair select_task_rq hardcoded 32-bit shift for cpus_allowed
**File**: `sched/fair.rs:901`
**Description**: `1u32 << wake` panics if `wake >= 32`. Safe with MAX_CPUS=4 but not future-proof.

### [Low] F04-13: Stale comment in DL pick_next_cpu overflow path
**File**: `sched/deadline.rs:256`
**Description**: Comment says "More than 16 skipped" but buffer size is 64.

### [Low] F04-14: update_min_vruntime only considers leftmost entry
**File**: `sched/fair.rs:415-425`
**Description**: Linux considers `max(min_vruntime, min(curr.vruntime, leftmost.vruntime))`. Rux only checks leftmost. Minor fairness deviation possible.

### [Low] F04-15: dec_time_slice not atomic (load + store)
**File**: `sched/rt.rs:361-369`
**Description**: Uses `load()` then `store()` — not atomic. Should use `fetch_sub`. Safe by convention since only tick handler touches current task's slice.

### [Info] F04-16: SchedClass trait methods are stubs — all logic in sched.rs
**File**: `sched/class.rs`, `fair.rs`, `rt.rs`, etc.
**Description**: `SchedClass` trait methods are all no-ops. Actual scheduling logic in `sched.rs` operates directly on sub-queues. Trait abstraction is unused.

### [Info] F04-17: No RT throttling / bandwidth limiting
**File**: `sched/rt.rs`
**Description**: No RT throttling mechanism. A SCHED_FIFO task can monopolize CPU indefinitely, starving all lower-priority tasks.
**Linux**: `sysctl_sched_rt_runtime` defaults to 950ms/s.

---

## Batch 5: Process Management (9 files, ~4,800 lines) — 9/9 reviewed

### [Critical] [BUG] F05-07: `do_waitid` never produces `CLD_KILLED` — the check is dead code **[FIXED]**
**File**: `process/exit.rs:533-539, 569-574`
**Description**: When a zombie child's `exit_code` is negative (killed by signal), line 537 computes `result_code = (-raw_exit) as i32` which is positive. Then at line 569, `result_code >= 0` is always true. The `CLD_KILLED` branch at line 574 is unreachable dead code.
**Linux**: Linux `wait_task_zombie()` correctly distinguishes `CLD_EXITED` from `CLD_KILLED` by checking `exit_code & 0x7f` (signal field).
**Impact**: `waitid()` always reports `CLD_EXITED` for signal-killed processes. `si_code` will be `CLD_EXITED (1)` instead of `CLD_KILLED (2)`.

### [High] [BUG] F05-02: `new_idle_at` does not initialize several fields with Drop implementations
**File**: `process/task.rs:716-923`
**Description**: `new_idle_at` does not initialize `comm`, `exe_path` (`Box<[u8]>`), `sem_undo` (`Spinlock<Vec<...>>`), `itimer_ids`, `posix_timers` (`Spinlock<Vec<...>>`), and `kernel_stack_bottom`. Fields with Drop implementations (`Box`, `Spinlock<Vec>`) will cause UB if idle task is ever dropped.
**Linux**: Linux uses `INIT_TASK` with all fields explicitly initialized.
**Impact**: Dormant UB — currently safe because idle tasks are never destroyed.

### [High] [POSIX] F05-11: AT_RANDOM contains deterministic values instead of random bytes
**File**: `process/exec.rs:401-402`
**Description**: The random bytes for `AT_RANDOM` are hardcoded as `0xdeadc0debeefcafe` and `0x123456789abcdef0`. These are completely deterministic, defeating the purpose of `AT_RANDOM` which provides 16 bytes of entropy for ASLR and security. musl libc uses `AT_RANDOM` to seed its stack canary (`__stack_chk_guard`).
**Linux**: Linux writes 16 bytes from `get_random_bytes()`.
**Impact**: All processes share the same stack canary value. Stack buffer overflow attacks become trivial.

### [High] [BUG] F05-12: `AT_EXECFN` points to argv[0] instead of the executable pathname
**File**: `process/exec.rs:387`
**Description**: `AT_EXECFN` is set to `argv_addrs.first().copied().unwrap_or(0)`, pointing to the first argv string. In Linux, `AT_EXECFN` must point to the executable's pathname string (the file path used in `execve`), not `argv[0]`.
**Linux**: Linux copies the binary name string to the stack and sets `AT_EXECFN` to it.
**Impact**: musl's dynamic linker uses `AT_EXECFN` to find the executable path. If `argv[0]` differs from the actual path (e.g., busybox symlinks), the linker may fail.

### [Medium] [BUG] F05-03: Type-punned ptr::write uses `Box<T>` instead of `Arc<T>`
**File**: `process/task.rs:830, 840, 844, 1036, 1048, 1052`
**Description**: Actual field types are `Option<Arc<...>>` but `ptr::write` casts to `*mut Option<Box<...>>`. Works by accident because both have identical 8-byte layout with `None`. Misleading and fragile.
**Impact**: No runtime bug currently (only `None` is written).

### [Medium] [POSIX] F05-08: `do_wait` with WUNTRACED reports the same stopped child repeatedly
**File**: `process/exit.rs:252-258`
**Description**: `do_wait` unconditionally reports any STOPPED child. A stopped child will be reported on every `waitpid(-1, &status, WUNTRACED)` call. POSIX requires `WUNTRACED` only reports a stop event once.
**Linux**: Uses `task_ptrace` and `jobctl` flags to track reported stops.
**Impact**: `waitpid` loop returns same child PID repeatedly.

### [Medium] [BUG] F05-13: All ELF segments mapped with RWX permissions
**File**: `process/exec.rs:135-138`
**Description**: Initial mapping for ELF segments uses `R|W|X|U|A|D` flags. Actual per-segment permissions are only recorded in VMA metadata. Hardware page table entries retain RWX, defeating W^X.
**Linux**: Maps each segment with exact permissions from ELF program header.
**Impact**: Security — data sections executable, code sections writable.

### [Medium] [BUG] F05-14: Resource leak if exec fails after address space allocation
**File**: `process/exec.rs:132-144`
**Description**: After `create_user_address_space()` succeeds, any later failure returns error without freeing page tables. Physical pages leaked.
**Linux**: Cleans up `mm_struct` on failure via `mm_release()` / `mmdrop()`.
**Impact**: Memory leak on failed execve calls.

### [Medium] [BUG] F05-17: `CLONE_CHILD_SETTID` incorrectly sets `clear_child_tid`
**File**: `process/fork.rs:324`
**Description**: `CLONE_CHILD_SETTID` block calls `set_clear_child_tid()`. Should only write TID to `child_tid`, not set `clear_child_tid`. Setting `clear_child_tid` is exclusively `CLONE_CHILD_CLEARTID`'s job.
**Linux**: Sets `set_child_tid` and `clear_child_tid` independently.
**Impact**: Spurious futex wakes in pthread implementations.

### [Medium] [DESIGN] F05-20: `kthread_stop` does not actually wait for thread to exit
**File**: `process/kthread.rs:166-187`
**Description**: `kthread_stop()` returns 0 immediately. Linux's version blocks until target thread exits via completion variable.
**Linux**: Uses `wait_for_completion()`.
**Impact**: Callers expecting to wait for thread proceed immediately, potential use-after-free.

### [Low] [DESIGN] F05-01: Comment says O(log N) but hash lookup is O(N) per bucket
**File**: `process/mod.rs:52`
**Description**: Doc comment claims "O(log N)" but hash table has linear scan per bucket.

### [Low] [DESIGN] F05-04: `new_task_at` / `new_idle_at` manual field-by-field construction
**File**: `process/task.rs:716-1162`
**Description**: ~300 lines each of manual `ptr::write(offset_of!(...))`. Extremely verbose, error-prone.

### [Info] [DESIGN] F05-05: `kernel_stack_bottom` not atomic but read concurrently
**File**: `process/task.rs:446`
**Description**: Plain `usize` read from trap/interrupt context while another CPU may be forking. Low risk on single-CPU.

### [Low] [DESIGN] F05-06: `wake_up` exclusive entry semantics differ from Linux
**File**: `process/wait.rs:154-158`
**Description**: Breaks on first exclusive entry regardless of `nr` parameter. Linux counts exclusive entries against `nr_exclusive`.

### [Low] [DESIGN] F05-09: `release_task` frees fork_pt_regs from heap inconsistently
**File**: `process/exit.rs:47-57`
**Description**: Comment says heap-allocated but fork.rs says stack-based. Guarded by null check.

### [Info] [POSIX] F05-10: `do_exit` does not send SIGCHLD to parent
**File**: `process/exit.rs:183-193`
**Description**: Only wakes parent from wait queue. Parents using `SIGCHLD` signal handlers won't be notified.

### [Low] [POSIX] F05-15: Missing auxiliary vector entries expected by musl
**File**: `process/exec.rs:372-398`
**Description**: Missing `AT_SYSINFO_EHDR`, `AT_MINSIGSTKSZ`, `AT_HWCAP2`. `AT_HWCAP` is 0. musl works without these but no hardware acceleration.

### [Low] [DESIGN] F05-16: `set_start_stack` uses raw stack_top
**File**: `process/exec.rs:423`
**Description**: Records raw stack top before args placement. Linux records actual initial SP.

### [Low] [DESIGN] F05-18: `CLONE_CHILD_SETTID` writes from parent context
**File**: `process/fork.rs:329-336`
**Description**: Writes to child's memory from parent's page tables. Works with CLONE_VM but wrong for fork.

### [Low] [DESIGN] F05-19: Hash function is trivial modulo
**File**: `process/pid_hash.rs:38-40`
**Description**: `(pid as usize) % 256` — no diffusion for adversarial patterns.

### [Low] [DESIGN] F05-21: KthreadInfo never cleaned up when kernel thread exits
**File**: `process/kthread.rs:30`
**Description**: `KTHREAD_MAP` entries never removed. Grows without bound with kernel threads.

---

## Batch 6: Sync Primitives (8 files, ~2,900 lines) — 8/8 reviewed

### [Critical] [BUG] F06-02: Semaphore down()/up() deadlock — woken waiter can never acquire **[FIXED]**
**File**: `sync/semaphore.rs:79-128` and `267-282`
**Description**: `down()` slow path unconditionally does `fetch_sub(1)`, making count negative. `up()` does `fetch_add(1)` which only increments toward zero. After `up()`, woken waiter checks `count > 0` and loops back to sleep since count is still 0. Binary semaphore with one waiter requires two `up()` calls.
**Linux**: `__up()` does NOT increment count when waiters exist. Directly wakes waiter via `wake_q_add()`. Woken waiter's loop checks `waiter.up`, not count.
**Impact**: Semaphore deadlock — woken waiter sleeps forever after single `up()`.

### [Critical] [BUG] F06-15: Condvar wait() lost-wakeup race between add() and set_state() **[FIXED]**
**File**: `sync/condvar.rs:95-125`
**Description**: Sequence is: (1) `self.wait.add(entry)` (2) `set_state(INTERRUPTIBLE)` (3) `mutex.unlock()`. Between step 1 and 2, concurrent `signal()` can find the entry, set `woken=true`, call `wake_up_process(task)` — but task state is still RUNNING, so wake fails. Step 2 sets INTERRUPTIBLE but wakeup already consumed. Task sleeps forever.
**Linux**: `prepare_to_wait()` holds waitqueue lock across list insertion AND state change.
**Impact**: Deadlock under concurrent condvar signal/wait.

### [High] [BUG] F06-16: Condvar wait_interruptible() same lost-wakeup race **[FIXED]**
**File**: `sync/condvar.rs:142-180`
**Description**: Same issue as F06-15. Uses manual `add()` + `set_state()` instead of `prepare_to_wait()`.
**Fix**: Use `self.wait.prepare_to_wait()` for atomic insertion + state change.

### [High] [BUG] F06-05: RwSpinlock reader count overflow corrupts writer bit
**File**: `sync/rwlock.rs:44-59`
**Description**: `compare_exchange_weak(s, s + 1, ...)`. If reader count is `0x7FFFFFFF`, `s + 1 = 0x80000000 = WRITER_BIT`. CAS succeeds, writer bit set without writer. Subsequent writers/readers spin forever.
**Linux**: Checks for max readers before CAS.
**Impact**: Theoretical — 2^31 concurrent readers impossible on RISC-V.

### [High] [BUG] F06-07: wake_up() calls wake_up_process() under waitqueue spinlock
**File**: `process/wait.rs:130-163`
**Description**: Holds waitqueue lock while calling `wake_up_process()` which acquires GRQ spinlock. Creates lock ordering: waitqueue → GRQ. If any path holds GRQ then waits on waitqueue, deadlock.
**Linux**: Collects tasks into `wake_q` under lock, then calls `wake_up_q()` outside lock.
**Fix**: Collect task pointers into local array, drop lock, then wake each task.

### [Medium] [BUG] F06-03: down_interruptible shares same deadlock as down() **[FIXED]**
**File**: `sync/semaphore.rs:160-204`
**Description**: Same issue as F06-02 applies to `down_interruptible()`. Post-wakeup CAS loop has identical failure mode.

### [Medium] [BUG] F06-08: wait_event_interruptible returns bool, no -ERESTARTSYS
**File**: `process/wait.rs:278-323`
**Description**: Returns `true`/`false` instead of `0`/`-ERESTARTSYS`. Callers must manually translate. Risk of silent success on signal.
**Linux**: Returns `-ERESTARTSYS` on signal, 0 on success.

### [Medium] [BUG] F06-11: futex_requeue doesn't hold bucket2 lock, waiter leak window
**File**: `sync/futex.rs:609-621`
**Description**: Between Phase 1 (unlink from bucket1) and Phase 3 (insert into bucket2), requeued entries are in limbo. If `futex_cleanup` runs for waiting task, entry is orphaned, leaking waiter slot.
**Linux**: Holds both hash bucket locks via `double_lock_hb()`.

### [Low] [DESIGN] F06-01: lock_irq and lock_irqsave are identical
**File**: `sync/spinlock.rs:182-197`
**Description**: Both implementations identical. Semantic distinction only.

### [Low] [DESIGN] F06-04: Semaphore down_trylock lock-free unlike Linux
**File**: `sync/semaphore.rs:236-248`
**Description**: Uses atomic `fetch_sub` without spinlock. Briefly negative count can cause spurious wake attempt. Benign.

### [Low] [DESIGN] F06-06: RwSpinlock write() starvation — no fairness guarantee
**File**: `sync/rwlock.rs:82-98`
**Description**: Writer spins on `state != 0`. Under continuous reader load, writer starves indefinitely.
**Linux**: `qrwlock` uses writer-pending flag for fairness.

### [Low] [DESIGN] F06-10: futex_wait returns 0 on spurious wakeup
**File**: `sync/futex.rs:328-342`
**Description**: Correct behavior — user must re-check. Comment misleading.

### [Low] [DESIGN] F06-12: FUTEX_WAKE_OP stub — musl compat risk
**File**: `sync/futex.rs:653-656`
**Description**: Dispatched to plain `futex_wake()` without implementing atomic operation on uaddr2. Used by some musl versions for condvar.

### [Low] [DESIGN] F06-13: Futex hash function is weak
**File**: `sync/futex.rs:128-131`
**Description**: `uaddr.wrapping_add(pid) % HASH_SIZE` — poor distribution for page-aligned allocations.

### [Low] [INFO] F06-14: ENOMEM defined locally instead of using errno module
**File**: `sync/futex.rs:443`
**Description**: `const ENOMEM: i32 = 12;` while other errnos imported from errno module.

---

## Batch 7: FS Core (23 files, ~7,500 lines) — 23/23 reviewed

### [High] [BUG] F07-01: Pipe read/write race condition — buffer lock released between check and read
**File**: `fs/pipe.rs:191-198`
**Description**: `pipe_read()` checks `is_write_closed()` and `available_read()` in separate lock acquisitions. Between check and actual read, write end could close or data could be consumed. Same TOCTOU in `pipe_file_read()`.
**Linux**: Linux holds `pipe_lock` across entire check-then-read sequence.
**Impact**: Data corruption or missed EOF in multi-threaded pipe scenarios.

### [High] [BUG] F07-02: File::set_flags data race with concurrent reads
**File**: `fs/file.rs:148-152`
**Description**: Uses raw `UnsafeCell` write without synchronization. Multiple threads sharing fd table (CLONE_FILES) can race on fcntl F_SETFL.
**Linux**: Uses `f_lock` spinlock for F_SETFL.
**Impact**: Data race with shared fd tables — undefined behavior.

### [High] [BUG] F07-03: Pipe double-free — both read and write ends free the same Pipe
**File**: `fs/pipe.rs:401-406`
**Description**: Both close paths see `is_read_closed() && is_write_closed()` and both call `Box::from_raw(pipe_ptr)`. No reference counting on the Pipe structure.
**Linux**: Uses reference counting (`pipe_inode_info` with kref).
**Impact**: Double-free causing memory corruption or kernel panic.

### [High] [BUG] F07-04: RootFS inode private_data holds raw Arc pointer without keeping Arc alive
**File**: `fs/rootfs.rs:1198, 1258, 1364, 1400, 1444, 1736`
**Description**: `Arc::as_ptr(&root_node)` extracts raw pointer but Arc is dropped when local goes out of scope. `private_data` holds dangling pointer.
**Linux**: Stores proper reference via `dentry->d_inode`.
**Impact**: Use-after-free when accessing RootFS inodes through VFS layer.

### [High] [BUG] F07-05: DevFS iget leaks Arc via into_raw with no matching from_raw
**File**: `fs/devfs/mod.rs:424-425`
**Description**: `Arc::into_raw(child_arc)` prevents drop but no matching `Arc::from_raw()` on inode free. Memory leak for every devfs inode lookup.
**Impact**: Memory leak growing with inode lookup rate.

### [Medium] [BUG] F07-06: reg_file_write does not enforce O_APPEND semantics
**File**: `fs/file.rs:471-486`
**Description**: Does not check for `O_APPEND` flag. Every write should atomically move position to end. Current code writes at current position.
**Linux**: `generic_write_checks()` forces offset to `i_size` for append.
**Impact**: O_APPEND files overwrite data instead of appending.

### [Medium] [BUG] F07-07: SEEK_CUR overflow in reg_file_lseek
**File**: `fs/file.rs:490-504`
**Description**: `current_pos + offset` can overflow `isize`. Wrapped value could appear positive while being semantically wrong.
**Linux**: Uses `loff_t` (u64) and checks for overflow explicitly.

### [Medium] [BUG] F07-08: Dup2 does not validate oldfd before close — TOCTOU race
**File**: `fs/file.rs:359-373`
**Description**: `get_file(oldfd)` then `close_fd(newfd)` then `install_fd()` — between steps, another thread could modify the fd table.
**Linux**: Holds fdtable lock throughout dup2.

### [Medium] [BUG] F07-09: Path::join() does not actually join paths
**File**: `fs/path.rs:148-154`
**Description**: Always returns `self.path` regardless of `other`. Function is a no-op. Currently no callers, but latent bug.

### [Medium] [BUG] F07-10: LOOKUP_PARENT has same value as LOOKUP_DOWN
**File**: `fs/path.rs:38-39`
**Description**: `LOOKUP_PARENT = 0x0010` and `LOOKUP_DOWN = 0x0010` — same value for different semantics.
**Linux**: `LOOKUP_PARENT = 0x2000`.
**Impact**: Flag collision — LOOKUP_PARENT cannot be distinguished from LOOKUP_DOWN.

### [Medium] [BUG] F07-11: RootFS symlink following does not follow final symlink component
**File**: `fs/rootfs.rs:1082`
**Description**: `lookup_follow()` only follows symlinks when `i < components.len() - 1`. Never on the final component. `open("/sym")` returns symlink node instead of target.
**Linux**: Always follows symlinks on final component unless O_NOFOLLOW.
**Impact**: Symlinks at end of path won't resolve to targets.

### [Medium] [POSIX] F07-12: F_DUPFD does not honor minimum fd argument
**File**: `fs/vfs.rs:1375-1382`
**Description**: Allocates lowest available fd, then checks if `fd >= min_fd`. Should allocate starting from `min_fd`.
**Linux**: Scans from `arg` upward to find lowest free fd >= arg.
**Impact**: F_DUPFD with arg > 0 will fail incorrectly.

### [Medium] [POSIX] F07-13: getdents64 d_off is relative to current buffer
**File**: `fs/vfs.rs:1644`
**Description**: `d_off` set to offset within current buffer. Linux sets it to absolute offset from directory start. `seekdir()`/`telldir()` would break.

### [Medium] [POSIX] F07-14: O_CREAT file mode not filtered by umask
**File**: `fs/vfs.rs:1123`
**Description**: Mode passed directly to `create_fn()` without applying process umask. POSIX requires umask application.
**Linux**: `inode_init_owner()` applies `current_umask()`.

### [Medium] [POSIX] F07-15: Permission check missing supplementary groups
**File**: `fs/permission.rs:22-52`
**Description**: `generic_permission()` checks owner and group bits but does not iterate supplementary groups.
**Linux**: `in_group_p()` checks all supplementary groups.
**Impact**: Group permission checks fail for supplementary groups.

### [Medium] [DESIGN] F07-16: Dentry/inode cache uses single-slot hash (no chaining)
**File**: `fs/dentry.rs:409-444`
**Description**: Each hash bucket holds exactly one entry. Collision replaces old entry via LRU. Poor cache hit rates even when mostly empty.
**Linux**: Uses hash table with chaining (hlist).

### [Medium] [DESIGN] F07-17: Two separate do_mount implementations with different behavior
**File**: `fs/superblock.rs:287-304` and `fs/mount.rs:131-173`
**Description**: `superblock::do_mount()` uses FsRegistry. `mount::do_mount()` hardcodes type matching. Only `mount::do_mount()` is used. `superblock::do_mount()` is dead code.

### [Medium] [DESIGN] F07-18: build_path holds parent lock while acquiring child lock
**File**: `fs/dentry.rs:148-191`
**Description**: Lock ordering issue when walking dentry tree. Potential deadlock with concurrent opposite-direction traversal.

### [Low] [BUG] F07-19: Pipe wait uses schedule() without setting TASK_INTERRUPTIBLE
**File**: `fs/pipe.rs:256-266`
**Description**: Adds to wait queue and calls `schedule()` without setting state to INTERRUPTIBLE. May cause busy-wait loop.

### [Low] [BUG] F07-20: IoCompletion::wait() same missing TASK_INTERRUPTIBLE
**File**: `fs/io_completion.rs:59-79`
**Description**: Same pattern — `schedule()` without setting task state.

### [Low] [BUG] F07-21: File::close() casts const Arc pointer to mut
**File**: `fs/file.rs:227`
**Description**: `Arc::as_ptr() as *mut File` — UB if another Arc clone exists.

### [Low] [BUG] F07-22: FdTable hardcodes 1024 fd limit
**File**: `fs/file.rs:253`
**Description**: `[Option<Arc<File>>; 1024]`. Linux supports up to NR_OPEN (65536+).
**Impact**: Programs needing > 1024 fds will fail.

### [Low] [DESIGN] F07-23: FsRegistry uses Spinlock without irqsave
**File**: `fs/superblock.rs:225`
**Description**: Could deadlock if register called from interrupt context.

### [Low] [DESIGN] F07-24: RootFS rename cycle detection broken for deep hierarchies
**File**: `fs/rootfs.rs:925-937`
**Description**: Only works at first level. Deeper directory hierarchies break out early.
**Impact**: Renaming directory into own subdirectory may succeed.

### [Low] [POSIX] F07-25: vfs_chown does not clear setuid/setgid bits
**File**: `fs/vfs.rs:958-990`
**Description**: POSIX requires clearing setuid/setgid on owner change. Rux does not clear these bits.
**Linux**: `inode->i_mode &= ~(S_ISUID|S_ISGID)` in `notify_change()`.

### [Low] [POSIX] F07-26: vfs_ftruncate does not check fd was opened for writing
**File**: `fs/vfs.rs:997-1028`
**Description**: No check that fd was opened O_WRONLY or O_RDWR.
**Linux**: Checks `!(file->f_mode & FMODE_WRITE)` in `do_ftruncate`.

### [Info] F07-27: eventfd/signalfd/timerfd/poll/random/memfd modules listed but not implemented
**Description**: Listed in task spec but no implementation files exist.

### [Info] F07-28: getdents64 does not add "." and ".." entries
**File**: `fs/vfs.rs:1599-1658`
**Description**: Consistent with filesystem readdir callbacks. Works for musl but technically violates POSIX.

---

## Batch 8: Ext4 + JBD2 (18 files, ~6,800 lines) — 18/18 reviewed

### [High] [BUG] F08-01: `Ext4SuperBlockOnDisk` field `s_frags_per_group` should be `s_clusters_per_group`
**File**: `fs/ext4/superblock.rs:28`
**Description**: Field name uses ext2/3 legacy terminology. ext4 uses `s_clusters_per_group`. No runtime impact since field unused.

### [High] [BUG] F08-02: `read_inode` does not check `ino` against filesystem range
**File**: `fs/ext4/mod.rs:249-283`
**Description**: Computes block group from `ino` without validating `ino <= total_inodes`. Out-of-range `ino` causes out-of-bounds read from group descriptor table → kernel crash.
**Linux**: Validates against `s_inodes_count`.
**Impact**: Kernel crash with crafted inode number.

### [High] [BUG] F08-03: Fast symlink threshold inconsistency — `< 60` vs `<= 60`
**File**: `fs/ext4/mod.rs:734` vs `mod.rs:1378` vs `fs/ext4/inode.rs:186`
**Description**: One place uses `size < 60`, another `size <= 60`. Ext4 fast symlink threshold is exactly 60 bytes (`EXT4_N_BLOCKS * 4 = 15 * 4`). Strict `< 60` excludes 60-byte symlinks, forcing them into slow path which may read wrong data.
**Linux**: Uses `<= EXT4_FAST_SYMLINK_MAX_LEN`.

### [High] [BUG] F08-04: `find_dir_entry` breaks on `inode == 0` instead of skipping
**File**: `fs/ext4/namei.rs:1174`
**Description**: Deleted directory entries (`inode=0`) cause `break` instead of `continue`. Valid entries after deleted ones are never found.
**Linux**: `ext4_find_entry()` skips `inode == 0` entries.
**Impact**: Files after deleted entries in directory cannot be found. Data integrity bug.

### [High] [BUG] F08-05: `is_dir_empty` same `inode == 0` break bug
**File**: `fs/ext4/namei.rs:1411`
**Description**: Same as F08-04. Reports non-empty directory as empty if deleted entries precede valid ones.
**Linux**: `empty_dir()` skips `inode == 0`.
**Impact**: `rmdir` may delete non-empty directory → data loss.

### [High] [BUG] F08-06: `Ext4DirEntry::from_bytes` does not validate `rec_len`
**File**: `fs/ext4/dir.rs:36-64`
**Description**: Never validates `rec_len` is within remaining block data. Corrupted `rec_len` could cause out-of-bounds read.
**Linux**: Validates `rec_len >= 8` and `rec_len <= blocksize - offset`.

### [Medium] [BUG] F08-07: `Ext4InodeOnDisk` blocks high bits lost in write_inode roundtrip
**File**: `fs/ext4/inode.rs:51`
**Description**: `write_inode` writes `inode.blocks as u32`, losing high 32 bits from `osd2.l_i_blocks_high`. Files > 2TB lose block count.
**Linux**: Reads/writes `l_i_blocks_high` in `osd2`.

### [Medium] [BUG] F08-08: Group descriptor reads assume 64-byte struct for 32-bit descriptors
**File**: `fs/ext4/mod.rs:195-200`
**Description**: If `desc_size` is 32 (non-64bit fs), `*gd_ptr` reads 64 bytes from 32-byte descriptor. High fields contain garbage from adjacent descriptor.
**Linux**: Separate 32-bit and 64-bit descriptor accessors.

### [Medium] [BUG] F08-09: write_inode/read_inode only use `bg_inode_table_lo`
**File**: `fs/ext4/inode.rs:327, 408, 511`
**Description**: Ignores high 32 bits in `bg_inode_table_hi`. Needed for filesystems > 16TB.
**Linux**: Combines hi/lo when 64-bit feature enabled.

### [Medium] [DESIGN] F08-10: No inode cache — every lookup re-reads inode table from disk
**File**: `fs/ext4/mod.rs:249-283`
**Description**: Every `read_inode` calls `bio::bread`. VFS icache exists but path resolution calls read_inode for each component.
**Linux**: `iget`/`iput` avoids redundant disk reads.
**Impact**: Performance — redundant I/O.

### [Medium] [BUG] F08-12: `create_vfs_inode` leaks Box::into_raw Ext4Inode
**File**: `fs/ext4/mod.rs:1773-1774`
**Description**: `Box::into_raw(ext4_copy)` stored in `inode.sb`. Never freed when VFS inode destroyed. Memory leak per inode creation.
**Linux**: Frees `ext4_inode_info` on inode destroy.

### [Medium] [BUG] F08-13: `get_or_create_ra_state` leaks Box if file closed without close
**File**: `fs/ext4/file.rs:813-823`
**Description**: `Box::into_raw(ReadAheadState)` freed in `ext4_file_close`, but leaked if file dropped without close. Race condition with concurrent calls.

### [Medium] [BUG] F08-14: ext4_file_write_vfs modifies cached Ext4Inode via const pointer
**File**: `fs/ext4/file.rs:775-783`
**Description**: Casts `inode.sb` (`*const u8`) to `*mut Ext4Inode` and writes. Violates Rust aliasing rules. Data race if concurrent `ext4_getattr`.

### [Medium] [BUG] F08-15: `add_dir_entry` tail insertion may corrupt directory block
**File**: `fs/ext4/mod.rs:1183-1207`
**Description**: Splits previous entry without checking its `rec_len` actually spans to `offset`. Overlap possible with fragmented directories.
**Linux**: `ext4_add_dirent_to_buf()` carefully validates boundaries.

### [Low] [DESIGN] F08-16: SuperBlock on-disk layout missing MMP fields
**File**: `fs/ext4/superblock.rs:126-128`
**Description**: Missing `s_mmp_update_interval` and `s_mmp_block`. All subsequent fields at wrong offsets. Unused fields, no runtime impact.

### [Low] [DESIGN] F08-17: UIDs > 65535 truncated — osd2 uid/gid high bits ignored
**File**: `fs/ext4/inode.rs:109-110, 133-134`
**Description**: `from_disk` reads only `i_uid` (u16). Ignores `l_i_uid_high` in osd2. Same for gid.
**Linux**: Uses hi/lo combination for 32-bit UID/GID.

### [Low] [DESIGN] F08-18: Global CURRENT_JOURNAL_HANDLE not SMP-safe
**File**: `fs/ext4/namei.rs:37`
**Description**: Single `AtomicUsize` for transaction handle. Documented limitation.

### [Low] [DESIGN] F08-19: Journal commit does not compute checksums
**File**: `fs/jbd2/commit.rs:161, 254-259`
**Description**: `h_chksum = [0; 8]`. Checksums never calculated. Reduces crash recovery corruption detection.

### [Low] [DESIGN] F08-20: Revoke module is placeholder stubs
**File**: `fs/jbd2/revoke.rs`
**Description**: All functions return `Ok(())` or `None`. Revoke prevents replay of freed-and-reused blocks. Without it, crash recovery may corrupt data.

### [Low] [DESIGN] F08-21: Checkpoint module is placeholder stubs
**File**: `fs/jbd2/checkpoint.rs`
**Description**: `jbd2_log_do_checkpoint` doesn't write out buffers. Journal space never reclaimed. Fills up after limited writes.

### [Low] [DESIGN] F08-22: Duplicate journal_start/journal_stop implementations
**File**: `fs/jbd2/journal.rs:776-824` and `fs/jbd2/transaction.rs:135-255`
**Description**: Two implementations exist. Only transaction.rs version used. journal.rs version is dead code.

### [Low] [DESIGN] F08-23: ext4_file_write invalidates page cache after write
**File**: `fs/ext4/file.rs:789`
**Description**: Discards all cached pages including just-written data. Sequential write-then-read must re-read from disk.
**Linux**: Updates page cache pages, doesn't invalidate.

### [Low] [BUG] F08-25: `ext4_ext_get_block` doesn't validate extent entries against block boundary
**File**: `fs/ext4/extent.rs:135-146`
**Description**: No check that `eh_entries * sizeof(Ext4Extent)` fits within i_block. Corrupted `eh_entries` causes out-of-bounds read.

### [Info] [DESIGN] F08-26: `ext4_file_lseek` is unused dead code
**File**: `fs/ext4/file.rs:616-638`
**Description**: VFS uses `reg_file_lseek` instead. `SEEK_CUR` returns `FunctionNotImplemented`.

### [Info] [DESIGN] F08-27: `Ext4BlockIterator` and `BlockMappingLayer` unused stubs
**File**: `fs/ext4/indirect.rs:13-76`
**Description**: Defined but never used. Future block iteration interface stubs.

### [Info] [DESIGN] F08-28: `max_file_size`/`get_indirect_level` only used in tests
**File**: `fs/ext4/indirect.rs:213-251`
**Description**: Correct utility functions, but no runtime callers.

---

## Wave 2 Summary

| Batch | Files | Critical | High | Medium | Low | Info | Total |
|-------|-------|----------|------|--------|-----|------|-------|
| Batch 5: Process | 9 | 1 | 3 | 6 | 10 | 2 | 22 |
| Batch 6: Sync | 8 | 2 | 3 | 3 | 7 | 0 | 15 |
| Batch 7: FS Core | 23 | 0 | 5 | 11 | 8 | 2 | 26 |
| Batch 8: Ext4/JBD2 | 18 | 0 | 6 | 8 | 10 | 3 | 27 |
| **Wave 2 Total** | **58** | **3** | **17** | **28** | **35** | **7** | **90** |

---

## Batch 9: ProcFS (11 files, ~1,726 lines) — 11/11 reviewed

### [Critical] [BUG] F09-11: SumGuard uses t6 (x31) register without preserving it — clobbers user register **[FIXED]**
**File**: `fs/procfs/pid.rs:27-33` and `39-47`
**Description**: Inline assembly uses `t6` (x31) to load SUM bit mask but does NOT list `t6` in clobbers. Compiler may allocate a variable to `t6` before/after asm block and lose its value. Should add `out("t6") _` to asm block.
**Impact**: Potential data corruption if compiler allocated value to `t6`.

### [High] [BUG] F09-01: Duplicate PID entry in /proc root directory listing
**File**: `fs/procfs/mod.rs:316-332` and `1135-1146`
**Description**: `list_children()` adds current process as PID directory entry. Then `procfs_readdir()` adds ALL processes from `pid_hash_collect_all()`. Current process appears twice.
**Linux**: Lists all PIDs exactly once.
**Impact**: `ls /proc` shows duplicate entries. `ps` may double-count.

### [High] [BUG] F09-02: /proc lookup returns None for all PID directories
**File**: `fs/procfs/mod.rs:466-471`
**Description**: `lookup()` checks `is_pid_dir()` and returns `None` with TODO comment. PID subdirectory lookups always fail via this path.
**Linux**: `proc_lookup` handles PID directories directly.

### [High] [BUG] F09-28: generate_mountinfo produces hardcoded static output
**File**: `fs/procfs/mounts.rs:47-61`
**Description**: Returns hardcoded string with three static entries (rootfs, proc, devtmpfs). Does not read actual mount information. Stale if filesystems mounted after boot.
**Linux**: Generates dynamically from mount namespace.
**Impact**: `/proc/mountinfo` shows wrong data after additional mounts.

### [High] [POSIX] F09-30: Load average not exponentially decayed — all three values identical
**File**: `fs/procfs/loadavg.rs:14-17`
**Description**: Returns `(load, load, load)` — identical 1/5/15 min averages. Linux uses exponential moving averages over 1, 5, 15 minute windows.
**Linux**: `get_avenrun()` with exponential decay.
**Impact**: `uptime`, `top`, `htop` show identical load averages — no historical information.

### [Medium] [BUG] F09-03: ref_count fetch_sub returns old value, not new value
**File**: `fs/procfs/mod.rs:340-342`
**Description**: `put()` returns pre-decrement value. Callers checking `put() == 0` will never trigger. Should check `== 1` for last-reference detection.

### [Medium] [DESIGN] F09-04: list_children adds only current PID, not all processes
**File**: `fs/procfs/mod.rs:323-329`
**Description**: Only adds `current_pid()`. Inconsistent with `procfs_readdir` which adds all PIDs.

### [Medium] [BUG] F09-05: procfs_file_close race condition on concurrent access
**File**: `fs/procfs/mod.rs:1004-1015`
**Description**: Nulls private_data then frees Box. If read/lseek in progress, use-after-free. Safe on single-core non-preemptible.

### [Medium] [DESIGN] F09-06: Inode number collision possible for PID files
**File**: `fs/procfs/mod.rs:749-751`
**Description**: `pid * 1000 + kind` — collisions possible when PIDs > ~1000. Discriminant values fragile.

### [High] [BUG] F09-12: User memory read in generate_cmdline/environ has no page-fault handling
**File**: `fs/procfs/pid.rs:207-218` and `332-342`
**Description**: Uses `SumGuard` + byte-by-byte `read_volatile` without validating pages mapped. Page fault on freed address space = kernel panic.
**Linux**: Uses `access_remote_vm()` with proper page fault handling.
**Impact**: Reading `/proc/[pid]/cmdline` for concurrently-exiting process can crash kernel.

### [Medium] [POSIX] F09-13: /proc/[pid]/status uses uid instead of fsuid in fourth Uid field
**File**: `fs/procfs/pid.rs:150`
**Description**: Outputs `uid, euid, suid, uid` — fourth field repeats real UID instead of fsuid.
**Linux**: Shows `uid, euid, suid, fsuid`.

### [Medium] [POSIX] F09-14: /proc/[pid]/status uses gid instead of fsgid in fourth Gid field
**File**: `fs/procfs/pid.rs:151`
**Description**: Same as F09-13 for group IDs.

### [Medium] [POSIX] F09-15: /proc/[pid]/stat field values mostly hardcoded to 0
**File**: `fs/procfs/pid.rs:244-261`
**Description**: Format string has ~52 fields but most are zero. `utime`, `stime`, `vsize`, `rss` etc. lack actual accounting data.
**Linux**: Fills all 52 fields from task struct.

### [Medium] [BUG] F09-16: generate_exe_link prepends "/" even for non-absolute paths
**File**: `fs/procfs/pid.rs:280-281`
**Description**: Formats as `format!("/{}", name_str)` producing `/toybox` instead of `/bin/toybox`.
**Linux**: Points to full absolute path.

### [Medium] [DESIGN] F09-17: parse_pid does not check for leading zeros or overflow
**File**: `fs/procfs/pid.rs:71-81`
**Description**: Accepts "00", "01" as valid PIDs. No length check — multiplication overflow possible.

### [Medium] [POSIX] F09-21: Missing "hart isa:" line from RISC-V cpuinfo format
**File**: `fs/procfs/cpuinfo.rs:12-40`
**Description**: Linux RISC-V `/proc/cpuinfo` includes per-hart "hart isa:" line. Rux omits it.
**Linux**: `cpu.c:367-368` prints ISA string.

### [Medium] [POSIX] F09-23: Idle time set equal to uptime — incorrect
**File**: `fs/procfs/uptime.rs:19`
**Description**: Always shows 100% idle. Should track actual CPU idle time.
**Linux**: Sums `get_idle_time()` across all CPUs.

### [Medium] [DESIGN] F09-25: MAX_CPUS redefined locally instead of using config::MAX_CPUS
**File**: `fs/procfs/interrupts.rs:17`
**Description**: `const MAX_CPUS: usize = 4` instead of importing from config.

### [Medium] [POSIX] F09-27: /proc/version format differs from Linux format
**File**: `fs/procfs/version.rs:16-20`
**Description**: Third field is KERNEL_VERSION repeated instead of build-time version string.

### [Low] [DESIGN] F09-07: size() regenerates entire file content on every call
**File**: `fs/procfs/mod.rs:281-287`

### [Low] [POSIX] F09-08: procfs_file_write returns EBADF instead of EINVAL/EPERM
**File**: `fs/procfs/mod.rs:974-976`

### [Low] [DESIGN] F09-09: Global ProcFS state uses AtomicPtr with leaked Box
**File**: `fs/procfs/mod.rs:517-522`

### [Low] [POSIX] F09-10: procfs root st_nlink is 1, should be 2 + nchildren
**File**: `fs/procfs/mod.rs:822`

### [Low] [DESIGN] F09-18: Task state check order misses combined states
**File**: `fs/procfs/pid.rs:87-97`

### [Low] [DESIGN] F09-19: list_fds iterates 1024 fd slots unconditionally
**File**: `fs/procfs/pid.rs:461`

### [Low] [POSIX] F09-20: Default /proc/cmdline contains synthetic BOOT_IMAGE entry
**File**: `fs/procfs/cmdline.rs:22`

### [Low] [DESIGN] F09-22: ISA string hardcoded to "rv64imafdc"
**File**: `fs/procfs/cpuinfo.rs:26`

### [Low] [DESIGN] F09-24: Timer frequency hardcoded to 10 MHz
**File**: `fs/procfs/uptime.rs:30`

### [Low] [DESIGN] F09-26: Interrupts output hardcodes IRQ ranges regardless of PLIC config
**File**: `fs/procfs/interrupts.rs:122`

### [Low] [DESIGN] F09-29: /proc/filesystems lists hardcoded types
**File**: `fs/procfs/mounts.rs:31-43`

### [Low] [DESIGN] F09-31: running_tasks hardcoded to 1
**File**: `fs/procfs/loadavg.rs:31`

### [Low] [DESIGN] F09-33: meminfo column alignment differs from Linux
**File**: `fs/procfs/meminfo.rs:47-119`

---

## Batch 10: Networking (13 files, ~6,000 lines) — 13/13 reviewed

### [Critical] [BUG] F10-03: Socket Arc reference count leaked via into_raw **[FIXED]**
**File**: `net/socket.rs:537`
**Description**: `Arc::into_raw(Arc::clone(&socket))` creates second Arc reference but `socket_close` never calls `Arc::from_raw` to reclaim it. Socket struct and all data never freed.
**Impact**: Memory leak on every socket creation.

### [Critical] [POSIX] F10-31: Socket creation fallback returns raw table index, not fd **[FIXED]**
**File**: `syscall/network.rs:40-74`
**Description**: Fallback path returns TCP/UDP internal table index as "fd", but never registers in process fd table. musl programs get EBADF on read/write/poll.
**Impact**: All musl network programs broken via fallback path.

### [Medium] [POSIX] F10-05: Socket::send drops UDP data with dest_addr silently
**File**: `net/socket.rs:244-245`
**Description**: `send()` with `dest_addr` immediately returns `Ok(buf.len())` without sending. `sendto()` on unconnected UDP socket silently drops data.

### [Medium] [POSIX] F10-06: Socket::accept always returns EAGAIN
**File**: `net/socket.rs:314-327`
**Description**: Never dequeues established connections. TCP servers cannot work.

### [Medium] [BUG] F10-11: ARP lookup byte order mismatch — cache always misses
**File**: `net/arp.rs:268-271, 506`
**Description**: Cache stores host-byte-order IPs but `resolve_ip` passes network-byte-order. ARP resolution always fails, forcing broadcast MAC.

### [Medium] [BUG] F10-13: ipv4_send uses hardcoded source IP 192.168.1.100
**File**: `net/ipv4/mod.rs:224`
**Description**: Source IP hardcoded as `0xC0A80164`. Ignores actual local IP configuration.

### [Medium] [BUG] F10-14: ipv4_send tot_len potential u16 overflow
**File**: `net/ipv4/mod.rs:213`
**Description**: `(IPHDR_LEN + skb.len) as u16` silently truncates for packets > 65535 bytes.

### [Medium] [BUG] F10-19: UDP checksum byte order incorrect
**File**: `net/udp.rs:438`
**Description**: `uhdr.len` is network byte order but used directly in checksum sum. Produces incorrect checksums.

### [Medium] [POSIX] F10-20: UDP receive doesn't check destination IP
**File**: `net/udp.rs:590-604`
**Description**: Matches only by port, ignores bound IP address. INADDR_ANY vs specific IP not distinguished.

### [Medium] [BUG] F10-21: TCP pseudo-header protocol field byte order error
**File**: `net/tcp.rs:1808`
**Description**: `sum += (6u32 << 8)` produces `0x0600` instead of `0x0006`. TCP checksum always incorrect. Peers reject our packets.

### [Medium] [BUG] F10-22: TCP retransmit uses snd_nxt instead of seg.seq
**File**: `net/tcp.rs:1283`
**Description**: Fast retransmit and timeout retransmit send data with wrong sequence number. TCP connection breaks under packet loss.

### [Medium] [BUG] F10-24: SYN retransmit creates duplicate pending connections
**File**: `net/tcp.rs:1434-1454`
**Description**: No dedup check — second SYN creates second TcpSocket. Peer receives multiple SYN-ACKs.

### [Medium] [BUG] F10-29: TCP timer/syscall data race on socket table
**File**: `net/tcp_timer.rs:156-159`
**Description**: Timer softirq and syscalls both access TCP_SOCKET_TABLE without locking. Data race on single-core if softirq preempts syscall.

### [Medium] [POSIX] F10-32: sys_bind tries TCP then UDP — may bind wrong socket type
**File**: `syscall/network.rs:126-138`
**Description**: Fallback path matches first table regardless of socket type. fd=N could match both TCP and UDP tables.

### [Medium] [POSIX] F10-33: sys_connect fallback only supports TCP, no UDP
**File**: `syscall/network.rs:221-226`

### [Medium] [BUG] F10-07: socket_close fragile Arc/raw pointer ownership
**File**: `net/socket.rs:395-418`

### [Medium] [BUG] F10-02: SkBuff implements Send but not Sync — future SMP issue
**File**: `net/buffer.rs:127`

### [Low] [DESIGN] F10-01: SKBUFF_ALLOCATOR_ID unused
**File**: `net/buffer.rs:130`

### [Low] [DESIGN] F10-09: get_device_mac returns hardcoded MAC
**File**: `net/ethernet.rs:318-320`

### [Low] [POSIX] F10-10: Loopback polls only one packet per cycle
**File**: `net/ethernet.rs:405-407`

### [Low] [DESIGN] F10-12: ARP LRU eviction scans all entries
**File**: `net/arp.rs:230-238`

### [Low] [BUG] F10-15: ip_rcv doesn't validate IHL against packet length
**File**: `net/ipv4/mod.rs:287-294`

### [Low] [BUG] F10-16: Route tie-breaking uses >= instead of >
**File**: `net/ipv4/route.rs:114`

### [Low] [DESIGN] F10-17: route_output is no-op placeholder
**File**: `net/ipv4/route.rs:241-246`

### [Low] [BUG] F10-18: ICMP checksum code duplication (functionally correct)
**File**: `net/icmp.rs:59-90`

### [Low] [DESIGN] F10-25: TCP flags truncated to 8 bits
**File**: `net/tcp.rs:1905`

### [Low] [POSIX] F10-26: TCP receive doesn't validate checksum
**File**: `net/tcp.rs:1938-1955`

### [Low] [BUG] F10-27: close() doesn't start retransmit timer for FIN
**File**: `net/tcp.rs:1072-1086`

### [Low] [DESIGN] F10-30: Hardcoded 10ms jiffy conversion
**File**: `net/tcp.rs:436`

### [Low] [POSIX] F10-34: TCP send fallback silently drops data
**File**: `syscall/network.rs:276-277`

### [Info] [DESIGN] F10-04: static mut Spinlock pattern non-idiomatic
**File**: `net/socket.rs:498`

### [Info] [DESIGN] F10-28: Monolithic TCP state machine (intentional)
**File**: `net/tcp.rs`

---

## Batch 11: Syscalls (11 files, ~12,900 lines) — 11/11 reviewed

### [Critical] [BUG] F11-07: getdents64 SUM bit manipulation unsafe **[FIXED — merged into F11-01]**
**File**: `syscall/file.rs`
**Description**: getdents64 implementation manipulates SUM bit for user-space writes without proper guards.

### [Critical] [BUG] F11-16: Negative return truncation in io.rs **[FIXED — `as u32 as u64` → `as i32 as i64` in i64 refactor]**
**File**: `syscall/io.rs`
**Description**: Negative return values from syscalls truncated incorrectly, losing error information.

### [Critical] [BUG] F11-21: argv/envp manual SUM bit in process.rs
**File**: `syscall/process.rs`
**Description**: Manual SUM bit manipulation for argv/envp copy without proper safety guards.

### [High] Multiple: Direct user-space writes without copy_to_user
**Files**: `syscall/file.rs`, `syscall/signal.rs`, `syscall/time.rs`, `syscall/process.rs`
**Description**: Multiple syscall handlers write directly to user memory without using `copy_to_user`. Bypasses address validation and SUM bit management.

> **Note**: Batch 11 detailed findings (F11-01 through F11-52) require re-review for full detail. The agent completed review of all 11 syscall files identifying 3 Critical, 9 High, 30+ Medium, 10 Low findings. Key themes: inconsistent user-space memory access, POSIX compliance gaps in errno values, and missing edge case handling.

---

## Batch 11: Syscalls (11 files, ~12,900 lines) — 11/11 reviewed

### [Critical] [BUG] F11-01: sys_getdents64 manually manages SUM bit instead of using copy_to_user **[FIXED]**
**File**: `syscall/file.rs:280-299`
**Description**: Uses inline asm to manually set/clear SUM bit and `core::ptr::copy_nonoverlapping` instead of `copy_to_user`. Redundant, error-prone, and risks SUM bit corruption if interrupted.
**Linux**: Uses `copy_to_user` uniformly.
**Impact**: ABI — SUM bit state may leak, corrupting subsequent user/kernel memory accesses.

### [Critical] [BUG] F11-02: sys_tgkill returns EINVAL for signal 0 instead of succeeding **[FIXED]**
**File**: `syscall/process.rs:1462-1483`
**Description**: When `sig == 0`, `send_signal(tid, 0)` rejects `sig < 1` with EINVAL. Per POSIX, signal 0 should perform permission checks and return 0 on success. `sys_kill` handles this correctly but `sys_tgkill` does not.
**Linux**: `tgkill(2)` with sig=0 returns 0 if process exists and caller has permission.
**Impact**: POSIX — musl `pthread_kill` with sig=0 (thread liveness checks) gets EINVAL.

### [High] [POSIX] F11-03: sched_getaffinity return value does not match Linux ABI
**File**: `syscall/sched.rs:703-728`
**Description**: Returns `mask_len as u64` (user-provided size) instead of kernel's cpumask size. Linux returns `min(sizeof(cpumask_t), user_size)`.
**Impact**: ABI — musl may misinterpret the affinity mask size.

### [High] [BUG] F11-06: mmap/munmap/mremap/madvise/mincore/msync return positive errno **[FIXED — i64 refactor eliminated `as u64` cast ambiguity]**
**File**: `syscall/memory.rs` (multiple locations)
**Description**: All memory syscalls return `mmap_error::* as u64` — positive errno values (e.g., 12 for ENOMEM) instead of negative. musl interprets these as successful mappings at low addresses.
**Linux**: Returns `(unsigned long) -errno` on error.
**Impact**: ABI — **all memory allocation error detection broken** for musl binaries. This is the single most impactful finding.

### [High] [POSIX] F11-05: SchedAttr struct layout may not match Linux ABI
**File**: `syscall/sched.rs:173-186`
**Description**: Rux struct is 48 bytes; Linux's `sched_attr` with util clamp fields is 56 bytes. `sched_util_min/max` silently ignored.
**Impact**: ABI — newer user-space using util clamp gets values silently dropped.

### [Medium] [POSIX] F11-13: copy_argv_from_user/copy_envp_from_user manually manage SUM bit
**File**: `syscall/process.rs:118-146`
**Description**: Manual `csrs sstatus` / `csrc sstatus` for SUM bit. If page fault occurs between set and clear, SUM remains set. Should use `copy_from_user`.
**Linux**: Uses `copy_from_user` / `strncpy_from_user`.

### [Medium] [POSIX] F11-14: sys_fstatat/fchmodat error returns may use positive errno
**File**: `syscall/file.rs:239, 997-999`
**Description**: `Err(e) => e as i64 as u64` — if VFS returns positive errno, user space sees positive return value. Pattern should be `-(errno as i64) as u64`.

### [Medium] [BUG] F11-20: sys_sched_getaffinity ignores PID argument
**File**: `syscall/sched.rs:703-728`
**Description**: `let _pid = args[0] as u32` — PID is discarded. Always returns current CPU's mask regardless of target PID.
**Linux**: Returns -ESRCH for non-existent PIDs.

### [Medium] [BUG] F11-22: sys_prlimit64 always returns EPERM for set operations
**File**: `syscall/process.rs:1311-1312`
**Description**: Unconditionally returns -EPERM when new_rlim is non-null. Should allow setting within current hard limits without CAP_SYS_RESOURCE.
**Impact**: Daemons that raise RLIMIT_NOFILE will fail.

### [Medium] [POSIX] F11-37: sys_clock_getres returns hardcoded 1ns resolution for all clocks
**File**: `syscall/time.rs:269-288`
**Description**: Ignores clk_id, returns 1ns for all clocks. With 10 MHz timer, actual resolution is 100ns.
**Impact**: User space makes incorrect assumptions about timer precision.

### [Medium] [BUG] F11-38: sys_clock_getres ignores clock ID validation
**File**: `syscall/time.rs:269-270`
**Description**: `let _clk_id` — any clock ID accepted. Should return -EINVAL for unsupported clocks.
**Linux**: Returns -EINVAL for invalid clock IDs.

### [Medium] [BUG] F11-39: nanosleep does not validate tv_nsec range
**File**: `syscall/time.rs:162-166`
**Description**: No check that `tv_nsec` is in [0, 999,999,999]. POSIX requires -EINVAL for out-of-range values.

### [Medium] [BUG] F11-12: sys_kill with pid < 0 does not check if any process was found
**File**: `syscall/process.rs:573-583`
**Description**: `kill(-pgid, 0)` returns 0 even if no process is in the specified group. Linux returns -ESRCH.

### [Medium] [BUG] F11-25: sys_sendto TCP fallback silently drops data
**File**: `syscall/network.rs:274-277`
**Description**: Returns `data.len()` without sending. Simulates success but data is discarded.

### [Medium] [DESIGN] F11-16: sys_ioctl uses hardcoded fd >= 1000 for framebuffer detection
**File**: `syscall/io.rs:517`
**Description**: Magic number convention conflicts with processes having > 1000 open fds.

### [Medium] [POSIX] F11-18: sys_readlinkat returns EINVAL for null buffer, should return EFAULT
**File**: `syscall/file.rs:471-473`

### [Medium] [POSIX] F11-19: sys_fstatat does not reject unknown flags
**File**: `syscall/file.rs:193-242`

### [Medium] [BUG] F11-17: sys_close does not check fd < 0
**File**: `syscall/file.rs:132-148`

### [Low] [POSIX] F11-31: sys_preadv reads garbage from unused arg[4] on riscv64
**File**: `syscall/io.rs:806-808`
**Description**: On riscv64, offset is in single register. Code reads arg[4] as high 64 bits, creating 128-bit offset from garbage.

### [Low] [INFO] F11-32: getrandom uses insecure PRNG (LCG)
**File**: `syscall/misc.rs:1373-1385`

### [Low] [DESIGN] F11-34/35/36: epoll_wait/poll/pselect6 busy-wait with yield_cpu
**File**: `syscall/misc.rs`

### [Low] [INFO] F11-27: SyscallNo enum has incorrect numbers (unused for dispatch)
**File**: `syscall/mod.rs:35-241`

---

## Batch 12: IPC (6 files, ~3,400 lines) — 6/6 reviewed

### [High] [POSIX] F12-02: Semaphore value range not validated against SEMVMX
**File**: `ipc/sysv_sem.rs`
**Description**: `semop` does not check SEMVMX overflow. `SEM_UNDO` exit adjustments not bounded to 0..SEMVMX range.
**Linux**: Validates against `SEMVMX` (32767).

### [High] [POSIX] F12-03: semop SEM_UNDO adjustment not bounded
**File**: `ipc/sysv_sem.rs`
**Description**: SEM_UNDO adjustments on process exit not clamped to valid range.

### [High] [BUG] F12-12: msgsnd reads mtype from user before access_ok validation
**File**: `ipc/sysv_msg.rs`
**Description**: Reads 8-byte mtype from user pointer before validating the pointer. Potential security issue.

### [High] [POSIX] F12-34: POSIX MQ fd not integrated into process fd table
**File**: `ipc/posix_mq.rs`
**Description**: MQ file descriptors stored in separate PID-indexed global table. `close()`, `poll()`, `select()` from musl won't work.
**Linux**: Uses real file descriptors via anon_inode.

### [Medium] [POSIX] F12-04: semop does not check for negative result values
**File**: `ipc/sysv_sem.rs`

### [Medium] [POSIX] F12-07: shmget does not validate size against existing segment
**File**: `ipc/sysv_shm.rs`

### [Medium] [POSIX] F12-09: semget does not validate nsems against existing set
**File**: `ipc/sysv_sem.rs`

### [Medium] [POSIX] F12-10: msgctl IPC_SET does not check CAP_SYS_RESOURCE for large msg_qbytes
**File**: `ipc/sysv_msg.rs`

### [Medium] [POSIX] F12-15: IPC_SET uses hardcoded byte offsets instead of struct field access
**File**: `ipc/util.rs`

### [Medium] [BUG] F12-24: sys_shmdt TOCTOU race — VMA lock dropped before IPC lock acquired
**File**: `ipc/sysv_shm.rs`
**Description**: Between releasing VMA lock and acquiring IPC lock, VMA could be freed by another thread.

### [Medium] [BUG] F12-25: shm_detach_vma nattch count inconsistency
**File**: `ipc/sysv_shm.rs`
**Description**: Related to F12-24 — nattch count may be inaccurate due to race window.

### [Medium] [DESIGN] F12-26: IPC_INFO/SHM_INFO use raw byte offsets
**File**: `ipc/util.rs`, `ipc/sysv_shm.rs`

### [Medium] [POSIX] F12-27: shmctl missing capability checks
**File**: `ipc/sysv_shm.rs`

### [Medium] [DESIGN] F12-16: Multiple IPC_SET operations fragile with hardcoded offsets
**File**: `ipc/util.rs`

### [Medium] [DESIGN] F12-17: IPC struct layout assumptions not verified with repr(C)
**File**: Multiple IPC files

### [Medium] F12-05 through F12-08: Additional POSIX compliance gaps in shm/sem/msg operations

### [Medium] F12-18 through F12-23: Additional IPC edge cases and race conditions

### [Low] F12-28 through F12-38: Additional minor IPC issues

### [Info] F12-39 through F12-42: IPC observations

---

## Wave 3 Summary

| Batch | Files | Critical | High | Medium | Low | Info | Total |
|-------|-------|----------|------|--------|-----|------|-------|
| Batch 9: ProcFS | 11 | 1 | 4 | 10 | 11 | 0 | 26 |
| Batch 10: Network | 13 | 2 | 0 | 13 | 14 | 2 | 31 |
| Batch 11: Syscalls | 11 | 3 | 9 | ~30 | ~10 | 0 | ~52 |
| Batch 12: IPC | 6 | 0 | 4 | ~22 | ~12 | ~4 | ~42 |
| **Wave 3 Total** | **41** | **6** | **17** | **~75** | **~47** | **~6** | **~151** |

---

## Batch 13: Interrupts (8 files, ~1,700 lines) — 8/8 reviewed

### [Medium] [BUG] F13-01: NMI_MASK bit width mismatch with Linux (1-bit vs 4-bit)
**File**: `interrupt/preempt.rs:25`
**Description**: Rux defines NMI as 1 bit (bit 20). Linux uses 4 bits ([20:23]). Limits nesting depth.

### [Medium] [BUG] F13-02: `__do_softirq` runs with IRQs disabled, unlike Linux
**File**: `interrupt/softirq.rs:136-193`
**Description**: Never re-enables IRQs during softirq processing. Linux calls `local_irq_enable()` before dispatch, reducing interrupt latency.
**Linux**: Enables IRQs during softirq handler dispatch.
**Impact**: Increased interrupt latency during softirq processing.

### [Medium] [DESIGN] F13-03: `__do_softirq` lacks time-based termination
**File**: `interrupt/softirq.rs:39,151-180`
**Description**: Only uses MAX_SOFTIRQ_RESTART (10). Linux also checks 2ms time budget.

### [Medium] [DESIGN] F13-08: raise_softirq_irqoff doc comment about IRQ requirement is misleading
**File**: `interrupt/softirq.rs:113-122`
**Description**: Comment says "Caller must have IRQs disabled" but atomic operations make this unnecessary in Rux.

### [Low] [DESIGN] F13-04: handle_fasteoi_irq does not hold desc lock during flow
**File**: `interrupt/irqdesc.rs:387-420`

### [Low] [DESIGN] F13-06: tasklet_enable missing smp_mb__before_atomic barrier
**File**: `interrupt/tasklet.rs:95-102`

### [Low] [DESIGN] F13-09: SOFTIRQ_VEC and per-CPU arrays are static mut without proper abstraction
**File**: `interrupt/softirq.rs:55-75`

### [Low] [DESIGN] F13-10: ksoftirqd_fn stale BKL comment
**File**: `interrupt/ksoftirqd.rs:56-69`

### [Low] [DESIGN] F13-12: tasklet_kill busy-waits without yielding
**File**: `interrupt/tasklet.rs:194-210`

### [Low] [DESIGN] F13-14: SOFTIRQ_IN_PROGRESS recursion guard race window
**File**: `interrupt/softirq.rs:139-144`

### [Info] F13-11: ksoftirqd wake flag race is benign
### [Info] F13-13: irq_get_name uses lock() instead of lock_irqsave()
### [Info] F13-15: Lock ordering in handle_fasteoi_irq is safe
### [Info] F13-16: request_irq chip null check is correct

---

## Batch 14: Drivers (28 files, ~8,200 lines) — 28/28 reviewed

### [High] [BUG] F14-10: VirtIO init status validation not checked
**File**: `drivers/virtio/mod.rs:169-173`
**Description**: Status reads assigned to `_status` but never validated. Device may silently reject state transitions.

### [High] [BUG] F14-11: reset_desc_allocator creates stale avail ring entries
**File**: `drivers/virtio/virtio_pci.rs:958,1163`
**Description**: Resets descriptor allocator each I/O but avail ring retains stale entries. Fragile — breaks under concurrent I/O.

### [High] [BUG] F14-16: alloc_desc may return in-use descriptors under load
**File**: `drivers/virtio/queue.rs:555-579`
**Description**: `next_desc.fetch_add(1) % queue_size` wraps around. If all descriptors submitted but device hasn't completed, returns in-use descriptor. Violates VirtIO protocol.
**Impact**: Data corruption under high I/O load.

### [High] [BUG] F14-22: VirtIONetRegs struct has wrong padding/offsets (dead code)
**File**: `drivers/net/virtio_net.rs:16-51`
**Description**: Struct never used — all access via hardcoded offsets. Misleading dead code.

### [Medium] [BUG] F14-01: blkdev_read allocates unnecessary temp buffer; data may not reach caller
**File**: `drivers/blkdev/mod.rs:260-276`

### [Medium] [BUG] F14-24: refill_rx_buffers uses virtual address for DMA
**File**: `drivers/net/virtio_net.rs:570`
**Description**: `buf_ptr as u64` is virtual address. VirtIO DMA needs physical address via `virt_to_phys()`.
**Impact**: DMA writes to wrong address. Currently masked by identity mapping.

### [Medium] [BUG] F14-19: read_virtio_cap reads 32-bit fields as 16-bit
**File**: `drivers/virtio/virtio_pci.rs:155-161`
**Description**: Two single-byte reads combined as 16-bit. VirtIO PCI spec defines 32-bit values.

### [Medium] [BUG] F14-31: send_command non-volatile write to avail ring idx
**File**: `drivers/gpu/virtio_gpu.rs:660`
**Description**: Direct `avail.idx = ` without `write_volatile`. Compiler may optimize store.

### [Medium] [BUG] F14-38: virtio_input read_event same non-volatile avail ring write
**File**: `drivers/input/virtio_input.rs:406-415`

### [Medium] [BUG] F14-26: blit_rect underflow for small rectangles
**File**: `drivers/gpu/framebuffer.rs:127-136`

### [Medium] [DESIGN] F14-03: CLINT hardcodes hart count to 4
**File**: `drivers/intc/clint.rs:21-26`

### [Medium] [DESIGN] F14-14: Duplicate block device initialization via parallel probe paths
**File**: `drivers/virtio/probe.rs:51-101, 175-206`

### [Medium] [DESIGN] F14-18: read_block/write_block allocates new VirtQueue per I/O
**File**: `drivers/virtio/virtio_pci.rs:598-749`

### [Medium] [ABI] F14-28: fbdev_ioctl writes without validating user pointer
**File**: `drivers/gpu/fbdev.rs:225-229`

### [Medium] [ABI] F14-36: evdev_ioctl uses magic fd numbers (2000/2001) instead of real fds
**File**: `drivers/input/evdev.rs:268-269`

### [Low] F14-02 through F14-09, F14-15, F14-17, F14-20-21, F14-25, F14-27, F14-29-30, F14-32, F14-35, F14-37, F14-39: Additional design/low findings

### [Info] F14-33: PS/2 driver is dead code on RISC-V
### [Info] F14-34: InputEvent layout is actually ABI-compatible on RV64 (revised)

---

## Batch 17: Build Files (4 files) — 4/4 reviewed

### [Medium] [DESIGN] F17-01: parse_dot_config section splitting is fundamentally broken
**File**: `kernel/build.rs:33`
**Description**: Splits on first underscore — `kernel_hz` becomes section=`kernel`, key=`hz` instead of correct `[scheduler] kernel_hz`. Menuconfig workflow non-functional.

### [Medium] [DESIGN] F17-05: task_pool_size has no Kernel.toml entry
**File**: `kernel/build.rs:601`
**Description**: Constant is generated with default 16 but no TOML key exists to configure it.

### [Low] [DESIGN] F17-02: Stale ARM/PCI config keys in Kernel.toml not generated into config.rs
### [Low] [DESIGN] F17-03: ENABLE_GIC generated but never used (ARM interrupt controller)
### [Low] [DESIGN] F17-04: log_level read from wrong [debug] section, not [printk]
### [Low] [DESIGN] F17-06: build/Makefile declares run target but has no run rule
### [Low] [DESIGN] F17-07: debug target missing tcg,thread=single flag (timer bug regression)
### [Low] [DESIGN] F17-08: [performance] section in Kernel.toml is informational-only
### [Info] F17-09: Cargo.toml features aarch64/x86_64 exist but project is RISC-V only

---

## Batch 18: Linker Script (1 file) — 1/1 reviewed

### [Medium] [DESIGN] F18-01: Boot stack size hardcoded in linker.ld (256KB) and boot.S (0x40000), not from config
**File**: `arch/riscv64/linker.ld:66`, `arch/riscv64/boot.S:57`
**Description**: Three different values: linker.ld=256KB, boot.S=256KB (0x40000), Kernel.toml=64KB (boot_stack_size). Config value is misleading.

### [Medium] [BUG] F18-02: Missing PROVIDE(__global_pointer$) in linker script
**File**: `arch/riscv64/linker.ld`
**Description**: boot.S references `__global_pointer$` but linker script never defines it. Works by implicit linker behavior — fragile.

### [Low] [DESIGN] F18-03: .got section not explicitly placed
### [Low] [DESIGN] F18-04: .bss section missing explicit AT() directive
### [Low] [DESIGN] F18-05: No .got.plt or .dynamic sections handled

---

## Wave 4+5 Summary

| Batch | Files | Critical | High | Medium | Low | Info | Total |
|-------|-------|----------|------|--------|-----|------|-------|
| Batch 13: Interrupts | 8 | 0 | 0 | 4 | 6 | 5 | 15 |
| Batch 14: Drivers | 28 | 0 | 4 | 12 | 16 | 2 | 34 |
| Batch 17: Build | 4 | 0 | 0 | 2 | 6 | 1 | 9 |
| Batch 18: Linker | 1 | 0 | 0 | 2 | 3 | 0 | 5 |
| **Wave 4+5 Total** | **41** | **0** | **4** | **20** | **31** | **8** | **63** |

---

## Batch 15: Security/DFX/IO_uring (13 files, ~2,500 lines) — 13/13 reviewed

### [High] [BUG] F15-09: io_uring submit_sqes TOCTOU — re-reads sq_head from shared ring per iteration
**File**: `io_uring/mod.rs:471-497`
**Description**: Each loop iteration re-reads `head` from shared SQ ring via `read_volatile`. Malicious user can modify `sq_ring` head between iterations, causing double-processing or skipping of SQEs.
**Linux**: Linux reads `sq ring head` once on enter, advances locally, writes back only after all submissions complete.
**Impact**: Malicious user process can manipulate SQ ring head to cause double submission or skip SQEs.

### [Medium] [BUG] F15-10: io_uring submit_sqes re-reads sq_head from ring instead of caching
**File**: `io_uring/mod.rs:471-477`
**Description**: Same as F15-09 — `head` re-read inside `for` loop allows user-space to modify between iterations.

### [Medium] [DESIGN] F15-11: io_uring mmap_handler requires exact length match
**File**: `io_uring/mod.rs:418-419`
**Description**: Requires `length == region.size`. Linux allows `length >= required_size`.

### [Medium] [BUG] F15-12: io_uring SQE index not validated against sq_entries
**File**: `io_uring/mod.rs:483-488`
**Description**: `sqe_idx = array[head & mask]` — no bounds check. Malicious user can set index >= sq_entries, causing out-of-bounds read from sqes region.
**Linux**: Validates `READ_ONCE(ring->array[i]) < ctx->sq_entries`.

### [Medium] [BUG] F15-14: io_uring_op_close fd validation (previously fixed, confirmed correct)

### [Low] [DESIGN] F15-01: security_init uses static mut bool instead of AtomicBool
**File**: `security/mod.rs:88-94`

### [Low] [DESIGN] F15-03: LSM_CHAIN and LSM_COUNT are static mut
**File**: `security/lsm.rs:106-109`

### [Low] [DESIGN] F15-05: CapLsm SignalSend hook always returns 0 (correct by design)
**File**: `security/cap_lsm.rs:41-50`

### [Low] [POSIX] F15-06: Taint string missing 4 Linux 6.x flags (16 vs 20 chars)
**File**: `dfx/taint.rs:85`

### [Low] [DESIGN] F15-07: hung_task reports PID instead of comm name
**File**: `dfx/hung_task.rs:152-157`

### [Low] [DESIGN] F15-08: softlockup now_ns assumes 10MHz timebase
**File**: `dfx/softlockup.rs:43`

### [Low] [DESIGN] F15-13: IORING_FEAT_NODROP declared but not used
**File**: `io_uring/mod.rs:38`

### [Info] F15-02: can_send_signal does not check target_cred for null
### [Info] F15-04: lsm_count exposes internal state (diagnostic only)

---

## Batch 16: Tests (56 files, ~6,000 lines) — 56/56 reviewed

### [Medium] [BUG] F16-01: test_sys_ids modifies global UID to 1000 and may not restore
**File**: `tests/syscall_process.rs:260-283`
**Description**: Calls `sys_setuid(1000)` changing UID from root. If restore fails, subsequent tests run as non-root, causing unexpected failures.
**Impact**: Sequential test pollution — tests after test_sys_ids may run with UID 1000.

### [Medium] [DESIGN] F16-03: test_cow only tests constants, not actual COW semantics
**File**: `tests/mem_cow.rs:1-97`
**Description**: Verifies PAGE_SHIFT, PAGE_SIZE, getpid consistency — never actually fork + write + verify parent page unchanged.
**Impact**: Major coverage gap for core kernel feature (Copy-on-Write).

### [Medium] [DESIGN] F16-04: test_fork creates zombie children without reaping
**File**: `tests/fork.rs:29-43`
**Description**: Forks 3 children, never calls wait4. Same issue in test_boundary.rs (20 children) and test_smp_schedule.rs (5 tasks).
**Impact**: Zombie accumulation may cause subsequent test failures.

### [Low] [BUG] F16-02: test_sys_ioctl expects hardcoded 25x80 winsize
**File**: `tests/syscall_io.rs:271-289`

### [Low] [DESIGN] F16-05: test_execve only validates constants, doesn't test actual execve
**File**: `tests/execve.rs:1-88`

### [Low] [DESIGN] F16-06: Test framework uses SeqCst unnecessarily (single-threaded tests)
**File**: `tests/mod.rs:95-106`

### [Low] [DESIGN] F16-07: test_tcp_handshake sets state fields directly, no actual packet exchange
**File**: `tests/tcp_handshake.rs:1-252`

### [Info] [DESIGN] F16-08: Syscall number validation tests duplicated across 50+ files

---

## Wave 4+5 Final Summary

| Batch | Files | Critical | High | Medium | Low | Info | Total |
|-------|-------|----------|------|--------|-----|------|-------|
| Batch 13: Interrupts | 8 | 0 | 0 | 4 | 6 | 5 | 15 |
| Batch 14: Drivers | 28 | 0 | 4 | 12 | 16 | 2 | 34 |
| Batch 15: Security/DFX/IO_uring | 13 | 0 | 1 | 4 | 7 | 2 | 14 |
| Batch 16: Tests | 56 | 0 | 0 | 3 | 4 | 1 | 8 |
| Batch 17: Build | 4 | 0 | 0 | 2 | 6 | 1 | 9 |
| Batch 18: Linker | 1 | 0 | 0 | 2 | 3 | 0 | 5 |
| **Wave 4+5 Total** | **110** | **0** | **5** | **27** | **42** | **11** | **85** |

---

# Grand Summary — All 18 Batches

| Wave | Batches | Files | Critical | High | Medium | Low | Info | Total |
|------|---------|-------|----------|------|--------|-----|------|-------|
| Wave 1 | 1-4 | 71 | 4 | 7 | 26 | 20 | 4 | 61 |
| Wave 2 | 5-8 | 58 | 3 | 17 | 28 | 35 | 7 | 90 |
| Wave 3 | 9-12 | 41 | 6 | 17 | ~75 | ~47 | ~6 | ~151 |
| Wave 4+5 | 13-18 | 110 | 0 | 5 | 27 | 42 | 11 | 85 |
| **Total** | **18** | **280** | **13** | **46** | **~156** | **~144** | **~28** | **~387** |

## Critical Findings (must fix)

| ID | Subsystem | Title | Status |
|----|-----------|-------|--------|
| F02-01 | Signal | SignalFrame uc pointer off by 4 bytes (alignment padding) | **FIXED** |
| F03-01 | MM | get_zeroed_page writes to physical address, not virtual | **FIXED** |
| F04-01 | Scheduler | RT enqueue lacks on_rq guard — duplicate enqueue corrupts list | **FIXED** |
| F04-02 | Scheduler | DL dequeue matches by mutable deadline, misses entries | **FIXED** |
| F05-07 | Process | do_waitid never produces CLD_KILLED — dead code | **FIXED** |
| F06-02 | Sync | Semaphore down()/up() deadlock — woken waiter cannot acquire | **FIXED** |
| F06-15 | Sync | Condvar wait() lost-wakeup race between add() and set_state() | **FIXED** |
| F09-11 | ProcFS | SumGuard clobbers t6 (x31) register without declaring clobber | **FIXED** |
| F10-03 | Network | Socket Arc reference count leaked via into_raw | **FIXED** |
| F10-31 | Network | Socket creation fallback returns raw table index, not fd | **FIXED** |
| F11-01 | Syscall | sys_getdents64 manually manages SUM bit | **FIXED** |
| F11-02 | Syscall | sys_tgkill returns EINVAL for signal 0 | **FIXED** |
| F11-06 | Syscall | mmap/munmap/mremap return positive errno (not negative) | **FIXED** (i64 refactor) |

### Additional fixes from i64 refactor

| ID | Subsystem | Title | Status |
|----|-----------|-------|--------|
| F11-07 | Syscall | getdents64 SUM bit manipulation unsafe | **FIXED** (merged into F11-01) |
| F11-16 | Syscall | Negative return truncation in io.rs (`as u32 as u64`) | **FIXED** (i64 refactor) |
| F06-03 | Sync | down_interruptible shares same deadlock as down() | **FIXED** |
| F06-16 | Sync | Condvar wait_interruptible() same lost-wakeup race | **FIXED** |

## Top 10 Highest-Impact Fixes (recommended priority)

1. ~~**F11-06**: mmap-family returns positive errno — **all memory allocation error detection broken**~~ **FIXED**
2. ~~**F06-02**: Semaphore deadlock — woken waiter sleeps forever~~ **FIXED**
3. ~~**F06-15**: Condvar lost-wakeup — deadlock under concurrent signal/wait~~ **FIXED**
4. **F10-21**: TCP checksum byte order error — peers reject our packets
5. **F08-04/05**: ext4 dir entry inode==0 break — files disappear, rmdir deletes non-empty dirs
6. **F05-11**: AT_RANDOM hardcoded — stack canary identical across all processes
7. ~~**F02-01**: SignalFrame uc pointer off by 4 — SA_SIGINFO handlers read corrupted ucontext~~ **FIXED**
8. **F07-03**: Pipe double-free — kernel panic on pipe close
9. ~~**F09-11**: SumGuard t6 clobber — potential data corruption~~ **FIXED**
10. **F10-11**: ARP byte order mismatch — all outbound traffic uses broadcast MAC
