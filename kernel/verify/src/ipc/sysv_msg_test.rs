//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System V message queue invariant tests.
//!
//! Functions copied from: kernel/src/ipc/sysv_msg.rs

use proptest::prelude::*;

// ============================================================================
// Copied constants and functions from kernel/src/ipc/sysv_msg.rs
// ============================================================================

pub const MSG_EXCEPT: i32 = 0o20000;

struct Msg {
    mtype: i64,
}

fn find_msg_match(messages: &[Msg], msgtyp: i64, msgflg: i32) -> Option<usize> {
    if messages.is_empty() {
        return None;
    }

    if msgtyp == 0 {
        return Some(0);
    }

    if msgtyp > 0 {
        let except = (msgflg & MSG_EXCEPT) != 0;
        for (i, msg) in messages.iter().enumerate() {
            if except {
                if msg.mtype != msgtyp {
                    return Some(i);
                }
            } else {
                if msg.mtype == msgtyp {
                    return Some(i);
                }
            }
        }
        return None;
    }

    // msgtyp < 0: return first message with lowest type <= |msgtyp|
    let abs_type = (-msgtyp) as i64;
    let mut best_idx: Option<usize> = None;
    let mut best_type: i64 = i64::MAX;

    for (i, msg) in messages.iter().enumerate() {
        if msg.mtype <= abs_type && msg.mtype < best_type {
            best_type = msg.mtype;
            best_idx = Some(i);
        }
    }
    best_idx
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-MSG-1: empty queue returns None for all msgtyp
    #[test]
    fn test_empty_queue(
        msgtyp in -100i64..100i64,
        msgflg in 0i32..0o40000i32,
    ) {
        let messages: Vec<Msg> = vec![];
        prop_assert_eq!(find_msg_match(&messages, msgtyp, msgflg), None);
    }

    /// INV-MSG-2: msgtyp=0 returns first message
    #[test]
    fn test_receive_first(types in prop::collection::vec(1i64..100i64, 1..20)) {
        let messages: Vec<Msg> = types.iter().map(|&t| Msg { mtype: t }).collect();
        let result = find_msg_match(&messages, 0, 0);
        prop_assert_eq!(result, Some(0));
    }

    /// INV-MSG-3: msgtyp>0 finds first message of exact type
    #[test]
    fn test_receive_exact_type(
        target_type in 1i64..50i64,
        other_types in prop::collection::vec(1i64..100i64, 0..10),
    ) {
        let mut types = other_types;
        types.push(target_type);
        types.push(target_type); // two copies

        let messages: Vec<Msg> = types.iter().map(|&t| Msg { mtype: t }).collect();

        let result = find_msg_match(&messages, target_type, 0);
        match result {
            Some(idx) => {
                prop_assert_eq!(messages[idx].mtype, target_type);
                // Verify it's the first occurrence
                for i in 0..idx {
                    prop_assert_ne!(messages[i].mtype, target_type,
                        "found match at {} but earlier match at {}", idx, i);
                }
            }
            None => prop_assert!(false, "should find message with type {}", target_type),
        }
    }

    /// INV-MSG-4: msgtyp>0 returns None when no message matches
    #[test]
    fn test_no_exact_match(
        target_type in 1i64..50i64,
        other_types in prop::collection::vec(51i64..100i64, 1..10),
    ) {
        let messages: Vec<Msg> = other_types.iter().map(|&t| Msg { mtype: t }).collect();
        let result = find_msg_match(&messages, target_type, 0);
        prop_assert!(result.is_none());
    }

    /// INV-MSG-5: MSG_EXCEPT finds first non-matching message
    #[test]
    fn test_msg_except(target_type in 1i64..50i64) {
        let messages: Vec<Msg> = vec![
            Msg { mtype: target_type },
            Msg { mtype: target_type },
            Msg { mtype: target_type + 1 },
            Msg { mtype: target_type },
        ];

        let result = find_msg_match(&messages, target_type, MSG_EXCEPT);
        prop_assert_eq!(result, Some(2));
    }

    /// INV-MSG-6: msgtyp<0 finds message with lowest type <= |msgtyp|
    #[test]
    fn test_negative_msgtyp(abs_type in 5i64..50i64) {
        let types = vec![
            abs_type + 10,
            abs_type - 2,
            abs_type,
            abs_type - 5, // lowest, should be found
            abs_type + 1,
        ];

        let messages: Vec<Msg> = types.iter().map(|&t| Msg { mtype: t }).collect();

        let result = find_msg_match(&messages, -abs_type, 0);
        prop_assert_eq!(result, Some(3)); // abs_type - 5 is lowest
    }

    /// INV-MSG-7: msgtyp<0 returns None when all types > |msgtyp|
    #[test]
    fn test_negative_no_match(abs_type in 1i64..50i64) {
        let types = vec![abs_type + 1, abs_type + 5, abs_type + 10];
        let messages: Vec<Msg> = types.iter().map(|&t| Msg { mtype: t }).collect();

        let result = find_msg_match(&messages, -abs_type, 0);
        prop_assert!(result.is_none());
    }

    /// INV-MSG-8: negative msgtyp respects first-encountered for ties
    #[test]
    fn test_negative_first_encountered(abs_type in 5i64..50i64) {
        let low_type = abs_type - 3;
        let messages: Vec<Msg> = vec![
            Msg { mtype: abs_type },
            Msg { mtype: low_type },   // first occurrence of lowest
            Msg { mtype: low_type },    // second occurrence
        ];

        let result = find_msg_match(&messages, -abs_type, 0);
        prop_assert_eq!(result, Some(1));
    }

    /// INV-MSG-9: single message queue behavior
    #[test]
    fn test_single_message(
        mtype in 1i64..100i64,
        msgtyp in -100i64..100i64,
        msgflg in 0i32..0o40000i32,
    ) {
        let messages = vec![Msg { mtype }];
        let result = find_msg_match(&messages, msgtyp, msgflg);

        if msgtyp == 0 {
            prop_assert_eq!(result, Some(0));
        } else if msgtyp > 0 {
            let except = (msgflg & MSG_EXCEPT) != 0;
            if except {
                if mtype != msgtyp {
                    prop_assert_eq!(result, Some(0));
                } else {
                    prop_assert_eq!(result, None);
                }
            } else {
                if mtype == msgtyp {
                    prop_assert_eq!(result, Some(0));
                } else {
                    prop_assert_eq!(result, None);
                }
            }
        } else {
            let abs_type = (-msgtyp) as i64;
            if mtype <= abs_type {
                prop_assert_eq!(result, Some(0));
            } else {
                prop_assert_eq!(result, None);
            }
        }
    }

    /// INV-MSG-10: MSG_EXCEPT with no exceptable messages returns None
    #[test]
    fn test_msg_except_all_match(target_type in 1i64..50i64) {
        let messages: Vec<Msg> = (0..5).map(|_| Msg { mtype: target_type }).collect();
        let result = find_msg_match(&messages, target_type, MSG_EXCEPT);
        prop_assert!(result.is_none());
    }
}
