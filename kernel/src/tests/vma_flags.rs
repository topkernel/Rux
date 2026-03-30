use crate::mm::vma::VmaFlags;
use super::{test_pass, test_fail, test_group_start};

pub fn test_vma_flags() {
    test_group_start("vma_flags");

    // Test 1: Default is zero
    test_assert_eq!(VmaFlags::new().bits(), 0, "VmaFlags::new().bits() == 0");

    // Test 2: READ flag
    let r = VmaFlags::from_bits(0x01);
    test_assert!(r.is_readable(), "READ is_readable");
    test_assert!(!r.is_writable(), "READ not writable");
    test_assert!(!r.is_executable(), "READ not executable");

    // Test 3: WRITE flag
    let w = VmaFlags::from_bits(0x02);
    test_assert!(w.is_writable(), "WRITE is_writable");
    test_assert!(!w.is_readable(), "WRITE not readable");
    test_assert!(!w.is_executable(), "WRITE not executable");

    // Test 4: EXEC flag
    let x = VmaFlags::from_bits(0x04);
    test_assert!(x.is_executable(), "EXEC is_executable");
    test_assert!(!x.is_readable(), "EXEC not readable");

    // Test 5: SHARED flag
    let s = VmaFlags::from_bits(0x08);
    test_assert!(s.is_shared(), "SHARED is_shared");
    test_assert!(!s.is_private(), "SHARED not private");

    // Test 6: PRIVATE flag
    let p = VmaFlags::from_bits(0x10);
    test_assert!(p.is_private(), "PRIVATE is_private");
    test_assert!(!p.is_shared(), "PRIVATE not shared");

    // Test 7: Combined flags
    let rw = VmaFlags::from_bits(0x01 | 0x02);
    test_assert!(rw.is_readable() && rw.is_writable(), "RW both readable and writable");
    test_assert!(!rw.is_executable(), "RW not executable");

    // Test 8: RWE combined
    let rwx = VmaFlags::from_bits(0x01 | 0x02 | 0x04);
    test_assert!(rwx.is_readable() && rwx.is_writable() && rwx.is_executable(), "RWX all true");

    // Test 9: contains()
    let flags = VmaFlags::from_bits(0x03);
    test_assert!(flags.contains(0x01), "contains(0x01) in 0x03");
    test_assert!(flags.contains(0x02), "contains(0x02) in 0x03");
    test_assert!(!flags.contains(0x04), "!contains(0x04) in 0x03");

    // Test 10: insert() and remove()
    let mut flags = VmaFlags::new();
    flags.insert(0x01);
    test_assert!(flags.contains(0x01), "insert READ");
    flags.insert(0x02);
    test_assert!(flags.contains(0x03), "insert WRITE, contains both");
    flags.remove(0x01);
    test_assert!(!flags.contains(0x01) && flags.contains(0x02), "remove READ, keep WRITE");

    // Test 11: Extended flags
    let growsdown = VmaFlags::from_bits(0x100);
    test_assert!(growsdown.contains(0x100), "GROWSDOWN == 0x100");

    let locked = VmaFlags::from_bits(0x2000);
    test_assert!(locked.contains(0x2000), "LOCKED == 0x2000");

    let io = VmaFlags::from_bits(0x4000);
    test_assert!(io.contains(0x4000), "IO == 0x4000");

    // Test 12: bits() roundtrip
    let bits = 0x01 | 0x02 | 0x08 | 0x100;
    let flags = VmaFlags::from_bits(bits);
    test_assert_eq!(flags.bits(), bits, "bits() roundtrip");
}
