//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64位内核启动流程

// 包含 boot.S 汇编代码
core::arch::global_asm!(include_str!("boot.S"));

/// 设备树指针（由 boot.S 设置）
extern "C" {
    /// 设备树指针（由 OpenSBI 通过 a1 寄存器传递）
    static dtb_pointer: u64;
}

pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}

/// 获取设备树指针
///
/// OpenSBI 在跳转到内核时，a1 寄存器包含设备树指针
/// 如果没有设备树，a1 的值为 0
pub fn get_dtb_pointer() -> u64 {
    unsafe { dtb_pointer }
}
