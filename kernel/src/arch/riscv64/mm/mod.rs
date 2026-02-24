//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V Sv39 虚拟内存管理
//!
//! RISC-V Sv39 分页规范：
//! - 3 级页表（512 PTE/级）
//! - 39 位虚拟地址（512GB）
//! - 4KB 页大小
//! - 页表项：10 位 PPN + 10 位标志
//!
//! 参考：
//! - RISC-V 特权架构规范 v20211203
//! - rCore-Tutorial-v3

// 基础内存管理（原 mm.rs 内容）
mod base;
pub use base::*;

// 页故障处理
pub mod fault;
pub use fault::{do_page_fault, MmFaultResult as FaultResult, fixup_exception};
