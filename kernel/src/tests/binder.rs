//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Binder unit tests (kernel/src/tests/binder.rs).
//!
//! All binder unit tests live here per the test-layout policy — nothing
//! test-shaped stays inside kernel/src/ipc/binder.rs itself. The pieces
//! under test are crate-visible (pub(crate)) in ipc/binder.rs; test-only
//! infrastructure (the fake BinderAlloc with phys=0) must be mem::forget
//! at the end so Drop never hands the buddy allocator a bogus block.

use crate::fs::dev_t::{DevNo, MISC_MAJOR};
use alloc::vec::Vec;
use crate::ipc::binder::{
    devno_ctx, ev_name, BufBlock, BinderAlloc, BinderCtx, DEV_BINDER, DEV_HWBINDER,
    DEV_VNDBINDER, EV_BWR, EV_DEAD_REPLY, EV_ERR_RECV, EV_PARK, EV_REAP, EV_REKICK,
    EV_REPLY_GONE, EV_REPLY_POP, EV_REPLY_PUSHED, EV_REPLY_RECV, EV_TC_RECV,
    EV_TXN_PQUEUED, EV_TXN_PARKED_ASYNC, EV_TXN_RECV, EV_TXN_RECV_END, EV_TXN_SENT,
    EV_TXN_TQUEUED, EV_WAKE_RET,
};

/// The alloc-recycle lives, ported verbatim from the former in-module
/// test (ipc/binder.rs): a replied-then-BC_FREE'd sync block must NOT
/// carry reply_done into its next allocation — sweep_stale_async treats
/// txn==None && reply_done as "reclaim NOW (no grace)", so a stale
/// marker handed the block to a third transaction while the second was
/// still being processed.
fn test_alloc_recycle() {
    let mut alloc = BinderAlloc {
        // new_buf/free_buf never dereference kvirt; phys stays 0 so Drop
        // would be a no-op-free — but Drop runs put_page on null pages and
        // skips free_pages when still_pinned stays false... a zeroed phys
        // would free_pages(0, order). Leak the alloc instead: forget it.
        kvirt: core::ptr::null_mut(),
        phys: 0,
        npages: 4,
        order: 2,
        user_base: 0x1000,
        blocks: Vec::new(),
        free_async_space: 8192,
        last_sweep_j: 0,
        next_buf_id: 1,
    };
    alloc.blocks.push(BufBlock {
        off: 0,
        len: 16384,
        free: true,
        async_block: false,
        allow_user_free: false,
        data_size: 0,
        offsets_size: 0,
        extra_size: 0,
        buf_id: 0,
        txn: None,
        reply_done: false,
    });

    // Life 1: sync txn buffer, delivered (allow_user_free), replied
    // (reply_done), then freed by the server's BC_FREE_BUFFER — the
    // standard request-buffer flow of every binder server.
    let off1 = match alloc.new_buf(100, 0, 0, false) {
        Some(o) => o,
        None => {
            crate::tests::test_fail("binder_alloc_l1", "first sync alloc failed");
            return;
        }
    };
    {
        let b = alloc.blocks.iter_mut().find(|b| b.off == off1).unwrap();
        b.allow_user_free = true;
        b.reply_done = true; // set by the reply path (BC_REPLY)
    }
    match alloc.free_buf(alloc.user_base + off1 as u64) {
        Some(t) if t.is_none() => {}
        _ => {
            crate::tests::test_fail("binder_alloc_l1", "free of sync buffer failed");
            return;
        }
    }
    crate::tests::test_pass("binder_alloc_l1");

    // Life 2: the SAME block (first-fit from the low end) serves a new
    // sync txn. It must come back clean: reply_done false, no stale txn.
    let off2 = match alloc.new_buf(100, 0, 0, false) {
        Some(o) => o,
        None => {
            crate::tests::test_fail("binder_alloc_l2", "second sync alloc failed");
            return;
        }
    };
    crate::test_assert_eq!(off2, off1, "first-fit recycles the same block");
    let clean = alloc
        .blocks
        .iter()
        .find(|b| b.off == off2)
        .map(|b| !b.reply_done && b.txn.is_none() && !b.free)
        .unwrap_or(false);
    if clean {
        crate::tests::test_pass("binder_alloc_l2_clean");
    } else {
        crate::tests::test_fail(
            "binder_alloc_l2_clean",
            "recycled sync block carries stale reply_done/txn (sweep would reclaim an in-flight buffer)",
        );
    }

    // The allocation must never escape this test (phys==0 would hand the
    // buddy allocator a bogus block on Drop).
    core::mem::forget(alloc);
}

/// ev_name must resolve every defined event id to its stable name — the
/// dfx/trace layers and the OH crash hunts key on these strings.
fn test_ev_names() {
    const CASES: &[(u8, &str)] = &[
        (EV_TXN_SENT, "TXN_SENT"),
        (EV_TXN_TQUEUED, "TXN_TQUEUED"),
        (EV_TXN_PQUEUED, "TXN_PQUEUED"),
        (EV_TXN_PARKED_ASYNC, "PARKED_ASYNC"),
        (EV_REPLY_POP, "REPLY_POP"),
        (EV_REPLY_PUSHED, "REPLY_PUSHED"),
        (EV_REPLY_GONE, "REPLY_GONE"),
        (EV_TXN_RECV, "TXN_RECV"),
        (EV_REPLY_RECV, "REPLY_RECV"),
        (EV_TC_RECV, "TC_RECV"),
        (EV_ERR_RECV, "ERR_RECV"),
        (EV_PARK, "PARK"),
        (EV_WAKE_RET, "WAKE_RET"),
        (EV_BWR, "BWR"),
        (EV_REAP, "REAP"),
        (EV_DEAD_REPLY, "DEAD_REPLY"),
        (EV_REKICK, "REKICK"),
        (EV_TXN_RECV_END, "TXN_RECV_END"),
    ];
    let mut all_ok = true;
    for (ev, name) in CASES {
        if ev_name(*ev) != *name {
            all_ok = false;
        }
    }
    if all_ok {
        crate::tests::test_pass("binder_ev_name_table");
    } else {
        crate::tests::test_fail("binder_ev_name_table", "ev_name drifted from the stable event names");
    }
    // Undefined ids map to "?" without panicking (they are printed in
    // crash dumps for arbitrary event bytes).
    crate::test_assert_eq!(ev_name(0), "?", "undefined event id maps to ?");
    crate::test_assert_eq!(ev_name(255), "?", "undefined event id maps to ?");
    crate::tests::test_pass("binder_ev_name_undefined_safe");
}

/// The three binder device contexts are distinct and unreachable from
/// any other device number: handle-0 resolution is per-context, so a
/// wrong devno_ctx mapping would cross-wire /dev/hwbinder nodes into
/// /dev/binder's context manager.
fn test_devno_ctx() {
    crate::test_assert_eq!(devno_ctx(&DEV_BINDER), Some(BinderCtx::Binder), "/dev/binder context");
    crate::test_assert_eq!(devno_ctx(&DEV_HWBINDER), Some(BinderCtx::HwBinder), "/dev/hwbinder context");
    crate::test_assert_eq!(devno_ctx(&DEV_VNDBINDER), Some(BinderCtx::VndBinder), "/dev/vndbinder context");
    // The minors must stay distinct — the three contexts collapse into
    // one if any two device numbers collide.
    crate::test_assert!(!(DEV_BINDER == DEV_HWBINDER), "binder/hwbinder minors distinct");
    crate::test_assert!(!(DEV_BINDER == DEV_VNDBINDER), "binder/vndbinder minors distinct");
    crate::test_assert!(!(DEV_HWBINDER == DEV_VNDBINDER), "hwbinder/vndbinder minors distinct");
    // An unrelated misc device number resolves to no binder context.
    let other = DevNo::new(MISC_MAJOR, 1);
    crate::test_assert_eq!(devno_ctx(&other), None, "non-binder device has no context");
}

pub fn test_binder() {
    crate::tests::test_group_start("binder");
    test_alloc_recycle();
    test_ev_names();
    test_devno_ctx();
}
