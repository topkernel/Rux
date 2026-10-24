//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: small-object slab allocator with per-CPU magazines (pa16).
//
// Exercises the magazine state machine on the running CPU: fast
// push/pop, batch refill, row-full snapshot flush, page reclaim after
// deferred frees, and — load-bearing for the wild-write hunt line —
// the consume-and-taint screening that must keep misaligned/double
// frees out of circulation (a magazine that parks a bad pointer would
// hand it back out on the next alloc).
//
// Runs single-CPU in the unit-test boot; the SMP cross-CPU paths are
// covered by the OH soak (wildwrite-hunt line validation ladder).

use crate::mm::small_alloc;
use super::{test_pass, test_fail, test_group_start};

const SIZES: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

fn all_distinct(ptrs: &[usize]) -> bool {
    let n = ptrs.len();
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n {
            if ptrs[i] == ptrs[j] {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

pub fn test_small_alloc() {
    test_group_start("small_alloc magazine");

    // ---- Test 1: reject path (no heap touch) ----
    if small_alloc::alloc(0, 8).is_null()
        && small_alloc::alloc(2048, 8).is_null()
        && small_alloc::alloc(16, 2048).is_null()
    {
        test_pass("oversize/zero layouts rejected");
    } else {
        test_fail("oversize/zero layouts rejected", "null expected");
    }

    // ---- Test 2: round-trip every class, content integrity ----
    let mut ok = true;
    for &s in SIZES.iter() {
        let mut v = [0usize; 4];
        for k in 0..4 {
            let p = small_alloc::alloc(s, s) as usize;
            if p == 0 {
                ok = false;
                break;
            }
            unsafe {
                core::ptr::write_volatile(p as *mut u64, 0xAA00_0000_0000_0000 | s as u64);
                // For s == 8 the "last word" IS the first word — skip the
                // end tag there or it overwrites the start tag.
                if s > 8 {
                    core::ptr::write_volatile(
                        (p + s - 8) as *mut u64,
                        0xBB00_0000_0000_0000 | k as u64,
                    );
                }
            }
            v[k] = p;
        }
        if !all_distinct(&v) {
            ok = false;
        }
        for &p in v.iter() {
            if p != 0 {
                unsafe {
                    if core::ptr::read_volatile(p as *const u64) != 0xAA00_0000_0000_0000 | s as u64
                    {
                        ok = false;
                    }
                }
                small_alloc::free(p);
            }
        }
    }
    if ok {
        test_pass("round-trip all 8 classes, tags intact");
    } else {
        test_fail("round-trip all 8 classes, tags intact", "tag mismatch or null");
    }

    // ---- Test 3: churn across magazine boundaries ----
    // 80 objects per class = 5 magazines' worth: forces refill batches,
    // row-full snapshot flushes, and page reclaim of fully-freed pages.
    const N: usize = 80;
    let mut ok = true;
    for &s in SIZES.iter() {
        let mut v = [0usize; N];
        let mut k = 0;
        while k < N {
            let p = small_alloc::alloc(s, s) as usize;
            if p == 0 {
                ok = false;
                break;
            }
            unsafe { core::ptr::write_volatile(p as *mut u64, (k as u64) << 8 | s as u64) };
            v[k] = p;
            k += 1;
        }
        if !all_distinct(&v) {
            ok = false; // a double hand-out would show as a duplicate live ptr
        }
        // free ALL (snapshot flush fires at MAG_DEPTH boundaries)
        for &p in v.iter() {
            if p != 0 {
                small_alloc::free(p);
            }
        }
        // re-alloc the same count: pages must come back (reclaimed pages
        // re-enter via the buddy), pointers again distinct, writes stick
        let mut w = [0usize; N];
        let mut k = 0;
        while k < N {
            let p = small_alloc::alloc(s, s) as usize;
            if p == 0 {
                ok = false;
                break;
            }
            unsafe { core::ptr::write_volatile(p as *mut u64, 0xDEAD_BEEF_0000_0000 | k as u64) };
            w[k] = p;
            k += 1;
        }
        if !all_distinct(&w) {
            ok = false;
        }
        for &p in w.iter() {
            if p != 0 {
                small_alloc::free(p);
            }
        }
    }
    if ok {
        test_pass("churn 80/class across flush+refill boundaries");
    } else {
        test_fail("churn 80/class across flush+refill boundaries", "null or duplicate ptr");
    }

    // ---- Test 4: magazine hot loop (free->alloc reuse) ----
    let mut ok = true;
    let mut i = 0;
    while i < 200 {
        let p = small_alloc::alloc(64, 64) as usize;
        if p == 0 {
            ok = false;
            break;
        }
        unsafe { core::ptr::write_volatile(p as *mut u64, 0x1234_5678_0000_0000 | i as u64) };
        small_alloc::free(p);
        i += 1;
    }
    if ok {
        test_pass("alloc/free hot loop x200");
    } else {
        test_fail("alloc/free hot loop x200", "null alloc");
    }

    // ---- Test 5: consume-and-taint screening ----
    // A misaligned free into a live slab page must be consumed + tainted,
    // never parked in the magazine (a later alloc must not hand it out),
    // and must not corrupt the neighboring live objects. (A raw DOUBLE
    // free of a valid slot is documented pre-magazine behavior — silent
    // freelist dup — and is smoke-tested LAST so its residue cannot
    // poison the distinctness assertions above.)
    let mut ok = true;
    const BAD_PAGE_BIT: u32 = 0x0020;
    let t0 = crate::dfx::taint::get_taints();
    let p = small_alloc::alloc(8, 8) as usize;
    if p == 0 {
        test_fail("screening setup", "alloc null");
        return;
    }
    unsafe { core::ptr::write_volatile(p as *mut u64, 0x5555_5555_5555_5555) };
    let bad = p + 3; // same page (S=8 objects end at most at page-8), misaligned
    small_alloc::free(bad); // must taint + consume, never park
    let t1 = crate::dfx::taint::get_taints();
    if t1 & BAD_PAGE_BIT == 0 && t0 & BAD_PAGE_BIT == 0 {
        ok = false; // screening must have tailed to the locked taint path
    }
    // the legitimate neighbor must be untouched and still freeable
    if unsafe { core::ptr::read_volatile(p as *const u64) } != 0x5555_5555_5555_5555 {
        ok = false;
    }
    small_alloc::free(p);
    // allocator must still serve clean allocations
    let a = small_alloc::alloc(8, 8) as usize;
    let b = small_alloc::alloc(8, 8) as usize;
    if a == 0 || b == 0 || a == bad || b == bad {
        ok = false; // the misaligned pointer must never be handed out
    }
    if a != 0 {
        small_alloc::free(a);
    }
    if b != 0 {
        small_alloc::free(b);
    }
    if ok {
        test_pass("misaligned free consumed+tainted, never circulated");
    } else {
        test_fail("misaligned free consumed+tainted, never circulated", "see serial");
    }

    // ---- Test 6: multi-page span + partial free + realloc ----
    // 600 class-8 objects span 2+ pages; freeing a strided half, then
    // reallocating, exercises partially-free relink (STATE_FULL ->
    // STATE_ACTIVE) interleaved with magazine flushes.
    let mut ok = true;
    const M: usize = 600;
    let mut v = [0usize; M];
    let mut k = 0;
    while k < M {
        let p = small_alloc::alloc(8, 8) as usize;
        if p == 0 {
            ok = false;
            break;
        }
        unsafe { core::ptr::write_volatile(p as *mut u16, k as u16) };
        v[k] = p;
        k += 1;
    }
    if !all_distinct(&v) {
        ok = false;
    }
    k = 0;
    while k < M {
        if k % 2 == 0 {
            small_alloc::free(v[k]);
            v[k] = 0;
        }
        k += 1;
    }
    k = 0;
    while k < M {
        if v[k] == 0 {
            let p = small_alloc::alloc(8, 8) as usize;
            if p == 0 {
                ok = false;
                break;
            }
            v[k] = p;
        }
        k += 1;
    }
    if !all_distinct(&v) {
        ok = false;
    }
    k = 0;
    while k < M {
        if v[k] != 0 {
            small_alloc::free(v[k]);
        }
        k += 1;
    }
    if ok {
        test_pass("600-object span, strided partial free, realloc");
    } else {
        test_fail("600-object span, strided partial free, realloc", "null or duplicate ptr");
    }

    // ---- Test 7 (last on purpose): double-free smoke ----
    // Documented pre-magazine semantics: a double free of a valid slot
    // silently duplicates it on the freelist (no taint on a
    // partially-free page). The magazine must not turn this into a
    // crash or a bad hand-out class beyond that known residue: assert
    // no panic, continued service, and the census machinery intact.
    let p = small_alloc::alloc(8, 8) as usize;
    if p != 0 {
        small_alloc::free(p);
        small_alloc::free(p);
    }
    let a = small_alloc::alloc(8, 8) as usize;
    if a != 0 {
        unsafe { core::ptr::write_volatile(a as *mut u64, 0x77) };
        small_alloc::free(a);
    }
    if a != 0 {
        test_pass("double-free smoke: no panic, allocator serves");
    } else {
        test_fail("double-free smoke: no panic, allocator serves", "alloc null");
    }

    test_group_start("small_alloc census");
    small_alloc::census_line();
    test_pass("census emitted");
}
