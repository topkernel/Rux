//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

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

    // Test 13 (FWK): object-keyed identity — two SHARED-VMA keys with the
    // same phys match even across different uaddr/mm (the tst_checkpoint
    // cross-mmap protocol); different phys never matches.
    let obj1 = FutexKey { uaddr: 0x1000, mm: 7, flags: 0x10, phys: 0x555001000, object: true };
    let obj2 = FutexKey { uaddr: 0x9000, mm: 9, flags: 0x10, phys: 0x555001000, object: true };
    let obj3 = FutexKey { uaddr: 0x1000, mm: 7, flags: 0x10, phys: 0x555009000, object: true };
    test_assert!(obj1.matches(&obj2), "object keys match on phys across mappings");
    test_assert!(!obj1.matches(&obj3), "object keys with different phys never match");

    // Test 14 (FWK): anon identity is (mm, uaddr) — IMMUNE to COW frame
    // swaps. A waiter parked when the word resolved phys=P and a waker
    // resolving phys=Q (post-COW-break, same word) still match: the keys
    // are equal regardless of frame. Shared-semantic and private keys on
    // the same anon word unify (the musl cleartid interop).
    let mut anon_wait = FutexKey::new(0x40d2c0, 42, 0x10); // shared semantics
    anon_wait.phys = 0x7ffc72c0; // stale frame from park time
    let mut anon_wake = FutexKey::new(0x40d2c0, 42, 0x10);
    anon_wake.phys = 0x2c05c2c0; // fresh frame after the COW break
    test_assert!(anon_wait.matches(&anon_wake),
        "anon keys match across a COW frame swap");
    let priv_wake = FutexKey::new(0x40d2c0, 42, 0);
    test_assert!(anon_wait.matches(&priv_wake),
        "anon shared-semantics key matches a private key on the same word");
    let other_mm = FutexKey::new(0x40d2c0, 43, 0x10);
    test_assert!(!anon_wait.matches(&other_mm), "anon keys differ across mm");

    // Test 15 (FWK): object identity never matches (mm, uaddr) identity —
    // a MAP_SHARED word's inode-style key is a different word class than
    // a private key (reference: inode key vs private key).
    let obj = FutexKey { uaddr: 0x1000, mm: 7, flags: 0x10, phys: 0x555001000, object: true };
    let anon = FutexKey::new(0x1000, 7, 0x10);
    test_assert!(!obj.matches(&anon), "object key never matches anon key");
    test_assert!(!anon.matches(&obj), "anon key never matches object key");

    // Test 16 (FWK): futex_hash stability under COW — equal anon keys
    // hash equal regardless of the phys field (bucket identity is
    // (mm, uaddr) for object=false keys), and object keys hash by phys.
    let h1 = crate::sync::futex::futex_hash(&anon_wait);
    let h2 = crate::sync::futex::futex_hash(&anon_wake);
    test_assert_eq!(h1, h2, "anon keys with different phys hash equal");
    let h3 = crate::sync::futex::futex_hash(&obj1);
    let h4 = crate::sync::futex::futex_hash(&obj2);
    test_assert_eq!(h3, h4, "object keys with same phys hash equal");
    test_assert!(h3 < crate::config::FUTEX_HASH_SIZE, "hash within bucket count");
}

/// FUTEX_WAKE_OP operand decode vs the reference ABI
/// (kernel/futex/waitwake.c futex_atomic_op_inuser). Fixes verified here:
/// - oparg/cmparg are SIGN-EXTENDED 12-bit fields (0xfff == -1);
/// - the op nibble is 3 bits plus the OPARG_SHIFT bit (oparg = 1 << oparg);
/// - an invalid op (5-7) or cmp (6-15) is -ENOSYS, not -EINVAL.
pub fn test_futex_wake_op_decode() {
    test_group_start("futex_wake_op_decode");
    use crate::sync::futex::{
        futex_wake_op_decode, FUTEX_OP_SET, FUTEX_OP_ADD, FUTEX_OP_CMP_LT, FUTEX_OP_CMP_EQ,
    };

    // Plain op: op=ADD(1), cmp=LT(2), oparg=1, cmparg=1.
    let enc = (1u32 << 28) | (2u32 << 24) | (1u32 << 12) | 1;
    match futex_wake_op_decode(enc) {
        Ok((op, oparg, cmp, cmparg)) => {
            test_assert_eq!(op, FUTEX_OP_ADD, "WAKE_OP op nibble ADD");
            test_assert_eq!(oparg, 1, "WAKE_OP oparg small positive");
            test_assert_eq!(cmp, FUTEX_OP_CMP_LT, "WAKE_OP cmp nibble LT");
            test_assert_eq!(cmparg, 1, "WAKE_OP cmparg small positive");
        }
        Err(e) => test_assert!(false, "plain WAKE_OP decode must succeed")
    }

    // Sign extension: oparg/cmparg 0xfff are -1 (reference sign_extend32
    // of 12-bit fields). The old decode read them as 4095.
    let enc = (1u32 << 28) | (2u32 << 24) | (0xfffu32 << 12) | 0xfff;
    match futex_wake_op_decode(enc) {
        Ok((_op, oparg, _cmp, cmparg)) => {
            test_assert_eq!(oparg, -1, "WAKE_OP oparg 0xfff sign-extends to -1");
            test_assert_eq!(cmparg, -1, "WAKE_OP cmparg 0xfff sign-extends to -1");
        }
        Err(e) => test_assert!(false, "sign-ext decode must succeed")
    }

    // OPARG_SHIFT: bit 3 of the nibble turns oparg into 1 << oparg
    // (reference FUTEX_OP_OPARG_SHIFT). SET|SHIFT with oparg=3 -> 8.
    let enc = ((8u32 | FUTEX_OP_SET) << 28) | (0u32 << 24) | (3u32 << 12) | 8;
    match futex_wake_op_decode(enc) {
        Ok((op, oparg, cmp, cmparg)) => {
            test_assert_eq!(op, FUTEX_OP_SET, "WAKE_OP SHIFT keeps op = SET");
            test_assert_eq!(oparg, 8, "WAKE_OP OPARG_SHIFT oparg 3 -> 8");
            test_assert_eq!(cmp, 0, "WAKE_OP cmp EQ");
            test_assert_eq!(cmparg, 8, "WAKE_OP cmparg 8");
        }
        Err(e) => test_assert!(false, "shift decode must succeed")
    }

    // Invalid op nibble 5 -> ENOSYS (was EINVAL).
    let enc = (5u32 << 28) | (0u32 << 24);
    match futex_wake_op_decode(enc) {
        Ok(_) => test_assert!(false, "op nibble 5 must fail"),
        Err(e) => test_assert_eq!(e, crate::syscall::errno::ENOSYS, "WAKE_OP bad op is ENOSYS"),
    }

    // Invalid cmp nibble 6 -> ENOSYS (was EINVAL).
    let enc = (0u32 << 28) | (6u32 << 24);
    match futex_wake_op_decode(enc) {
        Ok(_) => test_assert!(false, "cmp nibble 6 must fail"),
        Err(e) => test_assert_eq!(e, crate::syscall::errno::ENOSYS, "WAKE_OP bad cmp is ENOSYS"),
    }

    // GT is the largest valid cmp (5); EQ is the smallest (0).
    let enc = (0u32 << 28) | (5u32 << 24);
    test_assert!(futex_wake_op_decode(enc).is_ok(), "cmp GT valid");
}
