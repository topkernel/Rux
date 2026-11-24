//! The io_uring SQE OPERATION HANDLERS (moved from
//! kernel/src/io_uring/mod.rs at the cold-four policy flip — sole copy
//! here; zero behavior change). Pure policy over the FileOps vtable —
//! the read/write bottoms ride the framework's uaccess_safe typed
//! twins and the existing vtable fields (crate-plan 3a rule 2: field
//! dispatch the caller already performs).

use super::*;

/// Dispatch a single SQE to the appropriate operation handler.
pub fn io_uring_dispatch_op(sqe: &IoUringSqe) -> i32 {
    match sqe.opcode {
        IORING_OP_NOP => 0,
        IORING_OP_READ => io_uring_op_read(sqe),
        IORING_OP_WRITE => io_uring_op_write(sqe),
        IORING_OP_FSYNC => io_uring_op_fsync(sqe),
        IORING_OP_CLOSE => io_uring_op_close(sqe),
        IORING_OP_FADVISE => io_uring_op_fadvise(sqe),
        _ => -22, // EINVAL
    }
}

/// IORING_OP_READ: read from fd into user buffer.
fn io_uring_op_read(sqe: &IoUringSqe) -> i32 {
    use crate::arch::uaccess::access_ok;

    let fd = sqe.fd as usize;
    let buf = sqe.addr as usize;
    // Cap the transfer length: sqe.len is user-controlled (up to 4GB) and the
    // kernel heap cannot satisfy huge allocations (alloc failure would panic).
    let len = (sqe.len as usize).min(crate::arch::uaccess::MAX_RW_COUNT);
    let off = sqe.off as i64;

    if len == 0 { return 0; }
    if !access_ok(buf, len) { return -14; } // EFAULT

    let file = match crate::fs::file::get_file_fd(fd) {
        Some(f) => f,
        None => return -9, // EBADF
    };

    // Use file position if off == -1
    let use_file_pos = off == -1;

    if use_file_pos {
        // read_fn (e.g. file_read in fs/file.rs) reads from file.pos and
        // advances it by the number of bytes read. No manual pos update
        // needed — the old code double-counted by adding result again.
        do_read(&file, buf, len)
    } else {
        // pread: read from a specific offset without changing file position.
        let saved_pos = file.get_pos();
        let _ = file.set_pos(off as u64);
        let result = do_read(&file, buf, len);
        let _ = file.set_pos(saved_pos);
        result
    }
}

fn do_read(file: &Arc<File>, buf: usize, len: usize) -> i32 {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return -9, // EBADF
    };
    let read_fn = match ops.read {
        Some(f) => f,
        None => return -22, // EINVAL
    };

    // R20-FS9 (SYSA-C1 family, io_uring leftover): stage at most RW_CHUNK at
    // a time — a single `vec![0u8; len]` with user len up to MAX_RW_COUNT
    // (2GB) far exceeds the kernel heap and panics on allocation failure.
    // Matches the chunked staging the syscall read/write paths already use.
    const RW_CHUNK: usize = crate::syscall::support::RW_CHUNK;
    let mut kbuf = alloc::vec![0u8; len.min(RW_CHUNK)];
    let mut total = 0usize;

    while total < len {
        let chunk = core::cmp::min(len - total, kbuf.len());
        let n = read_fn(file, &mut kbuf[..chunk]);
        if n <= 0 {
            return if total > 0 { total as i32 } else { n as i32 };
        }
        let uncopied =
            crate::arch::uaccess_safe::copy_out((buf + total) as usize, &kbuf[..n as usize]);
        if uncopied != 0 {
            return if total > 0 { total as i32 } else { -14 }; // EFAULT
        }
        total += n as usize;
        // R24: short chunk means EOF or "nothing more available now" (pipe
        // drained / socket drained) — sys_read's chunk loop breaks here;
        // without the break an io_uring read larger than RW_CHUNK from a
        // pipe blocks for a second fill instead of returning the partial
        // data (do_write already had the matching short-write break).
        if (n as usize) < chunk {
            break;
        }
    }

    total as i32
}

/// IORING_OP_WRITE: write from user buffer to fd.
fn io_uring_op_write(sqe: &IoUringSqe) -> i32 {
    use crate::arch::uaccess::access_ok;

    let fd = sqe.fd as usize;
    let buf = sqe.addr as usize;
    // Cap the transfer length: sqe.len is user-controlled (up to 4GB) and the
    // kernel heap cannot satisfy huge allocations (alloc failure would panic).
    let len = (sqe.len as usize).min(crate::arch::uaccess::MAX_RW_COUNT);
    let off = sqe.off as i64;

    if len == 0 { return 0; }
    if !access_ok(buf, len) { return -14; } // EFAULT

    let file = match crate::fs::file::get_file_fd(fd) {
        Some(f) => f,
        None => return -9, // EBADF
    };

    let use_file_pos = off == -1;

    if use_file_pos {
        // write_fn (e.g. file_write in fs/file.rs) writes from file.pos and
        // advances it. No manual pos update needed — the old code
        // double-counted by adding result again.
        do_write(&file, buf, len)
    } else {
        // pwrite: write at a specific offset without changing file position.
        let saved_pos = file.get_pos();
        let _ = file.set_pos(off as u64);
        let result = do_write(&file, buf, len);
        let _ = file.set_pos(saved_pos);
        result
    }
}

fn do_write(file: &Arc<File>, buf: usize, len: usize) -> i32 {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return -9,
    };
    let write_fn = match ops.write {
        Some(f) => f,
        None => return -22,
    };

    // R20-FS9: chunked staging, same rationale as do_read.
    const RW_CHUNK: usize = crate::syscall::support::RW_CHUNK;
    let mut kbuf = alloc::vec![0u8; len.min(RW_CHUNK)];
    let mut total = 0usize;

    while total < len {
        let chunk = core::cmp::min(len - total, kbuf.len());
        let uncopied = crate::arch::uaccess_safe::copy_in(&mut kbuf[..chunk], (buf + total) as usize);
        if uncopied != 0 {
            return if total > 0 { total as i32 } else { -14 }; // EFAULT
        }
        let n = write_fn(file, &kbuf[..chunk]);
        if n <= 0 {
            return if total > 0 { total as i32 } else { n as i32 };
        }
        total += n as usize;
        if (n as usize) < chunk {
            break; // short write — stop
        }
    }

    total as i32
}

/// IORING_OP_FSYNC: sync file to disk.
fn io_uring_op_fsync(_sqe: &IoUringSqe) -> i32 {
    // No real fsync in Rux (ramdisk/ramfs)
    0
}

/// IORING_OP_CLOSE: close a file descriptor.
///
/// Rejects attempts to close the io_uring ring fd itself to prevent
/// use-after-free of the ring structures.
fn io_uring_op_close(sqe: &IoUringSqe) -> i32 {
    let fd = sqe.fd as usize;

    // Guard: refuse to close a file whose ops match IO_URING_OPS, as that
    // would be the ring fd itself. Closing it would leave the ring mapped
    // in user memory but with freed kernel structures — a UAF.
    if let Some(file) = crate::fs::file::get_file_fd(fd) {
        if let Some(ops) = file.get_ops() {
            if core::ptr::eq(ops, &IO_URING_OPS as *const FileOps) {
                return -22; // EINVAL
            }
        }
    }

    match crate::fs::file::close_file_fd(fd) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// IORING_OP_FADVISE: advise on file access pattern (ignored).
fn io_uring_op_fadvise(_sqe: &IoUringSqe) -> i32 {
    0
}
