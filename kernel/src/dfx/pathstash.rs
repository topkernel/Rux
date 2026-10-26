//! Per-task last-open path stash for TXN-HANG (dfx=txnhunt).
//!
//! Leg-28 item 2: sc303's TXN-HANG showed storage_daemon parked
//! UNINTERRUPTIBLE in open() for 5.5s→26s under the code-37 handler,
//! but the entry-frame report only prints a0/a1 — the raw user
//! pathname pointer, useless from the serial log. The audit runs in
//! timer context against ANOTHER task's address space, so it cannot
//! copy the string at print time; instead sys_openat stashes the
//! resolved full path here at entry (in the task's own context, after
//! resolve_user_path succeeded — the D-state stall happens later,
//! inside the open work itself, so the stash holds exactly the path
//! the stuck open is working on).
//!
//! Contract: zero cost when dfx=txnhunt is off (one switch check);
//! no locks — 8 slots indexed by tid % 8, tid word acts as the
//! publication flag; diagnostics only, torn reads can mix bytes of
//! two paths of tasks colliding on a slot, which is cosmetic (bounded
//! print, utf8-checked).

use core::sync::atomic::{AtomicU32, Ordering};

const N: usize = 8;
const CAP: usize = 128;

struct Slot {
    tid: AtomicU32,
    len: AtomicU32,
    buf: [u8; CAP],
}

static mut SLOTS: [Slot; N] = [const {
    Slot {
        tid: AtomicU32::new(0),
        len: AtomicU32::new(0),
        buf: [0; CAP],
    }
}; N];

/// Record the current task's last resolved open path. Called from
/// sys_openat after resolution; arm-gated.
pub fn stash_open(path: &str) {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TxnHang) {
        return;
    }
    // current_pid() is the task (thread) id — the same id TXN-HANG
    // reports as the server tid.
    let tid = crate::process::current_pid();
    if tid == 0 {
        return;
    }
    let bytes = path.as_bytes();
    let n = bytes.len().min(CAP);
    // SAFETY: diagnostic write into a static slot; this task is the
    // only writer for its tid slot, publication ordered via tid/len.
    let s = unsafe { &mut *core::ptr::addr_of_mut!(SLOTS[(tid as usize) % N]) };
    s.tid.store(0, Ordering::Release);
    s.buf[..n].copy_from_slice(&bytes[..n]);
    s.len.store(n as u32, Ordering::Release);
    s.tid.store(tid, Ordering::Release);
}

/// Look up the last open path stashed by `tid`. Returns None when the
/// slot is empty, owned by another tid, or the path was never written.
pub fn lookup_open(tid: u32) -> Option<&'static str> {
    let i = (tid as usize) % N;
    // SAFETY: diagnostic read of the static slot table; the 'static
    // borrow is derived from the static itself and bytes are
    // utf8-checked below.
    let s: &'static Slot = unsafe { &*core::ptr::addr_of!(SLOTS).cast::<Slot>().add(i) };
    if s.tid.load(Ordering::Acquire) != tid {
        return None;
    }
    let n = s.len.load(Ordering::Acquire) as usize;
    if n == 0 || n > CAP {
        return None;
    }
    core::str::from_utf8(&s.buf[..n]).ok()
}
