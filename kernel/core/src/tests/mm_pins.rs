//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! MM + cmdline pins — ported from the dead #[cfg(test)] blocks in
//! kernel/core/src/mm/{slab,vma,page_desc}.rs and kernel/core/src/cmdline.rs per
//! the test-layout policy (host-side test blocks never compile in this
//! no_std crate). Formerly mm_sched_pins.rs: the sched half (the fair
//! LoadWeight/SchedEntity/CfsRunQueue pins) moved to the sched
//! crate with the S1b flip and runs via the SERVICE-TEST SEAM.

use crate::cmdline::{get_all_params, get_init_program, get_root_device, has_param, is_root_readonly, CMDLINE_LEN, CMDLINE_PTR};
use crate::mm::page_desc::{Page, PageFlag, PageFlags};
use crate::mm::slab::SlabAllocator;
use crate::mm::vma::{VirtAddr, Vma, VmaFlags, VmaOrigin};
use core::sync::atomic::Ordering;

pub fn test_mm_pins() {
    crate::tests::test_group_start("mm_pins");
    test_slab_pins();
    test_vma_pins();
    test_page_desc_pins();
    test_cmdline_pins();
}

// ---- mm/slab.rs ----------------------------------------------------------

fn test_slab_pins() {
    crate::test_assert_eq!(SlabAllocator::find_cache_index(1), Some(0), "1 byte -> class 0");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(8), Some(0), "8 bytes -> class 0");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(9), Some(1), "9 bytes -> class 1");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(16), Some(1), "16 bytes -> class 1");
    // The dead block said class 6; the actual size table is
    // [8,16,32,64,128,256,512,1024,2048,4096], so 100 bytes lands in the
    // 128-byte class (index 4).
    crate::test_assert_eq!(SlabAllocator::find_cache_index(100), Some(4), "100 bytes -> class 4 (128B class)");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(4096), Some(9), "4096 bytes -> class 9");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(4097), None, "4097 bytes -> over max");
    crate::test_assert_eq!(SlabAllocator::find_cache_index(0), None, "0 bytes -> no class");
    crate::tests::test_pass("mm-slab-classes");
}

// ---- mm/vma.rs ------------------------------------------------------------

fn test_vma_pins() {
    let start = VirtAddr::new(0x1000);
    let end = VirtAddr::new(0x2000);
    let flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE);
    let vma = Vma::new(start, end, flags);
    crate::test_assert_eq!(vma.start(), start, "vma start");
    crate::test_assert_eq!(vma.end(), end, "vma end");
    crate::test_assert_eq!(vma.size(), 0x1000, "vma size");
    crate::test_assert_eq!(vma.page_count(), 1, "vma page count");

    let vma = Vma::new(VirtAddr::new(0x1000), VirtAddr::new(0x3000), VmaFlags::new());
    crate::test_assert!(vma.contains(VirtAddr::new(0x1000)), "contains start");
    crate::test_assert!(vma.contains(VirtAddr::new(0x2000)), "contains middle");
    crate::test_assert!(!vma.contains(VirtAddr::new(0x3000)), "excludes end");
    crate::test_assert!(!vma.contains(VirtAddr::new(0xfff)), "excludes below");

    // Creator stamp + fork modifier survive splits; set_creator clears a
    // previous creator while keeping the fork bit.
    let mut vma = Vma::new(VirtAddr::new(0x1000), VirtAddr::new(0x4000), VmaFlags::new());
    crate::test_assert_eq!(vma.origin().creator_name(), "unknown", "fresh origin");
    vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
    let (head, tail) = vma.split(VirtAddr::new(0x2000)).unwrap();
    crate::test_assert_eq!(head.origin().creator_name(), "mmap", "head origin after split");
    crate::test_assert_eq!(tail.origin().creator_name(), "mmap", "tail origin after split");
    let mut o = tail.origin();
    o.mark_forked();
    crate::test_assert!(o.is_forked(), "forked bit survives split");
    crate::test_assert_eq!(o.creator_name(), "mmap", "creator survives fork mark");
    let mut o2 = VmaOrigin::from_bits(VmaOrigin::MPROTECT | VmaOrigin::FORKED);
    crate::test_assert_eq!(o2.creator_name(), "mprotect", "mprotect creator name");
    crate::test_assert!(o2.is_forked(), "mprotect forked");
    o2.set_creator(VmaOrigin::MMAP);
    crate::test_assert_eq!(o2.creator_name(), "mmap", "set_creator overrides");
    crate::test_assert!(o2.is_forked(), "set_creator keeps forked");
    crate::tests::test_pass("mm-vma");
}

// ---- mm/page_desc.rs --------------------------------------------------------

fn test_page_desc_pins() {
    let mut flags = PageFlags::new();
    crate::test_assert!(!flags.test(PageFlag::Locked), "fresh not locked");
    crate::test_assert!(!flags.test(PageFlag::Dirty), "fresh not dirty");
    flags.set(PageFlag::Locked);
    crate::test_assert!(flags.test(PageFlag::Locked), "set locked");
    flags.set(PageFlag::Dirty);
    crate::test_assert!(flags.test(PageFlag::Dirty), "set dirty");
    flags.clear(PageFlag::Locked);
    crate::test_assert!(!flags.test(PageFlag::Locked) && flags.test(PageFlag::Dirty), "clear locked keeps dirty");

    let page = Page::new();
    crate::test_assert_eq!(page.refcount(), 0, "fresh refcount 0");
    page.get_page();
    crate::test_assert_eq!(page.refcount(), 1, "get -> 1");
    page.get_page();
    crate::test_assert_eq!(page.refcount(), 2, "get -> 2");
    page.put_page();
    crate::test_assert_eq!(page.refcount(), 1, "put -> 1");
    page.put_page();
    crate::test_assert_eq!(page.refcount(), 0, "put -> 0");

    let page = Page::new();
    crate::test_assert_eq!(page.mapcount(), -1, "fresh mapcount -1 (unmapped)");
    crate::test_assert!(!page.is_mapped(), "fresh unmapped");
    page.add_mapcount();
    crate::test_assert_eq!(page.mapcount(), 0, "add -> 0");
    crate::test_assert!(page.is_mapped(), "mapped at 0");
    page.add_mapcount();
    crate::test_assert_eq!(page.mapcount(), 1, "add -> 1");
    page.sub_mapcount();
    crate::test_assert_eq!(page.mapcount(), 0, "sub -> 0");
    crate::tests::test_pass("mm-page-desc");
}

// ---- cmdline.rs -----------------------------------------------------------------

/// Point the global cmdline at a static string (the suite runs at boot end,
/// after the last real parse; the kernel halts right after the summary).
fn set_test_cmdline(cmdline: &'static str) {
    let ptr = cmdline.as_ptr() as *mut u8;
    CMDLINE_PTR.store(ptr, Ordering::SeqCst);
    CMDLINE_LEN.store(cmdline.len(), Ordering::SeqCst);
}

fn test_cmdline_pins() {
    set_test_cmdline("root=/dev/vda rw console=ttyS0");
    crate::test_assert_eq!(get_root_device(), "/dev/vda", "root= parse");
    crate::test_assert!(!is_root_readonly(), "rw default");

    set_test_cmdline("init=/sbin/init root=/dev/ram0");
    crate::test_assert_eq!(get_init_program(), "/sbin/init", "init= parse");

    set_test_cmdline("debug quiet root=/dev/ram0");
    crate::test_assert!(has_param("debug"), "has debug");
    crate::test_assert!(has_param("quiet"), "has quiet");
    crate::test_assert!(!has_param("ro"), "no ro");

    set_test_cmdline("root=/dev/vda init=/hello_world debug");
    let params = get_all_params();
    crate::test_assert_eq!(params.len(), 2, "two key=value params");
    crate::test_assert_eq!(params[0], (alloc::string::String::from("root"), alloc::string::String::from("/dev/vda")), "param 0");
    crate::test_assert_eq!(params[1], (alloc::string::String::from("init"), alloc::string::String::from("/hello_world")), "param 1");
    crate::tests::test_pass("cmdline");
}
