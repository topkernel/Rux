//! POSIX Message Queues
//!
//! Implements mq_open, mq_unlink, mq_timedsend, mq_timedreceive, mq_notify, mq_getsetattr
//! following the Linux kernel design. POSIX MQs are file descriptor-based.

use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, copy_to_user};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use core::sync::atomic::{AtomicI32, AtomicI64, Ordering};

use super::util::*;

// ============================================================================
// UAPI Structures
// ============================================================================

/// struct mq_attr — POSIX message queue attributes
/// Must match the glibc/Linux layout for RV64. Total: 64 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MqAttr {
    pub mq_flags: i64,
    pub mq_maxmsg: i64,
    pub mq_msgsize: i64,
    pub mq_curmsgs: i64,
    pub __reserved: [i64; 4],
}

impl Default for MqAttr {
    fn default() -> Self {
        Self {
            mq_flags: 0,
            mq_maxmsg: 10,
            mq_msgsize: 8192,
            mq_curmsgs: 0,
            __reserved: [0; 4],
        }
    }
}

// ============================================================================
// Kernel Structures
// ============================================================================

/// A single message in a POSIX MQ.
struct MqMsg {
    /// Message priority (0 = highest).
    priority: u32,
    /// Message data.
    data: alloc::vec::Vec<u8>,
}

/// POSIX message queue object.
pub struct PosixMq {
    /// Queue name (e.g. "/myqueue").
    name: alloc::vec::Vec<u8>,
    /// Owner uid.
    uid: u32,
    /// Owner gid.
    gid: u32,
    /// Permissions.
    mode: u16,
    /// Messages in the queue.
    messages: Spinlock<alloc::vec::Vec<MqMsg>>,
    /// Current byte count.
    cbytes: AtomicI32,
    /// Queue attributes.
    attr: Spinlock<MqAttr>,
    /// Time of last send.
    stime: AtomicI64,
    /// Time of last receive.
    rtime: AtomicI64,
    /// Time of last attribute change.
    ctime: AtomicI64,
    /// Whether this queue has been unlinked.
    unlinked: AtomicI32,
    /// Number of open file descriptors referencing this queue.
    refcount: AtomicI32,
    /// Wait queue for senders (queue full).
    wq_send: WaitQueueHead,
    /// Wait queue for receivers (queue empty).
    wq_recv: WaitQueueHead,
    /// PID of registered notification process (0 = none).
    notify_pid: AtomicI32,
    /// Signal number for notification.
    notify_signo: AtomicI32,
}

impl PosixMq {
    fn new(name: &[u8], mode: u16, attr: Option<&MqAttr>) -> Self {
        let default_attr = MqAttr::default();
        let mq_attr = attr.unwrap_or(&default_attr);
        let cred = crate::sched::current().map(|t| (t.cred().uid, t.cred().gid));
        let (uid, gid) = cred.unwrap_or((0, 0));

        Self {
            name: name.to_vec(),
            uid,
            gid,
            mode,
            messages: Spinlock::new(alloc::vec::Vec::new()),
            cbytes: AtomicI32::new(0),
            attr: Spinlock::new(MqAttr {
                mq_flags: 0,
                // R32 (NEW-9): cap user-supplied attributes. mq_msgsize
                // bounds the per-message Vec allocation in mq_timedsend,
                // so an attacker-sized value (GBs) walked straight into
                // the 32MB kernel heap (R7-D4/SYSA-C1 class). Limits
                // mirror Linux RLIMIT_MSGQUEUE ballparks.
                mq_maxmsg: if mq_attr.mq_maxmsg > 0 { mq_attr.mq_maxmsg.min(1024) } else { 10 },
                mq_msgsize: if mq_attr.mq_msgsize > 0 { mq_attr.mq_msgsize.min(1024 * 1024) } else { 8192 },
                mq_curmsgs: 0,
                __reserved: [0; 4],
            }),
            stime: AtomicI64::new(0),
            rtime: AtomicI64::new(0),
            ctime: AtomicI64::new(ipc_current_time()),
            unlinked: AtomicI32::new(0),
            refcount: AtomicI32::new(1),
            wq_send: WaitQueueHead::new(),
            wq_recv: WaitQueueHead::new(),
            notify_pid: AtomicI32::new(0),
            notify_signo: AtomicI32::new(0),
        }
    }

    fn is_unlinked(&self) -> bool {
        self.unlinked.load(Ordering::Relaxed) != 0
    }
}

// ============================================================================
// Global POSIX MQ registry
// ============================================================================

const MQ_MAX: usize = 256;

static MQ_TABLE: Spinlock<[Option<alloc::sync::Arc<PosixMq>>; MQ_MAX]> =
    Spinlock::new([const { None }; MQ_MAX]);

// ============================================================================
// Helper functions
// ============================================================================

/// Find a POSIX MQ by name. Returns (index, Arc<PosixMq>) or None.
fn mq_find_by_name(name: &[u8]) -> Option<(usize, alloc::sync::Arc<PosixMq>)> {
    let table = MQ_TABLE.lock();
    for (i, slot) in table.iter().enumerate() {
        if let Some(ref mq) = slot {
            if !mq.is_unlinked() && mq.name == name {
                return Some((i, mq.clone()));
            }
        }
    }
    None
}

/// Allocate a slot for a new POSIX MQ.
///
/// R36-B3: the name-uniqueness recheck happens under the SAME lock as the
/// insertion. sys_mq_open's find-then-create sequence releases MQ_TABLE
/// between the two, so two concurrent mq_open(O_CREAT) calls with the same
/// name could BOTH miss the find and each insert its own queue — duplicate
/// instances of one POSIX name (later opens/unlinks then only ever saw the
/// first, and the second was unreachable but immortal). On NameExists the
/// caller retries the find phase, which now observes the winner.
enum MqAlloc {
    Created(usize),
    NameExists,
    Full,
}

fn mq_alloc(mq: PosixMq) -> MqAlloc {
    let mut table = MQ_TABLE.lock();
    for slot in table.iter() {
        if let Some(ref m) = slot {
            if !m.is_unlinked() && m.name == mq.name {
                return MqAlloc::NameExists;
            }
        }
    }
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(alloc::sync::Arc::new(mq));
            return MqAlloc::Created(i);
        }
    }
    MqAlloc::Full
}

/// Parse name from userspace pointer. Must start with '/'.
fn mq_parse_name(name_ptr: *const u8) -> Result<alloc::vec::Vec<u8>, i32> {
    if name_ptr.is_null() || !access_ok(name_ptr as usize, 256) {
        return Err(-errno::EFAULT);
    }
    // R32 (NEW-5): copy the whole 256-byte window through the
    // exception-table path. The old per-byte read_volatile had no
    // exception entry: a name that ran into an unmapped page before its
    // NUL terminator faulted the kernel (R20-3 class).
    let mut raw = [0u8; 256];
    // SAFETY: name_ptr was null-checked and access_ok-validated for 256
    // bytes above; raw is a 256-byte stack buffer.
    let uncopied = unsafe { copy_from_user(raw.as_mut_ptr(), name_ptr, 256) };
    let copied = 256 - uncopied;
    if copied == 0 {
        return Err(-errno::EFAULT);
    }
    let name: alloc::vec::Vec<u8> = raw[..copied].iter().copied().take_while(|&b| b != 0).collect();
    // No NUL within the readable region and the rest is unreadable: the
    // name is unterminated and would silently truncate to a DIFFERENT
    // (possibly valid) queue name — report EFAULT instead.
    if uncopied > 0 && name.len() == copied {
        return Err(-errno::EFAULT);
    }
    if name.is_empty() || name[0] != b'/' {
        return Err(-errno::EINVAL);
    }
    if name.len() > 255 {
        return Err(-errno::ENAMETOOLONG);
    }
    Ok(name)
}

// ============================================================================
// Syscall Implementations
// ============================================================================

/// sys_mq_open — Open or create a message queue (NR 180)
pub fn sys_mq_open(args: [u64; 6]) -> i64 {
    let name_ptr = args[0] as *const u8;
    let oflag = args[1] as i32;
    let mode = args[2] as u32;
    let attr_ptr = args[3] as *const MqAttr;

    let name = match mq_parse_name(name_ptr) {
        Ok(n) => n,
        Err(e) => return e as i64,
    };

    // Check for close-on-exec
    let _cloexec = (oflag & O_CLOEXEC_MQ as i32) != 0;

    // Read optional attributes
    let attr = if !attr_ptr.is_null() && (oflag & O_CREAT_MQ as i32) != 0 {
        if !access_ok(attr_ptr as usize, core::mem::size_of::<MqAttr>()) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: attr_ptr was null-checked and access_ok-validated above;
        // MqAttr is #[repr(C)] and size_of matches the expected layout.
        Some(unsafe { *attr_ptr })
    } else {
        None
    };

    let creating = (oflag & O_CREAT_MQ as i32) != 0;
    let excl = (oflag & O_EXCL_MQ as i32) != 0;

    // R36-B3: find-then-create wrapped in a retry loop — mq_alloc rechecks
    // the name under the table lock and reports NameExists when a concurrent
    // creator won the race; we then re-run the find phase instead of
    // inserting a duplicate instance of the same POSIX name.
    loop {
        // Find existing queue
        if let Some((_idx, mq)) = mq_find_by_name(&name) {
            if excl {
                return -(errno::EEXIST as i64);
            }
            // Check read/write permission
            let can_read = ((oflag & 3) != 1) && ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o4);
            let can_write = ((oflag & 3) != 0) && ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o2);

            if !can_read && !can_write {
                return -(errno::EACCES as i64);
            }

            // Allocate a file descriptor
            // R32 (NEW-6): allocate + store in ONE critical section. The old
            // allocate_mq_fd()/store_mq_fd() pair only "reserved" a slot by
            // leaving it None, so two concurrent mq_open calls could draw the
            // SAME fd number; the second store overwrote the first, leaking a
            // refcount and breaking the first caller's fd.
            let fd = match allocate_and_store_mq_fd(mq.clone()) {
                Some(f) => f,
                None => return -(errno::EMFILE as i64),
            };

            mq.refcount.fetch_add(1, Ordering::Relaxed);
            return fd as i64;
        }

        // Queue not found
        if !creating {
            return -(errno::ENOENT as i64);
        }

        // Create new queue
        let mq = PosixMq::new(&name, mode as u16, attr.as_ref());
        let idx = match mq_alloc(mq) {
            MqAlloc::Created(i) => i,
            MqAlloc::NameExists => continue, // lost the create race — re-find
            MqAlloc::Full => return -(errno::ENOSPC as i64),
        };

        let mq = MQ_TABLE.lock()[idx].as_ref().unwrap().clone();

        let fd = match allocate_and_store_mq_fd(mq.clone()) {
            Some(f) => f,
            None => {
                // R32 (NEW-6): no fd available — remove the queue we just
                // created instead of leaking it in MQ_TABLE forever (it had
                // refcount 1 with no fd that could ever release it).
                let mut table = MQ_TABLE.lock();
                let _dropped = table[idx].take();
                return -(errno::EMFILE as i64);
            }
        };
        return fd as i64;
    }
}

/// sys_mq_unlink — Remove a message queue (NR 181)
pub fn sys_mq_unlink(args: [u64; 6]) -> i64 {
    let name_ptr = args[0] as *const u8;

    let name = match mq_parse_name(name_ptr) {
        Ok(n) => n,
        Err(e) => return e as i64,
    };

    let mut table = MQ_TABLE.lock();
    for slot in table.iter_mut() {
        if let Some(ref mq) = slot {
            if !mq.is_unlinked() && mq.name == name {
                // Permission check (review IPC: mq_unlink 无权限): the
                // caller needs write access to the queue, or CAP_SYS_ADMIN.
                let cred_ok = crate::sched::current().map(|t| {
                    t.cred().euid == mq.uid
                        || ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o2)
                }).unwrap_or(false);
                if !cred_ok && !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
                    return -(errno::EACCES as i64);
                }
                mq.unlinked.store(1, Ordering::Relaxed);
                // If refcount is 0, we can free immediately
                if mq.refcount.load(Ordering::Relaxed) == 0 {
                    *slot = None;
                }
                return 0;
            }
        }
    }
    -(errno::ENOENT as i64)
}

/// Parse a timespec timeout pointer into a jiffies deadline.
/// Returns Ok(None) if timeout_ptr is null (block forever).
///
/// Review IPC (mq timeout EFAULT): an unreadable timespec used to be
/// silently treated as "block forever" — the caller then hung on a queue
/// that would never satisfy it. Faulty pointers are now EFAULT and
/// negative/overflowing fields EINVAL.
fn parse_mq_timeout(timeout_ptr: *const u8) -> Result<Option<u64>, i32> {
    if timeout_ptr.is_null() {
        return Ok(None);
    }
    if !access_ok(timeout_ptr as usize, 16) {
        return Err(-errno::EFAULT);
    }
    // SAFETY: timeout_ptr was access_ok-validated for 16 bytes above;
    // casting to two consecutive i64 values (sec + nsec) is within bounds.
    let ts_sec = unsafe { *(timeout_ptr as *const i64) };
    let ts_nsec = unsafe { *((timeout_ptr as *const i64).add(1)) };
    if ts_sec < 0 || ts_nsec < 0 || ts_nsec >= 1_000_000_000 {
        return Err(-errno::EINVAL);
    }
    let timeout_jiffies = (ts_sec as u64)
        .saturating_mul(crate::drivers::timer::HZ as u64)
        .saturating_add(
            (ts_nsec as u64) * crate::drivers::timer::HZ as u64 / 1_000_000_000,
        );
    Ok(Some(crate::drivers::timer::get_jiffies() + timeout_jiffies))
}

/// sys_mq_timedsend — Send a message to a message queue (NR 182)
pub fn sys_mq_timedsend(args: [u64; 6]) -> i64 {
    let mqdes = args[0] as i32;
    let msg_ptr = args[1] as *const u8;
    let msg_len = args[2] as usize;
    let msg_prio = args[3] as u32;
    let timeout_ptr = args[4] as *const u8;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -(errno::EBADF as i64),
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -(errno::EINVAL as i64);
    }

    // Check message size (acquire messages first, then attr — consistent lock ordering)
    {
        let messages = mq.messages.lock();
        let attr = mq.attr.lock();
        if msg_len > attr.mq_msgsize as usize {
            return -(errno::EMSGSIZE as i64);
        }
    }

    // Check permission
    if !ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o2) {
        return -(errno::EACCES as i64);
    }

    if msg_prio >= 32768 {
        return -(errno::EINVAL as i64);
    }

    // Copy message data
    if !access_ok(msg_ptr as usize, msg_len) {
        return -(errno::EFAULT as i64);
    }
    let mut data = alloc::vec::Vec::with_capacity(msg_len);
    data.resize(msg_len, 0);
    // SAFETY: msg_ptr was access_ok-validated for msg_len bytes above;
    // data is a Vec with capacity msg_len, so the destination is valid.
    // The uncopied count MUST be checked — a partial copy would silently
    // zero-fill the tail (review IPC M: mq_send 忽略 copy_from_user 返回值).
    if unsafe { copy_from_user(data.as_mut_ptr(), msg_ptr, msg_len) } != 0 {
        return -(errno::EFAULT as i64);
    }

    // Parse timeout
    let deadline = match parse_mq_timeout(timeout_ptr) {
        Ok(d) => d,
        Err(e) => return e as i64,
    };

    // Check O_NONBLOCK_MQ and mq_maxmsg once (immutable during this call)
    let (nonblock, max_msgs) = {
        let attr = mq.attr.lock();
        let nb = (attr.mq_flags & O_NONBLOCK_MQ as i64) != 0;
        let mm = attr.mq_maxmsg;
        (nb, mm)
    };

    // Send loop — follows the Linux prepare_to_wait/finish_wait pattern:
    // 1. Hold messages lock while checking condition AND adding to wait queue
    //    (prevents lost wakeup race between drop(lock) and add(wq))
    // 2. Release lock, then schedule
    // 3. After wakeup, re-acquire lock to safely remove from wait queue
    //    (wake_up_all iterates the list concurrently)
    loop {
        let mut messages = mq.messages.lock();

        let was_empty = messages.is_empty();
        if (messages.len() as i64) < max_msgs {
            // Space available — insert message (sorted by priority)
            let insert_pos = messages.iter().position(|m| m.priority < msg_prio)
                .unwrap_or(messages.len());
            messages.insert(insert_pos, MqMsg { priority: msg_prio, data });
            mq.attr.lock().mq_curmsgs += 1;
            mq.cbytes.fetch_add(msg_len as i32, Ordering::Relaxed);
            mq.stime.store(ipc_current_time(), Ordering::Relaxed);
            drop(messages);
            // Wake up receivers
            mq.wq_recv.wake_up_all();
            // Notification fires ONLY on the empty -> non-empty transition
            // (review IPC P2: mq notify 每次入队触发 — POSIX delivers the
            // signal exactly once per empty-queue episode; firing on every
            // send defeated the receiver's "queue was empty" contract).
            // The signal carries no siginfo payload yet: the kernel signal
            // layer has no public sigqueue/SI_MESGQ API (tracked follow-up;
            // wiring sigev_value needs signal-layer support owned by the
            // sync agent).
            if was_empty {
                let notify_pid = mq.notify_pid.swap(0, Ordering::Relaxed);
                if notify_pid != 0 {
                    let signo = mq.notify_signo.load(Ordering::Relaxed);
                    if signo > 0 {
                        let _ = crate::signal::send_signal(notify_pid as u32, signo);
                    }
                }
            }
            return 0;
        }

        // Queue full — check exit conditions while holding lock
        if nonblock {
            return -(errno::EAGAIN as i64);
        }

        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
                return -(errno::ETIMEDOUT as i64);
            }
        }

        // R39: register + set INTERRUPTIBLE atomically under the WQ's own
        // lock (prepare_to_wait). The old add()+set_state pair held the
        // messages lock, but the WAKER takes the WQ lock — a wake landing
        // between the two calls consumed the one-shot token (entry marked
        // woken) while Task::wake_up dropped it (target still RUNNING);
        // with no message-recheck before schedule() the receiver then
        // blocked forever despite a message being present.
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        };
        mq.wq_send.prepare_to_wait(current as *mut _, false, true);

        // Release lock, then schedule. Arm a wakeup timer for the deadline
        // so an empty queue with no producer still returns ETIMEDOUT (the
        // old code parsed the deadline but nothing ever woke us).
        drop(messages);
        // R23-5: signal delivered while still RUNNING → no wakeup; recheck
        // before sleeping (same unkillable window R22-1 closed for SysV).
        // R24: the early return must ALSO remove our wait-queue entry (as
        // R22-1's SysV fix does) — a leaked entry keeps a raw Task pointer
        // that a later wake_up_all() dereferences after the task exited
        // (UAF wake), and entries accumulate on every EINTR retry.
        if crate::signal::signal_pending() {
            {
                let _messages = mq.messages.lock();
                mq.wq_send.remove(current as *mut _);
            }
            if let Some(cur) = crate::sched::current() {
                (*cur).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING,
                ));
                crate::sched::dequeue_task(&*cur);
            }
            return -(errno::EINTR as i64);
        }
        let timer_id = deadline
            .map(|dl| crate::timer::add_timer_wakeup(dl, crate::sched::get_current_pid()))
            .unwrap_or(0);
        // R32 (NEW-3 twin): timer pool exhausted — a timed send would sleep
        // forever on a full queue. Remove the wait entry and fail instead.
        if deadline.is_some() && timer_id == 0 {
            {
                let _messages = mq.messages.lock();
                mq.wq_send.remove(current as *mut _);
            }
            (*current).set_state(crate::process::task::TaskState::new(
                crate::process::task::TaskState::RUNNING,
            ));
            crate::sched::dequeue_task(&*current);
            return -(errno::ENOMEM as i64);
        }
        crate::sched::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }

        // Re-acquire lock to safely remove from wait queue
        // (wake_up_all iterates the list; we must hold a lock to avoid corruption)
        let _messages = mq.messages.lock();
        mq.wq_send.remove(current as *mut _);
    }
}

/// sys_mq_timedreceive — Receive a message from a message queue (NR 183)
pub fn sys_mq_timedreceive(args: [u64; 6]) -> i64 {
    let mqdes = args[0] as i32;
    let msg_ptr = args[1] as *mut u8;
    let msg_len = args[2] as usize;
    let prio_ptr = args[3] as *mut u32;
    let timeout_ptr = args[4] as *const u8;

    if msg_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -(errno::EBADF as i64),
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -(errno::EINVAL as i64);
    }

    // Check permission
    if !ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o4) {
        return -(errno::EACCES as i64);
    }

    // Parse timeout
    let deadline = match parse_mq_timeout(timeout_ptr) {
        Ok(d) => d,
        Err(e) => return e as i64,
    };

    // Check O_NONBLOCK_MQ once (immutable during this call)
    let nonblock = {
        let attr = mq.attr.lock();
        (attr.mq_flags & O_NONBLOCK_MQ as i64) != 0
    };

    // Receive loop — same prepare_to_wait/finish_wait pattern as timedsend:
    // hold messages lock while checking condition AND adding to wait queue
    loop {
        let mut messages = mq.messages.lock();

        if !messages.is_empty() {
            // Got a message — update stats while holding lock
            let msg = messages.remove(0);
            mq.attr.lock().mq_curmsgs -= 1;
            // R32 (NEW-10): POSIX mq_receive must fail with EMSGSIZE when
            // the user buffer is smaller than the message — the old silent
            // truncation destroyed message data.
            if msg.data.len() > msg_len {
                messages.insert(0, msg);
                mq.attr.lock().mq_curmsgs += 1;
                return -(errno::EMSGSIZE as i64);
            }
            let copy_len = msg.data.len();
            mq.cbytes.fetch_sub(copy_len as i32, Ordering::Relaxed);
            mq.rtime.store(ipc_current_time(), Ordering::Relaxed);
            drop(messages);
            // Wake up senders (space freed)
            mq.wq_send.wake_up_all();

            // Copy data to userspace
            if !access_ok(msg_ptr as usize, copy_len) {
                // Put message back
                let mut messages = mq.messages.lock();
                messages.insert(0, msg);
                mq.attr.lock().mq_curmsgs += 1;
                mq.cbytes.fetch_add(copy_len as i32, Ordering::Relaxed);
                return -(errno::EFAULT as i64);
            }
            // SAFETY: msg_ptr was access_ok-validated for copy_len bytes above;
            // msg.data.as_ptr() is valid for msg.data.len() bytes (>= copy_len).
            unsafe { copy_to_user(msg_ptr, msg.data.as_ptr(), copy_len); }

            // Copy priority
            if !prio_ptr.is_null() && access_ok(prio_ptr as usize, 4) {
                // SAFETY: prio_ptr was access_ok-validated for 4 bytes above;
                // writing a u32 value to a valid userspace pointer.
                unsafe { core::ptr::write_volatile(prio_ptr, msg.priority) };
            }

            return copy_len as i64;
        }

        // Queue empty — check exit conditions while holding lock
        if nonblock {
            return -(errno::EAGAIN as i64);
        }

        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
                return -(errno::ETIMEDOUT as i64);
            }
        }

        // R39: atomic register + INTERRUPTIBLE under the WQ lock — same
        // lost-wakeup closure as mq_timedsend above.
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        };
        mq.wq_recv.prepare_to_wait(current as *mut _, false, true);

        // Release lock, then schedule. Arm a wakeup timer for the deadline
        // so an empty queue with no producer still returns ETIMEDOUT (the
        // old code parsed the deadline but nothing ever woke us).
        drop(messages);
        // R23-5: signal delivered while still RUNNING → no wakeup; recheck
        // before sleeping (same unkillable window R22-1 closed for SysV).
        // R24: remove the wait-queue entry before returning — see the
        // timedsend R24 note (leaked entry = stale Task pointer for later
        // wake_up_all calls).
        if crate::signal::signal_pending() {
            {
                let _messages = mq.messages.lock();
                mq.wq_recv.remove(current as *mut _);
            }
            if let Some(cur) = crate::sched::current() {
                (*cur).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING,
                ));
                crate::sched::dequeue_task(&*cur);
            }
            return -(errno::EINTR as i64);
        }
        let timer_id = deadline
            .map(|dl| crate::timer::add_timer_wakeup(dl, crate::sched::get_current_pid()))
            .unwrap_or(0);
        // R32 (NEW-3 twin): timer pool exhausted — a timed receive would
        // sleep forever on an empty queue. Remove the wait entry and fail.
        if deadline.is_some() && timer_id == 0 {
            {
                let _messages = mq.messages.lock();
                mq.wq_recv.remove(current as *mut _);
            }
            (*current).set_state(crate::process::task::TaskState::new(
                crate::process::task::TaskState::RUNNING,
            ));
            crate::sched::dequeue_task(&*current);
            return -(errno::ENOMEM as i64);
        }
        crate::sched::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }

        // Re-acquire lock to safely remove from wait queue
        let _messages = mq.messages.lock();
        mq.wq_recv.remove(current as *mut _);
    }
}

/// SIGEV notification constants
const SIGEV_NONE: i32 = 0;
const SIGEV_SIGNAL: i32 = 1;
const SIGEV_THREAD: i32 = 2;

/// struct sigevent layout for RV64 (first two fields needed for mq_notify).
/// sigev_value (8 bytes) is at offset 8, but we only need sigev_notify (offset 0)
/// and sigev_signo (offset 4).
#[repr(C)]
struct SigEvent {
    sigev_value: u64,    // union { int, void*, void(*)(sigval_t) }
    sigev_signo: i32,
    sigev_notify: i32,
}

/// sys_mq_notify — Register for notification when message arrives (NR 184)
pub fn sys_mq_notify(args: [u64; 6]) -> i64 {
    let mqdes = args[0] as i32;
    let sevp = args[1] as *const SigEvent;

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -(errno::EBADF as i64),
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -(errno::EINVAL as i64);
    }

    // Deregister if sevp is NULL or SIGEV_NONE
    if sevp.is_null() {
        mq.notify_pid.store(0, Ordering::Relaxed);
        return 0;
    }

    if !access_ok(sevp as usize, core::mem::size_of::<SigEvent>()) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: sevp was access_ok-validated for size_of::<SigEvent>() above;
    // SigEvent is #[repr(C)] and the read is within validated bounds.
    let sev = unsafe { core::ptr::read(sevp) };

    if sev.sigev_notify == SIGEV_NONE {
        mq.notify_pid.store(0, Ordering::Relaxed);
        return 0;
    }

    if sev.sigev_notify == SIGEV_SIGNAL {
        let pid = crate::sched::current().map(|t| t.pid() as i32).unwrap_or(0);
        // Only allow registration if no one else is registered
        let old_pid = mq.notify_pid.swap(pid, Ordering::Relaxed);
        if old_pid != 0 && old_pid != pid {
            // Another process already registered — per POSIX, this is EBUSY
            mq.notify_pid.store(old_pid, Ordering::Relaxed);
            return -(errno::EBUSY as i64);
        }
        mq.notify_signo.store(sev.sigev_signo, Ordering::Relaxed);
        return 0;
    }

    // SIGEV_THREAD not supported
    -(errno::ENOSYS as i64)
}

/// sys_mq_getsetattr — Get/set message queue attributes (NR 185)
pub fn sys_mq_getsetattr(args: [u64; 6]) -> i64 {
    let mqdes = args[0] as i32;
    let attr_ptr = args[1] as *mut MqAttr;
    let newattr_ptr = args[2] as *const MqAttr;

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -(errno::EBADF as i64),
    };

    // Set new attributes (only mq_flags can be changed)
    if !newattr_ptr.is_null() {
        if !access_ok(newattr_ptr as usize, core::mem::size_of::<MqAttr>()) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: newattr_ptr was access_ok-validated for size_of::<MqAttr>() above;
        // MqAttr is #[repr(C)] and the dereference is within validated bounds.
        let newattr = unsafe { *newattr_ptr };
        let mut attr = mq.attr.lock();
        attr.mq_flags = newattr.mq_flags;
    }

    // Get current attributes
    if !attr_ptr.is_null() {
        if !access_ok(attr_ptr as usize, core::mem::size_of::<MqAttr>()) {
            return -(errno::EFAULT as i64);
        }
        let attr = *mq.attr.lock();
        // SAFETY: attr_ptr was access_ok-validated for size_of::<MqAttr>() above;
        // &attr is a stack-local copy of the queue attributes.
        unsafe {
            copy_to_user(
                attr_ptr as *mut u8,
                &attr as *const MqAttr as *const u8,
                core::mem::size_of::<MqAttr>(),
            );
        }
    }

    0
}

// ============================================================================
// Permission checking for POSIX MQ
// ============================================================================

/// Check POSIX MQ permissions (similar to file permission check).
fn ipc_check_permissions_mq(uid: u32, gid: u32, mode: u16, desired: u16) -> bool {
    let cred = match crate::sched::current() {
        Some(t) => t.cred(),
        None => return false,
    };

    if cred.euid == 0 {
        return true;
    }

    if cred.euid == uid {
        let owner_bits = ((mode >> 6) & 0o7) as u16;
        return (desired & owner_bits) == desired;
    }

    if cred.egid == gid {
        let group_bits = ((mode >> 3) & 0o7) as u16;
        return (desired & group_bits) == desired;
    }

    let other_bits = (mode & 0o7) as u16;
    (desired & other_bits) == desired
}

// ============================================================================
// Per-process MQ fd tracking (PID-keyed global table)
// ============================================================================
// TODO (F12-34): MQ fds should be integrated into the per-process fd table
// (task.fdtable) so that close(), dup2(), poll(), and fork() work correctly.
// Current limitations:
// - fork() does not inherit MQ fds (PID-keyed lookup breaks)
// - close() via regular fd path does not release MQ resources
// - dup2()/fcntl() cannot operate on MQ fds
// - fd numbers 512-575 may collide with regular file descriptors

const MQ_FDS_MAX: usize = 64;

struct MqFdSlot {
    pid: u32,
    mq: alloc::sync::Arc<PosixMq>,
}

static MQ_FD_TABLE: Spinlock<[Option<MqFdSlot>; MQ_FDS_MAX]> =
    Spinlock::new([const { None }; MQ_FDS_MAX]);

/// Allocate a file descriptor number for a POSIX MQ and store the queue
/// reference in one critical section (see R32 NEW-6 note in sys_mq_open).
fn allocate_and_store_mq_fd(mq: alloc::sync::Arc<PosixMq>) -> Option<i32> {
    let pid = crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0);
    let mut table = MQ_FD_TABLE.lock();
    for i in 0..MQ_FDS_MAX {
        if table[i].is_none() {
            table[i] = Some(MqFdSlot { pid, mq });
            return Some((512 + i) as i32);
        }
    }
    None
}

/// Get the MQ reference at the given fd slot for the current process.
fn get_mq_fd(fd: usize) -> Option<alloc::sync::Arc<PosixMq>> {
    if fd < 512 {
        return None;
    }
    let idx = fd - 512;
    if idx >= MQ_FDS_MAX {
        return None;
    }
    let pid = crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0);
    let table = MQ_FD_TABLE.lock();
    table[idx].as_ref().and_then(|slot| {
        if slot.pid == pid {
            Some(slot.mq.clone())
        } else {
            None
        }
    })
}

/// Clean up all MQ fd entries for a given task (called from do_exit).
pub fn mq_fds_cleanup(task: *mut crate::process::Task) {
    if task.is_null() {
        return;
    }
    // SAFETY: task was null-checked above and is a valid pointer to the exiting
    // task passed from do_exit; pid() is safe to call on it.
    let pid = unsafe { (*task).pid() as u32 };

    // Phase 1: Collect matching entries and clear them from the fd table.
    // We must release the fd table lock before touching the global table
    // to avoid lock ordering issues.
    let mut to_free: alloc::vec::Vec<alloc::sync::Arc<PosixMq>> = alloc::vec::Vec::new();
    {
        let mut table = MQ_FD_TABLE.lock();
        for i in 0..MQ_FDS_MAX {
            if let Some(ref s) = table[i] {
                if s.pid == pid {
                    let mq = s.mq.clone();
                    table[i] = None;
                    to_free.push(mq);
                }
            }
        }
    }

    // Phase 2: Decrement refcounts and free unlinked+last-ref queues.
    for mq in to_free.iter() {
        // R32 (NEW-7): the exiting process may have been the notifier —
        // clear the registration so a dead PID does not eat the next
        // message's notification.
        if mq.notify_pid.load(Ordering::Relaxed) == pid as i32 {
            mq.notify_pid.store(0, Ordering::Relaxed);
        }
        let prev = mq.refcount.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 && mq.is_unlinked() {
            let mut global = MQ_TABLE.lock();
            for gslot in global.iter_mut() {
                if let Some(ref g) = *gslot {
                    // R32 (NEW-8): match by Arc identity, not by name —
                    // after unlink a NEW queue with the same name may
                    // already occupy the table, and the old name match
                    // freed the WRONG (still-live) queue.
                    if g.is_unlinked() && alloc::sync::Arc::ptr_eq(g, mq) {
                        *gslot = None;
                        break;
                    }
                }
            }
        }
    }
}

/// Close a POSIX MQ fd for the current process.
/// Decrements refcount, frees the queue from global table if unlinked+refcount==0.
pub fn close_mq_fd(fd: i32) -> i32 {
    if (fd as usize) < 512 {
        return -errno::EBADF;
    }
    let idx = (fd as usize) - 512;
    if idx >= MQ_FDS_MAX {
        return -errno::EBADF;
    }
    let pid = crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0);
    let mut table = MQ_FD_TABLE.lock();
    match table[idx].take() {
        Some(slot) if slot.pid == pid => {
            // R32 (NEW-7): only clear a notification registered by THIS
            // process — the old unconditional swap(0) also killed another
            // process's mq_notify registration when an unrelated fd of
            // the same queue closed.
            if slot.mq.notify_pid.load(Ordering::Relaxed) == pid as i32 {
                slot.mq.notify_pid.store(0, Ordering::Relaxed);
            }
            // Decrement refcount
            let prev = slot.mq.refcount.fetch_sub(1, Ordering::Relaxed);
            // If unlinked and last reference, free from global table
            if prev == 1 && slot.mq.is_unlinked() {
                drop(table);
                let mut global = MQ_TABLE.lock();
                for gslot in global.iter_mut() {
                    if let Some(ref g) = *gslot {
                        // R32 (NEW-8): Arc identity, not name — see the
                        // mq_fds_cleanup note (a re-created same-name
                        // queue was freed by mistake).
                        if g.is_unlinked() && alloc::sync::Arc::ptr_eq(g, &slot.mq) {
                            *gslot = None;
                            break;
                        }
                    }
                }
            }
            0
        }
        Some(_) => -errno::EBADF,
        None => -errno::EBADF,
    }
}
