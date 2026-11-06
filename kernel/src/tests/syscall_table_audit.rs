// MIT License
//!
// Copyright (c) 2026 Fei Wang
//!

//! x86_64 syscall-number table audit: our native coverage must match
// the reference x86_64 syscall table (arch/x86/entry/syscalls/
// syscall_64.tbl, linux-6.6). PR#11 (9c1be9b1) closed the silent
// fall-through class — an untranslated native number used to run the
// SAME-NUMBERED generic syscall (37 alarm ran linkat, 215
// epoll_wait_old ran munmap, 172 iopl "succeeded" as getpid). This
// test pins the fix: every reference-implemented number must be
// handled (translate(), the dispatch pre-match, or a whitelisted
// known gap), and every reference HOLE must stay unmapped (ENOSYS —
// never a same-numbered generic dispatch).
//!
// Regeneration: numbers below are generated from the reference
// syscall_64.tbl (implemented = table entry with a non-sys_ni impl;
// holes = numbers absent from the table up to its max 453; the
// 462..470 range is outside this table and not audited here).

use crate::syscall::x86_compat::{translate, UNMAPPED_NR};
use crate::syscall::SyscallArgs;
use super::{test_pass, test_fail, test_group_start};

const REF_IMPLEMENTED: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79,
    80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
    96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,
    112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127,
    128, 129, 130, 131, 132, 133, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144,
    145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 157, 158, 159, 160, 161,
    162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 175, 176, 179, 186,
    187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202,
    203, 204, 206, 207, 208, 209, 210, 212, 213, 216, 217, 218, 219, 220, 221, 222,
    223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 237, 238, 239,
    240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
    256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271,
    272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287,
    288, 289, 290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303,
    304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319,
    320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333, 334, 424,
    425, 426, 427, 428, 429, 430, 431, 432, 433, 434, 435, 436, 437, 438, 439, 440,
    441, 442, 443, 444, 445, 446, 447, 448, 449, 450, 451, 452, 453,
];

const REF_HOLES: &[u16] = &[
    134, 156, 174, 177, 178, 180, 181, 182, 183, 184, 185, 205, 211, 214, 215, 236,
    335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350,
    351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365, 366,
    367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380, 381, 382,
    383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398,
    399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414,
    415, 416, 417, 418, 419, 420, 421, 422, 423,
];

// Numbers handled by dispatch.rs BEFORE translate() — x86_64-native
// syscalls with no generic slot (poll/select/dup2/dup/pause/alarm/
// sysfs/epoll_create/epoll_wait) plus arch_prctl (158, handled by its
// own early arm; glibc/musl TLS requirement). Keep in sync with the
// pre-match in syscall_handler_inner.
const PRE_MATCH: &[u16] = &[7, 23, 33, 34, 37, 139, 158, 213, 232];

// Reference-implemented numbers we deliberately leave ENOSYS
// (documented gaps — obsolete/niche ABIs whose users (glibc/musl)
// prefer the modern equivalent): 78 getdents (pre-getdents64 dirent
// layout), 132 utime, 136 ustat, 154 modify_ldt, 172 iopl, 173
// ioperm, 235 utimes, 248 add_key, 249 request_key, 250 keyctl, 261
// futimesat. Any of these gaining an implementation must leave this
// list (test 3 enforces the sync).
const KNOWN_GAPS: &[u16] = &[
    78, 132, 136, 154, 172, 173, 235, 248, 249, 250, 261,
];

const ZERO_ARGS: SyscallArgs = [0; 6];

pub fn test_syscall_table_audit() {
    test_group_start("syscall-table-audit");

    // Test 1: every reference-implemented native number is handled —
    // a NEW hole here is the PR#11 bug class reborn (the syscall would
    // ENOSYS where the reference works).
    let mut missing = alloc::vec::Vec::new();
    for &nr in REF_IMPLEMENTED {
        let handled = PRE_MATCH.contains(&nr)
            || KNOWN_GAPS.contains(&nr)
            || translate(nr as u64, &ZERO_ARGS).is_some();
        if !handled {
            missing.push(nr);
        }
    }
    if missing.is_empty() {
        test_pass("all reference-implemented natives handled");
    } else {
        test_fail(
            "reference-native coverage",
            &alloc::format!("unhandled natives: {:?}", missing),
        );
    }

    // Test 2: every reference hole stays unmapped — covering a hole
    // would implement a syscall the reference ENOSYS-es, and (pre-PR#11)
    // was the silent same-numbered-generic dispatch shape.
    let mut covered_holes = alloc::vec::Vec::new();
    for &nr in REF_HOLES {
        if PRE_MATCH.contains(&nr) || translate(nr as u64, &ZERO_ARGS).is_some() {
            covered_holes.push(nr);
        }
    }
    if covered_holes.is_empty() {
        test_pass("all reference holes stay ENOSYS");
    } else {
        test_fail(
            "reference holes must stay ENOSYS",
            &alloc::format!("holes mapped: {:?}", covered_holes),
        );
    }

    // Test 3: KNOWN_GAPS is honest — a "gap" that gained an
    // implementation must leave the list (stale entries hide coverage).
    let mut stale = alloc::vec::Vec::new();
    for &nr in KNOWN_GAPS {
        if translate(nr as u64, &ZERO_ARGS).is_some() {
            stale.push(nr);
        }
    }
    if stale.is_empty() {
        test_pass("known-gaps list in sync");
    } else {
        test_fail(
            "known-gaps list in sync",
            &alloc::format!("stale entries: {:?}", stale),
        );
    }

    // Test 4: spot-check the argument-remap semantics for the classic
    // divergent numbers (the historical fall-through victims named in
    // PR#11's message all sit in these families).
    let ok = translate(0, &ZERO_ARGS).map(|(g, _)| g) == Some(63)      // read
        && translate(1, &ZERO_ARGS).map(|(g, _)| g) == Some(64)        // write
        && translate(3, &ZERO_ARGS).map(|(g, _)| g) == Some(57)        // close
        && translate(9, &ZERO_ARGS).map(|(g, _)| g) == Some(222)       // mmap
        && translate(202, &ZERO_ARGS).map(|(g, _)| g) == Some(98)      // futex
        && translate(233, &ZERO_ARGS).map(|(g, _)| g) == Some(21)      // epoll_ctl
        && translate(257, &ZERO_ARGS).map(|(g, _)| g) == Some(56)      // openat
        && translate(2, &[1, 2, 3, 0, 0, 0]).map(|(g, a)| {
            // open(path, flags, mode) -> openat(AT_FDCWD, path, flags, mode)
            (g == 56 && a[0] == (-100i64) as u64 && a[1] == 1)
        }) == Some(true);
    if ok {
        test_pass("divergent-family translations remap correctly");
    } else {
        test_fail("divergent spot checks", "a classic family remap changed");
    }

    // Test 5: dispatch_target routes unmapped natives to the ENOSYS arm
    // (UNMAPPED_NR), never the same-numbered generic.
    let (target, _) = crate::syscall::x86_compat::dispatch_target(215, &ZERO_ARGS);
    if target == UNMAPPED_NR {
        test_pass("unmapped native routes to ENOSYS");
    } else {
        test_fail("unmapped native dispatch", "routed to a live syscall");
    }
    let _ = UNMAPPED_NR;
}
