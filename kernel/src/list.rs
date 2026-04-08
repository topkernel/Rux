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
        let mut pos = (*head).next;
        let mut iterations = 0usize;
        while pos != head {
            if iterations > 1000 {
                // Prevent infinite loop
                use crate::console::putchar;
                const MSG: &[u8] = b"ListHead::for_each: Too many iterations, breaking\n";
                for &b in MSG {
                    putchar(b);
                }
                break;
            }
            iterations += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_init() {
        let mut head = ListHead::new();
        head.init();
        assert!(head.is_empty());
        assert_eq!(head.next, &head as *const _ as *mut _);
        assert_eq!(head.prev, &head as *const _ as *mut _);
    }

    #[test]
    fn test_list_add() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add(&mut head);

            assert!(!head.is_empty());
            assert_eq!(head.next, &node1 as *const _ as *mut _);
            assert_eq!(head.prev, &node1 as *const _ as *mut _);
        }
    }

    #[test]
    fn test_list_add_tail() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add_tail(&mut head);

            let mut node2 = ListHead::new();
            node2.add_tail(&mut head);

            // head -> node1 -> node2 -> head
            assert_eq!(head.next, &node1 as *const _ as *mut _);
            assert_eq!(node1.next, &node2 as *const _ as *mut _);
            assert_eq!(node2.next, &head as *const _ as *mut _);
        }
    }

    #[test]
    fn test_list_del() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add(&mut head);

            assert!(!head.is_empty());

            node1.del();

            assert!(head.is_empty());
        }
    }
}
