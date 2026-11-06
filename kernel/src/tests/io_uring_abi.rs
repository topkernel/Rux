//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! io_uring ABI edges: ring mmap length validation and the non-blocking
//! kernel-side eventfd signal used for CQE notification.

use crate::io_uring::ring_map_len;
use crate::syscall::misc::{eventfd_signal, sys_eventfd2};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_io_uring_abi() {
    test_group_start("io_uring abi");

    // liburing-shaped lengths: exact byte sizes, not page multiples.
    // sq ring for 8 entries (header 24 + 4*8), cq ring cqes for 8
    // entries, sqes 64*8, and the full region.
    let region = 4096;
    let ok = ring_map_len(24 + 4 * 8, region) == Ok(4096)
        && ring_map_len(32 + 16 * 8, region) == Ok(4096)
        && ring_map_len(64 * 8, region) == Ok(4096)
        && ring_map_len(4096, region) == Ok(4096)
        && ring_map_len(4096 + 1, 2 * 4096) == Ok(2 * 4096);
    if ok {
        test_pass("ring mmap accepts lengths up to the region");
    } else {
        test_fail("ring mmap length", "liburing-shaped length rejected");
    }
    let rejects = ring_map_len(0, region) == Err(-22)
        && ring_map_len(4096 + 1, region) == Err(-22)
        && ring_map_len(usize::MAX, region) == Err(-22);
    if rejects {
        test_pass("ring mmap rejects 0 / past-region / overflowing lengths");
    } else {
        test_fail("ring mmap length", "bad length accepted");
    }

    // eventfd_signal: +1 per call, never blocks, readable through read(2).
    let fd = sys_eventfd2([0, 0, 0, 0, 0, 0]);
    if fd < 0 {
        test_skip("eventfd_signal", "eventfd2 failed");
        return;
    }
    // SAFETY: fd was just installed in this task's table.
    let file = match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => {
            test_skip("eventfd_signal", "fd lookup failed");
            return;
        }
    };
    let signalled = eventfd_signal(&file) && eventfd_signal(&file);
    let mut buf = [0u8; 8];
    let n = match file.get_ops().and_then(|o| o.read) {
        Some(read) => read(&file, &mut buf),
        None => -1,
    };
    if signalled && n == 8 && u64::from_le_bytes(buf) == 2 {
        test_pass("eventfd_signal adds to the counter");
    } else {
        test_fail(
            "eventfd_signal",
            &alloc::format!("signalled={} read={} value={}", signalled, n, u64::from_le_bytes(buf)),
        );
    }
    drop(file);
    let _ = crate::syscall::file::sys_close([fd as u64, 0, 0, 0, 0, 0]);
}
