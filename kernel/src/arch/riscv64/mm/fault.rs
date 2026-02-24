//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 页故障处理
//!
//! 参考 Linux 内核 arch/riscv/mm/fault.c
//!
//! 处理流程：
//! 1. 区分内核/用户模式
//! 2. 检查中断上下文
//! 3. 查找 VMA
//! 4. 验证权限
//! 5. 处理 COW
//! 6. 处理匿名页
//! 7. 发送信号或 OOM
//!
//! # 异常表机制
//!
//! 异常表用于安全地处理内核访问用户空间时可能发生的异常。
//! 典型用例：
//! - `copy_to_user()`: 将数据从内核复制到用户空间
//! - `copy_from_user()`: 将数据从用户空间复制到内核
//! - `get_user()`: 从用户空间读取单个值
//! - `put_user()`: 向用户空间写入单个值
//!
//! 当这些操作访问无效的用户地址时，会触发页故障。
//! 异常表记录了每个可能失败的访问指令及其修复处理程序。
//! 如果页故障发生在这些指令上，内核会跳转到修复处理程序，
//! 而不是崩溃。
//!
//! 参考 Linux: arch/riscv/include/asm/asm-extable.h

use crate::arch::riscv64::pt_regs::PtRegs;
use crate::arch::riscv64::mm::{VirtAddr, FaultFlags, AddressSpace, handle_cow_fault, handle_mm_fault};
use crate::println;
use crate::process::task::TaskState;

/// 页故障处理结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// 处理成功，可以重试指令
    Handled,
    /// 地址不在任何 VMA 中（段错误）
    Segfault,
    /// 权限不足（保护错误）
    PermissionDenied,
    /// 内存不足
    OutOfMemory,
    /// 已经映射（不需要处理）
    AlreadyMapped,
    /// COW 处理中（由 handle_cow_fault 处理）
    CowPending,
    /// 内核异常已修复（通过异常表）
    Fixed,
    /// 无法修复的内核异常
    KernelPanic,
}

/// 异常表项
///
/// 用于内核访问用户空间时的异常修复。
/// 当内核在指定地址发生异常时，跳转到修复地址继续执行。
///
/// # 内存布局
/// 每个条目占用 16 字节（2 × 8 字节地址）
///
/// # 参考
/// Linux: arch/riscv/include/asm/asm-extable.h
#[repr(C)]
pub struct ExceptionTableEntry {
    /// 可能发生异常的指令地址（PC 值）
    pub insn: u64,
    /// 修复后的跳转地址（处理异常后继续执行的位置）
    pub fixup: u64,
}

/// 异常表边界符号（由链接器脚本定义）
extern "C" {
    /// 异常表起始地址
    static __ex_table_start: ExceptionTableEntry;
    /// 异常表结束地址
    static __ex_table_end: ExceptionTableEntry;
}

/// 查找异常表中的修复地址
///
/// 使用线性搜索在异常表中查找匹配的指令地址。
/// 如果找到，返回修复地址；否则返回 None。
///
/// # 参数
/// - `addr`: 发生异常的指令地址（通常是 EPC 值）
///
/// # 返回
/// - `Some(fixup_addr)`: 找到修复地址
/// - `None`: 未找到匹配条目
///
/// # 性能
/// 线性搜索 O(n)，但异常表通常很小（几十到几百条），
/// 对性能影响可接受。如需优化可改用二分查找（需要表排序）。
///
/// # 参考
/// Linux: kernel/extable.c: search_exception_tables()
pub fn fixup_exception(addr: u64) -> Option<u64> {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;

        // 计算表中的条目数量
        let count = (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>();

        // 线性搜索
        for i in 0..count {
            let entry = &*start.add(i);
            if entry.insn == addr {
                return Some(entry.fixup);
            }
        }
    }

    None
}

/// 检查异常表是否为空
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        start == end
    }
}

/// 获取异常表条目数量
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>()
    }
}

/// 发送信号给当前进程
///
/// # 参数
/// - `sig`: 信号编号
/// - `code`: 信号代码（SI_XXX）
/// - `addr`: 触发异常的地址
fn send_signal(sig: i32, _code: i32, addr: u64) {
    // TODO: 实现完整的信号发送机制
    // 目前简化处理：终止进程
    if let Some(current) = crate::sched::current() {
        println!("do_page_fault: Sending signal {} to PID {} at addr={:#x}",
                 sig, current.pid(), addr);
        // 设置进程为僵尸状态
        current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
    }
}

/// 检查是否在中断上下文中
#[inline]
fn in_interrupt() -> bool {
    // TODO: 实现中断上下文检测
    // 目前简化为 false
    false
}

/// 页面错误处理 - bad_area 路径
///
/// 当地址不在有效的 VMA 中时调用
fn bad_area(regs: &mut PtRegs, access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    // 用户模式访问无效地址
    if regs.user_mode() {
        // 发送 SIGSEGV
        let sig = if access_type & FaultFlags::WRITE != 0 {
            11  // SIGSEGV
        } else if access_type & FaultFlags::EXEC != 0 {
            11  // SIGSEGV
        } else {
            11  // SIGSEGV
        };

        send_signal(sig, 1, fault_addr.bits());  // SEGV_MAPERR = 1
        return MmFaultResult::Segfault;
    }

    // 内核模式访问无效地址
    // 检查异常表
    if let Some(fixup) = fixup_exception(regs.epc) {
        regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // 无法修复，内核恐慌
    println!("do_page_fault: Kernel mode access to invalid address {:#x}, epc={:#x}",
             fault_addr.bits(), regs.epc);
    MmFaultResult::KernelPanic
}

/// 页面错误处理 - no_context 路径
///
/// 当无法获取有效的进程上下文时调用
fn no_context(regs: &mut PtRegs, fault_addr: VirtAddr) -> MmFaultResult {
    // 检查异常表
    if let Some(fixup) = fixup_exception(regs.epc) {
        regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // 无法处理
    println!("do_page_fault: No context for fault at {:#x}, epc={:#x}",
             fault_addr.bits(), regs.epc);
    MmFaultResult::KernelPanic
}

/// do_page_fault - 页故障处理主函数
///
/// 参考 Linux: arch/riscv/mm/fault.c: do_page_fault()
///
/// # 参数
/// - `regs`: 陷阱帧/寄存器状态
/// - `access_type`: 访问类型 (FaultFlags)
///
/// # 返回
/// 处理结果
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    let fault_addr = VirtAddr::new(regs.badaddr);

    // 获取当前进程的地址空间
    let current = match crate::sched::current() {
        Some(t) => t,
        None => {
            // 没有当前进程，可能是早期启动阶段
            return no_context(regs, fault_addr);
        }
    };

    let addr_space = match current.address_space() {
        Some(aspace) => aspace,
        None => {
            // 内核线程没有地址空间
            return no_context(regs, fault_addr);
        }
    };

    // 检查是否在中断上下文中
    if in_interrupt() {
        // 中断上下文中不能睡眠
        return no_context(regs, fault_addr);
    }

    // 内核模式访问
    if regs.kernel_mode() {
        // 检查异常表（copy_to_user/copy_from_user 等情况）
        if let Some(fixup) = fixup_exception(regs.epc) {
            regs.epc = fixup;
            return MmFaultResult::Fixed;
        }

        // 内核访问了无效地址（可能是 bug）
        println!("do_page_fault: Kernel page fault at {:#x}, epc={:#x}",
                 fault_addr.bits(), regs.epc);
        return MmFaultResult::KernelPanic;
    }

    // 用户模式页错误处理

    // 1. 调用 handle_mm_fault 处理
    let result = handle_mm_fault(&addr_space, fault_addr, access_type | FaultFlags::USER);

    match result {
        crate::arch::riscv64::mm::MmFaultResult::Handled => {
            // 页面已映射，可以重新执行指令
            return MmFaultResult::Handled;
        }
        crate::arch::riscv64::mm::MmFaultResult::CowPending => {
            // COW 页面，尝试写时复制
            match unsafe { handle_cow_fault(addr_space.root_ppn(), fault_addr) } {
                Some(()) => {
                    return MmFaultResult::Handled;
                }
                None => {
                    // COW 失败，可能是内存不足
                    println!("do_page_fault: COW failed at {:#x}", fault_addr.bits());
                    return MmFaultResult::OutOfMemory;
                }
            }
        }
        crate::arch::riscv64::mm::MmFaultResult::AlreadyMapped => {
            // 已映射但权限问题
            // 可能是写只读页等
            send_signal(11, 2, fault_addr.bits());  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::riscv64::mm::MmFaultResult::Segfault => {
            // 地址不在任何 VMA 中
            return bad_area(regs, access_type, fault_addr);
        }
        crate::arch::riscv64::mm::MmFaultResult::PermissionDenied => {
            // 权限不足
            send_signal(11, 2, fault_addr.bits());  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::riscv64::mm::MmFaultResult::OutOfMemory => {
            // 内存不足，发送 SIGKILL
            println!("do_page_fault: Out of memory at {:#x}", fault_addr.bits());
            send_signal(9, 0, fault_addr.bits());  // SIGKILL
            return MmFaultResult::OutOfMemory;
        }
    }
}
