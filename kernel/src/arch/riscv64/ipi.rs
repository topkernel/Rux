//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V IPI (Inter-Processor Interrupt) 支持
//!
//! 对应 Linux 的 arch/riscv/kernel/smp.c:
//! - smp_cross_call() - 发送跨 CPU 调用
//! - handle_IPI() - 处理 IPI
//!
//! IPI 类型：
//! - RESCHEDULE: 通知目标 CPU 重新调度（当有新任务或负载均衡时）
//! - STOP: 停止目标 CPU
//!
//! 使用 RISC-V 软件中断（SSIP）和 SBI IPI Extension (EID #0x735049)

use crate::sbi;
use crate::println;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum IpiType {
    /// 重新调度
    Reschedule = 0,
    /// 停止 CPU
    Stop = 1,
}

/// 发送 Reschedule IPI 到指定 CPU
///
/// 当某个 CPU 有新任务加入或需要负载均衡时，
/// 发送此 IPI 通知目标 CPU 重新调度
///
/// 对应 Linux 的 arch/riscv/kernel/smp.c:smp_cross_call()
///
/// # 参数
/// * `target_cpu` - 目标 CPU ID
pub fn send_reschedule_ipi(target_cpu: usize) {
    if target_cpu >= 4 {
        return;
    }

    // 不要发送给自己
    let current_cpu = crate::arch::cpu_id() as usize;
    if target_cpu == current_cpu {
        return;
    }

    // 通过 SBI 发送 IPI
    if sbi::send_ipi(target_cpu) {
        // 成功发送 IPI（静默，避免过多输出）
        // println!("ipi: Sent reschedule IPI to CPU {}", target_cpu);
    } else {
        println!("ipi: Failed to send reschedule IPI to CPU {}", target_cpu);
    }
}

/// 处理软件中断 IPI
///
/// 当接收到软件中断时调用此函数
/// 通知调度器重新调度
///
/// 对应 Linux 内核的 sched_IPI() + resched_curr()
///
/// # 参数
/// * `hart` - 当前 hart ID
pub fn handle_software_ipi(hart: usize) {
    // 处理 IPI - 触发调度器
    // 当其他 CPU 发送 Reschedule IPI 时，表示需要触发调度
    // 例如：唤醒了高优先级任务、需要负载均衡等

    #[cfg(feature = "riscv64")]
    {
        // 设置需要重新调度标志
        crate::sched::set_need_resched();

        // 立即调度
        crate::sched::schedule();
    }

    // println!("ipi: Hart {} received reschedule IPI", hart);
}

/// 处理 PLIC IPI（旧版，用于兼容）
///
/// # 参数
/// * `irq` - 中断号
/// * `hart` - 当前 hart ID
pub fn handle_ipi(irq: usize, hart: usize) {
    // 从中断号获取 IPI 类型
    match irq {
        10 => {
            // Reschedule IPI
            handle_software_ipi(hart);
        }
        11 => {
            // Stop IPI
            println!("ipi: Hart {} received stop IPI (IRQ 11), halting...", hart);
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        _ => {
            println!("ipi: Unknown IPI interrupt {} on hart {}", irq, hart);
        }
    }
}

/// 初始化 IPI 支持
///
/// 使能软件中断（SSIP）
pub fn init() {
    println!("ipi: Initializing RISC-V IPI support...");

    // 使能软件中断
    unsafe {
        // 设置 sie 寄存器的 SSIE 位 (bit 1)
        core::arch::asm!(
            "csrsi sie, 2",  // 设置 bit 1 (SSIE = 0x2)
            options(nomem, nostack)
        );
    }

    println!("ipi: IPI support initialized (using SBI IPI Extension)");
}
