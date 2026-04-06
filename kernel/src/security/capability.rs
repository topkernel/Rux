//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! POSIX.1e Capability constants and bitmask type.
//!
//! Provides the `Cap` type (equivalent to Linux's `kernel_cap_t`) and all 41
//! `CAP_*` constants matching the Linux UAPI numbering exactly.

/// Capability bitmask — 41 capabilities fit in a single u64.
///
/// Equivalent to Linux's `kernel_cap_t`.  Copy-friendly, no heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(u64);

/// Bits 0..=40 are valid (41 capabilities).
pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;

// ==================== CAP_* constants ====================
// Must match Linux UAPI numbers exactly (include/uapi/linux/capability.h).

/// Override file ownership restrictions.
pub const CAP_CHOWN: u32 = 0;
/// Override all DAC (read/write/execute) checks.
pub const CAP_DAC_OVERRIDE: u32 = 1;
/// Override DAC read/search on directories.
pub const CAP_DAC_READ_SEARCH: u32 = 2;
/// Override file owner ID checks (chmod, utimes).
pub const CAP_FOWNER: u32 = 3;
/// Override S_ISUID/S_ISGID restrictions.
pub const CAP_FSETID: u32 = 4;
/// Override signal sending UID check.
pub const CAP_KILL: u32 = 5;
/// Allow setgid/setgroups manipulation.
pub const CAP_SETGID: u32 = 6;
/// Allow setuid manipulation.
pub const CAP_SETUID: u32 = 7;
/// Modify capability bounding set.
pub const CAP_SETPCAP: u32 = 8;
/// Modify S_IMMUTABLE/S_APPEND file attributes.
pub const CAP_LINUX_IMMUTABLE: u32 = 9;
/// Bind TCP/UDP to ports below 1024.
pub const CAP_NET_BIND_SERVICE: u32 = 10;
/// Allow broadcast/multicast datagrams.
pub const CAP_NET_BROADCAST: u32 = 11;
/// Network administration (firewall, routing, interfaces).
pub const CAP_NET_ADMIN: u32 = 12;
/// Use RAW and PACKET sockets.
pub const CAP_NET_RAW: u32 = 13;
/// Lock shared memory segments, mlock/mlockall.
pub const CAP_IPC_LOCK: u32 = 14;
/// Override IPC ownership checks.
pub const CAP_IPC_OWNER: u32 = 15;
/// Load/unload kernel modules.
pub const CAP_SYS_MODULE: u32 = 16;
/// Perform I/O port operations (ioperm/iopl).
pub const CAP_SYS_RAWIO: u32 = 17;
/// Use chroot().
pub const CAP_SYS_CHROOT: u32 = 18;
/// ptrace() any process.
pub const CAP_SYS_PTRACE: u32 = 19;
/// Enable process accounting (acct()).
pub const CAP_SYS_PACCT: u32 = 20;
/// Catch-all for many administration operations.
pub const CAP_SYS_ADMIN: u32 = 21;
/// Use reboot().
pub const CAP_SYS_BOOT: u32 = 22;
/// Raise scheduling priority / use real-time policies.
pub const CAP_SYS_NICE: u32 = 23;
/// Override resource limits (setrlimit).
pub const CAP_SYS_RESOURCE: u32 = 24;
/// Set system clock (settimeofday, clock_settime, adjtimex).
pub const CAP_SYS_TIME: u32 = 25;
/// Configure TTY devices (vhangup).
pub const CAP_SYS_TTY_CONFIG: u32 = 26;
/// Create special files via mknod().
pub const CAP_MKNOD: u32 = 27;
/// Take file leases.
pub const CAP_LEASE: u32 = 28;
/// Write to kernel audit log.
pub const CAP_AUDIT_WRITE: u32 = 29;
/// Configure kernel audit subsystem.
pub const CAP_AUDIT_CONTROL: u32 = 30;
/// Set file capabilities (security.capability xattr).
pub const CAP_SETFCAP: u32 = 31;
/// Override MAC (Mandatory Access Control) policy.
pub const CAP_MAC_OVERRIDE: u32 = 32;
/// Configure MAC policy.
pub const CAP_MAC_ADMIN: u32 = 33;
/// Use syslog() and configure printk log level.
pub const CAP_SYSLOG: u32 = 34;
/// Wake system from suspend (alarm timers).
pub const CAP_WAKE_ALARM: u32 = 35;
/// Prevent system suspend.
pub const CAP_BLOCK_SUSPEND: u32 = 36;
/// Read kernel audit log.
pub const CAP_AUDIT_READ: u32 = 37;
/// Use performance monitoring events.
pub const CAP_PERFMON: u32 = 38;
/// BPF operations.
pub const CAP_BPF: u32 = 39;
/// Checkpoint/restore operations.
pub const CAP_CHECKPOINT_RESTORE: u32 = 40;

/// Highest valid capability number.
pub const CAP_LAST_CAP: u32 = CAP_CHECKPOINT_RESTORE;

impl Cap {
    /// Empty capability set — no capabilities.
    pub const EMPTY: Cap = Cap(0);
    /// Full capability set — all 41 bits set.
    pub const FULL: Cap = Cap(CAP_VALID_MASK);

    /// Create from raw u64, masking to valid bits.
    #[inline]
    pub const fn new(mask: u64) -> Self {
        Cap(mask & CAP_VALID_MASK)
    }

    /// Check if a specific capability is raised.
    /// Returns false for invalid capability numbers (> 40).
    #[inline]
    pub fn has(&self, cap: u32) -> bool {
        if cap > CAP_LAST_CAP {
            return false;
        }
        (self.0 >> cap) & 1 == 1
    }

    /// Raise a specific capability.
    #[inline]
    pub fn set(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP {
            self.0 |= 1u64 << cap;
        }
    }

    /// Clear a specific capability.
    #[inline]
    pub fn clear(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP {
            self.0 &= !(1u64 << cap);
        }
    }

    /// Bitwise AND — intersection of two sets.
    #[inline]
    pub fn intersect(&self, other: Cap) -> Cap {
        Cap(self.0 & other.0)
    }

    /// Bitwise OR — union of two sets.
    #[inline]
    pub fn union(&self, other: Cap) -> Cap {
        Cap(self.0 | other.0)
    }

    /// Bitwise XOR.
    #[inline]
    pub fn xor(&self, other: Cap) -> Cap {
        Cap(self.0 ^ other.0)
    }

    /// Complement (within valid mask).
    #[inline]
    pub fn complement(&self) -> Cap {
        Cap(CAP_VALID_MASK & !self.0)
    }

    /// Check if self is a subset of other (self & ~other == 0).
    #[inline]
    pub fn is_subset_of(&self, other: Cap) -> bool {
        (self.0 & !other.0) == 0
    }

    /// Check if any bit is set.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Raw bitmask value.
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Extract low 32 bits (for capget/capset ABI).
    #[inline]
    pub fn lo(&self) -> u32 {
        self.0 as u32
    }

    /// Extract high 32 bits (for capget/capset ABI).
    #[inline]
    pub fn hi(&self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Build from two u32 halves (for capset ABI).
    #[inline]
    pub fn from_halves(lo: u32, hi: u32) -> Self {
        Cap::new(((hi as u64) << 32) | (lo as u64))
    }
}
