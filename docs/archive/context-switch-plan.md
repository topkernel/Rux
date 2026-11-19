# Context Switch Alignment Plan

## Goal

Align Rux kernel's context switch mechanism with the Linux kernel, ensuring:
1. Full Linux ABI compatibility
2. Support for all necessary context states
3. Code structure consistent with Linux

## Completed Improvements

### 1. thread_info Style Fields

Added thread_info style fields at the beginning of the Task structure:

```rust
#[repr(C)]
pub struct Task {
    // thread_info fields (offset 0)
    ti_flags: AtomicU32,           // Process flags
    ti_preempt_count: AtomicI32,   // Preemption count
    ti_kernel_sp: AtomicU64,       // Kernel stack pointer
    ti_user_sp: AtomicU64,         // User stack pointer
    ti_cpu: AtomicI32,             // Running CPU
    // ... other fields
}
```

**New Constants**:
```rust
pub const TIF_SIGPENDING: u32 = 0;
pub const TIF_NEED_RESCHED: u32 = 1;
pub const TIF_NOTIFY_RESUME: u32 = 2;
pub const TIF_UPROBE: u32 = 3;
pub const TIF_MEMDIE: u32 = 4;
```

### 2. ThreadStruct Extension

Added fields required for context switching:

```rust
pub struct ThreadStruct {
    // Context switch fields
    pub ra: u64,      // Return address
    pub sp: u64,      // Stack pointer
    pub s: [u64; 12], // s0-s11
    pub sum: u64,     // SUM bit
    // ... other fields
}
```

### 3. SUM Bit Save/Restore

Added SUM bit save and restore in context_switch:

```rust
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Save current SUM bit status
    let sum_status: u64;
    core::arch::asm!(
        "csrr {0}, sstatus",
        "and {0}, {0}, {1}",
        out(reg) sum_status,
        in(reg) 0x40000u64,
        options(nomem, nostack)
    );

    // Call context switch
    cpu_switch_to(next_ctx, prev_ctx);

    // Update tp to point to new task
    // ...

    // Restore SUM bit status
    if sum_status != 0 {
        core::arch::asm!(
            "csrs sstatus, {0}",
            in(reg) 0x40000u64,
            options(nomem, nostack)
        );
    }
}
```

### 4. cpu_id() Update

Updated the cpu_id() function to support new tp usage:

```rust
pub fn cpu_id() -> usize {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // Check if tp is a small value (early boot stage hart_id)
        if tp_value < 0x1000 {
            tp_value as usize
        } else {
            // tp points to task_struct, get hart_id from ti_cpu field
            let cpu_ptr = (tp_value as usize + 0x18) as *const AtomicI32;
            (*cpu_ptr).load(Ordering::Relaxed) as usize
        }
    }
}
```

---

## Next Step: sscratch Detection Mechanism Implementation - Completed

### Background Analysis

Refer to `docs/architecture/boot-sequence-comparison.md`, key differences between Linux and Rux:

| Aspect | Linux | Rux (Current) |
|--------|-------|---------------|
| tp at boot | `init_task` pointer | `hart_id` |
| Kernel mode sscratch | 0 | Undefined |
| Detection method | csrrw swap | sstatus.SPP |

### Implementation Plan

Adopt **Plan A + Compatible Detection**: Switch tp after scheduler initialization, trap.S supports both modes simultaneously.

### Phase 2: Scheduler Initialization Modification

**Modified File**: `kernel/src/sched/sched.rs`

Added at the end of the `init()` function:
1. Set idle task's ti_cpu field
2. Set sscratch = 0 (indicates kernel mode)
3. Switch tp to point to idle task

### Phase 3: trap.S Modification

**Modified File**: `kernel/src/arch/riscv64/trap.S`

1. Use `csrrw tp, sscratch, tp` swap detection
2. Detection logic:
   - tp == 0: From kernel mode
   - tp >= 0x80000000: From user mode (valid task pointer)
   - tp < 0x80000000: Early boot stage (use sstatus.SPP)
3. When returning to user mode, set sscratch = tp (current task)

### Phase 4: Context Switch Update tp

**Modified File**: `kernel/src/arch/riscv64/context.rs`

1. Add `context_switch_asm` pure assembly function, update tp after context switch
2. Update `switch_to_user` to set sscratch = tp before sret

### Phase 5: Test Verification

```bash
make run
# Expected: shell starts normally
```

---

## Verification Methods

### Functional Testing
```bash
make run
# Expected: shell starts normally and can execute commands
```

### Unit Testing
```bash
make test
```

## Modified Files

| File | Modification |
|------|--------------|
| `kernel/src/process/task.rs` | Added thread_info fields and accessor methods |
| `kernel/src/arch/riscv64/thread.rs` | Added context switch fields |
| `kernel/src/arch/riscv64/context.rs` | Added SUM bit save/restore, tp update, context_switch_asm |
| `kernel/src/arch/riscv64/smp.rs` | Updated cpu_id() |
| `kernel/src/arch/riscv64/mod.rs` | Updated cpu_id() |
| `kernel/src/sched/sched.rs` | Added tp switch and sscratch initialization |
| `kernel/src/arch/riscv64/trap.S` | Added sscratch detection mechanism |

## Success Criteria

1. Kernel compiles successfully
2. All modules load normally
3. Shell starts normally
4. User programs run normally
5. sscratch detection mechanism works correctly
6. Consistent with Linux kernel behavior

---

## Detailed Design Documents

- [Context Switch Comparison Analysis](context-switch-analysis.md)
- [Boot Sequence Comparison](boot-sequence-comparison.md)
