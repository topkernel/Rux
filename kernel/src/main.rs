#![no_std]
#![no_main]
#![feature(lang_items, global_asm, naked_functions, alloc_error_handler, linkage)]

#[macro_use]
extern crate log;
extern crate alloc;

use core::panic::PanicInfo;
use core::arch::asm;

mod arch;
mod mm;
mod console;
mod print;
mod drivers;
mod config;
mod process;
mod fs;
mod signal;
mod collection;

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// 包含平台特定的汇编代码
#[cfg(feature = "aarch64")]
use core::arch::global_asm;

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 禁用中断直到中断控制器设置完成
    // DAIF: bit 2=I(IRQ), bit 3=F(FIQ)
    // DAIFSET #0xC = 设置 bits 2 和 3，禁用 IRQ 和 FIQ
    unsafe {
        asm!("msr daifset, #0xC", options(nomem, nostack));
    }

    // 初始化控制台（UART）
    console::init();

    println!("{} Kernel v{} starting...",
             crate::config::KERNEL_NAME,
             crate::config::KERNEL_VERSION);
    println!("Target platform: {}", crate::config::TARGET_PLATFORM);

    debug_println!("Initializing architecture...");
    arch::arch_init();

    debug_println!("Before trap init");
    debug_println!("Initializing trap handling...");
    arch::trap::init();
    debug_println!("After trap init");

    debug_println!("Initializing system calls...");
    arch::trap::init_syscall();

    debug_println!("Initializing heap...");
    crate::mm::init_heap();

    // 首先测试直接调用分配器
    debug_println!("Testing direct allocator call...");
    use core::alloc::{GlobalAlloc, Layout};
    unsafe {
        let layout = Layout::new::<u32>();
        let ptr = GlobalAlloc::alloc(&crate::mm::allocator::HEAP_ALLOCATOR, layout);
        if !ptr.is_null() {
            *(ptr as *mut u32) = 42;
            debug_println!("Direct alloc works!");
        } else {
            debug_println!("Direct alloc failed!");
        }
    }

    debug_println!("Testing SimpleVec...");
    use crate::collection::SimpleVec;
    match SimpleVec::with_capacity(10) {
        Some(mut test_vec) => {
            if test_vec.push(42) {
                debug_println!("SimpleVec::push works!");
                if let Some(val) = test_vec.get(0) {
                    // 使用多个 debug_println 调用
                    debug_println!("SimpleVec::get works, value = ");
                    unsafe {
                        use crate::console::putchar;
                        const DIGITS: &[u8] = b"0123456789";
                        let mut n = *val;
                        let mut buf = [0u8; 20];
                        let mut i = 19;
                        if n == 0 {
                            buf[i] = b'0';
                            i -= 1;
                        } else {
                            while n > 0 {
                                buf[i] = DIGITS[(n % 10) as usize];
                                n /= 10;
                                if i > 0 { i -= 1; }
                            }
                        }
                        for &b in &buf[i..] {
                            putchar(b);
                        }
                        const NEWLINE: &[u8] = b"\n";
                        for &b in NEWLINE {
                            putchar(b);
                        }
                    }
                } else {
                    debug_println!("SimpleVec::get failed!");
                }
            } else {
                debug_println!("SimpleVec::push failed!");
            }
        }
        None => {
            debug_println!("SimpleVec::with_capacity failed!");
        }
    }

    // 测试 SimpleBox
    debug_println!("Testing SimpleBox...");
    use crate::collection::SimpleBox;
    match SimpleBox::new(42) {
        Some(_box_val) => {
            debug_println!("SimpleBox works!");
        }
        None => {
            debug_println!("SimpleBox::new failed!");
        }
    }

    // 测试 SimpleString
    debug_println!("Testing SimpleString...");
    use crate::collection::SimpleString;
    match SimpleString::from_str("Hello Rux") {
        Some(_s) => {
            debug_println!("SimpleString works!");
        }
        None => {
            debug_println!("SimpleString::from_str failed!");
        }
    }

    // 测试 SimpleArc
    debug_println!("Testing SimpleArc...");
    use crate::collection::SimpleArc;
    match SimpleArc::new(12345) {
        Some(arc) => {
            // 测试克隆
            let _arc2 = arc.clone();
            debug_println!("SimpleArc works!");
        }
        None => {
            debug_println!("SimpleArc::new failed!");
        }
    }

    // GIC 初始化
    debug_println!("Initializing GIC...");
    drivers::intc::init();
    // 注意：IRQ 仍然禁用，将在稍后启用
    debug_println!("GIC initialized - IRQ still disabled");

    // 立即检查 GIC init 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_GIC: &[u8] = b"DEBUG: PMR immediately after GIC init = 0x";
        for &b in MSG_PMR_GIC {
            putchar(b);
        }
        let pmr_gic: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_gic,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_gic >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_gic & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // Timer 初始化（在 GIC 之后，IRQ 使能之前）
    debug_println!("Initializing timer...");
    drivers::timer::init();

    // 在 GIC 中使能物理定时器中断（IRQ 30）
    debug_println!("Enabling timer IRQ in GIC...");
    drivers::intc::enable_irq(30);  // ARMv8 物理定时器 IRQ

    // 检查 Timer init 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_TIMER: &[u8] = b"DEBUG: PMR after timer init = 0x";
        for &b in MSG_PMR_TIMER {
            putchar(b);
        }
        let pmr_timer: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_timer,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_timer >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_timer & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    debug_println!("Initializing scheduler...");
    process::sched::init();

    // 检查 Scheduler init 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_SCHED: &[u8] = b"DEBUG: PMR after scheduler init = 0x";
        for &b in MSG_PMR_SCHED {
            putchar(b);
        }
        let pmr_sched: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_sched,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_sched >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_sched & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    debug_println!("Initializing VFS...");
    crate::fs::vfs_init();

    // 检查 VFS init 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_VFS: &[u8] = b"DEBUG: PMR after VFS init = 0x";
        for &b in MSG_PMR_VFS {
            putchar(b);
        }
        let pmr_vfs: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_vfs,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_vfs >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_vfs & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // SMP 初始化（使用 PSCI）
    debug_println!("Initializing SMP...");
    crate::arch::aarch64::smp::SmpData::init(2);

    // 检查 SMP init 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_SMP: &[u8] = b"DEBUG: PMR after SMP init = 0x";
        for &b in MSG_PMR_SMP {
            putchar(b);
        }
        let pmr_smp: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_smp,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_smp >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_smp & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // 尝试 PSCI 启动次核
    debug_println!("Attempting PSCI CPU_ON...");
    crate::arch::aarch64::smp::boot_secondary_cpus();

    // 等待次核启动
    debug_println!("Waiting for secondary CPUs...");
    let mut wait_count = 0;
    while crate::arch::aarch64::smp::SmpData::get_active_cpu_count() < 2 {
        // 使用 NOP 而不是 WFI，避免在 IRQ 未正确配置时挂起
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
        wait_count += 1;
        if wait_count > 1000 {
            debug_println!("SMP: Timeout waiting for CPU 1");
            break;
        }
    }
    let active_cpus = crate::arch::aarch64::smp::SmpData::get_active_cpu_count();

    // 打印 CPU 数量（使用 putchar）
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"SMP: ";
        for &b in MSG {
            putchar(b);
        }
        let hex = b"0123456789";
        putchar(hex[active_cpus as usize]);
        const MSG2: &[u8] = b" CPUs online\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    // 在启用 IRQ 之前检查 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR0: &[u8] = b"DEBUG: PMR before IRQ enable = 0x";
        for &b in MSG_PMR0 {
            putchar(b);
        }
        let pmr_before: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_before,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_before >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_before & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // 启用 IRQ（GIC 已初始化，SMP 已启动）
    debug_println!("Enabling IRQ...");
    unsafe {
        // 直接设置 DAIF 为 0，使能所有中断
        core::arch::asm!("msr daif, xzr", options(nomem, nostack));  // DAIF = 0, 使能 IRQ 和 FIQ

        // 读取 DAIF 寄存器确认 IRQ 被使能
        use crate::console::putchar;
        const MSG_DAIF: &[u8] = b"DAIF after enable: 0x";
        for &b in MSG_DAIF {
            putchar(b);
        }
        let daif: u64;
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        let hex = b"0123456789ABCDEF";
        putchar(hex[((daif >> 4) & 0xF) as usize]);
        putchar(hex[(daif & 0xF) as usize]);
        const MSG_NL: &[u8] = b"\n";
        for &b in MSG_NL {
            putchar(b);
        }
    }

    // 检查 DAIF enable 后的 PMR
    unsafe {
        use crate::console::putchar;
        const MSG_PMR_DAIF: &[u8] = b"DEBUG: PMR after DAIF enable = 0x";
        for &b in MSG_PMR_DAIF {
            putchar(b);
        }
        let pmr_after_daif: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_after_daif,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_after_daif >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_after_daif & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    debug_println!("IRQ enabled");

    // 重启定时器以确保中断能够触发
    debug_println!("Restarting timer...");
    crate::drivers::timer::restart_timer();

    // 立即检查 PMR 状态
    unsafe {
        use crate::console::putchar;
        const MSG_PMR1: &[u8] = b"DEBUG: PMR after timer restart = 0x";
        for &b in MSG_PMR1 {
            putchar(b);
        }
        let pmr_after_timer: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr_after_timer,
            in(reg) 0x0801_0004,
            options(nomem, nostack)
        );
        let hex = b"0123456789ABCDEF";
        putchar(hex[((pmr_after_timer >> 4) & 0xF) as usize]);
        putchar(hex[(pmr_after_timer & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    debug_println!("System ready");

    // 调试：检查 GICC 寄存器状态
    unsafe {
        use crate::console::putchar;

        // 首先检查 GICD_CTLR（Distributor Control Register）
        const MSG_GICD_CTLR: &[u8] = b"DEBUG: Reading GICD_CTLR...\n";
        for &b in MSG_GICD_CTLR {
            putchar(b);
        }

        let gicd_ctlr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicd_ctlr,
            in(reg) 0x0800_0000,  // GICD_BASE + CTLR offset
            options(nomem, nostack)
        );

        const MSG_GICD_CTLR_VAL: &[u8] = b"DEBUG: GICD_CTLR = 0x";
        for &b in MSG_GICD_CTLR_VAL {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((gicd_ctlr >> 4) & 0xF) as usize]);
        putchar(hex[(gicd_ctlr & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }

        // 检查 Enable bit (bit 0)
        if gicd_ctlr & 1 != 0 {
            const MSG_ENABLED: &[u8] = b"DEBUG: GICD is enabled\n";
            for &b in MSG_ENABLED {
                putchar(b);
            }
        } else {
            const MSG_DISABLED: &[u8] = b"DEBUG: GICD is NOT enabled!\n";
            for &b in MSG_DISABLED {
                putchar(b);
            }
        }

        // 读取 GICC_CTLR（CPU Interface Control Register）
        const MSG_CTLR: &[u8] = b"DEBUG: Reading GICC_CTLR...\n";
        for &b in MSG_CTLR {
            putchar(b);
        }

        let ctlr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) ctlr,
            in(reg) 0x0801_0000,  // GICC_BASE + CTLR offset
            options(nomem, nostack)
        );

        const MSG_CTLR_VAL: &[u8] = b"DEBUG: GICC_CTLR = 0x";
        for &b in MSG_CTLR_VAL {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((ctlr >> 4) & 0xF) as usize]);
        putchar(hex[(ctlr & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 读取 GICC_PMR（Priority Mask Register）
        const MSG_PMR: &[u8] = b"DEBUG: Reading GICC_PMR...\n";
        for &b in MSG_PMR {
            putchar(b);
        }

        let pmr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pmr,
            in(reg) 0x0801_0004,  // GICC_BASE + PMR offset
            options(nomem, nostack)
        );

        const MSG_PMR_VAL: &[u8] = b"DEBUG: GICC_PMR = 0x";
        for &b in MSG_PMR_VAL {
            putchar(b);
        }
        putchar(hex[((pmr >> 4) & 0xF) as usize]);
        putchar(hex[(pmr & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 读取 GICC_BPR（Binary Point Register）
        const MSG_BPR: &[u8] = b"DEBUG: Reading GICC_BPR...\n";
        for &b in MSG_BPR {
            putchar(b);
        }

        let bpr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) bpr,
            in(reg) 0x0801_0008,  // GICC_BASE + BPR offset
            options(nomem, nostack)
        );

        const MSG_BPR_VAL: &[u8] = b"DEBUG: GICC_BPR = 0x";
        for &b in MSG_BPR_VAL {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[(bpr & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 读取 GICD_IGROUPR[0] - 检查 Group 配置
        const MSG_IGROUPR: &[u8] = b"DEBUG: Reading GICD_IGROUPR[0]...\n";
        for &b in MSG_IGROUPR {
            putchar(b);
        }

        let igroupr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) igroupr,
            in(reg) 0x0800_0080,  // GICD_BASE + IGROUPR offset
            options(nomem, nostack)
        );

        const MSG_IGROUPR_VAL: &[u8] = b"DEBUG: GICD_IGROUPR[0] = 0x";
        for &b in MSG_IGROUPR_VAL {
            putchar(b);
        }
        putchar(hex[((igroupr >> 28) & 0xF) as usize]);
        putchar(hex[((igroupr >> 24) & 0xF) as usize]);
        putchar(hex[((igroupr >> 20) & 0xF) as usize]);
        putchar(hex[((igroupr >> 16) & 0xF) as usize]);
        putchar(hex[((igroupr >> 12) & 0xF) as usize]);
        putchar(hex[((igroupr >> 8) & 0xF) as usize]);
        putchar(hex[((igroupr >> 4) & 0xF) as usize]);
        putchar(hex[(igroupr & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 检查 IRQ 30 的 Group 配置（bit 30）
        if igroupr & (1 << 30) != 0 {
            const MSG_GROUP1: &[u8] = b"DEBUG: IRQ 30 is in Group 1 (IRQ)\n";
            for &b in MSG_GROUP1 {
                putchar(b);
            }
        } else {
            const MSG_GROUP0: &[u8] = b"DEBUG: IRQ 30 is in Group 0 (FIQ)\n";
            for &b in MSG_GROUP0 {
                putchar(b);
            }
        }

        // 检查 GICD_ISENABLER 确认 IRQ 30 已使能
        const MSG_ISENABLER: &[u8] = b"DEBUG: Reading GICD_ISENABLER[0]...\n";
        for &b in MSG_ISENABLER {
            putchar(b);
        }

        let isenabler: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) isenabler,
            in(reg) 0x0800_0100,  // GICD_BASE + ISENABLER
            options(nomem, nostack)
        );

        const MSG_ISENABLER_VAL: &[u8] = b"DEBUG: GICD_ISENABLER[0] = 0x";
        for &b in MSG_ISENABLER_VAL {
            putchar(b);
        }
        putchar(hex[((isenabler >> 28) & 0xF) as usize]);
        putchar(hex[((isenabler >> 24) & 0xF) as usize]);
        putchar(hex[((isenabler >> 20) & 0xF) as usize]);
        putchar(hex[((isenabler >> 16) & 0xF) as usize]);
        putchar(hex[((isenabler >> 12) & 0xF) as usize]);
        putchar(hex[((isenabler >> 8) & 0xF) as usize]);
        putchar(hex[((isenabler >> 4) & 0xF) as usize]);
        putchar(hex[(isenabler & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 检查 IRQ 30 是否使能（bit 30）
        if isenabler & (1 << 30) != 0 {
            const MSG_ENABLED: &[u8] = b"DEBUG: IRQ 30 is enabled in ISENABLER\n";
            for &b in MSG_ENABLED {
                putchar(b);
            }
        } else {
            const MSG_DISABLED: &[u8] = b"DEBUG: IRQ 30 is NOT enabled in ISENABLER!\n";
            for &b in MSG_DISABLED {
                putchar(b);
            }
        }

        // 检查 ISPENDR 是否被 Timer 硬件自动设置
        const MSG_ISPENDR_BEFORE: &[u8] = b"DEBUG: Reading ISPENDR before manual set...\n";
        for &b in MSG_ISPENDR_BEFORE {
            putchar(b);
        }

        let ispendr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) ispendr,
            in(reg) 0x0800_0200,  // GICD_BASE + ISPENDR
            options(nomem, nostack)
        );

        const MSG_ISPENDR_VAL: &[u8] = b"DEBUG: GICD_ISPENDR[0] = 0x";
        for &b in MSG_ISPENDR_VAL {
            putchar(b);
        }
        putchar(hex[((ispendr >> 28) & 0xF) as usize]);
        putchar(hex[((ispendr >> 24) & 0xF) as usize]);
        putchar(hex[((ispendr >> 20) & 0xF) as usize]);
        putchar(hex[((ispendr >> 16) & 0xF) as usize]);
        putchar(hex[((ispendr >> 12) & 0xF) as usize]);
        putchar(hex[((ispendr >> 8) & 0xF) as usize]);
        putchar(hex[((ispendr >> 4) & 0xF) as usize]);
        putchar(hex[(ispendr & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 检查 bit 30
        if ispendr & (1 << 30) != 0 {
            const MSG_AUTO_SET: &[u8] = b"DEBUG: ISPENDR bit 30 was set by timer hardware\n";
            for &b in MSG_AUTO_SET {
                putchar(b);
            }
        } else {
            const MSG_NOT_SET: &[u8] = b"DEBUG: ISPENDR bit 30 was NOT set (manual set needed)\n";
            for &b in MSG_NOT_SET {
                putchar(b);
            }
        }

        // 尝试手动触发中断 - 设置 ISPENDR bit 30

        // 先检查 Timer 实际状态
        const MSG_TIMER_STATUS: &[u8] = b"DEBUG: Checking timer status...\n";
        for &b in MSG_TIMER_STATUS {
            putchar(b);
        }

        let cntp_ctl: u64;
        core::arch::asm!(
            "mrs {}, cntp_ctl_el0",
            out(reg) cntp_ctl,
            options(nomem, nostack)
        );

        const MSG_CTL: &[u8] = b"DEBUG: CNTP_CTL_EL0 = 0x";
        for &b in MSG_CTL {
            putchar(b);
        }
        putchar(hex[((cntp_ctl >> 4) & 0xF) as usize]);
        putchar(hex[(cntp_ctl & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 检查 ISTATUS (bit 2)
        if cntp_ctl & (1 << 2) != 0 {
            const MSG_ISTATUS: &[u8] = b"DEBUG: Timer ISTATUS=1 (interrupt pending)\n";
            for &b in MSG_ISTATUS {
                putchar(b);
            }
        } else {
            const MSG_NO_ISTATUS: &[u8] = b"DEBUG: Timer ISTATUS=0 (no interrupt)\n";
            for &b in MSG_NO_ISTATUS {
                putchar(b);
            }
        }

        // 尝试手动触发中断 - 设置 ISPENDR bit 30
        const MSG_TRIGGER: &[u8] = b"DEBUG: Manually setting ISPENDR bit 30...\n";
        for &b in MSG_TRIGGER {
            putchar(b);
        }

        core::arch::asm!(
            "str {}, [{}]",
            in(reg) 0x40000000u32,  // Set bit 30
            in(reg) 0x0800_0200,  // GICD_BASE + ISPENDR
            options(nomem, nostack)
        );

        const MSG_TRIGGER_DONE: &[u8] = b"DEBUG: ISPENDR bit 30 set\n";
        for &b in MSG_TRIGGER_DONE {
            putchar(b);
        }

        // 再次读取 GICC_IAR
        const MSG_IAR2: &[u8] = b"DEBUG: Reading GICC_IAR after manual trigger...\n";
        for &b in MSG_IAR2 {
            putchar(b);
        }

        let iar: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) iar,
            in(reg) 0x0801_000C,  // GICC_BASE + IAR offset
            options(nomem, nostack)
        );

        const MSG_IAR2_VAL: &[u8] = b"DEBUG: GICC_IAR = 0x";
        for &b in MSG_IAR2_VAL {
            putchar(b);
        }
        putchar(hex[((iar >> 12) & 0xF) as usize]);
        putchar(hex[((iar >> 8) & 0xF) as usize]);
        putchar(hex[((iar >> 4) & 0xF) as usize]);
        putchar(hex[(iar & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 如果 IAR 返回 30，写入 EOIR
        if iar == 30 {
            const MSG_EOIR: &[u8] = b"DEBUG: Writing EOIR for IRQ 30\n";
            for &b in MSG_EOIR {
                putchar(b);
            }

            core::arch::asm!(
                "str {}, [{}]",
                in(reg) 30u32,
                in(reg) 0x0801_0010,  // GICC_BASE + EOIR offset
                options(nomem, nostack)
            );
        }
    }

    // 测试 1: 使用底层 putchar 测试
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"After System ready\n";
        for &b in MSG {
            putchar(b);
        }

        // 测试 PID 获取
        const MSG2: &[u8] = b"Getting PID...\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    // 测试 2: 获取当前 PID
    let current_pid = process::sched::get_current_pid();

    // 打印 PID（使用十六进制）
    unsafe {
        use crate::console::putchar;
        const MSG3: &[u8] = b"Current PID: ";
        for &b in MSG3 {
            putchar(b);
        }

        let hex_chars = b"0123456789ABCDEF";
        let pid = current_pid as u64;
        putchar(hex_chars[((pid >> 60) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 56) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 52) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 48) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 44) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 40) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 36) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 32) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 28) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 24) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 20) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 16) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 12) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 8) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 4) & 0xF) as usize]);
        putchar(hex_chars[(pid & 0xF) as usize]);
        putchar(b'\n');
    }

    // 测试 fork 系统调用
    unsafe {
        use crate::console::putchar;
        const MSG5: &[u8] = b"Testing fork syscall...\n";
        for &b in MSG5 {
            putchar(b);
        }
    }

    // 直接调用 do_fork 测试（不通过系统调用）
    match process::sched::do_fork() {
        Some(child_pid) => {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Fork success: child PID = ";
                for &b in MSG {
                    putchar(b);
                }

                let hex_chars = b"0123456789ABCDEF";
                let pid = child_pid as u64;
                putchar(hex_chars[((pid >> 28) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 24) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 20) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 16) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 12) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 8) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 4) & 0xF) as usize]);
                putchar(hex_chars[(pid & 0xF) as usize]);
                putchar(b'\n');
            }
        }
        None => {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Fork failed\n";
                for &b in MSG {
                    putchar(b);
                }
            }
        }
    }

    // 主内核循环 - 等待中断
    unsafe {
        use crate::console::putchar;
        const MSG4: &[u8] = b"Entering main loop\n";
        for &b in MSG4 {
            putchar(b);
        }
    }

    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println!("!!! KERNEL PANIC !!!");
    println!("{}", _info);
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[lang = "eh_personality"]
extern "C" fn eh_personality() {
    loop {}
}

#[no_mangle]
extern "C" fn abort() -> ! {
    panic!("aborted");
}
