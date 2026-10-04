//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Pipe Filesystem
//!
//!
//! Core concepts:
//! - `struct pipe_inode_info`: Pipe information
//! - `struct pipe_buffer`: Pipe buffer
//! - Synchronous read/write operations
//!
//! POSIX guarantees implemented here:
//! - Writes of at most PIPE_BUF (4096) bytes are ATOMIC: a blocking writer
//!   waits until the WHOLE request fits, so chunks from concurrent writers
//!   never interleave (review 5.2: PIPE_BUF atomicity was broken).
//! - Pipe capacity is 64 KiB (Linux default, review 5.2: was 16 KiB).

use alloc::vec;
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::sync::Arc;
use crate::process::wait::WaitQueueHead;

/// Pipe buffer capacity (bytes). Linux default is 16 pages = 64 KiB.
/// Was 16 KiB (review 5.2: pipe capacity below Linux default).
/// NOTE: defined locally instead of config.rs — the shared config file is
/// concurrently edited by other repair agents.
pub const PIPE_BUF_SIZE: usize = 65536;
/// Public alias for the pipe capacity (F_GETPIPE_SZ).
pub const PIPE_CAPACITY: usize = PIPE_BUF_SIZE;

/// POSIX PIPE_BUF: max write size guaranteed atomic
const PIPE_BUF: usize = 4096;

#[repr(C)]
pub struct PipeBuffer {
    /// Buffer data
    data: Vec<u8>,
    /// Read pointer
    read_pos: AtomicUsize,
    /// Write pointer
    write_pos: AtomicUsize,
    /// Buffer size
    size: usize,
}

impl PipeBuffer {
    /// Create new pipe buffer with USABLE capacity `capacity` bytes.
    ///
    /// The ring keeps one slot empty to distinguish full from empty, so it
    /// allocates capacity+1 slots — the old code allocated exactly
    /// `capacity` slots and thus could only ever hold capacity-1 bytes
    /// while F_GETPIPE_SZ reported `capacity` (LTP pipe2_04: a full
    /// 65536-byte write into a fresh pipe returned 65535).
    pub fn new(capacity: usize) -> Self {
        // Manually allocate and initialize vector to avoid vec! macro
        let ring_len = capacity + 1;
        let mut data = Vec::with_capacity(ring_len);
        unsafe {
            core::ptr::write_bytes(data.as_mut_ptr(), 0, ring_len);
            data.set_len(ring_len);
        }

        Self {
            data,
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            size: ring_len,
        }
    }

    /// Usable capacity in bytes (ring length - 1 reserved slot).
    pub fn capacity(&self) -> usize {
        self.size - 1
    }

    /// Resize the ring to a new usable capacity, PRESERVING buffered data
    /// (F_SETPIPE_SZ semantics; Linux only fails with ENOSPC-ish EBUSY
    /// analogues we do not model — data that no longer fits is dropped).
    pub fn resize(&mut self, new_capacity: usize) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);
        let buffered = if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        };
        let keep = core::cmp::min(buffered, new_capacity);

        // Copy the OLDEST `keep` bytes out in FIFO order (split-segment
        // bulk copy — see read()/write() for why per-byte modulo is slow).
        let mut staged: Vec<u8> = vec![0u8; keep];
        let first = core::cmp::min(keep, self.size - read_pos);
        staged[..first].copy_from_slice(&self.data[read_pos..read_pos + first]);
        if keep > first {
            staged[first..keep].copy_from_slice(&self.data[0..keep - first]);
        }

        let ring_len = new_capacity + 1;
        let mut data = Vec::with_capacity(ring_len);
        unsafe {
            core::ptr::write_bytes(data.as_mut_ptr(), 0, ring_len);
            data.set_len(ring_len);
        }
        for (i, b) in staged.iter().enumerate() {
            data[i] = *b;
        }

        self.data = data;
        self.size = ring_len;
        self.read_pos.store(0, Ordering::Release);
        self.write_pos.store(keep, Ordering::Release);
        new_capacity
    }

    /// Read data from ring buffer into KERNEL memory (see read_xfer for the
    /// split-segment bulk-copy rationale and the generic version).
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if read_pos == write_pos {
            return 0; // Buffer empty
        }

        let total_available = if write_pos > read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        };

        let to_read = core::cmp::min(total_available, buf.len());
        if to_read == 0 {
            return 0;
        }

        // First segment: read_pos .. (wrap or end of copy)
        let first_len = core::cmp::min(to_read, self.size - read_pos);
        // SAFETY-free slice copy; both sides are in-bounds by construction.
        buf[..first_len].copy_from_slice(&self.data[read_pos..read_pos + first_len]);

        // Second segment: wrapped tail at the ring start
        if to_read > first_len {
            buf[first_len..to_read].copy_from_slice(&self.data[0..to_read - first_len]);
        }

        self.read_pos.store((read_pos + to_read) % self.size, Ordering::Release);
        to_read
    }

    /// Read from the ring into USER memory (exception-table copies).
    ///
    /// Returns bytes copied; a faulting second segment returns the first
    /// segment's bytes (short read, POSIX-compatible).
    ///
    /// # Safety
    /// `dst` must be a user pointer validated for `max` bytes (access_ok).
    pub unsafe fn read_user(&mut self, dst: *mut u8, max: usize) -> usize {
        use crate::arch::riscv64::uaccess::copy_to_user;
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if read_pos == write_pos || max == 0 {
            return 0;
        }
        let total_available = if write_pos > read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        };
        let to_read = core::cmp::min(total_available, max);
        let first_len = core::cmp::min(to_read, self.size - read_pos);

        // SAFETY: dst is access_ok-validated for max bytes; exception-table
        // copy returns the uncopied count on fault.
        let mut done = 0usize;
        if unsafe { copy_to_user(dst, self.data[read_pos..read_pos + first_len].as_ptr(), first_len) } == 0 {
            done = first_len;
            let second = to_read - first_len;
            if second > 0 {
                if unsafe { copy_to_user(dst.add(first_len), self.data[0..second].as_ptr(), second) } == 0 {
                    done = to_read;
                }
            }
        }
        if done > 0 {
            self.read_pos.store((read_pos + done) % self.size, Ordering::Release);
        }
        done
    }

    /// Write data to ring buffer from KERNEL memory (see write_xfer for the
    /// generic version).
    pub fn write(&mut self, buf: &[u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        // Calculate available space (keep one slot empty to distinguish full from empty)
        let available = if write_pos >= read_pos {
            self.size - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        };

        let to_write = core::cmp::min(available, buf.len());
        if to_write == 0 {
            return 0;
        }

        // First segment: write_pos .. (wrap or end of copy)
        let first_len = core::cmp::min(to_write, self.size - write_pos);
        self.data[write_pos..write_pos + first_len].copy_from_slice(&buf[..first_len]);

        // Second segment: wrapped tail at the ring start
        if to_write > first_len {
            self.data[0..to_write - first_len].copy_from_slice(&buf[first_len..to_write]);
        }

        self.write_pos.store((write_pos + to_write) % self.size, Ordering::Release);
        to_write
    }

    /// Write into the ring from USER memory (exception-table copies).
    /// Returns bytes copied; a fault mid-copy yields a short write.
    ///
    /// # Safety
    /// `src` must be a user pointer validated for `max` bytes (access_ok).
    pub unsafe fn write_user(&mut self, src: *const u8, max: usize) -> usize {
        use crate::arch::riscv64::uaccess::copy_from_user;
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        let available = if write_pos >= read_pos {
            self.size - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        };
        let to_write = core::cmp::min(available, max);
        if to_write == 0 {
            return 0;
        }
        let first_len = core::cmp::min(to_write, self.size - write_pos);

        // SAFETY: src is access_ok-validated for max bytes; exception-table
        // copy returns the uncopied count on fault.
        let mut done = 0usize;
        if unsafe { copy_from_user(self.data[write_pos..write_pos + first_len].as_mut_ptr(), src, first_len) } == 0 {
            done = first_len;
            let second = to_write - first_len;
            if second > 0 {
                if unsafe { copy_from_user(self.data[0..second].as_mut_ptr(), src.add(first_len), second) } == 0 {
                    done = to_write;
                }
            }
        }
        if done > 0 {
            self.write_pos.store((write_pos + done) % self.size, Ordering::Release);
        }
        done
    }

    /// Get available read bytes
    pub fn available_read(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        }
    }

    /// Get available write space
    pub fn available_write(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            self.size - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        }
    }
}

#[repr(C)]
pub struct Pipe {
    /// Pipe buffer
    buffer: Spinlock<PipeBuffer>,
    /// Read end closed
    read_closed: AtomicUsize,
    /// Write end closed
    write_closed: AtomicUsize,
    /// Read wait queue (for read blocking)
    read_queue: WaitQueueHead,
    /// Write wait queue (for write blocking)
    write_queue: WaitQueueHead,
}

impl Pipe {
    /// Create new pipe
    pub fn new() -> Self {
        // Default capacity: the 64KB ring, clamped down to
        // /proc/sys/fs/pipe-max-size for UNPRIVILEGED users (Linux
        // alloc_pipe_info; LTP fcntl35 sets pipe-max-size=4096 and
        // expects nobody's fresh pipe to report 4096 while root keeps
        // 64KB).
        let cap = {
            let max = crate::fs::procfs::sysctl::PIPE_MAX_SIZE
                .load(core::sync::atomic::Ordering::Relaxed) as usize;
            if PIPE_CAPACITY > max
                && !crate::security::capable(
                    crate::security::capability::CAP_SYS_RESOURCE,
                )
            {
                max.max(4096)
            } else {
                PIPE_CAPACITY
            }
        };
        Self {
            buffer: Spinlock::new(PipeBuffer::new(cap)),
            read_closed: AtomicUsize::new(0),
            write_closed: AtomicUsize::new(0),
            read_queue: WaitQueueHead::new(),
            write_queue: WaitQueueHead::new(),
        }
    }

    /// Close read end
    pub fn close_read(&self) {
        self.read_closed.store(1, Ordering::Release);
        // Wake up all write waiters (read end closed causes write to return SIGPIPE)
        self.write_queue.wake_up_all();
    }

    /// Close write end
    pub fn close_write(&self) {
        self.write_closed.store(1, Ordering::Release);
        // Wake up all read waiters (EOF)
        self.read_queue.wake_up_all();
    }

    /// FIFO lifecycle (fs/fifo.rs): re-arm the read end after the last
    /// reader closed and a NEW reader opened — the closed flags are sticky
    /// for anonymous pipes (one File per end) but a FIFO's inode outlives
    /// each open. Readers 0→1 transition clears the flag.
    pub fn reopen_read(&self) {
        self.read_closed.store(0, Ordering::Release);
    }

    /// FIFO lifecycle: writers 0→1 transition clears the write-closed
    /// (EOF) flag so readers block for fresh data instead of seeing the
    /// previous writer's EOF forever.
    pub fn reopen_write(&self) {
        self.write_closed.store(0, Ordering::Release);
        // Data may already be buffered by this new writer — wake readers.
        self.read_queue.wake_up_all();
    }

    /// Check if read end is closed
    pub fn is_read_closed(&self) -> bool {
        self.read_closed.load(Ordering::Acquire) == 1
    }

    /// Check if write end is closed
    pub fn is_write_closed(&self) -> bool {
        self.write_closed.load(Ordering::Acquire) == 1
    }

    /// Get read wait queue
    pub fn read_queue(&self) -> &WaitQueueHead {
        &self.read_queue
    }

    /// Get write wait queue
    pub fn write_queue(&self) -> &WaitQueueHead {
        &self.write_queue
    }
}

/// Dead code removed (review 5.2 low): the free-function pipe_read/pipe_write
/// were never called and bypassed the wait-queue logic of the FileOps
/// versions — keeping them invited misuse.

use crate::fs::file::{File, FileOps, FileFlags};

// ============================================================================
// vmsplice(2) helpers — LTP vmsplice01..04
// ============================================================================

/// Extract the shared `Pipe` behind a pipe `File` (both ends), or None.
pub fn pipe_of_file(file: &File) -> Option<&'static Pipe> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return None;
    }
    // SAFETY: ops identity confirms this is a pipe File; private_data was
    // installed by create_pipe as Arc::into_raw(Pipe) and remains valid
    // while the File exists.
    let ptr = unsafe { *file.private_data.get() }?;
    Some(unsafe { &*(ptr as *const Pipe) })
}

/// True when `file` is a pipe/fifo description (PIPE_OPS) — used by the
/// read/write syscall layer to take the direct user-transfer fast path.
pub fn is_pipe_file(file: &File) -> bool {
    file.get_ops()
        .is_some_and(|ops| core::ptr::eq(ops as *const _, &PIPE_OPS as *const _))
}

/// vmsplice(2) write direction: move user bytes into the pipe. Linux
/// vmsplice_to_pipe fills whatever pipe space exists and returns a SHORT
/// count (callers loop); it blocks only while the pipe is completely full
/// (LTP vmsplice01's poll+partial-write loop depends on this, and
/// vmsplice04's full-pipe blocking case on the wait below).
///
/// `iovs` are (user_base, len) pairs already validated in kernel space.
/// Returns bytes moved, or a negative errno.
pub fn pipe_vmsplice_to(file: &File, iovs: &[(usize, usize)], nonblock: bool) -> isize {
    use crate::arch::riscv64::uaccess::copy_from_user;

    let Some(pipe) = pipe_of_file(file) else {
        return -9; // EBADF
    };
    let file_nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    let nonblock = nonblock || file_nonblock;

    let total_len: usize = iovs.iter().map(|(_, l)| *l).sum();
    if total_len == 0 {
        return 0;
    }

    let mut kbuf = alloc::vec![0u8; total_len.min(PIPE_CAPACITY)];
    loop {
        {
            let mut guard = pipe.buffer.lock();
            let space = guard.available_write();
            if space > 0 {
                let want = total_len.min(space).min(kbuf.len());
                let mut filled = 0usize;
                for &(base, len) in iovs {
                    if filled >= want {
                        break;
                    }
                    let take = len.min(want - filled).min(kbuf.len() - filled);
                    if take == 0 {
                        continue;
                    }
                    // SAFETY: base is a user pointer; exception-table copy.
                    if unsafe {
                        copy_from_user(
                            kbuf.as_mut_ptr().add(filled),
                            base as *const u8,
                            take,
                        )
                    } != 0
                    {
                        if filled > 0 {
                            let n = guard.write(&kbuf[..filled]);
                            drop(guard);
                            pipe.read_queue().wake_up_all();
                            oasync_notify(pipe as *const _ as usize, true);
                            return n as isize;
                        }
                        return -14; // EFAULT
                    }
                    filled += take;
                }
                let n = guard.write(&kbuf[..filled]);
                drop(guard);
                pipe.read_queue().wake_up_all();
                oasync_notify(pipe as *const _ as usize, true);
                return n as isize;
            }
        }

        // Pipe full.
        if pipe.is_read_closed() {
            if let Some(current) = crate::sched::current() {
                let _ = crate::signal::send_signal(
                    (*current).pid(),
                    crate::signal::Signal::SIGPIPE as i32,
                );
            }
            return -(crate::errno::constants::EPIPE) as isize;
        }
        if nonblock {
            return -11; // EAGAIN
        }

        let current = match crate::sched::current() {
            Some(t) => t,
            None => return 0,
        };
        pipe.write_queue().prepare_to_wait(current, false, true);
        let recheck_ok = pipe.buffer.lock().available_write() > 0;
        if recheck_ok || pipe.is_read_closed() {
            pipe.write_queue().finish_wait(current);
            crate::sched::dequeue_task(&*current);
            continue;
        }
        if crate::signal::signal_pending() {
            pipe.write_queue().finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return -(crate::errno::constants::EINTR) as isize;
        }
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        pipe.write_queue().finish_wait(current);
        if crate::signal::signal_pending() {
            return -(crate::errno::constants::EINTR) as isize;
        }
    }
}

/// vmsplice(2) read direction: drain up to the iov total from the pipe
/// into user memory (LTP vmsplice03). Returns bytes moved (0 = EOF) or a
/// negative errno.
pub fn pipe_vmsplice_from(file: &File, iovs: &[(usize, usize)], nonblock: bool) -> isize {
    use crate::arch::riscv64::uaccess::copy_to_user;

    let Some(pipe) = pipe_of_file(file) else {
        return -9; // EBADF
    };
    let file_nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    let nonblock = nonblock || file_nonblock;

    let total_len: usize = iovs.iter().map(|(_, l)| *l).sum();
    if total_len == 0 {
        return 0;
    }

    let cap = total_len.min(PIPE_CAPACITY);
    let mut kbuf = alloc::vec![0u8; cap];
    loop {
        {
            let mut guard = pipe.buffer.lock();
            if pipe.is_write_closed() && guard.available_read() == 0 {
                return 0; // EOF
            }
            let avail = guard.available_read();
            if avail > 0 {
                let want = total_len.min(avail).min(cap);
                let n = guard.read(&mut kbuf[..want]);
                drop(guard);
                if n > 0 {
                    pipe.write_queue().wake_up_all();
                    oasync_notify(pipe as *const _ as usize, false);
                    // Copy out segment by segment.
                    let mut off = 0usize;
                    for &(base, len) in iovs {
                        if off >= n {
                            break;
                        }
                        let take = len.min(n - off);
                        // SAFETY: base is a user pointer; exception-table copy.
                        if unsafe {
                            copy_to_user(base as *mut u8, kbuf.as_ptr().add(off), take)
                        } != 0
                        {
                            return if off > 0 { off as isize } else { -14 };
                        }
                        off += take;
                    }
                    return off as isize;
                }
                return n as isize;
            }
        }

        if nonblock {
            return -11; // EAGAIN
        }
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return 0,
        };
        pipe.read_queue().prepare_to_wait(current, false, true);
        let recheck = {
            let guard = pipe.buffer.lock();
            guard.available_read() > 0 || pipe.is_write_closed()
        };
        if recheck {
            pipe.read_queue().finish_wait(current);
            crate::sched::dequeue_task(&*current);
            continue;
        }
        if crate::signal::signal_pending() {
            pipe.read_queue().finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return -(crate::errno::constants::EINTR) as isize;
        }
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        pipe.read_queue().finish_wait(current);
        if crate::signal::signal_pending() {
            return -(crate::errno::constants::EINTR) as isize;
        }
    }
}

pub fn pipe_file_read(file: &File, buf: &mut [u8]) -> isize {
    pipe_read_common(file, buf.as_mut_ptr(), buf.len(), false)
}

/// read(2) fast path for pipes: transfer directly user↔ring.
///
/// The generic sys_read stages through a kernel buffer (alloc + 2 copies);
/// for pipes that was ~half the per-op cost under TCG. Same semantics as
/// pipe_file_read — the caller must have access_ok-validated `dst`.
pub fn pipe_read_user(file: &File, dst: *mut u8, count: usize) -> isize {
    pipe_read_common(file, dst, count, true)
}

/// Shared pipe read core: `user` selects exception-table user copies
/// (direct transfer) vs kernel-slice copies (FileOps path).
///
/// # Safety
/// When `user` is true, `dst` must be a validated user pointer with `count`
/// writable bytes.
fn pipe_read_common(file: &File, dst: *mut u8, count: usize, user: bool) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // Check if non-blocking mode
        let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;

        loop {
            // Acquire lock once, hold across check + IO
            let mut guard = pipe.buffer.lock();

            // Check EOF condition: write end closed and buffer empty
            if pipe.is_write_closed() && guard.available_read() == 0 {
                return 0; // EOF
            }

            // Try to read data
            // SAFETY: user=true only from pipe_read_user with an
            // access_ok-validated pointer; user=false passes a kernel slice
            // pointer of `count` bytes.
            let avail = guard.available_read();
            let count = if avail > 0 {
                if user {
                    unsafe { guard.read_user(dst, count) }
                } else {
                    // SAFETY: dst is a kernel pointer to count writable bytes
                    // (the FileOps slice).
                    unsafe { guard.read(core::slice::from_raw_parts_mut(dst, count)) }
                }
            } else {
                0
            };
            if count > 0 {
                // Read successful, wake up write waiters (space available)
                drop(guard); // Release lock before waking waiters
                pipe.write_queue().wake_up_all();
                // O_ASYNC: space freed — signal write-end owners (fcntl31).
                oasync_notify(pipe as *const _ as usize, false);
                return count as isize;
            }
            // avail > 0 but 0 bytes copied: a USER fault on the destination
            // (kernel copies cannot fault). Never sleep-retry a faulting
            // buffer — that would livelock until the writer closes.
            if avail > 0 && user {
                return -14; // EFAULT
            }

            // Buffer empty — release lock before blocking
            drop(guard);

            if nonblock {
                // Non-blocking mode: return EAGAIN
                return -11_i32 as isize; // EAGAIN
            }

            // Blocking mode: use wait queue to wait for data
            // Condition: buffer has data or write end closed
            {
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return 0, // Cannot get current task, return EOF
                };

                // Use prepare_to_wait to atomically set INTERRUPTIBLE and
                // add to queue under the same lock.  This prevents the
                // lost-wakeup race where wake_up_all() fires between add()
                // and set_state().
                pipe.read_queue().prepare_to_wait(current, false, true);

                // Re-check the condition AFTER registering: if a writer
                // filled the buffer between our check and prepare_to_wait,
                // its wake_up_all() found an empty queue — don't sleep.
                if pipe.buffer.lock().available_read() > 0 || pipe.is_write_closed() {
                    pipe.read_queue().finish_wait(current);
                    // R36-B2 (R8-5 NEW-C2 discipline, missed here): a wake
                    // that landed between prepare_to_wait and this recheck
                    // enqueued us while we never slept — take ourselves
                    // back off the GRQ (on_rq guards make it a no-op
                    // otherwise) or nr_running stays inflated and the
                    // idle fast path is defeated until our next switch.
                    crate::sched::dequeue_task(&*current);
                    continue;
                }

                // R20-FS7: a signal that arrived while we were still RUNNING
                // (before prepare_to_wait) generated no wakeup; without this
                // recheck we would sleep on a pending SIGKILL and become
                // unkillable until a writer arrived.
                if crate::signal::signal_pending() {
                    pipe.read_queue().finish_wait(current);
                    // R36-B2: undo a concurrent data-arrival wake enqueue
                    // before returning (R9-17 discipline).
                    crate::sched::dequeue_task(&*current);
                    return -(crate::errno::constants::EINTR) as isize;
                }

                // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();

                pipe.read_queue().finish_wait(current);

                // Blocking pipe reads are interruptible by signals
                // (including SIGKILL) — without this check the process
                // could not be killed while blocked on an empty pipe.
                if crate::signal::signal_pending() {
                    return -(crate::errno::constants::EINTR) as isize;
                }

                // Recheck condition
                continue;
            }
        }
    } else {
        -9  // EBADF
    }
}

pub fn pipe_file_write(file: &File, buf: &[u8]) -> isize {
    pipe_write_common(file, buf.as_ptr(), buf.len(), false)
}

/// write(2) fast path for pipes: transfer directly user↔ring (see
/// pipe_read_user). The caller must have access_ok-validated `src`.
pub fn pipe_write_user(file: &File, src: *const u8, count: usize) -> isize {
    pipe_write_common(file, src, count, true)
}

/// Shared pipe write core (kernel slice vs direct user transfer).
///
/// # Safety
/// When `user` is true, `src` must be a validated user pointer with
/// `count` readable bytes.
fn pipe_write_common(file: &File, src: *const u8, count: usize, user: bool) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // Check if read end is closed
        if pipe.is_read_closed() {
            // Write to pipe with no readers -> SIGPIPE + EPIPE
            if let Some(current) = crate::sched::current() {
                let _ = crate::signal::send_signal((*current).pid(), crate::signal::Signal::SIGPIPE as i32);
            }
            return -(crate::errno::constants::EPIPE) as isize;
        }

        // Check if non-blocking mode
        let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;

        let mut total_written = 0;

        // Loop write until all data written or error encountered
        while total_written < count {
            let remaining = count - total_written;

            // POSIX PIPE_BUF atomicity: a write of <= PIPE_BUF bytes must be
            // atomic — never split across other writers. In blocking mode we
            // wait until the ENTIRE remaining chunk fits before copying any
            // of it (review 5.2: the old code partial-wrote whenever there
            // was any space, interleaving chunks from concurrent writers).
            let atomic = remaining <= PIPE_BUF;

            // Acquire lock once for check + IO
            let mut guard = pipe.buffer.lock();

            let space = guard.available_write();
            let can_write = if atomic {
                space >= remaining
            } else {
                space > 0
            };

            if can_write {
                // Write successful
                // SAFETY: user=true only from pipe_write_user with an
                // access_ok-validated pointer; user=false passes a kernel
                // slice pointer of `count` bytes.
                let count = if user {
                    unsafe { guard.write_user(src.add(total_written), remaining) }
                } else {
                    // SAFETY: src is a kernel pointer to count readable bytes
                    // (the FileOps slice).
                    unsafe {
                        guard.write(core::slice::from_raw_parts(src.add(total_written), remaining))
                    }
                };
                // can_write held but 0 bytes copied: a USER fault on the
                // source. Retrying would livelock (space never drains) —
                // return a short write or EFAULT.
                if count == 0 && user {
                    drop(guard);
                    return if total_written > 0 {
                        total_written as isize
                    } else {
                        -14 // EFAULT
                    };
                }
                total_written += count;
                // Release lock before waking waiters
                drop(guard);
                // Wake up read waiters (data available)
                pipe.read_queue().wake_up_all();
                // O_ASYNC: data arrived — signal read-end owners (fcntl31).
                oasync_notify(pipe as *const _ as usize, true);
                continue;
            }

            // Buffer full (or not enough room for the atomic chunk) —
            // release lock before blocking
            drop(guard);

            if nonblock {
                // Non-blocking mode: return bytes written or EAGAIN
                if total_written > 0 {
                    return total_written as isize;
                } else {
                    return -11_i32 as isize; // EAGAIN
                }
            }

            // Blocking mode: use wait queue to wait for space
            {
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return total_written as isize, // Cannot get current task, return bytes written
                };

                // Use prepare_to_wait to atomically set INTERRUPTIBLE and
                // add to queue, preventing lost-wakeup race.
                pipe.write_queue().prepare_to_wait(current, false, true);

                // Re-check the condition AFTER registering: if a reader
                // drained the buffer between our check and prepare_to_wait,
                // its wake_up_all() found an empty queue — don't sleep.
                // The predicate must MATCH the write condition above: an
                // atomic (<= PIPE_BUF) chunk needs space for the WHOLE
                // chunk, otherwise the loop below spins without sleeping.
                let recheck_ok = {
                    let guard = pipe.buffer.lock();
                    let space = guard.available_write();
                    if remaining <= PIPE_BUF {
                        space >= remaining
                    } else {
                        space > 0
                    }
                };
                if recheck_ok || pipe.is_read_closed() {
                    pipe.write_queue().finish_wait(current);
                    // R36-B2 (R8-5 NEW-C2 discipline, missed here): undo a
                    // concurrent drain/close wake that enqueued us while we
                    // never slept.
                    crate::sched::dequeue_task(&*current);
                    continue;
                }

                // R20-FS7: same pre-schedule signal recheck as the read path.
                if crate::signal::signal_pending() {
                    pipe.write_queue().finish_wait(current);
                    // R36-B2: undo a concurrent space-available wake enqueue
                    // before returning (R9-17 discipline).
                    crate::sched::dequeue_task(&*current);
                    if total_written > 0 {
                        return total_written as isize;
                    }
                    return -(crate::errno::constants::EINTR) as isize;
                }

                // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();

                pipe.write_queue().finish_wait(current);

                // Blocking pipe writes are interruptible by signals; a
                // partial write returns what was already written.
                if crate::signal::signal_pending() {
                    if total_written > 0 {
                        return total_written as isize;
                    }
                    return -(crate::errno::constants::EINTR) as isize;
                }

                // Check if read end closed while we were sleeping
                if pipe.is_read_closed() {
                    if total_written > 0 {
                        return total_written as isize;
                    }
                    if let Some(current) = crate::sched::current() {
                        let _ = crate::signal::send_signal((*current).pid(), crate::signal::Signal::SIGPIPE as i32);
                    }
                    return -(crate::errno::constants::EPIPE) as isize;
                }

                // Retry write
                continue;
            }
        }

        total_written as isize
    } else {
        -9  // EBADF
    }
}

// ============================================================================
// O_ASYNC (FASYNC) SIGIO delivery for pipes — LTP fcntl31
// ============================================================================
//
// Linux keeps the fasync owner on the open file description and delivers
// the configured signal (F_SETSIG, default SIGIO) when the description
// becomes readable (read end) or writable (write end). The owner is
// stored on File.f_owner/f_signum by the fcntl layer; this registry maps
// pipe identity → registered descriptions so the data path can find the
// owners to signal without a back-pointer from Pipe to its end Files.

struct OAsyncEntry {
    file_id: u64,
    pipe_addr: usize,
    /// true: read end (signal when data arrives); false: write end
    /// (signal when space frees up).
    read_end: bool,
    owner_kind: u8,
    owner_id: i32,
    signum: i32,
}

static PIPE_OASYNC: crate::sync::spinlock::Spinlock<alloc::vec::Vec<OAsyncEntry>> =
    crate::sync::spinlock::Spinlock::new(alloc::vec::Vec::new());

/// Refresh (or remove) this description's O_ASYNC registration after an
/// fcntl touched f_owner/f_signum/O_ASYNC. No-op for non-pipe files.
pub fn oasync_register_pipe(file: &File) {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return,
    };
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return;
    }
    // SAFETY: ops identity confirmed a pipe File; private_data holds the
    // shared Arc<Pipe> raw pointer installed by create_pipe.
    let pipe_addr = match unsafe { *file.private_data.get() } {
        Some(p) => p as usize,
        None => return,
    };
    let owner = *file.f_owner.lock();
    let signum = file.f_signum.load(core::sync::atomic::Ordering::Relaxed);
    let has_async = file.flags().bits() & FileFlags::O_ASYNC != 0;
    let mut reg = PIPE_OASYNC.lock();
    reg.retain(|e| e.file_id != file.file_id);
    if owner.kind != 0 && has_async {
        reg.push(OAsyncEntry {
            file_id: file.file_id,
            pipe_addr,
            read_end: file.flags().is_readonly(),
            owner_kind: owner.kind,
            owner_id: owner.id,
            signum: if signum == 0 {
                crate::signal::Signal::SIGIO as i32
            } else {
                signum
            },
        });
    }
}

/// Drop a dying description's registration (File::drop).
pub fn oasync_unregister_file(file_id: u64) {
    let mut reg = PIPE_OASYNC.lock();
    reg.retain(|e| e.file_id != file_id);
}

/// Send the configured signal to an owner: process (1), process group
/// (2, id negative in F_SETOWN terms — stored positive here) or thread
/// (3). Best-effort, like Linux's kill_fasync.
fn oasync_signal_owner(kind: u8, id: i32, sig: i32) {
    match kind {
        1 | 3 => {
            if id > 0 {
                let _ = crate::signal::send_signal(id as u32, sig);
            }
        }
        2 => {
            let pgid = id.unsigned_abs();
            crate::process::pid_hash::pid_hash_for_each_task(|task| unsafe {
                if (*task).pgid() == pgid {
                    let _ = crate::signal::send_signal((*task).pid(), sig);
                }
            });
        }
        _ => {}
    }
}

/// Notify O_ASYNC owners on `pipe_addr`: readable events signal read-end
/// owners, writable events signal write-end owners.
fn oasync_notify(pipe_addr: usize, readable: bool) {
    let reg = PIPE_OASYNC.lock();
    for e in reg.iter() {
        if e.pipe_addr == pipe_addr && e.read_end == readable {
            oasync_signal_owner(e.owner_kind, e.owner_id, e.signum);
        }
    }
}

pub fn pipe_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        if events & POLLIN != 0 {
            if pipe.buffer.lock().available_read() > 0 {
                ready |= POLLIN | POLLRDNORM;
            }
        }
        if events & POLLOUT != 0 {
            if pipe.buffer.lock().available_write() > 0 {
                ready |= POLLOUT | POLLWRNORM;
            }
        }

        // POLLHUP/POLLERR are reported regardless of the requested events
        // (Linux sets them unconditionally in pipe_poll) — review 5.2 low:
        // they used to be masked behind POLLIN/POLLOUT requests, hiding
        // EOF/reader-gone from callers that only polled the other direction.
        if pipe.is_write_closed() {
            ready |= POLLHUP;
        }
        if pipe.is_read_closed() {
            ready |= POLLERR;
        }
    }

    ready
}

fn pipe_file_close(file: &File) -> i32 {
    if let Some(pipe_ptr) = unsafe { file.private_data.get().replace(None) } {
        // Reconstruct the Arc from the raw pointer. This consumes one Arc refcount.
        // When the last close drops the last Arc, the Pipe is freed.
        let pipe = unsafe { Arc::from_raw(pipe_ptr as *const Pipe) };

        // Check file flags to determine whether to close read or write end
        if file.flags().is_readonly() || file.flags().is_rdwr() {
            // Close read end
            pipe.close_read();
        }

        if file.flags().is_writeonly() || file.flags().is_rdwr() {
            // Close write end
            pipe.close_write();
        }

        // `pipe` (the Arc) is dropped here, decrementing refcount.
        // The Pipe itself is freed when the last Arc goes out of scope.
        drop(pipe);

        0  // Success
    } else {
        -9  // EBADF
    }
}

/// Pipe file operations (module-level so other subsystems can identity-check
/// a File against it — FIONREAD etc.).
pub static PIPE_OPS: FileOps = FileOps {
    read: Some(pipe_file_read),
    write: Some(pipe_file_write),
    lseek: None,  // Pipe doesn't support lseek
    close: Some(pipe_file_close),
    poll: Some(pipe_file_poll),
};

/// Real readable byte count for FIONREAD on a pipe File.
/// Returns None when `file` is not a pipe (identity-checked against
/// PIPE_OPS so a foreign private_data pointer is never type-confused).
pub fn pipe_fionread(file: &File) -> Option<usize> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return None;
    }
    // SAFETY: ops identity confirmed this is a pipe File; private_data was
    // installed by create_pipe as Arc::into_raw(Pipe) and remains valid
    // while the File exists.
    let ptr = unsafe { *file.private_data.get() }?;
    let pipe = unsafe { &*(ptr as *const Pipe) };
    Some(pipe.available_read())
}

impl Pipe {
    /// Number of bytes currently buffered (for FIONREAD).
    pub fn available_read(&self) -> usize {
        self.buffer.lock().available_read()
    }

    /// Current usable capacity in bytes (F_GETPIPE_SZ).
    pub fn capacity(&self) -> usize {
        self.buffer.lock().capacity()
    }

    /// Current capacity and buffered bytes (for F_SETPIPE_SZ checks).
    fn cap_and_buffered(&self) -> (usize, usize) {
        let b = self.buffer.lock();
        (b.capacity(), b.available_read())
    }

    /// Resize (F_SETPIPE_SZ) with Linux's error contract
    /// (fs/pipe.c pipe_set_size):
    ///   - arg > 1<<31  → EINVAL
    ///   - arg == 0     → EINVAL (round_pipe_size(0) == 0 → no slots)
    ///   - growing past /proc/sys/fs/pipe-max-size without
    ///     CAP_SYS_RESOURCE → EPERM
    ///   - shrinking below the bytes already buffered → EBUSY
    /// Returns the granted capacity (page-rounded).
    pub fn resize_checked(&self, requested: usize) -> Result<usize, i32> {
        const PAGE: usize = 4096;
        const MAX_SIZE: usize = 1 << 31;
        if requested == 0 || requested > MAX_SIZE {
            return Err(-(crate::errno::constants::EINVAL as i32));
        }
        let new_cap = requested.div_ceil(PAGE) * PAGE;
        let max_pipe_size =
            crate::fs::procfs::sysctl::PIPE_MAX_SIZE.load(core::sync::atomic::Ordering::Relaxed)
                as usize;
        let (cur_cap, buffered) = self.cap_and_buffered();
        if new_cap > cur_cap
            && new_cap > max_pipe_size
            && !crate::security::capable(crate::security::capability::CAP_SYS_RESOURCE)
        {
            return Err(-(crate::errno::constants::EPERM as i32));
        }
        if buffered > new_cap {
            return Err(-(crate::errno::constants::EBUSY as i32));
        }
        self.buffer.lock().resize(new_cap);
        Ok(new_cap)
    }
}

/// F_GETPIPE_SZ entry for the fcntl layer: identity-checks the File
/// against PIPE_OPS and reports the live ring capacity.
/// Returns None when `file` is not a pipe.
pub fn pipe_get_sz(file: &File) -> Option<usize> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return None;
    }
    // SAFETY: ops identity confirmed this is a pipe File; private_data was
    // installed by create_pipe as Arc::into_raw(Pipe) and remains valid
    // while the File exists.
    let ptr = unsafe { *file.private_data.get() }?;
    let pipe = unsafe { &*(ptr as *const Pipe) };
    Some(pipe.capacity())
}

/// F_SETPIPE_SZ entry for the fcntl layer: identity-checks the File
/// against PIPE_OPS, then resizes the shared ring (both ends of the pipe
/// see the change — Linux stores the size on the pipe inode).
/// Returns Err(EINVAL) when `file` is not a pipe.
pub fn pipe_set_sz(file: &File, size: usize) -> Result<usize, i32> {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return Err(-(crate::errno::constants::EINVAL as i32)),
    };
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return Err(-(crate::errno::constants::EINVAL as i32));
    }
    // SAFETY: ops identity confirmed this is a pipe File; private_data was
    // installed by create_pipe as Arc::into_raw(Pipe) and remains valid
    // while the File exists.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return Err(-(crate::errno::constants::EINVAL as i32)),
    };
    let pipe = unsafe { &*(ptr as *const Pipe) };
    pipe.resize_checked(size)
}

/// Synthetic st_dev for anonymous pipe file descriptions (Linux keeps
/// pipes on an in-kernel "pipefs"; no other Rux filesystem uses this id).
const PIPE_STAT_DEV: u64 = 0x7069_7066_7300_0001;

/// fstat(2) for pipe file descriptions (no backing VFS inode).
///
/// Pipe Files are anonymous — `file.inode` is None — so fstat used to
/// fail with EBADF (worse: the syscall layer re-negated that into +9,
/// which glibc treats as success and leaves the caller's stat buffer
/// untouched). Fill a Linux-pipefs-style identity instead: both ends of
/// one pipe share the Arc<Pipe> address, so same-file comparisons see
/// the two ends as ONE inode (like Linux) and a pipe never collides
/// with the console or a real file.
///
/// Returns `Some(0)` when `file` is a pipe File, `None` otherwise.
pub fn pipe_file_stat(file: &File, stat: &mut crate::fs::Stat) -> Option<i32> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &PIPE_OPS as *const _) {
        return None;
    }
    // SAFETY: ops identity confirms this is a pipe File; private_data was
    // installed by create_pipe as Arc::into_raw(Pipe) and remains valid
    // while the File exists.
    let ptr = unsafe { *file.private_data.get() }?;
    stat.st_dev = PIPE_STAT_DEV;
    stat.st_ino = ptr as usize as u64;
    stat.st_nlink = 1;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_size = 0;
    stat.st_blocks = 0;
    stat.st_blksize = 4096;
    stat.set_fifo();
    stat.st_mode |= 0o600;
    Some(0)
}

pub fn create_pipe() -> (Arc<File>, Arc<File>) {
    // Create pipe wrapped in Arc so both ends share ownership.
    // The Pipe is freed when the last Arc drops.
    let pipe = Arc::new(Pipe::new());

    // Store Arc::into_raw as *mut u8 in private_data.
    // pipe_file_close reconstructs the Arc with Arc::from_raw to drop one ref.
    let read_ptr = Arc::into_raw(Arc::clone(&pipe)) as *mut u8;
    let write_ptr = Arc::into_raw(Arc::clone(&pipe)) as *mut u8;

    // Drop our local reference; ownership is now entirely in the two raw pointers.
    drop(pipe);

    // Create read end file
    let read_file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    read_file.set_ops(&PIPE_OPS);
    read_file.set_private_data(read_ptr);

    // Create write end file
    let write_file = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    write_file.set_ops(&PIPE_OPS);
    write_file.set_private_data(write_ptr);

    (read_file, write_file)
}
