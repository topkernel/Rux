//! RISC-V 64位架构支持
//!
//! 支持 RISC-V 64位 (RV64GC) 架构

pub mod boot;
pub mod trap;
pub mod context;
pub mod cpu;
pub mod syscall;
pub mod mm;

use crate::println;
use core::arch::asm;

// 导出上下文切换函数
pub use context::context_switch;

/// RISC-V 架构初始化 (兼容 main.rs 的调用)
pub fn arch_init() {
    init();
}

/// RISC-V 架构初始化
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

/// 打印 CPU 信息
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

/// 使能中断
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

/// 获取当前 CPU ID
pub fn cpu_id() -> u64 {
    unsafe {
        let mhartid: u64;
        asm!("csrrw {}, mhartid, zero", out(reg) mhartid);
        mhartid
    }
}
