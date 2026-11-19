# Kernel Big Lock

## Overview

The Rux kernel currently uses the Kernel Big Lock (BKL) as its primary synchronization mechanism. This is a coarse-grained lock that ensures only one CPU can execute kernel code at any given time.

## Design Goals

1. **Simplicity**: Using a single lock simplifies concurrency control during early kernel development
2. **Correctness First**: Avoids deadlocks and data races caused by fine-grained locks
3. **Progressive Optimization**: Provides a clear path for future lock splitting

## Current Implementation

### Architecture Diagram

```
+-----------------------------------------------------------------+
|                        User Space Processes                      |
|  +----------+  +----------+  +----------+  +----------+         |
|  | Process A|  | Process B|  | Process C|  | Process D|         |
|  +----+-----+  +----+-----+  +----+-----+  +----+-----+         |
+-------+-------------+-------------+-------------+---------------+
        | syscall/    | page fault  | interrupt   | syscall
        | trap        |             |             |
        v             v             v             v
+-----------------------------------------------------------------+
|                    KERNEL_LOCK (spinlock)                        |
|  +---------------------------------------------------------+   |
|  |  ACQUIRE ------------------------------------- RELEASE |   |
|  |    |                                               |     |   |
|  |    v                                               v     |   |
|  |  +-------------------------------------------------+   |   |
|  |  |              Kernel Critical Section             |   |   |
|  |  |  - System call handling                          |   |   |
|  |  |  - Page fault handling                           |   |   |
|  |  |  - Interrupt handling                            |   |   |
|  |  |  - Scheduler operations                          |   |   |
|  |  +-------------------------------------------------+   |   |
|  +---------------------------------------------------------+   |
+-----------------------------------------------------------------+
```

### Lock Lifecycle

```
User mode execution
    |
    v
+-------------+
| trap_entry  | --- KERNEL_LOCK_ACQUIRE ---> Lock acquired
+-------------+
    |
    v
+-------------+
| trap_handler| --- Handle syscall/exception/interrupt
+-------------+
    |
    v
+-------------+
| trap_exit   |
+-------------+
    |
    v
+-------------+
|return_user  | --- KERNEL_LOCK_RELEASE ---> Lock released
+-------------+
    |
    v
User mode execution
```

### Code Implementation

#### 1. Lock Variable Definition (kernel/src/sync/kernel_lock.rs)

```rust
/// Global kernel big lock (simple spinlock)
#[no_mangle]
pub static mut KERNEL_LOCK: AtomicBool = AtomicBool::new(false);

/// Check if kernel big lock is currently held
#[inline]
pub fn is_locked() -> bool {
    unsafe { KERNEL_LOCK.load(Ordering::Acquire) }
}
```

#### 2. Assembly Macros (kernel/src/arch/riscv64/trap.S)

```asm
// KERNEL_LOCK_ACQUIRE - Acquire kernel big lock
// Uses amoswap.w.aq instruction with acquire semantics
.macro KERNEL_LOCK_ACQUIRE
    la t0, KERNEL_LOCK
    li t2, 1
1:
    amoswap.w.aq t1, t2, (t0)    // Atomic swap
    bnez t1, 1b                   // Spin wait
.endm

// KERNEL_LOCK_RELEASE - Release kernel big lock
// Uses amoswap.w.rl instruction with release semantics
.macro KERNEL_LOCK_RELEASE
    la t0, KERNEL_LOCK
    amoswap.w.rl zero, zero, (t0)  // Atomic write 0
.endm
```

#### 3. Usage Locations

| Location | Operation | Description |
|----------|-----------|-------------|
| trap_entry (user -> kernel) | ACQUIRE | Acquire lock when entering kernel |
| trap_exit -> .Lreturn_user | RELEASE | Release lock when returning to user mode |
| ret_from_fork -> .Lret_from_fork_user | ACQUIRE + RELEASE | Fork child first schedule |
| handle_timer_interrupt | is_locked() | Skip scheduling when lock held |
| handle_page_fault | RELEASE + schedule | Release lock before scheduling when process aborts |
| Task::sleep | RELEASE + schedule + ACQUIRE | Release before sleep, re-acquire after wakeup |
| do_exit | RELEASE + schedule | Release lock when process exits |

### Sleep/Block Handling

When a system call needs to sleep (waiting for I/O, futex, semaphore, etc.), it must:
1. **Release kernel big lock**: Allow other processes to execute
2. **Call schedule()**: Switch to another process
3. **Re-acquire lock after wakeup**: Continue executing system call

```rust
// Sleep function template
pub fn sleep(state: TaskState) {
    // Set sleep state
    current.set_state(state);

    // Release kernel big lock
    crate::sync::kernel_lock_release();

    // Schedule to yield CPU
    crate::sched::schedule();

    // Re-acquire kernel big lock after wakeup
    crate::sync::kernel_lock_acquire();
}
```

Handled sleep points:
- `Task::sleep()` - Generic sleep function
- `wait_event!` / `wait_event_interruptible!` - Wait queue macros
- `ConditionVariable::wait()` - Condition variable
- `Semaphore::down()` - Semaphore P operation
- `futex_wait()` - Futex wait
- `pipe_file_read()` / `pipe_file_write()` - Pipe read/write
- `do_exit()` - Process exit
- `yield_cpu()` - Voluntary CPU yield
- `handle_pending_signals()` - STOP state in signal handling

### Kernel Entry/Exit Path Analysis

#### Entry Paths (User Mode -> Kernel Mode)

| Path | Lock Operation | Description |
|------|----------------|-------------|
| trap_entry -> .Lfrom_user | ACQUIRE | Normal syscall/exception/interrupt |
| trap_entry -> .Lfrom_kernel | None | Kernel mode interrupt, no lock needed |
| trap_entry -> .Learly_boot | None | Early boot stage |
| ret_from_fork -> .Lret_from_fork_user | ACQUIRE | Fork child first schedule |
| switch_to_user (context_switch) | None | init process, starts from kernel mode |

#### Exit Paths (Kernel Mode -> User Mode)

| Path | Lock Operation | Description |
|------|----------------|-------------|
| trap_exit -> .Lreturn_user | RELEASE | Normal return to user mode |
| trap_exit -> .Lreturn_kernel | None | Return to kernel mode |
| ret_from_fork -> .Lret_from_fork_user | RELEASE | Fork child returns to user mode |
| switch_to_user (context_switch) | None | init process first entry to user mode |

#### Special Paths

1. **init process startup**:
   - Created from `init.rs`, `ctx.sp = 0`
   - Started via `context_switch` -> `switch_to_user`
   - Does not go through trap entry, so no lock acquired
   - Will acquire lock normally on first trap

2. **fork child process**:
   - Starts execution from `ret_from_fork`
   - First acquires lock (simulating trap entry), then releases lock (returning to user mode)
   - Ensures lock state is consistent with normal trap

3. **Process sleep/wakeup**:
   - Releases lock before sleep, re-acquires after wakeup
   - Ensures other processes can acquire lock during sleep

### Key Design Decisions

#### 1. Why use assembly macros instead of Rust functions?

Initially implemented using Rust functions, but this caused userspace program execution anomalies. The reason was that function calls affected register state (possibly due to compiler optimization or calling convention issues).

**Solution**: Implement lock operations directly using inline assembly in trap.S, avoiding function call overhead and potential register state issues.

#### 2. Memory Ordering

- **ACQUIRE**: Uses `.aq` modifier, ensuring memory operations after lock acquisition won't be reordered before lock acquisition
- **RELEASE**: Uses `.rl` modifier, ensuring memory operations before lock release won't be reordered after lock release

#### 3. Scheduler Protection

When the kernel big lock is held, timer interrupt handling does not trigger scheduling:

```rust
fn handle_timer_interrupt(regs: &mut PtRegs) {
    let is_locked = crate::sync::is_locked();
    // ... handle timer ...
    if crate::sched::need_resched() && !is_locked {
        crate::sched::schedule();  // Only schedule when lock not held
    }
}
```

## Performance Impact

### Current Limitations

1. **Single-core equivalent**: Even on multi-core systems, only one core can execute kernel code
2. **Long critical sections**: Lock held for entire system call duration, blocking other CPUs
3. **Interrupt latency**: Interrupts on other CPUs must wait for lock release

### Applicable Scenarios

- Early kernel development
- Single or dual-core systems
- Functional verification phase

## Lock Splitting Plan

### Phase 1: Interrupt Context Separation

**Goal**: Allow interrupt handlers to execute in parallel

**Approach**:
```
Current:
  KERNEL_LOCK -------------------------------------
              |    syscall    | interrupt | syscall |
              +---------------+----------+---------+

After split:
  KERNEL_LOCK ---------------+-------+-------------
              |    syscall    |       |   syscall   |
              +---------------+       +-------------+
  IRQ_LOCK    -----------------------+-------------
                              | intr  |    intr     |
                              +-------+------------+
```

**Requirements**:
- Introduce `IRQ_LOCK` to protect interrupt handling
- Interrupt handlers don't access shared data or use separate locks

### Phase 2: Subsystem Independent Locks

**Goal**: Different subsystems can execute in parallel

**Approach**:
```
+-------------------------------------------------------------+
|                      Lock Hierarchy                          |
+-------------------------------------------------------------+
|  +-------------+  +-------------+  +-------------+          |
|  | SCHED_LOCK  |  |  MM_LOCK    |  |  FS_LOCK    |  ...     |
|  | Scheduler   |  | Memory Mgmt |  | File System |          |
|  +-------------+  +-------------+  +-------------+          |
+-------------------------------------------------------------+
|  +-------------+  +-------------+  +-------------+          |
|  | PIPE_LOCK   |  | SOCKET_LOCK |  | SIGNAL_LOCK |  ...     |
|  | Pipe        |  | Network     |  | Signal      |          |
|  +-------------+  +-------------+  +-------------+          |
+-------------------------------------------------------------+
```

**Lock Granularity Plan**:

| Subsystem | Lock Name | Protected Content |
|-----------|-----------|-------------------|
| Scheduler | `sched_lock` | Run queues, process state |
| Memory Management | `mm_lock` | Page tables, VMA, address space |
| File System | `fs_lock` | inode, dentry, superblock |
| Block Device | `bio_lock` | Buffer cache |
| Network | `net_lock` | socket, protocol stack |
| Signal | `signal_lock` | Signal handling, pending signals |

### Phase 3: Fine-grained Locks

**Goal**: Each data structure has its own lock

**Approach**:

```rust
// Process-level lock
struct Task {
    lock: SpinLock,      // Protects single process fields
    // ...
}

// inode-level lock
struct Inode {
    lock: RwLock,        // Read-write lock, allows multiple readers single writer
    // ...
}

// Per-CPU data (no lock needed)
struct PerCpu<T> {
    data: [T; MAX_CPUS],  // Each CPU accesses independently
}
```

### Phase 4: RCU (Read-Copy-Update) — Implemented

**Status**: Implemented in Phase 48 (Tiny RCU) and Phase 49 (RCU PID Hash)

**Implementation** (`sync/rcu.rs`):
- Non-preemptible RCU: `rcu_read_lock` = `preempt_disable`, `rcu_read_unlock` = `preempt_enable`
- Per-CPU callback lists for deferred reclamation
- Softirq-driven callback processing (`RCU_SOFTIRQ`)
- Generation-counter grace period detection
- QS hooks in `__schedule` and `cpu_idle_loop`

**RCU PID Hash Table** (`process/pid.rs`):
- BTreeMap → RCU-protected chained hash table
- Lock-free lookup via `rcu_read_lock`/`rcu_read_unlock`
- Per-bucket spinlock for insert/remove
- `synchronize_rcu` in `release_task` for safe deferred reclamation

**Applicable Scenarios**:
- PID hash table lookup (implemented)
- Path lookup (dentry cache) — future
- Process list traversal — future
- Network routing table — future

### Phase 5: SeqLock — Implemented

**Status**: Implemented in Phase 50

**Implementation** (`sync/seqlock.rs`):
- `RawSeqLock`: odd/even sequence counter for writer serialization
- `SeqLock<T: Copy>`: generic wrapper with lock-free readers and retry-on-write
- `SeqLockWriteGuard`: RAII write guard, increments sequence on drop
- Used for read-mostly data (loopback stats, hugepage stats)

**Approach**:
```
Write operation:
  1. Acquire write guard (sequence becomes odd)
  2. Modify data
  3. Drop guard (sequence becomes even)

Read operation:
  1. Read sequence (seq1)
  2. Read data
  3. Read sequence (seq2)
  4. If seq1 != seq2 or seq1 is odd → retry
```

## Lock Splitting Implementation Guide

### Step 1: Identify Shared Data

```bash
# Find global static variables
grep -rn "static" kernel/src --include="*.rs" | grep -v "static fn"

# Find potential race conditions
grep -rn "unsafe" kernel/src --include="*.rs"
```

### Step 2: Determine Lock Hierarchy

1. **Draw dependency graph**: Identify which subsystems depend on each other
2. **Define lock order**: Avoid deadlocks, always acquire locks in same order
3. **Document**: Purpose and acquisition order of each lock

### Step 3: Progressive Replacement

```rust
// 1. Add new lock
static SCHED_LOCK: SpinLock = SpinLock::new();

// 2. Use new lock in lock-holding code
fn schedule() {
    let _guard = SCHED_LOCK.lock();
    // Original code...
}

// 3. Remove kernel big lock (after confirming safety)
// KERNEL_LOCK no longer protects scheduler
```

### Deadlock Prevention

**Lock Order Rules** (from outer to inner):
1. `KERNEL_LOCK` (will eventually be removed)
2. `SCHED_LOCK`
3. `MM_LOCK`
4. `FS_LOCK`
5. `INODE_LOCK`
6. `PAGE_LOCK`

**Prohibited**:
- Acquiring locks in reverse order
- Calling functions that may acquire other locks while holding a lock (unless explicitly allowed)

## Testing Strategy

### Concurrency Tests

```rust
#[test]
fn test_concurrent_syscalls() {
    // Multi-threaded concurrent system calls
    // Verify data consistency
}

#[test]
fn test_lock_contention() {
    // Measure lock contention
    // Ensure no deadlocks
}
```

### Performance Benchmarks

```rust
#[bench]
fn bench_syscall_with_bkl(b: &mut Bencher) {
    // Measure syscall latency with kernel big lock
}

#[bench]
fn bench_syscall_without_bkl(b: &mut Bencher) {
    // Measure syscall latency after lock splitting
}
```

## References

- Linux kernel locking mechanisms: `Documentation/locking/`
- RCU design: `Documentation/RCU/`
- Spinlock implementation: `arch/riscv/include/asm/spinlock.h`

## Change History

| Date | Version | Description |
|------|---------|-------------|
| 2026-03-06 | 1.0 | Initial version, implemented kernel big lock |
| 2026-03-06 | 1.1 | Improved lock handling in sleep/block paths, fixed do_exit, yield_cpu, etc. |
| 2026-04-09 | 1.2 | Updated RCU to implemented status (Phase 48/49), added SeqLock (Phase 50) |
