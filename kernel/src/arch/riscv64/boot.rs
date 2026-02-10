//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64位内核启动流程

// 包含 boot.S 汇编代码
core::arch::global_asm!(include_str!("boot.S"));

pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}
