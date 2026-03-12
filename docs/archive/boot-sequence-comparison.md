# Linux vs Rux Boot Sequence Comparison

## Overview

This document compares the boot sequences of Linux and Rux kernels, focusing on:
1. Initialization of the tp (thread pointer) register
2. Timing of sscratch CSR setup
3. Transition from early boot to scheduler mode

## 1. Linux Boot Sequence

### 1.1 Boot CPU

**File**: `arch/riscv/kernel/head.S`

```asm
// head.S:307 - Before MMU enabled
la tp, init_task              // tp immediately points to init_task
la sp, init_thread_union + THREAD_SIZE

// head.S:330-333 - After MMU enabled (relocation)
la tp, init_task              // reload tp
la sp, init_thread_union + THREAD_SIZE
addi sp, sp, -PT_SIZE_ON_STACK
scs_load_current

// head.S:328 - Set trap vector
call .Lsetup_trap_vector
```

**`.Lsetup_trap_vector`**:
```asm
// head.S:189-199
.Lsetup_trap_vector:
    la a0, handle_exception
    csrw CSR_TVEC, a0

    // Key: Set sscratch = 0, indicating currently in kernel mode
    csrw CSR_SCRATCH, zero
    ret
```

### 1.2 Secondary CPUs

**SBI HSM method** (`cpu_ops_sbi.c`):
```c
// Pass idle task pointer when starting secondary CPU
bdata->task_ptr = tidle;
bdata->stack_ptr = task_pt_regs(tidle);
sbi_hsm_hart_start(hartid, boot_addr, hsm_data);
```

**Secondary CPU entry** (`head.S:128-163`):
```asm
secondary_start_sbi:
    // Load tp and sp from boot data passed by SBI
    li a2, SBI_HART_BOOT_TASK_PTR_OFFSET
    add a2, a2, a1
    REG_L tp, (a2)              // tp = idle task pointer

    // ... MMU setup ...

    call .Lsetup_trap_vector    // Set sscratch = 0
    call smp_callin
```

### 1.3 Linux's tp/sscratch Protocol

| Stage | tp Value | sscratch Value | Description |
|-------|----------|----------------|-------------|
| Kernel mode running | `current` task_struct | 0 | sscratch=0 indicates kernel mode |
| User mode running | User TLS | `current` task_struct | sscratch saves task pointer |
| Trap entry (from kernel) | Unchanged | 0 | After csrrw tp, sscratch, tp: tp=0 |
| Trap entry (from user) | User TLS -> task | task -> User TLS | After csrrw swap: tp=task |

**Trap entry detection** (`entry.S:96-106`):
```asm
handle_exception:
    csrrw tp, CSR_SCRATCH, tp   // Atomic swap
    bnez tp, .Lsave_context     // tp != 0 means from user mode
                                // tp == 0 means from kernel mode
```

**Before returning to user mode** (`entry.S:236-239`):
```asm
    // Save tp to sscratch so next trap can find kernel data structures
    csrw CSR_SCRATCH, tp
```

---

## 2. Rux Boot Sequence

### 2.1 Boot CPU

**File**: `kernel/src/arch/riscv64/boot.S`

```asm
_start:
    // a0 = hart_id (passed from OpenSBI)
    mv tp, a0                    // tp = hart_id (not a task pointer!)

    // Calculate per-CPU stack
    li t1, 65536
    mul t1, tp, t1
    la sp, _stack_bottom
    add sp, sp, t1
    addi sp, sp, 65536

    // Clear BSS (first hart only)
    // ...

    call rust_main               // Jump to Rust code
```

### 2.2 rust_main Initialization Flow

**File**: `kernel/src/main.rs`

```rust
fn rust_main() -> ! {
    // 1. SMP initialization
    let is_boot_hart = arch::smp::init();

    // 2. Console initialization
    console::init();

    // 3. Trap initialization (install stvec)
    arch::trap::init();

    // ... MMU, heap, filesystem initialization ...

    // 4. Scheduler initialization (create idle task)
    sched::init();               // <-- Creates idle task here

    // 5. Start init process
    init::init();

    // 6. Enter scheduling loop
    sched::cpu_idle_loop();
}
```

### 2.3 Scheduler Initialization

**File**: `kernel/src/sched/sched.rs`

```rust
pub fn init() {
    let cpu_id = crate::arch::cpu_id() as usize;
    init_per_cpu_rq(cpu_id);

    // Create idle task
    let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
    Task::new_idle_at(idle_ptr);

    // Set run queue
    rq_inner.idle = idle_ptr;
    rq_inner.current = idle_ptr;

    // Note: tp is still hart_id at this point, not idle task pointer!
}
```

### 2.4 Rux's tp/sscratch State

| Stage | tp Value | sscratch Value | Description |
|-------|----------|----------------|-------------|
| Boot (boot.S) | hart_id | Undefined | Passed from OpenSBI |
| Rust initialization | hart_id | Undefined | Not set |
| After scheduler init | hart_id | Undefined | **Problem: tp not updated** |
| User mode running | hart_id | Undefined | Using sstatus.SPP detection |

---

## 3. Key Differences Analysis

### 3.1 tp Register Usage

| Aspect | Linux | Rux |
|--------|-------|-----|
| At boot | `init_task` pointer | hart_id |
| After scheduling | `current` task pointer | hart_id (unchanged) |
| During context switch | Updated to new task | Not updated |

### 3.2 sscratch Usage

| Aspect | Linux | Rux |
|--------|-------|-----|
| Kernel mode | 0 | Undefined |
| User mode | task pointer | Undefined |
| Detection method | csrrw swap | sstatus.SPP |

### 3.3 Trap Detection Mechanism

**Linux (sscratch swap)**:
```asm
// 2 instructions to complete detection and tp save
csrrw tp, CSR_SCRATCH, tp
bnez tp, .Lfrom_user
```

**Rux (sstatus.SPP)**:
```asm
// 3+ instructions
csrr t0, sstatus
andi t0, t0, SR_SPP
bnez t0, .Lfrom_kernel
```

---

## 4. Conditions for Implementing sscratch Detection

For Rux to use Linux-style sscratch detection, the following must be met:

### 4.1 tp Points to task_struct

**Current Problem**: tp = hart_id, not a valid task pointer

**Solution**: After scheduler initialization, set tp = idle_task

### 4.2 sscratch Protocol

| State | sscratch | tp |
|--------|----------|-----|
| Kernel mode | 0 | current task |
| User mode | current task | user TLS |

### 4.3 Transition Period Handling

**Problem**: How to handle the transition from tp = hart_id to tp = task_struct?

**Linux Solution**: tp is a task pointer from the very first instruction, no transition period

**Rux Solution**: Need to switch tp at a safe point

---

## 5. Safe Implementation Plan

### 5.1 Plan A: Early Switch (Recommended)

Switch tp in `sched::init()`:

```rust
pub fn init() {
    let cpu_id = crate::arch::cpu_id() as usize;
    init_per_cpu_rq(cpu_id);

    // Create idle task
    let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
    Task::new_idle_at(idle_ptr);

    // Set ti_cpu field
    (*idle_ptr).set_cpu(cpu_id);

    // Set sscratch = 0 (kernel mode)
    unsafe {
        core::arch::asm!("csrw sscratch, zero");
    }

    // Switch tp to point to idle task
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);
    }

    // Set run queue
    rq_inner.idle = idle_ptr;
    rq_inner.current = idle_ptr;
}
```

**Advantages**:
- Short transition period, effective immediately after sched::init()
- No need to modify boot.S

**Notes**:
- Must complete sched::init() before using sscratch detection
- cpu_id() needs to support both modes simultaneously

### 5.2 Plan B: Initialize in boot.S

Similar to Linux, set tp = init_task in boot.S:

```asm
_start:
    mv tp, a0                    // Temporarily store hart_id

    // ... Stack setup ...

    // Create static idle task for each CPU
    la t0, idle_tasks
    slli t1, tp, 3               // t1 = hart_id * 8
    add t0, t0, t1
    ld tp, (t0)                  // tp = &idle_tasks[hart_id]

    // Set sscratch = 0
    csrw sscratch, zero

    call rust_main
```

**Advantages**:
- Consistent with Linux from the start
- No transition period

**Disadvantages**:
- Need to allocate idle task in boot.S (complex)
- Need to ensure idle task is initialized after BSS clearing

### 5.3 Recommended Plan: Plan A + Compatible Detection

Use Plan A, but trap.S needs to be compatible with both modes:

```asm
trap_entry:
    csrrw tp, sscratch, tp       // Attempt swap

    // Check if sscratch is initialized
    // If sscratch == 0 and tp was originally a small value, still in early boot
    li t0, 0x1000
    bltu tp, t0, .Learly_boot    // tp < 0x1000, early boot

    bnez tp, .Lfrom_user         // Normal sscratch detection
    j .Lfrom_kernel

.Learly_boot:
    // Early boot stage, use sstatus.SPP detection
    csrr t0, sstatus
    andi t0, t0, SR_SPP
    bnez t0, .Lfrom_kernel
    j .Lfrom_user
```

---

## 6. Implementation Steps

### Phase 1: Preparation (Completed)
- [x] Add thread_info field to Task structure
- [x] Add context fields to ThreadStruct
- [x] Add SUM bit save/restore to context_switch

### Phase 2: Scheduler Initialization Modification
1. Set tp = idle_task in `sched::init()`
2. Set sscratch = 0
3. Set idle task's ti_cpu field

### Phase 3: trap.S Modification
1. Use sscratch swap detection
2. Add early boot compatible detection
3. Set sscratch = tp when returning to user mode

### Phase 4: cpu_id() Update
1. Detect tp mode (hart_id vs task_struct)
2. Choose different access methods based on mode

### Phase 5: Testing and Verification
1. Verify kernel boots normally
2. Verify shell starts normally
3. Verify user programs run normally

---

## 7. Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Early trap handling | High | Add early boot compatible detection |
| tp switch timing | Medium | Switch at end of sched::init() |
| cpu_id() compatibility | Medium | Support dual mode detection |
| sscratch race | Low | Doesn't exist in single-core environment |
