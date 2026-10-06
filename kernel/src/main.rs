//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]

extern crate log;
extern crate alloc;

use core::panic::PanicInfo;
use alloc::format;

mod arch;

/// Print initialization status message
///
/// # Arguments
/// - `module`: Module name
/// - `desc`: Feature description
/// - `success`: Whether successful
///
/// # Format
/// Success: "module:             desc              [ok]"
/// Failure: Red line "module:             desc              [fail]"
pub fn print_status(module: &str, desc: &str, success: bool) {
    print_status_ex(module, desc, if success { Some(true) } else { Some(false) });
}

/// Print initialization status message with extended status support.
///
/// # Arguments
/// - `module`: Module name
/// - `desc`: Feature description
/// - `success`: Some(true) = [ok], Some(false) = [fail], None = [done] (neutral)
pub fn print_status_ex(module: &str, desc: &str, success: Option<bool>) {
    // ANSI color codes
    const RED: &[u8] = b"\x1b[31m";
    const RESET: &[u8] = b"\x1b[0m";
    const OK: &[u8] = b"[ok]";
    const FAIL: &[u8] = b"[fail]";
    const DONE: &[u8] = b"[done]";

    unsafe {
        use crate::console::putchar;

        // Print red start code on failure
        if success == Some(false) {
            for &b in RED {
                putchar(b);
            }
        }

        // Print module name + colon (fixed width 16 chars, left-aligned)
        for b in module.as_bytes() {
            putchar(*b);
        }
        putchar(b':');
        let module_len = module.len() + 1; // +1 for colon
        if module_len < 16 {
            for _ in 0..(16 - module_len) {
                putchar(b' ');
            }
        }

        // Print description (fixed width 32 chars, left-aligned, truncate if too long)
        // Print 2 spaces first as column separator
        putchar(b' ');
        putchar(b' ');
        let desc_bytes = desc.as_bytes();
        let desc_len = if desc_bytes.len() > 32 { 32 } else { desc_bytes.len() };
        for i in 0..desc_len {
            putchar(desc_bytes[i]);
        }
        if desc_len < 32 {
            for _ in 0..(32 - desc_len) {
                putchar(b' ');
            }
        }
        // Leave 3 spaces before status column for alignment
        putchar(b' ');
        putchar(b' ');
        putchar(b' ');

        // Print status symbol
        match success {
            Some(true) => {
                for &b in OK { putchar(b); }
            }
            Some(false) => {
                for &b in FAIL { putchar(b); }
            }
            None => {
                for &b in DONE { putchar(b); }
            }
        }

        // Print color reset code on failure
        if success == Some(false) {
            for &b in RESET {
                putchar(b);
            }
        }

        putchar(b'\n');
    }
}

#[cfg(feature = "riscv64")]
mod sbi;
mod mm;
mod console;
mod print;
mod printk;
mod drivers;
mod config;
mod list;
mod process;
mod sched;
mod fs;
mod signal;
mod security;
mod sync;
mod errno;
mod net;
mod cmdline;
mod init;
mod syscall;
mod interrupt;
mod dfx;
mod ipc;
mod io_uring;
mod timer;
mod module;

#[cfg(feature = "unit-test")]
mod tests;

// Allocation error handler for no_std
//
// OOM defense (GNOME-oom postmortem): when the kernel heap is exhausted the
// panic path used to be the FIRST casualty — `panic!` formatting touches
// subsystems that want the very heap that just failed, and the machine died
// with an empty log. Before panicking, emit one allocation-free forensic
// line over the raw UART (SBI putchar, no locks, no fmt buffers): the
// failing layout size plus the return-address chain of the caller that
// asked for the memory, for offline addr2line. The chain lives entirely in
// live frames (this handler <- alloc::alloc::alloc <- __rust_alloc <- the
// kernel caller), so the validated walk in memwatch::walk_fp_chain cannot
// fault.
//
// ENOMEM defense (heap-v2): the handler used to end in an unconditional
// `panic!`, which stops every CPU — one process's OOM killed the whole
// machine. Linux answers a GFP_KERNEL allocation failure with SIGKILL/OOM
// for the offending task, not a kernel halt. `#[alloc_error_handler]` has
// signature `fn(Layout) -> !`, so "return null to the caller" is not
// literally possible here (fallible callers already see null from
// GlobalAlloc::alloc); what we CAN do is scope the damage: when the failure
// arrives on the retryable path — task (process) context, IRQs enabled (no
// lock_irqsave held), scheduler up — kill the CURRENT task with SIGKILL and
// let its teardown (which frees memory) run. Only IRQ/atomic-context
// failures, early-boot failures, or a second failure during the teardown
// itself still fall through to panic.
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};

    taskdump_raw_line(b"\nALLOCTHROW size=");
    taskdump_dec(layout.size() as u64);
    taskdump_raw_line(b" align=");
    taskdump_dec(layout.align() as u64);
    let mut frames: [u64; 6] = [0; 6];
    // SAFETY: reads the frame-pointer chain of the current stack.
    unsafe {
        let fp = crate::dfx::backtrace::current_frame_pointer();
        crate::dfx::memwatch::walk_fp_chain(fp, &mut frames);
    }
    taskdump_raw_line(b" frames:");
    for f in frames.iter() {
        if *f == 0 {
            break;
        }
        taskdump_raw_line(b" ");
        taskdump_raw_line(format_hex(*f).as_bytes());
    }
    taskdump_raw_line(b"\n");

    // Scope the blast radius: process-context failures kill the offending
    // TASK (the kernel survives), everything else panics as before. The
    // TIF_MEMDIE latch is per-task (the same primitive mm/oom_kill.rs uses)
    // and prevents recursion: do_exit's own teardown (fd closes, mm drop,
    // printk) can allocate again and must land in panic, not back here.
    // SIGKILL (9) never triggers the coredump path inside do_exit, and the
    // flag dies with the task, so later failures on other tasks keep the
    // task-kill defense.
    let kill_self = crate::interrupt::preempt::in_task()
        && crate::arch::cpu::get_interrupts_state()
        && match crate::sched::current() {
            Some(task) => {
                use crate::process::task::TIF_MEMDIE;
                if task.test_ti_flag(TIF_MEMDIE) {
                    false
                } else {
                    task.set_ti_flag(TIF_MEMDIE);
                    true
                }
            }
            None => false,
        };
    if kill_self {
        taskdump_raw_line(b"ALLOCTHROW -> SIGKILL current task (task-context ENOMEM)\n");
        crate::process::exit::do_exit(-9);
    }
    panic!("Allocation error: {:?}", layout);
}

/// 16-hex-digit formatting into a fixed buffer (no allocator — usable on
/// the exhausted-heap path above; `format!` is NOT).
fn format_hex(v: u64) -> &'static str {
    static mut BUF: [u8; 19] = [b'0'; 19];
    // SAFETY: single-panicking-CPU path; the UART line below is emitted
    // before any other CPU can reach this code (they are stopped in the
    // panic handler shortly after).
    unsafe {
        BUF[0] = b'0';
        BUF[1] = b'x';
        for i in 0..16 {
            let nib = (v >> ((15 - i) * 4)) & 0xF;
            BUF[2 + i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
        }
        core::str::from_utf8_unchecked(&BUF)
    }
}

// x86_64 link shim for the trap-entry symbol generic process code
// references. arch/x86_64/trap.S is not written yet (X86-TODO agent
// x86-trap pins `ret_from_fork` in its contract); this WEAK definition
// only satisfies the link — the trap.S global definition overrides it
// automatically once that file lands.
#[cfg(feature = "x86_64")]
core::arch::global_asm!(
    r#"
.section .text.x86_trampoline_shim, "ax"
.weak ret_from_fork
ret_from_fork:
    hlt
    jmp ret_from_fork
"#
);

// Include platform-specific assembly code
#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

/// Kernel reservation at the RAM base (OpenSBI + kernel on riscv64; low
/// memory + kernel on x86_64) and the heap's physical base.
#[cfg(feature = "riscv64")]
const KERNEL_RESERVE_SIZE: usize = 0xC0_0000; // 12MB
#[cfg(feature = "x86_64")]
const KERNEL_RESERVE_SIZE: usize = 0x1E0_0000; // 30MB
#[cfg(feature = "riscv64")]
const KERNEL_HEAP_PHYS: usize = 0x80C0_0000;
#[cfg(feature = "x86_64")]
const KERNEL_HEAP_PHYS: usize = 0x4000_0000;

/// Physical base of RAM and the kernel's physical load address, per arch.
#[cfg(feature = "riscv64")]
const MEMORY_PHYS_BASE: usize = 0x8000_0000;
#[cfg(feature = "riscv64")]
const KERNEL_PHYS_LOAD_ADDR: usize = 0x8020_0000;
#[cfg(feature = "x86_64")]
const MEMORY_PHYS_BASE: usize = 0x0000_0000;
#[cfg(feature = "x86_64")]
const KERNEL_PHYS_LOAD_ADDR: usize = 0x0020_0000; // multiboot1 LMA

/// Usable memory regions from the multiboot/e820 map (x86_64).
///
/// Allocation-free: this runs BEFORE the heap exists (setup_linear_mapping
/// needs the regions first). Fill a static BSS array instead of collecting
/// into a Vec.
#[cfg(feature = "x86_64")]
fn x86_boot_memory_regions() -> &'static [cmdline::MemoryRegion] {
    static mut REGIONS: [cmdline::MemoryRegion; 64] =
        [cmdline::MemoryRegion { base: 0, size: 0 }; 64];
    let mut n = 0usize;
    for r in arch::boot::boot_memory_regions() {
        if !r.usable || n >= 64 {
            continue;
        }
        // SAFETY: single-threaded early boot; the array is written once
        // before any reference to it escapes.
        unsafe {
            REGIONS[n] = cmdline::MemoryRegion {
                base: r.start as usize,
                size: (r.end - r.start) as usize,
            };
        }
        n += 1;
    }
    // SAFETY: the first n entries were just initialized.
    unsafe { &REGIONS[..n] }
}

// Kernel main function
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // Initialize SMP (multi-core support) - must run first!
    // On QEMU virt, OpenSBI only starts one hart into S-mode.
    // Other harts will be started later via SBI HSM.
    arch::smp::init();

    // Initialize per-CPU interrupt stacks (must be before any traps)
    arch::smp::init_per_cpu_intr_stacks();

    // ========== The following code is only executed by the boot hart ==========

    // Initialize console (must be first, so other initialization can print)
    console::init();
    printk::init();
    printk::init_logger();

    // Print boot banner with ASCII art logo
    unsafe {
        use crate::console::putchar;

        // ANSI colors
        const CYAN: &[u8] = b"\x1b[36m";
        const BOLD: &[u8] = b"\x1b[1m";
        const GREEN: &[u8] = b"\x1b[32m";
        const RESET: &[u8] = b"\x1b[0m";

        // Print logo in cyan bold
        for &b in CYAN { putchar(b); }
        for &b in BOLD { putchar(b); }

        // ASCII Art Logo - RUX (using UTF-8 block character)
        // Block = 0xE2 0x96 0x88 (3 bytes in UTF-8)
        const L1: &[u8] = b"\n\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\n";
        const L2: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88\n";
        const L3: &[u8] = b"\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\n";
        const L4: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88\n";
        const L5: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\n";

        for &b in L1 { putchar(b); }
        for &b in L2 { putchar(b); }
        for &b in L3 { putchar(b); }
        for &b in L4 { putchar(b); }
        for &b in L5 { putchar(b); }

        // Reset before version info
        for &b in RESET { putchar(b); }

        // Print version info
        for &b in GREEN { putchar(b); }
        const VERSION: &[u8] = b"  [ RISC-V 64-bit | POSIX Compatible | v";
        for &b in VERSION { putchar(b); }
        let ver = env!("CARGO_PKG_VERSION");
        for b in ver.as_bytes() { putchar(*b); }
        const END: &[u8] = b" ]\n\n";
        for &b in END { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    // Initialize trap handling
    arch::trap::init();

    arch::trap::init_syscall();

    // Initialize MMU (must be before heap initialization)
    arch::mm::init();

    // Set va_pa_offset so phys_to_virt() works for subsequent initialization
    // This must be done before any code that uses phys_to_virt() or
    // accesses physical memory via linear mapping
    unsafe {
        arch::mm::memory_layout::KERNEL_MAP.va_pa_offset =
            arch::mm::VA_PA_OFFSET;
    }

    // ===== Setup linear mapping BEFORE heap (heap needs phys_to_virt) =====
    // paging_init approach:
    // 1. Initialize memblock
    // 2. Parse memory regions from DTB
    // 3. Create linear mapping at PAGE_OFFSET
    {
        // Initialize memblock
        mm::memblock_init();

        // Acquire the boot memory map: FDT on riscv64 (DTB is mapped by
        // boot.S's early page table at its physical address), the
        // bootloader-provided multiboot/e820 map (copied to BSS by
        // early_boot_init) on x86_64.
        #[cfg(feature = "riscv64")]
        let dtb_phys = arch::boot::get_dtb_pointer();
        #[cfg(feature = "riscv64")]
        let memory_regions = unsafe { cmdline::parse_memory_regions(dtb_phys) };
        #[cfg(feature = "x86_64")]
        let memory_regions = x86_boot_memory_regions();

        // Add memory regions to memblock
        for region in memory_regions.iter() {
            mm::memblock_add(region.base, region.size).ok();
        }

        // Reserve memory regions (kernel, heap, slab)
        let heap_start = KERNEL_HEAP_PHYS;
        let heap_size = crate::config::KERNEL_HEAP_SIZE;
        let slab_start = heap_start + heap_size;
        let slab_size = 4 * 1024 * 1024;

        #[cfg(feature = "riscv64")]
        mm::memblock_reserve(0x80000000, KERNEL_RESERVE_SIZE).ok();  // OpenSBI + kernel
        #[cfg(feature = "x86_64")]
        mm::memblock_reserve(0, KERNEL_RESERVE_SIZE).ok(); // low memory + kernel
        mm::memblock_reserve(heap_start, heap_size).ok(); // Heap
        mm::memblock_reserve(slab_start, slab_size).ok(); // Slab

        // Stay in Early stage for setup_linear_mapping (static BSS arrays always accessible)
        // Don't switch to Fixmap yet — Fixmap stage uses identity mapping which
        // doesn't exist in the permanent page table

        // Setup linear mapping (PAGE_OFFSET region)
        arch::mm::setup_linear_mapping(&memory_regions);

        // Now switch to fixmap stage (linear mapping is available)
        arch::mm::pt_ops_set_fixmap();

        // Calculate total physical memory for later use
        let total_phys_memory: usize = memory_regions.iter().map(|r| r.size).sum();
    }

    // Now linear mapping is available, phys_to_virt() works for all physical memory
    // Initialize heap allocator
    mm::init_heap();

    // Initialize Slab allocator (use virtual address in linear mapping region)
    let slab_phys = KERNEL_HEAP_PHYS + crate::config::KERNEL_HEAP_SIZE;
    let slab_start = slab_phys + arch::mm::VA_PA_OFFSET;
    mm::init_slab(slab_start, 4 * 1024 * 1024);  // 4MB for slab

    // ========== Heap initialized, format! can be used below ==========

    // Print boot message
    unsafe {
        use crate::console::putchar;
        const YELLOW: &[u8] = b"\x1b[33m";
        const RESET: &[u8] = b"\x1b[0m";
        for &b in YELLOW { putchar(b); }
        const MSG: &[u8] = b"Kernel starting...\n\n";
        for &b in MSG { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    // Print table header
    unsafe {
        use crate::console::putchar;
        const CYAN: &[u8] = b"\x1b[36m";
        const RESET: &[u8] = b"\x1b[0m";
        for &b in CYAN { putchar(b); }
        // Module(16) + 2 spaces + Description(32) + 3 spaces + Status
        const HEADER: &[u8] = b"Module            Description                        Status\n";
        for &b in HEADER { putchar(b); }
        const DIVIDER: &[u8] = b"----------------  --------------------------------   --------\n";
        for &b in DIVIDER { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    print_status("console", "UART ns16550a driver", true);

    // Initialize SMP multi-core support info
    {
        let cpu_count = arch::smp::num_started_cpus();
        if cpu_count > 1 {
            print_status("smp", &format!("{} CPU(s) online", cpu_count), true);
        }
    }

    print_status("trap", "stvec handler installed", true);
    print_status("trap", "ecall syscall handler", true);
    print_status("mm", "Sv39 3-level page table", true);
    print_status("mm", "satp CSR configured", true);
    print_status("mm", "buddy allocator order 0-12", true);

    // Display heap size using config value
    let heap_mb = crate::config::KERNEL_HEAP_SIZE / (1024 * 1024);
    let heap_info = format!("heap region {}MB @ {:#x}", heap_mb, KERNEL_HEAP_PHYS);
    print_status("mm", &heap_info, true);
    print_status("mm", "slab allocator 4MB", true);

    // Initialize command line argument parsing (needs to be after heap initialization)
    {
        #[cfg(feature = "riscv64")]
        {
            let dtb_ptr = arch::boot::get_dtb_pointer();
            cmdline::init(dtb_ptr);
            print_status("boot", "FDT/DTB parsed", true);
        }
        #[cfg(feature = "x86_64")]
        {
            cmdline::init_from(arch::boot::boot_cmdline());
            print_status("boot", "multiboot cmdline + e820 parsed", true);
        }
        if let Some(cmdline) = cmdline::get_cmdline() {
            if !cmdline.is_empty() {
                // Truncate long cmdline
                let display = if cmdline.len() > 22 {
                    let end = cmdline.floor_char_boundary(22);
                    format!("cmd: {}...", &cmdline[..end])
                } else {
                    format!("cmd: {}", cmdline)
                };
                print_status("boot", &display, true);
            }
        }
    }

    // Boot hart continues with remaining init (only hart reaches here)
    {
        // =====================================================================
        // paging_init - remaining phases
        // =====================================================================
        // Note: memblock_init, memory region parsing, memblock_reserve,
        // and setup_linear_mapping were already done above (before heap init).
        {
            // Re-read memory regions (riscv64 re-parses the FDT via the
            // linear mapping; the x86_64 multiboot copy in BSS is always
            // valid, so just rebuild the list)
            #[cfg(feature = "riscv64")]
            let memory_regions = {
                let dtb_phys = arch::boot::get_dtb_pointer();
                let dtb_virt = arch::mm::phys_to_virt(
                    arch::mm::PhysAddr::new(dtb_phys)
                ).bits();
                unsafe { cmdline::parse_memory_regions(dtb_virt) }
            };
            #[cfg(feature = "x86_64")]
            let memory_regions = x86_boot_memory_regions();

            // Calculate total physical memory
            let total_phys_memory: usize = memory_regions.iter().map(|r| r.size).sum();

            print_status("mm", &format!("linear mapping {} MB",
                total_phys_memory / (1024 * 1024)), true);

            // Initialize vmemmap mapping
            let start_pfn = MEMORY_PHYS_BASE / mm::PAGE_SIZE;
            let nr_pages = total_phys_memory / mm::PAGE_SIZE;

            if mm::vmemmap::init_vmemmap(start_pfn, nr_pages).is_ok() {
                print_status("mm", "vmemmap mapping initialized", true);
            } else {
                print_status("mm", "vmemmap mapping failed", false);
            }

            // Initialize kernel memory layout
            let heap_size = crate::config::KERNEL_HEAP_SIZE;
            let slab_start = KERNEL_HEAP_PHYS + heap_size;
            let slab_size = 4 * 1024 * 1024;
            let layout = mm::layout::KernelMemoryLayout::init_from_memblock(
                MEMORY_PHYS_BASE,
                total_phys_memory, // phys SIZE (was phys_base+size — review 4.20)
                KERNEL_PHYS_LOAD_ADDR,
                KERNEL_HEAP_PHYS,
            );
            mm::layout::kernel_layout_init(layout);
            // Boot-time cross-check: heap/slab ranges must be reserved in
            // memblock and inside physical memory (review 4.20).
            mm::layout::assert_memblock_consistency(&layout);
            print_status("mm", &format!("layout: kernel={:#x}-{:#x}",
                layout.kernel_start, layout.kernel_end), true);
            print_status("mm", &format!("layout: heap={:#x}-{:#x}",
                layout.heap_start, layout.heap_start + layout.heap_size), true);

            // Initialize page descriptors
            mm::page::init_page_descriptors(start_pfn, nr_pages);
            print_status("mm", &format!("{} page descriptors", nr_pages), true);

            // Initialize zone allocator
            let kernel_end = slab_start + slab_size;
            mm::init_zone_system(MEMORY_PHYS_BASE, total_phys_memory, kernel_end);
            print_status("mm", "zone allocator initialized", true);

            // Switch to late stage (use buddy allocator for page tables)
            arch::mm::pt_ops_set_late();

            // Print memblock summary
            let total_mb = mm::memblock_total_memory() / (1024 * 1024);
            let avail_mb = mm::memblock_available_memory() / (1024 * 1024);
            print_status("memblock", &format!("total {}MB, available {}MB", total_mb, avail_mb), true);
        }

        // Setup device mappings (PLIC, VirtIO, CLINT, etc.)
        arch::mm::setup_device_mappings();
        print_status("mm", "device mappings created", true);

        // Arm the wall clock from the goldfish RTC (QEMU virt's default
        // RTC @ 0x101000): one boot-time read derives the REALTIME epoch
        // offset used by clock_gettime/gettimeofday, the vDSO fast path
        // and filesystem timestamps. Must precede vdso_init() (which
        // snapshots the offset) and any file creation; if the device is
        // unreadable the offset stays 0 and the system runs on monotonic
        // boot time (the pre-RTC behaviour).
        drivers::rtc::rtc_init_wall_clock();

        // Initialize IRQ framework
        {
            interrupt::init();
            print_status("irq", "irq_desc array initialized", true);
        }

        // Initialize interrupt controller (PLIC on riscv64; the 8259 PIC
        // on x86_64 is programmed by the arch trap bring-up)
        {
            drivers::intc::init();
            #[cfg(feature = "riscv64")]
            print_status("intc", "PLIC @ 0x0C000000", true);
            #[cfg(feature = "riscv64")]
            print_status("intc", "IRQ domain + chip registered", true);
        }

        // Initialize IPI (inter-processor interrupt)
        {
            arch::ipi::init();
            #[cfg(feature = "riscv64")]
            print_status("ipi", "SSIP software IRQ + bitmap multiplexing", true);
        }

        // Initialize UART interrupt-driven RX (after PLIC)
        {
            console::init_irq();
            print_status("console", "UART interrupt-driven RX", true);
        }

        // Initialize file system
        {
            // Initialize block I/O layer
            fs::bio::init();
            print_status("bio", "buffer cache layer", true);

            // Initialize ext4 file system
            fs::ext4::init();
            print_status("fs", "ext4 driver loaded", true);

            // Initialize RootFS
            let rootfs_result = fs::rootfs::init_rootfs();
            print_status("fs", "ramfs mounted /", rootfs_result.is_ok());
            if rootfs_result.is_ok() {
                // Build dentry tree for rootfs
                fs::vfs::vfs_mount("/", fs::rootfs::create_root_inode(),
                    fs::mount::MntFlags::new(0));
            }

            // Initialize ProcFS and mount to /proc (if configured to enable)
            if crate::config::AUTO_MOUNT_PROCFS {
                let procfs_result = fs::procfs::init_procfs();
                print_status("fs", "procfs initialized", procfs_result.is_ok());
                if procfs_result.is_ok() {
                    let mount_result = fs::procfs::mount_procfs();
                    print_status("fs", "procfs mounted /proc", mount_result.is_ok());
                    if mount_result.is_ok() {
                        if let Some(sb) = fs::procfs::get_procfs_sb() {
                            // Build dentry tree for procfs
                            fs::vfs::vfs_mount("/proc", fs::procfs::create_root_inode(),
                                fs::mount::MntFlags::new(0));
                        }
                    }
                }
            }

            // Auto-mount sysfs at /sys (same rationale as procfs above):
            // stock userland — Xorg's fbdevhw fbdev_open() readlinks
            // /sys/class/graphics/fb0 and silently refuses /dev/fb0 when
            // the lookup fails; udev's DEVPATH walks need it too. runit
            // and other minimal inits never mount it themselves.
            {
                let sysfs_init = fs::sysfs::init_sysfs();
                let sysfs_mount = sysfs_init.and_then(|_| fs::sysfs::mount_sysfs());
                let ok = sysfs_mount
                    .map(|_| {
                        fs::vfs::vfs_mount(
                            "/sys",
                            fs::sysfs::create_root_inode(),
                            fs::mount::MntFlags::new(0),
                        );
                        fs::mount::register_mount("sysfs", "/sys", "sysfs", "rw");
                    })
                    .is_ok();
                print_status("fs", "sysfs mounted /sys", ok);
            }

            // cgroup v2 unified hierarchy (U1b) — systemd's hard
            // dependency: mount cgroup2 at /sys/fs/cgroup after the
            // sysfs/procfS boot mounts. The hierarchy root exposes
            // cgroup.controllers ("cpu memory pids") and a writable
            // cgroup.procs, which systemd's boot probe requires.
            {
                let cg_init = fs::cgroup::init_cgroupfs();
                print_status("cgroup", "v2 unified hierarchy init", cg_init.is_ok());
                if cg_init.is_ok() {
                    let cg_mount = fs::cgroup::mount_cgroupfs("/sys/fs/cgroup");
                    print_status("cgroup", "cgroup2 mounted /sys/fs/cgroup", cg_mount.is_ok());
                }
            }
        }

        // Initialize block devices (for rootfs)
        {
            // First scan MMIO devices (virtio-blk-device)
            let mmio_count = drivers::probe::init_block_devices();
            if mmio_count > 0 {
                print_status("driver", &format!("virtio-blk MMIO x{}", mmio_count), true);
            }
            // Then scan PCI devices (virtio-blk-pci)
            let pci_count = drivers::probe::init_pci_block_devices();
            if pci_count > 0 {
                print_status("driver", &format!("virtio-blk PCI x{}", pci_count), true);
                print_status("driver", "GenDisk registered", true);
            }

            // Auto-mount ext4 file system (if configured to enable)
            if crate::config::AUTO_MOUNT_EXT4 {
                // Try mounting from PCI device
                if let Some(disk) = drivers::virtio::get_pci_gen_disk() {
                    let mount_result = fs::ext4::mount_ext4(disk as *const _);
                    let mount_point = crate::config::EXT4_MOUNT_POINT;
                    print_status("fs", &format!("ext4 mounted {}", mount_point), mount_result.is_ok());
                    if mount_result.is_ok() {
                        if let Some(ext4_fs) = fs::ext4::get_ext4_fs() {
                            // Build dentry tree for ext4 (overlays root)
                            fs::vfs::vfs_mount("/", fs::ext4::create_root_inode(),
                                fs::mount::MntFlags::new(0));
                        }
                    }

                    // Remount procfs after ext4 mount (since ext4 overwrites root directory)
                    if mount_result.is_ok() && crate::config::AUTO_MOUNT_PROCFS {
                        let procfs_mount_result = fs::procfs::mount_procfs();
                        print_status("fs", "procfs remounted /proc", procfs_mount_result.is_ok());
                        if procfs_mount_result.is_ok() {
                            // Rebuild dentry tree for procfs after ext4 overlay
                            fs::vfs::vfs_mount("/proc", fs::procfs::create_root_inode(),
                                fs::mount::MntFlags::new(0));
                        }
                    }

                    // Re-link cgroup2 after ext4 overlay (U1b, same
                    // defensive re-mount as procfs above).
                    if mount_result.is_ok() {
                        let _ = fs::cgroup::mount_cgroupfs("/sys/fs/cgroup");
                    }
                } else if let Some(virtio_dev) = drivers::virtio::get_device() {
                    // Try mounting from MMIO device
                    let disk_ptr = &virtio_dev.disk as *const drivers::blkdev::GenDisk;
                    let mount_result = fs::ext4::mount_ext4(disk_ptr);
                    let mount_point = crate::config::EXT4_MOUNT_POINT;
                    print_status("fs", &format!("ext4 mounted {}", mount_point), mount_result.is_ok());
                    if mount_result.is_ok() {
                        if let Some(ext4_fs) = fs::ext4::get_ext4_fs() {
                            // Build dentry tree for ext4 (overlays root)
                            fs::vfs::vfs_mount("/", fs::ext4::create_root_inode(),
                                fs::mount::MntFlags::new(0));
                        }
                    }

                    // Remount procfs after ext4 mount
                    if mount_result.is_ok() && crate::config::AUTO_MOUNT_PROCFS {
                        let procfs_mount_result = fs::procfs::mount_procfs();
                        print_status("fs", "procfs remounted /proc", procfs_mount_result.is_ok());
                        if procfs_mount_result.is_ok() {
                            // Rebuild dentry tree for procfs after ext4 overlay
                            fs::vfs::vfs_mount("/proc", fs::procfs::create_root_inode(),
                                fs::mount::MntFlags::new(0));
                        }
                    }

                    // Re-link cgroup2 after ext4 overlay (U1b).
                    if mount_result.is_ok() {
                        let _ = fs::cgroup::mount_cgroupfs("/sys/fs/cgroup");
                    }
                }
            }
        }

        // Initialize swap on the root block device (tail carve).
        //
        // Boot-order constraints (P1 swap wiring, 2026-09):
        //  - AFTER bio::init() and the block-device probes above (the
        //    activation path reads the swap header and ext4 superblock
        //    via blkdev_read — the same sync-poll I/O the ext4 mount
        //    above already uses, proven safe with SEIE still off);
        //  - AFTER the zone allocator (mm::init_zone_system, earlier):
        //    swap only matters once pages can be allocated/reclaimed;
        //  - BEFORE kswapd::init() below, so no reclaim path can observe
        //    the subsystem half-initialized, and before init (PID 1).
        // Note: the plan originally said "before rootfs mount", but block
        // devices only appear after the rootfs/ext4 mount in this boot
        // sequence, so this is the earliest safe point.
        // swap_init refuses to enable when the tail carve would overlap
        // the ext4 filesystem (see the ext4 guard in mm/swap.rs).
        {
            mm::swap::swap_init();
            print_status_ex(
                "mm",
                &format!("swap {} MB tail carve", crate::config::SWAP_SIZE_MB),
                if mm::swap::nr_active_swap() { Some(true) } else { None },
            );
        }

        // Initialize persistent kernel log (write kmsg to /var/log/kmsg on disk)
        // Disabled: ext4 write operations corrupt filesystem
        // printk::persistent_log_init();

        // Initialize network devices
        {
            let device_count = drivers::probe::init_network_devices();
            if device_count > 0 {
                print_status("driver", &format!("virtio-net x{}", device_count), true);
            }
            // TCP manager, timer and routing table must be live before any
            // packet can arrive: RX/softirq paths dereference them
            // unconditionally, and the timer softirq ticks from the first
            // clock interrupt (review NET NEW-C4 — init was never called).
            crate::net::tcp::init_tcp_manager();
            crate::net::tcp_timer::init_tcp_timer_manager();
            crate::net::ipv4::route::route_init();
            // P2 vDSO: build the vDSO ELF page and prime the time-data
            // page — must run before the first execve maps it.
            crate::mm::vdso::vdso_init();
            print_status("vdso", "clock_gettime fast path", true);
            // P2 boot ip=a.b.c.d: override the hardcoded slirp default
            // (10.0.2.15) with the address given on the kernel command
            // line — replaces hardcoding for non-default QEMU networks.
            if let Some(ip_str) = cmdline::get_param("ip") {
                if let Some(ip) = cmdline::parse_ipv4_addr(&ip_str) {
                    crate::net::arp::set_local_ip(ip);
                    let a = (ip >> 24) & 0xFF;
                    let b = (ip >> 16) & 0xFF;
                    let c = (ip >> 8) & 0xFF;
                    let d = ip & 0xFF;
                    print_status(
                        "net",
                        &format!("boot ip={}.{}.{}.{}", a, b, c, d),
                        true,
                    );
                } else {
                    print_status("net", "bad ip= ignored (keep 10.0.2.15)", false);
                }
            }
            // P1 IPv6: SLAAC link-local (fe80::/64 + EUI-64) + Router
            // Solicitation. No NIC is fine — the stack simply stays v4-only.
            crate::net::ipv6::ipv6_init();
        }

        // Initialize security subsystem (before scheduler / process creation)
        security::security_init();

        // Initialize process scheduler
        {
            crate::process::pid_hash::init();
            sched::init();
            print_status("sched", "CFS scheduler v1", true);
            print_status("sched", "runqueue per-CPU", true);
            print_status("sched", "PID allocator init", true);
            print_status("sched", "idle task (PID 0)", true);

            // (PCP per-CPU pages init removed together with mm/pcp.rs —
            // the module had zero live consumers; see mm/mod.rs note.)
        }

        // Initialize kswapd background reclaim thread
        {
            mm::kswapd::init();
            print_status("mm", "kswapd reclaim thread", true);
        }

        // Initialize DFX subsystem (must be after sched::init)
        {
            dfx::init();
            print_status("dfx", "diagnostic subsystem", true);
        }

        // Initialize IPC subsystem
        {
            ipc::init();
            print_status("ipc", "System V + POSIX MQ", true);
        }

        // Start secondary CPUs via SBI HSM (must be after scheduler init).
        // Secondary CPUs call init_secondary() which does kmalloc (pid_hash_insert),
        // but they do NOT enable timer interrupts until cpu_idle_loop(),
        // so they won't participate in scheduling until boot CPU is ready.
        // (start_secondaries emits its own structured status row.)
        arch::smp::start_secondaries();

        // Initialize ksoftirqd per-CPU threads (must be after start_secondaries
        // because wake_up sends IPIs to target CPUs, which must be online).
        {
            interrupt::ksoftirqd::init();
            print_status("softirq", "ksoftirqd per-CPU threads", true);
        }

        // Start the hung-task detector kthread here (late boot — after
        // secondaries are online, same context as ksoftirqd; the early
        // dfx::init() slot runs BEFORE start_secondaries so its wake_up
        // IPIs could target offline CPUs). Its sleep loop is timer-driven.
        {
            crate::dfx::hung_task::init();
            print_status("dfx", "khungtaskd hung-task detector", true);
        }

        // Enable external interrupts
        {
            arch::trap::enable_external_interrupt();
            print_status("trap", "sie.SEIE enabled", true);
        }

        // ========== Graphics System Initialization (VirtIO-GPU) ==========
        #[cfg(feature = "riscv64")]
        {
            // Probe VirtIO-GPU device
            if let Some(mut gpu_device) = drivers::gpu::probe_virtio_gpu() {
                print_status("driver", "virtio-gpu probed", true);
                // Initialize framebuffer
                if let Some(fb_info) = gpu_device.init_framebuffer() {
                    print_status("gpu", &format!("{}x{} 32bpp framebuffer", fb_info.width, fb_info.height), true);
                    // Save framebuffer info for userspace mmap
                    drivers::gpu::set_framebuffer_info(*fb_info);
                    // Save GPU device for refresh
                    drivers::gpu::set_gpu_device(gpu_device);
                } else {
                    print_status("gpu", "framebuffer init failed", false);
                }
            } else {
                print_status("driver", "virtio-gpu not found", false);
            }
        }

        // ========== Initialize Input System ==========
        {
            // Initialize PS/2 driver (does nothing on RISC-V)
            drivers::input::init();

            // Initialize devfs (must be before evdev initialization)
            fs::devfs::init();
            printk::init_kmsg_device();
            print_status("fs", "devfs mounted /dev", true);
            // /dev/fb0 — framebuffer char device (registered when the GPU
            // initialized successfully above)
            #[cfg(feature = "riscv64")]
            if drivers::gpu::get_framebuffer_info().is_some() {
                match drivers::gpu::fbdev::init_fbdev() {
                    Ok(()) => print_status("driver", "/dev/fb0 registered", true),
                    Err(()) => print_status("driver", "/dev/fb0 registration failed", false),
                }
            }

            // Build dentry tree for devfs
            if let Some(root_entry) = fs::devfs::get_root_entry() {
                fs::vfs::vfs_mount("/dev", fs::devfs::create_root_inode(&root_entry),
                    fs::mount::MntFlags::new(0));
            }

            // Mount tmpfs at /dev/shm (P0-6): backing store for POSIX
            // shm_open — musl's libc maps shm_open("/name") onto
            // /dev/shm/name, so a working tmpfs there is all the kernel
            // needs to provide. Independent instance per do_mount.
            {
                let shm_result = fs::mount::do_mount("none", "/dev/shm", "tmpfs", 0);
                print_status("fs", "tmpfs mounted /dev/shm", shm_result.is_ok());
            }

            // Mount tmpfs at /run (U2): systemd's runtime-state
            // directory (/run/systemd, /run/udev, pidfiles). systemd
            // requires a writable /run very early — without it the boot
            // degrades to emergency mode. Mirrors the /dev/shm approach
            // (independent tmpfs instance via do_mount).
            {
                let run_result = fs::mount::do_mount("none", "/run", "tmpfs", 0);
                print_status("fs", "tmpfs mounted /run", run_result.is_ok());
            }

            // Initialize VirtIO Input devices
            let (kb_count, ptr_count) = drivers::input::init_virtio_input();

            // evdev devices registered
            print_status("driver", "evdev /dev/input/event0", true);
            print_status("driver", "evdev /dev/input/event1", true);

            if kb_count > 0 {
                print_status("driver", "virtio-keyboard", true);
            } else {
                print_status("driver", "PS/2 keyboard (stub)", true);
            }

            if ptr_count > 0 {
                print_status("driver", "virtio-tablet", true);
            } else {
                print_status("driver", "PS/2 mouse (stub)", true);
            }
        }

        // Run all unit tests (disable interrupts to avoid interference)
        #[cfg(feature = "unit-test")]
        {
            arch::trap::disable_timer_interrupt();
            tests::run_all_tests();
            // Panic after tests complete, don't load init
            let failed = tests::get_failed_count();
            if failed > 0 {
                panic!("{} test(s) failed!", failed);
            } else {
                // All tests passed, normal exit
                println!("\nAll tests passed! Halting...");
                loop {
                    crate::arch::cpu::wfi();
                }
            }
        }

        // Test user program execution
        {
            // Disable timer interrupt to avoid interfering with user program loading
            arch::trap::disable_timer_interrupt();
            // Timer interrupt will be enabled after init starts
        }

        // ========== Start init process ==========
        {
            // Get init path
            let init_path = cmdline::get_init_program();
            print_status("init", &format!("loading {}", init_path), true);
            init::init();
            print_status("init", "ELF loaded to user space", true);
            print_status("init", "init task (PID 1) enqueued", true);

            // Print shell welcome message after boot
            unsafe {
                use crate::console::putchar;
                let msg = b"\n\x1b[1;36mWelcome to \x1b[1;32mRux OS\x1b[0m \x1b[90m(RISC-V 64)\x1b[0m\n\x1b[90m- \x1b[1mmrsh\x1b[0m\x1b[90m (POSIX shell) | A minimal POSIX-compatible shell\x1b[0m\n";
                for &b in msg {
                    putchar(b);
                }
            }
        }

        println!();

        // Enable OOM killer now that boot is complete
        mm::kswapd::enable_oom();

        // ========== Timer interrupt setup ==========
        // Enable timer interrupts (also sets the first trigger internally)
        arch::trap::enable_timer_interrupt();

        // Signal secondary CPUs that they may now enable their timer interrupts.
        // This must happen AFTER boot CPU has finished all initialization
        // to prevent secondary timer interrupts from interfering with boot.
        // (riscv64 SBI HSM broadcast; x86_64 SMP bring-up is single-CPU.)
        #[cfg(feature = "riscv64")]
        arch::smp::signal_boot_complete();

        // ========== Enter scheduler main loop ==========
        // Note: don't use println! here — it might deadlock if printk lock is held

        // Debug messages go to ring buffer only; use `dmesg -n 7` to show on console.
        // Console loglevel is DEFAULT_CONSOLE_LOGLEVEL (KERN_INFO = 6).

        // Boot hart enters idle loop, participates in task scheduling
        sched::cpu_idle_loop();
    }
}

// Panic handler — uses dfx::backtrace for all output
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use core::fmt::Write;
    let mut w = dfx::backtrace::ConsoleWriter::new();

    // Header
    let _ = w.write_str("\n\nKernel panic - not syncing:\n\n");
    let _ = write!(w, "PANIC: {}\n", info.message());
    if let Some(loc) = info.location() {
        let _ = write!(w, "  Location: {}: {}\n", loc.file(), loc.line());
    }

    // Separator
    let _ = w.write_str("\n---[ end Kernel panic - not syncing ]---\n\n");

    // Dump CSRs, registers, and stack trace via dfx
    dfx::backtrace::dump_csrs();
    dfx::backtrace::dump_regs_inline();
    dfx::backtrace::dump_stack();

    // Stop the other CPUs (Linux panic → smp_send_stop). Without this the
    // panicking CPU halts while the others keep running — corrupting state
    // under the panic dump and stealing the UART mid-print (review批次8).
    #[cfg(feature = "riscv64")]
    {
        let me = arch::cpu_id() as usize;
        for cpu in 0..crate::config::MAX_CPUS {
            if cpu != me {
                arch::ipi::send_ipi_type(cpu, arch::ipi::IpiType::Stop);
            }
        }
    }

    // Flush persistent log
    crate::printk::persistent_log_flush();

    // Halt
    loop {
        crate::arch::cpu::wfi();
    }
}
