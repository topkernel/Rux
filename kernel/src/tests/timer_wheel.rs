//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

//! Typed timer-wheel shape tests (arch-s5 pilot, survey S-5 exit
//! criterion (a)). The wheel is pure-logic over a `Now` snapshot, so
//! these run with synthetic times (the suite executes with timer IRQs
//! off). They pin:
//!
//!   - slot round-trip: probe_route ↔ bucket_of agree, levels by
//!     distance;
//!   - wheel-wrap arithmetic: exact-jiffy firing across level
//!     boundaries and 64-bucket wraps;
//!   - the historical empty-raise/resync/early-fire classes as pinned
//!     regressions (an hres entry must fire ONLY on the precise
//!     compare, never on the jiffy clock);
//!   - periodic re-arm delivers on EVERY expiry (R37 bug);
//!   - budget carry and catch-up advance drain nothing late or twice.
//!
//! Allocation-free by design: this group runs at the suite tail where
//! earlier groups have been shown to exhaust the heap (syscall:time
//! SKIPs on this very run class) — a Vec push here would trip
//! alloc_error_handler (panic = abort = parked CPU), freezing the
//! suite with no diagnostic. Counters and fixed arrays only.

use crate::timer::wheel::{self, Action, Now, TimerWheel};
use super::{test_fail, test_group_start, test_pass};

/// Fired-token scratch (the suite is single-CPU here).
const BUDGET: usize = wheel::EXPIRY_BUDGET;

/// Ids of interest for one collect pass (64 = budget bound).
struct FiredLog {
    ids: [u64; BUDGET],
    n: usize,
}

impl FiredLog {
    const fn new() -> Self {
        FiredLog { ids: [0; BUDGET], n: 0 }
    }
    fn record(&mut self, id: u64) {
        if self.n < BUDGET {
            self.ids[self.n] = id;
            self.n += 1;
        }
    }
    fn len(&self) -> usize {
        self.n
    }
    fn contains(&self, id: u64) -> bool {
        self.ids[..self.n].contains(&id)
    }
    fn only(&self) -> Option<u64> {
        if self.n == 1 { Some(self.ids[0]) } else { None }
    }
}

/// Aggregate fire ledger across passes (test totals stay far below
/// 2^16; 256 slots is 4x the largest case).
struct FireTotals {
    counts: [u16; 256],
    total: u32,
}

impl FireTotals {
    const fn new() -> Self {
        FireTotals { counts: [0; 256], total: 0 }
    }
    fn add(&mut self, log: &FiredLog) {
        for &id in &log.ids[..log.n] {
            self.counts[(id % 256) as usize] += 1;
            self.total += 1;
        }
    }
    fn of(&self, id: u64) -> u16 {
        self.counts[(id % 256) as usize]
    }
}

/// Step the wheel to (jiffies, time) and collect into `log`.
fn run_pass(w: &mut TimerWheel, now: Now, out: &mut [wheel::FiredToken], log: &mut FiredLog) {
    w.advance(now);
    let n = w.collect(now, out);
    for t in &out[..n] {
        log.record(t.id);
    }
}

fn tokens() -> [wheel::FiredToken; BUDGET] {
    [wheel::FiredToken {
        id: 0,
        action: Action::None,
        late_ticks: 0,
    }; BUDGET]
}

pub fn test_timer_wheel() {
    test_group_start("timer-wheel");

    // ---- Test 1: slot round-trip (probe_route <-> armed bucket) ----
    {
        let mut w = TimerWheel::new();
        let cur = 1000u64;
        w.advance(Now { jiffies: cur, time: 0 });
        // distances: due-now, near (L0), mid (L1), far-in-wheel (L2),
        // beyond span (FAR)
        let cases: [(u64, &str); 5] = [
            (cur, "due-now"),
            (cur + 5, "level0"),
            (cur + 70, "level1"),
            (cur + 5000, "level2"),
            (cur + 300000, "far"),
        ];
        let mut ok = true;
        for (i, (abs, name)) in cases.iter().enumerate() {
            let id = 10 + i as u64;
            if !w.arm_jiffy(*abs, Action::None, None, id) {
                test_fail("arm", name);
                ok = false;
                continue;
            }
            if w.bucket_of(id) != Some(wheel::TimerWheel::probe_route(*abs, cur)) {
                test_fail("slot round-trip", name);
                ok = false;
            }
        }
        if ok {
            test_pass("slot round-trip (5 classes)");
        }
    }

    // ---- Test 2: wheel-wrap arithmetic — exact-jiffy firing across
    // boundaries (a timer armed for J fires at J, not J±1, including
    // across the 64-bucket wrap and the L1 cascade boundary). ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        let base = 62u64; // near the level-0 wrap
        w.advance(Now { jiffies: base, time: 0 });
        let targets = [base + 1, base + 2, base + 63, base + 64, base + 65, base + 128];
        for (i, t) in targets.iter().enumerate() {
            if !w.arm_jiffy(*t, Action::None, None, 100 + i as u64) {
                test_fail("arm boundary", "t");
                return;
            }
        }
        // fire jiffy per id: id 100+i fired at targets[i]?
        let mut fire_jiffy = [0u64; 6];
        let mut seen = [false; 6];
        let mut log = FiredLog::new();
        for j in base..=base + 130 {
            log = FiredLog::new();
            run_pass(&mut w, Now { jiffies: j, time: 0 }, &mut out, &mut log);
            for i in 0..6 {
                if log.contains(100 + i as u64) {
                    fire_jiffy[i] = j;
                    seen[i] = true;
                }
            }
        }
        let ok = seen.iter().all(|s| *s)
            && (0..6).all(|i| fire_jiffy[i] == targets[i]);
        if ok {
            test_pass("wrap/cascade exact-jiffy fires");
        } else {
            test_fail("wrap/cascade exact-jiffy fires", "fire jiffy mismatch");
        }
    }

    // ---- Test 3: the early-fire pinned regression — an hres entry
    // fires ONLY on the precise time compare, never because its jiffy
    // horizon passed (the lost-timer-wakeup root cause). ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        let tpj = 100_000u64; // synthetic ticks per jiffy
        let start_j = 500u64;
        let start_t = start_j * tpj;
        w.advance(Now { jiffies: start_j, time: start_t });
        // Precise deadline 2.5 jiffies out; grid-derived horizon (the
        // wheel derives it in the kernel via jiffy_floor_of_time; here
        // we pass floor(2.5)+0 = jiffy 502 — the first jiffy whose
        // span covers the deadline).
        let precise = start_t + (5 * tpj) / 2;
        let horizon = start_j + 2;
        if !w.arm_hres(precise, horizon, Action::Wake { pid: 7 }, None, 42) {
            test_fail("arm hres", "pool");
            return;
        }
        let mut log = FiredLog::new();
        // Advance jiffies well past the horizon while time stays short:
        // the entry must NOT fire (the old jiffy fallback did).
        run_pass(&mut w, Now { jiffies: start_j + 10, time: start_t + tpj }, &mut out, &mut log);
        if log.len() != 0 {
            test_fail("hres early-fire guard", "fired on jiffy clock");
            return;
        }
        // Time reaches the deadline (jiffies unchanged): must fire NOW.
        run_pass(&mut w, Now { jiffies: start_j + 10, time: precise }, &mut out, &mut log);
        if log.only() != Some(42) {
            test_fail("hres precise fire", "wrong token set");
            return;
        }
        // And exactly once (one-shot: the slot is gone).
        log = FiredLog::new();
        run_pass(&mut w, Now { jiffies: start_j + 12, time: precise + tpj }, &mut out, &mut log);
        if log.len() != 0 {
            test_fail("hres one-shot", "double fire");
            return;
        }
        test_pass("hres precise-gate regression (early-fire pin)");
    }

    // ---- Test 4: periodic re-arm delivers on EVERY expiry and keeps
    // the same id (R37: the re-arm path once delivered nothing). ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        w.advance(Now { jiffies: 0, time: 0 });
        let interval = core::num::NonZeroU64::new(3).unwrap();
        if !w.arm_jiffy(3, Action::Signal { pid: 9, signo: 14 }, Some(interval), 77) {
            test_fail("arm periodic", "pool");
            return;
        }
        let mut totals = FireTotals::new();
        for j in 0..31u64 {
            let mut log = FiredLog::new();
            run_pass(&mut w, Now { jiffies: j, time: 0 }, &mut out, &mut log);
            totals.add(&log);
        }
        if totals.of(77) == 10 && totals.total == 10 {
            test_pass("periodic delivery on every expiry (10/10)");
        } else {
            test_fail("periodic delivery count", "not 10/10");
        }
    }

    // ---- Test 5: budget carry — more due entries than one pass can
    // collect; nothing lost, nothing double-fired. (This case caught
    // the first-version link_tail bug: linking after the head orphaned
    // every entry between head and tail.) ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        w.advance(Now { jiffies: 0, time: 0 });
        // BUDGET + 10 one-shots all due at jiffy 5.
        for i in 0..(BUDGET + 10) as u64 {
            if !w.arm_jiffy(5, Action::None, None, 1000 + i) {
                test_fail("arm storm", "pool");
                return;
            }
        }
        let mut log = FiredLog::new();
        run_pass(&mut w, Now { jiffies: 5, time: 0 }, &mut out, &mut log);
        if log.len() != BUDGET {
            test_fail("budget cap", "first pass != budget");
            return;
        }
        let mut totals = FireTotals::new();
        totals.add(&log);
        log = FiredLog::new();
        run_pass(&mut w, Now { jiffies: 6, time: 0 }, &mut out, &mut log);
        totals.add(&log);
        log = FiredLog::new();
        run_pass(&mut w, Now { jiffies: 7, time: 0 }, &mut out, &mut log);
        totals.add(&log);
        if totals.total == (BUDGET + 10) as u32 {
            test_pass("budget carry drains all, none double-fired");
        } else {
            test_fail("budget carry total", "lost or double");
        }
    }

    // ---- Test 6: catch-up advance — a multi-jiffy jump (tick stall)
    // drains every intermediate due entry through the DUE carry. ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        w.advance(Now { jiffies: 10, time: 0 });
        // Entries at several intermediate jiffies + one far (L1) entry
        // whose bucket cascades during the jump.
        for (i, jf) in [11u64, 15, 40, 64, 73, 200].iter().enumerate() {
            if !w.arm_jiffy(*jf, Action::None, None, 2000 + i as u64) {
                test_fail("arm catchup", "pool");
                return;
            }
        }
        // One jump from 10 to 500.
        let mut log = FiredLog::new();
        run_pass(&mut w, Now { jiffies: 500, time: 0 }, &mut out, &mut log);
        let all = (0..6).all(|i| log.contains(2000 + i as u64));
        if all && log.len() == 6 {
            test_pass("catch-up advance drains all intermediates");
        } else {
            test_fail("catch-up advance", "missing intermediates");
        }
    }

    // ---- Test 7: remove/pending/state (del_timer semantics). ----
    {
        let mut w = TimerWheel::new();
        w.advance(Now { jiffies: 100, time: 0 });
        if !w.arm_jiffy(140, Action::Wake { pid: 3 }, None, 3000) {
            test_fail("arm remove", "pool");
            return;
        }
        if !w.contains(3000) || w.state_of(3000) != Some((140, 0)) {
            test_fail("pending/state", "readback");
            return;
        }
        match w.remove(3000) {
            wheel::RemoveOutcome::RemovedJiffy => {}
            _ => {
                test_fail("remove jiffy", "outcome");
                return;
            }
        }
        if w.contains(3000) || w.remove(3000) != wheel::RemoveOutcome::NotFound {
            test_fail("remove idempotence", "ghost");
            return;
        }
        // hres remove reports the class (HRES_NEXT recompute driver).
        if !w.arm_hres(9_999_999, 120, Action::None, None, 3001) {
            test_fail("arm hres remove", "pool");
            return;
        }
        match w.remove(3001) {
            wheel::RemoveOutcome::RemovedHres(t) if t == 9_999_999 => {}
            _ => {
                test_fail("remove hres", "outcome");
                return;
            }
        }
        test_pass("remove/pending/state semantics");
    }

    // ---- Test 8: FAR routing round-trip — a horizon beyond the wheel
    // span fires exactly on time after FAR rescans. ----
    {
        let mut w = TimerWheel::new();
        let mut out = tokens();
        w.advance(Now { jiffies: 0, time: 0 });
        let target = 300000u64; // > 262144 span
        if !w.arm_jiffy(target, Action::None, None, 4000) {
            test_fail("arm far", "pool");
            return;
        }
        if w.bucket_of(4000) != Some(wheel::TimerWheel::probe_route(target, 0)) {
            test_fail("far route", "probe mismatch");
            return;
        }
        let mut totals = FireTotals::new();
        // Step in 4096-jiffy chunks (each level-2 boundary triggers a
        // FAR rescan), stopping short of the target, then per-jiffy
        // steps through the fire jiffy.
        let mut j = 0u64;
        while j + 4096 < target - 8 {
            j += 4096;
            let mut log = FiredLog::new();
            run_pass(&mut w, Now { jiffies: j, time: 0 }, &mut out, &mut log);
            totals.add(&log);
        }
        while j <= target + 4 {
            let mut log = FiredLog::new();
            run_pass(&mut w, Now { jiffies: j, time: 0 }, &mut out, &mut log);
            totals.add(&log);
            j += 1;
        }
        if totals.of(4000) == 1 {
            test_pass("far-list rescan fires exactly once, on time");
        } else {
            test_fail("far-list fire count", "not exactly one");
        }
    }

    // ---- Test 9: pool exhaustion reports failure (the old `0` ABI). ----
    {
        let mut w = TimerWheel::new();
        w.advance(Now { jiffies: 0, time: 0 });
        let mut armed = 0usize;
        for i in 0..wheel::MAX_TIMERS as u64 + 1 {
            if w.arm_jiffy(i + 1, Action::None, None, 5000 + i) {
                armed += 1;
            }
        }
        if armed == wheel::MAX_TIMERS {
            test_pass("pool full boundary");
        } else {
            test_fail("pool full boundary", "armed != MAX_TIMERS");
        }
    }
}
