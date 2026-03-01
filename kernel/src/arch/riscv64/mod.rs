//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64位架构支持
//!
//! 支持 RISC-V 64位 (RV64GC) 架构

pub mod boot;
pub mod pt_regs;
pub mod trap;
pub mod context;
pub mod cpu;
// syscall 模块已移动到 kernel/src/syscall/
pub mod mm;
pub mod smp;
pub mod ipi;
pub mod process;
pub mod thread;
pub mod uaccess;

use crate::println;
use core::arch::asm;

// 包含用户模式切换汇编代码
core::arch::global_asm!(include_str!("usermode_asm.S"));



pub fn arch_init() {
    init();
}

pub fn init() {
    println!("arch: Initializing RISC-V architecture...");

    // 设置异常向量表
    trap::init();

    // 禁用中断
    unsafe {
        // RISC-V: 清除 mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus &= !(1 << 3); // Clear MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Interrupts disabled in machine mode");
    }

    // 打印 CPU 信息
    print_cpu_info();

    println!("arch: Architecture initialization [DONE]");
}

fn print_cpu_info() {
    unsafe {
        // 读取 mhartid (硬件线程 ID)
        let mhartid: u64;
        asm!("csrrw {}, mhartid, zero", out(reg) mhartid);

        // 读取 mimpid (机器实现 ID)
        let mimpid: u64;
        asm!("csrrw {}, mimpid, zero", out(reg) mimpid);

        // 读取 marchid (架构 ID)
        let marchid: u64;
        asm!("csrrw {}, marchid, zero", out(reg) marchid);

        println!("arch: mhartid (HART ID) = {}", mhartid);
        println!("arch: mimpid (Impl ID) = {:#x}", mimpid);
        println!("arch: marchid (Arch ID) = {:#x}", marchid);
    }
}

pub fn enable_interrupts() {
    unsafe {
        // 设置 mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus |= 1 << 3; // Set MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Machine-mode interrupts enabled");
    }
}

/// 获取当前 CPU (hart) ID
///
/// 在 S-mode 下，我们无法访问 mhartid CSR（只能从 M-mode 访问）。
/// 我们使用 tp 寄存器来存储 hart ID，这是 trap.S 中设置的标准方式。
///
/// 在 trap 入口时，tp 被设置为 hart ID + 1（以区分 0 和 null），
/// 所以我们需要减 1 来获取实际的 hart ID。
pub fn cpu_id() -> u64 {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // tp 存储 hart_id + 1，所以减 1 获取实际值
        // 但如果 tp 为 0，说明我们可能在早期启动阶段或用户态
        if tp_value == 0 {
            // 尝试从 sscratch 获取（如果是从用户态来的）
            let sscratch: u64;
            asm!("csrr {}, sscratch", out(reg) sscratch, options(nomem, nostack));

            // sscratch 存储的是 hart_id + 1
            if sscratch == 0 {
                // 如果 sscratch 也是 0，我们可能在启动阶段
                // 默认返回 0（boot hart）
                0
            } else {
                sscratch.saturating_sub(1)
            }
        } else {
            tp_value.saturating_sub(1)
        }
    }
}
