//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Page allocator
use crate::mm::page::{PhysAddr, VirtAddr, PhysFrame, VirtPage, PAGE_SIZE, PAGE_MASK};
use super::{test_pass, test_fail, test_group_start};

pub fn test_page_allocator() {
    test_group_start("page allocator");

    // Test 1: PAGE_SIZE and PAGE_MASK constants
    if PAGE_SIZE == 4096 && PAGE_MASK == 0xFFF {
        test_pass("PAGE_SIZE/PAGE_MASK constants");
    } else {
        test_fail("PAGE_SIZE/PAGE_MASK constants", "unexpected value");
    }

    // Test 2: PhysAddr basic operations
    // Note: PhysAddr::new() floors to page boundary (addr & !PAGE_MASK)
    let addr1 = PhysAddr::new(0x1000);
    let addr2 = PhysAddr::new(0x1234); // floors to 0x1000
    if addr1.as_usize() == 0x1000 && addr1.is_aligned()
        && addr2.as_usize() == 0x1000 && addr2.is_aligned() {
        test_pass("PhysAddr operations");
    } else {
        test_fail("PhysAddr operations", "address mismatch");
    }

    // Test 3: PhysAddr floor and ceil
    let addr = PhysAddr::new(0x1000);
    let floor = addr.floor();
    let ceil = addr.ceil();
    if floor.as_usize() == 0x1000 && ceil.as_usize() == 0x1000 {
        test_pass("PhysAddr floor/ceil");
    } else {
        test_fail("PhysAddr floor/ceil", "mismatch");
    }

    // Test 4: PhysAddr floor/ceil already-aligned (new() floors)
    // PhysAddr::new(0x1234) → 0x1000, so floor==ceil==0x1000
    let addr = PhysAddr::new(0x1234);
    let floor = addr.floor();
    let ceil = addr.ceil();
    if floor.as_usize() == 0x1000 && ceil.as_usize() == 0x1000 {
        test_pass("PhysAddr floor/ceil floored");
    } else {
        test_fail("PhysAddr floor/ceil floored", "mismatch");
    }

    // Test 5: PhysAddr frame_number
    let addr = PhysAddr::new(0x5000);
    if addr.frame_number() == 5 {
        test_pass("PhysAddr frame_number");
    } else {
        test_fail("PhysAddr frame_number", "expected 5");
    }

    // Test 6: VirtAddr basic operations
    // Note: VirtAddr::new() also floors to page boundary
    let vaddr1 = VirtAddr::new(0x1000);
    let vaddr2 = VirtAddr::new(0x5678); // floors to 0x5000
    if vaddr1.as_usize() == 0x1000 && vaddr1.is_aligned()
        && vaddr2.as_usize() == 0x5000 && vaddr2.is_aligned() {
        test_pass("VirtAddr operations");
    } else {
        test_fail("VirtAddr operations", "address mismatch");
    }

    // Test 7: VirtAddr floor and ceil
    let vaddr = VirtAddr::new(0x5000);
    let vfloor = vaddr.floor();
    let vceil = vaddr.ceil();
    if vfloor.as_usize() == 0x5000 && vceil.as_usize() == 0x5000 {
        test_pass("VirtAddr floor/ceil");
    } else {
        test_fail("VirtAddr floor/ceil", "mismatch");
    }

    // Test 8: VirtAddr page_number
    let vaddr = VirtAddr::new(0x7000);
    if vaddr.page_number() == 7 {
        test_pass("VirtAddr page_number");
    } else {
        test_fail("VirtAddr page_number", "expected 7");
    }

    // Test 9: PhysFrame basic operations
    let frame = PhysFrame::new(10);
    let start = frame.start_address();
    if frame.number == 10 && start.as_usize() == 10 * PAGE_SIZE {
        test_pass("PhysFrame operations");
    } else {
        test_fail("PhysFrame operations", "mismatch");
    }

    // Test 10: PhysFrame containing_address
    let addr = PhysAddr::new(0x5234);
    let frame = PhysFrame::containing_address(addr);
    if frame.number == 5 {
        test_pass("PhysFrame containing_address");
    } else {
        test_fail("PhysFrame containing_address", "expected 5");
    }

    // Test 11: PhysFrame range
    let frame = PhysFrame::new(3);
    let range = frame.range();
    if range.start.as_usize() == 3 * PAGE_SIZE && range.end.as_usize() == 4 * PAGE_SIZE {
        test_pass("PhysFrame range");
    } else {
        test_fail("PhysFrame range", "range mismatch");
    }

    // Test 12: VirtPage basic operations
    let vpage = VirtPage::new(8);
    let vstart = vpage.start_address();
    if vpage.number == 8 && vstart.as_usize() == 8 * PAGE_SIZE {
        test_pass("VirtPage operations");
    } else {
        test_fail("VirtPage operations", "mismatch");
    }

    // Test 13: VirtPage containing_address
    // VirtAddr::new(0x9ABC) → 0x9000, so page 9
    let vaddr = VirtAddr::new(0x9ABC);
    let vpage = VirtPage::containing_address(vaddr);
    if vpage.number == 9 {
        test_pass("VirtPage containing_address");
    } else {
        test_fail("VirtPage containing_address", "expected 9");
    }

    // Test 14: VirtPage range
    let vpage = VirtPage::new(12);
    let vrange = vpage.range();
    if vrange.start.as_usize() == 12 * PAGE_SIZE && vrange.end.as_usize() == 13 * PAGE_SIZE {
        test_pass("VirtPage range");
    } else {
        test_fail("VirtPage range", "range mismatch");
    }

    // Test 15: Zero address alignment
    let zero_phys = PhysAddr::new(0);
    let zero_virt = VirtAddr::new(0);
    if zero_phys.is_aligned() && zero_virt.is_aligned() && zero_phys.frame_number() == 0 {
        test_pass("Zero address alignment");
    } else {
        test_fail("Zero address alignment", "zero should be aligned");
    }
}
