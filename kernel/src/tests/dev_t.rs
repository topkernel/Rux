//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::dev_t::{DevNo, MEM_MAJOR, TTY_MAJOR, INPUT_MAJOR, DEV_NULL, DEV_ZERO, DEV_RANDOM, DEV_URANDOM, DEV_KMSG, DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER, EVDEV_MINOR_BASE};
use super::{test_pass, test_fail, test_group_start};

pub fn test_dev_t() {
    test_group_start("dev_t");

    // Test 1: DevNo construction
    let dev = DevNo::new(1, 3);
    test_assert!(dev.major == 1 && dev.minor == 3, "DevNo::new construction");

    // Test 2: DevNo to_u64 — Linux kernel internal dev_t layout (MINORBITS=20)
    let dev = DevNo::new(0xABCD, 0x1234);
    test_assert!(dev.to_u64() == ((0xABCDu64 << 20) | 0x1234u64), "DevNo::to_u64 encoding");
    // oversized minors are masked to 20 bits on encode
    test_assert!(DevNo::new(0, 0x12345678).to_u64() == 0x45678, "DevNo::to_u64 masks minor to 20 bits");

    // Test 3: DevNo from_u64
    let v = ((0xABCDu64 << 20) | 0x1234u64);
    let dev = DevNo::from_u64(v);
    test_assert!(dev.major == 0xABCD && dev.minor == 0x1234, "DevNo::from_u64 decoding");

    // Test 4: Roundtrip consistency
    let original = DevNo::new(42, 7);
    let roundtrip = DevNo::from_u64(original.to_u64());
    test_assert!(roundtrip == original, "DevNo roundtrip consistency");

    // Test 5: Roundtrip with max encodable values (major: 12 bits used by
    // dev_t, minor: 20 bits per MINORBITS)
    let max_dev = DevNo::new(0xFFF, 0xFFFFF);
    let rt = DevNo::from_u64(max_dev.to_u64());
    test_assert!(rt.major == 0xFFF && rt.minor == 0xFFFFF, "DevNo roundtrip max values");

    // Test 6: DevNo from_u64(0)
    let zero = DevNo::from_u64(0);
    test_assert!(zero.major == 0 && zero.minor == 0, "DevNo::from_u64(0)");

    // Test 7: DevNo default
    let default = DevNo::default();
    test_assert!(default.major == 0 && default.minor == 0, "DevNo::default");

    // Test 8: Major device number constants
    test_assert!(MEM_MAJOR == 1, "MEM_MAJOR == 1");
    test_assert!(TTY_MAJOR == 4, "TTY_MAJOR == 4");
    test_assert!(INPUT_MAJOR == 13, "INPUT_MAJOR == 13");

    // Test 9: Standard device constants
    test_assert!(DEV_NULL.major == MEM_MAJOR && DEV_NULL.minor == 3, "DEV_NULL = 1,3");
    test_assert!(DEV_ZERO.major == MEM_MAJOR && DEV_ZERO.minor == 5, "DEV_ZERO = 1,5");
    test_assert!(DEV_RANDOM.major == MEM_MAJOR && DEV_RANDOM.minor == 8, "DEV_RANDOM = 1,8");
    test_assert!(DEV_URANDOM.major == MEM_MAJOR && DEV_URANDOM.minor == 9, "DEV_URANDOM = 1,9");

    // Test 10: Input device constants
    test_assert!(DEV_KMSG.major == MEM_MAJOR && DEV_KMSG.minor == 11, "DEV_KMSG = 1,11");
    test_assert!(DEV_EVDEV_KEYBOARD.major == INPUT_MAJOR && DEV_EVDEV_KEYBOARD.minor == EVDEV_MINOR_BASE, "DEV_EVDEV_KEYBOARD");
    test_assert!(DEV_EVDEV_POINTER.major == INPUT_MAJOR && DEV_EVDEV_POINTER.minor == EVDEV_MINOR_BASE + 1, "DEV_EVDEV_POINTER");

    // Test 11: DevNo Ord trait (used for sorting)
    let a = DevNo::new(1, 5);
    let b = DevNo::new(2, 3);
    let c = DevNo::new(1, 5);
    test_assert!(a < b, "DevNo Ord: major comparison");
    test_assert!(a == c, "DevNo Eq");
}
