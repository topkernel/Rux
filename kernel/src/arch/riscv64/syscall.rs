//! RISC-V 64-bit 系统调用处理
//!
//! 实现基于 ecall (Environment Call) 指令的系统调用接口
//!
//! RISC-V 系统调用约定:
//! - a7: 系统调用号
//! - a0-a5: 参数 (最多6个)
//! - 返回值: a0
//! - 错误码: a0 设置为负数

use core::arch::asm;
use crate::println;
use crate::debug_println;

/// 系统调用编号 (RISC-V ABI)
///
/// 遵循 RISC-V Linux ABI 的系统调用号
/// 参考: arch/riscv/include/asm/unistd.h
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SyscallNo {
    /// IO 操作
    Read = 63,
    Write = 64,
    Open = 1024,
    Close = 57,
    Stat = 1038,
    Fstat = 80,
    Lstat = 1039,
    Poll = 168,
    Lseek = 62,
    Mmap = 222,
    Mprotect = 226,
    Munmap = 215,
    Brk = 214,
    Ioctl = 29,

    /// 进程操作
    Clone = 220,
    Execve = 221,
    Exit = 93,
    ExitGroup = 94,
    Wait4 = 260,
    Kill = 129,
    Getpid = 172,
    Getppid = 110,

    /// 文件操作
    Openat = 56,
    Unlink = 74,
    Mkdir = 77,
    Rmdir = 79,

    /// 信号操作
    RtSigaction = 134,
    RtSigprocmask = 135,
    RtSigreturn = 139,
    Sigaltstack = 132,

    /// 时间操作
    Gettimeofday = 169,
    ClockGettime = 113,
    ClockGetres = 114,

    /// 其他
    Pipe = 59,
    Dup = 23,
    Dup2 = 24,
    Getuid = 174,
    Getgid = 176,
    Geteuid = 175,
    Getegid = 177,
    Uname = 160,
    Fcntl = 25,
}

/// 系统调用寄存器上下文
/// 必须与 trap.S 中的栈帧布局匹配
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SyscallFrame {
    pub a0: u64,   // offset 0   - 返回值 / 第1个参数
    pub a1: u64,   // offset 8   - 第2个参数
    pub a2: u64,   // offset 16  - 第3个参数
    pub a3: u64,   // offset 24  - 第4个参数
    pub a4: u64,   // offset 32  - 第5个参数
    pub a5: u64,   // offset 40  - 第6个参数
    pub a6: u64,   // offset 48
    pub a7: u64,   // offset 56  - 系统调用号
    pub t0: u64,   // offset 64
    pub t1: u64,   // offset 72
    pub t2: u64,   // offset 80
    pub t3: u64,   // offset 88
    pub t4: u64,   // offset 96
    pub t5: u64,   // offset 104
    pub t6: u64,   // offset 112
    pub s0: u64,   // offset 120
    pub s1: u64,   // offset 128
    pub s2: u64,   // offset 136
    pub s3: u64,   // offset 144
    pub s4: u64,   // offset 152
    pub s5: u64,   // offset 160
    pub s6: u64,   // offset 168
    pub s7: u64,   // offset 176
    pub s8: u64,   // offset 184
    pub s9: u64,   // offset 192
    pub s10: u64,  // offset 200
    pub s11: u64,  // offset 208
    pub ra: u64,   // offset 216 - 返回地址
    pub sp: u64,   // offset 224 - 栈指针
    pub gp: u64,   // offset 232
    pub tp: u64,   // offset 240
    pub pc: u64,   // offset 248 - 程序计数器
    pub status: u64, // offset 256 - 程序状态 (mstatus)
}

impl Default for SyscallFrame {
    fn default() -> Self {
        Self {
            a0: 0,
            a1: 0,
            a2: 0,
            a3: 0,
            a4: 0,
            a5: 0,
            a6: 0,
            a7: 0,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
            t4: 0,
            t5: 0,
            t6: 0,
            s0: 0,
            s1: 0,
            s2: 0,
            s3: 0,
            s4: 0,
            s5: 0,
            s6: 0,
            s7: 0,
            s8: 0,
            s9: 0,
            s10: 0,
            s11: 0,
            ra: 0,
            sp: 0,
            gp: 0,
            tp: 0,
            pc: 0,
            status: 0,
        }
    }
}

/// 处理系统调用
///
/// RISC-V 系统调用约定:
/// - a7: 系统调用号
/// - a0-a5: 参数 (最多6个)
/// - 返回值: a0
/// - 错误码: a0 设置为负数
#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) {
    // 调试输出：打印系统调用号
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"[ECALL:";
        for &b in MSG {
            putchar(b);
        }
        // 打印 a7 的十六进制值
        let hex_chars = b"0123456789ABCDEF";
        let val = frame.a7;
        putchar(hex_chars[((val >> 4) & 0xF) as usize]);
        putchar(hex_chars[(val & 0xF) as usize]);
        const MSG2: &[u8] = b"]\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    let syscall_no = frame.a7;
    let args = [frame.a0, frame.a1, frame.a2, frame.a3, frame.a4, frame.a5];

    // 根据系统调用号分发
    frame.a0 = match syscall_no as u32 {
        63 => sys_read(args),
        64 => sys_write(args),
        56 => sys_openat(args),
        57 => sys_close(args),
        93 => sys_exit(args),
        172 => sys_getpid(args),
        110 => sys_getppid(args),
        129 => sys_kill(args),
        59 => sys_pipe(args),
        220 => sys_fork(args),
        221 => sys_execve(args),
        260 => sys_wait4(args),
        160 => sys_uname(args),
        174 => sys_getuid(args),
        176 => sys_getgid(args),
        175 => sys_geteuid(args),
        177 => sys_getegid(args),
        169 => sys_gettimeofday(args),
        113 => sys_clock_gettime(args),
        23 => sys_dup(args),
        24 => sys_dup2(args),
        25 => sys_fcntl(args),
        _ => {
            debug_println!("Unknown syscall: {}", syscall_no);
            -38_i64 as u64  // ENOSYS - 函数未实现
        }
    };
}

// ============================================================================
// 系统调用实现
// ============================================================================

/// read - 从文件描述符读取
fn sys_read(args: [u64; 6]) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;

    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let result = file.read(buf, count);
                if result < 0 {
                    result as u32 as u64  // 返回错误码
                } else {
                    result as u64  // 返回读取的字节数
                }
            }
            None => {
                debug_println!("sys_read: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

/// write - 写入到文件描述符
fn sys_write(args: [u64; 6]) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;

    unsafe {
        // Special handling for stdout (1) and stderr (2) - write directly to UART
        if fd == 1 || fd == 2 {
            use crate::console::putchar;
            let slice = core::slice::from_raw_parts(buf, count);
            for &b in slice {
                putchar(b);
            }
            return count as u64;
        }

        match get_file_fd(fd) {
            Some(file) => {
                let result = file.write(buf, count);
                if result < 0 {
                    result as u32 as u64  // 返回错误码
                } else {
                    result as u64  // 返回写入的字节数
                }
            }
            None => {
                debug_println!("sys_write: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

/// openat - 打开文件（相对于目录文件描述符）
fn sys_openat(args: [u64; 6]) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mode = args[3] as u32;

    println!("sys_openat: pathname_ptr={:#x}, flags={:#x}, mode={:#x}",
             pathname_ptr as usize, flags, mode);

    // 检查路径指针
    if pathname_ptr.is_null() {
        println!("sys_openat: null pathname");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取文件名（简化：假设以 null 结尾）
    let filename = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    // 转换为字符串
    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_openat: invalid utf-8 filename");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_openat: opening '{}'", filename_str);

    // 调用 VFS 打开文件
    match crate::fs::file_open(filename_str, flags, mode) {
        Ok(fd) => fd as u64,
        Err(e) => e as u32 as u64,
    }
}

/// close - 关闭文件描述符
fn sys_close(args: [u64; 6]) -> u64 {
    use crate::fs::close_file_fd;
    let fd = args[0] as usize;

    println!("sys_close: fd={}", fd);

    unsafe {
        match close_file_fd(fd) {
            Ok(()) => 0,
            Err(e) => e as u32 as u64,
        }
    }
}

/// pipe - 创建管道
fn sys_pipe(args: [u64; 6]) -> u64 {
    debug_println!("sys_pipe: not fully implemented");
    -38_i64 as u64  // ENOSYS
}

/// getpid - 获取进程 ID
fn sys_getpid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_pid() as u64
}

/// getppid - 获取父进程 ID
fn sys_getppid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_ppid() as u64
}

/// getuid - 获取用户 ID
fn sys_getuid(_args: [u64; 6]) -> u64 {
    0  // root 用户
}

/// getgid - 获取组 ID
fn sys_getgid(_args: [u64; 6]) -> u64 {
    0  // root 组
}

/// geteuid - 获取有效用户 ID
fn sys_geteuid(_args: [u64; 6]) -> u64 {
    0
}

/// getegid - 获取有效组 ID
fn sys_getegid(_args: [u64; 6]) -> u64 {
    0
}

/// exit - 退出当前进程
fn sys_exit(args: [u64; 6]) -> u64 {
    let exit_code = args[0] as i32;
    println!("sys_exit: exiting with code {}", exit_code);
    crate::process::sched::do_exit(exit_code);
}

/// kill - 向进程发送信号
fn sys_kill(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    println!("sys_kill: pid={}, sig={}", pid, sig);

    match crate::process::sched::send_signal(pid as u32, sig) {
        Ok(()) => 0,
        Err(e) => e as u32 as u64,
    }
}

/// fork - 创建子进程
fn sys_fork(_args: [u64; 6]) -> u64 {
    println!("sys_fork: creating new process");

    match crate::process::sched::do_fork() {
        Some(pid) => {
            println!("sys_fork: created process with PID {}", pid);
            pid as u64
        }
        None => {
            println!("sys_fork: failed to create process");
            (-1_i64) as u64  // 返回 -1 表示失败
        }
    }
}

/// execve - 执行新程序
///
/// 系统调用签名：int execve(const char *pathname, char *const argv[], char *const envp[]);
///
/// 对应 Linux 的 execve 系统调用 (syscall 221)
///
/// # 实现状态
///
/// 当前实现：
/// - ✅ 解析文件名
/// - ✅ 从 RootFS 读取 ELF 文件
/// - ✅ 使用 ElfLoader 验证和解析 ELF
/// - ✅ 打印详细的加载信息
/// - ⏳ 真正加载到内存并执行（需要完整的地址空间管理）
fn sys_execve(args: [u64; 6]) -> u64 {
    use crate::fs::elf::{ElfLoader, ElfError};
    use crate::fs;

    let pathname_ptr = args[0] as *const u8;
    let _argv = args[1] as *const *const u8;
    let _envp = args[2] as *const *const u8;

    println!("sys_execve: called");

    // ===== 1. 读取文件名 =====
    if pathname_ptr.is_null() {
        println!("sys_execve: null pathname");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取文件名（简化：假设以 null 结尾，最大长度 256）
    let filename = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_execve: invalid utf-8 filename");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_execve: pathname='{}'", filename_str);

    // ===== 2. 从文件系统读取文件 =====
    let file_data = fs::read_file_from_rootfs(filename_str);
    let file_data = match file_data {
        Some(data) => data,
        None => {
            println!("sys_execve: file not found: {}", filename_str);
            return -2_i64 as u64;  // ENOENT
        }
    };

    println!("sys_execve: file size = {} bytes", file_data.len());

    // ===== 3. 验证 ELF 格式 =====
    let validation_result = ElfLoader::validate(&file_data);
    if let Err(e) = validation_result {
        println!("sys_execve: invalid ELF: {:?}", e);
        return -8_i64 as u64;  // ENOEXEC
    }

    // ===== 4. 获取 ELF 头信息 =====
    let ehdr = unsafe { ElfLoader::get_entry(&file_data) };
    let entry = match ehdr {
        Ok(addr) => addr,
        Err(e) => {
            println!("sys_execve: failed to get entry: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: ELF entry point = {:#x}", entry);

    // ===== 5. 获取程序头表 =====
    let phdrs = match ElfLoader::get_program_headers(&file_data) {
        Ok(hdrs) => hdrs,
        Err(e) => {
            println!("sys_execve: failed to get program headers: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: {} program headers", phdrs.len());

    // ===== 6. 分析 PT_LOAD 段 =====
    let mut load_count = 0;
    for (i, phdr) in phdrs.iter().enumerate() {
        if phdr.is_load() {
            println!("  PT_LOAD[{}]: vaddr={:#x}, filesz={}, memsz={}, flags={:#x}",
                     i, phdr.p_vaddr, phdr.p_filesz, phdr.p_memsz, phdr.p_flags);
            load_count += 1;
        }
    }

    // ===== 7. 检查 PT_INTERP（动态链接器） =====
    if let Some(interp) = ElfLoader::get_interpreter(&file_data) {
        let interp_str = core::str::from_utf8(interp).unwrap_or("<invalid>");
        println!("sys_execve: interpreter: {}", interp_str);
    }

    // ===== 8. 模拟加载 =====
    // 注意：由于当前地址空间管理不完整，我们暂时不真正加载和执行
    // 未来需要实现：
    // - 分配用户虚拟地址空间
    // - 映射 PT_LOAD 段到内存
    // - 设置用户栈和参数（argv, envp）
    // - 使用 mret 指令跳转到用户空间

    println!("sys_execve: ELF validation successful");
    println!("sys_execve: {} loadable segments", load_count);
    println!("sys_execve: TODO: actually load and execute (needs address space management)");

    // 暂时返回成功，但不真正执行
    0
}

/// wait4 - 等待进程状态改变
fn sys_wait4(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let _wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    println!("sys_wait4: pid={}, options={}", pid, options);
    -38_i64 as u64  // ENOSYS
}

/// uname - 获取系统信息
fn sys_uname(args: [u64; 6]) -> u64 {
    println!("sys_uname: not fully implemented");
    -38_i64 as u64  // ENOSYS
}

/// gettimeofday - 获取系统时间
fn sys_gettimeofday(args: [u64; 6]) -> u64 {
    println!("sys_gettimeofday: not implemented");
    -38_i64 as u64  // ENOSYS
}

/// clock_gettime - 获取指定时钟的时间
fn sys_clock_gettime(args: [u64; 6]) -> u64 {
    println!("sys_clock_gettime: not implemented");
    -38_i64 as u64  // ENOSYS
}

/// dup - 复制文件描述符
fn sys_dup(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;
    println!("sys_dup: oldfd={}", oldfd);
    -24_i64 as u64  // EMFILE
}

/// dup2 - 复制文件描述符到指定位置
fn sys_dup2(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;
    println!("sys_dup2: oldfd={}, newfd={}", oldfd, newfd);
    -24_i64 as u64  // EMFILE
}

/// fcntl - 文件控制操作
fn sys_fcntl(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let cmd = args[1] as i32;
    let arg = args[2] as i64;
    println!("sys_fcntl: fd={}, cmd={}, arg={}", fd, cmd, arg);
    -38_i64 as u64  // ENOSYS
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 获取当前系统调用号 (从 a7 寄存器)
#[inline]
pub fn get_syscall_no() -> u64 {
    let no: u64;
    unsafe {
        asm!("mv {}, a7", out(reg) no, options(nomem, nostack, pure));
    }
    no
}

/// 设置系统调用返回值
#[inline]
pub unsafe fn set_syscall_ret(val: u64) {
    asm!("mv a0, {}", in(reg) val, options(nomem, nostack));
}

/// 用户空间地址范围
///
/// RISC-V 64-bit 用户空间地址范围（标准配置）
/// 用户空间：0x0000_0000_0000_0000 ~ 0x0000_ffff_ffff_ffff
/// 内核空间：0xffff_0000_0000_0000 ~ 0xffff_ffff_ffff_ffff
const USER_SPACE_END: u64 = 0x0000_ffff_ffff_ffff;

/// 验证用户空间指针
#[inline]
pub unsafe fn verify_user_ptr(ptr: u64) -> bool {
    ptr <= USER_SPACE_END
}

/// 验证用户空间指针数组
pub unsafe fn verify_user_ptr_array(ptr: u64, size: usize) -> bool {
    if ptr > USER_SPACE_END {
        return false;
    }

    match ptr.checked_add(size as u64) {
        Some(end) if end <= USER_SPACE_END => true,
        _ => false,
    }
}
