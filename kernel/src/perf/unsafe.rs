//! The perf subsystem's ONE audited unsafe file (mainline minimal-first
//! consolidation, 2026-11-27): every raw edge the event registry, the
//! mmap ring and the diagnostics still need lives here behind one
//! audited item per family; mod.rs is safe code over them.
//!
//! Edge families:
//! - the registry-owned event pointers (INV-PERF-EVENTREF-1)
//! - the event's own ring/direct-map pages (INV-PERF-RINGMEM-1)
//! - the write-once private-data cell (INV-PERF-PRIVDATA-1)
//! - the kernel stack-frame walk (INV-PERF-STACKWALK-1)
//! - pinned-task scalar reads (INV-PERF-PINNEDTASK-1)
//! - the ioctl attr/u64 user transfers (INV-PERF-UACCESS-1)

use super::PerfEvent;

// ------------------------------------------------------------------
// Registry-owned event pointers
// ------------------------------------------------------------------

/// INV-PERF-EVENTREF-1: a `*mut PerfEvent` enters circulation only via
/// `register` (which stores it in REGISTRY) and leaves only via
/// `unregister` (removed under the REGISTRY lock) + the close-path Box
/// reclaim; every deref below happens while the pointer is still a
/// registry member (or under the same lock that removed it), so the
/// pointee is live.
///
/// Shared view of a registry-live event.
pub(super) fn event_ref<'a>(p: *mut PerfEvent) -> &'a PerfEvent {
    // SAFETY: INV-PERF-EVENTREF-1 above (registry membership or
    // close-path single ownership proves validity).
    unsafe { &*(p as *const PerfEvent) }
}

/// Store the registry-assigned id into the freshly built event
/// (register path; the box is exclusively owned pre-publication).
pub(super) fn event_set_id(ev: *mut PerfEvent, id: u64) {
    // SAFETY: INV-PERF-EVENTREF-1 — pre-publication exclusive box.
    unsafe { (*ev).id = id; }
}

/// Read the `enabled` flag of an event being unregistered (the pointer
/// is still valid: removal happens under the same REGISTRY lock).
pub(super) fn event_enabled_armed(p: usize) -> bool {
    // SAFETY: INV-PERF-EVENTREF-1 — unregister-path, registry lock held.
    unsafe { (*(p as *mut PerfEvent)).enabled.load(core::sync::atomic::Ordering::Acquire) }
}

/// Wake an event's waiters after the registry walk (timerfd
/// expire-notify discipline: no locks held; registry membership proves
/// liveness — close unregisters before freeing).
pub(super) fn event_wake_all(p: *mut PerfEvent) {
    // SAFETY: INV-PERF-EVENTREF-1 above.
    unsafe { (*p).wait_queue.wake_up_all() };
}

/// The close-path Box reclaim (ptr came from Box::into_raw in
/// sys_perf_event_open; the file cell was already cleared, so no
/// reader can reach it).
pub(super) fn event_take_box(ptr: *mut PerfEvent) {
    // SAFETY: INV-PERF-EVENTREF-1 — single-owner close-path reclaim.
    unsafe { let _ = alloc::boxed::Box::from_raw(ptr); }
}

/// Read the id of an event being torn down by its own close path
/// (single-owner context).
pub(super) fn event_id_of(ev_ptr: *mut PerfEvent) -> u64 {
    // SAFETY: INV-PERF-EVENTREF-1 — close path owns the event.
    unsafe { (*ev_ptr).id }
}

// ------------------------------------------------------------------
// Event ring / direct-map pages
// ------------------------------------------------------------------

/// INV-PERF-RINGMEM-1: the ring pages are order-0 frames this event
/// allocated from the page allocator and own until perf_close's owner
/// drop; their direct-map translations are valid kernel memory for
/// that lifetime, and every accessor below stays inside one page.
///
/// In-page pointer `base + off` (off < PAGE_SIZE by the caller's
/// modulo).
pub(super) fn ring_page_ptr(base: *mut u8, off: usize) -> *mut u8 {
    // SAFETY: INV-PERF-RINGMEM-1 — direct-map page we own; off in page.
    unsafe { base.add(off) }
}

/// Copy `bytes` into the ring at the wrapped position (dst is our
/// direct-map page; n within the page).
pub(super) fn ring_copy(dst: *mut u8, src: &[u8], n: usize) {
    // SAFETY: INV-PERF-RINGMEM-1 — dst points into kernel direct-map
    // memory we own; n within the page.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, n) };
}

/// Volatile read of the user-visible data_tail word (our own page; a
/// racing user store is benign, u64-aligned).
pub(super) fn ring_tail_read(base: *mut u8, mp_data_tail: usize) -> u64 {
    // SAFETY: INV-PERF-RINGMEM-1 — our own metadata page, aligned.
    unsafe { core::ptr::read_volatile(base.add(mp_data_tail) as *const u64) }
}

/// Volatile release-store of the data_head word (the record bytes are
/// visible before the head bump).
pub(super) fn ring_head_write(base: *mut u8, mp_data_head: usize, head: u64) {
    // SAFETY: INV-PERF-RINGMEM-1 — our own metadata page, aligned.
    unsafe { core::ptr::write_volatile(base.add(mp_data_head) as *mut u64, head) };
}

/// Publish the ring metadata block on page 0 (setup path, page not yet
/// visible to the event owner).
pub(super) fn ring_meta_init(base: *mut u8, mp: &[(usize, u64)], mp32: &[(usize, u32)]) {
    // SAFETY: INV-PERF-RINGMEM-1 — our own metadata page; offsets are
    // the module's MP_* constants, well inside the page.
    unsafe {
        for (off, v) in mp32 {
            core::ptr::write_volatile(base.add(*off) as *mut u32, *v);
        }
        for (off, v) in mp {
            core::ptr::write_volatile(base.add(*off) as *mut u64, *v);
        }
    }
}

/// Zero a freshly allocated ring page (pre-publication).
pub(super) fn ring_page_zero(kv: *mut u8, page_size: usize) {
    // SAFETY: INV-PERF-RINGMEM-1 — the page is `page_size` bytes and not
    // yet published.
    unsafe { core::ptr::write_bytes(kv, 0, page_size) };
}

/// Install one user PTE for a ring page (fresh page, aligned, under the
/// caller's PTE lock).
pub(super) fn ring_map_page(
    root: u64,
    va: u64,
    pa: u64,
    flags: u64,
) {
    // SAFETY: INV-PERF-RINGMEM-1 — fresh pages, aligned, PTE lock held.
    unsafe {
        crate::arch::mm::map_page(
            root,
            crate::arch::mm::VirtAddr::new(va),
            crate::arch::mm::PhysAddr::new(pa),
            flags,
        );
    }
}

/// Take the mapping reference on a ring page just installed into user
/// PTEs (binder/io_uring discipline, review 2R.6).
pub(super) fn ring_page_get(page: *mut crate::mm::page_desc::Page) {
    // SAFETY: INV-PERF-RINGMEM-1 — descriptor of the page we mapped.
    unsafe { (*page).get_page(); }
}

/// Owner drop of one ring frame: put_page, free at 0, leak-and-report
/// on underflow (hp88a: an over-put frame may already be re-sold —
/// freeing again would hand it to two owners).
pub(super) fn ring_frame_owner_drop(p: usize) {
    let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(p));
    if page.is_null() {
        return;
    }
    // SAFETY: INV-PERF-RINGMEM-1 — descriptor of a page we allocated.
    let r = unsafe { (*page).put_page() };
    if r == 0 {
        crate::mm::page_alloc::free_pages(p, 0);
    } else if r < 0 {
        // Underflow refusal (refcount was already 0): the frame has been
        // over-put — it may already be back in the allocator and owned by
        // someone else. Freeing again would hand the same frame to two
        // owners; leak it and name the bug (hp88a: the old `r <= 0` freed
        // here — a double-free of a possibly re-sold frame).
        static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_err!(
                "PERF-CLOSE-UNDERFLOW: ring frame {:#x} refcount already 0 at owner drop — an extra put happened (leaking, not freeing) pid {}",
                p,
                crate::process::current_pid()
            );
        }
    }
}

/// Ring-audit descriptor read: refcount of a frame the ring still
/// lists (the descriptor of a frame we allocated).
pub(super) fn page_desc_refcount(pd: *mut crate::mm::page_desc::Page) -> i32 {
    // SAFETY: INV-PERF-RINGMEM-1 — descriptor of a ring frame we
    // allocated; the caller null-checked.
    unsafe { (*pd).refcount() }
}

/// The mmap-path twin: (refcount, mapcount) of a ring frame.
pub(super) fn page_desc_refs(pd: *mut crate::mm::page_desc::Page) -> (i32, i32) {
    // SAFETY: INV-PERF-RINGMEM-1 — descriptor of a ring frame we
    // allocated; the caller null-checked.
    unsafe { ((*pd).refcount(), (*pd).mapcount()) }
}

// ------------------------------------------------------------------
// Private-data cell
// ------------------------------------------------------------------

/// INV-PERF-PRIVDATA-1: the perf file's private-data cell is written
/// once at open (sys_perf_event_open, before the fd is published) and
/// taken once on the close path; reads happen on files whose fd is
/// held, so no take can race them.
///
/// Read the installed event pointer out of the private-data cell.
pub(super) fn privdata_get(file: &crate::fs::file::File) -> Option<*mut PerfEvent> {
    // SAFETY: INV-PERF-PRIVDATA-1 above (write-once cell discipline).
    unsafe { (*file.private_data.get()).map(|p| p as *mut PerfEvent) }
}

/// Clear the cell on the close path before the Box reclaim (so no new
/// reader can reach the event).
pub(super) fn privdata_clear(file: &crate::fs::file::File) {
    // SAFETY: INV-PERF-PRIVDATA-1 — the close-path take is single-owner.
    unsafe { *file.private_data.get() = None; }
}

// ------------------------------------------------------------------
// Kernel stack-frame walk
// ------------------------------------------------------------------

/// INV-PERF-STACKWALK-1: `bp` was validated (aligned, kernel range) by
/// the caller's loop; the two frame-word reads are the standard unwind
/// operation on a kernel stack. A torn read ends the walk via the
/// caller's monotonicity checks — worst case a garbage frame address
/// is filtered by the ksyms check.
///
/// Read (return-address, next-frame) at a validated frame pointer.
pub(super) fn frame_words(bp: u64) -> (u64, u64) {
    // SAFETY: INV-PERF-STACKWALK-1 above.
    unsafe {
        (
            core::ptr::read_volatile((bp + 8) as *const u64),
            core::ptr::read_volatile(bp as *const u64),
        )
    }
}

// ------------------------------------------------------------------
// Pinned-task scalar reads
// ------------------------------------------------------------------

/// INV-PERF-PINNEDTASK-1: the target Task pointer comes from
/// `pid_hash_lookup_pinned`, which took a reference — liveness is
/// proven for the block; the reads are plain integer counters, and the
/// caller drops the pin (`Task::task_put`) right after.
///
/// (nvcsw + nivcsw) of a pinned task.
pub(super) fn pinned_ctxsw(t: *mut crate::process::task::Task) -> u64 {
    // SAFETY: INV-PERF-PINNEDTASK-1 above (pinned reference live).
    unsafe {
        ((*t).nvcsw.load(core::sync::atomic::Ordering::Relaxed)
            + (*t).nivcsw.load(core::sync::atomic::Ordering::Relaxed)) as u64
    }
}

/// (min_flt + maj_flt) of a pinned task.
pub(super) fn pinned_faults(t: *mut crate::process::task::Task) -> u64 {
    // SAFETY: INV-PERF-PINNEDTASK-1 above.
    unsafe {
        ((*t).min_flt.load(core::sync::atomic::Ordering::Relaxed)
            + (*t).maj_flt.load(core::sync::atomic::Ordering::Relaxed)) as u64
    }
}

/// Real uid of a pinned task (the perf permission check's target).
pub(super) fn pinned_uid(t: *mut crate::process::task::Task) -> u32 {
    // SAFETY: INV-PERF-PINNEDTASK-1 above.
    unsafe { (*t).cred().uid }
}

// ------------------------------------------------------------------
// ioctl user transfers
// ------------------------------------------------------------------

/// INV-PERF-UACCESS-1: the ioctl `arg` addresses are user pointers
/// handed through the syscall entry; get_user/put_user fault safely on
/// bad addresses and the transfer is a plain u64 / fixed-layout
/// AttrHead (plain u32/u64 fields, the layout the reference shares).
///
/// Read a u64 ioctl argument (None on fault).
pub(super) fn u64_get(addr: usize) -> Option<u64> {
    // SAFETY: INV-PERF-UACCESS-1 — fault-safe user read.
    unsafe { crate::arch::uaccess::get_user(addr as *const u64) }
}

/// Write a u64 ioctl result (false on fault).
pub(super) fn u64_put(addr: usize, v: u64) -> bool {
    // SAFETY: INV-PERF-UACCESS-1 — fault-safe user write.
    unsafe { crate::arch::uaccess::put_user(addr as *mut u64, v) }
}

/// Read the fixed-layout AttrHead from user memory (None on fault).
pub(super) fn attr_head_get(addr: usize) -> Option<super::AttrHead> {
    // SAFETY: INV-PERF-UACCESS-1 — fault-safe user read of the POD head.
    unsafe { crate::arch::uaccess::get_user(addr as *const super::AttrHead) }
}
