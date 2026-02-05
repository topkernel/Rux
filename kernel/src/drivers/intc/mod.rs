//! 中断控制器驱动
//!
//! 支持 GICv2 模式（使用 GICC CPU 接口寄存器）

pub mod gic;

pub use gic::*;
