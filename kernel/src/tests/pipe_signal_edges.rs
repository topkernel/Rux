//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Pipe-buffer and signal-queue edges (tests/pipe_signal_edges.rs).
//!
//! Two mid-size dark spots the existing tests only scratch:
//! fs/pipe.rs's PipeBuffer mechanics (resize, peek, wrap-around
//! fill/drain across the ring boundary, user-pointer arms that the
//! boot-task context forces onto their error paths) and signal.rs's
//! pending queue (real-time queueing, drain order, peek without
//! dequeue). Pure kernel-state drives — no fds, no scheduler.

use crate::fs::pipe::PipeBuffer;
use crate::signal::{SigInfo, SigPending};

pub fn test_pipe_signal_edges() {
    crate::tests::test_group_start("pipe+signal: mechanics");

    // ---- pipe buffer ring mechanics ---------------------------------------
    let mut p = PipeBuffer::new(8);
    crate::test_assert!(p.capacity() == 8, "pipe: capacity honored");
    crate::test_assert!(p.available_read() == 0, "pipe: starts empty");
    crate::test_assert!(p.available_write() == 8, "pipe: full write budget");

    // partial write then drain
    let n = p.write(b"abc");
    crate::test_assert!(n == 3, "pipe: write 3");
    crate::test_assert!(p.available_read() == 3, "pipe: read side sees 3");
    let mut out = [0u8; 8];
    let got = p.read(&mut out);
    crate::test_assert!(got == 3 && &out[..3] == b"abc", "pipe: read echoes");

    // wrap-around: fill to the rim, drain 2, write 2 — the ring indices
    // now straddle the buffer end
    let _ = p.write(b"01234567");
    let mut two = [0u8; 2];
    let _ = p.read(&mut two);
    let n = p.write(b"XY");
    crate::test_assert!(n == 2, "pipe: wrap-around write");
    crate::test_assert!(p.available_read() == 8, "pipe: wrap-around fill level");
    let mut all = [0u8; 8];
    let got = p.read(&mut all);
    crate::test_assert!(got == 8 && &all[..8] == b"234567XY", "pipe: wrap-around order kept");

    // overflow write is partial (the writer never blocks here)
    let n = p.write(b"0123456789abcdef");
    crate::test_assert!(n == 8, "pipe: full-buffer write truncates");

    // peek does not consume
    let _ = p.read(&mut all);
    let _ = p.write(b"peek!");
    let mut pk = [0u8; 8];
    let n = p.peek(&mut pk);
    crate::test_assert!(n == 5 && &pk[..5] == b"peek!", "pipe: peek returns head");
    crate::test_assert!(p.available_read() == 5, "pipe: peek did not consume");

    // resize grows and clamps
    crate::test_assert!(p.resize(64) == 64, "pipe: resize grows");
    crate::test_assert!(p.capacity() == 64, "pipe: capacity follows resize");
    crate::test_assert!(p.resize(2) >= 2, "pipe: shrink clamps to live data");
    crate::test_assert!(p.available_read() == 5, "pipe: shrink keeps live bytes");

    // user-pointer arms in a no-user-mm context answer via the error path
    let n = p.read_user(usize::MAX - 15, 16);
    crate::test_assert!(n == 0, "pipe: read_user bad pointer -> 0");
    let n = p.write_user(usize::MAX - 15, 16);
    crate::test_assert!(n == 0, "pipe: write_user bad pointer -> 0");

    // ---- signal pending queue ----------------------------------------------
    let q = SigPending::new();
    crate::test_assert!(q.first().is_none(), "signal: set starts empty");
    q.add(9); // SIGKILL-like standard signal: coalesces
    q.add(9);
    crate::test_assert!(q.has(9), "signal: bit 9 pending");
    q.remove(9);
    crate::test_assert!(!q.has(9), "signal: standard signals coalesce in the bitmap");

    // real-time signals QUEUE (34..=64 band), FIFO order per number
    let info1 = SigInfo::new(34, 0, 1, 0);
    let info2 = SigInfo::new(34, 0, 1, 0);
    q.add_info(info1);
    q.add_info(info2);
    crate::test_assert!(q.rt_queued_count() == 2, "signal: rt signals queue");
    let peeked = q.peek_info(34);
    crate::test_assert!(peeked.is_some(), "signal: peek finds queued rt signal");
    let a = q.queue.dequeue();
    let b = q.queue.dequeue();
    crate::test_assert!(a.is_some() && b.is_some(), "signal: queue drains");
    crate::test_assert!(q.rt_queued_count() == 0, "signal: queue empty after drain");
    crate::test_assert!(q.first().is_none(), "signal: first() none when drained");
}
