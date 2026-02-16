//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 平台无关的地址空间接口
//!
//! 本模块重导出平台特定的 AddressSpace 实现：
//! - RISC-V: arch/riscv64/mm.rs
//!
//! 高级 VMA 操作（brk, mmap, munmap）在各平台实现中提供

// 平台特定的 AddressSpace 重导出
pub use crate::arch::riscv64::mm::AddressSpace;

// 重新导出常用类型
pub use crate::mm::page::{VirtAddr, PhysAddr, PAGE_SIZE};

// VMA 相关类型
pub use crate::mm::vma::{Vma, VmaFlags, VmaManager, VmaType, VmaError};

// 地图错误类型（公共接口）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// 已经映射
    AlreadyMapped,
    /// 未映射
    NotMapped,
    /// 内存不足
    OutOfMemory,
    /// 无效参数
    Invalid,
}

// 实现 From<VmaError> 以支持 ? 操作符
impl From<VmaError> for MapError {
    fn from(err: VmaError) -> Self {
        match err {
            VmaError::Overlap => MapError::AlreadyMapped,
            VmaError::NoSpace => MapError::OutOfMemory,
            VmaError::NotFound => MapError::NotMapped,
            VmaError::Invalid => MapError::Invalid,
        }
    }
}

// 页权限（公共接口）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Perm {
    /// 无访问
    None = 0,
    /// 只读
    Read = 1,
    /// 读写
    ReadWrite = 2,
    /// 读写执行
    ReadWriteExec = 3,
}

// 页表类型（公共接口）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageTableType {
    /// 内核页表
    Kernel = 0,
    /// 用户页表
    User = 1,
}
