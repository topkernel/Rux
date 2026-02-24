//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 进程/线程管理架构相关函数
//!
//! 参考 Linux: arch/riscv/kernel/process.c
//!
//! 主要函数:
//! - `start_thread`: execve 启动新程序
//! - `copy_thread`: fork 复制线程状态
//! - `flush_thread`: 清理线程状态

use crate::arch::riscv64::pt_regs::{PtRegs, SR_PIE, SR_SPP, SR_SUM};
use crate::arch::riscv64::mm::VirtAddr;
use crate::process::task::Task;
use core::arch::asm;

/// 启动新用户程序
///
/// 参考 Linux: arch/riscv/kernel/process.c start_thread()
///
/// 设置用户进程的初始状态:
/// - PC 指向程序入口点
/// - SP 指向用户栈顶
/// - 清零其他通用寄存器
/// - 设置 sstatus (用户模式, 启用中断)
///
/// # 参数
/// - `regs`: 要修改的 PtRegs
/// - `pc`: 程序入口地址
/// - `sp`: 用户栈指针
///
/// # 示例
/// ```ignore
/// let mut regs = PtRegs::default();
/// start_thread(&mut regs, entry_point, stack_top);
/// // 现在 regs 可以用于从 trap 返回到用户程序
/// ```
#[inline]
pub fn start_thread(regs: &mut PtRegs, pc: u64, sp: u64) {
    // 设置 PC 和 SP
    regs.epc = pc;
    regs.sp = sp;

    // 清零参数寄存器（a0-a7）
    regs.a0 = 0;
    regs.a1 = 0;
    regs.a2 = 0;
    regs.a3 = 0;
    regs.a4 = 0;
    regs.a5 = 0;
    regs.a6 = 0;
    regs.a7 = 0;

    // 清零返回地址
    regs.ra = 0;

    // 设置 sstatus:
    // - SPP = 0: 返回用户模式
    // - SPIE = 1: 启用中断
    // - SUM = 1: 允许 S-mode 访问用户内存
    regs.status = SR_PIE | SR_SUM;

    // 清零 cause 和 badaddr
    regs.cause = 0;
    regs.badaddr = 0;

    // 设置 orig_a0 为 0
    regs.orig_a0 = 0;
}

/// 复制线程状态 (fork)
///
/// 参考 Linux: arch/riscv/kernel/process.c copy_thread()
///
/// 为子进程创建初始状态:
/// - 复制父进程的寄存器状态
/// - 设置子进程返回值为 0 (a0 = 0)
/// - 设置返回地址为 ret_from_fork
///
/// # 参数
/// - `child`: 子进程的任务结构体
/// - `parent_regs`: 父进程的 PtRegs
///
/// # 返回
/// 成功返回子进程的 PtRegs 指针，失败返回 None
///
/// # 注意
/// 此函数分配的内存由调用者负责释放
pub unsafe fn copy_thread(
    child: *mut Task,
    parent_regs: &PtRegs,
) -> Option<*mut PtRegs> {
    use alloc::alloc::{alloc, Layout};

    // 分配内存用于子进程的 PtRegs
    let pt_regs_size = core::mem::size_of::<PtRegs>();
    let layout = Layout::from_size_align(pt_regs_size, 16).ok()?;

    let mem_ptr = alloc(layout);
    if mem_ptr.is_null() {
        return None;
    }

    let child_regs = mem_ptr as *mut PtRegs;

    // 复制父进程的寄存器状态
    // 注意: epc + 4 跳过 ecall 指令
    core::ptr::write(child_regs, PtRegs {
        epc: parent_regs.epc + 4,     // 跳过 ecall 指令
        ra: parent_regs.ra,
        sp: parent_regs.sp,           // 用户栈指针
        gp: parent_regs.gp,           // 全局指针
        tp: parent_regs.tp,           // 线程指针 (TLS)
        t0: parent_regs.t0,
        t1: parent_regs.t1,
        t2: parent_regs.t2,
        s0: parent_regs.s0,
        s1: parent_regs.s1,
        a0: 0,                        // 子进程返回值为 0
        a1: parent_regs.a1,
        a2: parent_regs.a2,
        a3: parent_regs.a3,
        a4: parent_regs.a4,
        a5: parent_regs.a5,
        a6: parent_regs.a6,
        a7: parent_regs.a7,
        s2: parent_regs.s2,
        s3: parent_regs.s3,
        s4: parent_regs.s4,
        s5: parent_regs.s5,
        s6: parent_regs.s6,
        s7: parent_regs.s7,
        s8: parent_regs.s8,
        s9: parent_regs.s9,
        s10: parent_regs.s10,
        s11: parent_regs.s11,
        t3: parent_regs.t3,
        t4: parent_regs.t4,
        t5: parent_regs.t5,
        t6: parent_regs.t6,
        status: parent_regs.status,   // sstatus
        badaddr: parent_regs.badaddr, // stval
        cause: parent_regs.cause,     // scause
        orig_a0: 0,                   // 子进程 orig_a0 = 0
    });

    // 设置子进程的 fork 信息
    (*child).set_fork_child(child_regs);

    // 复制 CPU 上下文 (callee-saved registers)
    // 设置入口点为 ret_from_fork
    extern "C" {
        fn ret_from_fork();
    }

    let child_ctx = (*child).context_mut();
    // ra 将在 ret_from_fork 中从栈上恢复
    child_ctx.pc = ret_from_fork as u64;

    Some(child_regs)
}

/// 清理线程状态
///
/// 参考 Linux: arch/riscv/kernel/process.c flush_thread()
///
/// 在 execve 时清理旧线程的状态:
/// - 清空 FPU 状态
/// - 清空向量扩展状态
/// - 其他架构特定清理
///
/// # 注意
/// 目前是空实现，待添加 FPU/向量扩展支持后完善
#[inline]
pub fn flush_thread() {
    // TODO: 实现 FPU 状态清理
    // TODO: 实现向量扩展状态清理
}

/// 获取当前进程的 PtRegs
///
/// 参考 Linux: current_pt_regs()
///
/// 返回当前进程在 trap 入口时保存的寄存器状态
#[inline]
pub fn current_pt_regs() -> *const PtRegs {
    crate::arch::riscv64::trap::current_pt_regs()
}

/// 获取任务的 PtRegs
///
/// 参考 Linux: task_pt_regs(task)
///
/// # 参数
/// - `task`: 任务结构体指针
///
/// # 返回
/// 任务的 PtRegs 指针
///
/// # 注意
/// 对于正在运行的任务，应该使用 current_pt_regs()
/// 此函数主要用于获取被 fork 的子进程的 PtRegs
#[inline]
pub fn task_pt_regs(task: *const Task) -> *const PtRegs {
    unsafe {
        // Task 结构体的 fork_child 字段存储了 PtRegs 指针
        (*task).fork_pt_regs()
    }
}

/// 获取用户栈指针
///
/// 从 PtRegs 中提取用户栈指针
#[inline]
pub fn user_stack_pointer(regs: &PtRegs) -> u64 {
    regs.sp
}

/// 设置用户栈指针
///
/// 修改 PtRegs 中的用户栈指针
#[inline]
pub fn set_user_stack_pointer(regs: &mut PtRegs, sp: u64) {
    regs.sp = sp;
}

/// 获取指令指针
///
/// 从 PtRegs 中提取程序计数器
#[inline]
pub fn instruction_pointer(regs: &PtRegs) -> u64 {
    regs.epc
}

/// 设置指令指针
///
/// 修改 PtRegs 中的程序计数器
#[inline]
pub fn set_instruction_pointer(regs: &mut PtRegs, pc: u64) {
    regs.epc = pc;
}

/// 检查地址是否在用户空间
///
/// RISC-V Sv39: 用户空间地址 0x0000_0000 - 0x003F_FFFF_FFFF
///
/// # 参数
/// - `addr`: 要检查的地址
///
/// # 返回
/// 如果在用户空间返回 true，否则返回 false
#[inline]
pub fn is_user_address(addr: u64) -> bool {
    // Sv39: 用户地址的高 25 位必须全为 0 或全为 1
    // 用户空间: 0x0000_0000_0000_0000 - 0x0000_003F_FFFF_FFFF
    let addr_virt = VirtAddr::new(addr);
    addr_virt.bits() < 0x0040_0000_0000
}

/// 读取用户空间数据
///
/// 安全地从用户空间读取数据，如果访问失败返回错误
///
/// # 参数
/// - `to`: 目标缓冲区（内核空间）
/// - `from`: 源地址（用户空间）
/// - `count`: 读取字节数
///
/// # 返回
/// 成功返回 0，失败返回未复制的字节数（正数）或负的错误码
///
/// # 参考
/// Linux: _copy_from_user()
pub unsafe fn copy_from_user(
    to: *mut u8,
    from: *const u8,
    count: usize,
) -> isize {
    // 使用 uaccess 模块的异常表版本
    let uncopied = super::uaccess::copy_from_user(to, from, count);
    uncopied as isize
}

/// 写入用户空间数据
///
/// 安全地向用户空间写入数据，如果访问失败返回错误
///
/// # 参数
/// - `to`: 目标地址（用户空间）
/// - `from`: 源数据（内核空间）
/// - `count`: 写入字节数
///
/// # 返回
/// 成功返回 0，失败返回未写入的字节数（正数）或负的错误码
///
/// # 参考
/// Linux: _copy_to_user()
pub unsafe fn copy_to_user(
    to: *mut u8,
    from: *const u8,
    count: usize,
) -> isize {
    // 使用 uaccess 模块的异常表版本
    let uncopied = super::uaccess::copy_to_user(to, from, count);
    uncopied as isize
}
