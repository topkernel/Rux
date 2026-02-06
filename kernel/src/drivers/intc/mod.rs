//! 中断控制器驱动
//!
//! 支持 GICv3（通用中断控制器 v3）
//! 用于 QEMU virt 平台（实际运行在 GICv3 模式）

pub mod gicv3;

pub use gicv3::*;
