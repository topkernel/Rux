//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The syscall subsystem's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L7 (2026-11-20), framekernel doctrine: the syscall
//! SERVICE files (`misc.rs`, `process.rs`, `file.rs`, `memory.rs`,
//! `signal.rs`, ...) end with ZERO `unsafe` keywords — checkable by
//! grep, ready for a crate-level `forbid(unsafe_code)`. Every
//! genuinely-unsafe operation they need lives HERE as a named edge with
//! an INV contract, in the shape of `kernel/src/sync/edges.rs` and
//! `kernel/src/process/unsafe_edges.rs` (one audit per edge, not per
//! call site). User-memory copies do not live here at all: they route
//! through the already-safe `crate::arch::uaccess_safe` entries, and
//! task-pointer borrows reuse
//! [`crate::process::unsafe_edges::borrow_task`].
//!
//! INV registry:
//!
//! - INV-SYSC-1 (intrusive interest/wake lists) — the epoll family's
//!   `ListHead` nodes (`EpollEntry`, `EpollWakeNode`): the `list` field
//!   is the FIRST field of the node type, so a node pointer doubles as
//!   its `ListHead` pointer and the container_of cast is the pointer
//!   cast itself. Every node is Box-allocated, linked/unlinked only
//!   under the lock that owns the list (the per-epoll `entries` lock or
//!   the wake-shard irqsave lock), and freed only after `unlink` (or
//!   when never linked). Walks capture `next` before the body touches
//!   the node — the unlink-during-walk discipline the pre-conversion
//!   loops documented inline.
//! - INV-SYSC-2 (unshared box + exactly-once reclaim) — constructor-site
//!   raw pointers produced by `Box::into_raw` in the same function:
//!   either the box is still uniquely owned (created, not yet installed
//!   into any shared structure), or it is the reclaim drop matching
//!   exactly that `into_raw`. `boxed_drop` is the pairing release for
//!   every reclaim site; a pointer is dropped exactly once per
//!   successful `into_raw` (error paths drop when the install failed,
//!   success paths hand ownership to the close op).
//! - INV-SYSC-3 (current-task park/unwind) — the repeated
//!   set_state(RUNNING) + dequeue_task pair on a raw current-task
//!   pointer (the NEW-C2 racing-wake-undo idiom): `task` is the calling
//!   CPU's current task or a waiter this CPU just parked; the pair must
//!   run together so a wake that enqueued us between the state check
//!   and the sleep is undone before the caller re-runs.

use crate::list::ListHead;
use crate::process::task::Task;

// ---------------------------------------------------------------------------
// INV-SYSC-1: epoll intrusive-list family
// ---------------------------------------------------------------------------

/// The first node pointer after the sentinel `head`
/// (INV-SYSC-1: `head` is a live, initialized sentinel under its lock).
#[inline]
pub fn list_first(head: *mut ListHead) -> *mut ListHead {
    debug_assert!(!head.is_null());
    // SAFETY: INV-SYSC-1 — plain load of the sentinel's next field.
    unsafe { (*head).next }
}

/// The node after `cur` (INV-SYSC-1: `cur` is a linked node's ListHead;
/// capture-before-body discipline — call this BEFORE unlinking `cur`).
#[inline]
pub fn list_next(cur: *mut ListHead) -> *mut ListHead {
    debug_assert!(!cur.is_null());
    // SAFETY: INV-SYSC-1 — plain load of a linked node's next field.
    unsafe { (*cur).next }
}

/// The node object whose FIRST field is the ListHead `cur`
/// (INV-SYSC-1 shared arm: read-only walk body under the owning lock).
#[inline]
pub fn list_entry<T>(cur: *mut ListHead) -> &'static T {
    debug_assert!(!cur.is_null());
    debug_assert!(cur as usize % core::mem::align_of::<T>() == 0);
    // SAFETY: INV-SYSC-1 — cur is a linked node's ListHead and T's
    // first field (offset 0), so the cast IS the container_of; the
    // node is alive and unlinked only under the caller-held lock.
    unsafe { &*(cur as *const T) }
}

/// The node object mutably (INV-SYSC-1 exclusive arm: read-modify walk
/// body under the owning lock; no other alias is live).
#[inline]
pub fn list_entry_mut<T>(cur: *mut ListHead) -> &'static mut T {
    debug_assert!(!cur.is_null());
    debug_assert!(cur as usize % core::mem::align_of::<T>() == 0);
    // SAFETY: INV-SYSC-1 — same container_of as list_entry, made
    // exclusive: the caller holds the lock that serializes every
    // mutable touch of this list, so no alias can be live.
    unsafe { &mut *(cur as *mut T) }
}

/// Link a fresh, unlinked node at the tail of `head`'s list
/// (INV-SYSC-1 link arm: node was Box::into_raw'd and never linked;
/// caller holds the list's lock).
#[inline]
pub fn list_link_tail(node: *mut ListHead, head: *mut ListHead) {
    debug_assert!(!node.is_null() && !head.is_null());
    // SAFETY: INV-SYSC-1 — node's list is initialized (fresh) and
    // unlinked; head is the live sentinel held under the lock.
    unsafe { (*node).add_tail(head) }
}

/// Unlink a linked node (INV-SYSC-1 unlink arm: caller holds the list's
/// lock; the node is currently linked and its neighbors are live).
#[inline]
pub fn list_unlink(node: &mut ListHead) {
    // SAFETY: INV-SYSC-1 — lock-held unlink of a linked node.
    unsafe { node.del() }
}

// ---------------------------------------------------------------------------
// The `Send` proofs for the raw-pointer registry types (moved from
// misc.rs so the service file carries no `unsafe` keywords; same
// contracts the inline impls documented).
// ---------------------------------------------------------------------------

// SAFETY: the registry's epoll pointers are only dereferenced under the
// registry/shard lock and are removed in the owning EpollFile's close
// op, which runs before the box is freed (INV-SYSC-1/2 serialization).
unsafe impl Send for super::misc::EpollWakeRegistry {}

// SAFETY: same contract as EpollWakeRegistry: `ep` is only dereferenced
// while the binding is linked (shard lock held or pinned per
// notify_pins), and the node is unlinked before the epoll's close op
// frees the box (INV-SYSC-1/2).
unsafe impl Send for super::misc::EpollWakeNode {}

// SAFETY: same contract as EpollWakeRegistry (the per-shard type).
unsafe impl Send for super::misc::EpollWakeShard {}

// SAFETY: task pointers are only compared for identity; queue pointers
// are unregistered in the owning fd's close op before the box is freed.
unsafe impl Send for super::misc::SignalfdWakeRegistry {}

// ---------------------------------------------------------------------------
// INV-SYSC-2: unshared-box borrows + the exactly-once reclaim
// ---------------------------------------------------------------------------

/// Shared borrow of a `Box::into_raw` object the caller uniquely owns
/// (INV-SYSC-2 shared arm: created in this call path, not yet installed
/// into any shared structure — or reached through a discipline that
/// proves it alive, such as the File identity check that precedes every
/// private_data downcast).
#[inline]
pub fn boxed_ref<T>(p: *const T) -> &'static T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — non-null into_raw pointer whose box the
    // caller owns or has proven alive; the borrow ends before any drop.
    unsafe { &*p }
}

/// Mutable borrow of an unshared `Box::into_raw` object
/// (INV-SYSC-2 exclusive arm: the box is uniquely owned — created here,
/// or the close path's last-reference reclaim where nothing else can
/// alias it).
#[inline]
pub fn boxed_mut<T>(p: *mut T) -> &'static mut T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — non-null into_raw pointer, uniquely owned:
    // no alias can be live while this reference exists.
    unsafe { &mut *p }
}

/// Reclaim a `Box::into_raw` allocation (INV-SYSC-2 reclaim arm: the
/// exactly-once drop pairing the constructor's `into_raw` — error paths
/// of the install, or the close op running on the last reference).
#[inline]
pub fn boxed_drop<T>(p: *mut T) {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — the pointer came from Box::into_raw, is
    // unlinked/unregistered from every reachable structure, and is
    // dropped exactly once on this path.
    unsafe { drop(alloc::boxed::Box::from_raw(p)) }
}

// ---------------------------------------------------------------------------
// INV-SYSC-3: current-task park/unwind
// ---------------------------------------------------------------------------

/// Undo a racing wake enqueue: set the task RUNNING and dequeue it if a
/// concurrent wake had re-queued us while we were still executing
/// (INV-SYSC-3: `task` is this CPU's current task — either parked by
/// this thread and re-taken, or never parked).
#[inline]
pub fn task_park_unwind(task: *mut Task) {
    let t = crate::process::unsafe_edges::borrow_task(task);
    t.set_state(crate::process::task::TaskState::new(
        crate::process::task::TaskState::RUNNING,
    ));
    crate::sched::dequeue_task(t);
}

// ---------------------------------------------------------------------------
// Arch reset edges (INV-SYSC-4: the reboot/panic ladder's port I/O and
// bare asm — deliberately-faulting or never-returning reset sequences
// that have no safe shape; reached only behind sys_reboot's CAP_SYS_BOOT
// gate or the panic timeout).
// ---------------------------------------------------------------------------

/// x86_64 panic reset: drain the 8042, pulse its reset line, then force
/// a triple fault through a null IDT (INV-SYSC-4; never returns to a
/// working machine — the CPU resets or QEMU dies).
#[cfg(feature = "x86_64")]
pub fn x86_panic_reset() {
    // SAFETY: INV-SYSC-4 — bare port I/O on the reset port and a
    // deliberate faulting instruction; neither returns.
    unsafe {
        // Drain the 8042 input buffer first (writing the command
        // while it is full is dropped on real hardware).
        for _ in 0..100_000 {
            if crate::arch::cpu::inb(0x64) & 0x02 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0x64, 0xFE);
        // Give the pulse time to land.
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }
        // Triple fault: load a null/limit-0 IDT and int3 — the
        // vector lookup must fail, the double fault must fail the
        // same way, and the CPU (QEMU included) resets.
        static NULL_IDT: [u64; 2] = [0; 2];
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &NULL_IDT,
            options(nomem, nostack)
        );
        core::arch::asm!("int3", options(nomem, nostack));
    }
}

/// x86_64 reboot pulse: the 8042 keyboard-controller reset line, then
/// the PCI/ATX reset port 0xcf9 (INV-SYSC-4; the caller parks after if
/// the platform ignores both).
#[cfg(feature = "x86_64")]
pub fn x86_reboot_pulse() {
    // SAFETY: INV-SYSC-4 — the two reset-port writes; reached only
    // behind the CAP_SYS_BOOT gate in sys_reboot.
    unsafe {
        crate::arch::cpu::outb(0x64, 0xFE);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0xcf9, 0x0E);
    }
}

/// riscv64 SBI legacy shutdown ecall (FID 0x8) — the pre-0.3 power-off
/// path (INV-SYSC-4: the privileged ecall itself).
#[cfg(feature = "riscv64")]
pub fn sbi_shutdown_ecall() {
    // SAFETY: INV-SYSC-4 — privileged SBI ecall; reached only after the
    // CAP_SYS_BOOT gate in sys_reboot (or the panic fallback).
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 0x8u64,
            out("a0") _,
            out("a1") _,
            options(nomem)
        );
    }
}

// ---------------------------------------------------------------------------
// Filetyped subsystem calls that are still `unsafe fn` in their home
// crates: thin named wrappers restating the call-site discipline once.
// ---------------------------------------------------------------------------

/// Resolve a file's pidfd identity (pidfd_open and /proc/<pid> dir fds).
///
/// INV-SYSC-5: the caller has verified the File's ops identity
/// (PIDFD_OPS / PROC_PIDDIR_OPS) before this call — the slot was
/// populated by `pidfd::attach` (Box::into_raw) and stays owned by the
/// file for its lifetime, so the returned reference is valid while the
/// caller holds its fd-table Arc.
#[inline]
pub fn pidfd_data_of(file: &crate::fs::File) -> Option<&'static crate::fs::pidfd::PidFdData> {
    // SAFETY: INV-SYSC-5 — identity-checked private_data downcast; the
    // box lives as long as the File.
    unsafe { crate::fs::pidfd::data_of(file) }
}

/// ext4 iget by on-disk inode number (the name_to_handle_at follower).
///
/// INV-SYSC-6: `base` carries EXT4_INODE_OPS (the caller dispatches on
/// the ops identity before reaching here) — the SAFETY contract
/// `ext4::iget_by_ino` states.
#[inline]
pub fn ext4_iget_by_ino(
    base: &crate::fs::inode::Inode,
    ino: u64,
) -> Result<alloc::sync::Arc<crate::fs::inode::Inode>, crate::errno::Errno> {
    // SAFETY: INV-SYSC-6 — base is an ext4 inode (ops-checked).
    unsafe { crate::fs::ext4::iget_by_ino(base, ino) }
}
