//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 异常处理
//!
//! 处理各种异常和中断，与 Linux 内核兼容

use core::arch::asm;

#[cfg(feature = "riscv64")]
use riscv::register::sie;

// 包含 trap.S 汇编代码
#[cfg(feature = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

// 重导出 PtRegs 和相关常量
pub use super::pt_regs::{PtRegs, Cause, PT_REGS_SIZE};
pub use super::pt_regs::{SR_SPP, SR_PIE, SR_SIE, SR_SUM};

/// 当前 CPU 的 PtRegs 指针（用于 fork）
static CURRENT_PT_REGS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// 获取当前的 PtRegs 指针
/// 用于 fork 复制父进程的 trap 状态
pub fn current_pt_regs() -> *const PtRegs {
    CURRENT_PT_REGS.load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// 初始化 trap 处理
pub fn init() {
    unsafe {
        // 设置 stvec 指向 trap_entry
        extern "C" {
            fn trap_entry();
        }

        let stvec_value = trap_entry as *const () as u64;
        asm!(
            "csrw stvec, {}",
            in(reg) stvec_value,
            options(nostack)
        );

        // 初始化 sscratch 为 hart_id + 1
        let hart_id: u64;
        asm!(
            "mv {}, tp",
            out(reg) hart_id,
            options(nomem, nostack, pure)
        );
        let sscratch_value = hart_id + 1;

        asm!(
            "csrw sscratch, {}",
            in(reg) sscratch_value,
            options(nomem, nostack)
        )
    }
}

pub fn init_syscall() {
    // RISC-V 使用 ecall 指令，在异常处理中分发
}

pub fn enable_timer_interrupt() {
    unsafe {
        asm!(
            "li t0, 32",           // STIE 位 (2^5)
            "csrw sie, t0",
            options(nomem, nostack)
        );

        // 设置 SIE 和 SUM 位
        asm!(
            "csrsi sstatus, 2",      // SIE = 0x2
            "li t0, 262144",         // SUM = 0x40000
            "csrs sstatus, t0",
            options(nomem, nostack)
        );
    }
}

pub fn disable_timer_interrupt() {
    unsafe {
        sie::clear_stimer();
    }
}

pub fn enable_external_interrupt() {
    unsafe {
        asm!(
            "li t0, 512",          // SEIE 位 (2^9)
            "csrw sie, t0",
            options(nomem, nostack)
        );

        asm!(
            "csrsi sstatus, 2",
            "li t0, 262144",
            "csrs sstatus, t0",
            options(nomem, nostack)
        );
    }
}

/// Trap 处理函数
///
/// 由 trap.S 调用，传入 PtRegs 指针
#[no_mangle]
pub extern "C" fn trap_handler(regs: *mut PtRegs) {
    unsafe {
        // 保存当前 PtRegs 指针（用于 fork）
        CURRENT_PT_REGS.store(regs as u64, core::sync::atomic::Ordering::Relaxed);

        let regs_ref = &mut *regs;
        let cause = Cause::from_cause(regs_ref.cause);

        // 调试输出（可选，排除定时器中断）
        // if !matches!(cause, Cause::SupervisorTimer) {
        //     crate::println!("TRAP: {:?} epc={:#x} badaddr={:#x}",
        //         cause, regs_ref.epc, regs_ref.badaddr);
        // }

        match cause {
            // 定时器中断
            Cause::SupervisorTimer => {
                handle_timer_interrupt(regs_ref);
            }

            // 软件中断 (IPI)
            Cause::SupervisorSoft => {
                handle_software_interrupt(regs_ref);
            }

            // 外部中断
            Cause::SupervisorExternal => {
                handle_external_interrupt(regs_ref);
            }

            // 用户态系统调用
            Cause::EcallUser => {
                handle_syscall(regs_ref);
            }

            // 非法指令
            Cause::IllegalInstruction => {
                if regs_ref.user_mode() {
                    handle_illegal_instruction(regs_ref);
                } else {
                    crate::println!("trap: Illegal instruction in kernel at epc={:#x}",
                        regs_ref.epc);
                    regs_ref.epc += 4;
                }
            }

            // 断点
            Cause::Breakpoint => {
                handle_breakpoint(regs_ref);
            }

            // 指令页错误
            Cause::InstructionPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::EXEC);
            }

            // 加载页错误
            Cause::LoadPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::READ);
            }

            // 存储页错误
            Cause::StoreAmoPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::WRITE);
            }

            // 其他异常
            _ => {
                handle_unknown_exception(regs_ref, cause);
            }
        }

        // 清除当前 PtRegs 指针
        CURRENT_PT_REGS.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// 处理定时器中断
fn handle_timer_interrupt(regs: &mut PtRegs) {
    // 1. 更新 jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 2. 调度器 tick
    #[cfg(feature = "riscv64")]
    crate::sched::scheduler_tick();

    // 3. 设置下一次定时器中断
    crate::drivers::timer::set_next_trigger();

    // 4. 如果需要重新调度
    #[cfg(feature = "riscv64")]
    if crate::sched::need_resched() {
        // 保存当前状态并调度
        // 注意：调度会修改 regs，返回时会恢复新进程的状态
        crate::sched::schedule();
    }
}

/// 处理软件中断 (IPI)
fn handle_software_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    // 清除软件中断
    unsafe {
        core::arch::asm!("csrc sip, 0x2", options(nomem, nostack));
    }

    // 处理 IPI
    crate::arch::ipi::handle_software_ipi(hart_id as usize);
}

/// 处理外部中断
fn handle_external_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    if let Some(irq) = crate::drivers::intc::plic::claim(hart_id as usize) {
        match irq {
            1..=8 => {
                // VirtIO MMIO 设备中断
                crate::drivers::virtio::interrupt_handler();
            }
            32..=127 => {
                // VirtIO PCI 设备中断
                crate::drivers::virtio::interrupt_handler_pci(irq as usize);
            }
            10 => {
                // UART 中断
            }
            11..=13 => {
                // IPI 中断
                crate::arch::ipi::handle_ipi(irq, hart_id as usize);
            }
            _ => {
                // 未知中断
            }
        }

        crate::drivers::intc::plic::complete(hart_id as usize, irq);
    }
}

/// 处理系统调用
fn handle_syscall(regs: &mut PtRegs) {
    // 保存 orig_a0（在 trap.S 中已经完成，这里确保一下）
    // regs.orig_a0 已经在汇编中设置

    // 默认返回值为 -ENOSYS
    regs.a0 = crate::errno::constants::ENOSYS as u64;

    // 跳过 ecall 指令
    regs.epc += 4;

    // 调用系统调用处理
    crate::arch::riscv64::syscall::syscall_handler(regs);
}

/// 处理非法指令
fn handle_illegal_instruction(regs: &mut PtRegs) {
    crate::println!("trap: Illegal instruction at epc={:#x} (user mode)", regs.epc);

    // 发送 SIGILL 或终止进程
    if let Some(current) = crate::sched::current() {
        crate::println!("trap: Terminating process PID {}", current.pid());
        current.set_state(crate::process::task::TaskState::Zombie);
        crate::sched::schedule();
    }

    regs.epc += 4;
}

/// 处理断点
fn handle_breakpoint(regs: &mut PtRegs) {
    if regs.user_mode() {
        crate::println!("trap: Breakpoint at epc={:#x} (user mode)", regs.epc);

        // 发送 SIGTRAP 或终止进程
        if let Some(current) = crate::sched::current() {
            crate::println!("trap: Terminating process PID {}", current.pid());
            current.set_state(crate::process::task::TaskState::Zombie);
            crate::sched::schedule();
        }
    } else {
        crate::println!("trap: Breakpoint at epc={:#x} (kernel mode)", regs.epc);
    }

    regs.epc += 4;
}

/// 处理页错误
fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
    use crate::arch::riscv64::mm::{FaultFlags, MmFaultResult, handle_mm_fault, handle_cow_fault, VirtAddr};

    let fault_addr = VirtAddr::new(regs.badaddr);

    // 内核模式页错误
    if regs.kernel_mode() {
        // TODO: 实现 fixup_exception
        crate::println!("trap: Page fault in kernel at {:#x}, epc={:#x}",
            fault_addr.bits(), regs.epc);
        // 暂时不跳过，让内核崩溃以便调试
        return;
    }

    // 用户模式页错误
    if let Some(current) = crate::sched::current() {
        if let Some(addr_space) = current.address_space() {
            let mut flags = access_type | FaultFlags::USER;

            let result = handle_mm_fault(&addr_space, fault_addr, flags);

            match result {
                MmFaultResult::Handled => {
                    // 页面已映射，重新执行指令
                    return;
                }
                MmFaultResult::CowPending => {
                    // COW 页面，尝试写时复制
                    // handle_cow_fault 是 unsafe 函数
                    match unsafe { handle_cow_fault(addr_space.root_ppn(), fault_addr) } {
                        Some(()) => {
                            // COW 成功，重新执行指令
                            return;
                        }
                        None => {
                            crate::println!("trap: COW failed at {:#x}", fault_addr.bits());
                        }
                    }
                }
                MmFaultResult::AlreadyMapped => {
                    // 已映射但权限问题
                    crate::println!("trap: Access denied at {:#x}", fault_addr.bits());
                }
                MmFaultResult::Segfault => {
                    crate::println!("trap: Segfault at {:#x}, epc={:#x}",
                        fault_addr.bits(), regs.epc);
                }
                MmFaultResult::PermissionDenied => {
                    crate::println!("trap: Permission denied at {:#x}", fault_addr.bits());
                }
                MmFaultResult::OutOfMemory => {
                    crate::println!("trap: Out of memory at {:#x}", fault_addr.bits());
                }
            }
        }
    }

    // 无法处理，终止进程
    crate::println!("trap: Terminating process due to unhandled page fault");
    if let Some(current) = crate::sched::current() {
        current.set_state(crate::process::task::TaskState::Zombie);
        crate::sched::schedule();
    }
}

/// 处理未知异常
fn handle_unknown_exception(regs: &mut PtRegs, cause: Cause) {
    crate::println!("trap: Unknown exception: {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);

    if regs.user_mode() {
        // 终止用户进程
        if let Some(current) = crate::sched::current() {
            current.set_state(crate::process::task::TaskState::Zombie);
            crate::sched::schedule();
        }
    }

    // 跳过指令
    regs.epc += 4;
}

// ============================================================================
// 兼容性：保留旧的 TrapFrame 类型别名
// ============================================================================

/// 旧的 TrapFrame 类型别名（兼容性）
pub type TrapFrame = PtRegs;

/// 旧的 ExceptionCause 类型别名（兼容性）
pub type ExceptionCause = Cause;

/// 获取当前的 TrapFrame 指针（兼容性）
#[deprecated(note = "Use current_pt_regs instead")]
pub fn current_trap_frame() -> *const TrapFrame {
    current_pt_regs()
}
