//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! inotify(7) — filesystem event notification (P0-4).
//!
//! Minimal complete implementation:
//! - `inotify_init1(2)` creates an instance (event queue + watch table)
//!   and returns an fd backed by `INOTIFY_OPS` FileOps;
//! - `inotify_add_watch(2)` resolves the path, identifies the inode by
//!   `(fs_id, ino)` and registers a watch in the global registry; the wd
//!   is per-instance and monotonically increasing;
//! - VFS change points call [`notify`]/[`notify_file`] which enqueues
//!   `struct inotify_event` records on every registered instance whose
//!   watch mask matches, and wakes blocked readers;
//! - `read(2)` drains the queue (blocking with wait-queue discipline,
//!   EAGAIN under O_NONBLOCK/IN_NONBLOCK); `poll` reports POLLIN when
//!   the queue is non-empty;
//! - the queue is capped at MAX_QUEUED_EVENTS (Linux default 16384);
//!   on overflow the next read reports IN_Q_OVERFLOW;
//! - IN_ONESHOT watches are removed after their first event; a deleted
//!   watched inode gets IN_DELETE_SELF + IN_IGNORED and its watches are
//!   removed automatically.
//!
//! Lock order: WATCH_REGISTRY -> InotifyInstance.inner (never nested the
//! other way; add/rm_watch take them sequentially).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use crate::fs::file::{File, FileOps, FileFlags, get_file_fd_install};
use crate::fs::inode::Inode;
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// UAPI constants
// ============================================================================

/// inotify event bits (inotify(7)).
pub mod inotify_events {
    /// File was accessed (read).
    pub const IN_ACCESS: u32 = 0x0000_0001;
    /// File was modified.
    pub const IN_MODIFY: u32 = 0x0000_0002;
    /// Metadata changed.
    pub const IN_ATTRIB: u32 = 0x0000_0004;
    /// Writable file was closed.
    pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
    /// Unwritable file was closed.
    pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
    /// File was opened.
    pub const IN_OPEN: u32 = 0x0000_0020;
    /// File moved out of watched dir.
    pub const IN_MOVED_FROM: u32 = 0x0000_0040;
    /// File moved into watched dir.
    pub const IN_MOVED_TO: u32 = 0x0000_0080;
    /// File/dir created in watched dir.
    pub const IN_CREATE: u32 = 0x0000_0100;
    /// File/dir deleted from watched dir.
    pub const IN_DELETE: u32 = 0x0000_0200;
    /// Watched file/dir was itself deleted.
    pub const IN_DELETE_SELF: u32 = 0x0000_0400;
    /// Watched file/dir was itself moved.
    pub const IN_MOVE_SELF: u32 = 0x0000_0800;

    /// Watch was removed (explicitly or automatically). Always delivered.
    pub const IN_IGNORED: u32 = 0x0000_8000;
    /// Event queue overflowed.
    pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
    /// Filesystem containing the watched inode was unmounted.
    pub const IN_UNMOUNT: u32 = 0x0000_2000;

    /// Subject of the event is a directory (modifier, not maskable).
    pub const IN_ISDIR: u32 = 0x4000_0000;
    /// Add to the existing mask instead of replacing it.
    pub const IN_MASK_ADD: u32 = 0x2000_0000;
    /// Remove the watch after one event.
    pub const IN_ONESHOT: u32 = 0x8000_0000;

    /// Bits a user may ask to watch.
    pub const IN_ALL_EVENTS: u32 = IN_ACCESS
        | IN_MODIFY
        | IN_ATTRIB
        | IN_CLOSE_WRITE
        | IN_CLOSE_NOWRITE
        | IN_OPEN
        | IN_MOVED_FROM
        | IN_MOVED_TO
        | IN_CREATE
        | IN_DELETE
        | IN_DELETE_SELF
        | IN_MOVE_SELF;
}

use inotify_events::*;

/// inotify_init1 flags (same encoding as the O_* flags).
pub const IN_CLOEXEC: u32 = FileFlags::O_CLOEXEC; // 0o2000000
pub const IN_NONBLOCK: u32 = FileFlags::O_NONBLOCK; // 0o4000

/// Default event-queue cap (Linux /proc/sys/fs/inotify/max_queued_events).
pub const MAX_QUEUED_EVENTS: usize = 16384;

// ============================================================================
// Instance structures
// ============================================================================

/// One queued event (pre-serialized form). `name` carries the directory
/// entry name (no NUL) for events reported through a parent watch.
struct QueuedEvent {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: Option<Vec<u8>>,
}

/// A watch owned by one instance.
#[derive(Clone, Copy)]
struct Watch {
    wd: i32,
    key: (u64, u64),
    mask: u32,
}

struct InotifyInner {
    watches: Vec<Watch>,
    queue: VecDeque<QueuedEvent>,
    next_wd: i32,
    /// Set when an event was dropped due to a full queue; the next read
    /// reports IN_Q_OVERFLOW first.
    overflow: bool,
}

/// An inotify instance. Shared as `Arc<InotifyInstance>` between the
/// File's private_data and the global watch registry; the FileOps close
/// op removes the registry entries before dropping the Arc.
pub struct InotifyInstance {
    inner: Spinlock<InotifyInner>,
    /// Readers blocked on an empty queue.
    wait_queue: WaitQueueHead,
}

impl InotifyInstance {
    fn new() -> Self {
        Self {
            inner: Spinlock::new(InotifyInner {
                watches: Vec::new(),
                queue: VecDeque::new(),
                next_wd: 1,
                overflow: false,
            }),
            wait_queue: WaitQueueHead::new(),
        }
    }

    /// Queue an event for watch `wd` of this instance if the watch's mask
    /// selects it (IN_ISDIR/IN_IGNORED/IN_Q_OVERFLOW/IN_UNMOUNT are
    /// delivered with the event regardless of the mask). Handles
    /// IN_ONESHOT removal and queue-overflow marking, then wakes readers.
    fn enqueue(&self, wd: i32, raw_mask: u32, cookie: u32, name: Option<Vec<u8>>) {
        {
            let mut inner = self.inner.lock();
            let watch_mask = match inner.watches.iter().find(|w| w.wd == wd) {
                Some(w) => w.mask,
                None => return,
            };
            let effective = raw_mask
                & (watch_mask | IN_ISDIR | IN_IGNORED | IN_Q_OVERFLOW | IN_UNMOUNT);
            if effective == 0 {
                return;
            }
            if inner.queue.len() >= MAX_QUEUED_EVENTS {
                inner.overflow = true;
                return;
            }
            inner.queue.push_back(QueuedEvent {
                wd,
                mask: effective,
                cookie,
                name,
            });
            // IN_ONESHOT: the watch dies after its first event.
            if watch_mask & IN_ONESHOT != 0 {
                inner.watches.retain(|w| w.wd != wd);
            }
        }
        self.wait_queue.wake_up_all();
    }
}

// ============================================================================
// Global watch registry (inode key -> watching instances)
// ============================================================================

struct WatchRegistration {
    key: (u64, u64),
    instance: Arc<InotifyInstance>,
    wd: i32,
    mask: u32,
}

static WATCH_REGISTRY: Spinlock<Vec<WatchRegistration>> = Spinlock::new(Vec::new());

/// Fast-path guard: 0 watchers makes every notify() one atomic load
/// (read/write hot paths call notify unconditionally).
static WATCH_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Rename cookie: nonzero value connecting IN_MOVED_FROM/IN_MOVED_TO.
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

/// Allocate a rename cookie (used by vfs_rename to pair the halves).
pub fn alloc_cookie() -> u32 {
    NEXT_COOKIE.fetch_add(1, Ordering::Relaxed)
}

fn inode_key(inode: &Inode) -> (u64, u64) {
    (inode.fs_id, inode.ino)
}

// ============================================================================
// Notification entry points (called from VFS change points)
// ============================================================================

/// Core notify.
///
/// - `inode`: subject of the event (file being opened/modified/deleted).
///   Watches ON that inode receive a name-less event.
/// - `parent`: directory containing the subject. Watches on the parent
///   receive the event WITH `name` (directory-watch semantics: vim/cargo
///   watch the directory and match on name).
/// - `mask`: event bits; the caller adds IN_ISDIR when the subject is a
///   directory.
/// - `cookie`: nonzero to pair rename halves.
///
/// Safe to call from any context (spinlock-protected queue).
pub fn notify(
    parent: Option<&Inode>,
    inode: Option<&Inode>,
    mask: u32,
    name: Option<&[u8]>,
    cookie: u32,
) {
    if WATCH_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    let parent_key = parent.map(inode_key);
    let inode_key = inode.map(inode_key);

    // Snapshot matching registrations under the registry lock; enqueue
    // happens under the instance lock (lock order registry -> inner).
    // Direct watches on the subject get the NAME-LESS event; watches on
    // the parent directory get the same event TAGGED with the entry
    // name. Routing by registration key (never by "has any watch") keeps
    // a directory-only watcher from receiving a confusing name-less
    // file event on its own wd.
    let (inode_targets, parent_targets): (Vec<_>, Vec<_>) = {
        let registry = WATCH_REGISTRY.lock();
        let mut inode_targets = Vec::new();
        let mut parent_targets = Vec::new();
        for r in registry.iter() {
            if inode_key == Some(r.key) {
                inode_targets.push((Arc::clone(&r.instance), r.wd));
            }
            if parent_key == Some(r.key) && name.is_some() {
                parent_targets.push((Arc::clone(&r.instance), r.wd));
            }
        }
        (inode_targets, parent_targets)
    };

    for (instance, wd) in inode_targets {
        instance.enqueue(wd, mask, cookie, None);
    }
    if let Some(n) = name {
        for (instance, wd) in parent_targets {
            instance.enqueue(wd, mask, cookie, Some(Vec::from(n)));
        }
    }

    // Automatic watch removal: the watched inode itself was deleted.
    // Linux delivers IN_DELETE_SELF (if masked) — done above — then
    // IN_IGNORED (always) and drops every watch on the inode.
    if let Some(ik) = inode_key {
        if mask & IN_DELETE_SELF != 0 {
            let victims: Vec<Arc<InotifyInstance>> = {
                let registry = WATCH_REGISTRY.lock();
                for r in registry.iter().filter(|r| r.key == ik) {
                    r.instance.enqueue(r.wd, IN_IGNORED, 0, None);
                }
                registry
                    .iter()
                    .filter(|r| r.key == ik)
                    .map(|r| Arc::clone(&r.instance))
                    .collect()
            };
            for inst in victims {
                let mut inner = inst.inner.lock();
                inner.watches.retain(|w| w.key != ik);
            }
            let mut registry = WATCH_REGISTRY.lock();
            registry.retain(|r| r.key != ik);
            WATCH_COUNT.store(registry.len(), Ordering::Release);
        }
    }
}

/// Notify through a File: extracts the subject inode plus the parent
/// dentry/name so directory watches see named events. Used for
/// IN_OPEN/IN_ACCESS/IN_MODIFY/IN_CLOSE_* on the read/write/close paths.
pub fn notify_file(file: &File, mask: u32) {
    if WATCH_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    // SAFETY: inode and dentry are written once at open time and never
    // mutated afterwards; read-only access.
    let inode = unsafe { (*file.inode.get()).clone() };
    let dentry = unsafe { (*file.dentry.get()).clone() };
    let inode = match inode {
        Some(i) => i,
        None => return,
    };
    let mut mask = mask;
    if inode.mode.is_directory() {
        mask |= IN_ISDIR;
    }
    let (parent, name): (Option<Arc<Inode>>, Option<Vec<u8>>) = match &dentry {
        Some(d) => {
            let name = d.get_name();
            let parent_inode = d.parent.lock().clone().and_then(|p| p.get_inode());
            (
                parent_inode,
                if name.is_empty() { None } else { Some(name.into_bytes()) },
            )
        }
        None => (None, None),
    };
    notify(parent.as_deref(), Some(&inode), mask, name.as_deref(), 0);
}

/// Close-time event for a File (IN_CLOSE_WRITE / IN_CLOSE_NOWRITE).
pub fn notify_file_close(file: &File) {
    let mask = if file.flags().is_readonly() {
        IN_CLOSE_NOWRITE
    } else {
        IN_CLOSE_WRITE
    };
    notify_file(file, mask);
}

// ============================================================================
// Instance API (syscalls)
// ============================================================================

/// Create an inotify instance and install its fd. Returns the fd.
pub fn inotify_init1(flags: u32) -> Result<usize, i32> {
    let bad_bits = !(IN_CLOEXEC | IN_NONBLOCK);
    if flags & bad_bits != 0 {
        return Err(-crate::errno::constants::EINVAL);
    }

    let file = Arc::new(File::new(FileFlags::new(flags & IN_NONBLOCK)));
    file.set_ops(&INOTIFY_OPS);
    let instance = Arc::new(InotifyInstance::new());
    // Ownership: the raw Arc reference lives in private_data until the
    // close op reclaims it (pipe.rs discipline).
    file.set_private_data(Arc::into_raw(instance) as *mut u8);

    // SAFETY: get_file_fd_install installs the Arc<File> into the current
    // task's fd table (well-defined safe-to-call syscall-layer helper).
    match unsafe { get_file_fd_install(file) } {
        Some(fd) => {
            if flags & IN_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            Ok(fd)
        }
        None => Err(-crate::errno::constants::EMFILE),
    }
}

/// Recover the Arc<InotifyInstance> from a File's private_data without
/// consuming the stored reference (identity-checked against INOTIFY_OPS).
fn instance_of(file: &File) -> Option<Arc<InotifyInstance>> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &INOTIFY_OPS as *const _) {
        return None;
    }
    // SAFETY: private_data was installed by inotify_init1 as
    // Arc::into_raw and stays valid while the File exists; the recovered
    // Arc reference is cloned and re-forgotten, never dropped.
    let ptr = unsafe { *file.private_data.get() }?;
    let arc = unsafe { Arc::from_raw(ptr as *const InotifyInstance) };
    let cloned = Arc::clone(&arc);
    core::mem::forget(arc);
    Some(cloned)
}

/// inotify_add_watch: register (or update) a watch for `path`.
/// Returns the watch descriptor.
pub fn inotify_add_watch(fd: usize, path: &str, mask: u32) -> Result<i32, i32> {
    // Must request at least one event bit (Linux: EINVAL otherwise).
    if mask & (IN_ALL_EVENTS | IN_ATTRIB) == 0 {
        return Err(-crate::errno::constants::EINVAL);
    }
    let file = unsafe { crate::fs::file::get_file_fd(fd) }
        .ok_or(-crate::errno::constants::EBADF)?;
    let instance = instance_of(&file).ok_or(-crate::errno::constants::EINVAL)?;

    // Resolve the path to an inode (following symlinks, like Linux).
    let vpath = crate::fs::vfs::path_lookup(path, 0)?;
    let inode = vpath
        .inode
        .ok_or(-crate::errno::constants::ENOENT)?;
    let key = inode_key(&inode);

    let effective_mask = mask & (IN_ALL_EVENTS | IN_MASK_ADD | IN_ONESHOT);

    // Per-instance watch list: existing watch on the same inode keeps its
    // wd; IN_MASK_ADD ORs into it, otherwise the mask is replaced.
    let wd = {
        let mut inner = instance.inner.lock();
        if let Some(existing) = inner.watches.iter_mut().find(|w| w.key == key) {
            if effective_mask & IN_MASK_ADD != 0 {
                existing.mask |= effective_mask & !IN_MASK_ADD;
            } else {
                existing.mask = effective_mask;
            }
            existing.wd
        } else {
            let wd = inner.next_wd;
            inner.next_wd += 1;
            inner.watches.push(Watch {
                wd,
                key,
                mask: effective_mask & !IN_MASK_ADD,
            });
            wd
        }
    };

    // Mirror into the global registry (upsert, keyed by instance+inode).
    {
        let mut registry = WATCH_REGISTRY.lock();
        let stored_mask = effective_mask & !IN_MASK_ADD;
        let mut found = false;
        for r in registry.iter_mut() {
            if r.key == key && Arc::ptr_eq(&r.instance, &instance) {
                r.mask = stored_mask;
                found = true;
                break;
            }
        }
        if !found {
            registry.push(WatchRegistration {
                key,
                instance: Arc::clone(&instance),
                wd,
                mask: stored_mask,
            });
        }
        WATCH_COUNT.store(registry.len(), Ordering::Release);
    }

    Ok(wd)
}

/// inotify_rm_watch: remove a watch by wd. Queues IN_IGNORED (Linux
/// always delivers it on watch removal).
pub fn inotify_rm_watch(fd: usize, wd: i32) -> Result<(), i32> {
    let file = unsafe { crate::fs::file::get_file_fd(fd) }
        .ok_or(-crate::errno::constants::EBADF)?;
    let instance = instance_of(&file).ok_or(-crate::errno::constants::EINVAL)?;

    let key = {
        let mut inner = instance.inner.lock();
        let watch = inner.watches.iter().find(|w| w.wd == wd).copied();
        match watch {
            Some(w) => {
                // Queue IN_IGNORED while the watch is still registered —
                // enqueue() delivers by looking the wd up in the watch
                // list (IN_IGNORED bypasses the mask check but not the
                // watch lookup). Then remove it.
                let ev = QueuedEvent {
                    wd,
                    mask: IN_IGNORED,
                    cookie: 0,
                    name: None,
                };
                if inner.queue.len() < MAX_QUEUED_EVENTS {
                    inner.queue.push_back(ev);
                }
                inner.watches.retain(|w| w.wd != wd);
                w.key
            }
            None => return Err(-crate::errno::constants::EINVAL),
        }
    };

    {
        let mut registry = WATCH_REGISTRY.lock();
        registry.retain(|r| !(r.key == key && Arc::ptr_eq(&r.instance, &instance)));
        WATCH_COUNT.store(registry.len(), Ordering::Release);
    }

    // Wake readers parked on the queue we just appended to.
    instance.wait_queue.wake_up_all();
    Ok(())
}

// ============================================================================
// FileOps: read / poll / close
// ============================================================================

/// Serialize one event into `buf`, returning bytes written (0 when it
/// does not fit). Record layout: struct inotify_event
/// { s32 wd; u32 mask; u32 cookie; u32 len; char name[len]; } — the name
/// is NUL-terminated and NUL-padded to 4-byte alignment (inotify(7)).
fn write_event(buf: &mut [u8], ev: &QueuedEvent) -> usize {
    let name_len = match &ev.name {
        Some(n) => (n.len() + 1 + 3) & !3usize, // + NUL, pad to multiple of 4
        None => 0,
    };
    let total = 16 + name_len;
    if buf.len() < total {
        return 0;
    }
    buf[0..4].copy_from_slice(&ev.wd.to_le_bytes());
    buf[4..8].copy_from_slice(&ev.mask.to_le_bytes());
    buf[8..12].copy_from_slice(&ev.cookie.to_le_bytes());
    buf[12..16].copy_from_slice(&(name_len as u32).to_le_bytes());
    if name_len > 0 {
        let n = ev.name.as_ref().unwrap();
        buf[16..16 + n.len()].copy_from_slice(n);
        for b in buf[16 + n.len()..16 + name_len].iter_mut() {
            *b = 0;
        }
    }
    total
}

fn inotify_read(file: &File, buf: &mut [u8]) -> isize {
    let instance = match instance_of(file) {
        Some(i) => i,
        None => return -crate::errno::constants::EBADF as isize,
    };
    let nonblock = file.flags().bits() & FileFlags::O_NONBLOCK != 0;

    // A buffer too small for even the event header is EINVAL (Linux).
    if buf.len() < 16 {
        return -crate::errno::constants::EINVAL as isize;
    }

    loop {
        let mut total = 0usize;
        let stuck = {
            let mut inner = instance.inner.lock();
            if inner.overflow {
                // Report the overflow once, before anything else.
                inner.overflow = false;
                let ev = QueuedEvent {
                    wd: -1,
                    mask: IN_Q_OVERFLOW,
                    cookie: 0,
                    name: None,
                };
                let n = write_event(&mut buf[total..], &ev);
                if n == 0 {
                    inner.overflow = true;
                    return -crate::errno::constants::EINVAL as isize;
                }
                total += n;
            }
            // Drain whole events; never split a partial event.
            while let Some(ev) = inner.queue.front() {
                let n = write_event(&mut buf[total..], ev);
                if n == 0 {
                    break;
                }
                inner.queue.pop_front();
                total += n;
            }
            // Queue non-empty but nothing drained: the first (named)
            // event does not fit the buffer — EINVAL, like Linux, never
            // a blocking loop on a non-empty queue.
            !inner.queue.is_empty() && total == 0
        };
        if total > 0 {
            return total as isize;
        }
        if stuck {
            return -crate::errno::constants::EINVAL as isize;
        }

        // Queue empty.
        if nonblock {
            return -crate::errno::constants::EAGAIN as isize;
        }

        // Blocking read: wait-queue discipline (prepare_to_wait,
        // recheck, schedule, finish) mirroring pipe_file_read.
        let current = match crate::sched::current() {
            Some(task) => task,
            None => return 0,
        };
        instance.wait_queue.prepare_to_wait(current, false, true);

        let has_events = {
            let inner = instance.inner.lock();
            !inner.queue.is_empty() || inner.overflow
        };
        if has_events {
            instance.wait_queue.finish_wait(current);
            // Undo a concurrent event-arrival wake that enqueued us
            // while we never slept (R36-B2 discipline).
            crate::sched::dequeue_task(&*current);
            continue;
        }
        if crate::signal::signal_pending() {
            instance.wait_queue.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return -crate::errno::constants::EINTR as isize;
        }
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        instance.wait_queue.finish_wait(current);
        if crate::signal::signal_pending() {
            return -crate::errno::constants::EINTR as isize;
        }
        // Loop: drain what arrived.
    }
}

fn inotify_poll(file: &File, _events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let instance = match instance_of(file) {
        Some(i) => i,
        None => return 0,
    };
    let inner = instance.inner.lock();
    if !inner.queue.is_empty() || inner.overflow {
        POLLIN | POLLRDNORM
    } else {
        0
    }
}

fn inotify_close(file: &File) -> i32 {
    // SAFETY: private_data.take() hands us the Arc::into_raw'd reference
    // exactly once (close runs once, at last reference).
    let ptr = unsafe { file.private_data.get().replace(None) };
    if let Some(ptr) = ptr {
        let instance = unsafe { Arc::from_raw(ptr as *const InotifyInstance) };
        // Watches die with the fd: drop every registry entry of this
        // instance (retain matches on Arc identity — no inner lock
        // needed, so no registry -> inner nesting here).
        {
            let mut registry = WATCH_REGISTRY.lock();
            registry.retain(|r| !Arc::ptr_eq(&r.instance, &instance));
            WATCH_COUNT.store(registry.len(), Ordering::Release);
        }
        // Wake blocked readers so they observe the now-dead instance.
        instance.wait_queue.wake_up_all();
        // `instance` (the recovered reference) drops here.
    }
    0
}

/// FileOps for inotify instance fds.
static INOTIFY_OPS: FileOps = FileOps {
    read: Some(inotify_read),
    write: None,
    lseek: None,
    close: Some(inotify_close),
    poll: Some(inotify_poll),
};
