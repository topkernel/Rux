use crate::fs::bio::BufferState;
use super::{test_pass, test_fail, test_group_start};

pub fn test_buffer_state() {
    test_group_start("buffer_state");

    // Test 1: Default state
    let state = BufferState::new();
    test_assert!(!state.is_dirty(), "new state not dirty");
    test_assert!(!state.is_locked(), "new state not locked");
    test_assert!(!state.is_uptodate(), "new state not uptodate");
    test_assert!(!state.is_mapped(), "new state not mapped");

    // Test 2: Set dirty
    let mut state = BufferState::new();
    state.set(1); // BH_Dirty
    test_assert!(state.is_dirty(), "set BH_Dirty → is_dirty()");

    // Test 3: Clear dirty
    state.clear(1); // BH_Dirty
    test_assert!(!state.is_dirty(), "clear BH_Dirty → !is_dirty()");

    // Test 4: Set lock
    let mut state = BufferState::new();
    state.set(2); // BH_Lock
    test_assert!(state.is_locked(), "set BH_Lock → is_locked()");

    // Test 5: Set uptodate
    let mut state = BufferState::new();
    state.set(0); // BH_Uptodate
    test_assert!(state.is_uptodate(), "set BH_Uptodate → is_uptodate()");

    // Test 6: Set mapped
    let mut state = BufferState::new();
    state.set(4); // BH_Mapped
    test_assert!(state.is_mapped(), "set BH_Mapped → is_mapped()");

    // Test 7: Multiple bits don't interfere
    let mut state = BufferState::new();
    state.set(1); // BH_Dirty
    state.set(2); // BH_Lock
    test_assert!(state.is_dirty() && state.is_locked(), "dirty && locked");
    state.clear(1); // clear dirty
    test_assert!(!state.is_dirty() && state.is_locked(), "!dirty && still locked");

    // Test 8: test() method
    let mut state = BufferState::new();
    state.set(1); // BH_Dirty
    test_assert!(state.test(1), "test(BH_Dirty) == true");
    test_assert!(!state.test(2), "test(BH_Lock) == false");

    // Test 9: Set clear set again
    let mut state = BufferState::new();
    state.set(0);
    state.clear(0);
    state.set(0);
    test_assert!(state.is_uptodate(), "set-clear-set uptodate");

    // Test 10: All bits set simultaneously
    let mut state = BufferState::new();
    state.set(0); // Uptodate
    state.set(1); // Dirty
    state.set(2); // Lock
    state.set(3); // Req
    state.set(4); // Mapped
    test_assert!(state.is_uptodate() && state.is_dirty() && state.is_locked() && state.is_mapped(),
        "all bits set simultaneously");
}
