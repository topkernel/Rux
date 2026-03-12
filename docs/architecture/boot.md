# Rux Kernel Boot Process

This document describes the complete boot process of the Rux kernel from OpenSBI to userspace programs.

**Last Updated**: 2026-03-04
**Architecture**: RISC-V 64-bit (RV64GC)

---

## Boot Process Overview

```
QEMU starts
    |
    v
OpenSBI (M-mode)
    |  Initialize hardware, provide SBI services
    v
Rux Kernel (S-mode)
    |  Kernel initialization
    v
Init Process (U-mode)
    |  Shell / Desktop
    v
User Programs
```

---

## 1. OpenSBI Boot (M-mode)

### 1.1 QEMU Configuration

```bash
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -bios default \          # Use QEMU built-in OpenSBI
    -kernel rux.elf
```

### 1.2 OpenSBI Functions

- Initialize UART, CLINT, PLIC
- Set up M-mode trap handling
- Provide SBI call interface
- Jump to S-mode kernel entry

### 1.3 OpenSBI Output

```
OpenSBI v0.9
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|
        | |
        |_|

Platform Name             : riscv-virtio,qemu
Platform HART Count       : 4
Firmware Base             : 0x80000000
Firmware Size             : 128 KB
Domain0 Next Address      : 0x0000000080200000  <- Kernel entry
Domain0 Next Mode         : S-mode
```

---

## 2. Kernel Boot (S-mode)

### 2.1 Assembly Entry

**File**: `kernel/src/arch/riscv64/boot.S`

```asm
.section .init.entry
.global _start

_start:
    # 1. Disable all interrupts
    csrw sie, zero

    # 2. Set kernel stack
    la sp, _stack_top

    # 3. Clear BSS section
    la t0, __bss_start
    la t1, __bss_end
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    bne t0, t1, 1b

    # 4. Save DTB pointer (a1 -> s0)
    mv s0, a1

    # 5. Jump to Rust entry
    call rust_main

    # 6. Should not return
2:  wfi
    j 2b
```

### 2.2 Rust Main Function

**File**: `kernel/src/main.rs`

```rust
#[no_mangle]
pub extern "C" fn rust_main(dtb_ptr: usize) -> ! {
    // 1. Console initialization
    console::init();

    // 2. Print boot banner
    print_banner();

    // 3. Architecture initialization
    arch::arch_init();

    // 4. Trap initialization
    trap::init();

    // 5. System call initialization
    syscall::init();

    // 6. Heap allocator initialization
    mm::init_heap();

    // 7. Scheduler initialization
    sched::init();

    // 8. VFS initialization
    fs::vfs_init();

    // 9. Device driver initialization
    drivers::init();

    // 10. SMP multi-core boot
    smp::start_secondary_harts();

    // 11. Start init process
    init::start_init();

    // 12. Enter scheduler main loop
    sched::scheduler_main();
}
```

### 2.3 Subsystem Initialization

| Step | Module | Description |
|------|--------|-------------|
| 1 | console | UART ns16550a driver |
| 2 | arch | MMU, page tables, CPU detection |
| 3 | trap | stvec, sscratch setup |
| 4 | syscall | System call dispatcher |
| 5 | heap | Buddy + Slab allocators |
| 6 | sched | CFS scheduler initialization |
| 7 | vfs | ramfs, ext4, procfs, devfs |
| 8 | drivers | VirtIO-blk/net/gpu/input |
| 9 | smp | Secondary core boot (SBI HSM) |
| 10 | init | Create init process (PID 1) |

### 2.4 Boot Log

```
██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██

  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...

Module            Description                        Status
----------------  --------------------------------   --------
console:          UART ns16550a driver               [ok]
smp:              4 CPU(s) online                    [ok]
trap:             stvec handler installed            [ok]
trap:             ecall syscall handler              [ok]
mm:               Sv39 3-level page table            [ok]
mm:               satp CSR configured                [ok]
mm:               buddy allocator order 0-12         [ok]
mm:               heap region 16MB @ 0x80A00000      [ok]
mm:               slab allocator 1MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
mm:               user frame allocator 64MB          [ok]
mm:               16384 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs mounted /proc               [ok]
fs:               devfs mounted /dev                 [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           virtio-net x1                      [ok]
driver:           virtio-gpu x1                      [ok]
driver:           virtio-input x1                    [ok]
sched:            CFS scheduler v1                   [ok]
trap:             sie.SEIE enabled                   [ok]
init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]
```

---

## 3. SMP Multi-core Boot

### 3.1 Secondary Core Boot Process

**File**: `kernel/src/arch/riscv64/smp.rs`

```rust
pub fn start_secondary_harts() {
    for hart_id in 1..4 {
        // Start secondary core using SBI HSM extension
        let result = sbi::hart_start(
            hart_id,
            SECONDARY_ENTRY as u64,  // Secondary core entry address
            0,                        // Boot argument
        );

        if result.is_ok() {
            println!("smp: hart {} started", hart_id);
        }
    }

    // Wait for all secondary cores to be ready
    while SMP_DATA.online_count() < 4 {
        core::hint::spin_loop();
    }
}
```

### 3.2 Secondary Core Entry

```rust
#[no_mangle]
pub extern "C" fn secondary_start(hart_id: usize) -> ! {
    // 1. Initialize local data
    arch::init_per_cpu(hart_id);

    // 2. Initialize per-CPU scheduler
    sched::init_per_cpu(hart_id);

    // 3. Mark as online
    SMP_DATA.mark_online(hart_id);

    // 4. Enable interrupts
    arch::enable_irq();

    // 5. Enter scheduler main loop
    sched::scheduler_main();
}
```

---

## 4. Init Process Boot

### 4.1 Init Creation

**File**: `kernel/src/init.rs`

```rust
pub fn start_init() {
    // 1. Load shell ELF from ext4
    let elf_data = fs::ext4::read_file("/bin/shell").expect("shell not found");

    // 2. Create init process
    let init_task = Task::new_user(
        "init",
        &elf_data,
        &["/bin/shell"],
        &[],
    ).expect("failed to create init");

    // 3. Set PID to 1
    assert_eq!(init_task.pid, 1);

    // 4. Add to scheduler queue
    sched::enqueue(init_task);
}
```

### 4.2 First User Mode Switch

**File**: `kernel/src/arch/riscv64/usermode_asm.S`

```asm
# switch_to_user(entry, stack)
# Switch from kernel mode to user mode to execute first user program

switch_to_user:
    mv t5, a0              # entry
    mv t6, a1              # user_stack

    # Set sstatus.SPP = 0 (return to U-mode)
    csrr t1, sstatus
    li t0, ~0x100          # Clear SPP
    and t1, t1, t0
    li t0, 0x20            # Set SPIE
    or t1, t1, t0
    csrw sstatus, t1

    # Set entry point
    csrw sepc, t5

    # Flush TLB
    sfence.vma

    # Set user stack
    mv sp, t6

    # Return to user mode
    sret
```

---

## 5. Key Initialization Order

### 5.1 Required Order

| Order | Prerequisite | Description |
|-------|--------------|-------------|
| MMU -> PLIC | MMU first | PLIC registers need MMIO mapping |
| PLIC -> SMP | PLIC first | Secondary cores need to handle external interrupts |
| Trap -> Scheduler | Trap first | Scheduler depends on context switching |
| Heap -> Scheduler | Heap first | Process structures need dynamic allocation |
| All init -> IRQ | Init complete | Prevent early interrupts |

### 5.2 Current Order Verification

```rust
// Correct order
arch::arch_init();       // MMU
trap::init();            // Trap
syscall::init();         // System calls
mm::init_heap();         // Heap
sched::init();           // Scheduler
drivers::init();         // PLIC, VirtIO
smp::start_secondary();  // SMP
init::start_init();      // Init process
```

---

## 6. Troubleshooting

### 6.1 Boot Failure

**Symptoms**: No output or immediate crash

**Check**:
1. Is OpenSBI loading correctly?
2. Is kernel entry address correct (0x80200000)?
3. Is stack pointer valid?

### 6.2 MMU Initialization Failure

**Symptoms**: Page fault or illegal instruction

**Check**:
1. Are page tables properly aligned (4KB)?
2. Is satp correctly set?
3. Are memory attributes correct?

### 6.3 SMP Boot Failure

**Symptoms**: Only main core working

**Check**:
1. Is SBI HSM supported?
2. Is secondary core entry address correct?
3. Is per-CPU data initialized?

### 6.4 Init Process Failure

**Symptoms**: No shell prompt

**Check**:
1. Is ext4 properly mounted?
2. Does /bin/shell exist?
3. Is ELF loading correct?
4. Is user mode switch successful?

---

## References

- [RISC-V Privileged Architecture Specification](https://riscv.org/technical/specifications/)
- [OpenSBI Documentation](https://github.com/riscv/opensbi)
- [Linux RISC-V Boot](https://kernel.org/doc/html/latest/riscv/boot.html)

---

**Document Version**: v2.0.0
**Last Updated**: 2026-03-04
