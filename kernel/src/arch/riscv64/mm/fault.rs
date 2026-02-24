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

use crate::arch::riscv64::pt_regs::PtRegs;
use crate::arch::riscv64::mm::{VirtAddr, FaultFlags, AddressSpace, handle_cow_fault, handle_mm_fault};
use crate::println;

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
/// 用于内核访问用户空间时的异常修复
/// 当内核在指定地址发生异常时，跳转到修复地址继续执行
///
/// 注意：异常表功能尚未完全实现，目前仅作为数据结构定义
#[allow(dead_code)]
#[repr(C)]
pub struct ExceptionTableEntry {
    /// 可能发生异常的指令地址
    pub insn: u64,
    /// 修复后的跳转地址
    pub fixup: u64,
}

/// 查找异常表中的修复地址
///
/// # 参数
/// - `addr`: 发生异常的指令地址
///
/// # 返回
/// 如果找到修复地址返回 Some(fixup_addr)，否则返回 None
///
/// 注意：目前异常表功能尚未完全实现，始终返回 None
pub fn fixup_exception(addr: u64) -> Option<u64> {
    // TODO: 实现异常表查找
    // 需要在链接器脚本中定义 __ex_table_start 和 __ex_table_end 符号
    // 并在汇编中使用 .pushsection .ex_table, "a" 添加条目
    let _ = addr;
    None
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
        current.set_state(crate::process::task::TaskState::Zombie);
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
