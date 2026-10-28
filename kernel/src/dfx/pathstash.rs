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
    op: AtomicU32,
    len: AtomicU32,
    buf: [u8; CAP],
}

static mut SLOTS: [Slot; N] = [const {
    Slot {
        tid: AtomicU32::new(0),
        op: AtomicU32::new(0),
        len: AtomicU32::new(0),
        buf: [0; CAP],
    }
}; N];

/// Stash op tags (leg-29A): the syscall family that recorded the path.
pub const OP_OPEN: u32 = 1;
pub const OP_WRITE: u32 = 2;
pub const OP_SYNC: u32 = 3;
/// leg-36B: truncate family. sc333B's foundation-main wedge sat in
/// ftruncate(fd, len) 80+ s while holding a server txn; the serial only
/// showed the raw fd. Both truncate entries stash their resolved path
/// so TXN-HANG can name the file.
pub const OP_TRUNC: u32 = 4;

/// Record the current task's last resolved open path. Called from
/// sys_openat after resolution; arm-gated.
pub fn stash_open(path: &str) {
    stash_file(OP_OPEN, path);
}

/// Record the current task's last file path with the syscall-family
/// tag (open / regular-file write / fsync-fdatasync). Each family
/// re-stashes at ITS entry, so a thread parked inside one of them
/// always shows its own file. Arm-gated on TxnHang (leg-29A: sc310's
/// hiview handler sat in fdatasync 43 min with only a raw fd known).
pub fn stash_file(op: u32, path: &str) {
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
    s.op.store(op, Ordering::Release);
    s.len.store(n as u32, Ordering::Release);
    s.tid.store(tid, Ordering::Release);
}

/// Lazy-path variant for call sites that would have to CONSTRUCT the
/// path per call: `stash_file(OP_WRITE, &file.path())` evaluates
/// `File::path()` (dentry walk + String formatting) on every write(2)
/// BEFORE stash_file's switch check discards it — a probe cost paid on
/// the serial-clean family with the switch off (p5c decomposition,
/// leg 44). The switch check runs BEFORE `f`; with TxnHang off this is
/// one atomic load and no allocation.
pub fn stash_file_lazy(op: u32, f: impl FnOnce() -> alloc::string::String) {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TxnHang) {
        return;
    }
    let path = f();
    stash_file(op, &path);
}

/// Look up the last (op, path) stashed by `tid`. Returns None when the
/// slot is empty, owned by another tid, or the path was never written.
pub fn lookup_file(tid: u32) -> Option<(u32, &'static str)> {
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
    let op = s.op.load(Ordering::Acquire);
    let p = core::str::from_utf8(&s.buf[..n]).ok()?;
    Some((op, p))
}

/// Backward-compatible open-only lookup (leg-28 print).
pub fn lookup_open(tid: u32) -> Option<&'static str> {
    lookup_file(tid).map(|(_, p)| p)
}
