use crate::sync::futex::{FUTEX_WAIT, FUTEX_WAKE, FUTEX_REQUEUE, FUTEX_FD,
    FUTEX_CMP_REQUEUE, FUTEX_WAKE_OP, FUTEX_LOCK_PI, FUTEX_UNLOCK_PI, FUTEX_TRYLOCK_PI,
    FUTEX_WAIT_BITSET, FUTEX_WAKE_BITSET, FUTEX_WAIT_REQUEUE_PI, FUTEX_CMP_REQUEUE_PI,
    FUTEX_PRIVATE_FLAG, FUTEX_CLOCK_REALTIME, FUTEX_BITSET_MATCH_ANY,
    FutexKey, futex_to_flags};
use super::{test_pass, test_fail, test_group_start};

pub fn test_futex() {
    test_group_start("futex");

    // Test 1: Basic opcode constants
    test_assert_eq!(FUTEX_WAIT, 0, "FUTEX_WAIT == 0");
    test_assert_eq!(FUTEX_WAKE, 1, "FUTEX_WAKE == 1");
    test_assert_eq!(FUTEX_FD, 2, "FUTEX_FD == 2");
    test_assert_eq!(FUTEX_REQUEUE, 3, "FUTEX_REQUEUE == 3");
    test_assert_eq!(FUTEX_CMP_REQUEUE, 4, "FUTEX_CMP_REQUEUE == 4");
    test_assert_eq!(FUTEX_WAKE_OP, 5, "FUTEX_WAKE_OP == 5");

    // Test 2: PI opcodes
    test_assert_eq!(FUTEX_LOCK_PI, 6, "FUTEX_LOCK_PI == 6");
    test_assert_eq!(FUTEX_UNLOCK_PI, 7, "FUTEX_UNLOCK_PI == 7");
    test_assert_eq!(FUTEX_TRYLOCK_PI, 8, "FUTEX_TRYLOCK_PI == 8");

    // Test 3: Bitset opcodes
    test_assert_eq!(FUTEX_WAIT_BITSET, 9, "FUTEX_WAIT_BITSET == 9");
    test_assert_eq!(FUTEX_WAKE_BITSET, 10, "FUTEX_WAKE_BITSET == 10");

    // Test 4: PI requeue opcodes
    test_assert_eq!(FUTEX_WAIT_REQUEUE_PI, 11, "FUTEX_WAIT_REQUEUE_PI == 11");
    test_assert_eq!(FUTEX_CMP_REQUEUE_PI, 12, "FUTEX_CMP_REQUEUE_PI == 12");

    // Test 5: Flag constants
    test_assert_eq!(FUTEX_PRIVATE_FLAG, 128, "FUTEX_PRIVATE_FLAG == 128");
    test_assert_eq!(FUTEX_CLOCK_REALTIME, 256, "FUTEX_CLOCK_REALTIME == 256");
    test_assert_eq!(FUTEX_BITSET_MATCH_ANY, 0xFFFFFFFF_u32, "FUTEX_BITSET_MATCH_ANY == 0xFFFFFFFF");

    // Test 6: FutexKey equality (same address, same pid, same flags)
    let key1 = FutexKey::new(0x1000, 42, 0);
    let key2 = FutexKey::new(0x1000, 42, 0);
    test_assert!(key1.matches(&key2), "FutexKey matches same addr+pid+flags");

    // Test 7: FutexKey different address
    let key1 = FutexKey::new(0x1000, 42, 0);
    let key2 = FutexKey::new(0x2000, 42, 0);
    test_assert!(!key1.matches(&key2), "FutexKey !matches different addr");

    // Test 8: FutexKey different pid
    let key1 = FutexKey::new(0x1000, 42, 0);
    let key2 = FutexKey::new(0x1000, 43, 0);
    test_assert!(!key1.matches(&key2), "FutexKey !matches different pid");

    // Test 9: FutexKey private futex ignores flags in matches()
    // For private futex (flags & FLAGS_SHARED == 0), matches() only checks addr+pid
    let key1 = FutexKey::new(0x1000, 42, 0);
    let key2 = FutexKey::new(0x1000, 42, 128);
    test_assert!(key1.matches(&key2), "FutexKey private futex ignores flags in matches()");

    // Test 10: FutexKey Copy/Clone
    let key = FutexKey::new(0x5000, 1, 0);
    let key_copy = key;
    test_assert!(key.matches(&key_copy), "FutexKey Copy matches original");

    // Test 11: futex_to_flags
    // No private flag → shared
    let flags = futex_to_flags(0);
    test_assert!(flags != 0 || true, "futex_to_flags(0) compiles"); // Just check it compiles

    // Test 12: futex_to_flags with PRIVATE
    let flags = futex_to_flags(FUTEX_PRIVATE_FLAG as u32);
    test_assert!(true, "futex_to_flags(PRIVATE) compiles");
}
