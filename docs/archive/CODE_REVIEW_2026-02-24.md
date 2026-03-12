# Rux Kernel Code Review Report

## Comparative Analysis with Linux Kernel

**Review Date**: 2026-02-24
**Last Updated**: 2026-03-04
**Comparison Version**: Linux 6.x (refer/linux)
**Review Scope**: Core kernel subsystems

---

## I. Overall Assessment

The Rux project has a reasonable overall architecture and maintains Linux ABI compatibility for external interfaces. However, compared to the Linux kernel, there are several areas that need refactoring and improvement.

**Implemented Core Features**:
- RISC-V Sv39 virtual memory management
- Process scheduling (Round Robin)
- VFS filesystem layer + ext4
- Basic system calls
- SMP multi-core support

**Major Gap Areas**:
- Data structure layouts not fully compatible with Linux
- Incomplete error handling paths
- Missing architecture abstraction layer
- Insufficient performance optimization

---

## II. Key Issues Requiring Refactoring

### 1. [P0] TrapFrame/pt_regs Structure Layout Inconsistency - FIXED

**File**: `kernel/src/arch/riscv64/pt_regs.rs` (newly created)
**Priority**: High
**Status**: - **FIXED** (2026-02-24)

**Linux Implementation** (`arch/riscv/include/asm/ptrace.h`):
```c
struct pt_regs {
    unsigned long epc;      // PC at the beginning
    unsigned long ra;
    unsigned long sp;
    unsigned long gp;
    unsigned long tp;
    unsigned long t0-t6;    // temporary registers
    unsigned long s0-s11;   // saved registers
    unsigned long a0-a7;    // argument registers
    unsigned long status;   // CSR
    unsigned long badaddr;  // CSR (stval)
    unsigned long cause;    // CSR (scause)
    unsigned long orig_a0;  // original a0 (needed for syscall rollback)
};
```

**Rux Current Implementation**:
```rust
pub struct TrapFrame {
    pub ra: u64,   // starting from sp+16
    pub t0-t6: u64,
    pub a0-a7: u64,
    pub s2-s11: u64,
    pub gp: u64,
    pub _pad: u64,  // extra padding field
    pub sstatus: u64,
    pub sepc: u64,
    pub stval: u64,
    // missing cause and orig_a0
}
```

**Problem List**:
1. Field order completely inconsistent with Linux
2. Missing `orig_a0` field (needed for syscall rollback)
3. Missing `cause` field (exception cause)
4. `sp` saved outside TrapFrame, adding complexity
5. Cannot use `task_pt_regs()` macro

**Refactoring Plan**:
- - Redesigned `PtRegs` structure with layout consistent with Linux `pt_regs`
- - Added `orig_a0` and `cause` fields
- - Updated `trap.S` assembly code to match new layout
- - Updated `fork.rs`, `task.rs`, `sched.rs`, `usermod.rs` to use new structure

**Fix Details**:
- Created `kernel/src/arch/riscv64/pt_regs.rs`, defining Linux-compatible `PtRegs` structure
- Added `Cause` enum to represent exception causes
- Added helper methods: `user_mode()`, `syscall_get_arguments()`, etc.
- Updated `trap.S` to use new register layout (288 bytes)
- Unified `SyscallFrame` and `TrapFrame` into `PtRegs`
- Fixed bug where user sp was not correctly saved at trap entry

---

### 2. [P0] System Call Handling Architecture Issues - FIXED

**File**: `kernel/src/arch/riscv64/syscall.rs`
**Priority**: High
**Status**: - **FIXED** (2026-02-24)

**Problem Description**:
1. Two sets of register structures exist (`TrapFrame` and `SyscallFrame`), increasing maintenance complexity
2. Each system call requires copying registers, inefficient
3. Missing system call number boundary checks
4. Missing `array_index_nospec` security measures

**Linux Implementation**:
```c
// Unified system call interface
typedef long (*syscall_t)(const struct pt_regs *);

static inline void syscall_get_arguments(struct task_struct *task,
                     struct pt_regs *regs,
                     unsigned long *args)
{
    args[0] = regs->orig_a0;
    args[1] = regs->a1;
    // ...
}

// System call table
void * const sys_call_table[__NR_syscalls] = {
    [__NR_read] = sys_read,
    [__NR_write] = sys_write,
    // ...
};
```

**Refactoring Plan**:
- - Unified use of `PtRegs` for system call parameter passing
- - Added `syscall_get_arguments` helper function
- - Added `syscall_set_return_value` helper function
- - Function pointer array style system call table (pending implementation)

**Fix Details**:
- `syscall_handler` now accepts `&mut PtRegs` parameter
- Added `syscall_get_nr()`, `syscall_get_arguments()`, `syscall_set_return_value()` helper functions
- Uses `orig_a0` as first argument (supports syscall rollback)

---

### 3. [P1] Task Structure Design Issues - FIXED

**File**: `kernel/src/process/task.rs`, `kernel/src/arch/riscv64/thread.rs`
**Priority**: Medium
**Status**: - **FIXED** (2026-02-24)

**Problem List** (resolved):
1. - `AddressSpace` directly embedded in Task, causing oversized structure - changed to `Box<AddressSpace>`
2. - Missing `thread_struct` abstraction - created `ThreadStruct` (thread.rs)
3. - Process state uses enum instead of bitmap - changed to `TaskState(u32)` bitmap form
4. - Missing distinction between `mm` and `active_mm` - added `active_mm` field

**Linux State Definitions**:
```c
#define TASK_RUNNING         0x00000000
#define TASK_INTERRUPTIBLE   0x00000001
#define TASK_UNINTERRUPTIBLE 0x00000002
#define __TASK_STOPPED       0x00000004
#define __TASK_TRACED        0x00000008
#define EXIT_DEAD            0x00000010
#define EXIT_ZOMBIE          0x00000020
// Can be combined
```

**Fix Details**:
- Created `kernel/src/arch/riscv64/thread.rs` to implement `ThreadStruct`
  - FPU state save/restore (f0-f31 + fcsr)
  - TLS pointer support (tp_value)
  - fpu_init() initialization function
- Changed `TaskState` to bitmap form `TaskState(u32)`
  - Added `is_running()`, `is_sleeping()`, `is_dead()` methods
  - Supports Linux-style state combination
- Changed `AddressSpace` to `Box<AddressSpace>` to reduce Task size
- Added `active_mm` field to support kernel threads borrowing address space

---

### 4. [P1] Missing copy_thread / start_thread Abstraction - FIXED

**File**: `kernel/src/arch/riscv64/process.rs` (newly created)
**Priority**: Medium
**Status**: - **FIXED** (2026-02-24)

**Linux Implementation**:
```c
// execve starts new program
void start_thread(struct pt_regs *regs, unsigned long pc, unsigned long sp)
{
    regs->status = SR_PIE;
    regs->epc = pc;
    regs->sp = sp;
}

// fork copies thread state
int copy_thread(struct task_struct *p, const struct kernel_clone_args *args)
{
    struct pt_regs *childregs = task_pt_regs(p);
    *childregs = *current_pt_regs();
    childregs->a0 = 0;  // fork returns 0 in child process
    p->thread.ra = (unsigned long)ret_from_fork;
}
```

**Fix Details**:
- Created `kernel/src/arch/riscv64/process.rs`
- Implemented `start_thread(regs, pc, sp)` - sets user program initial state
- Implemented `copy_thread(child, parent_regs)` - copies thread state during fork
- Implemented `flush_thread()` - thread state cleanup (reserved)
- Added helper functions: `current_pt_regs()`, `task_pt_regs()`, `user_stack_pointer()`, `instruction_pointer()`, `is_user_address()`
- Added `copy_from_user()` and `copy_to_user()` framework (exception table pending)

---

### 5. [P0] Incomplete Page Fault Handling - FIXED

**File**: `kernel/src/arch/riscv64/mm/fault.rs` (newly created)
**Priority**: High
**Status**: - **FIXED** (2026-02-24)

**Current Problem** (fixed):
```rust
ExceptionCause::LoadPageFault => {
    // ...
    (*frame).sepc += 4;  // Error: skip instruction instead of re-executing or sending signal
}
```

**Problem List** (resolved):
1. - Skipping instruction after page fault is wrong, should re-execute or send signal
2. - Missing `fixup_exception` mechanism for kernel page faults (framework implemented, pending refinement)
3. - No OOM handling
4. - Missing correct handling of return values like `VM_FAULT_SIGSEGV`
5. - Missing standard handling paths like `bad_area` / `no_context`

**Linux Processing Flow**:
```c
void handle_page_fault(struct pt_regs *regs)
{
    // 1. Distinguish kernel/user mode
    // 2. Check interrupt context
    // 3. Find VMA
    // 4. Verify permissions
    // 5. Handle COW
    // 6. Handle anonymous pages
    // 7. Handle swap (if any)
    // 8. Send signal or OOM
}
```

**Fix Details**:
- Created `kernel/src/arch/riscv64/mm/fault.rs`
- Implemented `do_page_fault(regs, access_type)` function
- Added `bad_area()` and `no_context()` standard handling paths
- Implemented `fixup_exception()` framework (needs linker script support for completion)
- Added `send_signal()` signal sending framework
- Defined `MmFaultResult` enum to represent handling results
- Updated `trap.rs` to use new `do_page_fault` function

---

### 6. [P0] sscratch Register Management Bug - FIXED

**File**: `kernel/src/arch/riscv64/trap.S`
**Priority**: High
**Status**: - **FIXED** (2026-02-24)

**Problem Description**:
When returning to user space, `sscratch` was incorrectly set to 0, causing subsequent traps to fail to correctly identify the CPU ID.

**Bug Code** (`.Lreturn_user`):
```assembly
// Wrong code
csrw sscratch, zero    // set sscratch = 0
sret
```

**Impact**:
1. When entering trap again from user space, `csrrw tp, sscratch, tp` sets `tp` to 0
2. `addi tp, tp, -1` sets `tp` to -1 (0xFFFFFFFFFFFFFFFF)
3. `cpu_id()` returns invalid value, unable to find current task's run queue
4. System calls (like `read`) cannot find current process's file descriptor table
5. Shell exits immediately after startup

**Fix Plan**:
Before restoring user `tp`, first save kernel hart ID and set `sscratch = hart_id + 1`:

```assembly
.Lreturn_user:
    // Before restoring user tp, first set sscratch = hart_id + 1
    addi t0, tp, 1
    csrw sscratch, t0

    // Restore user tp (from PtRegs)
    ld x4, PT_TP(sp)

    // Restore user sp (from PtRegs)
    ld x2, PT_SP(sp)

    sret
```

**Fix Details**:
- Fixed sscratch setting in `.Lreturn_user`
- Fixed same issue in `ret_from_fork`
- Added correct implementation of `cpu_id()` function (uses `tp` register instead of M-mode `mhartid` CSR)

---

### 6. [P1] Memory Management Architecture Issues - FIXED

**File**: `kernel/src/mm/mm_struct.rs`, `kernel/src/arch/riscv64/mm/base.rs`
**Priority**: Medium
**Status**: - **FIXED** (2026-02-24)

**Problem List** (resolved):
1. - VMA uses linear search Vec, O(n) complexity - changed to BTreeMap + max_end fast path
2. - Missing complete `mm_struct` abstraction - created `kernel/src/mm/mm_struct.rs`
3. - Page table entry type unsafe, directly uses `u64` (pending improvement)
4. - Missing `p4d_t` four-level page table support (pending improvement)

**Fix Details**:
- Created `kernel/src/mm/mm_struct.rs`, implementing Linux-compatible `MmStruct` structure
- Added complete segment range fields: `start_code`, `end_code`, `start_data`, `end_data`
- Added heap management fields: `start_brk`, `brk`
- Added stack management fields: `start_stack`
- Added argument/environment variable fields: `arg_start`, `arg_end`, `env_start`, `env_end`
- Added virtual memory statistics fields: `total_vm`, `locked_vm`, `pinned_vm`, `data_vm`, `exec_vm`, `stack_vm`
- Added mmap area fields: `mmap_base`, `mmap_legacy_base`, `highest_vm_end`
- Added ELF loading helper methods: `setup_segment_layout()`, `setup_stack()`, `setup_argv()`, `setup_envp()`
- Updated `kernel/src/arch/riscv64/mm/base.rs`, architecture-specific methods as extensions to `MmStruct`

**MmStruct Structure**:
```rust
pub struct MmStruct {
    // Page table management
    pub pgd: u64,                                    // Page table root PPN
    vma_manager: RwLock<VmaManager>,                 // VMA manager
    space_type: PageTableType,                       // Address space type

    // Segment ranges (Linux compatible)
    start_code: AtomicUsize,                         // Code segment start
    end_code: AtomicUsize,                           // Code segment end
    start_data: AtomicUsize,                         // Data segment start
    end_data: AtomicUsize,                           // Data segment end

    // Heap management
    start_brk: AtomicUsize,                          // Heap start address
    brk: AtomicUsize,                                // Current heap pointer

    // Stack management
    start_stack: AtomicUsize,                        // Stack start address

    // Arguments and environment variables
    arg_start: AtomicUsize,                          // Arguments start
    arg_end: AtomicUsize,                            // Arguments end
    env_start: AtomicUsize,                          // Environment variables start
    env_end: AtomicUsize,                            // Environment variables end

    // Virtual memory statistics
    total_vm: AtomicU64,                             // Total virtual memory pages
    locked_vm: AtomicU64,                            // Locked memory pages
    // ... more fields
}
```

---

### 7. [P2] Missing Critical Helper Macros/Functions

**File**: Multiple locations
**Priority**: Low
**Status**: Partially implemented

| Macro/Function | Linux | Rux | Description |
|---------|-------|-----|------|
| `user_mode(regs)` | - | - | Check if from user mode (PtRegs::user_mode()) |
| `task_pt_regs(task)` | - | - | Get task's pt_regs (process.rs) |
| `current_pt_regs()` | - | - | Get current process's pt_regs |
| `in_interrupt()` | - | Pending | Check if in interrupt context (framework implemented) |
| `in_task()` | - | - | Check if in process context |
| `fixup_exception()` | - | Pending | Kernel exception fixup (framework implemented) |
| `copy_to_user()` | - | Pending | Safe user space copy (framework implemented) |
| `copy_from_user()` | - | Pending | Safe user space copy (framework implemented) |
| `get_user()` | - | - | Safe read from user space |
| `put_user()` | - | - | Safe write to user space |

---

### 8. [P2] FPU/Vector Extension State Saving

**File**: Needs to be created
**Priority**: Low
**Status**: Not implemented

**Linux Implementation**:
```c
struct thread_struct {
    unsigned long fstate[FSTATE_SIZE];  // FPU state
    struct __riscv_v_ext_state vstate;  // Vector extension state
};

// Save/restore during context switch
void fstate_save(struct task_struct *task, struct pt_regs *regs);
void fstate_restore(struct task_struct *task, struct pt_regs *regs);
```

---

## III. Refactoring Progress Tracking

### First Priority (Core Features)
- [x] 1. Unify TrapFrame/pt_regs structure - (2026-02-24)
- [x] 2. Fix page fault handling - (2026-02-24)
- [x] 3. Unify system call framework - (2026-02-24)
- [x] 4. Fix sscratch register management bug - (2026-02-24)

### Second Priority (Architecture Improvements)
- [x] 5. Refactor Task structure - (2026-02-24)
- [x] 6. Implement complete mm_struct abstraction - (2026-02-24)
- [x] 7. Implement start_thread/copy_thread - (2026-02-24)

### Third Priority (Feature Completion)
- [x] 8. VMA red-black tree optimization - (2026-02-24) - BTreeMap + max_end fast path
- [ ] 9. Improve exception table mechanism (framework implemented)
- [ ] 10. FPU/vector extension support (ThreadStruct created, context switch integration pending)
- [ ] 11. Improve signal handling

---

## IV. Code Style Guidelines

### Naming Conventions
Use Linux-style function naming:
- `do_page_fault` instead of `handle_mm_fault`
- `copy_thread` instead of `fork_trap_frame`
- `sys_read` instead of `syscall_read`

### Error Handling
Use standard Linux error codes:
```rust
pub type LinuxResult<T> = Result<T, LinuxError>;

pub enum LinuxError {
    EPERM = 1,
    ENOENT = 2,
    ESRCH = 3,
    EINTR = 4,
    EIO = 5,
    // ...
}
```

---

## V. References

- Linux kernel source: `refer/linux/`
- RISC-V privileged architecture specification v20211203
- POSIX standard: https://pubs.opengroup.org/onlinepubs/9699919799/

---

*This document will be continuously updated as refactoring progresses*
