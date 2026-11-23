//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The JOURNAL SEAM FLOOR (rung 5): the two journal-owned pieces the
//! framework must reach WITHOUT naming the ext4/jbd2 service crate.
//!
//! 1. `EXT4_BIG_LOCK` — the fs big-lock protocol is sync-floor per the
//!    micro-core design (§2): the exit path, the virtio VW watchdog and
//!    dfx taskdump touch the static directly; the service crate
//!    consumes it as a pub API.
//!
//! 2. `JournalCaptureState` — the deferred-publication capture window
//!    header. The buffer cache's dirty-marking hot path
//!    (bio::publication_capture_note_dirty) reads the CURRENT task's
//!    journal handle capture state on every dirty buffer; the handle
//!    type itself is crate-owned, so the crate's `Handle` carries this
//!    state as its FIRST field under `#[repr(C)]` and the framework
//!    dereferences the type-erased task slot as a pointer to this
//!    header (INV-JSEAM-1). That keeps the unarmed fast path TWO loads
//!    (task, slot) — no fn-pointer call was added to the data path
//!    (the S1 devirtualization lesson).
//!
//!    INV-JSEAM-1 (fixed-offset header): every pointer stored in
//!    `Task::journal_handle` points to a `jbd2 Handle` whose first
//!    field is `JournalCaptureState`, and the struct is `#[repr(C)]`
//!    so field order is guaranteed. The writer side (the service
//!    crate's set_current_handle) and the reader side (bio's capture
//!    note, the framework) agree on this layout by construction.

use core::cell::Cell;

/// The fs journal BIG LOCK (history: see sync/mutex.rs; relocated from
/// ext4/mod.rs at rung 5). Task-recursive: a loop device backed by a
/// file on ANOTHER ext4 instance re-enters ext4 from inside the block
/// layer. Nested acquire by the SAME task is legal; the outermost
/// guard releases. do_exit() force-drops via release_if_owner (R73)
/// since RAII never runs through do_exit.
pub static EXT4_BIG_LOCK: crate::sync::SleepMutex =
    crate::sync::SleepMutex::new_rt("EXT4-BIGLOCK", true);

/// Capture window modes (`JournalCaptureState::mode`).
pub const CAPTURE_OFF: u8 = 0;
pub const CAPTURE_PRE: u8 = 1;
pub const CAPTURE_POST: u8 = 2;

/// The deferred-publication capture window state (see
/// bio::PENDING_PUBLICATIONS). While armed in PRE mode (create-side
/// operations: mkdir/create/symlink/link/rename-add), every buffer
/// marked dirty on the capture device is recorded on `pre_list`; the
/// list becomes the `pre` set of the publication created at the
/// directory-entry write (an exact snapshot of THIS operation's own
/// metadata — the earlier device-wide dirty snapshot cost O(cache) per
/// op and O(N^2+) across a create-heavy loop such as LTP creat05/
/// fork09, which create 1021 files in setup). While armed in POST mode
/// (unlink/rmdir/rename-replace) buffers marked dirty are appended to
/// the `post` set of publication `post_id` instead. One mode at a time.
pub struct JournalCaptureState {
    /// 0=off 1=pre 2=post (CAPTURE_* above).
    pub mode: Cell<u8>,
    /// POST-mode target publication id.
    pub post_id: Cell<u64>,
    /// The capture device (major, minor).
    pub dev_key: Cell<(u32, u32)>,
    /// PRE-mode capture accumulator (task-local: only the owning
    /// task's syscalls append, via the dirty-mark hook).
    pub pre_list: crate::sync::spinlock::Spinlock<alloc::vec::Vec<u64>>,
}

impl JournalCaptureState {
    pub const fn new() -> Self {
        JournalCaptureState {
            mode: Cell::new(CAPTURE_OFF),
            post_id: Cell::new(0),
            dev_key: Cell::new((0, 0)),
            pre_list: crate::sync::spinlock::Spinlock::new(alloc::vec::Vec::new()),
        }
    }

    /// Arm the PRE-capture window (create side): buffers marked dirty
    /// on `device` from now on belong BEFORE the entry publication this
    /// operation will make. Disarmed by take_pre_capture at the entry
    /// write (publish_captured_entry).
    pub fn begin_pre_capture(&mut self, device: (u32, u32)) {
        self.pre_list.lock().clear();
        self.dev_key.set(device);
        self.mode.set(CAPTURE_PRE);
    }

    /// Disarm the PRE-capture window and return the captured block
    /// list. Never disturbs an armed POST window (rename-replace
    /// publishes its new-name entry between the target removal and the
    /// tail of the frees): the PRE accumulator is drained and only the
    /// PRE mode bit is cleared.
    pub fn take_pre_capture(&mut self) -> alloc::vec::Vec<u64> {
        let out = core::mem::take(&mut *self.pre_list.lock());
        if self.mode.get() == CAPTURE_PRE {
            self.mode.set(CAPTURE_OFF);
        }
        out
    }

    /// Arm the POST-capture window: buffers marked dirty from now on
    /// (on `device`) belong after publication `pub_id`. Caller must
    /// call end_post_capture before dropping the handle. Arming POST
    /// while a PRE window is open freezes the PRE accumulator (it is
    /// taken later by take_pre_capture) and routes new dirty-marks to
    /// the POST record — the rename-replace shape.
    pub fn begin_post_capture(&mut self, device: (u32, u32), pub_id: u64) {
        self.dev_key.set(device);
        self.post_id.set(pub_id);
        self.mode.set(CAPTURE_POST);
    }

    /// Disarm the POST-capture window (a PRE window may have been
    /// frozen by it; this never touches PRE state other than leaving
    /// mode OFF).
    pub fn end_post_capture(&mut self) {
        if self.mode.get() == CAPTURE_POST {
            self.mode.set(CAPTURE_OFF);
        }
    }

    /// The dirty-mark hot path's accumulator arm (framework side of
    /// INV-JSEAM-1).
    pub fn pre_push(&self, blocknr: u64) {
        self.pre_list.lock().push(blocknr);
    }
}

/// The current task's journal-handle slot, type-erased (null outside an
/// ext4 operation). The framework's journal-seam call sites hand this to
/// the crate's hook rows; the dirty-mark hot path's unarmed fast path
/// stays two loads.
pub fn current_handle_raw() -> *mut () {
    match crate::sched::current() {
        Some(t) => t.journal_handle.get(),
        None => core::ptr::null_mut(),
    }
}

/// Ext4FileSystem's journal slot — an ASSERTING WRAPPER (the type's
/// raw-pointer protocol state stays crate-side; the Send/Sync
/// assertion is journal-protocol-level and lives HERE, framework-side,
/// like the lock above).
///
/// INV-JSEAM-4: the wrapped `Arc<Journal>` is shared across threads
/// exactly as before (task contexts + the commit kthread); every raw
/// pointer inside Journal is dereferenced only under the journal's own
/// spinlocks (the jbd2 protocol), so sharing the Arc is sound. The
/// wrapper exists because the crate cannot carry the `unsafe impl`.
pub struct AssertingSyncArc<T>(pub Option<alloc::sync::Arc<T>>);
// SAFETY: INV-JSEAM-4 above — the assertion is the JOURNAL use's to
// make (the only user is Ext4FileSystem::journal; the type stays
// narrow-purposed and documented).
unsafe impl<T> Send for AssertingSyncArc<T> {}
// SAFETY: INV-JSEAM-4 above.
unsafe impl<T> Sync for AssertingSyncArc<T> {}

impl<T> AssertingSyncArc<T> {
    pub const fn empty() -> Self {
        AssertingSyncArc(None)
    }
    /// Clone the inner Arc (None when none is loaded).
    pub fn get(&self) -> Option<alloc::sync::Arc<T>> {
        self.0.clone()
    }
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }
    pub fn set(&mut self, j: alloc::sync::Arc<T>) {
        self.0 = Some(j);
    }
    pub fn clear(&mut self) {
        self.0 = None;
    }
}
