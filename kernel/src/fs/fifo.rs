//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! FIFO (named pipe) support — P1 mknod/mkfifo.
//!
//! A FIFO is a directory entry with S_IFIFO type whose data path is the
//! anonymous-pipe machinery (`fs::pipe`). The link between the on-disk /
//! in-memory inode and the live pipe is a global registry keyed by the
//! inode identity `(fs_id, ino)`:
//!
//! - `mknod(path, S_IFIFO|mode, ...)` creates the file (regular create on
//!   the parent FS), retypes it to S_IFIFO and registers a fresh FifoPeer.
//! - `open(fifo)` looks the peer up by the resolved inode's (fs_id, ino)
//!   and applies POSIX open semantics (see `fifo_open_file`).
//! - `unlink(fifo)` drops the registry entry; fds that still hold the pipe
//!   keep it alive through their private_data Arc (POSIX: an unlinked FIFO
//!   that is still open keeps working).
//!
//! Re-creation of the same path allocates a NEW inode number → a new peer,
//! matching Linux (the old inode's pipe dies with its last fd).

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::errno;
use crate::fs::file::{File, FileFlags, FileOps};
use crate::fs::pipe::Pipe;
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

/// Live state of one FIFO inode.
pub struct FifoPeer {
    /// The shared pipe buffer + data wait queues.
    pub pipe: Arc<Pipe>,
    /// Open reader fds (O_RDONLY / O_RDWR).
    readers: AtomicUsize,
    /// Open writer fds (O_WRONLY / O_RDWR).
    writers: AtomicUsize,
    /// Wait queue for blocking opens (reader waiting for a writer and
    /// vice versa) — woken on every counter transition 0↔n.
    open_queue: WaitQueueHead,
}

impl FifoPeer {
    fn new() -> Self {
        Self {
            pipe: Arc::new(Pipe::new()),
            readers: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
            open_queue: WaitQueueHead::new(),
        }
    }

    pub fn readers(&self) -> usize {
        self.readers.load(Ordering::Acquire)
    }

    pub fn writers(&self) -> usize {
        self.writers.load(Ordering::Acquire)
    }
}

/// Global FIFO registry: (fs_id, ino) → live peer.
static FIFO_REGISTRY: Spinlock<BTreeMap<(u64, u64), Arc<FifoPeer>>> =
    Spinlock::new(BTreeMap::new());

/// Get (or lazily create) the peer for a FIFO inode. The lazy creation
/// covers FIFO inodes that predate the registry entry (e.g. a FIFO created
/// before this wave, or a rootfs tree built at boot): they get a fresh
/// empty pipe, which is the best possible reconstruction.
pub fn peer_for(fs_id: u64, ino: u64) -> Arc<FifoPeer> {
    let mut reg = FIFO_REGISTRY.lock();
    reg.entry((fs_id, ino)).or_insert_with(|| Arc::new(FifoPeer::new())).clone()
}

/// Drop the registry entry for an unlinked FIFO inode. The peer (and its
/// pipe) survive while open fds hold their Arc; only future opens of the
/// same inode number (impossible after unlink frees the inode) would miss.
pub fn forget(fs_id: u64, ino: u64) {
    FIFO_REGISTRY.lock().remove(&(fs_id, ino));
}

/// Blocking wait until `pred()` holds, interruptible by signals.
/// Mirrors the pipe read/write wait discipline (prepare_to_wait + recheck
/// to avoid the lost-wakeup race). Returns Err(EINTR) on a pending signal.
fn wait_event_open(peer: &FifoPeer, pred: impl Fn() -> bool) -> Result<(), i32> {
    loop {
        if pred() {
            return Ok(());
        }
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return Ok(()), // no task context: don't block
        };
        peer.open_queue.prepare_to_wait(current, false, true);
        // Re-check AFTER registering (lost-wakeup discipline).
        if pred() {
            peer.open_queue.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return Ok(());
        }
        if crate::signal::signal_pending() {
            peer.open_queue.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
        }
        // R54: re-arm IRQs across the schedule so timers/IPIs reach this CPU.
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        peer.open_queue.finish_wait(current);
        if crate::signal::signal_pending() {
            return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
        }
    }
}

/// Open one end of a FIFO. POSIX semantics (Linux fifo_open):
/// - O_RDONLY blocking: waits until at least one writer opened the FIFO;
///   O_RDONLY|O_NONBLOCK: succeeds immediately (a later read blocks/EAGAINs).
/// - O_WRONLY blocking: waits until at least one reader opened the FIFO;
///   O_WRONLY|O_NONBLOCK with no reader: ENXIO.
/// - O_RDWR: succeeds immediately (Linux behavior; also the classic
///   avoid-blocking trick).
pub fn fifo_open_file(peer: &Arc<FifoPeer>, flags: u32) -> Result<Arc<File>, i32> {
    let nonblock = flags & FileFlags::O_NONBLOCK != 0;
    match flags & FileFlags::O_ACCMODE {
        FileFlags::O_RDONLY => {
            if !nonblock {
                wait_event_open(peer, || peer.writers() > 0)?;
            }
        }
        FileFlags::O_WRONLY => {
            if nonblock {
                if peer.readers() == 0 {
                    return Err(errno::Errno::NoSuchDeviceOrAddress.as_neg_i32()); // ENXIO
                }
            } else {
                wait_event_open(peer, || peer.readers() > 0)?;
            }
        }
        _ => {
            // O_RDWR (or O_PATH-ish accmode 3): both counters below.
        }
    }

    let accmode = flags & FileFlags::O_ACCMODE;
    if accmode == FileFlags::O_RDONLY || accmode == FileFlags::O_RDWR {
        let prev = peer.readers.fetch_add(1, Ordering::AcqRel);
        if prev == 0 {
            peer.pipe.reopen_read();
            peer.open_queue.wake_up_all();
        }
    }
    if accmode == FileFlags::O_WRONLY || accmode == FileFlags::O_RDWR {
        let prev = peer.writers.fetch_add(1, Ordering::AcqRel);
        if prev == 0 {
            peer.pipe.reopen_write();
            peer.open_queue.wake_up_all();
        }
    }

    let file = Arc::new(File::new(FileFlags::new(flags)));
    file.set_ops(&FIFO_OPS);
    // One Arc reference per open File — fifo_file_close consumes it.
    file.set_private_data(Arc::into_raw(Arc::clone(&peer.pipe)) as *mut u8);
    Ok(file)
}

// ============================================================================
// File operations — data path is the anonymous-pipe one verbatim; only
// close differs (peer counter bookkeeping + reopen of the pipe ends).
// ============================================================================

fn fifo_file_read(file: &File, buf: &mut [u8]) -> isize {
    crate::fs::pipe::pipe_file_read(file, buf)
}

fn fifo_file_write(file: &File, buf: &[u8]) -> isize {
    crate::fs::pipe::pipe_file_write(file, buf)
}

fn fifo_file_poll(file: &File, events: u16) -> u16 {
    crate::fs::pipe::pipe_file_poll(file, events)
}

/// FIFO close: drop our pipe Arc reference and close the pipe ends ONLY
/// when this was the last open File of that direction (the anonymous-pipe
/// close marks the end closed unconditionally — correct for its one-File-
/// per-end model, wrong for a FIFO with several concurrent readers or
/// writers). The peer counters live in the registry entry, recovered via
/// the File's inode identity; an UNLINKED FIFO (registry entry gone) has
/// no future opens to protect, so the ends close unconditionally.
fn fifo_file_close(file: &File) -> i32 {
    let accmode = file.flags().bits() & FileFlags::O_ACCMODE;
    let is_reader = accmode == FileFlags::O_RDONLY || accmode == FileFlags::O_RDWR;
    let is_writer = accmode == FileFlags::O_WRONLY || accmode == FileFlags::O_RDWR;

    // Last-of-direction decision (registry-aware), wakeups outside the lock.
    // SAFETY: inode is written once at open time; read-only access here.
    let inode_opt = unsafe { (*file.inode.get()).clone() };
    let (last_reader, last_writer, wake_peer) = if let Some(ref inode) = inode_opt {
        let reg_peer = FIFO_REGISTRY.lock().get(&(inode.fs_id, inode.ino)).cloned();
        match reg_peer {
            Some(peer) => {
                let lr = if is_reader {
                    peer.readers.fetch_sub(1, Ordering::AcqRel) == 1
                } else {
                    false
                };
                let lw = if is_writer {
                    peer.writers.fetch_sub(1, Ordering::AcqRel) == 1
                } else {
                    false
                };
                (lr, lw, Some(peer))
            }
            // Unlinked FIFO: no bookkeeping to update.
            None => (is_reader, is_writer, None),
        }
    } else {
        (is_reader, is_writer, None)
    };
    if let Some(peer) = wake_peer {
        // New blocking opens of the other direction may proceed now.
        peer.open_queue.wake_up_all();
    }

    // Pipe end close + Arc drop.
    // SAFETY: private_data was installed by fifo_open_file as
    // Arc::into_raw(Pipe) and remains valid while the File exists.
    unsafe {
        if let Some(ptr) = file.private_data.get().replace(None) {
            let pipe = Arc::from_raw(ptr as *const Pipe);
            if last_reader {
                pipe.close_read();
            }
            if last_writer {
                pipe.close_write();
            }
            drop(pipe);
        }
    }
    0
}

/// FIFO file operations: pipe data path + FIFO-aware close.
static FIFO_OPS: FileOps = FileOps {
    read: Some(fifo_file_read),
    write: Some(fifo_file_write),
    lseek: None, // unseekable (ESPIPE via the default in File::lseek)
    close: Some(fifo_file_close),
    poll: Some(fifo_file_poll),
};
