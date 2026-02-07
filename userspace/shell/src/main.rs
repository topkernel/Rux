#![no_std]
#![no_main]

//! 简单的 Shell - Rux OS 的第一个用户程序
//!
//! 这个 shell 会：
//! 1. 尝试执行 /hello_world
//! 2. 如果失败，打印错误信息

use core::panic::PanicInfo;

// 系统调用接口
mod syscall {
    /// 系统调用号
    pub const SYS_WRITE: u64 = 64;
    pub const SYS_EXIT: u64 = 93;
    pub const SYS_EXECVE: u64 = 221;

    /// 执行系统调用
    #[inline(always)]
    pub unsafe fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let ret: u64;
        core::arch::asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            lateout("a1") _,
            lateout("a2") _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
            options(nostack, nomem)
        );
        ret
    }

    /// 执行 execve 系统调用
    #[inline(always)]
    pub unsafe fn sys_execve(pathname: &str, argv: u64, envp: u64) -> u64 {
        syscall3(SYS_EXECVE, pathname.as_ptr() as u64, argv, envp)
    }
}

// 标准输出
fn print(s: &str) {
    unsafe {
        syscall::syscall3(
            syscall::SYS_WRITE,
            1,  // stdout
            s.as_ptr() as u64,
            s.len() as u64,
        );
    }
}

// 用户程序入口
#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("Rux Simple Shell v0.1\n");
    print("Attempting to execute /hello_world...\n");

    // 尝试执行 hello_world
    let result = unsafe {
        syscall::sys_execve("/hello_world\0", 0, 0)
    };

    if result == 0 {
        print("execve succeeded!\n");
    } else {
        print("execve failed with error: ");
        // 简单的错误码打印
        let err = -(result as i64);
        let digit = b'0' + (err as u8);
        unsafe {
            use core::arch::asm;
            asm!("
                mv a0, {}
                ecall
           ", in(reg) digit as u64);
        }
        print("\n");
    }

    // 退出
    unsafe {
        syscall::syscall3(syscall::SYS_EXIT, 0, 0, 0);
    }

    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

// Panic 处理
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("PANIC!\n");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
