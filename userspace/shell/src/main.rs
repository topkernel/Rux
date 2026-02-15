//! 简单的交互式 Shell - Rux OS
//!
//! 功能：
//! - 显示提示符
//! - 读取用户输入
//! - 执行内置命令（echo, help, exit）
//! - 执行外部程序（通过 execve）

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
    pub const SYS_FORK: u64 = 220;
    pub const SYS_EXECVE: u64 = 221;
    pub const SYS_WAIT4: u64 = 260;

    /// 执行系统调用 (3 参数)
    #[inline(always)]
    pub unsafe fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let ret: u64;
        asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            inlateout("a1") a1 => _,
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

    /// fork 进程
    pub fn fork() -> i64 {
        unsafe { syscall3(SYS_FORK, 0, 0, 0) as i64 }
    }

    /// 执行程序
    pub fn execve(pathname: *const u8, argv: *const *const u8, envp: *const *const u8) -> i64 {
        unsafe { syscall3(SYS_EXECVE, pathname as u64, argv as u64, envp as u64) as i64 }
    }

    /// 等待子进程
    pub fn wait4(pid: i64, wstatus: *mut i32, options: i32, rusage: *mut u8) -> i64 {
        unsafe { syscall3(SYS_WAIT4, pid as u64, wstatus as u64, options as u64) as i64 }
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

/// 执行外部程序
fn execute_program(pathname: &[u8], args: &[&[u8]]) -> i64 {
    // 构建路径字符串（null-terminated）
    let mut path_buf = [0u8; 256];
    if pathname.len() >= path_buf.len() {
        return -1;
    }
    path_buf[..pathname.len()].copy_from_slice(pathname);
    path_buf[pathname.len()] = 0; // null terminator

    // 构建参数数组
    let mut argv_buf = [0 as *const u8; 16];
    let mut argv_strs = [[0u8; 128]; 16];

    // 第一个参数是程序名
    argv_strs[0][..pathname.len()].copy_from_slice(pathname);
    argv_strs[0][pathname.len()] = 0;
    argv_buf[0] = argv_strs[0].as_ptr();

    // 添加额外参数
    for (i, arg) in args.iter().enumerate() {
        if i + 1 >= argv_buf.len() {
            break;
        }
        if arg.len() >= argv_strs[i + 1].len() {
            break;
        }
        argv_strs[i + 1][..arg.len()].copy_from_slice(arg);
        argv_strs[i + 1][arg.len()] = 0;
        argv_buf[i + 1] = argv_strs[i + 1].as_ptr();
    }

    // envp 为空
    let envp: *const *const u8 = core::ptr::null();

    syscall::execve(
        path_buf.as_ptr(),
        argv_buf.as_ptr(),
        envp,
    )
}

/// 运行外部命令（fork + execve + wait）
fn run_external_command(cmd: &str, args: &[&str]) {
    // 构建完整路径
    let mut pathname = [0u8; 256];
    let cmd_bytes = cmd.as_bytes();

    if cmd.starts_with('/') {
        // 绝对路径
        if cmd_bytes.len() < pathname.len() {
            pathname[..cmd_bytes.len()].copy_from_slice(cmd_bytes);
        }
    } else if cmd.starts_with("./") {
        // 当前目录
        if cmd_bytes.len() < pathname.len() {
            pathname[..cmd_bytes.len()].copy_from_slice(cmd_bytes);
        }
    } else {
        // 在 /bin 中查找
        let bin_prefix = b"/bin/";
        pathname[..bin_prefix.len()].copy_from_slice(bin_prefix);
        if bin_prefix.len() + cmd_bytes.len() < pathname.len() {
            pathname[bin_prefix.len()..bin_prefix.len() + cmd_bytes.len()].copy_from_slice(cmd_bytes);
        }
    }

    // 转换参数（先复制到数组，再创建切片引用）
    let mut args_bytes: [[u8; 128]; 16] = [[0; 128]; 16];
    let mut args_lens: [usize; 16] = [0; 16];
    for (i, arg) in args.iter().enumerate() {
        if i >= args_bytes.len() {
            break;
        }
        let arg_bytes = arg.as_bytes();
        let len = arg_bytes.len().min(args_bytes[i].len());
        args_bytes[i][..len].copy_from_slice(&arg_bytes[..len]);
        args_lens[i] = len;
    }

    // 创建切片引用数组
    let mut args_slices: [&[u8]; 16] = [&[]; 16];
    for i in 0..args.len().min(args_slices.len()) {
        args_slices[i] = &args_bytes[i][..args_lens[i]];
    }

    // fork 子进程
    let pid = syscall::fork();

    if pid < 0 {
        // fork 失败
        print_bytes(b"fork failed\n");
    } else if pid == 0 {
        // 子进程：执行程序
        let ret = execute_program(&pathname, &args_slices);
        print_bytes(b"execve failed\n");
        syscall::exit(1);
    } else {
        // 父进程：等待子进程结束
        let mut status: i32 = 0;
        syscall::wait4(pid, &mut status as *mut i32, 0, core::ptr::null_mut());
    }
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
                print_bytes(b"  <program>    - Execute external program\n");
                println();
            }
            "exit" | "quit" => {
                print_bytes(b"Goodbye!\n");
                break;
            }
            _ => {
                // 尝试执行外部程序
                // 收集参数（最多 8 个）
                let mut args: [&str; 8] = [""; 8];
                let mut arg_count = 0;
                for arg in parts {
                    if arg_count >= args.len() {
                        break;
                    }
                    args[arg_count] = arg;
                    arg_count += 1;
                }
                run_external_command(cmd, &args[..arg_count]);
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
