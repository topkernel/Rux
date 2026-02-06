//! RISC-V 64位内核启动流程

use crate::println;

/// 获取当前核心 ID (Hart ID)
pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}
