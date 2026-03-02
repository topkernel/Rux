//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 设备号定义
//!
//! 参考 Linux include/linux/kdev_t.h 和 include/linux/major.h

/// 设备号 (major:minor)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    /// 主设备号 (12 bits in Linux, we use u32 for simplicity)
    pub major: u32,
    /// 次设备号 (20 bits in Linux, we use u32 for simplicity)
    pub minor: u32,
}

impl DevNo {
    /// 创建新的设备号
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// 从 u64 转换
    pub const fn from_u64(v: u64) -> Self {
        Self {
            major: (v >> 32) as u32,
            minor: v as u32,
        }
    }

    /// 转换为 u64
    pub const fn to_u64(&self) -> u64 {
        ((self.major as u64) << 32) | (self.minor as u64)
    }
}

impl Default for DevNo {
    fn default() -> Self {
        Self { major: 0, minor: 0 }
    }
}

// ============================================================================
// Linux 标准主设备号
// ============================================================================

/// 内存设备 (null, zero, random, etc.)
pub const MEM_MAJOR: u32 = 1;

/// TTY 设备
pub const TTY_MAJOR: u32 = 4;

/// 并行端口
pub const LP_MAJOR: u32 = 6;

/// SCSI 磁盘
pub const SCSI_DISK_MAJOR: u32 = 8;

/// MTD 块设备
pub const MTD_BLOCK_MAJOR: u32 = 31;

/// IDE 磁盘
pub const IDE_DISK_MAJOR: u32 = 33;

/// Framebuffer 设备
pub const FB_MAJOR: u32 = 29;

/// 输入设备 (键盘、鼠标、evdev 等)
pub const INPUT_MAJOR: u32 = 13;

/// evdev 次设备号起始
pub const EVDEV_MINOR_BASE: u32 = 64;

/// 鼠标设备 (mice, mouse0, mouse1, etc.)
pub const MICE_MINOR: u32 = 63;
pub const MOUSE_MINOR_BASE: u32 = 32;

// ============================================================================
// 常用设备
// ============================================================================

/// /dev/null
pub const DEV_NULL: DevNo = DevNo::new(MEM_MAJOR, 3);

/// /dev/zero
pub const DEV_ZERO: DevNo = DevNo::new(MEM_MAJOR, 5);

/// /dev/random
pub const DEV_RANDOM: DevNo = DevNo::new(MEM_MAJOR, 8);

/// /dev/urandom
pub const DEV_URANDOM: DevNo = DevNo::new(MEM_MAJOR, 9);

/// /dev/input/event0 (键盘)
pub const DEV_EVDEV_KEYBOARD: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE);

/// /dev/input/event1 (鼠标/指针)
pub const DEV_EVDEV_POINTER: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE + 1);
