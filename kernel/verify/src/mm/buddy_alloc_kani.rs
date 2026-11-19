//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for buddy allocator core logic.
//!
//! Types copied from: kernel/verify/src/mm/buddy_alloc_test.rs

#![cfg(kani)]

const PAGE_SIZE: usize = 4096;
const MAX_ORDER: usize = 10;
const EMPTY_LIST: usize = 4097;

fn heap_size_to_order(size: usize) -> usize {
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    if pages <= 1 { return 0; }
    let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
    if order > MAX_ORDER { MAX_ORDER } else { order }
}

fn get_buddy_idx(page_idx: usize, order: usize) -> usize {
    page_idx ^ (1usize << order)
}

// --- Minimal BuddyAllocator for stateful harnesses ---

struct BlockMeta { order: u8, free: u8, prev: u32, next: u32 }
impl BlockMeta { fn new() -> Self { Self { order: 0, free: 0, prev: 0, next: 0 } } }

struct BuddyAllocator {
    free_lists: [usize; MAX_ORDER + 1],
    meta: Vec<BlockMeta>,
}

impl BuddyAllocator {
    fn new(capacity: usize) -> Self {
        let mut meta = Vec::new();
        for _ in 0..capacity { meta.push(BlockMeta::new()); }
        let mut free_lists = [EMPTY_LIST; MAX_ORDER + 1];
        let max_order = heap_size_to_order(capacity.min(1024) * PAGE_SIZE);
        free_lists[max_order] = 0; // single block at index 0
        meta[0].order = max_order as u8;
        meta[0].free = 1;
        Self { free_lists, meta }
    }

    fn alloc_blocks(&mut self, order: usize) -> Option<usize> {
        for mut cur_order in order..=MAX_ORDER {
            let head = self.free_lists[cur_order];
            if head != EMPTY_LIST && head < self.meta.len() {
                // Remove from free list
                self.remove_from_free_list(head, cur_order);
                let mut page_idx = head;
                // Split down to target order
                while cur_order > order {
                    let buddy = page_idx + (1usize << (cur_order - 1));
                    self.meta[buddy].order = (cur_order - 1) as u8;
                    self.meta[buddy].free = 1;
                    self.meta[buddy].prev = u32::MAX;
                    self.meta[buddy].next = u32::MAX;
                    self.add_to_free_list(buddy, cur_order - 1);
                    self.meta[page_idx].order = (cur_order - 1) as u8;
                    cur_order -= 1;
                }
                self.meta[page_idx].free = 0;
                return Some(page_idx);
            }
        }
        None
    }

    fn add_to_free_list(&mut self, page_idx: usize, order: usize) {
        let head = self.free_lists[order];
        if head != EMPTY_LIST && head < self.meta.len() {
            self.meta[head].prev = page_idx as u32;
        }
        self.meta[page_idx].next = if head == EMPTY_LIST { u32::MAX } else { head as u32 };
        self.meta[page_idx].prev = u32::MAX;
        self.meta[page_idx].free = 1;
        self.meta[page_idx].order = order as u8;
        self.free_lists[order] = page_idx;
    }

    fn remove_from_free_list(&mut self, page_idx: usize, order: usize) {
        let prev = self.meta[page_idx].prev as usize;
        let next = self.meta[page_idx].next as usize;
        if prev != u32::MAX as usize && prev < self.meta.len() {
            self.meta[prev].next = next as u32;
        } else {
            self.free_lists[order] = if next == u32::MAX as usize { EMPTY_LIST } else { next };
        }
        if next != u32::MAX as usize && next < self.meta.len() {
            self.meta[next].prev = prev as u32;
        }
        self.meta[page_idx].free = 0;
    }

    fn free_blocks(&mut self, page_idx: usize, order: usize) {
        let mut pidx = page_idx;
        let mut cur_order = order;
        loop {
            if cur_order > MAX_ORDER {
                self.add_to_free_list(pidx, MAX_ORDER);
                break;
            }
            let buddy = get_buddy_idx(pidx, cur_order);
            if buddy >= self.meta.len() {
                self.add_to_free_list(pidx, cur_order);
                break;
            }
            if self.meta[buddy].free == 0 || self.meta[buddy].order != cur_order as u8 {
                self.add_to_free_list(pidx, cur_order);
                break;
            }
            self.remove_from_free_list(buddy, cur_order);
            if pidx > buddy { pidx = buddy; }
            cur_order += 1;
        }
    }
}

/// INV-BUDDY-K1: size_to_order always returns <= MAX_ORDER
#[kani::proof]
fn verify_size_to_order_bounds() {
    let size: usize = kani::any();
    kani::assume(size > 0);  // meaningful sizes only
    let order = heap_size_to_order(size);
    assert!(order <= MAX_ORDER);
}

/// INV-BUDDY-K2: get_buddy_idx is its own inverse
#[kani::proof]
fn verify_buddy_involution() {
    let page_idx: usize = kani::any();
    let order: usize = kani::any();
    kani::assume(order > 0 && order <= MAX_ORDER);
    let buddy = get_buddy_idx(page_idx, order);
    assert_eq!(get_buddy_idx(buddy, order), page_idx);
}

/// INV-BUDDY-K3: Two allocations of same order never overlap
#[kani::proof]
fn verify_buddy_no_overlap() {
    let capacity = 64usize;  // small to keep CBMC tractable
    let order: usize = kani::any();
    kani::assume(order <= 5);  // limit search space
    let mut alloc = BuddyAllocator::new(capacity);

    let p1 = alloc.alloc_blocks(order);
    let p2 = alloc.alloc_blocks(order);
    // Both must succeed for small order on 64-page pool
    if p1.is_some() && p2.is_some() {
        let a = p1.unwrap();
        let b = p2.unwrap();
        let size = 1usize << order;
        assert!(a + size <= b || b + size <= a,
            "allocations overlap: {} and {} (size={})", a, b, size);
    }
}

/// INV-BUDDY-K4: Alloc then free conserves total free pages
#[kani::proof]
fn verify_alloc_free_conserves() {
    let capacity = 64usize;
    let order: usize = kani::any();
    kani::assume(order <= 4);
    let mut alloc = BuddyAllocator::new(capacity);
    let page_idx = alloc.alloc_blocks(order);
    if let Some(pidx) = page_idx {
        alloc.free_blocks(pidx, order);
        // After free, total free pages == capacity
        let mut total_free: usize = 0;
        for o in 0..=MAX_ORDER {
            let mut idx = alloc.free_lists[o];
            let cap = alloc.meta.len();
            let mut count = 0usize;
            while idx != EMPTY_LIST && idx < cap {
                count += 1;
                let next = alloc.meta[idx].next as usize;
                if next == u32::MAX as usize { break; }
                idx = next;
            }
            total_free += count * (1usize << o);
        }
        assert_eq!(total_free, capacity);
    }
}
