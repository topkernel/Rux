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
global_asm!(include_str!("arch/aarch64/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 禁用中断直到中断控制器设置完成
    unsafe {
        asm!("msr daifset, #2", options(nomem, nostack));
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
                if let Some(val) = test_vec.get(0) {
                    #[cfg(debug_assertions)]
                    println!("SimpleVec works! value = {}", val);
                }
            }
        }
        None => {}
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

    debug_println!("Initializing scheduler...");
    process::sched::init();

    debug_println!("Initializing VFS...");
    crate::fs::vfs_init();

    // 初始化 GIC（必须在 SMP 和 IRQ 之前）
    debug_println!("Initializing GIC...");
    drivers::intc::init();

    // 注意：IRQ 将在 SMP 初始化完成后再启用
    debug_println!("IRQ disabled - will enable after SMP init");

    // SMP 初始化 - 启动次核（必须在 GIC 之后）
    debug_println!("Booting secondary CPUs...");
    {
        use arch::aarch64::smp::{boot_secondary_cpus, SmpData};
        // 初始化 SMP 数据结构
        SmpData::init(2); // 支持 2 个 CPU
        boot_secondary_cpus();

        // 等待次核启动完成
        // 使用适中的延迟循环，确保 CPU 1 完全启动并进入 WFI
        for _ in 0..20000000 {
            unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
        }

        // 内存屏障，确保看到 CPU 1 的状态更新
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        let active = SmpData::get_active_cpu_count();
        println!("SMP: {} CPUs online", active);
    }

    // SMP 初始化完成，现在启用 IRQ
    debug_println!("SMP init complete, enabling IRQ...");
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
    debug_println!("IRQ enabled");

    // Debug: 检查是否到达这里（应该只有 CPU 0 能到达）
    println!("DEBUG: After SMP block, CPU={}", arch::aarch64::cpu::get_core_id());

    // Timer 暂时禁用（需要进一步调试）
    /*
    debug_println!("Initializing timer...");
    drivers::timer::init();
    */

    debug_println!("System ready");

    // 获取当前 PID
    let current_pid = process::sched::get_current_pid();
    println!("Current PID: {:#x}", current_pid as u64);

    // 测试 fork 系统调用
    debug_println!("Testing fork syscall...");

    // 直接调用 do_fork 测试（不通过系统调用）
    match process::sched::do_fork() {
        Some(child_pid) => {
            println!("Fork success: child PID = {:#x}", child_pid as u64);
        }
        None => {
            println!("Fork failed");
        }
    }

    // 主内核循环 - 等待中断
    debug_println!("Entering main loop");

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
