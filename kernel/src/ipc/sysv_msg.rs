//! System V Message Queues
//!
//! Implements msgget, msgctl, msgsnd, msgrcv following the Linux kernel design.

use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, copy_to_user};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};

use super::util::*;

// ============================================================================
// UAPI Structures
// ============================================================================

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

// ============================================================================
// Syscall Implementations
// ============================================================================

/// sys_msgget — Create or find a message queue (NR 186)
pub fn sys_msgget(args: [u64; 6]) -> u64 {
    let key = args[0] as i32;
    let msgflg = args[1] as i32;

    match MSG_IDS.alloc(MsgQueue::new(key, (msgflg & 0o777) as u16), key, msgflg) {
        Ok((id, _)) => id as u64,
        Err(e) => e as u64,
    }
}

/// sys_msgctl — Message queue control operations (NR 187)
pub fn sys_msgctl(args: [u64; 6]) -> u64 {
    let msqid = args[0] as i32;
    let cmd = args[1] as i32;
    let buf = args[2];

    let idx = match MSG_IDS.find(msqid) {
        Some(i) => i,
        None => return -errno::EINVAL as u64,
    };

    match cmd {
        IPC_RMID => {
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
            let buf_ptr = buf as *mut MsqidDsUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<MsqidDsUapi>()) {
                return -errno::EFAULT as u64;
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
                if let Some(ref entry) = slots[idx] {
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
                return -errno::EFAULT as u64;
            }
            let idx2 = match MSG_IDS.find_with_perms(msqid, 0o6) {
                Ok(i) => i,
                Err(e) => return e as u64,
            };
            let mut slots = MSG_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                // Update mode from msg_perm.offset(20) which is the mode field
                let new_mode = unsafe { core::ptr::read_volatile(buf_ptr.add(20) as *const u16) };
                entry.inner.perm.update_mode(new_mode);
                // Update qbytes from offset after msg_perm(48) + stime(8) + rtime(8) + ctime(8) = 72
                let new_qbytes = unsafe { core::ptr::read_volatile(buf_ptr.add(72) as *const u64) };
                if new_qbytes > 0 {
                    entry.inner.qbytes.store(new_qbytes as usize, Ordering::Relaxed);
                }
                entry.inner.msg_ctime.store(ipc_current_time(), Ordering::Relaxed);
            }
            0
        }
        IPC_INFO => {
            // struct msginfo — returned in the buffer
            // Fields: msgpool, msgmap, msgmax, msgmnb, msgmni, msgssz, msgtql, msgseg
            // Each unsigned long (8 bytes on RV64) = 64 bytes total
            let buf_ptr = buf as *mut u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, 64) {
                return -errno::EFAULT as u64;
            }
            unsafe { core::ptr::write_bytes(buf_ptr, 0, 64) };
            // msgmax (max message size) at offset 0*8
            unsafe { core::ptr::write_volatile(buf_ptr as *mut u64, 8192u64) };
            // msgmnb (max bytes on queue) at offset 1*8
            unsafe { core::ptr::write_volatile(buf_ptr.add(8) as *mut u64, 16384u64) };
            // msgmni (max queues) at offset 2*8
            unsafe { core::ptr::write_volatile(buf_ptr.add(16) as *mut u64, 256u64) };
            // msgssz (message segment size) at offset 3*8
            unsafe { core::ptr::write_volatile(buf_ptr.add(24) as *mut u64, 16u64) };
            // msgtql (max messages system-wide) at offset 4*8
            unsafe { core::ptr::write_volatile(buf_ptr.add(32) as *mut u64, 65536u64) };
            MSG_IDS.count() as u64
        }
        _ => -errno::EINVAL as u64,
    }
}

/// sys_msgsnd — Send a message to a queue (NR 189)
pub fn sys_msgsnd(args: [u64; 6]) -> u64 {
    let msqid = args[0] as i32;
    let msgp = args[1] as *const u8;
    let msgsz = args[2] as usize;
    let msgflg = args[3] as i32;

    if msgp.is_null() {
        return -errno::EFAULT as u64;
    }
    // Message size must be >= 0
    if msgsz > 8192 {
        return -errno::EINVAL as u64;
    }

    // Read mtype (first 8 bytes, i64)
    let mtype = unsafe { core::ptr::read_volatile(msgp as *const i64) };
    if mtype <= 0 {
        return -errno::EINVAL as u64;
    }

    // Copy message data (after the 8-byte mtype)
    let data_ptr = unsafe { msgp.add(8) };
    if !access_ok(data_ptr as usize, msgsz) {
        return -errno::EFAULT as u64;
    }
    let mut data = alloc::vec::Vec::with_capacity(msgsz);
    data.resize(msgsz, 0);
    unsafe {
        copy_from_user(data.as_mut_ptr(), data_ptr, msgsz);
    }

    let idx = match MSG_IDS.find_with_perms(msqid, 0o2) {
        Ok(i) => i,
        Err(e) => return e as u64,
    };

    let nowait = (msgflg & IPC_NOWAIT) != 0;

    loop {
        // Check if queue has space
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if !entry.deleted {
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
                    return -errno::EIDRM as u64;
                }
            } else {
                return -errno::EINVAL as u64;
            }
        }

        // No space
        if nowait {
            return -errno::EAGAIN as u64;
        }

        // Check for signals
        if crate::signal::signal_pending() {
            return -errno::EINTR as u64;
        }

        // Block on wq_send
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -errno::EIDRM as u64;
                }
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -errno::ESRCH as u64,
                };
                let wq_entry = crate::process::wait::WaitQueueEntry::new(current as *mut _, false);
                entry.inner.wq_send.add(wq_entry);

                // Set INTERRUPTIBLE while holding lock to prevent lost wakeup
                unsafe {
                    (*current).set_state(
                        crate::process::task::TaskState::new(
                            crate::process::task::TaskState::INTERRUPTIBLE,
                        ),
                    );
                }
            }
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

/// sys_msgrcv — Receive a message from a queue (NR 188)
pub fn sys_msgrcv(args: [u64; 6]) -> u64 {
    let msqid = args[0] as i32;
    let msgp = args[1] as *mut u8;
    let msgsz = args[2] as usize;
    let msgtyp = args[3] as i64;
    let msgflg = args[4] as i32;

    if msgp.is_null() {
        return -errno::EFAULT as u64;
    }
    if !access_ok(msgp as usize, msgsz + 8) {
        return -errno::EFAULT as u64;
    }

    let idx = match MSG_IDS.find_with_perms(msqid, 0o4) {
        Ok(i) => i,
        Err(e) => return e as u64,
    };

    let nowait = (msgflg & IPC_NOWAIT) != 0;
    let msg_noerror = (msgflg & MSG_NOERROR) != 0;

    loop {
        // Try to find a matching message
        let result = {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -errno::EIDRM as u64;
                }
                let mut messages = entry.inner.messages.lock();
                let match_idx = find_msg_match(&messages, msgtyp, msgflg);

                if let Some(mi) = match_idx {
                    let msg = messages.remove(mi);
                    entry.inner.cbytes.fetch_sub(msg.data.len(), Ordering::Relaxed);
                    entry.inner.qnum.fetch_sub(1, Ordering::Relaxed);
                    entry.inner.msg_rtime.store(ipc_current_time(), Ordering::Relaxed);
                    entry.inner.msg_lrpid.store(get_current_pid(), Ordering::Relaxed);
                    Some(msg)
                } else {
                    None
                }
            } else {
                return -errno::EINVAL as u64;
            }
        };

        if let Some(msg) = result {
            // Copy mtype (8 bytes)
            unsafe {
                core::ptr::write_volatile(msgp as *mut i64, msg.mtype);
            }
            // Copy message data
            let msg_len = msg.data.len();
            let copy_len = if msg_len > msgsz {
                if msg_noerror {
                    // Truncate and return success
                    msgsz
                } else {
                    // Restore the message to queue
                    let slots = MSG_IDS.slots.lock();
                    if let Some(ref entry) = slots[idx] {
                        let mut messages = entry.inner.messages.lock();
                        messages.push(msg);
                        entry.inner.cbytes.fetch_add(msg_len, Ordering::Relaxed);
                        entry.inner.qnum.fetch_add(1, Ordering::Relaxed);
                    }
                    return -errno::E2BIG as u64;
                }
            } else {
                msg.data.len()
            };
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
            return copy_len as u64;
        }

        // No matching message
        if nowait {
            return -errno::ENOMSG as u64;
        }

        if crate::signal::signal_pending() {
            return -errno::EINTR as u64;
        }

        // Block on wq_recv
        {
            let slots = MSG_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.deleted {
                    return -errno::EIDRM as u64;
                }
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -errno::ESRCH as u64,
                };
                let wq_entry = crate::process::wait::WaitQueueEntry::new(current as *mut _, false);
                entry.inner.wq_recv.add(wq_entry);

                // Set INTERRUPTIBLE while holding lock to prevent lost wakeup
                unsafe {
                    (*current).set_state(
                        crate::process::task::TaskState::new(
                            crate::process::task::TaskState::INTERRUPTIBLE,
                        ),
                    );
                }
            }
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
