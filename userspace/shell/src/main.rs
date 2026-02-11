//! 简单的交互式 Shell - Rux OS
//!
//! 功能：
//! - 显示提示符
//! - 读取用户输入
//! - 执行内置命令（echo, help, exit）

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ============================================================================
// 系统调用
// ============================================================================

mod syscall {
    use core::arch::asm;

    /// 系统调用号 (RISC-V)
    pub const SYS_READ: u64 = 63;
    pub const SYS_WRITE: u64 = 64;
    pub const SYS_EXIT: u64 = 93;

    /// 执行系统调用
    #[inline(always)]
    pub unsafe fn syscall3(n: u64, a0: u64, _a1: u64, _a2: u64) -> u64 {
        let ret: u64;
        asm!(
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

    /// 从标准输入读取
    pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
        unsafe {
            let len = buf.len();
            let ptr = buf.as_mut_ptr();
            syscall3(SYS_READ, fd, ptr as u64, len as u64) as i64
        }
    }

    /// 写入到标准输出
    pub fn write(fd: u64, buf: &[u8]) -> i64 {
        unsafe {
            let len = buf.len();
            let ptr = buf.as_ptr();
            syscall3(SYS_WRITE, fd, ptr as u64, len as u64) as i64
        }
    }

    /// 退出进程
    pub fn exit(code: i32) -> ! {
        unsafe {
            syscall3(SYS_EXIT, code as u64, 0, 0);
        }
        loop {
            unsafe { asm!("wfi", options(nomem, nostack)); }
        }
    }
}

// ============================================================================
// 简单输出函数
// ============================================================================

fn print(s: &str) {
    syscall::write(1, s.as_bytes());
}

fn print_bytes(bytes: &[u8]) {
    syscall::write(1, bytes);
}

fn println() {
    print("\n");
}

// ============================================================================
// Shell 主循环
// ============================================================================

fn shell_main() {
    println();
    print_bytes(b"========================================\n");
    print_bytes(b"  Rux OS - Simple Shell v0.1\n");
    print_bytes(b"========================================\n");
    print_bytes(b"Type 'help' for available commands\n");
    println();

    let mut buf = [0u8; 256];

    loop {
        print("rux> ");

        // 读取一行
        let mut len = 0;

        while len < buf.len() {
            let ret = syscall::read(0, &mut buf[len..]);

            if ret <= 0 {
                break;
            }

            let ch = buf[len];

            if ch == b'\n' || ch == b'\r' {
                break;
            }

            if ch == b'\x08' || ch == b'\x7f' {
                // 退格键
                if len > 0 {
                    len -= 1;
                    print_bytes(b"\x08 \x08");
                }
            } else if ch >= b' ' {
                // 可打印字符，先回显，再增加长度
                print_bytes(&[ch]);
                len += 1;
            }
        }

        println();

        // 解析命令
        let input = core::str::from_utf8(&buf[..len]).unwrap_or("");
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        // 简单的命令解析
        let mut parts = input.split_whitespace();
        let cmd = parts.next().unwrap_or("");

        match cmd {
            "echo" => {
                // 打印参数
                let mut first = true;
                for arg in parts {
                    if !first {
                        print(" ");
                    }
                    first = false;
                    print(arg);
                }
                println();
            }
            "help" => {
                print_bytes(b"Rux Simple Shell v0.1\n");
                print_bytes(b"Available commands:\n");
                print_bytes(b"  echo <args>  - Print arguments\n");
                print_bytes(b"  help         - Show this help message\n");
                print_bytes(b"  exit         - Exit the shell\n");
                println();
            }
            "exit" | "quit" => {
                print_bytes(b"Goodbye!\n");
                break;
            }
            _ => {
                print("Unknown command: ");
                print(cmd);
                println();
                print_bytes(b"Type 'help' for available commands\n");
            }
        }
    }
}

// ============================================================================
// 入口点
// ============================================================================

#[no_mangle]
pub extern "C" fn _start() -> ! {
    shell_main();
    syscall::exit(0);
}

// ============================================================================
// Panic 处理
// ============================================================================

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print_bytes(b"Shell PANIC!\n");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
