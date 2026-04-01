//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::sync::semaphore::{Semaphore, Mutex};
use super::{test_pass, test_fail, test_group_start};

pub fn test_semaphore() {
    test_group_start("semaphore");

    // Test 1: Semaphore::new with positive value
    let sem = Semaphore::new(1);
    test_assert_eq!(sem.count(), 1, "Semaphore::new(1).count() == 1");

    // Test 2: Semaphore::new with zero
    let sem = Semaphore::new(0);
    test_assert_eq!(sem.count(), 0, "Semaphore::new(0).count() == 0");

    // Test 3: Semaphore::new with larger value
    let sem = Semaphore::new(5);
    test_assert_eq!(sem.count(), 5, "Semaphore::new(5).count() == 5");

    // Test 4: down_trylock on available semaphore
    let sem = Semaphore::new(1);
    let result = sem.down_trylock();
    test_assert!(result.is_ok(), "down_trylock on available semaphore succeeds");
    test_assert_eq!(sem.count(), 0, "count decremented after down_trylock");

    // Test 5: down_trylock on unavailable semaphore
    let sem = Semaphore::new(0);
    let result = sem.down_trylock();
    test_assert!(result.is_err(), "down_trylock on unavailable semaphore fails");
    test_assert_eq!(sem.count(), 0, "count unchanged after failed down_trylock");

    // Test 6: up restores count
    let sem = Semaphore::new(0);
    sem.up();
    test_assert_eq!(sem.count(), 1, "up() increments count");

    // Test 7: Multiple down_trylock
    let sem = Semaphore::new(3);
    let r1 = sem.down_trylock();
    let r2 = sem.down_trylock();
    let r3 = sem.down_trylock();
    let r4 = sem.down_trylock(); // Should fail
    test_assert!(r1.is_ok() && r2.is_ok() && r3.is_ok(), "3 down_trylock on sem(3) succeed");
    test_assert!(r4.is_err(), "4th down_trylock on sem(3) fails");
    test_assert_eq!(sem.count(), 0, "count == 0 after 3 successful down_trylock");

    // Test 8: up after down restores
    let sem = Semaphore::new(1);
    let _ = sem.down_trylock();
    sem.up();
    let result = sem.down_trylock();
    test_assert!(result.is_ok(), "down_trylock succeeds after up");

    // Test 9: Mutex::new
    let mutex = Mutex::new();
    test_assert!(true, "Mutex::new() compiles");

    // Test 10: Mutex try_lock
    let mutex = Mutex::new();
    let guard = mutex.try_lock();
    test_assert!(guard.is_ok(), "Mutex::try_lock() on unlocked mutex succeeds");
    // try_lock returns Result<(), ()>, not a guard — must unlock explicitly
    mutex.unlock();

    // Test 11: Mutex try_lock while held
    let _ = mutex.try_lock(); // Lock it
    // Should fail because mutex is held
    let guard2 = mutex.try_lock();
    test_assert!(guard2.is_err(), "Mutex::try_lock() fails while held");
    mutex.unlock(); // Release
    // Should succeed after unlock
    let guard3 = mutex.try_lock();
    test_assert!(guard3.is_ok(), "Mutex::try_lock() succeeds after unlock");
    mutex.unlock();
}
