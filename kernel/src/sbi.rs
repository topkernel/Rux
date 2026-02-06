//! RISC-V SBI (Supervisor Binary Interface) 调用封装
//!
//! 使用 sbi-rt crate 的 SBI 0.2 扩展

pub use sbi_rt::{SbiRet};

/// SBI 0.2 TIMER extension 的 set_timer (推荐使用)
pub use sbi_rt::set_timer;
