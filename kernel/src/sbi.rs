//! RISC-V SBI (Supervisor Binary Interface) 调用封装

use core::arch::asm;

/// SBI 扩展 ID (Legacy SBI 0.00)
pub mod eid {
    pub const TIMER: usize = 0x00;  // Legacy timer extension
}

/// SBI 功能 ID
pub mod fid {
    pub const SET_TIMER: usize = 0x00;  // set_timer
}

/// SBI 调用返回值
#[derive(Debug, Clone, Copy)]
pub struct SbiRet {
    pub error: isize,
    pub value: usize,
}

/// 设置定时器 (Legacy SBI call)
pub fn set_timer(stime: u64) -> SbiRet {
    unsafe {
        let mut error: usize = 0;
        let mut value: usize;

        asm!(
            "ecall",
            inlateout("a0") stime as usize => value,
            inlateout("a1") error => _,
            in("a6") fid::SET_TIMER,
            in("a7") eid::TIMER,
            options(nostack)
        );

        SbiRet {
            error: error as isize,
            value,
        }
    }
}
