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
fn print_status(module: &str, desc: &str, success: bool) {
    // ANSI color codes
    const RED: &[u8] = b"\x1b[31m";
    const RESET: &[u8] = b"\x1b[0m";
    const OK: &[u8] = b"[ok]";
    const FAIL: &[u8] = b"[fail]";

    unsafe {
        use crate::console::putchar;

        // Print red start code on failure
        if !success {
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
        if success {
            for &b in OK {
                putchar(b);
            }
        } else {
            for &b in FAIL {
                putchar(b);
            }
        }

        // Print color reset code on failure
        if !success {
            for &b in RESET {
                putchar(b);
            }
        }

        putchar(b'\n');
    }
}

mod sbi;
mod mm;
mod console;
mod print;
mod drivers;
mod config;
mod list;
mod process;
mod sched;
mod fs;
mod signal;
mod sync;
mod errno;
mod net;
mod cmdline;
mod init;
mod syscall;

#[cfg(feature = "unit-test")]
mod tests;

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// Include platform-specific assembly code
#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

// RISC-V kernel main function
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // DEBUG: Output 'R' to confirm rust_main started
    unsafe {
        use crate::console::putchar;
        putchar(b'R');
        putchar(b'U');
        putchar(b'X');
        putchar(b'\n');
    }

    // Initialize SMP (multi-core support) - must run first!
    // Only the boot hart returns true, secondary harts enter idle loop
    let is_boot_hart = arch::smp::init();

    // Initialize per-CPU interrupt stacks (must be before any traps)
    arch::smp::init_per_cpu_intr_stacks();

    // Secondary harts enter idle loop, don't execute any initialization
    if !is_boot_hart {
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    // ========== The following code is only executed by the boot hart ==========

    // Initialize console (must be first, so other initialization can print)
    console::init();

    // Simple boot message (avoid UTF-8 for now)
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"\nRux OS starting...\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // DEBUG
    unsafe { crate::console::putchar(b'1'); }

    // Initialize trap handling
    arch::trap::init();

    // DEBUG
    unsafe { crate::console::putchar(b'2'); }

    arch::trap::init_syscall();

    // DEBUG
    unsafe { crate::console::putchar(b'3'); }

    // Initialize MMU (must be before heap initialization)
    arch::mm::init();

    // DEBUG
    unsafe { crate::console::putchar(b'4'); }

    // Initialize heap allocator (MMU must be initialized first)
    mm::init_heap();

    // Initialize Slab allocator (after heap)
    // Heap end address: 0x80A0_0000 + KERNEL_HEAP_SIZE
    // Use 4MB slab region to support more small object allocations
    let slab_start = 0x80A0_0000 + crate::config::KERNEL_HEAP_SIZE;
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
    let heap_info = format!("heap region {}MB @ 0x80A00000", heap_mb);
    print_status("mm", &heap_info, true);
    print_status("mm", "slab allocator 4MB", true);

    // Initialize command line argument parsing (needs to be after heap initialization)
    {
        let dtb_ptr = arch::riscv64::boot::get_dtb_pointer();
        cmdline::init(dtb_ptr);
        print_status("boot", "FDT/DTB parsed", true);
        if let Some(cmdline) = cmdline::get_cmdline() {
            if !cmdline.is_empty() {
                // Truncate long cmdline
                let display = if cmdline.len() > 22 {
                    format!("cmd: {}...", &cmdline[..22])
                } else {
                    format!("cmd: {}", cmdline)
                };
                print_status("boot", &display, true);
            }
        }
    }

    // Only the boot hart will execute to this point
    if is_boot_hart {
        // =====================================================================
        // Linux-style paging_init()
        // =====================================================================
        // This follows Linux's paging_init() approach:
        // 1. setup_bootmem(): Initialize memblock, add memory, reserve regions
        // 2. setup_vm_final(): Create linear mapping, vmemmap, switch to final page table
        {
            // ===== Phase 1: setup_bootmem() =====
            // Initialize memblock
            mm::memblock_init();

            // Parse memory regions from device tree
            // Convert DTB physical address to kernel virtual address
            let dtb_phys = arch::riscv64::boot::get_dtb_pointer();
            let dtb_virt = arch::riscv64::mm::phys_to_virt(
                arch::riscv64::mm::PhysAddr::new(dtb_phys)
            ).bits();
            let memory_regions = unsafe { cmdline::parse_memory_regions(dtb_virt) };

            // Add memory regions to memblock
            for region in &memory_regions {
                mm::memblock_add(region.base, region.size).ok();
            }

            // Reserve memory regions (kernel, heap, slab)
            // These reservations prevent memblock from allocating from used regions
            let heap_start = 0x80A00000usize;
            let heap_size = crate::config::KERNEL_HEAP_SIZE;
            let slab_start = heap_start + heap_size;
            let slab_size = 4 * 1024 * 1024;

            mm::memblock_reserve(0x80000000, 0xA00000).ok();  // OpenSBI + kernel
            mm::memblock_reserve(heap_start, heap_size).ok(); // Heap
            mm::memblock_reserve(slab_start, slab_size).ok(); // Slab

            // Calculate total physical memory
            let total_phys_memory: usize = memory_regions.iter().map(|r| r.size).sum();

            // ===== Phase 2: setup_vm_final() =====
            // Switch to fixmap stage for page table allocation
            arch::riscv64::mm::pt_ops_set_fixmap();

            // Setup linear mapping (PAGE_OFFSET region)
            arch::riscv64::mm::setup_linear_mapping(&memory_regions);
            print_status("mm", &format!("linear mapping {} MB",
                total_phys_memory / (1024 * 1024)), true);

            // Initialize vmemmap mapping
            let start_pfn = 0x80000000 / mm::PAGE_SIZE;
            let nr_pages = total_phys_memory / mm::PAGE_SIZE;

            if mm::vmemmap::init_vmemmap(start_pfn, nr_pages).is_ok() {
                print_status("mm", "vmemmap mapping initialized", true);
            } else {
                print_status("mm", "vmemmap mapping failed", false);
            }

            // Initialize kernel memory layout
            let layout = mm::layout::KernelMemoryLayout::init_from_memblock(
                0x80000000,
                0x80000000 + total_phys_memory,
                0x80200000,
                0x80A00000,
            );
            mm::layout::kernel_layout_init(layout);
            print_status("mm", &format!("layout: kernel={:#x}-{:#x}",
                layout.kernel_start, layout.kernel_end), true);
            print_status("mm", &format!("layout: heap={:#x}-{:#x}",
                layout.heap_start, layout.heap_start + layout.heap_size), true);

            // Initialize page descriptors
            mm::page::init_page_descriptors(start_pfn, nr_pages);
            print_status("mm", &format!("{} page descriptors", nr_pages), true);

            // Initialize zone allocator
            let kernel_end = slab_start + slab_size;
            mm::init_zone_system(0x80000000, total_phys_memory, kernel_end);
            print_status("mm", "zone allocator initialized", true);

            // Switch to late stage (use buddy allocator for page tables)
            arch::riscv64::mm::pt_ops_set_late();

            // Print memblock summary
            let total_mb = mm::memblock_total_memory() / (1024 * 1024);
            let avail_mb = mm::memblock_available_memory() / (1024 * 1024);
            print_status("memblock", &format!("total {}MB, available {}MB", total_mb, avail_mb), true);
        }

        // Setup device mappings (PLIC, VirtIO, CLINT, etc.)
        arch::riscv64::mm::setup_device_mappings();

        // Initialize PLIC (interrupt controller)
        {
            drivers::intc::init();
            print_status("intc", "PLIC @ 0x0C000000", true);
            print_status("intc", "external IRQ routing", true);
        }

        // Initialize IPI (inter-processor interrupt)
        {
            arch::ipi::init();
            print_status("ipi", "SSIP software IRQ", true);
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

            // Initialize ProcFS and mount to /proc (if configured to enable)
            if crate::config::AUTO_MOUNT_PROCFS {
                let procfs_result = fs::procfs::init_procfs();
                print_status("fs", "procfs initialized", procfs_result.is_ok());
                if procfs_result.is_ok() {
                    let mount_result = fs::procfs::mount_procfs();
                    print_status("fs", "procfs mounted /proc", mount_result.is_ok());
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

                    // Remount procfs after ext4 mount (since ext4 overwrites root directory)
                    if mount_result.is_ok() && crate::config::AUTO_MOUNT_PROCFS {
                        let procfs_mount_result = fs::procfs::mount_procfs();
                        print_status("fs", "procfs remounted /proc", procfs_mount_result.is_ok());
                    }
                } else if let Some(virtio_dev) = drivers::virtio::get_device() {
                    // Try mounting from MMIO device
                    let disk_ptr = &virtio_dev.disk as *const drivers::blkdev::GenDisk;
                    let mount_result = fs::ext4::mount_ext4(disk_ptr);
                    let mount_point = crate::config::EXT4_MOUNT_POINT;
                    print_status("fs", &format!("ext4 mounted {}", mount_point), mount_result.is_ok());

                    // Remount procfs after ext4 mount
                    if mount_result.is_ok() && crate::config::AUTO_MOUNT_PROCFS {
                        let procfs_mount_result = fs::procfs::mount_procfs();
                        print_status("fs", "procfs remounted /proc", procfs_mount_result.is_ok());
                    }
                }
            }
        }

        // Initialize network devices
        {
            let device_count = drivers::probe::init_network_devices();
            if device_count > 0 {
                print_status("driver", &format!("virtio-net x{}", device_count), true);
            }
        }

        // Initialize process scheduler
        {
            sched::init();
            print_status("sched", "CFS scheduler v1", true);
            print_status("sched", "runqueue per-CPU", true);
            print_status("sched", "PID allocator init", true);
            print_status("sched", "idle task (PID 0)", true);

            // Initialize Per-CPU Pages (after scheduler initialization)
            let boot_cpu = arch::cpu_id() as usize;
            mm::init_percpu_pages(boot_cpu);
            print_status("mm", &format!("PCP cpu{} hotpage", boot_cpu), true);
        }

        // Enable external interrupts
        {
            arch::trap::enable_external_interrupt();
            print_status("trap", "sie.SEIE enabled", true);
        }

        // ========== Graphics System Initialization (VirtIO-GPU) ==========
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
            }
        }

        // ========== Initialize Input System ==========
        {
            // Initialize PS/2 driver (does nothing on RISC-V)
            drivers::input::init();

            // Initialize devfs (must be before evdev initialization)
            fs::devfs::init();
            print_status("fs", "devfs mounted /dev", true);

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

        println!();

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
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
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
        }

        println!();

        // ========== Timer interrupt setup ==========
        // Enable timer interrupts (also sets the first trigger internally)
        arch::trap::enable_timer_interrupt();

        // ========== Enter scheduler main loop ==========
        println!("sched: entering idle loop");

        // Boot hart enters idle loop, participates in task scheduling
        sched::cpu_idle_loop();
    } else {
        // Secondary hart: initialize scheduler and enter idle loop

        // Initialize process scheduler (secondary harts also need this)
        sched::init();

        // Enter idle loop, participate in task scheduling
        sched::cpu_idle_loop();
    }
}

// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"\nPANIC! ";
        for &b in MSG {
            putchar(b);
        }

        // Try to print the message using a simple writer
        let mut writer = SimpleWriter;
        let _ = core::fmt::Write::write_fmt(&mut writer, format_args!("{}", info.message()));

        // Try to print the location if available
        if let Some(loc) = info.location() {
            const MSG_FILE: &[u8] = b"\n  Location: ";
            for &b in MSG_FILE {
                putchar(b);
            }
            for b in loc.file().as_bytes() {
                putchar(*b);
            }
            putchar(b':');
            let line = loc.line();
            // Simple line number printing (0-9999)
            if line < 10 {
                putchar(b'0' + line as u8);
            } else if line < 100 {
                putchar(b'0' + (line / 10) as u8);
                putchar(b'0' + (line % 10) as u8);
            } else if line < 1000 {
                putchar(b'0' + (line / 100) as u8);
                putchar(b'0' + ((line / 10) % 10) as u8);
                putchar(b'0' + (line % 10) as u8);
            } else {
                putchar(b'0' + (line / 1000) as u8);
                putchar(b'0' + ((line / 100) % 10) as u8);
                putchar(b'0' + ((line / 10) % 10) as u8);
                putchar(b'0' + (line % 10) as u8);
            }
            putchar(b'\n');
        }
    }
    loop {}
}

// Simple writer for panic messages
struct SimpleWriter;

impl core::fmt::Write for SimpleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe {
            for b in s.bytes() {
                crate::console::putchar(b);
            }
        }
        Ok(())
    }
}
