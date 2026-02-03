pub mod boot;
pub mod mm;
pub mod cpu;
pub mod trap;
pub mod context;
pub mod syscall;

pub use boot::init;
pub use trap::*;
pub use context::{context_switch, UserContext};

/// 初始化架构相关功能
///
/// 这会按顺序初始化：
/// 1. 基础引导设置
/// 2. MMU 和页表
pub fn arch_init() {
    boot::init();
    unsafe { mm::init(); }
}
