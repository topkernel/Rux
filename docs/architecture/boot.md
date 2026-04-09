# Rux Kernel Boot Process

This document describes the complete boot process of the Rux kernel from OpenSBI to userspace programs.

**Last Updated**: 2026-04-09
**Architecture**: RISC-V 64-bit (RV64GC)

---

## Boot Process Overview

```
QEMU starts
    |
    v
OpenSBI (M-mode)
    |  Initialize hardware, provide SBI services
    |  Load kernel at 0x80200000 (physical)
    v
Rux Kernel Entry (_start)          [Physical address, MMU off]
    |  boot.S: setup stack, clear BSS, create page tables
    |  Enable MMU via trampoline, trap to virtual address
    v
Rux Kernel (Virtual address)       [MMU on, Sv39]
    |  Switch to early page table
    |  Jump to rust_main()
    v
rust_main()
    |  Phase 1: Console, Trap, MMU init
    |  Phase 2: memblock, linear mapping, heap, slab
    |  Phase 3: Device mappings, PLIC, IPI
    |  Phase 4: Filesystem, drivers, scheduler
    |  Phase 5: Init process, timer, scheduler loop
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
- Load kernel ELF at physical address `0x80200000`
- Jump to S-mode kernel entry (`_start`)

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

## 2. Kernel Assembly Entry (`_start`)

**File**: `kernel/src/arch/riscv64/boot.S`

### 2.1 Key Concept: VMA vs LMA

The kernel uses a Linux-style **split address** linking strategy:

| Property | Value | Description |
|----------|-------|-------------|
| **VMA** (Virtual Memory Address) | `0xFFFFFFFF80000000` | Where the kernel *thinks* it lives |
| **LMA** (Load Memory Address) | `0x80200000` | Where OpenSBI actually loads the kernel |
| **VA_OFFSET** | `KERNEL_VIRT - KERNEL_PHYS` | `0xFFFFFFFF80000000 - 0x80200000` |

The linker script uses the `AT()` directive to separate VMA and LMA:

```ld
SECTIONS {
    . = KERNEL_VIRT;                          // VMA = virtual address
    .text : AT(KERNEL_PHYS) ALIGN(4096) { }   // LMA = physical address
    .rodata : AT(ADDR(.rodata) - VA_OFFSET) { }
    .data : AT(ADDR(.data) - VA_OFFSET) { }
    .bss : AT(ADDR(.bss) - VA_OFFSET) { }
}
```

When `_start` executes, the CPU is running at the physical address (`0x80200000`), but all symbols (like `__bss_start`, `_stack_top`) are linked at virtual addresses. The assembly code must subtract `VA_OFFSET` from these symbols to obtain physical addresses.

### 2.2 Boot Sequence Step by Step

```asm
_start:
    mv tp, a0                    // Save hart ID to tp
    mv s0, a1                    // Save DTB pointer to s0

    // Disable interrupts
    csrw sie, zero
    csrw sip, zero
```

#### Step 1: Compute VA_OFFSET

```asm
    li t1, KERNEL_VIRT           // t1 = 0xFFFFFFFF80000000
    li t3, 1025                  // 1025 = 0x401
    slli t3, t3, 21              // t3 = 0x401 << 21 = 0x80200000
    sub t1, t1, t3               // t1 = VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS
```

A shift/add trick is used instead of `li t3, KERNEL_PHYS` because `li` on large immediates uses `lui` which sign-extends, potentially causing issues with the subtraction.

#### Step 2: Setup Early Stack (Physical Address)

```asm
    la t0, _stack_bottom         // VMA of _stack_bottom
    sub sp, t0, t1               // sp = physical address of _stack_bottom
    li t0, 0x40000               // 256KB stack
    add sp, sp, t0               // sp = physical stack top
```

#### Step 3: Clear BSS Section

All symbols from `la` are virtual addresses, so `VA_OFFSET` is subtracted:

```asm
    la t0, __bss_start
    sub t0, t0, t1               // physical address
    la t2, __bss_end
    sub t2, t2, t1               // physical address
4:  bgeu t0, t2, 5f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 4b
```

#### Step 4: Create Trampoline Page Tables

The trampoline page table provides the **minimal** mapping needed to transition from physical to virtual address execution. It only maps the first 8MB of the kernel.

**Memory layout of page tables (all in `.data` section):**

| Table | Purpose | Location |
|-------|---------|----------|
| `trampoline_pg_dir` | PGD for MMU enable (2MB kernel only) | `.data` section |
| `trampoline_pmd` | PMD for trampoline (2 entries) | `.data` section |
| `early_pg_dir` | PGD for full early mapping | `.data` section |
| `early_pmd` | PMD for kernel region | `.data` section |
| `early_pmd_io` | PMD for MMIO identity mapping | `.data` section |

**Trampoline page table structure:**

```
trampoline_pg_dir (PGD):
  [510] -> trampoline_pmd          (VPN2=510 for KERNEL_VIRT)

trampoline_pmd (PMD):
  [0] -> 0x80200000 (first 2MB: text, rodata, data)     V|R|W|X|G|A|D
  [1] -> 0x80400000 (second 2MB: bss + stack)           V|R|W|X|G|A|D
  [2] -> 0x80600000 (third 2MB)                          V|R|W|X|G|A|D
  [3] -> 0x80800000 (fourth 2MB)                         V|R|W|X|G|A|D
```

#### Step 5: Create Early Page Tables

The early page table provides the **full** early mapping including MMIO identity mapping:

```
early_pg_dir (PGD):
  [0]   -> early_pmd_io            (identity map first 1GB for MMIO)
  [510] -> early_pmd               (VPN2=510 for KERNEL_VIRT)

early_pmd (PMD):
  [0] -> 0x80200000 (first 2MB)    V|R|W|X|G|A|D
  [1] -> 0x80400000 (second 2MB)   V|R|W|X|G|A|D

early_pmd_io (PMD):
  [0]   -> 0x00000000 (identity, first 2MB)              V|R|W|A|D|G
  [128] -> 0x10000000 (UART MMIO, 2MB)                   V|R|W|A|D|G
  [256] -> 0x80200000 (identity, kernel start 2MB)       V|R|W|X|G|A|D
```

The identity mapping of `0x80200000` in `early_pmd_io` is needed for the Fixmap-stage page table allocator, which uses identity mapping to access physical addresses.

#### Step 6: Save DTB Pointer

```asm
    la t0, dtb_pointer
    sub t0, t0, t1               // physical address of dtb_pointer
    sd s0, 0(t0)                 // save DTB physical address
```

#### Step 7: Initialize KERNEL_MAP Structure

The `KERNEL_MAP` structure is a `KernelMapping` (defined in `memory_layout.rs`) initialized at boot time. It mirrors Linux's `kernel_mapping` from `arch/riscv/include/asm/page.h`.

```asm
    la t0, KERNEL_MAP
    sub t0, t0, t1               // physical address
    li t2, KERNEL_VIRT
    sd t2, 0(t0)                 // virt_addr = 0xFFFFFFFF80000000
    li t2, KERNEL_PHYS
    sd t2, 8(t0)                 // phys_addr = 0x80200000
    // va_kernel_pa_offset = KERNEL_VIRT - KERNEL_PHYS
    sub t2, t2, t2               // t2 = 0 (clear)
    li t2, KERNEL_VIRT
    li t3, 1025
    slli t3, t3, 21              // 0x80200000
    sub t2, t2, t3               // va_kernel_pa_offset
    sd t2, 40(t0)                // offset 40 = field 5
```

The `va_pa_offset` field is filled later in `rust_main()` after `PAGE_OFFSET` is determined.

#### Step 8: Enable MMU (Trampoline)

This is the critical transition from physical to virtual address execution:

```asm
    // Set stvec to VA of the instruction AFTER csrw satp
    la t3, 2f
    csrw stvec, t3

    // Compute satp value for trampoline page table
    la t0, trampoline_pg_dir
    sub t0, t0, t1               // physical address
    srli t0, t0, 12              // PPN
    li t2, SATP_MODE_SV39        // 0x8000000000000000
    or t0, t0, t2

    // Enable MMU!
    sfence.vma
    csrw satp, t0

    // The NEXT instruction fetch will fault because:
    // - PC is at physical address
    // - MMU now translates it via trampoline_pg_dir
    // - But the trampoline only maps KERNEL_VIRT region
    // - The fault triggers a trap to stvec (set to VA of "2:")
    // - This effectively "jumps" us to the virtual address!

    .align 2
2:  // Now running at VIRTUAL address!
```

**How the trampoline works:**

1. `csrw satp, t0` enables Sv39 MMU with the trampoline page table
2. The very next instruction fetch at the physical PC address causes a page fault
3. The CPU traps to `stvec` (which was set to the VA of label `2:`)
4. The trap mechanism always uses the virtual address in `stvec`
5. Execution continues at label `2:` at the virtual address `0xFFFFFFFF80000000 + offset`

This technique is used by Linux (`arch/riscv/kernel/head.S`) and avoids the need for identity-mapped kernel code.

#### Step 9: Fix Up Stack Pointer

```asm
    la t0, _stack_top            // Now la returns VA (PC-relative in VA space)
    mv sp, t0                    // sp = virtual address of stack top
```

#### Step 10: Reload Global Pointer

```asm
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop
```

#### Step 11: Switch to Full Early Page Table

```asm
    la t0, early_pg_dir          // VA
    sub t0, t0, t2               // Convert to physical for satp
    srli t0, t0, 12
    li t2, SATP_MODE_SV39
    or t0, t0, t2
    csrw satp, t0
    sfence.vma
```

The early page table provides:
- Kernel at `KERNEL_VIRT` (8MB via `early_pmd`)
- UART identity mapping at `0x10000000` (via `early_pmd_io`)
- Kernel identity mapping at `0x80200000` (via `early_pmd_io`)

#### Step 12: Jump to rust_main

```asm
    la t0, trap_entry
    csrw stvec, t0               // Set trap vector

    la t0, rust_main
    jr t0                        // Jump to Rust code at virtual address
```

---

## 3. `rust_main()` - Kernel Initialization

**File**: `kernel/src/main.rs`

`rust_main()` takes no arguments. The DTB pointer is stored in the global `dtb_pointer` variable (set by `boot.S`), accessed via `arch::riscv64::boot::get_dtb_pointer()`.

### 3.1 Phase 1: Early Initialization (No Heap)

```rust
pub extern "C" fn rust_main() -> ! {
    // 1. SMP init - only boot hart continues, others enter WFI loop
    let is_boot_hart = arch::smp::init();

    // 2. Initialize per-CPU interrupt stacks
    arch::smp::init_per_cpu_intr_stacks();

    // 3. Console (first output-capable subsystem)
    console::init();

    // 4. Trap handling (stvec, sscratch)
    arch::trap::init();
    arch::trap::init_syscall();

    // 5. MMU init (permanent page table, switch from early_pg_dir)
    arch::mm::init();

    // 6. Set va_pa_offset for phys_to_virt()
    KERNEL_MAP.va_pa_offset = VA_PA_OFFSET;
```

### 3.2 Phase 2: Memory Management Setup

```rust
    // 7. memblock initialization
    mm::memblock_init();

    // 8. Parse DTB memory regions
    let memory_regions = cmdline::parse_memory_regions(dtb_phys);

    // 9. Reserve memory (OpenSBI + kernel, heap, slab)
    mm::memblock_reserve(0x80000000, 0xA00000);
    mm::memblock_reserve(heap_start, heap_size);
    mm::memblock_reserve(slab_start, slab_size);

    // 10. Setup linear mapping at PAGE_OFFSET
    //     Maps ALL physical memory: phys + VA_PA_OFFSET -> virt
    arch::riscv64::mm::setup_linear_mapping(&memory_regions);

    // 11. Switch to Fixmap stage (linear mapping now available)
    arch::riscv64::mm::pt_ops_set_fixmap();

    // 12. Initialize heap allocator
    mm::init_heap();

    // 13. Initialize slab allocator
    mm::init_slab(slab_start, 4 * 1024 * 1024);
```

### 3.3 Phase 3: Late Initialization (Heap Available)

```rust
    // 14. Parse DTB command line (now with linear mapping)
    cmdline::init(dtb_ptr);

    // 15. vmemmap initialization
    mm::vmemmap::init_vmemmap(start_pfn, nr_pages);

    // 16. Kernel memory layout
    mm::layout::kernel_layout_init(layout);

    // 17. Page descriptors
    mm::page::init_page_descriptors(start_pfn, nr_pages);

    // 18. Zone allocator
    mm::init_zone_system(0x80000000, total_phys_memory, kernel_end);

    // 19. Switch to Late stage (buddy allocator for page tables)
    arch::riscv64::mm::pt_ops_set_late();

    // 20. Device mappings (VirtIO, PLIC, CLINT, PCIe)
    arch::riscv64::mm::setup_device_mappings();
```

### 3.4 Phase 4: Drivers and Subsystems

```rust
    // 21. PLIC (interrupt controller)
    drivers::intc::init();

    // 22. IPI (inter-processor interrupt)
    arch::ipi::init();

    // 23. Filesystem
    fs::bio::init();              // Buffer cache
    fs::ext4::init();             // ext4 driver
    fs::rootfs::init_rootfs();    // ramfs at /
    fs::procfs::init_procfs();    // procfs at /proc

    // 24. Block devices (VirtIO-blk MMIO + PCI)
    drivers::probe::init_block_devices();
    drivers::probe::init_pci_block_devices();

    // 25. Mount ext4
    fs::ext4::mount_ext4(disk);

    // 26. Network devices
    drivers::probe::init_network_devices();

    // 27. Process scheduler
    sched::init();

    // 28. Enable external interrupts
    arch::trap::enable_external_interrupt();

    // 29. Input devices + devfs
    drivers::input::init();
    fs::devfs::init();
```

### 3.5 Phase 5: Init Process and Scheduler

```rust
    // 30. Load and start init process (PID 1)
    init::init();

    // 31. Enable timer interrupts
    arch::trap::enable_timer_interrupt();

    // 32. Enter scheduler idle loop
    sched::cpu_idle_loop();
```

---

## 4. Virtual Memory Layout

### 4.1 Sv39 Address Space

RISC-V Sv39 uses 39-bit virtual addresses with sign extension:

```
User Space (256GB):    0x0000000000000000 - 0x0000003FFFFFFFFF
                      VPN2[0..255]

Kernel Space (256GB):  0xFFFFFFC000000000 - 0xFFFFFFFFFFFFFFFF
                      VPN2[256..511]
```

### 4.2 Kernel Virtual Memory Layout (Linux-compatible)

```
High Address
0xFFFFFFFF_FFFFFFFF +-----------------------+
                    |                       |
0xFFFFFFFF_80000000 | Kernel Image Mapping  |  VPN2[510]
                    | (text/data/bss)       |  KERNEL_LINK_ADDR
                    |                       |
0xFFFFFFFF_7FFFFFFF +-----------------------+
                    | (unmapped gap)        |
                    +-----------------------+
                    | VMALLOC (64GB)        |  VMALLOC_START .. VMALLOC_END
                    |                       |  = PAGE_OFFSET - 64GB
                    +-----------------------+
                    | vmemmap (4GB)         |  VMEMMAP_START .. VMEMMAP_END
                    | (page descriptors)    |  = VMALLOC_START - 4GB
0xFFFFFFD6_00000000 +-----------------------+
                    | Linear Mapping        |  PAGE_OFFSET
                    | (phys + VA_PA_OFFSET) |  Dynamically sized
                    |                       |  Maps all physical memory
                    +-----------------------+
0xFFFFFFC0_00000000 | (user/kernel boundary)|
Low Address         +-----------------------+
```

### 4.3 Key Address Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `KERNEL_LINK_ADDR` | `0xFFFFFFFF80000000` | Kernel VMA (VPN2[510]) |
| `KERNEL_PHYS` | `0x80200000` | Kernel LMA (where OpenSBI loads) |
| `PAGE_OFFSET` | `0xFFFFFFD600000000` | Start of linear mapping |
| `VA_PA_OFFSET` | `PAGE_OFFSET - 0x80000000` | phys_to_virt conversion |
| `va_kernel_pa_offset` | `KERNEL_LINK_ADDR - KERNEL_PHYS` | Kernel VA/PA conversion |
| `VMALLOC_START` | `PAGE_OFFSET - 64GB` | VMALLOC region start |
| `VMEMMAP_START` | `VMALLOC_START - 4GB` | vmemmap region start |
| `TASK_SIZE` | 256GB | Maximum user virtual address |

### 4.4 Address Conversion Functions

```rust
// Linear mapping: phys -> virt
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new(phys.0 + KERNEL_MAP.va_pa_offset)
}

// Linear mapping: virt -> phys
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    PhysAddr::new(virt.0 - KERNEL_MAP.va_pa_offset)
}

// Kernel mapping: phys -> kernel virt
// Used for ROOT_PAGE_TABLE and static BSS data
let root_phys = root_virt - KERNEL_MAP.va_kernel_pa_offset;
```

---

## 5. Page Table Allocation (Three-Stage)

The kernel uses a Linux-style three-stage page table allocation strategy:

### 5.1 Stages

| Stage | Allocator | Virtual Address Access | When |
|-------|-----------|----------------------|------|
| **Early** | Static BSS arrays | `virt = phys + va_kernel_pa_offset` | Before linear mapping |
| **Fixmap** | memblock | `virt = phys + va_pa_offset` (linear mapping) | After linear mapping, before buddy |
| **Late** | Zone buddy allocator | `virt = phys + va_pa_offset` (linear mapping) | After zone allocator init |

### 5.2 Early Stage Details

Static arrays in `.bss` section at `KERNEL_LINK_ADDR`:

```rust
#[link_section = ".bss"]
static mut EARLY_PMD: [PageTable; 8] = ...;    // 8 L1 tables (8GB coverage)
#[link_section = ".bss"]
static mut EARLY_PTE: [PageTable; 128] = ...;  // 128 L0 tables (256MB coverage)
```

Since these are at `KERNEL_LINK_ADDR`, the physical address is:
```
phys = virt - va_kernel_pa_offset
```

### 5.3 Stage Transitions

```rust
// After setup_linear_mapping() creates linear mapping:
pt_ops_set_fixmap();    // Early -> Fixmap

// After zone allocator is initialized:
pt_ops_set_late();      // Fixmap -> Late
```

---

## 6. MMU Initialization (`arch::mm::init()`)

**File**: `kernel/src/arch/riscv64/mm/mmu_init.rs`

Called from `rust_main()` after trap handling is set up. At this point, MMU is already enabled via the boot.S trampoline (using `early_pg_dir`).

### 6.1 What `mm::init()` Does

1. **Clear `ROOT_PAGE_TABLE`** (in `.bss`)
2. **Map kernel at `KERNEL_LINK_ADDR`** using 2MB huge pages:
   - `KERNEL_VIRT(0xFFFFFFFF80000000) -> KERNEL_PHYS(0x80200000)`
   - Uses `map_pmd_huge_page()` to avoid allocating many L0 tables
3. **Map UART** at physical address `0x10000000` (identity mapping, temporary)
4. **Map DTB** at linear mapping address
5. **Initialize UART fixmap**
6. **Switch to `ROOT_PAGE_TABLE`** via `MmStruct::new_kernel(root_ppn).enable()`

### 6.2 Permanent Page Table Structure (after init)

```
ROOT_PAGE_TABLE (PGD):
  [0]   -> early_pmd_dev           (UART identity mapping, temporary)
  [510] -> early_pmd               (KERNEL_LINK_ADDR, 2MB huge pages)
  ... (rest filled by setup_linear_mapping)
```

### 6.3 Linear Mapping Setup

Called from `rust_main()` after `mm::init()`:

```rust
arch::riscv64::mm::setup_linear_mapping(&memory_regions);
```

Maps ALL physical memory at `PAGE_OFFSET`:
```
virt = phys + VA_PA_OFFSET
```

Uses `best_map_size()` to select 2MB huge pages when aligned, falling back to 4KB pages for unaligned regions.

After linear mapping is established, `phys_to_virt()` and `virt_to_phys()` work for all physical memory.

---

## 7. Trap Handling

**File**: `kernel/src/arch/riscv64/trap.S`, `kernel/src/arch/riscv64/trap.rs`

### 7.1 sscratch/tp Protocol

| State | sscratch | tp |
|-------|----------|----|
| Kernel running | 0 | current task pointer |
| User running | current task pointer | user TLS (tp) |

On trap entry:
```asm
csrrw tp, sscratch, tp    // Swap tp and sscratch
beqz tp, .Lfrom_kernel    // tp=0: from kernel
bltu tp, 0x80000000, .Learly_boot  // tp<0x80000000: early boot
j .Lfrom_user             // Otherwise: from user mode
```

### 7.2 Early Boot Phase

When `tp < 0x80000000` (e.g., `tp = hart_id` during early boot), the trap handler falls into the early boot path which uses `sstatus.SPP` to determine kernel vs user mode, and does not use the kernel big lock.

### 7.3 PtRegs Layout (288 bytes)

```
Offset  Field       Description
------  -----       -----------
0x00    epc         Program counter (sepc)
0x08    ra          x1
0x10    sp          x2
0x18    gp          x3
0x20    tp          x4
0x28-0xF8           x5-x31 (t0-t6, s0-s11, a0-a7)
0x100   status      sstatus CSR
0x108   badaddr     stval CSR
0x110   cause       scause CSR
0x118   orig_a0     Original a0 (for syscall rollback)
```

### 7.4 Return from Trap

`ret_from_exception` is the common return path for both trap returns and fork child processes:

1. Check `sstatus.SPP` to determine user vs kernel return
2. For user return: save `ti_kernel_sp`, check signals, check `need_resched`
3. Restore registers from PtRegs
4. `sret` to user mode

---

## 8. Init Process Boot

**File**: `kernel/src/init.rs`

### 8.1 Init Creation Flow

1. Load ELF program from filesystem (try PCI ext4, MMIO ext4, rootfs)
2. Create task structure (`Task::new_task_at`)
3. Create user address space with `create_user_address_space()`
4. Allocate and map user memory
5. Load ELF segments into physical memory (via linear mapping)
6. Set up user stack with argc, argv, auxv (AT_PHDR, AT_ENTRY, AT_PAGESZ, etc.)
7. Create PtRegs at kernel stack top with user entry point and stack
8. Set `thread.ra = ret_from_exception`, `thread.sp = pt_regs`
9. Register VMAs for ELF segments and stack
10. Add to scheduler run queue

### 8.2 First User Mode Switch

The init process returns to user mode through `ret_from_exception`:
- `__switch_to` restores `sp = thread.sp` (which points to PtRegs)
- `ret_from_exception` checks `SPP=0` (user mode), restores registers from PtRegs
- `sret` jumps to `sepc` (user entry point) with `sp` (user stack)

### 8.3 Fork Child Return

Forked children use a similar path through `ret_from_fork_user_asm`:
1. `schedule_tail(prev)` cleans up previous task
2. `ret_from_fork_user(regs)` is called (currently a no-op)
3. Falls through to `ret_from_exception`
4. PtRegs was set up by `copy_thread()` with `a0=0` (child return value)

---

## 9. SMP Multi-core Boot

**File**: `kernel/src/arch/riscv64/smp.rs`

### 9.1 Boot Hart Detection

All harts enter `rust_main()` simultaneously. The first hart to execute `smp::init()` wins a CAS (compare-and-swap) on `ACTUAL_BOOT_HART` and becomes the boot hart. Other harts wait for `SMP_INIT_DONE` flag, then enter WFI loop.

```rust
pub fn init() -> bool {
    if ACTUAL_BOOT_HART.compare_exchange(u32::MAX, my_hart, ...).is_ok() {
        SMP_INIT_DONE.store(1, Ordering::Release);
        true  // boot hart
    } else {
        while SMP_INIT_DONE.load(Ordering::Acquire) == 0 {
            asm!("wfi");
        }
        false  // secondary hart
    }
}
```

### 9.2 Secondary Harts

Secondary harts do not execute any kernel initialization. They remain in a WFI loop until the scheduler is initialized and explicitly starts them via SBI HSM (`sbi::hart_start()`).

### 9.3 Per-CPU Data

- **Interrupt stacks**: 16KB per CPU in `.bss` section (`PER_CPU_INTR_STACKS[4]`)
- **CPU ID detection**: Early boot uses `tp = hart_id`; after scheduler uses `task_struct.ti_cpu`

---

## 10. Device Mappings

**File**: `kernel/src/arch/riscv64/mm/mmu_init.rs`

`setup_device_mappings()` creates identity mappings for all MMIO regions:

| Device | Physical Range | Size | Purpose |
|--------|---------------|------|---------|
| UART | `0x10000000` | 4KB | Console I/O |
| VirtIO MMIO | `0x10001000` | 32KB | Block/Net/GPU/Input |
| PLIC | `0x0C000000` | ~2MB | Interrupt controller |
| CLINT | `0x02000000` | 64KB | Timer/IPI |
| PCIe ECAM | `0x30000000` | 1MB | PCI config space |
| PCI MMIO | `0x40000000` | 256MB | PCI device memory |

These mappings use `map_kernel_region()` which maps `virt=phys` (identity mapping) with device flags (V|R|W|A|D|G, no X).

---

## 11. Linker Script Details

**File**: `kernel/src/arch/riscv64/linker.ld`

```
VMA = 0xFFFFFFFF80000000 (KERNEL_VIRT)
LMA = 0x80200000 (KERNEL_PHYS)

.text     @ VMA, loaded at LMA         (code)
.rodata   @ VMA, loaded at LMA+delta   (read-only data)
.data     @ VMA, loaded at LMA+delta   (initialized data)
.bss      @ VMA, loaded at LMA+delta   (zero-initialized)
.stack    @ VMA (256KB)                (boot stack)
```

The code model must be `medany` (set in `.cargo/config.toml`) to enable PC-relative addressing, which works at both physical and virtual addresses.

---

## 12. Key Initialization Order

### 12.1 Required Dependencies

| Order | Module | Depends On |
|-------|--------|-----------|
| 1 | SMP init | (none) |
| 2 | Per-CPU intr stacks | (none) |
| 3 | Console | UART MMIO (boot.S identity mapping) |
| 4 | Trap | Console (for debug output) |
| 5 | MMU init | Trap (for page fault handling) |
| 6 | va_pa_offset | MMU init |
| 7 | memblock | va_pa_offset |
| 8 | DTB parse | memblock |
| 9 | Linear mapping | memblock, Early page tables |
| 10 | Fixmap stage | Linear mapping |
| 11 | Heap | Linear mapping |
| 12 | Slab | Heap |
| 13 | vmemmap | Heap |
| 14 | Page descriptors | vmemmap |
| 15 | Zone allocator | Page descriptors |
| 16 | Late stage | Zone allocator |
| 17 | Device mappings | Late stage |
| 18 | PLIC | Device mappings |
| 19 | Filesystem | PLIC, Device mappings |
| 20 | Scheduler | Heap |
| 21 | Init process | Scheduler, Filesystem |
| 22 | Timer interrupt | Init process (after user programs loaded) |

### 12.2 Critical Ordering Notes

- **Console before everything else**: Needed for `print!`/`println!` in subsequent init
- **Trap before MMU**: Page faults during page table setup need trap handler
- **Linear mapping before heap**: `phys_to_virt()` needs linear mapping to access physical memory
- **Device mappings before PLIC**: PLIC registers are MMIO-mapped
- **Timer interrupt last**: Avoids timer interrupts interfering with init process loading

---

## 13. Troubleshooting

### 13.1 Boot Failure (No Output)

**Symptoms**: No output after OpenSBI banner

**Check**:
1. Is `_start` at physical address `0x80200000`? (check linker script `ENTRY(_start)` and `boot.o` link order)
2. Is `VA_OFFSET` computed correctly? (`0xFFFFFFFF80000000 - 0x80200000`)
3. Is BSS cleared? (check `__bss_start`/`__bss_end` VA-to-PA conversion)
4. Is stack valid? (check `_stack_bottom`/`_stack_top` VA-to-PA conversion)

### 13.2 MMU Trampoline Failure

**Symptoms**: Hangs immediately after `csrw satp`

**Check**:
1. Is `stvec` set to VA of the label after `csrw satp`?
2. Is trampoline page table correctly mapping `KERNEL_VIRT` -> first 2MB?
3. Is `sfence.vma` executed before `csrw satp`?

### 13.3 Page Fault in Kernel Mode

**Symptoms**: `trap: Kernel panic - page fault`

**Check**:
1. Is the address in linear mapping region? (`addr >= PAGE_OFFSET`)
2. Is the physical address valid? (`addr - VA_PA_OFFSET >= 0x80000000`)
3. Is the page table entry correct? (check permissions, PPN)
4. For function pointers: are they already virtual addresses? (no `+ VA_PA_OFFSET` needed)

### 13.4 User Process Hangs

**Symptoms**: Shell starts but no output/input

**Check**:
1. Are kernel PGD entries (VPN2[256..511]) copied to user page table? (`copy_kernel_mappings`)
2. Is MMIO identity mapping available in user page table? (needed for syscalls that access devices)
3. Is `copy_kernel_mappings` creating new L0 tables instead of sharing kernel pointers?

---

## References

- [RISC-V Privileged Architecture Specification](https://riscv.org/technical/specifications/)
- [OpenSBI Documentation](https://github.com/riscv/opensbi)
- [Linux RISC-V Boot](https://kernel.org/doc/html/latest/riscv/boot.html)
- Linux `arch/riscv/kernel/head.S` - MMU trampoline
- Linux `arch/riscv/include/asm/page.h` - PAGE_OFFSET definitions
- Linux `arch/riscv/mm/init.c` - Memory initialization

---

**Document Version**: v4.0.0
**Last Updated**: 2026-04-09
