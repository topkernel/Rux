//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Page allocator
use crate::println;
use crate::mm::page::{PhysAddr, VirtAddr, PhysFrame, VirtPage, FrameAllocator};
use super::{test_pass, test_fail, test_group_start};

pub fn test_page_allocator() {
    test_group_start("page allocator");

    // Test 1: PhysAddr basic operations
    let addr1 = PhysAddr::new(0x1000);
    let addr2 = PhysAddr::new(0x1234);
    if addr1.as_usize() == 0x1000 && addr1.is_aligned()
        && addr2.as_usize() == 0x1000 && addr2.is_aligned() {
        test_pass("PhysAddr operations");
    } else {
        test_fail("PhysAddr operations", "address mismatch");
    }

    // Test 2: PhysAddr floor and ceil
    let addr = PhysAddr::new(0x1000);
    let floor = addr.floor();
    let ceil = addr.ceil();
    if floor.as_usize() == 0x1000 && ceil.as_usize() == 0x1000 {
        test_pass("PhysAddr floor/ceil");
    } else {
        test_fail("PhysAddr floor/ceil", "mismatch");
    }

    // Test 3: PhysAddr frame_number
    let addr = PhysAddr::new(0x5000);
    if addr.frame_number() == 5 {
        test_pass("PhysAddr frame_number");
    } else {
        test_fail("PhysAddr frame_number", "expected 5");
    }

    // Test 4: VirtAddr basic operations
    let vaddr1 = VirtAddr::new(0x1000);
    let vaddr2 = VirtAddr::new(0x5678);
    if vaddr1.as_usize() == 0x1000 && vaddr1.is_aligned()
        && vaddr2.as_usize() == 0x5000 {
        test_pass("VirtAddr operations");
    } else {
        test_fail("VirtAddr operations", "address mismatch");
    }

    // Test 5: VirtAddr floor and ceil
    let vaddr = VirtAddr::new(0x5000);
    let vfloor = vaddr.floor();
    let vceil = vaddr.ceil();
    if vfloor.as_usize() == 0x5000 && vceil.as_usize() == 0x5000 {
        test_pass("VirtAddr floor/ceil");
    } else {
        test_fail("VirtAddr floor/ceil", "mismatch");
    }

    // Test 6: VirtAddr page_number
    let vaddr = VirtAddr::new(0x7000);
    if vaddr.page_number() == 7 {
        test_pass("VirtAddr page_number");
    } else {
        test_fail("VirtAddr page_number", "expected 7");
    }

    // Test 7: PhysFrame basic operations
    let frame = PhysFrame::new(10);
    let start = frame.start_address();
    if frame.number == 10 && start.as_usize() == 0xA000 {
        test_pass("PhysFrame operations");
    } else {
        test_fail("PhysFrame operations", "mismatch");
    }

    // Test 8: PhysFrame containing_address
    let addr = PhysAddr::new(0x5234);
    let frame = PhysFrame::containing_address(addr);
    if frame.number == 5 {
        test_pass("PhysFrame containing_address");
    } else {
        test_fail("PhysFrame containing_address", "expected 5");
    }

    // Test 9: PhysFrame range
    let frame = PhysFrame::new(3);
    let range = frame.range();
    if range.start.as_usize() == 0x3000 && range.end.as_usize() == 0x4000 {
        test_pass("PhysFrame range");
    } else {
        test_fail("PhysFrame range", "range mismatch");
    }

    // Test 10: VirtPage basic operations
    let vpage = VirtPage::new(8);
    let vstart = vpage.start_address();
    if vpage.number == 8 && vstart.as_usize() == 0x8000 {
        test_pass("VirtPage operations");
    } else {
        test_fail("VirtPage operations", "mismatch");
    }

    // Test 11: VirtPage containing_address
    let vaddr = VirtAddr::new(0x9ABC);
    let vpage = VirtPage::containing_address(vaddr);
    if vpage.number == 9 {
        test_pass("VirtPage containing_address");
    } else {
        test_fail("VirtPage containing_address", "expected 9");
    }

    // Test 12: VirtPage range
    let vpage = VirtPage::new(12);
    let vrange = vpage.range();
    if vrange.start.as_usize() == 0xC000 && vrange.end.as_usize() == 0xD000 {
        test_pass("VirtPage range");
    } else {
        test_fail("VirtPage range", "range mismatch");
    }

    // Test 13: FrameAllocator basic operations
    let allocator = FrameAllocator::new(100);
    allocator.init(0);

    let frame0 = allocator.allocate();
    let frame1 = allocator.allocate();
    if frame0.is_some() && frame0.unwrap().number == 0
        && frame1.is_some() && frame1.unwrap().number == 1 {
        test_pass("FrameAllocator allocation");
    } else {
        test_fail("FrameAllocator allocation", "allocation failed");
        return;
    }

    // Test 14: FrameAllocator exhaustion
    let small_allocator = FrameAllocator::new(5);
    small_allocator.init(0);
    let mut all_allocated = true;
    for i in 0..5 {
        match small_allocator.allocate() {
            Some(frame) if frame.number == i => {}
            _ => { all_allocated = false; break; }
        }
    }
    let exhausted = small_allocator.allocate().is_none();
    if all_allocated && exhausted {
        test_pass("FrameAllocator exhaustion");
    } else {
        test_fail("FrameAllocator exhaustion", "unexpected behavior");
    }

    // Test 15: FrameAllocator deallocate
    let test_allocator = FrameAllocator::new(10);
    test_allocator.init(0);
    if let Some(frame) = test_allocator.allocate() {
        test_allocator.deallocate(frame);
        test_pass("FrameAllocator deallocate");
    } else {
        test_fail("FrameAllocator deallocate", "allocate failed");
    }

    println!("test: Page allocator testing completed.");
}
