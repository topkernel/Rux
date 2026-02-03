#![no_std]
#![no_main]
#![feature(lang_items, global_asm, naked_functions, alloc_error_handler)]

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

// 手动实现 __rust_alloc 函数（绕过 global_allocator 的符号可见性问题）
use core::alloc::Layout;
use core::alloc::GlobalAlloc;

#[no_mangle]
unsafe extern "C" fn __rust_alloc(size: usize, _align: usize) -> *mut u8 {
    use crate::console::putchar;
    const MSG: &[u8] = b"__rust_alloc\n";
    for &b in MSG {
        putchar(b);
    }

    let layout = Layout::from_size_align(size, 8).unwrap_or_else(|_| Layout::new::<u8>());
    GlobalAlloc::alloc(&crate::mm::allocator::HEAP_ALLOCATOR, layout)
}

#[no_mangle]
unsafe extern "C" fn __rust_dealloc(ptr: *mut u8, _size: usize, _align: usize) {
    if ptr.is_null() {
        return;
    }
    // Bump分配器不支持释放，所以忽略
}

#[no_mangle]
unsafe extern "C" fn __rust_realloc(_ptr: *mut u8, _old_size: usize, _align: usize, new_size: usize) -> *mut u8 {
    __rust_alloc(new_size, 8)
}

#[no_mangle]
unsafe extern "C" fn __rust_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    let ptr = __rust_alloc(size, align);
    if !ptr.is_null() {
        core::ptr::write_bytes(ptr, 0, size);
    }
    ptr
}

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

    debug_println!("Testing Vec with capacity...");
    use alloc::vec::Vec;
    let mut test_vec = Vec::with_capacity(10);
    test_vec.push(42);
    debug_println!("Vec works!");

    debug_println!("Initializing scheduler...");
    process::sched::init();

    debug_println!("Initializing VFS...");
    crate::fs::vfs_init();

    // 暂时禁用 GIC 和 Timer，避免导致挂起
    // 这些会导致内核挂起，需要进一步调试
    /*
    debug_println!("Initializing GIC...");
    drivers::intc::init();

    debug_println!("Initializing timer...");
    drivers::timer::init();
    */

    debug_println!("System ready");

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
