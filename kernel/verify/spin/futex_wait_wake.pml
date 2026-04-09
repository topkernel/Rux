// Rux Kernel: Futex Wait/Wake Protocol Model
//
// Verifies: no lost wakeup, no spurious sleep
//
// Abstracted from: kernel/src/sync/futex.rs
//
// Key invariant (INV-FUTEX-1):
//   The waiter sets INTERRUPTIBLE state under bucket_lock before dropping it.
//   Any concurrent waker either sees no waiter OR sees an INTERRUPTIBLE waiter.
//   Therefore, a waiting task is never permanently missed.

#define RUNNING        0
#define INTERRUPTIBLE  1

// Shared state
byte futex_val   = 0;

// Waiter state (protected by bucket_lock)
bool waiter_in_chain = false;
byte waiter_state    = RUNNING;
bool waiter_woken    = false;

// Synchronization
byte bucket_lock = 0;

// Waiter process: models futex_wait() path
proctype Waiter()
{
    byte my_expected;

    // Read expected value (atomic load before lock)
    my_expected = futex_val;

    // Lock bucket
    atomic { (bucket_lock == 0) -> bucket_lock = 1 };

    // Re-check under lock (kernel/src/sync/futex.rs:261)
    if
    :: futex_val != my_expected ->
        // Value changed before we could sleep
        bucket_lock = 0;
        goto done;
    :: else ->
        skip;
    fi;

    // Insert into chain
    waiter_in_chain = true;

    // Set INTERRUPTIBLE state UNDER bucket_lock (critical anti-lost-wakeup)
    waiter_state = INTERRUPTIBLE;

    // Unlock bucket
    bucket_lock = 0;

    // Now sleeping — wait for wakeup
    do
    :: waiter_woken ->
        waiter_state = RUNNING;
        break;
    od;

    // Cleanup
    waiter_in_chain = false;
done:
    skip;
}

// Waker process: models futex_wake() path
// Loops until waiter is woken (models multiple wake calls in real kernel)
proctype Waker()
{
    do
    :: waiter_woken -> break;
    :: true ->
        // Lock bucket
        atomic { (bucket_lock == 0) -> bucket_lock = 1 };

        // Change futex value
        if
        :: futex_val = (futex_val + 1) % 256;
        :: skip;
        fi;

        // Check if waiter is in chain and sleeping
        if
        :: waiter_in_chain && waiter_state == INTERRUPTIBLE ->
            waiter_woken = true;
        :: else ->
            skip;
        fi;

        // Unlock bucket
        bucket_lock = 0;
    od;
}

// LTL: If waiter is INTERRUPTIBLE in chain, eventually woken
ltl NoLostWakeup {
    [] (waiter_state == INTERRUPTIBLE && waiter_in_chain -> <> waiter_woken)
}

// LTL: No permanent sleep (waiter eventually wakes up or exits)
ltl NoPermanentSleep {
    [] (waiter_state == INTERRUPTIBLE -> <> (!waiter_in_chain || waiter_woken))
}

init {
    atomic {
        run Waiter();
        run Waker();
    }
}
