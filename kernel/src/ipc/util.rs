//! IPC core infrastructure
//!
//! Provides the central IPC object registry (ipc_ids), permission checking,
//! and ID encoding/decoding following the Linux kernel's design.

use crate::sync::spinlock::Spinlock;
use crate::sched;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// ============================================================================
// IPC Constants (matching Linux UAPI)
// ============================================================================

pub const IPC_CREAT: i32 = 0o1000;
pub const IPC_EXCL: i32 = 0o2000;
pub const IPC_NOWAIT: i32 = 0o4000;

/// IPC_RMID: Remove identifier
pub const IPC_RMID: i32 = 0;
/// IPC_SET: Set options
pub const IPC_SET: i32 = 1;
/// IPC_STAT: Get options
pub const IPC_STAT: i32 = 2;
/// IPC_INFO: See ipc info
pub const IPC_INFO: i32 = 3;

// Semaphore control commands
pub const GETPID: i32 = 11;
pub const GETVAL: i32 = 12;
pub const GETALL: i32 = 13;
pub const GETNCNT: i32 = 14;
pub const GETZCNT: i32 = 15;
pub const SETVAL: i32 = 16;
pub const SETALL: i32 = 17;

// Semaphore operation flags
pub const SEM_UNDO: u16 = 0x1000;

// Message queue flags
pub const MSG_NOERROR: i32 = 0o10000;
pub const MSG_EXCEPT: i32 = 0o20000;
pub const MSG_COPY: i32 = 0o40000;

// Shared memory flags
pub const SHM_RDONLY: i32 = 0o10000;
pub const SHM_RND: i32 = 0o20000;
pub const SHM_REMAP: i32 = 0o40000;
pub const SHM_EXEC: i32 = 0o100000;

// POSIX MQ open flags
pub const O_CREAT_MQ: i32 = 0o100;
pub const O_EXCL_MQ: i32 = 0o200;
pub const O_NONBLOCK_MQ: i32 = 0o400;
pub const O_CLOEXEC_MQ: i32 = 0o2000000;

// Maximum number of IPC objects per type
const IPC_IDS_MAX: usize = 256;

// ============================================================================
// UAPI Structures (ABI-compatible, #[repr(C)])
// ============================================================================

/// struct ipc64_perm — userspace-visible IPC permission structure.
/// Must match Linux's asm-generic/ipcbuf.h exactly for RV64.
/// Total: 48 bytes.
#[repr(C)]
pub struct IpcPermUapi {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u32,
    pub seq: u16,
    pub __pad2: u16,
    pub __unused1: u64,
    pub __unused2: u64,
}

impl Default for IpcPermUapi {
    fn default() -> Self {
        Self {
            key: 0, uid: 0, gid: 0, cuid: 0, cgid: 0,
            mode: 0, seq: 0, __pad2: 0, __unused1: 0, __unused2: 0,
        }
    }
}

// ============================================================================
// Kernel IPC Permission Structure
// ============================================================================

/// kern_ipc_perm — kernel-internal permission structure embedded in all IPC objects.
#[derive(Clone)]
pub struct KernIpcPerm {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub seq: u32,
}

impl KernIpcPerm {
    pub fn new(key: i32, mode: u16) -> Self {
        let cred = sched::current().map(|t| (t.cred().uid, t.cred().gid, t.cred().euid, t.cred().egid));
        let (uid, gid, cuid, cgid) = cred.unwrap_or((0, 0, 0, 0));
        Self {
            key,
            uid,
            gid,
            cuid,
            cgid,
            mode,
            seq: 0,
        }
    }

    /// Convert to UAPI structure for copy_to_user
    pub fn to_uapi(&self) -> IpcPermUapi {
        IpcPermUapi {
            key: self.key,
            uid: self.uid,
            gid: self.gid,
            cuid: self.cuid,
            cgid: self.cgid,
            mode: self.mode as u32,
            seq: self.seq as u16,
            __pad2: 0,
            __unused1: 0,
            __unused2: 0,
        }
    }

    /// Update uid/gid/mode from IPC_SET.
    /// Matches Linux's `ipc_update_perm()`: updates uid, gid, and replaces
    /// the lower 9 permission bits while preserving upper bits.
    pub fn update_from_set(&mut self, new_uid: u32, new_gid: u32, new_mode: u32) {
        self.uid = new_uid;
        self.gid = new_gid;
        self.mode = (self.mode & !0o777u16) | (new_mode as u16 & 0o777);
    }

    /// Set creator uid/gid (only allowed by root)
    pub fn set_creator(&mut self, cuid: u32, cgid: u32) {
        self.cuid = cuid;
        self.cgid = cgid;
    }
}

// ============================================================================
// IPC ID Encoding / Decoding
// ============================================================================

/// Build IPC ID from slot index and sequence number.
/// Linux format: (index << 16) | (seq & 0xFFFF)
#[inline]
pub fn ipc_build_id(index: usize, seq: u32) -> i32 {
    (((index as u32) << 16) | (seq & 0xFFFF)) as i32
}

/// Extract slot index from IPC ID.
#[inline]
pub fn ipc_id_to_index(id: i32) -> usize {
    ((id as u32) >> 16) as usize
}

/// Extract sequence number from IPC ID.
#[inline]
pub fn ipc_id_seq(id: i32) -> u32 {
    (id as u32) & 0xFFFF
}

// ============================================================================
// IpcIds — Central IPC Object Registry
// ============================================================================

/// Wrapper around each IPC object slot.
pub struct IpcObjectEntry<T> {
    pub inner: T,
    pub deleted: bool,
}

/// Generic IPC object registry. Manages allocation, lookup, and deletion
/// of IPC objects by key or ID.
pub struct IpcIds<T> {
    pub slots: Spinlock<[Option<alloc::boxed::Box<IpcObjectEntry<T>>>; IPC_IDS_MAX]>,
    next_seq: AtomicU32,
    pub(crate) count: AtomicUsize,
}

impl<T> IpcIds<T> {
    pub const fn new() -> Self {
        Self {
            slots: Spinlock::new([const { None }; IPC_IDS_MAX]),
            next_seq: AtomicU32::new(1),
            count: AtomicUsize::new(0),
        }
    }

    /// Find an IPC object by key. Returns slot index or None.
    /// Used for IPC_CREAT lookups.
    fn find_by_key_locked(&self, key: i32) -> Option<usize>
    where T: IpcObject {
        let slots = self.slots.lock();
        for i in 0..IPC_IDS_MAX {
            if let Some(ref entry) = slots[i] {
                if !entry.deleted && entry.inner.get_perm().key == key {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Allocate a new IPC object slot. Returns (ipc_id, slot_index) or error.
    /// If key != IPC_PRIVATE and an object with that key exists, returns its ID.
    pub fn alloc(&self, mut obj: T, key: i32, flags: i32) -> Result<(i32, usize), i32>
    where T: IpcObject {
        // Hold the lock for the entire operation to prevent TOCTOU races
        // where another thread deletes the slot between key lookup and reuse.
        let mut slots = self.slots.lock();

        // Check for existing key (unless IPC_PRIVATE)
        if key != 0 {
            for i in 0..IPC_IDS_MAX {
                if let Some(ref entry) = slots[i] {
                    if !entry.deleted && entry.inner.get_perm().key == key {
                        if flags & IPC_EXCL != 0 {
                            return Err(-17); // EEXIST
                        }
                        let perm = &entry.inner.get_perm();
                        if !ipc_check_permissions(perm, 0o6) {
                            return Err(-13); // EACCES
                        }
                        return Ok((ipc_build_id(i, perm.seq), i));
                    }
                }
            }
        }

        // Allocate a new slot
        let mut free_idx = None;
        for i in 0..IPC_IDS_MAX {
            if slots[i].is_none() {
                free_idx = Some(i);
                break;
            }
        }

        let idx = match free_idx {
            Some(i) => i,
            None => return Err(-28), // ENOSPC
        };

        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        if seq == 0 {
            // seq 0 is reserved, skip to 1
            self.next_seq.store(2, Ordering::Relaxed);
        }

        obj.get_perm_mut().seq = seq;

        slots[idx] = Some(alloc::boxed::Box::new(IpcObjectEntry {
            inner: obj,
            deleted: false,
        }));

        self.count.fetch_add(1, Ordering::Relaxed);
        Ok((ipc_build_id(idx, seq), idx))
    }

    /// Look up an IPC object by its ID. Verifies the sequence number matches.
    /// Returns None if not found or deleted.
    pub fn find(&self, id: i32) -> Option<usize>
    where T: IpcObject {
        let idx = ipc_id_to_index(id);
        let expected_seq = ipc_id_seq(id);

        if idx >= IPC_IDS_MAX {
            return None;
        }

        let slots = self.slots.lock();
        if let Some(ref entry) = slots[idx] {
            if !entry.deleted && entry.inner.get_perm().seq == expected_seq {
                return Some(idx);
            }
        }
        None
    }

    /// Look up an IPC object by ID and check read/write permissions.
    pub fn find_with_perms(&self, id: i32, desired_mode: u16) -> Result<usize, i32>
    where T: IpcObject {
        let idx = self.find(id).ok_or(-22)?; // EINVAL

        let slots = self.slots.lock();
        if let Some(ref entry) = slots[idx] {
            let perm = &entry.inner.get_perm();
            if !ipc_check_permissions(perm, desired_mode) {
                return Err(-13); // EACCES
            }
            return Ok(idx);
        }
        Err(-22) // EINVAL (shouldn't happen since find() succeeded)
    }

    /// Mark an IPC object as deleted. Wakes all waiters.
    pub fn remove(&self, id: i32) -> bool {
        let idx = ipc_id_to_index(id);
        if idx >= IPC_IDS_MAX {
            return false;
        }

        let mut slots = self.slots.lock();
        if let Some(ref mut entry) = slots[idx] {
            if !entry.deleted {
                entry.deleted = true;
                self.count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Remove slot entirely (after all attaches are gone for shared memory).
    pub fn free_slot(&self, id: i32) {
        let idx = ipc_id_to_index(id);
        if idx < IPC_IDS_MAX {
            let mut slots = self.slots.lock();
            slots[idx] = None;
        }
    }

    /// Get current count of active objects
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

// ============================================================================
// Trait for IPC objects — provides access to kern_ipc_perm
// ============================================================================

/// Trait implemented by all IPC kernel objects to expose their permission structure.
pub trait IpcObject {
    fn get_perm(&self) -> &KernIpcPerm;
    fn get_perm_mut(&mut self) -> &mut KernIpcPerm;
}

// ============================================================================
// Permission Checking
// ============================================================================

/// Check if the current process has the desired permissions on an IPC object.
/// Follows Linux's ipcperms() logic.
pub fn ipc_check_permissions(perm: &KernIpcPerm, desired_mode: u16) -> bool {
    let cred = match sched::current() {
        Some(t) => t.cred(),
        None => return false,
    };

    // CAP_IPC_OWNER: bypasses all checks
    if crate::security::has_capability(cred, crate::security::CAP_IPC_OWNER) {
        return true;
    }

    let mode = perm.mode as u16;

    // Owner check
    if cred.euid == perm.uid || cred.euid == perm.cuid {
        let owner_bits = (mode >> 6) & 0o7;
        return (desired_mode & owner_bits) == desired_mode;
    }

    // Group check
    if cred.egid == perm.gid || cred.egid == perm.cgid {
        let group_bits = (mode >> 3) & 0o7;
        return (desired_mode & group_bits) == desired_mode;
    }

    // Other check
    let other_bits = mode & 0o7;
    (desired_mode & other_bits) == desired_mode
}

/// Helper to get current jiffies-based time for IPC timestamps.
/// Returns time in seconds since boot.
pub fn ipc_current_time() -> i64 {
    let jiffies = crate::drivers::timer::get_jiffies();
    (jiffies / crate::drivers::timer::HZ as u64) as i64
}

/// Per-process semaphore undo entry.
/// Records an adjustment made to a semaphore so it can be reversed on process exit.
#[derive(Clone, Copy)]
pub struct SemUndoEntry {
    /// Semaphore set ID.
    pub semid: i32,
    /// Semaphore number within the set.
    pub sem_num: u16,
    /// Adjustment value (the sem_op that was applied).
    pub adjustment: i32,
}
