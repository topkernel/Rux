
// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]
// Typed per-CPU storage shape tests (rust-design-headroom §3,
// divergence D-7). Pin the properties the type promises, so an edit
// that weakens them fails here before it can drift into the tree:
//   - the guard's lifetime is the irq-off window (state restored on
//     drop, nested windows flag-correct),
//   - rows are reachable ONLY through with() (the re-entry tripwire
//     refuses a second live guard on the same slot: counted, tainted,
//     None returned — never aliased),
//   - an out-of-range cpu id takes the None arm (the r8final
//     survive-and-fall-back stance),
//   - values written under a guard survive to the next window.
use crate::sync::percpu;
use super::{test_pass, test_fail, test_group_start};

#[derive(Clone, Copy)]
struct Probe {
    a: u64,
    b: u64,
}

static SLOT_A: percpu::CpuLocal<Probe, { crate::config::MAX_CPUS }> =
    percpu::CpuLocal::new(Probe { a: 0, b: 0 });
static SLOT_B: percpu::CpuLocal<u64, { crate::config::MAX_CPUS }> = percpu::CpuLocal::new(7);
static SLOT_ZERO: percpu::CpuLocal<u64, 0> = percpu::CpuLocal::new(1);

pub fn test_percpu() {
    test_group_start("percpu-shape");

    // ---- Test 1: write under a guard, read back in the next window ----
    SLOT_A.with(|mut row| {
        row.a = 0xAAAA_5555_dead_beef;
        row.b = 42;
    });
    let readback = SLOT_A.with(|row| (row.a, row.b));
    if readback == Some((0xAAAA_5555_dead_beef, 42)) {
        test_pass("row write/read round-trip");
    } else {
        test_fail("row write/read round-trip", "value lost between windows");
    }

    // ---- Test 2: the guard is the irq-off window ----
    // Outside any window IRQs are on (test harness context); inside,
    // the nested save must report already-disabled; after drop the
    // saved state must be back exactly.
    let before = crate::arch::cpu::save_and_disable_irq();
    crate::arch::cpu::restore_irq(before);
    let ok = SLOT_B.with(|_row| {
        // Nested inside the guard: IRQs must be off (nested save
        // returns false = was-disabled).
        !crate::arch::cpu::save_and_disable_irq()
    });
    let after = crate::arch::cpu::save_and_disable_irq();
    crate::arch::cpu::restore_irq(after);
    if ok == Some(true) && before == after {
        test_pass("guard window = irq-off window");
    } else {
        test_fail("guard window = irq-off window", "nested state or restore wrong");
    }

    // ---- Test 3: nested windows on DIFFERENT slots are legal ----
    let nested = SLOT_A.with(|mut row| {
        row.a = row.a.wrapping_add(1);
        SLOT_B.with(|b| *b == 7)
    });
    if nested == Some(Some(true)) {
        test_pass("nested different slots");
    } else {
        test_fail("nested different slots", "inner with() failed");
    }

    // ---- Test 4: same-slot re-entry is refused and counted ----
    // (The re-entry also taints BAD_PAGE, but the taint SET is not
    // compared: the small_alloc group earlier in this boot already
    // sets the same flag, so the aggregate is unchanged by design.)
    let before_count = percpu::reentry_events();
    let inner = SLOT_A.with(|_row| {
        // Re-entering the SAME slot while the outer guard is live must
        // return None (never hand out a second &mut) and count one
        // re-entry event.
        SLOT_A.with(|_row2| ())
    });
    let ok = inner == Some(None) && percpu::reentry_events() == before_count + 1;
    if ok {
        test_pass("re-entry refused + counted");
    } else {
        test_fail(
            "re-entry refused + counted",
            "inner must be None with exactly one event",
        );
    }

    // ---- Test 5: out-of-range cpu arm (zero-capacity bank) ----
    // A bank with zero rows can never serve a cpu: with() must take
    // the None arm — the same arm an out-of-range cpu_id takes on a
    // real bank (the r8final divergence family; callers fall back to
    // their locked bodies).
    if SLOT_ZERO.with(|_row| ()).is_none() {
        test_pass("out-of-range cpu -> None arm");
    } else {
        test_fail("out-of-range cpu -> None arm", "zero bank served a row");
    }

    // ---- Test 6: the guard names its cpu ----
    let gcpu = SLOT_B.with(|row| row.cpu());
    if gcpu == Some(crate::arch::cpu_id() as usize) {
        test_pass("guard reports acting cpu");
    } else {
        test_fail("guard reports acting cpu", "cpu mismatch");
    }
}
