//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Device Number Definitions

/// Device number (major:minor)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    /// Major device number (12 bits in Linux, we use u32 for simplicity)
    pub major: u32,
    /// Minor device number (20 bits in Linux, we use u32 for simplicity)
    pub minor: u32,
}

impl DevNo {
    /// Create new device number
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Convert from u64
    pub const fn from_u64(v: u64) -> Self {
        Self {
            major: (v >> 32) as u32,
            minor: v as u32,
        }
    }

    /// Convert to u64
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
// Standard major device numbers
// ============================================================================

/// Memory devices (null, zero, random, etc.)
pub const MEM_MAJOR: u32 = 1;

/// TTY devices
pub const TTY_MAJOR: u32 = 4;

/// Parallel ports
pub const LP_MAJOR: u32 = 6;

/// SCSI disks
pub const SCSI_DISK_MAJOR: u32 = 8;

/// MTD block devices
pub const MTD_BLOCK_MAJOR: u32 = 31;

/// IDE disks
pub const IDE_DISK_MAJOR: u32 = 33;

/// Framebuffer devices
pub const FB_MAJOR: u32 = 29;

/// Input devices (keyboard, mouse, evdev, etc.)
pub const INPUT_MAJOR: u32 = 13;

/// evdev minor number base
pub const EVDEV_MINOR_BASE: u32 = 64;

/// Mouse devices (mice, mouse0, mouse1, etc.)
pub const MICE_MINOR: u32 = 63;
pub const MOUSE_MINOR_BASE: u32 = 32;

// ============================================================================
// Common devices
// ============================================================================

/// /dev/null
pub const DEV_NULL: DevNo = DevNo::new(MEM_MAJOR, 3);

/// /dev/zero
pub const DEV_ZERO: DevNo = DevNo::new(MEM_MAJOR, 5);

/// /dev/random
pub const DEV_RANDOM: DevNo = DevNo::new(MEM_MAJOR, 8);

/// /dev/urandom
pub const DEV_URANDOM: DevNo = DevNo::new(MEM_MAJOR, 9);

/// /dev/input/event0 (keyboard)
pub const DEV_EVDEV_KEYBOARD: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE);

/// /dev/input/event1 (mouse/pointer)
pub const DEV_EVDEV_POINTER: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE + 1);
