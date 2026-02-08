// 测试：页分配器
use crate::println;
use crate::mm::page::{PhysAddr, VirtAddr, PhysFrame, VirtPage, FrameAllocator, PAGE_SIZE};

pub fn test_page_allocator() {
    println!("test: Testing page allocator...");

    // 测试 1: PhysAddr 基本操作
    println!("test: 1. Testing PhysAddr operations...");
    let addr1 = PhysAddr::new(0x1000);
    assert_eq!(addr1.as_usize(), 0x1000, "Address should be 0x1000");
    assert!(addr1.is_aligned(), "0x1000 should be aligned");

    let addr2 = PhysAddr::new(0x1234);
    assert_eq!(addr2.as_usize(), 0x1000, "Address should be aligned to 0x1000");
    assert!(addr2.is_aligned(), "Aligned address should be aligned");
    // Note: PhysAddr::new() aligns addresses, so 0x1234 becomes 0x1000

    println!("test:    SUCCESS - PhysAddr operations work");

    // 测试 2: PhysAddr floor 和 ceil
    println!("test: 2. Testing PhysAddr floor and ceil...");
    // Note: PhysAddr::new() aligns addresses, so we test with aligned addresses
    let addr = PhysAddr::new(0x1000);
    let floor = addr.floor();
    assert_eq!(floor.as_usize(), 0x1000, "Floor of aligned addr should be same");

    let ceil = addr.ceil();
    assert_eq!(ceil.as_usize(), 0x1000, "Ceil of aligned addr should be same");
    println!("test:    SUCCESS - floor and ceil work");

    // 测试 3: PhysAddr frame_number
    println!("test: 3. Testing PhysAddr frame_number...");
    let addr = PhysAddr::new(0x5000);
    assert_eq!(addr.frame_number(), 5, "Frame number should be 5");
    println!("test:    SUCCESS - frame_number works");

    // 测试 4: VirtAddr 基本操作
    println!("test: 4. Testing VirtAddr operations...");
    let vaddr1 = VirtAddr::new(0x1000);
    assert_eq!(vaddr1.as_usize(), 0x1000, "Virtual address should be 0x1000");
    assert!(vaddr1.is_aligned(), "0x1000 should be aligned");

    let vaddr2 = VirtAddr::new(0x5678);
    assert_eq!(vaddr2.as_usize(), 0x5000, "Address should be aligned to 0x5000");
    println!("test:    SUCCESS - VirtAddr operations work");

    // 测试 5: VirtAddr floor 和 ceil
    println!("test: 5. Testing VirtAddr floor and ceil...");
    // Note: VirtAddr::new() aligns addresses, so we test with aligned addresses
    let vaddr = VirtAddr::new(0x5000);
    let vfloor = vaddr.floor();
    assert_eq!(vfloor.as_usize(), 0x5000, "Floor of aligned addr should be same");

    let vceil = vaddr.ceil();
    assert_eq!(vceil.as_usize(), 0x5000, "Ceil of aligned addr should be same");
    println!("test:    SUCCESS - VirtAddr floor and ceil work");

    // 测试 6: VirtAddr page_number
    println!("test: 6. Testing VirtAddr page_number...");
    let vaddr = VirtAddr::new(0x7000);
    assert_eq!(vaddr.page_number(), 7, "Page number should be 7");
    println!("test:    SUCCESS - page_number works");

    // 测试 7: PhysFrame 基本操作
    println!("test: 7. Testing PhysFrame operations...");
    let frame = PhysFrame::new(10);
    assert_eq!(frame.number, 10, "Frame number should be 10");

    let start = frame.start_address();
    assert_eq!(start.as_usize(), 0xA000, "Start address should be 0xA000");
    println!("test:    SUCCESS - PhysFrame operations work");

    // 测试 8: PhysFrame containing_address
    println!("test: 8. Testing PhysFrame containing_address...");
    let addr = PhysAddr::new(0x5234);
    let frame = PhysFrame::containing_address(addr);
    assert_eq!(frame.number, 5, "Frame should be number 5");
    println!("test:    SUCCESS - containing_address works");

    // 测试 9: PhysFrame range
    println!("test: 9. Testing PhysFrame range...");
    let frame = PhysFrame::new(3);
    let range = frame.range();
    assert_eq!(range.start.as_usize(), 0x3000, "Range start should be 0x3000");
    assert_eq!(range.end.as_usize(), 0x4000, "Range end should be 0x4000");
    println!("test:    SUCCESS - PhysFrame range works");

    // 测试 10: VirtPage 基本操作
    println!("test: 10. Testing VirtPage operations...");
    let vpage = VirtPage::new(8);
    assert_eq!(vpage.number, 8, "Page number should be 8");

    let vstart = vpage.start_address();
    assert_eq!(vstart.as_usize(), 0x8000, "Start address should be 0x8000");
    println!("test:    SUCCESS - VirtPage operations work");

    // 测试 11: VirtPage containing_address
    println!("test: 11. Testing VirtPage containing_address...");
    let vaddr = VirtAddr::new(0x9ABC);
    let vpage = VirtPage::containing_address(vaddr);
    assert_eq!(vpage.number, 9, "Page should be number 9");
    println!("test:    SUCCESS - VirtPage containing_address works");

    // 测试 12: VirtPage range
    println!("test: 12. Testing VirtPage range...");
    let vpage = VirtPage::new(12);
    let vrange = vpage.range();
    assert_eq!(vrange.start.as_usize(), 0xC000, "Range start should be 0xC000");
    assert_eq!(vrange.end.as_usize(), 0xD000, "Range end should be 0xD000");
    println!("test:    SUCCESS - VirtPage range works");

    // 测试 13: FrameAllocator 基本操作
    println!("test: 13. Testing FrameAllocator operations...");
    let allocator = FrameAllocator::new(100);

    // 初始化到起始帧
    allocator.init(0);

    // 分配第一帧
    match allocator.allocate() {
        Some(frame) => {
            assert_eq!(frame.number, 0, "First allocated frame should be 0");
            println!("test:    Allocated frame 0");
        }
        None => {
            println!("test:    FAILED - allocate returned None");
            return;
        }
    }

    // 分配第二帧
    match allocator.allocate() {
        Some(frame) => {
            assert_eq!(frame.number, 1, "Second allocated frame should be 1");
            println!("test:    Allocated frame 1");
        }
        None => {
            println!("test:    FAILED - allocate returned None");
            return;
        }
    }

    // 分配多帧并验证递增
    let mut last_frame = 0;
    for i in 2..10 {
        match allocator.allocate() {
            Some(frame) => {
                assert_eq!(frame.number, i, "Frame {} should be allocated", i);
                last_frame = frame.number;
            }
            None => {
                println!("test:    FAILED - allocate returned None for frame {}", i);
                return;
            }
        }
    }
    assert_eq!(last_frame, 9, "Should have allocated up to frame 9");
    println!("test:    SUCCESS - FrameAllocator allocation works");

    // 测试 14: FrameAllocator 耗尽
    println!("test: 14. Testing FrameAllocator exhaustion...");
    let small_allocator = FrameAllocator::new(5);
    small_allocator.init(0);

    // 分配所有帧
    for i in 0..5 {
        match small_allocator.allocate() {
            Some(frame) => assert_eq!(frame.number, i, "Should allocate frame {}", i),
            None => {
                println!("test:    FAILED - premature exhaustion at frame {}", i);
                return;
            }
        }
    }

    // 尝试分配超出限制的帧
    match small_allocator.allocate() {
        Some(_) => {
            println!("test:    FAILED - should return None when exhausted");
            return;
        }
        None => {
            println!("test:    Correctly returned None when exhausted");
        }
    }
    println!("test:    SUCCESS - FrameAllocator exhaustion handling works");

    // 测试 15: FrameAllocator deallocate (no-op in simple implementation)
    println!("test: 15. Testing FrameAllocator deallocate...");
    let test_allocator = FrameAllocator::new(10);
    test_allocator.init(0);

    let frame = match test_allocator.allocate() {
        Some(f) => f,
        None => {
            println!("test:    FAILED - allocate returned None");
            return;
        }
    };

    // deallocate 应该不会 panic（即使是 no-op）
    test_allocator.deallocate(frame);
    println!("test:    SUCCESS - deallocate does not panic");

    println!("test: Page allocator testing completed.");
}
