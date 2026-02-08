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
///
/// 对应 Linux 的 sys_pipe() (fs/pipe.c)
///
/// pipe() 创建一个管道，一个单工数据通道，可用于进程间通信。
///
/// # 参数
/// * pipefd[2] - 返回两个文件描述符
///   - pipefd[0]: 读端
///   - pipefd[1]: 写端
///
/// # 返回
/// * 0 表示成功，-1 表示失败（errno 设置为错误码）
fn sys_pipe(args: [u64; 6]) -> u64 {
    let pipefd_ptr = args[0] as *mut i32;

    // 检查指针有效性（简化检查，只检查是否为 null）
    if pipefd_ptr.is_null() {
        println!("sys_pipe: pipefd is null");
        return -14_i64 as u64;  // EFAULT
    }

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => {
            println!("sys_pipe: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
    };

    // 创建管道
    let (read_file, write_file) = crate::fs::create_pipe();

    let read_file = match read_file {
        Some(f) => f,
        None => {
            println!("sys_pipe: failed to create read file");
            return -12_i64 as u64;  // ENOMEM
        }
    };

    let write_file = match write_file {
        Some(f) => f,
        None => {
            println!("sys_pipe: failed to create write file");
            return -12_i64 as u64;  // ENOMEM
        }
    };

    // 分配文件描述符
    let read_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_pipe: failed to alloc read fd");
            return -24_i64 as u64;  // EMFILE - 进程打开文件数过多
        }
    };

    let write_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_pipe: failed to alloc write fd");
            // 释放已分配的读端（直接关闭文件描述符）
            let _ = fdtable.close_fd(read_fd);
            return -24_i64 as u64;  // EMFILE
        }
    };

    // 安装文件到 fdtable
    if fdtable.install_fd(read_fd, read_file).is_err() {
        println!("sys_pipe: failed to install read fd");
        let _ = fdtable.close_fd(read_fd);
        let _ = fdtable.close_fd(write_fd);
        return -9_i64 as u64;  // EBADF
    }

    if fdtable.install_fd(write_fd, write_file).is_err() {
        println!("sys_pipe: failed to install write fd");
        let _ = fdtable.close_fd(read_fd);
        let _ = fdtable.close_fd(write_fd);
        return -9_i64 as u64;  // EBADF
    }

    // 将文件描述符写入用户空间
    unsafe {
        *pipefd_ptr.add(0) = read_fd as i32;
        *pipefd_ptr.add(1) = write_fd as i32;
    }

    println!("sys_pipe: created pipe, read_fd={}, write_fd={}", read_fd, write_fd);

    0  // 成功
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
    crate::sched::do_exit(exit_code);
}

/// kill - 向进程发送信号
fn sys_kill(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    println!("sys_kill: pid={}, sig={}", pid, sig);

    match crate::sched::send_signal(pid as u32, sig) {
        Ok(()) => 0,
        Err(e) => e as u32 as u64,
    }
}

/// fork - 创建子进程
fn sys_fork(_args: [u64; 6]) -> u64 {
    println!("sys_fork: creating new process");

    match crate::sched::do_fork() {
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
pub fn sys_execve(args: [u64; 6]) -> u64 {
    use crate::fs::elf::ElfLoader;
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
    let ehdr = ElfLoader::get_entry(&file_data);
    let entry = match ehdr {
        Ok(addr) => addr,
        Err(e) => {
            println!("sys_execve: failed to get entry: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: ELF entry point = {:#x}", entry);

    // ===== 5. 获取程序头数量 =====
    let phdr_count = match ElfLoader::get_program_headers(&file_data) {
        Ok(count) => count,
        Err(e) => {
            println!("sys_execve: failed to get program headers: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: {} program headers", phdr_count);

    // 获取 ELF 头
    let ehdr = match unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(&file_data) } {
        Some(e) => e,
        None => {
            println!("sys_execve: failed to get ELF header");
            return -8_i64 as u64;
        }
    };

    // ===== 6. 分析 PT_LOAD 段 =====
    for i in 0..phdr_count {
        if let Some(phdr) = unsafe { ehdr.get_program_header(&file_data, i) } {
            if phdr.is_load() {
                println!("  PT_LOAD[{}]: vaddr={:#x}, filesz={}, memsz={}, flags={:#x}",
                         i, phdr.p_vaddr, phdr.p_filesz, phdr.p_memsz, phdr.p_flags);
            }
        }
    }

    // ===== 7. 检查 PT_INTERP（动态链接器） =====
    if let Some(interp) = ElfLoader::get_interpreter(&file_data) {
        let interp_str = core::str::from_utf8(interp).unwrap_or("<invalid>");
        println!("sys_execve: interpreter: {}", interp_str);
    }

    // ===== 8. 创建用户地址空间 =====
    use crate::arch::riscv64::mm::{
        create_user_address_space, alloc_and_map_user_memory,
        PageTableEntry, PAGE_SIZE
    };

    let user_root_ppn = match create_user_address_space() {
        Some(ppn) => {
            println!("sys_execve: created user address space (root_ppn={:#x})", ppn);
            ppn
        }
        None => {
            println!("sys_execve: failed to create user address space");
            return -12_i64 as u64;  // ENOMEM
        }
    };

    // ===== 9. 加载 PT_LOAD 段 =====
    for i in 0..phdr_count {
        if let Some(phdr) = unsafe { ehdr.get_program_header(&file_data, i) } {
            if phdr.is_load() {
                let vaddr = phdr.p_vaddr;
                let memsz = phdr.p_memsz as usize;
                let filesz = phdr.p_filesz as usize;
                let offset = phdr.p_offset as usize;

                // 页对齐
                let aligned_vaddr = vaddr & !(PAGE_SIZE as u64 - 1);
                let aligned_size = ((memsz as u64 + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1)) as usize;

                // 计算页标志
                let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
                if phdr.p_flags & crate::fs::elf::PF_R != 0 {
                    flags |= PageTableEntry::R;
                }
                if phdr.p_flags & crate::fs::elf::PF_W != 0 {
                    flags |= PageTableEntry::W;
                }
                if phdr.p_flags & crate::fs::elf::PF_X != 0 {
                    flags |= PageTableEntry::X;
                }
                // 用户可访问
                flags |= PageTableEntry::U;

                // 分配并映射内存
                let phys_addr = unsafe {
                    match alloc_and_map_user_memory(user_root_ppn, aligned_vaddr, aligned_size as u64, flags) {
                        Some(addr) => addr,
                        None => {
                            println!("sys_execve: failed to allocate memory for segment at {:#x}", vaddr);
                            return -12_i64 as u64;  // ENOMEM
                        }
                    }
                };

                // 复制 ELF 数据到物理内存
                unsafe {
                    let offset_in_segment = vaddr - aligned_vaddr;
                    let dst = (phys_addr + offset_in_segment) as *mut u8;
                    let src = file_data.as_ptr().add(offset);

                    if filesz > 0 {
                        core::ptr::copy_nonoverlapping(src, dst, filesz);
                    }

                    // BSS 段清零
                    if memsz > filesz {
                        let bss_start = dst.add(filesz);
                        let bss_size = memsz - filesz;
                        core::ptr::write_bytes(bss_start, 0, bss_size);
                    }
                }

                println!("sys_execve: loaded segment: vaddr={:#x}, memsz={}, phys={:#x}",
                         vaddr, memsz, phys_addr);
            }
        }
    }

    // ===== 10. 分配用户栈 =====
    const USER_STACK_SIZE: u64 = 8 * 1024 * 1024; // 8MB
    const USER_STACK_TOP: u64 = 0x0000_003f_ffff_f000u64;
    let user_stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;

    let stack_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W
        | PageTableEntry::A | PageTableEntry::D | PageTableEntry::U;

    let user_stack_phys = unsafe {
        match alloc_and_map_user_memory(user_root_ppn, user_stack_bottom, USER_STACK_SIZE, stack_flags) {
            Some(addr) => addr,
            None => {
                println!("sys_execve: failed to allocate user stack");
                return -12_i64 as u64;  // ENOMEM
            }
        }
    };

    println!("sys_execve: user stack: virt={:#x}, phys={:#x}", USER_STACK_TOP, user_stack_phys);

    // ===== 11. 设置 argv/envp 到用户栈 =====
    // Linux 栈布局（从高地址到低地址）：
    // | envp[n]     |
    // | ...         |
    // | envp[0]     |
    // | NULL        |  <- envp 数组结束
    // | argv[argc]  |  <- NULL
    // | argv[argc-1]|
    // | ...         |
    // | argv[0]     |
    // | argc        |  <- 栈指针指向这里

    let user_stack_with_args = match setup_user_stack(user_root_ppn, user_stack_phys, USER_STACK_TOP, args[1], args[2]) {
        Ok(sp) => sp,
        Err(e) => {
            println!("sys_execve: failed to setup user stack: {}", e);
            return -12_i64 as u64;  // ENOMEM
        }
    };

    println!("sys_execve: user stack with args: sp={:#x}", user_stack_with_args);

    // ===== 12. 切换到用户模式并执行 =====
    unsafe {
        switch_to_user(user_root_ppn, entry, user_stack_with_args);
    }

    // 不应该返回
    #[allow(unreachable_code)]
    {
        println!("sys_execve: unexpectedly returned from user mode");
        -1_i64 as u64
    }
}

/// 设置用户栈的 argv/envp
///
/// 对应 Linux 的 `setup_arg_page()` (fs/exec.c)
///
/// # 参数
/// - `user_root_ppn`: 用户页表的根 PPN
/// - `user_stack_phys`: 用户栈的物理地址
/// - `user_stack_top`: 用户栈顶的虚拟地址
/// - `argv`: argv 指针数组（用户空间）
/// - `envp`: envp 指针数组（用户空间）
///
/// # 返回
/// 成功返回新的栈指针（指向 argc），失败返回错误
fn setup_user_stack(
    _user_root_ppn: u64,
    user_stack_phys: u64,
    user_stack_top: u64,
    argv: u64,
    envp: u64,
) -> Result<u64, &'static str> {
    use alloc::vec::Vec;
    use core::slice;

    // ===== 1. 读取 argv 数组 =====
    let argv_ptr = argv as *const *const u8;
    let mut argv_strings: Vec<Vec<u8>> = Vec::new();

    if !argv_ptr.is_null() {
        unsafe {
            let mut i = 0;
            loop {
                let ptr = *argv_ptr.add(i);
                if ptr.is_null() {
                    break;
                }

                // 读取字符串
                let mut len = 0;
                let mut str_ptr = ptr;
                while len < 4096 {  // 最大长度限制
                    let byte = *str_ptr;
                    if byte == 0 {
                        break;
                    }
                    len += 1;
                    str_ptr = str_ptr.add(1);
                }

                let string_vec = slice::from_raw_parts(ptr, len).to_vec();
                argv_strings.push(string_vec);
                i += 1;

                if i >= 256 {  // 最多 256 个参数
                    break;
                }
            }
        }
    }

    let argc = argv_strings.len();

    // ===== 2. 读取 envp 数组 =====
    let envp_ptr = envp as *const *const u8;
    let mut envp_strings: Vec<Vec<u8>> = Vec::new();

    if !envp_ptr.is_null() {
        unsafe {
            let mut i = 0;
            loop {
                let ptr = *envp_ptr.add(i);
                if ptr.is_null() {
                    break;
                }

                // 读取字符串
                let mut len = 0;
                let mut str_ptr = ptr;
                while len < 4096 {
                    let byte = *str_ptr;
                    if byte == 0 {
                        break;
                    }
                    len += 1;
                    str_ptr = str_ptr.add(1);
                }

                let string_vec = slice::from_raw_parts(ptr, len).to_vec();
                envp_strings.push(string_vec);
                i += 1;

                if i >= 256 {  // 最多 256 个环境变量
                    break;
                }
            }
        }
    }

    println!("setup_user_stack: argc={}, envc={}", argc, envp_strings.len());

    // ===== 3. 计算需要的栈空间 =====
    // 栈布局（从高地址到低地址）：
    // | envp strings     |
    // | argv strings     |
    // | envp pointers    |
    // | NULL (envp 结束)  |
    // | NULL (argv[argc]) |
    // | argv pointers    |
    // | argc             |  <- SP

    let mut total_size = 0usize;

    // 环境变量字符串
    for s in &envp_strings {
        total_size += s.len() + 1;  // +1 for null terminator
    }
    // argv 字符串
    for s in &argv_strings {
        total_size += s.len() + 1;
    }

    // 指针对齐到 8 字节
    let ptr_size = 8;

    // envp 指针数组
    total_size += (envp_strings.len() + 1) * ptr_size;  // +1 for NULL

    // argv 指针数组
    total_size += (argc + 1) * ptr_size;  // +1 for NULL

    // argc
    total_size += 8;

    // 栈对齐到 16 字节
    total_size = (total_size + 15) & !15;

    println!("setup_user_stack: total stack size = {} bytes", total_size);

    // ===== 4. 在用户栈上布置数据 =====
    let mut current_vaddr = user_stack_top;
    let mut current_paddr = user_stack_phys;

    // 减去总大小
    current_vaddr -= total_size as u64;
    current_paddr -= total_size as u64;

    // 对齐栈指针
    current_vaddr &= !15;
    current_paddr &= !15;

    let _final_sp = current_vaddr;
    let mut offset = 0usize;

    // ===== 5. 写入字符串数据 =====
    // 首先写入所有环境变量字符串（在高地址）
    let mut envp_addrs: Vec<u64> = Vec::new();
    for s in &envp_strings {
        let str_vaddr = current_vaddr + offset as u64;
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u8;
            for (i, &byte) in s.iter().enumerate() {
                *dst.add(i) = byte;
            }
            *dst.add(s.len()) = 0;  // null terminator
        }
        envp_addrs.push(str_vaddr);
        offset += s.len() + 1;
    }

    // 然后 argv 字符串
    let mut argv_addrs: Vec<u64> = Vec::new();
    for s in &argv_strings {
        let str_vaddr = current_vaddr + offset as u64;
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u8;
            for (i, &byte) in s.iter().enumerate() {
                *dst.add(i) = byte;
            }
            *dst.add(s.len()) = 0;  // null terminator
        }
        argv_addrs.push(str_vaddr);
        offset += s.len() + 1;
    }

    // ===== 6. 写入指针数组 =====
    // 对齐到指针大小
    while offset % ptr_size != 0 {
        offset += 1;
    }

    // envp 指针数组
    for &addr in &envp_addrs {
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u64;
            *dst = addr;
        }
        offset += ptr_size;
    }
    // envp NULL 终止符
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = 0;
    }
    offset += ptr_size;

    // argv 指针数组（注意：需要倒序写入，因为栈从高地址向低地址增长）
    // 实际上我们不需要倒序，因为我们是从低地址向高地址构建的
    // 但是按照 Linux 的布局，argv[0] 应该在最低地址

    // 先写 argv NULL 终止符
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = 0;
    }
    offset += ptr_size;

    // 然后写 argv 指针（从后往前）
    for i in (0..argc).rev() {
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u64;
            *dst = argv_addrs[i];
        }
        offset += ptr_size;
    }

    // ===== 7. 写入 argc =====
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = argc as u64;
    }
    offset += 8;

    // 最终的栈指针应该在 argc 的位置
    let final_sp = current_vaddr + offset as u64 - 8;

    println!("setup_user_stack: final sp={:#x}, argc={}, argv={:#x}", final_sp, argc,
             if argc > 0 { argv_addrs[0] } else { 0 });

    Ok(final_sp)
}

/// 切换到用户模式
///
/// # 参数
/// - `user_root_ppn`: 用户页表的根 PPN
/// - `entry`: 用户程序入口点
/// - `user_stack`: 用户栈指针
unsafe fn switch_to_user(user_root_ppn: u64, entry: u64, user_stack: u64) -> ! {
    use crate::arch::riscv64::mm::Satp;

    // 保存当前内核栈
    let _kernel_stack: u64;
    core::arch::asm!("mv {}, sp", out(reg) _kernel_stack);

    // 设置用户页表
    let satp = Satp::sv39(user_root_ppn, 0);
    println!("sys_execve: switching to user mode, satp={:#x}, entry={:#x}, sp={:#x}",
             satp.0, entry, user_stack);

    // 设置用户模式下的寄存器状态
    // RISC-V User 模式:
    // - mstatus.MPP = 00 (U-mode)
    // - mstatus.MPIE = 1 (启用中断返回)
    // - mepc = entry point
    // - sp = user_stack

    core::arch::asm!(
        // 1. 设置用户栈
        "mv sp, {2}",

        // 2. 设置 mstatus (进入用户模式)
        // MPP = 00 (U-mode), MPIE = 1
        "li t0, 0x1880",  // MPP=0 (bits 12:11), MPIE=1 (bit 7), MIE=1 (bit 3)
        "csrw mstatus, t0",

        // 3. 设置 mepc (用户程序入口点)
        "csrw mepc, {0}",

        // 4. 设置 mtvec (内核陷阱向量，用于系统调用)
        // 这应该已经在 trap::init() 中设置好了

        // 5. 设置 satp (用户页表)
        "csrw satp, {1}",

        // 6. 刷新 TLB
        "sfence.vma zero, zero",

        // 7. mret - 返回到用户模式
        "mret",

        // 参数
        in(reg) entry,
        in(reg) satp.0,
        in(reg) user_stack,

        options(nostack, noreturn)
    );
}

/// wait4 - 等待进程状态改变
///
/// 对应 Linux 内核的 sys_wait4() (kernel/exit.c)
///
/// wait4() 挂起当前进程，直到指定的子进程状态改变：
/// - 子进程终止
/// - 子进程被信号停止
/// - 子进程被信号恢复
///
/// # 参数
/// * pid - 等待的进程 ID (-1 表示任意子进程)
/// * wstatus - 用于存储子进程退出状态
/// * options - 选项 (WNOHANG, WUNTRACED, WCONTINUED)
/// * rusage - 用于存储资源使用统计（当前未实现）
fn sys_wait4(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // 检查 wstatus 指针有效性（简化检查，只检查是否为 null）
    // 如果是 WNOHANG 且没有子进程退出，立即返回 0
    if options != 0 && (options & 0x01) != 0 {
        // WNOHANG: 如果没有子进程退出，立即返回
        match crate::sched::do_wait(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -10 => 0,  // EAGAIN -> 返回 0 表示没有子进程退出
            Err(e) => e as u32 as u64,
        }
    } else {
        // 阻塞等待子进程退出
        match crate::sched::do_wait(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) => e as u32 as u64,
        }
    }
}

/// uname - 获取系统信息
fn sys_uname(_args: [u64; 6]) -> u64 {
    println!("sys_uname: not fully implemented");
    -38_i64 as u64  // ENOSYS
}

/// gettimeofday - 获取系统时间
fn sys_gettimeofday(_args: [u64; 6]) -> u64 {
    println!("sys_gettimeofday: not implemented");
    -38_i64 as u64  // ENOSYS
}

/// clock_gettime - 获取指定时钟的时间
fn sys_clock_gettime(_args: [u64; 6]) -> u64 {
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

/// 系统调用的 write 实现（直接从 trap 调用）
///
/// 参数:
/// - fd: 文件描述符
/// - buf: 缓冲区指针
/// - count: 字节数
///
/// 返回: 实际写入的字节数，或错误码
pub fn sys_write_impl(fd: i32, buf: *const u8, count: usize) -> u64 {
    use crate::console::putchar;

    unsafe {
        // Special handling for stdout (1) and stderr (2) - write directly to UART
        if fd == 1 || fd == 2 {
            let slice = core::slice::from_raw_parts(buf, count);
            for &b in slice {
                putchar(b);
            }
            return count as u64;
        }

        // 其他文件描述符：使用 VFS
        use crate::fs::get_file_fd;
        match get_file_fd(fd as usize) {
            Some(_file) => {
                // TODO: 实现 VFS write
                crate::println!("sys_write: fd={}, count={} (VFS not implemented)", fd, count);
                -9_i32 as u64  // EBADF
            }
            None => {
                crate::println!("sys_write: invalid fd {}", fd);
                -9_i32 as u64  // EBADF
            }
        }
    }
}
