//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! IPI (Inter-Processor Interrupt) 支持
//!
//! 对应 Linux 的 arch/arm64/kernel/smp.c:
//! - smp_cross_call() - 发送跨 CPU 调用
//! - handle_IPI() - 处理 IPI
//!
//! 使用 GICv3 SGI (Software Generated Interrupt) 实现

use crate::console::putchar;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum IpiType {
    /// 重新调度
    Reschedule = 0,
    /// 停止 CPU
    Stop = 1,
}

impl IpiType {
    /// 将 IPI 类型转换为 SGI 中断号
    ///
    /// 在 GICv3 中，SGI 范围是 0-15
    /// 我们使用:
    /// - SGI 0: Reschedule
    /// - SGI 1: Stop
    #[inline]
    pub const fn as_sgi(self) -> u32 {
        self as u32
    }

    /// 从 SGI 中断号创建 IPI 类型
    #[inline]
    pub const fn from_sgi(sgi: u32) -> Option<Self> {
        match sgi {
            0 => Some(IpiType::Reschedule),
            1 => Some(IpiType::Stop),
            _ => None,
        }
    }
}

pub fn send_ipi(target_cpu: u64, ipi_type: IpiType) {
    use crate::console::putchar;

    unsafe {
        const MSG: &[u8] = b"[IPI: Sending IPI ";
        for &b in MSG {
            putchar(b);
        }

        let hex = b"0123456789ABCDEF";
        putchar(hex[ipi_type as u8 as usize]);
        putchar(b' ');
        putchar(b't');
        putchar(b'o');
        putchar(b' ');
        putchar(hex[(target_cpu & 0xF) as usize]);
        putchar(b']');
        putchar(b'\n');
    }

    let sgi = ipi_type.as_sgi();

    // QEMU virt 的 CPU MPIDR 格式：
    // - CPU 0: MPIDR = 0x80000000 (Aff0=0, Aff1=0, Aff2=0)
    // - CPU 1: MPIDR = 0x80000001 (Aff0=1, Aff1=0, Aff2=0)
    //
    // 对于简单的双核系统，Aff0 就是 CPU ID
    let aff0 = target_cpu as u64 & 0xFF;  // Aff0 (CPU ID)
    let aff1 = 0u64;  // Aff1 (cluster)
    let aff2 = 0u64;  // Aff2 (cluster group)

    // 构造 ICC_SGI1R_EL1 寄存器值
    // bit [40] = 1: 使用 TARGET_LIST 模式
    // bit [25:16] = Aff1
    // bit [15:0] = 目标列表 (bit i 表示 CPU i)
    // bit [3:0] = SGI 中断号
    let sgir = (1 << 40)               // TARGET_LIST 模式
             | (aff1 << 16)           // Aff1 值
             | (1u64 << aff0)         // 目标 CPU 位掩码
             | (sgi as u64);          // SGI 中断号

    unsafe {
        // 使用 mrs/msr 指令访问 ICC_SGI1R_EL1 系统寄存器
        core::arch::asm!(
            "msr ICC_SGI1R_EL1, {}",
            in(reg) sgir,
            options(nostack)
        );

        // 内存屏障，确保 IPI 发送完成
        core::arch::asm!("dmb sy", options(nomem, nostack));
    }
}

pub fn handle_ipi(sgi: u32) {
    use crate::console::putchar;

    let cpu_id = crate::arch::aarch64::cpu::get_core_id();

    unsafe {
        const MSG: &[u8] = b"[IPI: CPU";
        for &b in MSG {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[(cpu_id & 0xF) as usize]);
        const MSG2: &[u8] = b" received IPI ";
        for &b in MSG2 {
            putchar(b);
        }
        putchar(hex[(sgi & 0xF) as usize]);
        putchar(b']');
        putchar(b'\n');
    }

    match IpiType::from_sgi(sgi) {
        Some(IpiType::Reschedule) => {
            // 重新调度 IPI
            // TODO: 设置当前 CPU 的需要重新调度标志
            unsafe {
                const MSG: &[u8] = b"[IPI: Reschedule]\n";
                for &b in MSG {
                    putchar(b);
                }
            }
        }
        Some(IpiType::Stop) => {
            // 停止 CPU IPI
            // 进入低功耗状态
            unsafe {
                const MSG: &[u8] = b"[IPI: Stop - entering WFI]\n";
                for &b in MSG {
                    putchar(b);
                }

                // 进入 WFI (Wait For Interrupt) 状态
                loop {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        None => {
            // 未知的 IPI 类型
            unsafe {
                const MSG: &[u8] = b"[IPI: Unknown IPI type]\n";
                for &b in MSG {
                    putchar(b);
                }
            }
        }
    }
}

pub fn smp_send_reschedule() {
    let this_cpu = crate::arch::aarch64::cpu::get_core_id();

    // 发送给所有其他 CPU
    // TODO: 实现多核支持时，这里需要遍历所有在线 CPU
    // 目前只支持 2 个 CPU
    if this_cpu == 0 {
        // CPU 0 发送 IPI 到 CPU 1
        send_ipi(1, IpiType::Reschedule);
    } else {
        // CPU 1 发送 IPI 到 CPU 0
        send_ipi(0, IpiType::Reschedule);
    }
}
