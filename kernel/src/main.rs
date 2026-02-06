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

// RISC-V 汇编支持
// boot.S is not needed since boot.rs handles initialization
// Only trap.S (via global_asm in trap.rs) is required

// RISC-V kernel main function
#[cfg(feature = "riscv64")]
#[no_mangle]
pub extern "C" fn main() -> ! {
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

    debug_println!("System ready");

    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

// ARMv8 kernel entry point
#[cfg(feature = "aarch64")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 禁用中断直到中断控制器设置完成
    // 测试不同的 DAIF 值来找到正确的映射
    // 目标：同时屏蔽 I 和 F

    unsafe {
        // 尝试 0xC0 (bits 7,6) → 之前得到 0x80 (只有 bit 7)
        // 尝试 0x40 (bit 6) → 刚才得到 0x40 (正确!)
        // 尝试 0x80 (bit 7)
        let daif_val = 0xC0u64;  // 尝试设置 bits 7 和 6
        core::arch::asm!("msr daif, {}", in(reg) daif_val);
    }

    // 验证最终的 DAIF 值
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"_start: Final DAIF = 0x";
        for &b in MSG {
            putchar(b);
        }
        let daif_check: u64;
        asm!("mrs {}, daif", out(reg) daif_check, options(nomem, nostack));
        let hex = b"0123456789ABCDEF";
        for i in (0..16).rev() {
            putchar(hex[((daif_check >> (i * 4)) & 0xF) as usize]);
        }
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // 验证 DAIF 设置成功（在 UART 初始化之前直接使用 putchar）
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"_start: DAIF after masking = 0x";
        for &b in MSG {
            putchar(b);
        }
        let daif_check: u64;
        asm!("mrs {}, daif", out(reg) daif_check, options(nomem, nostack));
        // 打印完整的 64 位值（16 个十六进制数字）
        let hex = b"0123456789ABCDEF";
        for i in (0..16).rev() {
            putchar(hex[((daif_check >> (i * 4)) & 0xF) as usize]);
        }
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
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
    drivers::intc::init();  // 现在使用 GICv2 驱动
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

    // 检查点：确认代码能执行到这里
    unsafe {
        use crate::console::putchar;
        const MSG_CHK: &[u8] = b"CHK: Before scheduler init\n";
        for &b in MSG_CHK {
            putchar(b);
        }
    }

    debug_println!("Initializing scheduler...");
    // 临时禁用调度器初始化以测试
    // process::sched::init();

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
    // 临时禁用 VFS 初始化以测试
    // crate::fs::vfs_init();

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
    // 暂时禁用，调试 GICC_IAR
    debug_println!("Initializing SMP...");
    // crate::arch::aarch64::smp::SmpData::init(2);

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
    // 暂时禁用，调试 GICC_IAR
    debug_println!("Attempting PSCI CPU_ON...");
    // crate::arch::aarch64::smp::boot_secondary_cpus();

    // 等待次核启动
    debug_println!("Waiting for secondary CPUs...");
    // 临时禁用等待次核启动以测试
    /*
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
    */

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

    // 调试检查点
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"CHK1: Before IRQ enable\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // 启用 IRQ（GIC 已初始化，SMP 已启动）
    // debug_println!("Enabling IRQ...");
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"Enabling IRQ...\n";
        for &b in MSG {
            putchar(b);
        }
    }
    unsafe {
        // 只清除 I 位（bit 7），保留 F 位（bit 6/4）和其他位
        // 根据测试，msr daifset 的映射：imm[1] → bit 7 (I)
        // 所以 msr daifclr, #2 应该清除 bit 7
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));  // 只清除 I 位

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

        // 检查 GICD_ISPENDR0 以查看 IRQ 30 是否 pending
        const MSG_CHECK: &[u8] = b"DEBUG: Checking GICD_ISPENDR0 for IRQ 30...\n";
        for &b in MSG_CHECK {
            putchar(b);
        }
        let ispendr0: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) ispendr0,
            in(reg) 0x0800_0200usize,  // GICD_ISPENDR0
            options(nomem, nostack)
        );
        const MSG_ISPENDR: &[u8] = b"DEBUG: GICD_ISPENDR0 = 0x";
        for &b in MSG_ISPENDR {
            putchar(b);
        }
        for i in 0..8 {
            let shift = (7 - i) * 4;
            putchar(hex[((ispendr0 >> shift) & 0xF) as usize]);
        }
        const NL2: &[u8] = b"\n";
        for &b in NL2 {
            putchar(b);
        }

        // 检查 bit 30 (Timer IRQ)
        if ispendr0 & (1 << 30) != 0 {
            const MSG_PENDING: &[u8] = b"DEBUG: IRQ 30 (Timer) is PENDING in GICD!\n";
            for &b in MSG_PENDING {
                putchar(b);
            }
        } else {
            const MSG_NOT_PENDING: &[u8] = b"DEBUG: IRQ 30 (Timer) is NOT pending in GICD\n";
            for &b in MSG_NOT_PENDING {
                putchar(b);
            }
        }

        // 检查 GICD_ISENABLER0 以查看 IRQ 30 是否 enabled
        const MSG_CHECK_EN: &[u8] = b"DEBUG: Checking GICD_ISENABLER0 for IRQ 30...\n";
        for &b in MSG_CHECK_EN {
            putchar(b);
        }
        let isenabler0: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) isenabler0,
            in(reg) 0x0800_0100usize,  // GICD_ISENABLER0
            options(nomem, nostack)
        );
        const MSG_ISENABLER: &[u8] = b"DEBUG: GICD_ISENABLER0 = 0x";
        for &b in MSG_ISENABLER {
            putchar(b);
        }
        for i in 0..8 {
            let shift = (7 - i) * 4;
            putchar(hex[((isenabler0 >> shift) & 0xF) as usize]);
        }
        const NL3: &[u8] = b"\n";
        for &b in NL3 {
            putchar(b);
        }

        // 检查 bit 30 (Timer IRQ)
        if isenabler0 & (1 << 30) != 0 {
            const MSG_ENABLED: &[u8] = b"DEBUG: IRQ 30 (Timer) is ENABLED in GICD!\n";
            for &b in MSG_ENABLED {
                putchar(b);
            }
        } else {
            const MSG_NOT_ENABLED: &[u8] = b"DEBUG: IRQ 30 (Timer) is NOT enabled in GICD!\n";
            for &b in MSG_NOT_ENABLED {
                putchar(b);
            }
        }

        // 检查 IRQ 30 的优先级
        const MSG_CHECK_PRIO: &[u8] = b"DEBUG: Checking IRQ 30 priority...\n";
        for &b in MSG_CHECK_PRIO {
            putchar(b);
        }
        let ipriorityr7: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) ipriorityr7,
            in(reg) 0x0800_041Cusize,  // GICD_IPRIORITYR7 (IRQ 28-31)
            options(nomem, nostack)
        );
        // Timer 是 IRQ 30，对应 IPRIORITYR7 的字节 2 (bits 16-23)
        let irq30_prio = (ipriorityr7 >> 16) & 0xFF;
        const MSG_PRIO: &[u8] = b"DEBUG: IRQ 30 priority = 0x";
        for &b in MSG_PRIO {
            putchar(b);
        }
        putchar(hex[((irq30_prio >> 4) & 0xF) as usize]);
        putchar(hex[(irq30_prio & 0xF) as usize]);
        const NL4: &[u8] = b"\n";
        for &b in NL4 {
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

    // 使用 putchar 打印确认
    unsafe {
        use crate::console::putchar;
        const MSG1: &[u8] = b"System ready\n";
        for &b in MSG1 {
            putchar(b);
        }

        const MSG: &[u8] = b"After System ready - checkpoint 1\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // 调试：检查 GICC 寄存器状态
    // 临时禁用以测试
    /*
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

        // 注释掉手动触发中断的调试代码，让真实的定时器中断处理
        /*
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
        const MSG_IAR2: &[u8] = b"DEBUG: Reading GICD_ISACTIVER[0]...\n";
        for &b in MSG_IAR2 {
            putchar(b);
        }

        let isactiver: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) isactiver,
            in(reg) 0x0800_0300,  // GICD_BASE + ISACTIVER0
            options(nomem, nostack)
        );

        const MSG_ISACTIVER: &[u8] = b"DEBUG: ISACTIVER[0] = 0x";
        for &b in MSG_ISACTIVER {
            putchar(b);
        }
        putchar(hex[((isactiver >> 28) & 0xF) as usize]);
        putchar(hex[((isactiver >> 24) & 0xF) as usize]);
        putchar(hex[((isactiver >> 20) & 0xF) as usize]);
        putchar(hex[((isactiver >> 16) & 0xF) as usize]);
        putchar(hex[((isactiver >> 12) & 0xF) as usize]);
        putchar(hex[((isactiver >> 8) & 0xF) as usize]);
        putchar(hex[((isactiver >> 4) & 0xF) as usize]);
        putchar(hex[(isactiver & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        if isactiver & (1 << 30) != 0 {
            const MSG_ACTIVE: &[u8] = b"DEBUG: IRQ 30 is ACTIVE (not just pending)\n";
            for &b in MSG_ACTIVE {
                putchar(b);
            }
        }

        const MSG_IAR2_READ: &[u8] = b"DEBUG: Reading GICC_IAR after manual trigger...\n";
        for &b in MSG_IAR2_READ {
            putchar(b);
        }

        let iar_mmio: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) iar_mmio,
            in(reg) 0x0801_000C,  // GICC_BASE + IAR offset
            options(nomem, nostack)
        );

        const MSG_IAR_MMIO: &[u8] = b"DEBUG: GICC_IAR (MMIO) = 0x";
        for &b in MSG_IAR_MMIO {
            putchar(b);
        }
        putchar(hex[((iar_mmio >> 12) & 0xF) as usize]);
        putchar(hex[((iar_mmio >> 8) & 0xF) as usize]);
        putchar(hex[((iar_mmio >> 4) & 0xF) as usize]);
        putchar(hex[(iar_mmio & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 尝试通过系统寄存器读取
        const MSG_SYSREG: &[u8] = b"DEBUG: Reading ICC_IAR1_EL1 (sysreg)...\n";
        for &b in MSG_SYSREG {
            putchar(b);
        }
        let iar_sysreg: u32;
        core::arch::asm!(
            "mrs {}, iar_el1",
            out(reg) iar_sysreg,
            options(nomem, nostack)
        );

        const MSG_IAR_SYS: &[u8] = b"DEBUG: ICC_IAR1_EL1 = 0x";
        for &b in MSG_IAR_SYS {
            putchar(b);
        }
        putchar(hex[((iar_sysreg >> 12) & 0xF) as usize]);
        putchar(hex[((iar_sysreg >> 8) & 0xF) as usize]);
        putchar(hex[((iar_sysreg >> 4) & 0xF) as usize]);
        putchar(hex[(iar_sysreg & 0xF) as usize]);
        for &b in NL {
            putchar(b);
        }

        // 使用系统寄存器读取的值
        let iar = iar_sysreg;

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
        } else if iar >= 1020 {
            const MSG_SPURIOUS: &[u8] = b"DEBUG: Spurious interrupt, no EOI needed\n";
            for &b in MSG_SPURIOUS {
                putchar(b);
            }
        }
        */
    }
    */

    // 测试 1: 使用底层 putchar 测试
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"After System ready\n";
        for &b in MSG {
            putchar(b);
        }

        // 测试：轮询检查 GICC_IAR
        const MSG_POLL: &[u8] = b"DEBUG: Polling GICC_IAR for pending interrupt...\n";
        for &b in MSG_POLL {
            putchar(b);
        }

        // 使用内存映射方式读取 IAR
        const MSG_MMIO: &[u8] = b"DEBUG: Reading IAR via memory-mapped interface...\n";
        for &b in MSG_MMIO {
            putchar(b);
        }
        let iar: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) iar,
            in(reg) 0x0801_000Cusize,  // GICC_BASE + IAR offset
            options(nomem, nostack)
        );

        // IAR format: [9:0] = INTID, [12:10] = CPUID
        let irq_id = iar & 0x3FF;
        let cpu_id = (iar >> 10) & 0x7;

        const MSG_IAR: &[u8] = b"DEBUG: GICC_IAR = 0x";
        for &b in MSG_IAR {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((iar >> 12) & 0xF) as usize]);
        putchar(hex[((iar >> 8) & 0xF) as usize]);
        putchar(hex[((iar >> 4) & 0xF) as usize]);
        putchar(hex[(iar & 0xF) as usize]);

        const MSG_IRQ: &[u8] = b" (IRQ=";
        for &b in MSG_IRQ {
            putchar(b);
        }
        // 打印 IRQ ID
        let mut irq_val = irq_id;
        let mut buf = [0u8; 10];
        let mut pos = 9;
        if irq_val == 0 {
            buf[pos] = b'0';
            pos -= 1;
        } else {
            while irq_val > 0 {
                buf[pos] = b'0' + ((irq_val % 10) as u8);
                irq_val /= 10;
                if pos > 0 { pos -= 1; }
            }
        }
        for &b in &buf[pos..] {
            putchar(b);
        }

        const MSG_CPU: &[u8] = b", CPU=";
        for &b in MSG_CPU {
            putchar(b);
        }
        putchar(hex[(cpu_id as usize)]);

        const MSG_END: &[u8] = b")\n";
        for &b in MSG_END {
            putchar(b);
        }

        // 检查是否为 Timer 中断
        if irq_id == 30 {
            const MSG_TIMER: &[u8] = b"*** TIMER INTERRUPT (IRQ 30) ***\n";
            for &b in MSG_TIMER {
                putchar(b);
            }

            // 写入 EOIR 结束中断
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) 30u32,
                in(reg) 0x0801_0010usize,  // GICC_BASE + EOIR offset
                options(nostack)
            );

            const MSG_EOIR: &[u8] = b"DEBUG: Wrote EOIR for IRQ 30\n";
            for &b in MSG_EOIR {
                putchar(b);
            }

            // 重启定时器
            crate::drivers::timer::restart_timer();
            const MSG_RESTART: &[u8] = b"DEBUG: Timer restarted\n";
            for &b in MSG_RESTART {
                putchar(b);
            }
        }

        if iar == 30 || iar == 0x1E {
            const MSG_FIQQ: &[u8] = b"DEBUG: Timer interrupt (IRQ 30) is pending at CPU Interface!\n";
            for &b in MSG_FIQQ {
                putchar(b);
            }
        } else if iar == 0x3FF {
            const MSG_SPUR: &[u8] = b"DEBUG: No interrupt pending (spurious)\n";
            for &b in MSG_SPUR {
                putchar(b);
            }
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
    // 在进入主循环前，测试 SGI (软件生成中断)
    unsafe {
        use crate::console::putchar;
        const MSG_TEST: &[u8] = b"\n=== Testing SGI (Software Generated Interrupt) ===\n";
        for &b in MSG_TEST {
            putchar(b);
        }

        // 触发 SGI 0（发送到 CPU 0）
        // GICD_SGIR: Software Generated Interrupt Register
        // 格式: [25:24] = Target List Filter (0 = specific CPUs)
        //        [23:16] = CPU Target Mask (bit 0 = CPU 0)
        //        [9:0] = SGI interrupt ID (0-15)
        // 我们触发 SGI 0 到 CPU 0
        const MSG_SGIR: &[u8] = b"Triggering SGI 0 to CPU 0...\n";
        for &b in MSG_SGIR {
            putchar(b);
        }
        core::arch::asm!(
            "ldr {}, [{}]",
            in(reg) 0x0000_0000u32,  // SGI 0 to CPU 0
            in(reg) 0x0800_0F00usize,  // GICD_SGIR
            options(nostack)
        );

        const MSG_WAIT: &[u8] = b"SGI triggered, waiting for IRQ exception...\n";
        for &b in MSG_WAIT {
            putchar(b);
        }
        const MSG_HEADER: &[u8] = b"\n=== Final GIC Configuration Check ===\n";
        for &b in MSG_HEADER {
            putchar(b);
        }

        let hex = b"0123456789ABCDEF";

        // 检查 1: GICC_CTLR (CPU Interface Control Register)
        const MSG1: &[u8] = b"GICC_CTLR (0x0801_0000) = 0x";
        for &b in MSG1 {
            putchar(b);
        }
        let gicc_ctlr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicc_ctlr,
            in(reg) 0x0801_0000usize,
            options(nomem, nostack)
        );
        for i in 0..8 {
            let shift = (7 - i) * 4;
            putchar(hex[((gicc_ctlr >> shift) & 0xF) as usize]);
        }
        const NL1: &[u8] = b"\n";
        for &b in NL1 {
            putchar(b);
        }
        // 检查位 0 (Enable)
        if gicc_ctlr & 0x1 != 0 {
            const MSG_EN: &[u8] = b"  [0] Enable = 1 (enabled)\n";
            for &b in MSG_EN {
                putchar(b);
            }
        } else {
            const MSG_DIS: &[u8] = b"  [0] Enable = 0 (DISABLED!)\n";
            for &b in MSG_DIS {
                putchar(b);
            }
        }
        // 检查位 1 (EnableGrp1)
        if gicc_ctlr & 0x2 != 0 {
            const MSG_GRP1: &[u8] = b"  [1] EnableGrp1 = 1 (IRQ enabled)\n";
            for &b in MSG_GRP1 {
                putchar(b);
            }
        } else {
            const MSG_NOGRP1: &[u8] = b"  [1] EnableGrp1 = 0 (IRQ disabled)\n";
            for &b in MSG_NOGRP1 {
                putchar(b);
            }
        }

        // 检查 2: GICC_PMR (Priority Mask Register)
        const MSG2: &[u8] = b"GICC_PMR (0x0801_0004) = 0x";
        for &b in MSG2 {
            putchar(b);
        }
        let gicc_pmr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicc_pmr,
            in(reg) 0x0801_0004usize,
            options(nomem, nostack)
        );
        putchar(hex[((gicc_pmr >> 4) & 0xF) as usize]);
        putchar(hex[(gicc_pmr & 0xF) as usize]);
        const NL2: &[u8] = b"\n";
        for &b in NL2 {
            putchar(b);
        }

        // 检查 3: GICC_BPR (Binary Point Register)
        const MSG3: &[u8] = b"GICC_BPR (0x0801_0008) = 0x";
        for &b in MSG3 {
            putchar(b);
        }
        let gicc_bpr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicc_bpr,
            in(reg) 0x0801_0008usize,
            options(nomem, nostack)
        );
        putchar(hex[(gicc_bpr & 0xF) as usize]);
        const NL3: &[u8] = b"\n";
        for &b in NL3 {
            putchar(b);
        }

        // 检查 4: GICC_IAR (Interrupt Acknowledge Register) - 应该是 spurious (1023)
        const MSG_IAR: &[u8] = b"GICC_IAR (0x0801_000C) = 0x";
        for &b in MSG_IAR {
            putchar(b);
        }
        let gicc_iar: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicc_iar,
            in(reg) 0x0801_000Cusize,
            options(nomem, nostack)
        );
        for i in 0..8 {
            let shift = (7 - i) * 4;
            putchar(hex[((gicc_iar >> shift) & 0xF) as usize]);
        }
        const NL4: &[u8] = b"\n";
        for &b in NL4 {
            putchar(b);
        }
        let iar_irq = gicc_iar & 0x3FF;
        const MSG_IAR_IRQ: &[u8] = b"  IRQ ID = ";
        for &b in MSG_IAR_IRQ {
            putchar(b);
        }
        let mut irq_val = iar_irq;
        if irq_val == 1023 {
            const MSG_SPUR: &[u8] = b"1023 (Spurious)\n";
            for &b in MSG_SPUR {
                putchar(b);
            }
        } else {
            let mut buf = [0u8; 10];
            let mut pos = 9;
            if irq_val == 0 {
                buf[pos] = b'0';
            } else {
                while irq_val > 0 && pos > 0 {
                    buf[pos] = b'0' + ((irq_val % 10) as u8);
                    irq_val /= 10;
                    pos -= 1;
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const NL5: &[u8] = b"\n";
            for &b in NL5 {
                putchar(b);
            }
        }

        // 检查 5: GICD_CTLR (Distributor Control Register)
        const MSG_GICD: &[u8] = b"GICD_CTLR (0x0800_0000) = 0x";
        for &b in MSG_GICD {
            putchar(b);
        }
        let gicd_ctlr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) gicd_ctlr,
            in(reg) 0x0800_0000usize,
            options(nomem, nostack)
        );
        for i in 0..8 {
            let shift = (7 - i) * 4;
            putchar(hex[((gicd_ctlr >> shift) & 0xF) as usize]);
        }
        const NL6: &[u8] = b"\n";
        for &b in NL6 {
            putchar(b);
        }
        if gicd_ctlr & 0x1 != 0 {
            const MSG_EN2: &[u8] = b"  [0] Enable = 1 (Distributor enabled)\n";
            for &b in MSG_EN2 {
                putchar(b);
            }
        } else {
            const MSG_DIS2: &[u8] = b"  [0] Enable = 0 (Distributor DISABLED!)\n";
            for &b in MSG_DIS2 {
                putchar(b);
            }
        }

        const MSG_FOOTER: &[u8] = b"=== End GIC Configuration Check ===\n\n";
        for &b in MSG_FOOTER {
            putchar(b);
        }

        // 检查 6: GICD_ITARGETSR0-7 (CPU targets for IRQ 0-31)
        const MSG_TARGET: &[u8] = b"Checking CPU targets for PPIs (IRQ 0-31)...\n";
        for &b in MSG_TARGET {
            putchar(b);
        }
        for i in 0..8 {
            const MSG_REG: &[u8] = b"  ITARGETSR";
            for &b in MSG_REG {
                putchar(b);
            }
            // 打印寄存器编号
            let mut reg_num = i;
            let mut buf = [0u8; 2];
            buf[0] = b'0' + (reg_num as u8);
            for &b in &buf[..1] {
                putchar(b);
            }
            const MSG_EQ: &[u8] = b" = 0x";
            for &b in MSG_EQ {
                putchar(b);
            }
            let itargetsr: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) itargetsr,
                in(reg) 0x0800_0800usize + (i as usize) * 4,
                options(nomem, nostack)
            );
            for j in 0..8 {
                let shift = (7 - j) * 4;
                putchar(hex[((itargetsr >> shift) & 0xF) as usize]);
            }
            const NL_T: &[u8] = b"\n";
            for &b in NL_T {
                putchar(b);
            }
        }
        const NL_NL: &[u8] = b"\n";
        for &b in NL_NL {
            putchar(b);
        }

        // 检查 7: VBAR_EL1 (验证异常向量表)
        const MSG_VBAR: &[u8] = b"Checking VBAR_EL1...\n";
        for &b in MSG_VBAR {
            putchar(b);
        }
        let vbar_el1: u64;
        core::arch::asm!(
            "mrs {}, vbar_el1",
            out(reg) vbar_el1,
            options(nomem, nostack)
        );
        const MSG_VBAR_VAL: &[u8] = b"VBAR_EL1 = 0x";
        for &b in MSG_VBAR_VAL {
            putchar(b);
        }
        for i in 0..16 {
            let shift = (15 - i) * 4;
            putchar(hex[((vbar_el1 >> shift) & 0xF) as usize]);
        }
        const NL_VBAR: &[u8] = b"\n";
        for &b in NL_VBAR {
            putchar(b);
        }

        // 如果 VBAR_EL1 = 0，说明没有正确设置！
        if vbar_el1 == 0 {
            const MSG_VBAR_ERR: &[u8] = b"ERROR: VBAR_EL1 = 0! Exception vector table not installed!\n";
            for &b in MSG_VBAR_ERR {
                putchar(b);
            }
        } else {
            const MSG_VBAR_OK: &[u8] = b"VBAR_EL1 is set correctly\n";
            for &b in MSG_VBAR_OK {
                putchar(b);
            }
        }

        // 检查 8: DAIF 寄存器 (IRQ 屏蔽位)
        const MSG_DAIF: &[u8] = b"Checking DAIF register...\n";
        for &b in MSG_DAIF {
            putchar(b);
        }
        let daif: u64;
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) daif,
            options(nomem, nostack)
        );
        const MSG_DAIF_VAL: &[u8] = b"DAIF = 0x";
        for &b in MSG_DAIF_VAL {
            putchar(b);
        }
        for i in 0..16 {
            let shift = (15 - i) * 4;
            putchar(hex[((daif >> shift) & 0xF) as usize]);
        }
        const NL_DAIF: &[u8] = b"\n";
        for &b in NL_DAIF {
            putchar(b);
        }

        // 检查 I 位 (bit 7) - IRQ 屏蔽位
        let irq_masked = (daif >> 7) & 0x1;
        const MSG_I: &[u8] = b"IRQ mask bit (I) = ";
        for &b in MSG_I {
            putchar(b);
        }
        if irq_masked != 0 {
            const MSG_I_SET: &[u8] = b"1 (IRQ is MASKED!)\n";
            for &b in MSG_I_SET {
                putchar(b);
            }
            const MSG_I_ENABLE: &[u8] = b"Enabling IRQ by clearing I bit...\n";
            for &b in MSG_I_ENABLE {
                putchar(b);
            }
            core::arch::asm!(
                "msr daifclr, #2",
                options(nomem, nostack)
            );
            const MSG_I_ENABLED: &[u8] = b"IRQ enabled (DAIFCLR #2 executed)\n";
            for &b in MSG_I_ENABLED {
                putchar(b);
            }
        } else {
            const MSG_I_CLEAR: &[u8] = b"0 (IRQ is enabled)\n";
            for &b in MSG_I_CLEAR {
                putchar(b);
            }
        }

        const MSG_MAIN: &[u8] = b"Entering main loop\n";
        for &b in MSG_MAIN {
            putchar(b);
        }
    }

    loop {
        unsafe {
            use crate::console::putchar;

            // 首先检查 Timer 是否真的在运行
            // 读取 CNT_TVAL_EL0 (Timer value register)
            let tval: u64;
            core::arch::asm!("mrs {}, cntp_tval_el0", out(reg) tval, options(nomem, nostack));

            // 读取 CNTCTL_EL0 (Timer control register)
            let ctl: u64;
            core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) ctl, options(nomem, nostack));

            const MSG_TIMER: &[u8] = b"Timer: CTL=0x";
            for &b in MSG_TIMER {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            putchar(hex[((ctl >> 4) & 0xF) as usize]);
            putchar(hex[(ctl & 0xF) as usize]);

            const MSG_TVAL: &[u8] = b", TVAL=";
            for &b in MSG_TVAL {
                putchar(b);
            }
            let mut n = tval;
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if n == 0 {
                buf[pos] = b'0';
            } else {
                while n > 0 {
                    buf[pos] = b'0' + ((n % 10) as u8);
                    n /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const NL: &[u8] = b"\n";
            for &b in NL {
                putchar(b);
            }

            // 清除 ISPENDR0 bit 30，然后看 Timer 是否会再次设置它
            // 这可以测试 Timer interrupt 是否真的在触发
            const MSG_CLEAR: &[u8] = b"Clearing ISPENDR0 bit 30...\n";
            for &b in MSG_CLEAR {
                putchar(b);
            }
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) 0x4000_0000u32,  // Clear bit 30
                in(reg) 0x0800_0280usize,  // GICD_ICPENDR0
                options(nomem, nostack)
            );

            // 检查 GICD_ISPENDR0，看 Timer 是否 pending
            let ispendr0: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) ispendr0,
                in(reg) 0x0800_0200usize,  // GICD_ISPENDR0
                options(nomem, nostack)
            );

            const MSG_ISPENDR: &[u8] = b"ISPENDR0 = 0x";
            for &b in MSG_ISPENDR {
                putchar(b);
            }
            putchar(hex[((ispendr0 >> 28) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 24) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 20) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 16) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 12) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 8) & 0xF) as usize]);
            putchar(hex[((ispendr0 >> 4) & 0xF) as usize]);
            putchar(hex[(ispendr0 & 0xF) as usize]);
            for &b in NL {
                putchar(b);
            }

            // === 第一阶段：基础配置检查 ===
            const MSG_PHASE1: &[u8] = b"\n=== Phase 1: Static Configuration Check ===\n";
            for &b in MSG_PHASE1 {
                putchar(b);
            }

            // 0. 测试所有可能的 GICC 基地址
            const MSG_GICC_TEST: &[u8] = b"Testing GICC base addresses...\n";
            for &b in MSG_GICC_TEST {
                putchar(b);
            }

            // 测试地址1: 0x0801_0000 (标准 GICv2 Physical CPU Interface)
            let gicc_iidr1: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) gicc_iidr1,
                in(reg) 0x0801_00FCusize,
                options(nomem, nostack)
            );
            const MSG_GICC1: &[u8] = b"  [0x0801_0000] GICC_IIDR = 0x";
            for &b in MSG_GICC1 {
                putchar(b);
            }
            putchar(hex[((gicc_iidr1 >> 28) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 24) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 20) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 16) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 12) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 8) & 0xF) as usize]);
            putchar(hex[((gicc_iidr1 >> 4) & 0xF) as usize]);
            putchar(hex[(gicc_iidr1 & 0xF) as usize]);
            const NLG1: &[u8] = b"\n";
            for &b in NLG1 {
                putchar(b);
            }

            // 测试地址2: 0x0800_0000 (可能 GICC 在 GICD 基址)
            let gicc_iidr2: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) gicc_iidr2,
                in(reg) 0x0800_00FCusize,
                options(nomem, nostack)
            );
            const MSG_GICC2: &[u8] = b"  [0x0800_0000] GICC_IIDR = 0x";
            for &b in MSG_GICC2 {
                putchar(b);
            }
            putchar(hex[((gicc_iidr2 >> 28) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 24) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 20) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 16) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 12) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 8) & 0xF) as usize]);
            putchar(hex[((gicc_iidr2 >> 4) & 0xF) as usize]);
            putchar(hex[(gicc_iidr2 & 0xF) as usize]);
            const NLG2: &[u8] = b"\n";
            for &b in NLG2 {
                putchar(b);
            }

            // 测试地址3: 0x0808_0000 (GICv3 GICR 基址)
            let gicr_iidr3: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) gicr_iidr3,
                in(reg) 0x0808_00FCusize,
                options(nomem, nostack)
            );
            const MSG_GICC3: &[u8] = b"  [0x0808_0000] GICR_IIDR = 0x";
            for &b in MSG_GICC3 {
                putchar(b);
            }
            putchar(hex[((gicr_iidr3 >> 28) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 24) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 20) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 16) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 12) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 8) & 0xF) as usize]);
            putchar(hex[((gicr_iidr3 >> 4) & 0xF) as usize]);
            putchar(hex[(gicr_iidr3 & 0xF) as usize]);
            const NLG3: &[u8] = b"\n";
            for &b in NLG3 {
                putchar(b);
            }

            // 判断哪个地址有效
            if gicc_iidr1 != 0 && gicc_iidr1 != 0xFFFFFFFF {
                const MSG_VALID1: &[u8] = b"  => 0x0801_0000 is VALID GICC base\n";
                for &b in MSG_VALID1 {
                    putchar(b);
                }
            } else if gicc_iidr2 != 0 && gicc_iidr2 != 0xFFFFFFFF {
                const MSG_VALID2: &[u8] = b"  => 0x0800_0000 is VALID GICC base\n";
                for &b in MSG_VALID2 {
                    putchar(b);
                }
            } else if gicr_iidr3 != 0 && gicr_iidr3 != 0xFFFFFFFF {
                const MSG_VALID3: &[u8] = b"  => 0x0808_0000 is VALID (GICv3 mode)\n";
                for &b in MSG_VALID3 {
                    putchar(b);
                }
            } else {
                const MSG_NONE: &[u8] = b"  => No memory-mapped GICC found!\n";
                for &b in MSG_NONE {
                    putchar(b);
                }
                const MSG_SYSREG: &[u8] = b"  => Must use system register interface (icc_iar1_el1)\n";
                for &b in MSG_SYSREG {
                    putchar(b);
                }
            }
            const NL0: &[u8] = b"\n";
            for &b in NL0 {
                putchar(b);
            }

            // 1. GICD_IGROUPR0 (中断组 - 关键！)
            let igroupr0: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) igroupr0,
                in(reg) 0x0800_0080usize,
                options(nomem, nostack)
            );
            const MSG_IGROUPR: &[u8] = b"GICD_IGROUPR0 = 0x";
            for &b in MSG_IGROUPR {
                putchar(b);
            }
            putchar(hex[((igroupr0 >> 28) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 24) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 20) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 16) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 12) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 8) & 0xF) as usize]);
            putchar(hex[((igroupr0 >> 4) & 0xF) as usize]);
            putchar(hex[(igroupr0 & 0xF) as usize]);

            // 检查 IRQ 30 的组
            let irq30_group = (igroupr0 >> 30) & 0x1;
            const MSG_IRQ30_GRP: &[u8] = b"\n  IRQ 30 Group = ";
            for &b in MSG_IRQ30_GRP {
                putchar(b);
            }
            if irq30_group == 0 {
                const MSG_G0: &[u8] = b"0 (Group 0 - FIQ)\n";
                for &b in MSG_G0 {
                    putchar(b);
                }
            } else {
                const MSG_G1: &[u8] = b"1 (Group 1 - IRQ)\n";
                for &b in MSG_G1 {
                    putchar(b);
                }
            }

            // 2. GICC_CTLR (CPU接口使能和组使能)
            let gicc_ctlr: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) gicc_ctlr,
                in(reg) 0x0801_0000usize,
                options(nomem, nostack)
            );
            const MSG_GICC_CTLR: &[u8] = b"GICC_CTLR = 0x";
            for &b in MSG_GICC_CTLR {
                putchar(b);
            }
            putchar(hex[((gicc_ctlr >> 4) & 0xF) as usize]);
            putchar(hex[(gicc_ctlr & 0xF) as usize]);
            const NL2: &[u8] = b"\n";
            for &b in NL2 {
                putchar(b);
            }
            if gicc_ctlr & 0x1 != 0 {
                const MSG_EN0: &[u8] = b"  [0] EnableGrp0 = 1 (Group 0 enabled)\n";
                for &b in MSG_EN0 {
                    putchar(b);
                }
            }
            if gicc_ctlr & 0x2 != 0 {
                const MSG_EN1: &[u8] = b"  [1] EnableGrp1 = 1 (Group 1 enabled)\n";
                for &b in MSG_EN1 {
                    putchar(b);
                }
            }

            // 3. DAIF (CPU中断屏蔽)
            let daif: u64;
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
            const MSG_DAIF_CHK: &[u8] = b"DAIF = 0x";
            for &b in MSG_DAIF_CHK {
                putchar(b);
            }
            for i in 0..16 {
                let shift = (15 - i) * 4;
                putchar(hex[((daif >> shift) & 0xF) as usize]);
            }
            const NL3: &[u8] = b"\n";
            for &b in NL3 {
                putchar(b);
            }
            let irq_masked = (daif >> 7) & 0x1;
            if irq_masked == 0 {
                const MSG_DAIF_OK: &[u8] = b"  IRQ not masked (I bit = 0) [OK]\n";
                for &b in MSG_DAIF_OK {
                    putchar(b);
                }
            } else {
                const MSG_DAIF_ERR: &[u8] = b"  IRQ MASKED (I bit = 1) [ERROR]\n";
                for &b in MSG_DAIF_ERR {
                    putchar(b);
                }
            }

            // === 第二阶段：SGI 测试 ===
            const MSG_PHASE2: &[u8] = b"\n=== Phase 2: SGI Test (Software Generated Interrupt) ===\n";
            for &b in MSG_PHASE2 {
                putchar(b);
            }

            // 发送 SGI 0 给 CPU 0
            const MSG_SGI_TRIGGER: &[u8] = b"Triggering SGI 0 to CPU 0...\n";
            for &b in MSG_SGI_TRIGGER {
                putchar(b);
            }
            core::arch::asm!(
                "ldr {}, [{}]",
                in(reg) 0x0000_0000u32,  // SGI 0 to CPU 0
                in(reg) 0x0800_0F00usize,  // GICD_SGIR
                options(nomem, nostack)
            );

            // 立即读取 GICC_IAR - 尝试系统寄存器接口（GICv3 风格）
            let iar_sgi_mem: u32;
            let iar_sgi_sysreg: u32;

            // 方法1：内存映射接口（GICv2 标准）
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) iar_sgi_mem,
                in(reg) 0x0801_000Cusize,
                options(nomem, nostack)
            );

            // 方法2：系统寄存器接口（GICv3 风格）
            core::arch::asm!(
                "mrs {}, icc_iar1_el1",
                out(reg) iar_sgi_sysreg,
                options(nomem, nostack)
            );

            const MSG_IAR_SGI: &[u8] = b"GICC_IAR after SGI:\n  Mem-mapped = 0x";
            for &b in MSG_IAR_SGI {
                putchar(b);
            }
            putchar(hex[((iar_sgi_mem >> 12) & 0xF) as usize]);
            putchar(hex[((iar_sgi_mem >> 8) & 0xF) as usize]);
            putchar(hex[((iar_sgi_mem >> 4) & 0xF) as usize]);
            putchar(hex[(iar_sgi_mem & 0xF) as usize]);

            const MSG_SYSREG: &[u8] = b"\n  SysReg (iar_el1) = 0x";
            for &b in MSG_SYSREG {
                putchar(b);
            }
            putchar(hex[((iar_sgi_sysreg >> 12) & 0xF) as usize]);
            putchar(hex[((iar_sgi_sysreg >> 8) & 0xF) as usize]);
            putchar(hex[((iar_sgi_sysreg >> 4) & 0xF) as usize]);
            putchar(hex[(iar_sgi_sysreg & 0xF) as usize]);
            const NL_SYS: &[u8] = b"\n";
            for &b in NL_SYS {
                putchar(b);
            }

            // 使用系统寄存器的结果
            let iar_sgi = iar_sgi_sysreg;
            putchar(hex[((iar_sgi >> 12) & 0xF) as usize]);
            putchar(hex[((iar_sgi >> 8) & 0xF) as usize]);
            putchar(hex[((iar_sgi >> 4) & 0xF) as usize]);
            putchar(hex[(iar_sgi & 0xF) as usize]);

            let sgi_id = iar_sgi & 0x3FF;
            const MSG_SGI_ID: &[u8] = b" (IRQ=";
            for &b in MSG_SGI_ID {
                putchar(b);
            }
            let mut n = sgi_id;
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if n == 0 {
                buf[pos] = b'0';
            } else {
                while n > 0 {
                    buf[pos] = b'0' + ((n % 10) as u8);
                    n /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const MSG_SGI_END: &[u8] = b")\n";
            for &b in MSG_SGI_END {
                putchar(b);
            }

            if sgi_id == 0 {
                const MSG_SGI_OK: &[u8] = b"[OK] SGI test PASSED! CPU interface and IRQ path working!\n";
                for &b in MSG_SGI_OK {
                    putchar(b);
                }
                const MSG_SGI_OK2: &[u8] = b"  Problem is likely Timer-specific or security state mismatch.\n";
                for &b in MSG_SGI_OK2 {
                    putchar(b);
                }
            } else if sgi_id >= 1022 {
                const MSG_SGI_FAIL: &[u8] = b"[FAIL] SGI test FAILED! Still spurious (1022/1023).\n";
                for &b in MSG_SGI_FAIL {
                    putchar(b);
                }
                const MSG_SGI_FAIL2: &[u8] = b"  Problem: CPU interface or Group Enable mismatch!\n";
                for &b in MSG_SGI_FAIL2 {
                    putchar(b);
                }
                const MSG_SGI_FAIL3: &[u8] = b"  Check: GICC_CTLR EnableGrp0/1 matches Timer Group!\n";
                for &b in MSG_SGI_FAIL3 {
                    putchar(b);
                }
            }

            // 检查 GICD_ICFGR0 和 ICFGR1（中断配置寄存器）
            // 控制中断是电平触发还是边沿触发
            let icfgr0: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) icfgr0,
                in(reg) 0x0800_0C00usize,  // GICD_ICFGR0 (IRQ 0-15)
                options(nomem, nostack)
            );

            let icfgr1: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) icfgr1,
                in(reg) 0x0800_0C04usize,  // GICD_ICFGR1 (IRQ 16-31)
                options(nomem, nostack)
            );

            const MSG_ICFGR: &[u8] = b"ICFGR0 = 0x";
            for &b in MSG_ICFGR {
                putchar(b);
            }
            putchar(hex[((icfgr0 >> 28) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 24) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 20) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 16) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 12) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 8) & 0xF) as usize]);
            putchar(hex[((icfgr0 >> 4) & 0xF) as usize]);
            putchar(hex[(icfgr0 & 0xF) as usize]);

            const MSG_ICFGR1: &[u8] = b", ICFGR1 = 0x";
            for &b in MSG_ICFGR1 {
                putchar(b);
            }
            putchar(hex[((icfgr1 >> 28) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 24) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 20) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 16) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 12) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 8) & 0xF) as usize]);
            putchar(hex[((icfgr1 >> 4) & 0xF) as usize]);
            putchar(hex[(icfgr1 & 0xF) as usize]);
            for &b in NL {
                putchar(b);
            }

            // 检查 IRQ 30 的配置（在 ICFGR1 的 bits 4-5，bit 12-13）
            // 每个中断占用 2 bits: [1:0] = trigger type
            // IRQ 30 对应 ICFGR1 的 [29:28] = bits [12:11] of the register value
            let irq30_cfg = (icfgr1 >> 12) & 0x3;
            const MSG_IRQ30_CFG: &[u8] = b"IRQ 30 config (ICFGR) = 0x";
            for &b in MSG_IRQ30_CFG {
                putchar(b);
            }
            putchar(hex[(irq30_cfg as usize) & 0x3]);
            if irq30_cfg == 0 {
                const MSG_LEVEL: &[u8] = b" (Level-sensitive)\n";
                for &b in MSG_LEVEL {
                    putchar(b);
                }
            } else if irq30_cfg == 2 || irq30_cfg == 3 {
                const MSG_EDGE: &[u8] = b" (Edge-triggered)\n";
                for &b in MSG_EDGE {
                    putchar(b);
                }
            } else {
                const MSG_UNKNOWN: &[u8] = b" (Unknown)\n";
                for &b in MSG_UNKNOWN {
                    putchar(b);
                }
            }

            // 读取 GICC_IAR
            let iar: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) iar,
                in(reg) 0x0801_000Cusize,  // GICC_IAR
                options(nomem, nostack)
            );

            const MSG_IAR: &[u8] = b"GICC_IAR = 0x";
            for &b in MSG_IAR {
                putchar(b);
            }
            putchar(hex[((iar >> 12) & 0xF) as usize]);
            putchar(hex[((iar >> 8) & 0xF) as usize]);
            putchar(hex[((iar >> 4) & 0xF) as usize]);
            putchar(hex[(iar & 0xF) as usize]);

            let irq_id = iar & 0x3FF;
            const MSG_IRQ: &[u8] = b" (IRQ=";
            for &b in MSG_IRQ {
                putchar(b);
            }
            let mut n = irq_id;
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if n == 0 {
                buf[pos] = b'0';
            } else {
                while n > 0 {
                    buf[pos] = b'0' + ((n % 10) as u8);
                    n /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const MSG_END: &[u8] = b")\n";
            for &b in MSG_END {
                putchar(b);
            }

            // 如果是有效的中断（不是 spurious 1022/1023）
            if irq_id < 1020 {
                const MSG_VALID: &[u8] = b"Valid interrupt received!\n";
                for &b in MSG_VALID {
                    putchar(b);
                }

                // 写入 EOIR
                core::arch::asm!(
                    "str {}, [{}]",
                    in(reg) iar,
                    in(reg) 0x0801_0010usize,  // GICC_EOIR
                    options(nomem, nostack)
                );

                const MSG_EOI: &[u8] = b"EOI written\n";
                for &b in MSG_EOI {
                    putchar(b);
                }

                // 重启 Timer
                const MSG_RESTART: &[u8] = b"Restarting timer...\n";
                for &b in MSG_RESTART {
                    putchar(b);
                }
                crate::drivers::timer::restart_timer();
            } else {
                // Spurious interrupt，继续循环
                const MSG_SPURIOUS: &[u8] = b"Spurious interrupt, waiting...\n";
                for &b in MSG_SPURIOUS {
                    putchar(b);
                }

                // 使用小延迟而不是 WFI
                for _ in 0..1000 {
                    core::arch::asm!("nop", options(nomem, nostack));
                }
            }
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
