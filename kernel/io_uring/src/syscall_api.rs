//! The io_uring SYSCALL ENTRY POINTS (moved from
//! kernel/core/src/io_uring/mod.rs at the cold-four policy flip — sole copy
//! here; zero behavior change, except the unsafe conversions noted in
//! the framework module's move note).

use super::*;
use alloc::boxed::Box;
use alloc::sync::Arc;

/// sys_io_uring_setup — create a new io_uring instance (NR 425).
pub fn sys_io_uring_setup(args: [u64; 6]) -> u64 {
    use crate::arch::uaccess::access_ok;

    let entries = args[0] as u32;
    let params_ptr = args[1] as *mut IoUringParams;

    if params_ptr.is_null() {
        return -(22i64) as u64; // EINVAL
    }
    if !access_ok(params_ptr as usize, core::mem::size_of::<IoUringParams>()) {
        return -(14i64) as u64; // EFAULT
    }

    let mut params = match crate::arch::uaccess_safe::read_user::<IoUringParams>(params_ptr as usize) {
        Some(p) => p,
        None => return -(14i64) as u64,
    };

    let ring = match io_uring_create(entries, &mut params) {
        Ok(r) => r,
        // `e` is already a NEGATIVE errno (io_uring_create returns Err(-12)
        // etc.) — do NOT negate again (review批次6 P2: the double negation
        // turned ENOMEM into a fake "returned fd 12").
        Err(e) => return (e as i64) as u64,
    };

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(12i64) as u64, // ENOMEM
    };

    let ring_ptr = Box::into_raw(ring) as *mut u8;
    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&IO_URING_OPS);
    file.set_private_data(ring_ptr);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            ring_release_on_install_error(ring_ptr);
            return -(24i64) as u64; // EMFILE
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {}
        Err(_) => {
            ring_release_on_install_error(ring_ptr);
            return -(12i64) as u64;
        }
    }

    // Copy updated params back
    let _ = crate::arch::uaccess_safe::copy_out_t(params_ptr as usize, &params);

    fd as u64
}

/// sys_io_uring_enter — submit SQEs and/or wait for CQEs (NR 426).
pub fn sys_io_uring_enter(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let to_submit = args[1] as u32;
    let min_complete = args[2] as u32;
    let flags = args[3] as u32;

    // Only IORING_ENTER_GETEVENTS is supported
    if flags & !IORING_ENTER_GETEVENTS != 0 {
        return -(22i64) as u64;
    }

    let file = match crate::fs::hotpath::get_file_fd(fd as usize) {
        Some(f) => f,
        None => return -(9i64) as u64, // EBADF
    };

    // Verify the fd really is an io_uring instance before treating its
    // private_data as a ring pointer — any other file type here would be a
    // type confusion (arbitrary kernel read/write).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &IO_URING_OPS as *const _)) {
        return -(9i64) as u64; // EBADF
    }

    // R7-C4 (IOU-H1 completion): the private_data pointer must be read
    // INSIDE the lifecycle lock — pin_verified_ring is the framework's
    // audited twin of that discipline (pointer read + try_ref under
    // LIFECYCLE_LOCK).
    let pinned = match pin_verified_ring(&file) {
        Some(p) => p,
        None => return -(9i64) as u64, // EBADF: ring being torn down
    };
    let ring = pinned.ring();

    // Submit SQEs
    let submitted = submit::submit_sqes(ring, to_submit);

    // Wait for completions if requested
    if flags & IORING_ENTER_GETEVENTS != 0 && min_complete > 0 {
        let result = submit::wait_for_cqes(ring, min_complete);
        if result < 0 {
            // result is already a negative errno — no double negation
            // (review批次6 P2).
            return (result as i64) as u64;
        }
    }

    // Linux io_uring_enter returns the number of CQEs ready (post-wait)
    // when nothing was submitted — previously a pure-enter wait call
    // always returned 0 and callers could not distinguish "woke with N
    // completions" from "timeout/nothing" (review批次6 M).
    if submitted == 0 {
        ring.cq_completed() as u64
    } else {
        submitted as u64
    }
}

/// sys_io_uring_register — register buffers/files/eventfd (NR 427).
pub fn sys_io_uring_register(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let opcode = args[1] as u32;
    let arg = args[2] as u64;
    let nr_args = args[3] as u32;

    let file = match crate::fs::hotpath::get_file_fd(fd as usize) {
        Some(f) => f,
        None => return -(9i64) as u64, // EBADF
    };

    // Verify the fd really is an io_uring instance (type-confusion guard,
    // same as sys_io_uring_enter).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &IO_URING_OPS as *const _)) {
        return -(9i64) as u64; // EBADF
    }

    // R7-C4 (IOU-H1 completion): pin under the lifecycle lock.
    let pinned = match pin_verified_ring(&file) {
        Some(p) => p,
        None => return -(9i64) as u64, // EBADF: ring being torn down
    };
    let ring = pinned.ring();

    match opcode {
        IORING_REGISTER_EVENTFD => {
            if nr_args != 1 { return -(22i64) as u64; }
            let eventfd_fd = arg as i32;
            if eventfd_fd < 0 { return -(9i64) as u64; }
            // Hold the actual file reference (immune to fd reuse) and verify
            // it really is an eventfd — anything else would make completion
            // notification write garbage into an unrelated file.
            let efile = match crate::fs::hotpath::get_file_fd(eventfd_fd as usize) {
                Some(f) => f,
                None => return -(9i64) as u64,
            };
            if !efile.get_ops().is_some_and(crate::syscall::support::is_eventfd_ops) {
                return -(22i64) as u64; // EINVAL
            }
            ring.set_eventfd(eventfd_fd, Some(efile));
            0
        }
        IORING_UNREGISTER_EVENTFD => {
            ring.set_eventfd(-1, None);
            0
        }
        _ => -(22i64) as u64, // EINVAL
    }
}
