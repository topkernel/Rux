//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Doubly linked list implementation
//!
//!
//! Usage:
//! - Process tree: task_struct::children, task_struct::sibling
//! - Scheduler queue: rq::runqueue
//! - Device list: device::list
//!
//! Design features:
//! - Intrusive list: list_head is embedded directly in data structures
//! - Highly generic: same list can be used for different data types
//! - Small memory overhead: each node only needs 2 pointers (16 bytes)
//!
//! INV-LIST-1 (F15-registered primitive, safety-and-performance.md):
//! every list operation's contract is "the caller holds the embedding
//! structure's lock and passes nodes that are live members of THIS
//! list or freshly constructed" — the per-method notes below restate
//! that contract per shape.
//!
//! Zero-unsafe API surface (mainline minimal-first collection,
//! 2026-11-27): the raw link reads/writes moved to kernel/src/unsafe.rs
//! (INV-LIST-RAW-1); the API methods here stay `pub fn` — the contract
//! moved from the type system (unsafe fn) to the caller-side lock
//! discipline the INV records.
#![forbid(unsafe_code)]

use core::ptr;

#[repr(C)]
pub struct ListHead {
    /// Next node
    pub next: *mut ListHead,
    /// Previous node
    pub prev: *mut ListHead,
}

impl ListHead {
    /// Create a new list node
    ///
    /// Typically used to initialize list head
    pub const fn new() -> Self {
        Self {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }

    /// Initialize list node
    ///
    /// Make node point to itself, forming an empty list
    ///
    /// ...
    pub fn init(&mut self) {
        self.next = self;
        self.prev = self;
    }

    /// Check if list is empty
    ///
    /// ...
    pub fn is_empty(&self) -> bool {
        self.next == self as *const _ as *mut _
    }

    /// Insert current node after specified node
    ///
    /// # Arguments
    /// - `head`: List position to insert (insert after head)
    ///
    /// # Contract (INV-LIST-1 through the INV-LIST-RAW-1 edge)
    /// Caller holds the list-owning lock; `head` is a valid live
    /// position and `self` a freshly constructed node.
    ///
    /// ...
    pub fn add(&mut self, head: *mut ListHead) {
        let next = Self::raw_next(head);

        // Insert current node between head and head->next
        self.raw_link_between(head, next);
    }

    /// Add node at list tail
    ///
    /// # Arguments
    /// - `head`: List head (insert before head, i.e. at tail)
    ///
    /// # Contract (INV-LIST-1 through the INV-LIST-RAW-1 edge)
    /// Caller holds the list-owning lock; `head` is a valid live
    /// position and `self` a freshly constructed node.
    ///
    /// ...
    pub fn add_tail(&mut self, head: *mut ListHead) {
        let prev = Self::raw_prev(head);

        // Insert current node between head->prev and head
        self.raw_link_between(prev, head);
    }

    /// Delete current node from list
    ///
    /// # Contract (INV-LIST-1 through the INV-LIST-RAW-1 edge)
    /// Caller holds the list-owning lock; `self` is currently linked
    /// and its neighbors are live.
    ///
    /// ...
    pub fn del(&mut self) {
        self.raw_unlink();
    }

    /// Get reference to structure containing this ListHead
    ///
    /// # Arguments
    /// - `ptr`: ListHead pointer
    /// - `type`: Containing structure type
    /// - `member`: Field name of ListHead in the structure
    ///
    /// # Examples
    /// ```no_run
    /// # use crate::list::ListHead;
    /// # struct Task { children: ListHead, pid: u32 };
    /// # let list_head_ptr = &mut Task { children: ListHead::new(), pid: 0 }.children as *mut _;
    /// let task = ListHead::entry(list_head_ptr, Task, children);
    /// assert!(!task.is_null());
    /// ```
    ///
    /// # Contract (INV-LIST-1)
    /// Caller must ensure `ptr` is valid and points to correct `member`
    /// — the offset math is the container_of idiom, kept non-panicking
    /// (wrapping) exactly like the pointer arithmetic it replaces.
    ///
    /// ...
    pub fn entry<T>(ptr: *mut ListHead, member: impl OffsetHelper<T>) -> *mut T {
        // Calculate structure start address: ptr - offset_of(member)
        let offset = member.offset();
        (ptr as *mut u8).wrapping_sub(offset) as *mut T
    }

    /// Iterate through list
    ///
    /// # Arguments
    /// - `head`: List head
    /// - `f`: Closure to call for each node
    ///
    /// # Contract (INV-LIST-1 through the INV-LIST-RAW-1 edge)
    /// Caller holds the lock that serializes link/unlink on `head` for
    /// the whole walk; the list is not modified during iteration.
    ///
    /// ...
    pub fn for_each<F>(head: *mut ListHead, mut f: F)
    where
        F: FnMut(*mut ListHead),
    {
        // LTP-open lane (post-msgstress01 wedge, 2026-11-14): this walk
        // used to break at 1000 iterations "to prevent infinite loops".
        // The bound is a Linux-ABI divergence, not a safety net: the only
        // users are the task-tree children walks (for_each_child /
        // for_each_group_child / find_child_by_pid / count_children /
        // collect_children_pinned, all under PROCESS_TREE_LOCK), and a
        // parent can legitimately hold FAR more than 1000 children —
        // LTP msgstress01's timeout kill reparents ~1400 orphans to init
        // in one boot (TREE-RELEASE-HEAL 433 + the one-by-one reparents
        // ahead of it). New children are add_tail'd, so after the storm
        // every fresh child of pid 1 sat past position 1000; the cap then
        // truncated every do_wait walk, pid 1 never reaped or even FOUND
        // its later children, and all 32 subsequent tests in that scan
        // chunk returned the runner's exec-failed rc=254/127 (census
        // chunk 0600: "Too many iterations" before each RES row; the
        // storm1500 probe reproduces it exactly — /bin/echo exec rc=127
        // with 4 cap prints on the capped kernel, rc=0 x3 with zero
        // prints on this walk). Reference list_for_each_entry has NO
        // iteration cap; list integrity is a list-mutation invariant
        // (all mutations hold the owning lock), and a corrupted cycle
        // would be caught by the spinlock/deadlock watchdog — silently
        // truncating correct walks is the worse failure mode. Termination
        // is `pos != head`, exactly the reference discipline.
        let mut pos = Self::raw_next(head);
        while pos != head {
            let next = Self::raw_next(pos);
            f(pos);
            pos = next;
        }
    }

    /// Get first node
    ///
    /// ...
    pub fn first_entry<T>(head: *mut ListHead, member: impl OffsetHelper<T>) -> Option<*mut T> {
        let next = Self::raw_next(head);
        if next == head {
            None
        } else {
            Some(Self::entry(next, member))
        }
    }
}

pub trait OffsetHelper<T> {
    fn offset(&self) -> usize;
}

#[allow(dead_code)]
#[allow(unused_macros)]
macro_rules! impl_offset_helper {
    ($type:ty, $member:ident) => {
        impl OffsetHelper<$type> for fn() -> usize {
            fn offset(&self) -> usize {
                // core::mem::offset_of! (Rust 1.77+): the compile-time
                // constant the MaybeUninit/addr_of dance used to compute.
                ::core::mem::offset_of!($type, $member)
            }
        }
    };
}

