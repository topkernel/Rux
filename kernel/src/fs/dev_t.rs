//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Device Number Definitions

/// Device number (major:minor)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    /// Major device number
    pub major: u32,
    /// Minor device number
    pub minor: u32,
}

/// Number of bits used for minor device number (Linux: MINORBITS = 20)
const DEV_MINOR_BITS: u32 = 20;
/// Mask for minor device number
const DEV_MINOR_MASK: u64 = (1u64 << DEV_MINOR_BITS) - 1;

impl DevNo {
    /// Create new device number
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Convert from u64 (Linux kernel internal dev_t format)
    pub const fn from_u64(v: u64) -> Self {
        Self {
            major: (v >> DEV_MINOR_BITS) as u32,
            minor: (v & DEV_MINOR_MASK) as u32,
        }
    }

    /// Convert to u64 (Linux kernel internal dev_t format)
    pub const fn to_u64(&self) -> u64 {
        ((self.major as u64) << DEV_MINOR_BITS) | (self.minor as u64 & DEV_MINOR_MASK)
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

/// /dev/kmsg (kernel message buffer)
pub const DEV_KMSG: DevNo = DevNo::new(MEM_MAJOR, 11);

/// /dev/input/event0 (keyboard)
pub const DEV_EVDEV_KEYBOARD: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE);

/// /dev/input/event1 (mouse/pointer)
pub const DEV_EVDEV_POINTER: DevNo = DevNo::new(INPUT_MAJOR, EVDEV_MINOR_BASE + 1);
