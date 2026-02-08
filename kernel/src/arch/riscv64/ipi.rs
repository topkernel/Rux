//! RISC-V IPI (Inter-Processor Interrupt) 支持
//!
//! 对应 Linux 的 arch/riscv/kernel/smp.c:
//! - smp_cross_call() - 发送跨 CPU 调用
//! - handle_IPI() - 处理 IPI
//!
//! 使用 SBI IPI Extension (EID #0x735049)
use crate::sbi;
use crate::println;

/// IPI 类型
///
/// 对应 Linux 的 enum ipi_msg_type
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum IpiType {
    /// 重新调度
    Reschedule = 0,
    /// 停止 CPU
    Stop = 1,
}

/// 发送 IPI 到指定 CPU（使用 SBI IPI Extension）
///
/// # 参数
/// * `target_hart` - 目标 hart ID
/// * `ipi_type` - IPI 类型
///
/// # 实现
/// 使用 SBI IPI Extension (EID #0x735049, FID #0)
/// 对应 Linux 的 arch/riscv/kernel/sbi.c:__sbi_send_ipi_v02
///
/// 参考 Linux 内核实现：
/// - arch/riscv/kernel/sbi-ipi.c
/// - arch/riscv/kernel/sbi.c
pub fn send_ipi(target_hart: usize, _ipi_type: IpiType) {
    if target_hart >= 4 {
        return;
    }

    // 通过 SBI 发送 IPI
    sbi::send_ipi(target_hart);
}

/// 处理软件中断（来自 SBI IPI）
///
/// # 参数
/// * `_hart` - 当前 hart ID
pub fn handle_software_ipi(_hart: usize) {
    // 处理 IPI - 触发调度器
    // 对应 Linux 内核的 sched_IPI() + resched_curr()
    //
    // 当其他 CPU 发送 Reschedule IPI 时，表示需要触发调度
    // 例如：唤醒了高优先级任务、需要负载均衡等
    #[cfg(feature = "riscv64")]
    crate::sched::schedule();
}

/// 处理 IPI 中断（来自 PLIC）
///
/// # 参数
/// * `irq` - 中断号
/// * `hart` - 当前 hart ID
pub fn handle_ipi(irq: usize, hart: usize) {
    // 从中断号获取 IPI 类型
    match irq {
        10 => {
            // Reschedule IPI
            println!("ipi: Hart {} received reschedule IPI (IRQ 10)", hart);
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

/// 初始化 IPI
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
