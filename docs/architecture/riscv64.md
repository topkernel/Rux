# RISC-V 64-bit Architecture Implementation Document

This document details the Rux kernel implementation on the RISC-V 64-bit architecture.

**Last Updated**: 2026-03-04
**Status**: Fully implemented, the only supported architecture

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Memory Layout](#memory-layout)
- [Boot Process](#boot-process)
- [Exception Handling](#exception-handling)
- [System Calls](#system-calls)
- [CPU Operations](#cpu-operations)
- [Device Drivers](#device-drivers)
- [Multi-core Support](#multi-core-support)
- [References](#references)

---

## Architecture Overview

### RISC-V Privilege Levels

RISC-V defines three privilege levels (from lowest to highest):

1. **U-mode (User)** - User applications
2. **S-mode (Supervisor)** - Operating system kernel
3. **M-mode (Machine)** - Firmware/bootloader

**Rux Implementation**:
- **OpenSBI** runs in M-mode
- **Rux Kernel** runs in S-mode
- **User Programs** run in U-mode

```
+-------------------------------------+
|  OpenSBI (M-mode)                   |
|  0x80000000 - 0x801fffff            |
+-------------------------------------+
|  Rux Kernel (S-mode)                |
|  0x80200000+                        |
+-------------------------------------+
|  User Applications (U-mode)         |
|  Shell, Desktop, Toybox, etc.       |
+-------------------------------------+
```

### QEMU virt Platform

**Hardware Configuration**:
- CPU: RV64GC (RV64I M A F D C) - 4 cores
- Memory: 2GB (0x80000000 - 0x88000000)
- UART: ns16550a @ 0x10000000
- CLINT: @ 0x02000000
- PLIC: @ 0x0c000000

---

## Memory Layout

### Physical Memory Map

```
Address Range         Size     Usage
--------------------------------------------------
0x8000_0000 -       128KB    OpenSBI firmware
0x801f_ffff
0x8020_0000 -       ~2MB     Rux kernel code
0x8040_0000
0x8040_0000 -       16MB     Kernel heap (Buddy/Slab)
0x8140_0000
0x8140_0000 -       64MB     User physical page pool
0x8540_0000
```

### Virtual Memory Layout (Sv39)

```
Virtual Address Range  Usage
--------------------------------------------------
0x0000_0000_0000 -   User space (lower 256GB)
0x0000_003f_ffff

0xffff_ffc0_0000 -   Kernel space (upper 256GB)
0xffff_ffff_ffff
    +-- 0xffff_ffc0_8000_0000  Kernel code mapping
    +-- 0xffff_ffc0_8140_0000  User physical page mapping
    +-- 0xffff_ffc8_0000_0000  MMIO mapping
```

### Linker Script

**File**: `kernel/src/arch/riscv64/linker.ld`

```ld
MEMORY {
    /* Avoid OpenSBI firmware area */
    RAM : ORIGIN = 0x80200000, LENGTH = 126M
}

SECTIONS {
    .text : {
        *(.init.entry)
        *(.init)
        . = ALIGN(4);
        *(.tramp)       /* Exception vector table */
        *(.text.*)
        *(.rodata .rodata.*)
    } > RAM

    .data : {
        *(.data .data.*)
    } > RAM

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } > RAM

    /* Stack space */
    .stack : {
        . = ALIGN(16);
        _stack_bottom = .;
        . += 16384; /* 16KB stack */
        _stack_top = .;
    } > RAM
}
```

---

## Boot Process

### Boot Sequence

**File**: `kernel/src/arch/riscv64/boot.S`

```asm
.section .init.entry
.global _start

_start:
    # 1. Disable interrupts
    csrw sie, zero

    # 2. Set stack pointer
    la sp, _stack_top

    # 3. Clear BSS section
    la t0, __bss_start
    la t1, __bss_end
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    bne t0, t1, 1b

    # 4. Save DTB pointer (via s0 callee-saved)
    mv s0, a1

    # 5. Jump to Rust entry
    call rust_main

    # 6. Should not return
2:  wfi
    j 2b
```

### OpenSBI Integration

**OpenSBI Functions**:
- Initialize hardware (UART, CLINT, PLIC)
- Provide SBI call interface
- Jump to S-mode kernel

**Boot Process**:
```
1. QEMU starts -> M-mode
2. OpenSBI loads (0x80000000)
3. OpenSBI initializes hardware
4. OpenSBI jumps to kernel (0x80200000)
5. Kernel enters S-mode (_start)
6. Kernel initializes subsystems
7. Start init process (PID 1)
```

**Checkpoint Output**:
```
OpenSBI v0.9
...
Domain0 Next Address: 0x0000000080202b1c  <- Kernel entry point
Domain0 Next Mode: S-mode                 <- Enter S-mode

██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██

  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...
```

---

## Exception Handling

### CSR Registers

**Key S-mode CSRs**:

| CSR | Name | Purpose |
|-----|------|---------|
| `stvec` | Trap Vector | Exception vector table address |
| `sstatus` | Supervisor Status | Interrupt enable, status flags |
| `scause` | Supervisor Cause | Exception cause |
| `sepc` | Supervisor Exception PC | Exception return address |
| `stval` | Supervisor Trap Value | Exception-related information |
| `sie` | Supervisor Interrupt Enable | Interrupt enable |
| `sip` | Supervisor Interrupt Pending | Interrupt pending |
| `sscratch` | Scratch Register | User/kernel mode detection |

### sscratch Detection Mechanism

**Linux-style trap source detection**:

```asm
# When running in user mode: sscratch = current_task, tp = user TLS
# When running in kernel mode: sscratch = 0, tp = current_task

trap_entry:
    csrrw tp, sscratch, tp    # Atomic swap tp and sscratch
    bnez tp, .Lfrom_user      # tp != 0 means from user mode
    j .Lfrom_kernel           # tp == 0 means from kernel mode
```

### Trap Handling Framework

**Core Files**:
- `kernel/src/arch/riscv64/trap.S` - Trap entry/exit assembly code
- `kernel/src/arch/riscv64/trap.rs` - Trap handling Rust code

**Trap Handling Process**:

```assembly
trap_entry:
    csrrw tp, sscratch, tp     # Detect source and save tp

    # From user mode
.Lfrom_user:
    ld sp, TASK_TI_KERNEL_SP(tp)  # Load process kernel stack
    addi sp, sp, -272             # Allocate TrapFrame

    # Save general purpose registers
    sd x1, 8(sp)      # ra
    sd x5, 16(sp)     # t0
    # ... other registers ...

    # Save CSRs
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, scause
    csrr t3, stval
    sd t0, 216(sp)    # sstatus
    sd t1, 224(sp)    # sepc
    sd t2, 232(sp)    # scause
    sd t3, 240(sp)    # stval

    # Call Rust handler
    mv a0, sp
    call trap_handler

    # Restore and return
    # ...

    sret
```

### Exception Types

**Common Exceptions**:
- `0x2`: Illegal instruction
- `0x5`: Read access fault
- `0x7`: Write access fault
- `0x8`: User mode ecall
- `0xd`: Page fault (Store/AMO)

---

## System Calls

### System Call Interface

**Register Convention** (following RISC-V Linux ABI):
- `a7`: System call number
- `a0-a5`: Arguments
- `a0`: Return value

### Implemented System Calls (80+)

**File Operations**:
| Syscall Number | Name | Description |
|----------------|------|-------------|
| 56 | sys_openat | Open file |
| 57 | sys_close | Close file |
| 63 | sys_read | Read file |
| 64 | sys_write | Write file |
| 62 | sys_lseek | Seek file |
| 80 | sys_fstat | Get file status |
| 35 | sys_unlinkat | Delete file |
| 34 | sys_mkdirat | Create directory |

**Process Operations**:
| Syscall Number | Name | Description |
|----------------|------|-------------|
| 93 | sys_exit | Exit process |
| 172 | sys_getpid | Get process ID |
| 110 | sys_getppid | Get parent process ID |
| 220 | sys_clone | Create process/thread |
| 221 | sys_execve | Execute program |
| 260 | sys_wait4 | Wait for child process |

**Memory Operations**:
| Syscall Number | Name | Description |
|----------------|------|-------------|
| 214 | sys_brk | Adjust heap |
| 222 | sys_mmap | Memory mapping |
| 215 | sys_munmap | Unmap memory |
| 226 | sys_mprotect | Change protection |

**Network Operations**:
| Syscall Number | Name | Description |
|----------------|------|-------------|
| 198 | sys_socket | Create socket |
| 200 | sys_bind | Bind address |
| 201 | sys_listen | Listen for connections |
| 202 | sys_accept | Accept connection |
| 203 | sys_connect | Initiate connection |
| 206 | sys_sendto | Send data |
| 207 | sys_recvfrom | Receive data |

**Signal Operations**:
| Syscall Number | Name | Description |
|----------------|------|-------------|
| 129 | sys_kill | Send signal |
| 134 | sys_rt_sigaction | Set signal handler |
| 135 | sys_rt_sigprocmask | Signal mask |

### System Call Dispatch

**File**: `kernel/src/syscall/dispatch.rs`

```rust
pub fn dispatch_syscall(syscall_no: u64, args: &[u64; 6]) -> i64 {
    match syscall_no as usize {
        63 => sys_read(args[0], args[1] as *mut u8, args[2]),
        64 => sys_write(args[0], args[1] as *const u8, args[2]),
        93 => sys_exit(args[0] as i32),
        172 => sys_getpid(),
        220 => sys_clone(args),
        221 => sys_execve(args),
        // ... 80+ system calls
        _ => -ENOSYS,
    }
}
```

---

## CPU Operations

### Interrupt Control

**File**: `kernel/src/arch/riscv64/mod.rs`

```rust
/// Enable interrupts
pub fn enable_irq() {
    unsafe {
        asm!("csrsi sstatus, 2"); // Set SIE bit
    }
}

/// Disable interrupts
pub fn disable_irq() {
    unsafe {
        asm!("csrci sstatus, 2"); // Clear SIE bit
    }
}
```

### CPU ID Retrieval

```rust
pub fn cpu_id() -> usize {
    // Read current task from tp register, then get ti_cpu
    current_task().ti_cpu as usize
}

pub fn hart_id() -> usize {
    // Get hardware thread ID from SBI
    sbi_call(SBI_GET_HART_ID, 0, 0, 0).value as usize
}
```

### Counter Retrieval

```rust
pub fn read_counter() -> u64 {
    let time: u64;
    unsafe {
        asm!("csrr {}, time", out(reg) time);
    }
    time
}

pub fn get_counter_freq() -> u64 {
    // Query via SBI
    sbi_call(SBI_GET_TIME, 0, 0, 0).value
}
```

---

## Device Drivers

### UART Driver

**File**: `kernel/src/console.rs`

**Hardware Configuration**:
```rust
const UART0_BASE: usize = 0x1000_0000;  // ns16550a
```

### VirtIO Drivers

**File**: `kernel/src/drivers/virtio/`

**Supported Devices**:
- **virtio-blk** - Block device driver (ext4 file system)
- **virtio-net** - Network device driver
- **virtio-gpu** - GPU driver (framebuffer)
- **virtio-input** - Input device driver (keyboard/mouse)

### Interrupt Controller

**PLIC (Platform-Level Interrupt Controller)**

**File**: `kernel/src/drivers/intc/plic.rs`

```rust
/// PLIC initialization
pub fn init() {
    // Set priority threshold
    write_priority_threshold(0);

    // Enable all interrupts for each hart
    for hart in 0..4 {
        enable_all_interrupts(hart);
    }
}

/// External interrupt handling
pub fn handle_external_irq() {
    let claim = claim_interrupt();
    // Handle interrupt...
    complete_interrupt(claim);
}
```

---

## Multi-core Support

### SMP Initialization

**File**: `kernel/src/arch/riscv64/smp.rs`

```rust
/// Start secondary harts
pub fn start_secondary_harts() {
    for hart_id in 1..4 {
        // Start secondary hart via SBI HSM
        sbi_hsm_hart_start(hart_id, SECONDARY_ENTRY, 0);
    }
}

/// Secondary hart entry point
#[no_mangle]
pub extern "C" fn secondary_start(hart_id: usize) -> ! {
    // Initialize local data
    // Enter scheduling loop
    scheduler_main();
}
```

### IPI (Inter-Processor Interrupt)

**File**: `kernel/src/arch/riscv64/ipi.rs`

```rust
/// Send IPI
pub fn send_ipi(target_hart: usize, msg: IpiMessage) {
    IPI_QUEUE[target_hart].push(msg);
    sbi_send_ipi(1 << target_hart);
}

/// Handle IPI
pub fn handle_ipi() {
    while let Some(msg) = IPI_QUEUE[cpu_id()].pop() {
        match msg {
            IpiMessage::Reschedule => set_need_resched(),
            IpiMessage::Shutdown => halt(),
        }
    }
}
```

### Per-CPU Data

```rust
pub struct PerCpu {
    pub run_queue: CfsRunQueue,
    pub current_task: Option<Arc<Task>>,
    pub idle_task: Arc<Task>,
}

static PER_CPU: [SpinLock<PerCpu>; 4] = [...];
```

---

## CFS Scheduler

**File**: `kernel/src/sched/cfs.rs`

```rust
/// CFS run queue
pub struct CfsRunQueue {
    tasks: BTreeMap<u64, Arc<Task>>,  // Sorted by vruntime
    min_vruntime: u64,
    load_weight: u64,
}

/// Select next task
pub fn pick_next_task(&mut self) -> Option<Arc<Task>> {
    // Select task with minimum vruntime
    self.tasks.first_key_value().map(|(_, task)| task.clone())
}

/// Update vruntime
pub fn update_vruntime(task: &mut Task, delta: u64) {
    let weight = task.load_weight;
    let vruntime_delta = (delta * NICE_0_LOAD) / weight;
    task.vruntime += vruntime_delta;
}
```

---

## COW (Copy-on-Write)

**File**: `kernel/src/arch/riscv64/mm/base.rs`

```rust
/// COW page table copy
pub fn copy_page_table(src_root: PhysAddr, dst_root: PhysAddr) -> Result<(), i32> {
    for vpn in 0..512 {
        let pte = read_pte(src_root, vpn);
        if pte & PTE_V != 0 && pte & PTE_W != 0 {
            // Mark as COW: clear write permission, set COW flag
            let cow_pte = (pte & !PTE_W) | PTE_COW;
            write_pte(src_root, vpn, cow_pte);
            write_pte(dst_root, vpn, cow_pte);

            // Increment reference count
            inc_page_ref_count(pte_to_phys(pte));
        }
    }
    sfence_vma();
    Ok(())
}

/// COW page fault handling
pub fn handle_cow_fault(vaddr: VirtAddr) -> Result<PhysAddr, i32> {
    // Allocate new page and copy data
    // Update PTE
    // Flush TLB
}
```

---

## References

### Official Specifications
- [RISC-V Privileged Architecture Specification](https://riscv.org/technical/specifications/)
- [RISC-V Instruction Set Manual](https://riscv.org/technical/specifications/)
- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)

### Open Source Projects
- [OpenSBI](https://github.com/riscv/opensbi)
- [Linux RISC-V Port](https://kernel.org/doc/html/latest/riscv/index.html)

### QEMU Documentation
- [QEMU RISC-V virt Platform](https://www.qemu.org/docs/master/system/riscv/virt.html)

---

**Document Version**: v2.0.0
**Last Updated**: 2026-03-04
**Maintainer**: Rux Development Team
