//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Buddy allocator core logic invariant tests.
//!
//! Types copied from: kernel/src/mm/buddy_allocator.rs

use proptest::prelude::*;

// ============================================================================
// Copied pure functions from kernel/src/mm/buddy_allocator.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;
const MAX_ORDER: usize = 10;

pub fn heap_size_to_order(size: usize) -> usize {
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    if pages <= 1 {
        return 0;
    }
    let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
    if order > MAX_ORDER { MAX_ORDER } else { order }
}

pub fn size_to_order(size: usize) -> usize {
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    if pages <= 1 {
        return 0;
    }
    let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
    if order > MAX_ORDER { MAX_ORDER } else { order }
}

pub fn get_buddy_idx(page_idx: usize, order: usize) -> usize {
    let block_size_pages = 1usize << order;
    page_idx ^ block_size_pages
}

pub fn page_idx_to_addr(heap_start: usize, page_idx: usize) -> usize {
    heap_start + page_idx * PAGE_SIZE
}

pub fn addr_to_page_idx(heap_start: usize, addr: usize) -> usize {
    (addr - heap_start) / PAGE_SIZE
}

/// Verify-local block metadata and free list operations.
pub struct BlockMeta {
    pub order: u8,
    pub free: u8,
    pub prev: u16,
    pub next: u16,
}

impl BlockMeta {
    pub fn new() -> Self {
        Self { order: 0, free: 0, prev: 0, next: 0 }
    }
}

pub const EMPTY_LIST: usize = 4096 + 1;

pub struct MetaArray {
    data: Vec<BlockMeta>,
}

impl MetaArray {
    pub fn new(capacity: usize) -> Self {
        let mut v = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            v.push(BlockMeta::new());
        }
        Self { data: v }
    }

    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    pub fn get(&self, idx: usize) -> &BlockMeta {
        &self.data[idx]
    }

    pub fn get_mut(&mut self, idx: usize) -> &mut BlockMeta {
        &mut self.data[idx]
    }
}

pub struct BuddyAllocator {
    free_lists: Vec<usize>,
    meta: MetaArray,
    heap_start: usize,
}

impl BuddyAllocator {
    pub fn new(capacity_pages: usize) -> Self {
        let mut free_lists = Vec::with_capacity(MAX_ORDER + 1);
        for _ in 0..=MAX_ORDER {
            free_lists.push(EMPTY_LIST);
        }
        Self {
            free_lists,
            meta: MetaArray::new(capacity_pages),
            heap_start: 0x80A00000,
        }
    }

    pub fn init(&mut self, heap_size: usize) {
        let max_order = heap_size_to_order(heap_size.min(self.meta.data.len() * PAGE_SIZE));
        for i in 0..=MAX_ORDER {
            self.free_lists[i] = EMPTY_LIST;
        }
        self.init_block(0, max_order, true);
        self.add_to_free_list(0, max_order);
    }

    pub fn init_block(&mut self, page_idx: usize, order: usize, free: bool) {
        let meta = self.meta.get_mut(page_idx);
        meta.order = order as u8;
        meta.free = if free { 1 } else { 0 };
        meta.prev = 0;
        meta.next = 0;
    }

    pub fn add_to_free_list(&mut self, page_idx: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }
        {
            let meta = self.meta.get_mut(page_idx);
            meta.order = order as u8;
            meta.free = 1;
        }
        let list_head = self.free_lists[order];
        if list_head != EMPTY_LIST && list_head < self.meta.capacity() {
            self.meta.get_mut(list_head).prev = page_idx as u16;
        }
        {
            let meta = self.meta.get_mut(page_idx);
            meta.next = if list_head == EMPTY_LIST { 0xFFFF } else { list_head as u16 };
            meta.prev = 0xFFFF;
        }
        self.free_lists[order] = page_idx;
    }

    pub fn remove_from_free_list(&mut self, page_idx: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }
        let prev_idx = self.meta.get(page_idx).prev as usize;
        let next_idx = self.meta.get(page_idx).next as usize;
        if prev_idx != 0xFFFF && prev_idx < self.meta.capacity() {
            self.meta.get_mut(prev_idx).next = next_idx as u16;
        } else {
            let new_head = if next_idx == 0xFFFF { EMPTY_LIST } else { next_idx };
            self.free_lists[order] = new_head;
        }
        if next_idx != 0xFFFF && next_idx < self.meta.capacity() {
            self.meta.get_mut(next_idx).prev = prev_idx as u16;
        }
        self.meta.get_mut(page_idx).free = 0;
    }

    pub fn alloc_blocks(&mut self, order: usize) -> Option<usize> {
        for mut current_order in order..=MAX_ORDER {
            let list_head = self.free_lists[current_order];
            if list_head != EMPTY_LIST && list_head < self.meta.capacity() {
                self.remove_from_free_list(list_head, current_order);
                let mut page_idx = list_head;
                while current_order > order {
                    let block_size_pages = 1usize << current_order;
                    let buddy_idx = page_idx + (block_size_pages / 2);
                    self.init_block(buddy_idx, current_order - 1, true);
                    self.add_to_free_list(buddy_idx, current_order - 1);
                    self.init_block(page_idx, current_order - 1, false);
                    current_order -= 1;
                }
                self.init_block(page_idx, order, false);
                return Some(page_idx);
            }
        }
        None
    }

    pub fn free_blocks(&mut self, page_idx: usize, order: usize) {
        let mut page_idx = page_idx;
        let mut current_order = order;
        loop {
            if current_order > MAX_ORDER {
                self.add_to_free_list(page_idx, MAX_ORDER);
                break;
            }
            let buddy_idx = get_buddy_idx(page_idx, current_order);
            if buddy_idx >= self.meta.capacity() {
                self.add_to_free_list(page_idx, current_order);
                break;
            }
            let buddy_meta = self.meta.get(buddy_idx);
            if buddy_meta.free == 0 || buddy_meta.order != current_order as u8 {
                self.add_to_free_list(page_idx, current_order);
                break;
            }
            self.remove_from_free_list(buddy_idx, current_order);
            if page_idx > buddy_idx {
                page_idx = buddy_idx;
            }
            current_order += 1;
        }
    }

    pub fn free_list_count(&self, order: usize) -> usize {
        let mut count = 0usize;
        let mut idx = self.free_lists[order];
        let cap = self.meta.capacity();
        while idx != EMPTY_LIST && idx < cap {
            count += 1;
            let next = self.meta.get(idx).next as usize;
            if next == 0xFFFF { break; }
            idx = next;
        }
        count
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-BUDDY-1: size_to_order(1..4096) == 0
    #[test]
    fn test_size_to_order_small(size in 1usize..4096usize) {
        prop_assert_eq!(size_to_order(size), 0);
    }

    /// INV-BUDDY-2: size_to_order(4096) == 0 (1 page, returns 0 for pages <= 1)
    #[test]
    fn test_size_to_order_one_page(_v in 0u8..1u8) {
        prop_assert_eq!(size_to_order(4096), 0);
    }

    /// INV-BUDDY-3: size_to_order(2^order * 4096) == order
    #[test]
    fn test_size_to_order_exact(order in 0usize..10usize) {
        let size = (1usize << order) * PAGE_SIZE;
        prop_assert_eq!(size_to_order(size), order);
    }

    /// INV-BUDDY-4: size_to_order is monotonically non-decreasing
    #[test]
    fn test_size_to_order_monotone(
        s1 in 1usize..100_000usize,
        s2 in 1usize..100_000usize,
    ) {
        let (small, large) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
        prop_assert!(size_to_order(small) <= size_to_order(large));
    }

    /// INV-BUDDY-5: heap_size_to_order matches size_to_order
    #[test]
    fn test_heap_size_to_order(size in 4096usize..1_073_741_824usize) {
        prop_assert_eq!(heap_size_to_order(size), size_to_order(size));
    }

    /// INV-BUDDY-6: get_buddy_idx is its own inverse
    #[test]
    fn test_buddy_involution(
        page_idx in 0usize..2048usize,
        order in 1usize..10usize,
    ) {
        let buddy = get_buddy_idx(page_idx, order);
        prop_assert_eq!(get_buddy_idx(buddy, order), page_idx);
    }

    /// INV-BUDDY-7: buddy differs at order bit
    #[test]
    fn test_buddy_differs_at_order_bit(
        page_idx in 0usize..2048usize,
        order in 1usize..10usize,
    ) {
        let buddy = get_buddy_idx(page_idx, order);
        let bit = 1usize << order;
        prop_assert_eq!(page_idx ^ bit, buddy);
    }

    /// INV-BUDDY-8: page_idx_to_addr and addr_to_page_idx are inverse
    #[test]
    fn test_addr_roundtrip(
        page_idx in 0usize..4096usize,
    ) {
        let addr = page_idx_to_addr(0x80A00000, page_idx);
        prop_assert_eq!(addr_to_page_idx(0x80A00000, addr), page_idx);
    }

    /// INV-BUDDY-9: alloc + free conserves all pages (total free == capacity)
    #[test]
    fn test_alloc_free_roundtrip(
        order in 0usize..6usize,
    ) {
        let capacity = 1024;
        let mut alloc = BuddyAllocator::new(capacity);
        alloc.init(capacity * PAGE_SIZE);

        let page_idx = alloc.alloc_blocks(order);
        prop_assert!(page_idx.is_some());

        let page_idx = page_idx.unwrap();
        // Page should be at the right alignment
        let block_size_pages = 1usize << order;
        prop_assert_eq!(page_idx % block_size_pages, 0);

        // Free and verify total free pages == capacity
        alloc.free_blocks(page_idx, order);

        let mut total_free: usize = 0;
        for o in 0..=MAX_ORDER {
            total_free += alloc.free_list_count(o) * (1usize << o);
        }
        prop_assert_eq!(total_free, capacity);
    }

    /// INV-BUDDY-10: buddy merging on sequential alloc+free
    #[test]
    fn test_buddy_merging(_v in 0u8..1u8) {
        let capacity = 1024;
        let mut alloc = BuddyAllocator::new(capacity);
        alloc.init(capacity * PAGE_SIZE);

        // Allocate 2 blocks of same order
        let p1 = alloc.alloc_blocks(1).unwrap();
        let p2 = alloc.alloc_blocks(1).unwrap();

        // They should be buddies
        let buddy = get_buddy_idx(p1, 1);
        prop_assert_eq!(buddy, p2);

        // Free both — buddies should merge; total free pages == capacity
        alloc.free_blocks(p1, 1);
        alloc.free_blocks(p2, 1);

        let mut total_free: usize = 0;
        for o in 0..=MAX_ORDER {
            total_free += alloc.free_list_count(o) * (1usize << o);
        }
        prop_assert_eq!(total_free, capacity);
    }

    /// INV-BUDDY-11: total pages conserved across alloc/free
    #[test]
    fn test_total_pages_conserved(
        ops in proptest::collection::vec(
            (0usize..6usize, proptest::bool::ANY),
            0..20
        ),
    ) {
        let capacity = 1024;
        let mut alloc = BuddyAllocator::new(capacity);
        alloc.init(capacity * PAGE_SIZE);

        let mut allocated: Vec<(usize, usize)> = Vec::new();
        for (order, do_alloc) in ops {
            if do_alloc {
                if let Some(page_idx) = alloc.alloc_blocks(order) {
                    allocated.push((page_idx, order));
                }
            } else if let Some((page_idx, order)) = allocated.pop() {
                alloc.free_blocks(page_idx, order);
            }
        }

        // Count all free pages across all orders
        let mut total_free: usize = 0;
        for order in 0..=MAX_ORDER {
            total_free += alloc.free_list_count(order) * (1usize << order);
        }

        let total_allocated: usize = allocated.iter().map(|(_, o)| 1usize << o).sum();
        // free + allocated should equal total capacity minus initial used
        // (the initial block is one large block; after alloc/free the math works)
        prop_assert_eq!(total_free + total_allocated, capacity);
    }
}
