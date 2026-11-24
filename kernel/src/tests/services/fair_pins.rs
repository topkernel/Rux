//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Fair-class pins — the sched half of the old kernel/src/tests/
//! mm_sched_pins.rs, moved with the S1b crate flip (the test follows the
//! policy files). The pinned types are the framework's storage-side
//! entities (sched/entities.rs, reached through the re-export chain);
//! runs in-guest through the SERVICE-TEST SEAM (bin registers
//! sched::run_unit_tests; the framework's pass/fail counters are
//! shared, so the pins land in the same suite totals).

use rux::sched::{CfsRunQueue, LoadWeight, SchedEntity};

pub fn test_fair_pins() {
    rux::tests::test_group_start("fair_pins");

    let lw = LoadWeight::from_nice(0);
    rux::test_assert_eq!(lw.weight, 1024, "nice 0 weight 1024");
    let lw_high = LoadWeight::from_nice(-20);
    rux::test_assert!(lw_high.weight > lw.weight, "nice -20 heavier");
    let lw_low = LoadWeight::from_nice(19);
    rux::test_assert!(lw_low.weight < lw.weight, "nice 19 lighter");

    let se = SchedEntity::new();
    let delta = 1_000_000u64; // 1ms
    let vruntime = se.calc_delta_fair(delta);
    rux::test_assert_eq!(vruntime, delta, "nice 0 vruntime equals runtime");

    let rq = CfsRunQueue::new();
    rux::test_assert!(rq.is_empty(), "fresh cfs rq empty");
    rux::test_assert_eq!(rq.nr_running(), 0, "fresh cfs rq nr_running 0");
    rux::tests::test_pass("sched-fair");
}
