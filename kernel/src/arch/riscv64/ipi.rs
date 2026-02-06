//! RISC-V IPI (Inter-Processor Interrupt) 支持
//!
//! 对应 Linux 的 arch/riscv/kernel/smp.c:
//! - smp_cross_call() - 发送跨 CPU 调用
//! - handle_IPI() - 处理 IPI
//!
//! 使用 PLIC 实现
use crate::drivers::intc::plic;
use crate::println;
use core::sync::atomic::{AtomicU32, Ordering};

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

impl IpiType {
    /// 将 IPI 类型转换为 PLIC 中断号
    ///
    /// 在 PLIC 中，我们使用中断 10-13 作为 IPI
    /// - IRQ 10: Reschedule
    /// - IRQ 11: Stop
    /// - IRQ 12-13: 保留
    #[inline]
    pub const fn as_irq(self) -> usize {
        10 + self as usize
    }

    /// 从 PLIC 中断号创建 IPI 类型
    #[inline]
    pub const fn from_irq(irq: usize) -> Option<Self> {
        match irq {
            10 => Some(IpiType::Reschedule),
            11 => Some(IpiType::Stop),
            _ => None,
        }
    }
}

/// IPI 计数器（用于统计和调试）
static IPI_COUNT: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 发送 IPI 到指定 CPU
///
/// # 参数
/// * `target_hart` - 目标 hart ID
/// * `ipi_type` - IPI 类型
///
/// # 实现
/// RISC-V 的 IPI 通过 PLIC 实现
/// 注意：QEMU virt 可能不完全支持软件触发 IPI
pub fn send_ipi(target_hart: usize, ipi_type: IpiType) {
    if target_hart >= 4 {
        println!("ipi: Invalid target hart {}", target_hart);
        return;
    }

    let irq = ipi_type.as_irq();

    // TODO: 实现 PLIC IPI 发送
    // 标准的 RISC-V PLIC 不直接支持软件触发中断
    // 需要使用特定的方法或 SBI 扩展
    println!("ipi: Sending IPI to hart {}, irq={}", target_hart, irq);

    // 暂时只更新计数器
    IPI_COUNT[target_hart].fetch_add(1, Ordering::Relaxed);
}

/// 处理 IPI 中断
///
/// # 参数
/// * `irq` - 中断号
/// * `hart` - 当前 hart ID
pub fn handle_ipi(irq: usize, hart: usize) {
    // 从中断号获取 IPI 类型
    if let Some(ipi_type) = IpiType::from_irq(irq) {
        match ipi_type {
            IpiType::Reschedule => {
                // 重新调度信号
                println!("ipi: Hart {} received reschedule IPI", hart);
                // TODO: 触发调度器
            }
            IpiType::Stop => {
                // 停止 CPU
                println!("ipi: Hart {} received stop IPI, halting...", hart);
                // 进入空闲循环
                loop {
                    unsafe {
                        core::arch::asm!("wfi", options(nomem, nostack));
                    }
                }
            }
        }
    } else {
        println!("ipi: Unknown IPI interrupt {}", irq);
    }
}

/// 获取 IPI 计数
pub fn get_ipi_count(hart: usize) -> u32 {
    if hart < 4 {
        IPI_COUNT[hart].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// 初始化 IPI
pub fn init() {
    println!("ipi: Initializing RISC-V IPI support...");

    // IPI 使用 PLIC 中断 10-13
    // 这些中断已经在 plic::init() 中被使能

    println!("ipi: IPI support initialized (framework only, PLIC IPI pending)");
}
