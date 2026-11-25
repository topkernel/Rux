//! Framework-side IPC hook table (crate-flip rung: ipc): every
//! lib-side consumer of the MOVABLE ipc policy (sysv sem/msg/shm,
//! posix_mq, keys, access_token) resolves through these rows (the
//! NetHooks pattern: nullable-default fns, no-op until the service
//! crate registers; the composition bin calls ipc::register_all
//! before the framework impl).
//!
//! binder.rs is NOT behind this table — it stays in the framework tree
//! (its block-view windows, Arc adoption and diagnostic walkers are
//! floor-shaped; the rung's verdict), so every binder call site keeps
//! its direct `crate::ipc::binder::*` path.
//!
//! Row discipline: exit/exec/fork/sysctl/procfs/devfs/mm-vma rows are
//! all COLD paths (process teardown, boot-time registration, sysctl
//! reads). No row sits on a Tier-2 guarded hot path — the semop copy
//! staging and the mq wake paths are FORWARD calls the service makes
//! into the framework (direct, LTO-inlinable), and they gain nothing
//! from this table.

#![forbid(unsafe_code)]
use crate::fs::dev_t::DevNo;
use crate::fs::file::FileOps;
use crate::process::Task;
use alloc::vec::Vec;

/// The keys quota static the service owns, indexed for the sysctl rows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyQuota {
    RootMaxKeys,
    RootMaxBytes,
    MaxKeys,
    MaxBytes,
}

/// The ipc hook set: every lib-side consumer of the movable ipc policy.
#[derive(Clone, Copy)]
pub struct IpcHooks {
    // ---- process exit (do_exit's cleanup pass) ----
    /// Release the exiting task's POSIX MQ fd entries (posix_mq).
    /// (sysv_sem's sem_undo_exit stays a DIRECT framework call — the
    /// file's own shelving verdict.)
    pub mq_fds_cleanup: fn(&Task),

    // ---- exec ----
    /// execve clears the thread keyring (keys).
    pub exec_clear_thread_keyring: fn(),

    // ---- fork: access_token inheritance (access_token) ----
    pub at_registered_token_of: fn(u32) -> Option<(u64, u64)>,
    pub at_mirror_set: fn(u32, bool, u64),

    // ---- /proc/sys rows (sysctl) ----
    pub msgmni: fn() -> i32,
    pub set_msgmni: fn(i32) -> bool,
    pub shm_ctlmax: fn() -> u64,
    pub set_shm_ctlmax: fn(u64),
    pub shm_ctlni: fn() -> u64,
    /// The service caps the stored value at its registry bound
    /// (IPC_IDS_MAX), preserving the historical sysctl clamp.
    pub set_shm_ctlni: fn(u64),
    pub shm_ctlall: fn() -> u64,
    pub set_shm_ctlall: fn(u64),
    /// The registry bound the ctlni clamp uses (sysctl display).
    pub ipc_ids_max: fn() -> u64,
    pub queues_max: fn() -> i32,
    pub set_queues_max: fn(i32) -> bool,
    pub key_quota: fn(KeyQuota) -> i32,
    pub set_key_quota: fn(KeyQuota, i32),

    // ---- /proc/sysvipc rows (procfs) ----
    pub msg_proc_show: fn() -> Vec<u8>,
    pub shm_proc_show: fn() -> Vec<u8>,

    // ---- mm VMA walk rows (fork inheritance + exit teardown) ----
    /// A SharedMemory VMA is being inherited by fork: bump nattch.
    pub shm_attach_vma: fn(i32),
    /// A SharedMemory VMA is being torn down with the mm: detach.
    pub shm_detach_vma: fn(i32),

    // ---- devfs: the access_token char device registration ----
    /// The char-device number + ops static for /dev/access_token_id
    /// (None until the service registers; devfs skips the node then —
    /// the composed kernel always registers).
    pub at_char_device: fn() -> Option<(DevNo, &'static FileOps)>,

    // ---- binder's BINDER_GET_SENDER_INFO consult (cold ioctl) ----
    /// The sender's (token, ftoken) pair (binder.rs consults the
    /// access_token registry through this row; (0, 0) = no token).
    pub at_token_of: fn(u32) -> (u64, u64),

    // ---- boot ----
    /// The ipc policy's boot-time init (const-initialized registries;
    /// kept as a row so future init work rides the seam).
    pub init: fn(),
}

impl IpcHooks {
    pub const EMPTY: IpcHooks = IpcHooks {
        mq_fds_cleanup: |_| {},
        exec_clear_thread_keyring: || {},
        at_registered_token_of: |_| None,
        at_mirror_set: |_, _, _| {},
        msgmni: || 0,
        set_msgmni: |_| false,
        shm_ctlmax: || 0,
        set_shm_ctlmax: |_| {},
        shm_ctlni: || 0,
        set_shm_ctlni: |_| {},
        shm_ctlall: || 0,
        set_shm_ctlall: |_| {},
        ipc_ids_max: || 0,
        queues_max: || 0,
        set_queues_max: |_| false,
        key_quota: |_| 0,
        set_key_quota: |_, _| {},
        msg_proc_show: || Vec::new(),
        shm_proc_show: || Vec::new(),
        shm_attach_vma: |_| {},
        shm_detach_vma: |_| {},
        at_char_device: || None,
        at_token_of: |_| (0, 0),
        init: || {},
    };
}

pub static IPC_HOOKS: crate::safety::slot::OnceSlot<IpcHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (called by the ipc service crate at boot).
pub fn ipc_register_hooks(hooks: IpcHooks) {
    let _ = IPC_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> IpcHooks {
    *IPC_HOOKS.get().unwrap_or(&IpcHooks::EMPTY)
}

// The lib-side call surface (one wrapper per row; the framework call
// sites name these, never the table).

#[inline]
pub fn mq_fds_cleanup(task: &Task) {
    (hooks().mq_fds_cleanup)(task)
}
#[inline]
pub fn exec_clear_thread_keyring() {
    (hooks().exec_clear_thread_keyring)()
}
#[inline]
pub fn at_registered_token_of(pid: u32) -> Option<(u64, u64)> {
    (hooks().at_registered_token_of)(pid)
}
#[inline]
pub fn at_mirror_set(pid: u32, set_ftoken: bool, v: u64) {
    (hooks().at_mirror_set)(pid, set_ftoken, v)
}
#[inline]
pub fn msgmni() -> i32 {
    (hooks().msgmni)()
}
#[inline]
pub fn set_msgmni(v: i32) -> bool {
    (hooks().set_msgmni)(v)
}
#[inline]
pub fn shm_ctlmax() -> u64 {
    (hooks().shm_ctlmax)()
}
#[inline]
pub fn set_shm_ctlmax(v: u64) {
    (hooks().set_shm_ctlmax)(v)
}
#[inline]
pub fn shm_ctlni() -> u64 {
    (hooks().shm_ctlni)()
}
#[inline]
pub fn set_shm_ctlni(v: u64) {
    (hooks().set_shm_ctlni)(v)
}
#[inline]
pub fn shm_ctlall() -> u64 {
    (hooks().shm_ctlall)()
}
#[inline]
pub fn set_shm_ctlall(v: u64) {
    (hooks().set_shm_ctlall)(v)
}
#[inline]
pub fn ipc_ids_max() -> u64 {
    (hooks().ipc_ids_max)()
}
#[inline]
pub fn queues_max() -> i32 {
    (hooks().queues_max)()
}
#[inline]
pub fn set_queues_max(v: i32) -> bool {
    (hooks().set_queues_max)(v)
}
#[inline]
pub fn key_quota(q: KeyQuota) -> i32 {
    (hooks().key_quota)(q)
}
#[inline]
pub fn set_key_quota(q: KeyQuota, v: i32) {
    (hooks().set_key_quota)(q, v)
}
#[inline]
pub fn msg_proc_show() -> Vec<u8> {
    (hooks().msg_proc_show)()
}
#[inline]
pub fn shm_proc_show() -> Vec<u8> {
    (hooks().shm_proc_show)()
}
#[inline]
pub fn shm_attach_vma(fd: i32) {
    (hooks().shm_attach_vma)(fd)
}
#[inline]
pub fn shm_detach_vma(shmid: i32) {
    (hooks().shm_detach_vma)(shmid)
}
#[inline]
pub fn at_char_device() -> Option<(DevNo, &'static FileOps)> {
    (hooks().at_char_device)()
}
#[inline]
pub fn at_token_of(pid: u32) -> (u64, u64) {
    (hooks().at_token_of)(pid)
}
#[inline]
pub fn init() {
    (hooks().init)()
}
