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
                mq_maxmsg: if mq_attr.mq_maxmsg > 0 { mq_attr.mq_maxmsg } else { 10 },
                mq_msgsize: if mq_attr.mq_msgsize > 0 { mq_attr.mq_msgsize } else { 8192 },
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
fn mq_alloc(mq: PosixMq) -> Option<usize> {
    let mut table = MQ_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(alloc::sync::Arc::new(mq));
            return Some(i);
        }
    }
    None
}

/// Parse name from userspace pointer. Must start with '/'.
fn mq_parse_name(name_ptr: *const u8) -> Result<alloc::vec::Vec<u8>, i32> {
    if name_ptr.is_null() {
        return Err(-errno::EFAULT);
    }
    // Read name byte by byte, max 256 chars
    let mut name = alloc::vec::Vec::with_capacity(64);
    for i in 0..256 {
        // SAFETY: name_ptr was null-checked above; we read up to 256 bytes until
        // a NUL terminator, staying within a reasonable bound for a queue name.
        let b = unsafe { core::ptr::read_volatile(name_ptr.add(i)) };
        if b == 0 {
            break;
        }
        name.push(b);
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
pub fn sys_mq_open(args: [u64; 6]) -> u64 {
    let name_ptr = args[0] as *const u8;
    let oflag = args[1] as i32;
    let mode = args[2] as u32;
    let attr_ptr = args[3] as *const MqAttr;

    let name = match mq_parse_name(name_ptr) {
        Ok(n) => n,
        Err(e) => return e as u64,
    };

    // Check for close-on-exec
    let _cloexec = (oflag & O_CLOEXEC_MQ as i32) != 0;

    // Read optional attributes
    let attr = if !attr_ptr.is_null() && (oflag & O_CREAT_MQ as i32) != 0 {
        if !access_ok(attr_ptr as usize, core::mem::size_of::<MqAttr>()) {
            return -errno::EFAULT as u64;
        }
        // SAFETY: attr_ptr was null-checked and access_ok-validated above;
        // MqAttr is #[repr(C)] and size_of matches the expected layout.
        Some(unsafe { *attr_ptr })
    } else {
        None
    };

    let creating = (oflag & O_CREAT_MQ as i32) != 0;
    let excl = (oflag & O_EXCL_MQ as i32) != 0;

    // Find existing queue
    if let Some((_idx, mq)) = mq_find_by_name(&name) {
        if excl {
            return -errno::EEXIST as u64;
        }
        // Check read/write permission
        let can_read = (oflag & 3 != 0) && ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o4);
        let can_write = (oflag & 3 != 0) && ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o2);

        if !can_read && !can_write {
            return -errno::EACCES as u64;
        }

        // Allocate a file descriptor
        let fd = match allocate_mq_fd() {
            Some(f) => f,
            None => return -errno::EMFILE as u64,
        };

        mq.refcount.fetch_add(1, Ordering::Relaxed);
        store_mq_fd(fd as usize, mq);
        return fd as u64;
    }

    // Queue not found
    if !creating {
        return -errno::ENOENT as u64;
    }

    // Create new queue
    let mq = PosixMq::new(&name, mode as u16, attr.as_ref());
    let idx = match mq_alloc(mq) {
        Some(i) => i,
        None => return -errno::ENOSPC as u64,
    };

    let mq = MQ_TABLE.lock()[idx].as_ref().unwrap().clone();

    let fd = match allocate_mq_fd() {
        Some(f) => f,
        None => return -errno::EMFILE as u64,
    };

    store_mq_fd(fd as usize, mq);
    fd as u64
}

/// sys_mq_unlink — Remove a message queue (NR 181)
pub fn sys_mq_unlink(args: [u64; 6]) -> u64 {
    let name_ptr = args[0] as *const u8;

    let name = match mq_parse_name(name_ptr) {
        Ok(n) => n,
        Err(e) => return e as u64,
    };

    let mut table = MQ_TABLE.lock();
    for slot in table.iter_mut() {
        if let Some(ref mq) = slot {
            if !mq.is_unlinked() && mq.name == name {
                mq.unlinked.store(1, Ordering::Relaxed);
                // If refcount is 0, we can free immediately
                if mq.refcount.load(Ordering::Relaxed) == 0 {
                    *slot = None;
                }
                return 0;
            }
        }
    }
    -errno::ENOENT as u64
}

/// Parse a timespec timeout pointer into a jiffies deadline.
/// Returns None if timeout_ptr is null (block forever).
fn parse_mq_timeout(timeout_ptr: *const u8) -> Option<u64> {
    if timeout_ptr.is_null() {
        return None;
    }
    if !access_ok(timeout_ptr as usize, 16) {
        // Caller should check EFAULT before calling; return None to block
        return None;
    }
    // SAFETY: timeout_ptr was access_ok-validated for 16 bytes above;
    // casting to two consecutive i64 values (sec + nsec) is within bounds.
    let ts_sec = unsafe { *(timeout_ptr as *const i64) };
    let ts_nsec = unsafe { *((timeout_ptr as *const i64).add(1)) };
    if ts_sec < 0 || ts_nsec < 0 || ts_nsec >= 1_000_000_000 {
        return None;
    }
    let timeout_jiffies = (ts_sec as u64) * crate::drivers::timer::HZ as u64
        + (ts_nsec as u64) * crate::drivers::timer::HZ as u64 / 1_000_000_000;
    Some(crate::drivers::timer::get_jiffies() + timeout_jiffies)
}

/// sys_mq_timedsend — Send a message to a message queue (NR 182)
pub fn sys_mq_timedsend(args: [u64; 6]) -> u64 {
    let mqdes = args[0] as i32;
    let msg_ptr = args[1] as *const u8;
    let msg_len = args[2] as usize;
    let msg_prio = args[3] as u32;
    let timeout_ptr = args[4] as *const u8;

    if msg_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -errno::EBADF as u64,
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -errno::EINVAL as u64;
    }

    // Check message size (acquire messages first, then attr — consistent lock ordering)
    {
        let messages = mq.messages.lock();
        let attr = mq.attr.lock();
        if msg_len > attr.mq_msgsize as usize {
            return -errno::EMSGSIZE as u64;
        }
    }

    // Check permission
    if !ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o2) {
        return -errno::EACCES as u64;
    }

    if msg_prio >= 32768 {
        return -errno::EINVAL as u64;
    }

    // Copy message data
    if !access_ok(msg_ptr as usize, msg_len) {
        return -errno::EFAULT as u64;
    }
    let mut data = alloc::vec::Vec::with_capacity(msg_len);
    data.resize(msg_len, 0);
    // SAFETY: msg_ptr was access_ok-validated for msg_len bytes above;
    // data is a Vec with capacity msg_len, so the destination is valid.
    unsafe { copy_from_user(data.as_mut_ptr(), msg_ptr, msg_len); }

    // Parse timeout
    let deadline = parse_mq_timeout(timeout_ptr);

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

        if (messages.len() as i64) < max_msgs {
            // Space available — insert message (sorted by priority)
            let insert_pos = messages.iter().position(|m| m.priority > msg_prio)
                .unwrap_or(messages.len());
            messages.insert(insert_pos, MqMsg { priority: msg_prio, data });
            mq.attr.lock().mq_curmsgs += 1;
            mq.cbytes.fetch_add(msg_len as i32, Ordering::Relaxed);
            mq.stime.store(ipc_current_time(), Ordering::Relaxed);
            drop(messages);
            // Wake up receivers
            mq.wq_recv.wake_up_all();
            // Send notification signal if registered (one-shot)
            let notify_pid = mq.notify_pid.swap(0, Ordering::Relaxed);
            if notify_pid != 0 {
                let signo = mq.notify_signo.load(Ordering::Relaxed);
                if signo > 0 {
                    let _ = crate::signal::send_signal(notify_pid as u32, signo);
                }
            }
            return 0;
        }

        // Queue full — check exit conditions while holding lock
        if nonblock {
            return -errno::EAGAIN as u64;
        }

        if crate::signal::signal_pending() {
            return -errno::EINTR as u64;
        }

        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
                return -errno::ETIMEDOUT as u64;
            }
        }

        // Add to wait queue WHILE holding messages lock — prevents lost wakeup
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return -errno::ESRCH as u64,
        };
        let wq_entry = crate::process::wait::WaitQueueEntry::new(current as *mut _, false);
        mq.wq_send.add(wq_entry);

        // SAFETY: current is a valid raw pointer from sched::current();
        // set_state is safe to call on the current task before schedule().
        unsafe {
            (*current).set_state(
                crate::process::task::TaskState::new(crate::process::task::TaskState::INTERRUPTIBLE),
            );
        }

        // Release lock, then schedule
        drop(messages);
        crate::sched::schedule();

        // Re-acquire lock to safely remove from wait queue
        // (wake_up_all iterates the list; we must hold a lock to avoid corruption)
        let _messages = mq.messages.lock();
        mq.wq_send.remove(current as *mut _);
    }
}

/// sys_mq_timedreceive — Receive a message from a message queue (NR 183)
pub fn sys_mq_timedreceive(args: [u64; 6]) -> u64 {
    let mqdes = args[0] as i32;
    let msg_ptr = args[1] as *mut u8;
    let msg_len = args[2] as usize;
    let prio_ptr = args[3] as *mut u32;
    let timeout_ptr = args[4] as *const u8;

    if msg_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -errno::EBADF as u64,
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -errno::EINVAL as u64;
    }

    // Check permission
    if !ipc_check_permissions_mq(mq.uid, mq.gid, mq.mode, 0o4) {
        return -errno::EACCES as u64;
    }

    // Parse timeout
    let deadline = parse_mq_timeout(timeout_ptr);

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
            let copy_len = if msg.data.len() > msg_len { msg_len } else { msg.data.len() };
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
                return -errno::EFAULT as u64;
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

            return copy_len as u64;
        }

        // Queue empty — check exit conditions while holding lock
        if nonblock {
            return -errno::EAGAIN as u64;
        }

        if crate::signal::signal_pending() {
            return -errno::EINTR as u64;
        }

        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
                return -errno::ETIMEDOUT as u64;
            }
        }

        // Add to wait queue WHILE holding messages lock — prevents lost wakeup
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return -errno::ESRCH as u64,
        };
        let wq_entry = crate::process::wait::WaitQueueEntry::new(current as *mut _, false);
        mq.wq_recv.add(wq_entry);

        // SAFETY: current is a valid raw pointer from sched::current();
        // set_state is safe to call on the current task before schedule().
        unsafe {
            (*current).set_state(
                crate::process::task::TaskState::new(crate::process::task::TaskState::INTERRUPTIBLE),
            );
        }

        // Release lock, then schedule
        drop(messages);
        crate::sched::schedule();

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
pub fn sys_mq_notify(args: [u64; 6]) -> u64 {
    let mqdes = args[0] as i32;
    let sevp = args[1] as *const SigEvent;

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -errno::EBADF as u64,
    };

    if mq.is_unlinked() && mq.refcount.load(Ordering::Relaxed) <= 1 {
        return -errno::EINVAL as u64;
    }

    // Deregister if sevp is NULL or SIGEV_NONE
    if sevp.is_null() {
        mq.notify_pid.store(0, Ordering::Relaxed);
        return 0;
    }

    if !access_ok(sevp as usize, core::mem::size_of::<SigEvent>()) {
        return -errno::EFAULT as u64;
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
            return -errno::EBUSY as u64;
        }
        mq.notify_signo.store(sev.sigev_signo, Ordering::Relaxed);
        return 0;
    }

    // SIGEV_THREAD not supported
    -errno::ENOSYS as u64
}

/// sys_mq_getsetattr — Get/set message queue attributes (NR 185)
pub fn sys_mq_getsetattr(args: [u64; 6]) -> u64 {
    let mqdes = args[0] as i32;
    let attr_ptr = args[1] as *mut MqAttr;
    let newattr_ptr = args[2] as *const MqAttr;

    // Get the MQ from the fd
    let mq = match get_mq_fd(mqdes as usize) {
        Some(m) => m,
        None => return -errno::EBADF as u64,
    };

    // Set new attributes (only mq_flags can be changed)
    if !newattr_ptr.is_null() {
        if !access_ok(newattr_ptr as usize, core::mem::size_of::<MqAttr>()) {
            return -errno::EFAULT as u64;
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
            return -errno::EFAULT as u64;
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

const MQ_FDS_MAX: usize = 64;

struct MqFdSlot {
    pid: u32,
    mq: alloc::sync::Arc<PosixMq>,
}

static MQ_FD_TABLE: Spinlock<[Option<MqFdSlot>; MQ_FDS_MAX]> =
    Spinlock::new([const { None }; MQ_FDS_MAX]);

/// Allocate a file descriptor number for a POSIX MQ.
fn allocate_mq_fd() -> Option<i32> {
    let table = MQ_FD_TABLE.lock();
    for i in 0..MQ_FDS_MAX {
        if table[i].is_none() {
            return Some((512 + i) as i32);
        }
    }
    None
}

/// Store a MQ reference at the given fd slot.
fn store_mq_fd(fd: usize, mq: alloc::sync::Arc<PosixMq>) {
    let idx = fd - 512;
    if idx >= MQ_FDS_MAX {
        return;
    }
    let pid = crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0);
    let mut table = MQ_FD_TABLE.lock();
    table[idx] = Some(MqFdSlot { pid, mq });
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
        let prev = mq.refcount.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 && mq.is_unlinked() {
            let mut global = MQ_TABLE.lock();
            for gslot in global.iter_mut() {
                if let Some(ref g) = *gslot {
                    if g.is_unlinked() && g.name == mq.name {
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
            // Clear notification if this process was the notifier
            let _ = slot.mq.notify_pid.swap(0, Ordering::Relaxed);
            // Decrement refcount
            let prev = slot.mq.refcount.fetch_sub(1, Ordering::Relaxed);
            // If unlinked and last reference, free from global table
            if prev == 1 && slot.mq.is_unlinked() {
                drop(table);
                let mut global = MQ_TABLE.lock();
                for gslot in global.iter_mut() {
                    if let Some(ref g) = *gslot {
                        if g.is_unlinked() && g.name == slot.mq.name {
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
