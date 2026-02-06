//! Rux 内核配置（自动生成）
//!
//! 此文件由 build.rs 根据 Kernel.toml 自动生成，请勿手动修改

// ============================================================
// 基本信息
// ============================================================

/// 内核名称
pub const KERNEL_NAME: &str = "Rux";

/// 内核版本
pub const KERNEL_VERSION: &str = "0.1.0";

/// 目标平台
pub const TARGET_PLATFORM: &str = "riscv64";

// ============================================================
// 内存配置
// ============================================================

/// 内核堆大小（字节）
pub const KERNEL_HEAP_SIZE: usize = 16777216;

/// 物理内存大小（字节）
pub const PHYS_MEMORY_SIZE: usize = 2147483648;

/// 页大小
pub const PAGE_SIZE: usize = 4096;

/// 页大小位移
pub const PAGE_SHIFT: usize = 12;

// ============================================================
// 驱动配置
// ============================================================

/// 是否启用UART驱动
pub const ENABLE_UART: bool = true;

/// 是否启用定时器驱动
pub const ENABLE_TIMER: bool = true;

/// 是否启用GIC中断控制器
pub const ENABLE_GIC: bool = false;
