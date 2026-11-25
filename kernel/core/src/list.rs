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
//! every unsafe method's contract is "the caller holds the embedding
//! structure's lock and passes nodes that are live members of THIS
//! list or freshly constructed" — the per-method Safety notes below
//! restate that contract per shape. This module IS the primitive core
//! (the plan's round-C class (a)); it stays unsafe-fn by design.

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
    /// # Safety
    /// Caller must ensure `head` is valid
    ///
    /// ...
    pub unsafe fn add(&mut self, head: *mut ListHead) {
        let next = (*head).next;

        // Insert current node between head and head->next
        self.next = next;
        self.prev = head;
        (*head).next = self;
        (*next).prev = self;
    }

    /// Add node at list tail
    ///
    /// # Arguments
    /// - `head`: List head (insert before head, i.e. at tail)
    ///
    /// # Safety
    /// Caller must ensure `head` is valid
    ///
    /// ...
    pub unsafe fn add_tail(&mut self, head: *mut ListHead) {
        let prev = (*head).prev;

        // Insert current node between head->prev and head
        self.next = head;
        self.prev = prev;
        (*head).prev = self;
        (*prev).next = self;
    }

    /// Delete current node from list
    ///
    /// # Safety
    /// Caller must ensure node is in the list
    ///
    /// ...
    pub unsafe fn del(&mut self) {
        let next = self.next;
        let prev = self.prev;

        (*next).prev = prev;
        (*prev).next = next;

        // Mark as deleted (point to self, for debugging)
        self.next = self as *mut _;
        self.prev = self as *mut _;
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
    /// let task = unsafe { ListHead::entry(list_head_ptr, Task, children) };
    /// assert_eq!((*task).pid, 0);
    /// ```
    ///
    /// # Safety
    /// Caller must ensure `ptr` is valid and points to correct `member`
    ///
    /// ...
    pub unsafe fn entry<T>(ptr: *mut ListHead, member: impl OffsetHelper<T>) -> *mut T {
        // Calculate structure start address: ptr - offset_of(member)
        let offset = member.offset();
        (ptr as *mut u8).sub(offset) as *mut T
    }

    /// Iterate through list
    ///
    /// # Arguments
    /// - `head`: List head
    /// - `f`: Closure to call for each node
    ///
    /// # Safety
    /// Caller must ensure `head` is valid and list is not modified during iteration
    ///
    /// ...
    pub unsafe fn for_each<F>(head: *mut ListHead, mut f: F)
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
        let mut pos = (*head).next;
        while pos != head {
            let next = (*pos).next;
            f(pos);
            pos = next;
        }
    }

    /// Get first node
    ///
    /// ...
    pub unsafe fn first_entry<T>(head: *mut ListHead, member: impl OffsetHelper<T>) -> Option<*mut T> {
        if (*head).next == head {
            None
        } else {
            Some(Self::entry((*head).next, member))
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
                // Use core::mem::offset_of! (Rust 1.77+)
                // If not available, use unsafe alternative
                extern crate core;
                // SAFETY: MaybeUninit is never read, only its pointer is used to compute
                // the offset of a field — this is a standard offset-of pattern.
                unsafe {
                    let dummy = core::mem::MaybeUninit::<$type>::uninit();
                    let base = dummy.as_ptr();
                    let member_ptr = core::ptr::addr_of!((*base).$member);
                    (member_ptr as usize) - (base as usize)
                }
            }
        }
    };
}

