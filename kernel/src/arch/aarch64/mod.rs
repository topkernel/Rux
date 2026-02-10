//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

pub mod boot;
pub mod mm;
pub mod cpu;
pub mod trap;
pub mod context;
pub mod syscall;
pub mod smp;
pub mod ipi;

pub use boot::init;
pub use trap::*;
pub use context::{context_switch, UserContext};
pub use smp::{boot_secondary_cpus, SmpData};
pub use ipi::{send_ipi, handle_ipi, IpiType, smp_send_reschedule};

/// 初始化架构相关功能
///
/// 这会按顺序初始化：
/// 1. 基础引导设置
/// 2. MMU 和页表
pub fn arch_init() {
    boot::init();
    unsafe { mm::init(); }
}
