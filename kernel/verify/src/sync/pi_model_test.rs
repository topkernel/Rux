//! Scenario battery for the PI chain model (surfaces as one suite in
//! the kernel-side `cargo test`).

use super::pi_model::*;

fn assert_clean(w: &PiWorld) {
    if let Some(v) = w.invariant_violation() {
        panic!("{}", v);
    }
}

#[test]
fn nice_floor_never_raises() {
    let mut w = PiWorld::new();
    let owner = w.task(Rank::Cfs(0));
    let waiter = w.task(Rank::Cfs(10));
    let l = w.lock(owner);
    assert!(w.block_on(waiter, l));
    // The CFS top waiter at nice 10 must NOT pull the nice-0 owner up.
    assert_eq!(w.tasks[owner].live, Rank::Cfs(0));
    assert_clean(&w);
}

/// The mission's "nested locks >= 3 deep" shape, taken one further:
/// A(CFS) holds L0 <- B(10) holds L1 <- C(20) holds L2 <- D(99).
/// The boost from D must walk three edges down to A.
#[test]
fn four_deep_chain_boosts_to_the_bottom() {
    let mut w = PiWorld::new();
    let a = w.task(Rank::Cfs(0));
    let b = w.task(Rank::Rt(10));
    let c = w.task(Rank::Rt(20));
    let d = w.task(Rank::Rt(99));
    let l0 = w.lock(a);
    let l1 = w.lock(b);
    let l2 = w.lock(c);

    assert!(w.block_on(b, l0)); // B queues on L0
    w.chain_walk(b); // A -> 10
    assert_eq!(w.tasks[a].live, Rank::Rt(10));

    assert!(w.block_on(c, l1)); // C queues on L1
    w.chain_walk(c); // refresh C@20, B -> 20, refresh B@20, A -> 20
    assert_eq!(w.tasks[a].live, Rank::Rt(20));
    assert_eq!(w.tasks[b].live, Rank::Rt(20));

    assert!(w.block_on(d, l2)); // D queues on L2
    w.chain_walk(c); // the park-path walk from D's edge's owner chain
    assert_eq!(w.tasks[a].live, Rank::Rt(99), "boost reached A through 3 edges");
    assert_eq!(w.tasks[b].live, Rank::Rt(99));
    assert_eq!(w.tasks[c].live, Rank::Rt(99));
    assert_clean(&w);
}

/// A DEBOOST propagates: the top waiter leaves; the middle relaxes.
/// The outer owner keeps a serving (sticky) boost until its own last
/// waiter leaves — the kernel's `already` short-circuit; priority is
/// never wrong-LOW (documented divergence from the reference's exact
/// re-set, run/RT-R6.md).
#[test]
fn deboost_propagates() {
    let mut w = PiWorld::new();
    let a = w.task(Rank::Cfs(0));
    let b = w.task(Rank::Rt(40));
    let c = w.task(Rank::Rt(90));
    let l0 = w.lock(a);
    let l1 = w.lock(b);
    assert!(w.block_on(b, l0));
    assert!(w.block_on(c, l1));
    w.chain_walk(b);
    assert_eq!(w.tasks[a].live, Rank::Rt(90));
    // C leaves L1's queue: B relaxes to its base 40...
    w.unblock(c);
    w.chain_walk(b);
    assert_eq!(w.tasks[b].live, Rank::Rt(40), "B deboosted");
    // ...A keeps the serving 90 until its own waiter (B) leaves.
    assert_eq!(w.tasks[a].live, Rank::Rt(90), "sticky until drain");
    w.unblock(b);
    assert_eq!(w.tasks[a].live, Rank::Cfs(0), "A fully restored at drain");
    assert_clean(&w);
}
