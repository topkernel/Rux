//! RISC-V 64位内核启动流程

// 包含 boot.S 汇编代码
core::arch::global_asm!(include_str!("boot.S"));

/// 获取当前核心 ID (Hart ID)
pub fn get_core_id() -> u64 {
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}
