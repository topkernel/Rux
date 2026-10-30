//! System V Message Queues
//!
//! Implements msgget, msgctl, msgsnd, msgrcv following the Linux kernel design.

use crate::arch::uaccess::{access_ok, copy_from_user, copy_to_user, get_user, put_user};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};

use super::util::*;

// Compile-time verification: IpcPermUapi mode field offset must be 20 bytes
const _: () = assert!(core::mem::offset_of!(IpcPermUapi, mode) == 20);

// ============================================================================
// UAPI Structures
// ============================================================================

/// struct msginfo — returned by IPC_INFO / MSG_INFO
/// Must match Linux's include/uapi/linux/msg.h. Total: 30 bytes.
#[repr(C)]
pub struct MsgInfoUapi {
    pub msgpool: i32,  // +0  pool pages
    pub msgmap: i32,   // +4  pool entries
    pub msgmax: i32,   // +8  max message size
    pub msgmnb: i32,   // +12 max bytes on queue
    pub msgmni: i32,   // +16 max queues
    pub msgssz: i32,   // +20 message segment size
    pub msgtql: i32,   // +24 max messages system-wide
    pub msgseg: u16,   // +28 number of segments
}

/// struct msqid64_ds — returned by IPC_STAT, IPC_SET
/// Must match asm-generic/msgbuf.h for RV64. Total: 120 bytes.
#[repr(C)]
pub struct MsqidDsUapi {
    pub msg_perm: IpcPermUapi,
    pub msg_stime: i64,
    pub msg_rtime: i64,
    pub msg_ctime: i64,
    pub __msg_cbytes: u64,
    pub msg_qnum: u64,
    pub msg_qbytes: u64,
    pub msg_lspid: u32,
    pub msg_lrpid: u32,
    pub __unused4: u64,
    pub __unused5: u64,
}

// ============================================================================
// Kernel Structures
// ============================================================================

/// A single message in the queue.
struct Msg {
    /// Message type (must be > 0).
    mtype: i64,
    /// Message data payload.
    data: alloc::vec::Vec<u8>,
}

/// Message queue (the IPC object).
pub struct MsgQueue {
    pub perm: KernIpcPerm,
    /// Messages in the queue.
    messages: Spinlock<alloc::vec::Vec<Msg>>,
    /// Current total bytes in queue.
    cbytes: AtomicUsize,
    /// Maximum bytes allowed in queue.
    qbytes: AtomicUsize,
    /// Number of messages currently in queue.
    qnum: AtomicUsize,
    /// Time of last msgsnd.
    msg_stime: AtomicI64,
    /// Time of last msgrcv.
    msg_rtime: AtomicI64,
    /// Time of last msgctl that changed the queue.
    msg_ctime: AtomicI64,
    /// PID of last msgsnd.
    msg_lspid: AtomicU32,
    /// PID of last msgrcv.
    msg_lrpid: AtomicU32,
    /// Wait queue for senders (queue full).
    wq_send: WaitQueueHead,
    /// Wait queue for receivers (queue empty).
    wq_recv: WaitQueueHead,
}

impl IpcObject for MsgQueue {
    fn get_perm(&self) -> &KernIpcPerm {
        &self.perm
    }
    fn get_perm_mut(&mut self) -> &mut KernIpcPerm {
        &mut self.perm
    }
}

impl MsgQueue {
    fn new(key: i32, mode: u16) -> Self {
        Self {
            perm: KernIpcPerm::new(key, mode),
            messages: Spinlock::new(alloc::vec::Vec::new()),
            cbytes: AtomicUsize::new(0),
            qbytes: AtomicUsize::new(16 * 1024), // default 16KB
            qnum: AtomicUsize::new(0),
            msg_stime: AtomicI64::new(0),
            msg_rtime: AtomicI64::new(0),
            msg_ctime: AtomicI64::new(ipc_current_time()),
            msg_lspid: AtomicU32::new(0),
            msg_lrpid: AtomicU32::new(0),
            wq_send: WaitQueueHead::new(),
            wq_recv: WaitQueueHead::new(),
        }
    }
}

// ============================================================================
// Global message queue registry
// ============================================================================

fn get_current_pid() -> u32 {
    crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0)
}

static MSG_IDS: IpcIds<MsgQueue> = IpcIds::new();

/// Runtime MSGMNI — number of message queues allowed system-wide. Writable
/// through /proc/sys/kernel/msgmni (LTP msgget03 lowers it to a small value
/// and expects the very next msgget to fail with ENOSPC). Capped by the
/// slot-array size IPC_IDS_MAX.
static MSGMNI: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(256);

/// Read the runtime MSGMNI limit (procfs sysctl + IPC_INFO/MSG_INFO).
pub fn msgmni() -> i32 {
    MSGMNI.load(Ordering::Relaxed)
}

/// Set the runtime MSGMNI limit (procfs sysctl write). Returns false when the
/// requested value is out of range.
pub fn set_msgmni(v: i32) -> bool {
    if v < 1 || v > IPC_IDS_MAX as i32 {
        return false;
    }
    MSGMNI.store(v, Ordering::Relaxed);
    true
}

/// /proc/sysvipc/msg contents — one line per queue, Linux ipc/util.c
/// formatting. LTP's GET_USED_QUEUES() (msgget03, msgstress01) counts lines
/// to learn how many queues exist.
pub fn proc_show() -> alloc::vec::Vec<u8> {
    use alloc::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = writeln!(
        out,
        "       key      msqid perms      uid      gid  cuid   cgid  stime  rtime  ctime       cbytes       qnum qbytes lspid lrpid"
    );
    let slots = MSG_IDS.slots.lock();
    for (i, slot) in slots.iter().enumerate() {
        if let Some(ref entry) = slot {
            if entry.deleted {
                continue;
            }
            let q = &entry.inner;
            let id = ipc_build_id(i, q.perm.seq);
            let _ = writeln!(
                out,
                "{:10} {:10} {:6o} {:8} {:8} {:6} {:6} {:6} {:6} {:6} {:12} {:12} {:6} {:6} {:6}",
                q.perm.key,
                id,
                q.perm.mode & 0o7777,
                q.perm.uid,
                q.perm.gid,
                q.perm.cuid,
                q.perm.cgid,
                q.msg_stime.load(Ordering::Relaxed),
                q.msg_rtime.load(Ordering::Relaxed),
                q.msg_ctime.load(Ordering::Relaxed),
                q.cbytes.load(Ordering::Relaxed),
                q.qnum.load(Ordering::Relaxed),
                q.qbytes.load(Ordering::Relaxed),
                q.msg_lspid.load(Ordering::Relaxed),
                q.msg_lrpid.load(Ordering::Relaxed),
            );
        }
    }
    out.into_bytes()
}

/// Highest allocated slot index, or -1 when the table is empty — Linux's
/// ipc_get_maxidx(). Returned by IPC_INFO/MSG_INFO; LTP passes the value
/// straight into MSG_STAT (raw index), so the +1 the old code added made
/// every subsequent MSG_STAT hit an empty slot (EINVAL).
fn msg_max_idx() -> i32 {
    let slots = MSG_IDS.slots.lock();
    for i in (0..IPC_IDS_MAX).rev() {
        if slots[i].is_some() {
            return i as i32;
        }
    }
    -1
}

// ============================================================================
// Syscall Implementations
// ============================================================================

/// sys_msgget — Create or find a message queue (NR 186)
pub fn sys_msgget(args: [u64; 6]) -> i64 {
    let key = args[0] as i32;
    let msgflg = args[1] as i32;

    // ENOSPC at the runtime MSGMNI (Linux msgget checks ids->in_use >=
    // ns->msg_ctlmni inside newque — i.e. only on the create path). An
    // existing key still resolves, and a bare lookup on a nonexistent key
    // is ENOENT (alloc answers that under the table lock); only a queue
    // that would actually be created hits the limit.
    if MSG_IDS.count() >= msgmni() as usize {
        let existing = if key != 0 { MSG_IDS.find_by_key(key) } else { None };
        let would_create = existing.is_none() && (key == 0 || (msgflg & IPC_CREAT) != 0);
        if would_create {
            return -(errno::ENOSPC as i64);
        }
    }
    match MSG_IDS.alloc(MsgQueue::new(key, (msgflg & 0o777) as u16), key, msgflg) {
        Ok((id, _)) => id as i64,
        Err(e) => e as i64,
    }
}

/// sys_msgctl — Message queue control operations (NR 187)
pub fn sys_msgctl(args: [u64; 6]) -> i64 {
    let msqid = args[0] as i32;
    let cmd = args[1] as i32;
    let buf = args[2];

    // MSG_INFO-style commands interpret the first argument as a raw kernel
    // slot index (MSG_STAT) or ignore it entirely (IPC_INFO/MSG_INFO), so
    // they must NOT go through the id→slot lookup. The old unconditional
    // find() gate rejected msgctl(0, IPC_INFO) with EINVAL before the cmd
    // match ever ran (LTP msgctl12, shmctl03's msg twin).
    match cmd {
        IPC_INFO | 11 | 12 | 13 => {}
        _ => {
            let _ = match MSG_IDS.find(msqid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
        }
    }

    match cmd {
        IPC_RMID => {
            // Owner check (review IPC P2): uid OR cuid path, or CAP_SYS_ADMIN.
            // Linux does no mode-bit check for RMID (no EACCES path exists).
            let idx = match MSG_IDS.find(msqid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    let cred = crate::sched::current().map(|t| t.cred());
                    let allowed = match cred {
                        Some(ref c) => {
                            c.euid == entry.inner.perm.uid
                                || c.euid == entry.inner.perm.cuid
                                || crate::security::capable(crate::security::CAP_SYS_ADMIN)
                        }
                        None => false,
                    };
                    if !allowed {
                        return -(errno::EPERM as i64);
                    }
                }
            }
            // Wake all blocked senders/receivers before destroying
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    entry.inner.wq_send.wake_up_all();
                    entry.inner.wq_recv.wake_up_all();
                }
            }
            let _ = MSG_IDS.remove(msqid);
            MSG_IDS.free_slot(msqid);
            0
        }
        IPC_STAT => {
            // R32 (NEW-4 twin): Linux requires S_IRUGO for IPC_STAT — the
            // old path used the perm-free find() and leaked the queue's
            // metadata to any caller.
            let idx2 = match MSG_IDS.find_with_perms(msqid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let buf_ptr = buf as *mut MsqidDsUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<MsqidDsUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let mut ds = MsqidDsUapi {
                msg_perm: IpcPermUapi::default(),
                msg_stime: 0,
                msg_rtime: 0,
                msg_ctime: 0,
                __msg_cbytes: 0,
                msg_qnum: 0,
                msg_qbytes: 0,
                msg_lspid: 0,
                msg_lrpid: 0,
                __unused4: 0,
                __unused5: 0,
            };
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx2] {
                    ds.msg_perm = entry.inner.perm.to_uapi();
                    ds.msg_stime = entry.inner.msg_stime.load(Ordering::Relaxed);
                    ds.msg_rtime = entry.inner.msg_rtime.load(Ordering::Relaxed);
                    ds.msg_ctime = entry.inner.msg_ctime.load(Ordering::Relaxed);
                    ds.__msg_cbytes = entry.inner.cbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_qnum = entry.inner.qnum.load(Ordering::Relaxed) as u64;
                    ds.msg_qbytes = entry.inner.qbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_lspid = entry.inner.msg_lspid.load(Ordering::Relaxed);
                    ds.msg_lrpid = entry.inner.msg_lrpid.load(Ordering::Relaxed);
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for size_of::<MsqidDsUapi>() above;
            // ds is a stack-local copy of the message queue metadata.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const MsqidDsUapi as *const u8,
                    core::mem::size_of::<MsqidDsUapi>(),
                );
            }
            0
        }
        IPC_SET => {
            let buf_ptr = buf as *const u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<MsqidDsUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let idx2 = match MSG_IDS.find(msqid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
            // Owner check (review IPC P2): euid == uid || cuid, or
            // CAP_SYS_ADMIN. Linux runs NO mode-bit check for IPC_SET, so a
            // non-owner with full rw mode still gets EPERM, never EACCES.
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx2] {
                    let cred = crate::sched::current().map(|t| t.cred());
                    let allowed = match cred {
                        Some(ref c) => {
                            c.euid == entry.inner.perm.uid
                                || c.euid == entry.inner.perm.cuid
                                || crate::security::capable(crate::security::CAP_SYS_ADMIN)
                        }
                        None => false,
                    };
                    if !allowed {
                        return -(errno::EPERM as i64);
                    }
                }
            }
            // Struct copy through the exception-table path (review IPC M).
            let mut ds = MsqidDsUapi {
                msg_perm: IpcPermUapi::default(),
                msg_stime: 0,
                msg_rtime: 0,
                msg_ctime: 0,
                __msg_cbytes: 0,
                msg_qnum: 0,
                msg_qbytes: 0,
                msg_lspid: 0,
                msg_lrpid: 0,
                __unused4: 0,
                __unused5: 0,
            };
            // SAFETY: buf_ptr was access_ok-validated above; ds is a
            // stack-local repr(C) struct of exactly that size.
            if unsafe {
                crate::arch::uaccess::copy_from_user(
                    &mut ds as *mut MsqidDsUapi as *mut u8,
                    buf_ptr,
                    core::mem::size_of::<MsqidDsUapi>(),
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            let mut slots = MSG_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                let new_uid = ds.msg_perm.uid;
                let new_gid = ds.msg_perm.gid;
                let new_mode = ds.msg_perm.mode;
                entry.inner.perm.update_from_set(new_uid, new_gid, new_mode);
                // Per reference msgctl IPC_SET: msg_qbytes is stored
                // UNCONDITIONALLY (0 is a legal value that blocks all
                // sends — the old >0 gate silently ignored it), and raising
                // it above the system default (MSGMNB, 16384) requires
                // CAP_SYS_RESOURCE.
                let new_qbytes = ds.msg_qbytes;
                if new_qbytes > 16384
                    && !crate::security::capable(crate::security::CAP_SYS_RESOURCE)
                {
                    return -(errno::EPERM as i64);
                }
                entry.inner.qbytes.store(new_qbytes as usize, Ordering::Relaxed);
                entry.inner.msg_ctime.store(ipc_current_time(), Ordering::Relaxed);
            }
            0
        }
        IPC_INFO => {
            // struct msginfo — 7 int + 1 unsigned short = 30 bytes
            let buf_ptr = buf as *mut MsgInfoUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<MsgInfoUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let info = MsgInfoUapi {
                msgpool: 0,
                msgmap: 0,
                msgmax: 8192,
                msgmnb: 16384,
                msgmni: msgmni(),
                msgssz: 16,
                msgtql: 65536,
                msgseg: 0xffff,
            };
            // SAFETY: buf_ptr was null-checked and access_ok-validated above.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &info as *const MsgInfoUapi as *const u8,
                    core::mem::size_of::<MsgInfoUapi>(),
                );
            }
            // Return: highest used slot index (Linux ipc_get_maxidx);
            // -1 when empty. LTP feeds this value straight into MSG_STAT.
            msg_max_idx() as i64
        }
        12 => {
            // MSG_INFO — like IPC_INFO but returns current usage
            let buf_ptr = buf as *mut MsgInfoUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<MsgInfoUapi>()) {
                return -(errno::EFAULT as i64);
            }
            // Count total messages across all queues
            let mut total_msgs: usize = 0;
            {
                let slots = MSG_IDS.slots.lock();
                for entry in slots.iter() {
                    if let Some(ref e) = entry {
                        if !e.deleted {
                            total_msgs += e.inner.qnum.load(Ordering::Relaxed);
                        }
                    }
                }
            }
            let info = MsgInfoUapi {
                msgpool: MSG_IDS.count() as i32,
                msgmap: total_msgs as i32,
                msgmax: 8192,
                msgmnb: 16384,
                msgmni: msgmni(),
                msgssz: 16,
                msgtql: 65536,
                msgseg: 0xffff,
            };
            // SAFETY: buf_ptr was null-checked and access_ok-validated above.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &info as *const MsgInfoUapi as *const u8,
                    core::mem::size_of::<MsgInfoUapi>(),
                );
            }
            // Return: highest used slot index, -1 when empty (see IPC_INFO).
            msg_max_idx() as i64
        }
        13 => {
            // MSG_STAT_ANY — MSG_STAT without the permission check (uapi:
            // MSG_STAT=11, MSG_INFO=12, MSG_STAT_ANY=13; the old arm at 13
            // implemented MSG_STAT and the real MSG_STAT(11) fell through
            // to EINVAL — LTP msgctl12 via libc msgctl(MSG_STAT=11)).
            // The reference msgctl_stat performs NO capability check for
            // STAT_ANY (it exists precisely so unprivileged checkpoint/
            // restore can read queue state); the old CAP_SYS_ADMIN gate
            // wrongly returned EPERM.
            let raw_idx = msqid as usize;
            let buf_ptr = buf as *mut MsqidDsUapi;
            if buf_ptr.is_null()
                || !access_ok(buf_ptr as usize, core::mem::size_of::<MsqidDsUapi>())
            {
                return -(errno::EFAULT as i64);
            }
            if raw_idx >= IPC_IDS_MAX {
                return -(errno::EINVAL as i64);
            }
            let mut ds = MsqidDsUapi {
                msg_perm: IpcPermUapi::default(),
                msg_stime: 0,
                msg_rtime: 0,
                msg_ctime: 0,
                __msg_cbytes: 0,
                msg_qnum: 0,
                msg_qbytes: 0,
                msg_lspid: 0,
                msg_lrpid: 0,
                __unused4: 0,
                __unused5: 0,
            };
            let result_id: i32;
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[raw_idx] {
                    if entry.deleted {
                        return -(errno::EINVAL as i64);
                    }
                    // MSG_STAT_ANY skips the permission check entirely.
                    ds.msg_perm = entry.inner.perm.to_uapi();
                    ds.msg_stime = entry.inner.msg_stime.load(Ordering::Relaxed);
                    ds.msg_rtime = entry.inner.msg_rtime.load(Ordering::Relaxed);
                    ds.msg_ctime = entry.inner.msg_ctime.load(Ordering::Relaxed);
                    ds.__msg_cbytes = entry.inner.cbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_qnum = entry.inner.qnum.load(Ordering::Relaxed) as u64;
                    ds.msg_qbytes = entry.inner.qbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_lspid = entry.inner.msg_lspid.load(Ordering::Relaxed);
                    ds.msg_lrpid = entry.inner.msg_lrpid.load(Ordering::Relaxed);
                    result_id = ipc_build_id(raw_idx, entry.inner.perm.seq);
                } else {
                    return -(errno::EINVAL as i64);
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated above;
            // ds is a stack-local repr(C) struct of exactly that size.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const MsqidDsUapi as *const u8,
                    core::mem::size_of::<MsqidDsUapi>(),
                );
            }
            result_id as i64
        }
        11 => {
            // MSG_STAT — IPC_STAT addressed by raw kernel slot index (no
            // seq); on success returns the real msqid. S_IRUGO applies.
            let raw_idx = msqid as usize;
            let buf_ptr = buf as *mut MsqidDsUapi;
            if buf_ptr.is_null()
                || !access_ok(buf_ptr as usize, core::mem::size_of::<MsqidDsUapi>())
            {
                return -(errno::EFAULT as i64);
            }
            if raw_idx >= IPC_IDS_MAX {
                return -(errno::EINVAL as i64);
            }
            let mut ds = MsqidDsUapi {
                msg_perm: IpcPermUapi::default(),
                msg_stime: 0,
                msg_rtime: 0,
                msg_ctime: 0,
                __msg_cbytes: 0,
                msg_qnum: 0,
                msg_qbytes: 0,
                msg_lspid: 0,
                msg_lrpid: 0,
                __unused4: 0,
                __unused5: 0,
            };
            let result_id: i32;
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[raw_idx] {
                    if entry.deleted {
                        return -(errno::EINVAL as i64);
                    }
                    if !ipc_check_permissions(entry.inner.get_perm(), 0o4) {
                        return -(errno::EACCES as i64);
                    }
                    ds.msg_perm = entry.inner.perm.to_uapi();
                    ds.msg_stime = entry.inner.msg_stime.load(Ordering::Relaxed);
                    ds.msg_rtime = entry.inner.msg_rtime.load(Ordering::Relaxed);
                    ds.msg_ctime = entry.inner.msg_ctime.load(Ordering::Relaxed);
                    ds.__msg_cbytes = entry.inner.cbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_qnum = entry.inner.qnum.load(Ordering::Relaxed) as u64;
                    ds.msg_qbytes = entry.inner.qbytes.load(Ordering::Relaxed) as u64;
                    ds.msg_lspid = entry.inner.msg_lspid.load(Ordering::Relaxed);
                    ds.msg_lrpid = entry.inner.msg_lrpid.load(Ordering::Relaxed);
                    result_id = ipc_build_id(raw_idx, entry.inner.perm.seq);
                } else {
                    return -(errno::EINVAL as i64);
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated above;
            // ds is a stack-local repr(C) struct of exactly that size.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const MsqidDsUapi as *const u8,
                    core::mem::size_of::<MsqidDsUapi>(),
                );
            }
            result_id as i64
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_msgsnd — Send a message to a queue (NR 189)
pub fn sys_msgsnd(args: [u64; 6]) -> i64 {
    let msqid = args[0] as i32;
    let msgp = args[1] as *const u8;
    let msgsz = args[2] as usize;
    let msgflg = args[3] as i32;

    if msgp.is_null() {
        return -(errno::EFAULT as i64);
    }
    // Message size must be >= 0
    if msgsz > 8192 {
        return -(errno::EINVAL as i64);
    }

    // Validate the full buffer (mtype header + data) before reading anything.
    if !access_ok(msgp as usize, msgsz + 8) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: msgp was null-checked and access_ok-validated above; get_user
    // is the exception-table copy path (SUM=0 safe). An unreadable head must
    // be EFAULT — the old unwrap_or(0) turned a PROT_NONE page into
    // mtype=0 and reported EINVAL (reference ksys_msgsnd: get_user failure
    // is EFAULT before any EINVAL on the type).
    let mtype = match unsafe { get_user(msgp as *const i64) } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    if mtype <= 0 {
        return -(errno::EINVAL as i64);
    }

    // SAFETY: msgp + 8 is within the access_ok-validated region.
    let data_ptr = unsafe { msgp.add(8) };
    let mut data = alloc::vec::Vec::with_capacity(msgsz);
    data.resize(msgsz, 0);
    // SAFETY: data_ptr was access_ok-validated for msgsz bytes above;
    // data is a Vec with capacity msgsz, so the destination is valid.
    // The return value (uncopied byte count) MUST be checked — a partial
    // copy would zero-fill the tail and corrupt the message (review IPC M:
    // msgsnd 忽略 copy_from_user 返回值).
    if unsafe { copy_from_user(data.as_mut_ptr(), data_ptr, msgsz) } != 0 {
        return -(errno::EFAULT as i64);
    }

    let idx = match MSG_IDS.find_with_perms(msqid, 0o2) {
        Ok(i) => i,
        Err(e) => return e as i64,
    };

    let nowait = (msgflg & IPC_NOWAIT) != 0;

    loop {
        // Check if queue has space
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                // R10-7: seq revalidation at the LOOP TOP too — the
                // post-registration recheck alone still trusted a stale
                // idx after a wake from RMID+recreate.
                if !entry.deleted
                    && super::util::ipc_id_seq(msqid) == entry.inner.get_perm().seq
                {
                    let cbytes = entry.inner.cbytes.load(Ordering::Relaxed);
                    let qbytes = entry.inner.qbytes.load(Ordering::Relaxed);
                    if cbytes + msgsz <= qbytes {
                        // Space available — insert message
                        let mut messages = entry.inner.messages.lock();
                        messages.push(Msg {
                            mtype,
                            data: core::mem::take(&mut data),
                        });
                        entry.inner.cbytes.fetch_add(msgsz, Ordering::Relaxed);
                        entry.inner.qnum.fetch_add(1, Ordering::Relaxed);
                        entry.inner.msg_stime.store(ipc_current_time(), Ordering::Relaxed);
                        entry.inner.msg_lspid.store(get_current_pid(), Ordering::Relaxed);
                        // Wake up receivers
                        entry.inner.wq_recv.wake_up_all();
                        return 0;
                    }
                } else {
                    return -(errno::EIDRM as i64);
                }
            } else {
                // Slot vanished mid-loop: an RMID freed it after waking us
                // — EIDRM, not EINVAL (review IPC L: RMID 后 EIDRM 口径).
                return -(errno::EIDRM as i64);
            }
        }

        // No space
        if nowait {
            return -(errno::EAGAIN as i64);
        }

        // Check for signals
        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        // Block on wq_send
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -(errno::EIDRM as i64);
                }
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -(errno::ESRCH as i64),
                };
                // R39: prepare_to_wait registers the entry AND sets
                // INTERRUPTIBLE atomically under the WQ's own lock. The old
                // add()+set_state() pair ran under the MSG_IDS slots lock —
                // but the WAKER takes the WQ lock, not the slots lock, so a
                // wake landing between the two calls marked our entry woken
                // and was DROPPED (Task::wake_up saw RUNNING): for an RMID
                // wake the one-shot token was consumed with the queue going
                // away, and the sleeper blocked forever (signal-only exit).
                entry.inner.wq_send.prepare_to_wait(current as *mut _, false, true);
            }
        }

        // R7-B6: re-check space AFTER registering on wq_send — a receiver
        // that freed space between the space check and the registration
        // already ran wake_up_all on an empty queue (lost wakeup).
        {
            let mut sent = false;
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    // R9-11: seq revalidation — the slot may have been
                    // RMID'd and re-created since our id was captured.
                    if !entry.deleted && super::util::ipc_id_seq(msqid) == entry.inner.get_perm().seq {
                        let cbytes = entry.inner.cbytes.load(Ordering::Relaxed);
                        let qbytes = entry.inner.qbytes.load(Ordering::Relaxed);
                        if cbytes + msgsz <= qbytes {
                            let mut messages = entry.inner.messages.lock();
                            messages.push(Msg { mtype, data: core::mem::take(&mut data) });
                            entry.inner.cbytes.fetch_add(msgsz, Ordering::Relaxed);
                            entry.inner.qnum.fetch_add(1, Ordering::Relaxed);
                            entry.inner.msg_stime.store(ipc_current_time(), Ordering::Relaxed);
                            entry.inner.msg_lspid.store(get_current_pid(), Ordering::Relaxed);
                            entry.inner.wq_recv.wake_up_all();
                            sent = true;
                        }
                    }
                }
            }
            if sent {
                let current = crate::sched::current().unwrap();
                {
                    let slots = MSG_IDS.slots.lock();
                    if let Some(ref entry) = slots[idx] {
                        entry.inner.wq_send.remove(current as *mut _);
                    }
                }
                unsafe {
                    (*current).set_state(
                        crate::process::task::TaskState::new(
                            crate::process::task::TaskState::RUNNING,
                        ),
                    );
                }
                crate::sched::dequeue_task(&*current);
                return 0;
            }
        }

        // R22-1: signal delivered while still RUNNING → no wakeup; recheck
        // or the blocked send/receive is unkillable.
        if crate::signal::signal_pending() {
            let cur = crate::sched::current().unwrap();
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    entry.inner.wq_send.remove(cur as *mut _);
                    entry.inner.wq_recv.remove(cur as *mut _);
                }
            }
            (*cur).set_state(crate::process::task::TaskState::new(crate::process::task::TaskState::RUNNING));
            crate::sched::dequeue_task(&*cur);
            return -(errno::EINTR as i64);
        }
        crate::sched::schedule();

        // Clean up wait queue entry after wakeup
        {
            let current = crate::sched::current().unwrap();
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                entry.inner.wq_send.remove(current as *mut _);
            }
        }
    }
}
pub fn sys_msgrcv(args: [u64; 6]) -> i64 {
    let msqid = args[0] as i32;
    let msgp = args[1] as *mut u8;
    let msgsz = args[2] as usize;
    let msgtyp = args[3] as i64;
    let msgflg = args[4] as i32;

    if msgp.is_null() {
        return -(errno::EFAULT as i64);
    }
    // Negative (sign-extended, e.g. -1 → SIZE_MAX) or otherwise absurd
    // sizes are EINVAL (Linux do_msgrcv bounds msgsz; LTP msgrcv02 feeds
    // -1). The old `msgsz + 8` access check PANICKED on overflow in debug
    // builds and took the whole kernel down mid-sweep.
    if (msgsz as isize) < 0 {
        return -(errno::EINVAL as i64);
    }
    if !access_ok(msgp as usize, msgsz.saturating_add(8)) {
        return -(errno::EFAULT as i64);
    }

    let idx = match MSG_IDS.find_with_perms(msqid, 0o4) {
        Ok(i) => i,
        Err(e) => return e as i64,
    };

    let nowait = (msgflg & IPC_NOWAIT) != 0;
    let msg_noerror = (msgflg & MSG_NOERROR) != 0;

    let msg_copy = (msgflg & super::util::MSG_COPY) != 0;

    // MSG_COPY (reference CHECKPOINT_RESTORE semantics): msgtyp is the ORDINAL
    // position in the queue (0-based), NOT a type filter. The reference
    // requires IPC_NOWAIT and rejects MSG_EXCEPT with EINVAL (copy mode never
    // blocks); negative ordinals are EINVAL (review IPC M: MSG_COPY 仅队首).
    if msg_copy {
        if (msgflg & super::util::MSG_EXCEPT) != 0 || (msgflg & IPC_NOWAIT) == 0 {
            return -(errno::EINVAL as i64);
        }
        if msgtyp < 0 {
            return -(errno::EINVAL as i64);
        }
    }

    loop {
        // Try to find a matching message
        let result = {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -(errno::EIDRM as i64);
                }
                // R10-7: seq revalidation at the loop top (RMID+slot reuse).
                if super::util::ipc_id_seq(msqid) != entry.inner.get_perm().seq {
                    return -(errno::EIDRM as i64);
                }
                let mut messages = entry.inner.messages.lock();
                // MSG_COPY: treat msgtyp as the ordinal index into the queue.
                let match_idx = if msg_copy {
                    let ord = msgtyp as usize;
                    if ord < messages.len() {
                        Some(ord)
                    } else {
                        None
                    }
                } else {
                    find_msg_match(&messages, msgtyp, msgflg)
                };

                if let Some(mi) = match_idx {
                    // Size check BEFORE unlink (reference do_msgrcv): a
                    // too-small buffer must leave the queue untouched — no
                    // remove/re-insert churn, no rtime/lrpid update.
                    let head_len = messages[mi].data.len();
                    if head_len > msgsz && !msg_noerror {
                        return -(errno::E2BIG as i64);
                    }

                    if msg_copy {
                        // MSG_COPY: non-destructive read — copy without removing
                        let msg = &messages[mi];
                        // Copy mtype (8 bytes)
                        // SAFETY: msgp was access_ok-validated for msgsz+8 bytes above;
                        // put_user is the exception-table copy path.
                        unsafe {
                            let _ = put_user(msgp as *mut i64, msg.mtype);
                        }
                        // MSG_NOERROR truncates to msgsz like the normal read.
                        let msg_len = msg.data.len().min(msgsz);
                        // SAFETY: msgp+8 was access_ok-validated for msgsz bytes above;
                        // msg_len <= msgsz, so the copy is within bounds.
                        unsafe {
                            copy_to_user(
                                msgp.add(8),
                                msg.data.as_ptr(),
                                msg_len,
                            );
                        }
                        // Do NOT update stats, do NOT wake senders
                        return msg_len as i64;
                    }

                    // Normal destructive read
                    let msg = messages.remove(mi);
                    entry.inner.cbytes.fetch_sub(msg.data.len(), Ordering::Relaxed);
                    entry.inner.qnum.fetch_sub(1, Ordering::Relaxed);
                    entry.inner.msg_rtime.store(ipc_current_time(), Ordering::Relaxed);
                    entry.inner.msg_lrpid.store(get_current_pid(), Ordering::Relaxed);
                    Some((mi, msg))
                } else {
                    None
                }
            } else {
                // Slot vanished mid-loop (RMID) — EIDRM (review IPC L).
                return -(errno::EIDRM as i64);
            }
        };

        if let Some((_mi, msg)) = result {
            // Copy mtype (8 bytes)
            // SAFETY: msgp was access_ok-validated for msgsz+8 bytes above;
            // put_user is the exception-table copy path.
            unsafe {
                let _ = put_user(msgp as *mut i64, msg.mtype);
            }
            // Copy message data. The E2BIG check ran BEFORE the removal
            // above (reference do_msgrcv), so the only size case left here
            // is MSG_NOERROR truncation; the old remove-then-restore path
            // reordered the queue and updated rtime/lrpid on failure.
            let copy_len = msg.data.len().min(msgsz);
            // SAFETY: msgp+8 was access_ok-validated for msgsz bytes above;
            // copy_len <= msgsz, so the copy is within bounds.
            unsafe {
                copy_to_user(
                    msgp.add(8),
                    msg.data.as_ptr(),
                    copy_len,
                );
            }
            // Wake up senders (space freed)
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    entry.inner.wq_send.wake_up_all();
                }
            }
            return copy_len as i64;
        }

        // No matching message
        if nowait {
            return -(errno::ENOMSG as i64);
        }

        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        // Block on wq_recv
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -(errno::EIDRM as i64);
                }
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -(errno::ESRCH as i64),
                };
                // R39: atomic register + INTERRUPTIBLE under the WQ lock —
                // same lost-wakeup closure as sys_msgsnd (see the comment
                // there; the old add()+set_state pair let a waker consume
                // the one-shot token while we were still RUNNING).
                entry.inner.wq_recv.prepare_to_wait(current as *mut _, false, true);
            }
        }

        // R7-B6: re-check for a matching message AFTER registering on
        // wq_recv — a sender landing between the match scan and the
        // registration woke an empty queue (lost wakeup).
        {
            let mut retry_now = false;
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    // R9-11: seq revalidation (see sys_msgsnd).
                    if !entry.deleted && super::util::ipc_id_seq(msqid) == entry.inner.get_perm().seq {
                        let messages = entry.inner.messages.lock();
                        if find_msg_match(&messages, msgtyp, msgflg).is_some() {
                            retry_now = true;
                        }
                    }
                }
            }
            if retry_now {
                let current = crate::sched::current().unwrap();
                {
                    let slots = MSG_IDS.slots.lock();
                    if let Some(ref entry) = slots[idx] {
                        entry.inner.wq_recv.remove(current as *mut _);
                    }
                }
                unsafe {
                    (*current).set_state(
                        crate::process::task::TaskState::new(
                            crate::process::task::TaskState::RUNNING,
                        ),
                    );
                }
                crate::sched::dequeue_task(&*current);
                continue;
            }
        }

        // R22-1: signal delivered while still RUNNING → no wakeup; recheck
        // or the blocked send/receive is unkillable.
        if crate::signal::signal_pending() {
            let cur = crate::sched::current().unwrap();
            {
                let slots = MSG_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    entry.inner.wq_send.remove(cur as *mut _);
                    entry.inner.wq_recv.remove(cur as *mut _);
                }
            }
            (*cur).set_state(crate::process::task::TaskState::new(crate::process::task::TaskState::RUNNING));
            crate::sched::dequeue_task(&*cur);
            return -(errno::EINTR as i64);
        }
        crate::sched::schedule();

        // Clean up wait queue entry after wakeup
        {
            let current = crate::sched::current().unwrap();
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                entry.inner.wq_recv.remove(current as *mut _);
            }
        }
    }
}

/// Find a message matching the receive criteria.
/// - msgtyp == 0: return first message
/// - msgtyp > 0: return first message of that type
/// - msgtyp < 0: return first message with the lowest type <= |msgtyp|
fn find_msg_match(messages: &[Msg], msgtyp: i64, msgflg: i32) -> Option<usize> {
    if messages.is_empty() {
        return None;
    }

    if msgtyp == 0 {
        // Return first message
        return Some(0);
    }

    if msgtyp > 0 {
        // Return first message with matching type (or non-matching if MSG_EXCEPT)
        let except = (msgflg & super::util::MSG_EXCEPT) != 0;
        for (i, msg) in messages.iter().enumerate() {
            if except {
                if msg.mtype != msgtyp {
                    return Some(i);
                }
            } else {
                if msg.mtype == msgtyp {
                    return Some(i);
                }
            }
        }
        return None;
    }

    // msgtyp < 0: return first message with lowest type <= |msgtyp|
    let abs_type = (-msgtyp) as i64;
    let mut best_idx: Option<usize> = None;
    let mut best_type: i64 = i64::MAX;

    for (i, msg) in messages.iter().enumerate() {
        if msg.mtype <= abs_type && msg.mtype < best_type {
            best_type = msg.mtype;
            best_idx = Some(i);
        }
    }
    best_idx
}
