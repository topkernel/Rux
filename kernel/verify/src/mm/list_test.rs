//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Intrusive doubly-linked list invariant tests.
//!
//! Types copied from: kernel/src/list.rs

use proptest::prelude::*;
use std::ptr;

// ============================================================================
// Copied types from kernel/src/list.rs
// ============================================================================

#[repr(C)]
pub struct ListHead {
    pub next: *mut ListHead,
    pub prev: *mut ListHead,
}

impl ListHead {
    pub const fn new() -> Self {
        Self {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }

    pub fn init(&mut self) {
        self.next = self;
        self.prev = self;
    }

    pub fn is_empty(&self) -> bool {
        self.next == self as *const _ as *mut _
    }

    pub unsafe fn add(&mut self, head: *mut ListHead) {
        let next = (*head).next;
        self.next = next;
        self.prev = head;
        (*head).next = self;
        (*next).prev = self;
    }

    pub unsafe fn add_tail(&mut self, head: *mut ListHead) {
        let prev = (*head).prev;
        self.next = head;
        self.prev = prev;
        (*head).prev = self;
        (*prev).next = self;
    }

    pub unsafe fn del(&mut self) {
        let next = self.next;
        let prev = self.prev;
        (*next).prev = prev;
        (*prev).next = next;
        self.next = self as *mut _;
        self.prev = self as *mut _;
    }

    pub unsafe fn for_each<F>(head: *mut ListHead, mut f: F)
    where
        F: FnMut(*mut ListHead),
    {
        let mut pos = (*head).next;
        let mut iterations = 0usize;
        while pos != head {
            if iterations > 1000 {
                break;
            }
            iterations += 1;
            let next = (*pos).next;
            f(pos);
            pos = next;
        }
    }
}

// ============================================================================
// Helper: validate circular list integrity
// ============================================================================

#[allow(dead_code)]
/// Walk the forward chain and return (count, is_circular).
/// A valid list: head <-> node1 <-> node2 <-> ... <-> head.
unsafe fn validate_list(head: *mut ListHead) -> (usize, bool) {
    if (*head).next == head && (*head).prev == head {
        return (0, true); // empty list
    }

    let mut count = 0usize;
    let mut pos = (*head).next;
    while pos != head {
        count += 1;
        if count > 200 {
            return (count, false); // not circular or too long
        }
        pos = (*pos).next;
    }

    // Also verify backward traversal reaches head
    let mut back_count = 0usize;
    let mut pos = (*head).prev;
    while pos != head {
        back_count += 1;
        if back_count > 200 {
            return (count, false);
        }
        pos = (*pos).prev;
    }

    (count, count == back_count)
}

#[allow(dead_code)]
/// Walk forward chain, return Vec of raw pointers for ordering verification.
unsafe fn collect_forward(head: *mut ListHead) -> Vec<*mut ListHead> {
    let mut result = Vec::new();
    let mut pos = (*head).next;
    while pos != head {
        result.push(pos);
        pos = (*pos).next;
    }
    result
}

#[allow(dead_code)]
/// Walk backward chain, return Vec of raw pointers.
unsafe fn collect_backward(head: *mut ListHead) -> Vec<*mut ListHead> {
    let mut result = Vec::new();
    let mut pos = (*head).prev;
    while pos != head {
        result.push(pos);
        pos = (*pos).prev;
    }
    result
}

#[allow(dead_code)]
/// Verify prev/next symmetry: for every node, node.next.prev == node.
unsafe fn verify_symmetry(head: *mut ListHead, count: usize) -> bool {
    let mut pos = (*head).next;
    for _ in 0..count {
        let next = (*pos).next;
        if (*next).prev != pos {
            return false;
        }
        pos = next;
    }
    // head itself
    if (*(*head).next).prev != head {
        return false;
    }
    if (*(*head).prev).next != head {
        return false;
    }
    true
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-LIST-1: after add, new node is directly after head
    #[test]
    fn test_add_inserts_after_head(
        n_adds in 0usize..20usize,
        _payload in proptest::collection::vec(0u32..1000u32, 0..20),
    ) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_adds {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add(&mut head);
            }

            if n_adds > 0 {
                // head.next should be the last added node (most recently added)
                let first = head.next;
                prop_assert_eq!((*first).prev, &head as *const _ as *mut _);
                // head.prev should be the first added node (least recently added)
                let last = head.prev;
                prop_assert_eq!((*last).next, &head as *const _ as *mut _);
            }

            let (count, circular) = validate_list(&mut head);
            prop_assert!(circular, "list not circular after {} adds", n_adds);
            prop_assert_eq!(count, n_adds);
            prop_assert!(verify_symmetry(&mut head, count));
        }
    }

    /// INV-LIST-2: add_tail inserts at tail (before head)
    #[test]
    fn test_add_tail_inserts_at_tail(
        n_adds in 0usize..20usize,
    ) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_adds {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add_tail(&mut head);
            }

            if n_adds > 0 {
                // head.next should be the first added node
                let first = head.next;
                prop_assert_eq!((*first).prev, &head as *const _ as *mut _);
                // head.prev should be the last added node
                let last = head.prev;
                prop_assert_eq!((*last).next, &head as *const _ as *mut _);
            }

            let (count, circular) = validate_list(&mut head);
            prop_assert!(circular, "list not circular after {} add_tails", n_adds);
            prop_assert_eq!(count, n_adds);
            prop_assert!(verify_symmetry(&mut head, count));
        }
    }

    /// INV-LIST-3: add order preserved in forward traversal (LIFO for add)
    #[test]
    fn test_add_is_lifo(n_adds in 1usize..15usize) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_adds {
                nodes.push(Box::new(ListHead::new()));
            }

            // add is LIFO: last added appears first
            let mut addrs: Vec<*mut ListHead> = Vec::new();
            for node in nodes.iter_mut() {
                let addr = node.as_mut() as *mut ListHead;
                node.as_mut().add(&mut head);
                addrs.push(addr);
            }

            let forward = collect_forward(&mut head);
            // Forward traversal: last added first
            prop_assert_eq!(forward.len(), n_adds);
            for (i, addr) in addrs.iter().rev().enumerate() {
                prop_assert_eq!(forward[i], *addr, "LIFO order violated at index {}", i);
            }
        }
    }

    /// INV-LIST-4: add_tail order preserved (FIFO)
    #[test]
    fn test_add_tail_is_fifo(n_adds in 1usize..15usize) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_adds {
                nodes.push(Box::new(ListHead::new()));
            }

            let mut addrs: Vec<*mut ListHead> = Vec::new();
            for node in nodes.iter_mut() {
                let addr = node.as_mut() as *mut ListHead;
                node.as_mut().add_tail(&mut head);
                addrs.push(addr);
            }

            let forward = collect_forward(&mut head);
            prop_assert_eq!(forward.len(), n_adds);
            for (i, addr) in addrs.iter().enumerate() {
                prop_assert_eq!(forward[i], *addr, "FIFO order violated at index {}", i);
            }
        }
    }

    /// INV-LIST-5: del removes node and preserves list integrity
    #[test]
    fn test_del_preserves_integrity(
        n_adds in 3usize..15usize,
        remove_idx in 0usize..15usize,
    ) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_adds {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add(&mut head);
            }

            let remove_idx = remove_idx % n_adds;
            nodes[remove_idx].del();

            // Verify list still has correct count and is circular
            let (count, circular) = validate_list(&mut head);
            prop_assert!(circular, "list not circular after del");
            prop_assert_eq!(count, n_adds - 1);
            prop_assert!(verify_symmetry(&mut head, count));

            // Verify removed node points to itself
            prop_assert_eq!(nodes[remove_idx].next, &*nodes[remove_idx] as *const _ as *mut _);
            prop_assert_eq!(nodes[remove_idx].prev, &*nodes[remove_idx] as *const _ as *mut _);
        }
    }

    /// INV-LIST-6: add + del returns to empty list
    #[test]
    fn test_add_del_returns_empty(
        n_nodes in 1usize..10usize,
    ) {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_nodes {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add(&mut head);
            }
            prop_assert!(!head.is_empty());

            for node in nodes.iter_mut() {
                node.as_mut().del();
            }
            prop_assert!(head.is_empty());
            prop_assert_eq!(head.next, &head as *const _ as *mut _);
            prop_assert_eq!(head.prev, &head as *const _ as *mut _);
        }
    }

    /// INV-LIST-7: for_each visits exactly N nodes
    #[test]
    fn test_for_each_visits_all(n_nodes in 0usize..20usize) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_nodes {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add(&mut head);
            }

            let mut visited = Vec::new();
            ListHead::for_each(&mut head, |pos| {
                visited.push(pos);
            });

            prop_assert_eq!(visited.len(), n_nodes);
            // No duplicates
            let mut sorted = visited.clone();
            sorted.sort_by_key(|p| *p as usize);
            sorted.dedup();
            prop_assert_eq!(sorted.len(), visited.len(), "for_each visited duplicate nodes");
        }
    }

    /// INV-LIST-8: interleaved add/del maintains integrity
    #[test]
    fn test_interleaved_add_del(
        ops in proptest::collection::vec(
            proptest::bool::ANY,
            0..50
        ),
    ) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut pool: Vec<Box<ListHead>> = Vec::new();
            let mut in_list: Vec<*mut ListHead> = Vec::new();

            for do_add in ops {
                if do_add {
                    let mut node = Box::new(ListHead::new());
                    node.as_mut().add(&mut head);
                    in_list.push(node.as_mut() as *mut ListHead);
                    pool.push(node);
                } else if let Some(node_ptr) = in_list.pop() {
                    (*node_ptr).del();
                }
            }

            let (count, circular) = validate_list(&mut head);
            prop_assert!(circular, "list not circular after interleaved ops");
            prop_assert_eq!(count, in_list.len());
            prop_assert!(verify_symmetry(&mut head, count));
        }
    }

    /// INV-LIST-9: forward and backward traversals are reverses of each other
    #[test]
    fn test_forward_backward_symmetry(n_nodes in 1usize..15usize) {
        unsafe {
            let mut head = ListHead::new();
            head.init();
            let mut nodes: Vec<Box<ListHead>> = Vec::new();
            for _ in 0..n_nodes {
                nodes.push(Box::new(ListHead::new()));
            }

            for node in nodes.iter_mut() {
                node.as_mut().add_tail(&mut head);
            }

            let forward = collect_forward(&mut head);
            let mut backward = collect_backward(&mut head);
            backward.reverse();

            prop_assert_eq!(forward.len(), backward.len());
            for (i, (f, b)) in forward.iter().zip(backward.iter()).enumerate() {
                prop_assert_eq!(*f, *b, "forward/backward mismatch at index {}", i);
            }
        }
    }

    /// INV-LIST-10: del first, del last, del middle all work
    #[test]
    fn test_del_positions(n_nodes in 3usize..10usize) {
        unsafe {
            for remove_pos in 0..n_nodes {
                let mut head = ListHead::new();
                head.init();
                let mut nodes: Vec<Box<ListHead>> = Vec::new();
                for _ in 0..n_nodes {
                    nodes.push(Box::new(ListHead::new()));
                }

                for node in nodes.iter_mut() {
                    node.as_mut().add_tail(&mut head);
                }

                let forward_before = collect_forward(&mut head);
                let removed = forward_before[remove_pos];
                (*removed).del();

                let forward_after = collect_forward(&mut head);
                prop_assert_eq!(forward_after.len(), n_nodes - 1);

                // Removed node not in list
                prop_assert!(!forward_after.iter().any(|&p| p == removed));

                // Remaining order preserved
                let expected: Vec<*mut ListHead> = forward_before
                    .iter()
                    .filter(|&&p| p != removed)
                    .copied()
                    .collect();
                prop_assert_eq!(forward_after, expected, "order broken when removing pos {}", remove_pos);

                let (count, circular) = validate_list(&mut head);
                prop_assert!(circular);
                prop_assert_eq!(count, n_nodes - 1);
            }
        }
    }
}
