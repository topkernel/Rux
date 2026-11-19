// Rux Kernel: Lock Ordering Verification Model
//
// Verifies: no deadlock cycle in lock acquisition
//
// Abstracted from:
//   kernel/src/sync/spinlock.rs   — lock hierarchy documentation (lines 17-48)
//   kernel/src/sched/sched.rs     — GRQ lock
//   kernel/src/sync/futex.rs      — bucket lock, waiter slot lock
//
// Hierarchy:
//   Level 0: IRQ disable
//     Level 1: preempt_disable
//       Level 2a: GRQ lock
//       Level 2b: futex hash bucket lock
//         Level 3: waiter slot lock
//
// Key nesting (INV-LOCK-5):
//   bucket_lock -> GRQ lock (via wake_up -> enqueue_task)
//   GRQ -> bucket is FORBIDDEN (would create deadlock cycle).

// Lock state: 0=free, 1=held
byte irq_lock    = 0;
byte preempt_cnt = 0;
byte grq_lock    = 0;
byte bucket_lock = 0;
byte waiter_lock = 0;

// Process 1: Scheduler path
//   irq_save -> preempt_disable -> GRQ lock
proctype Scheduler()
{
    // irq_save
    atomic { (irq_lock == 0) -> irq_lock = 1 };
    // preempt_disable
    preempt_cnt = preempt_cnt + 1;
    // GRQ lock
    atomic { (grq_lock == 0) -> grq_lock = 1 };

    // Scheduler work
    skip;

    // Release: reverse order
    grq_lock = 0;
    preempt_cnt = preempt_cnt - 1;
    irq_lock = 0;
}

// Process 2: Futex wake path
//   irq_save -> preempt_disable -> bucket_lock -> waiter_slot -> GRQ lock
proctype FutexWake()
{
    // irq_save
    atomic { (irq_lock == 0) -> irq_lock = 1 };
    // preempt_disable
    preempt_cnt = preempt_cnt + 1;
    // bucket_lock
    atomic { (bucket_lock == 0) -> bucket_lock = 1 };
    // waiter_slot_lock
    atomic { (waiter_lock == 0) -> waiter_lock = 1 };

    // Inspect waiter
    skip;

    // Release waiter slot
    waiter_lock = 0;

    // Task::wake_up -> enqueue_task -> GRQ lock
    // GRQ nests INSIDE bucket (valid per INV-LOCK-5)
    atomic { (grq_lock == 0) -> grq_lock = 1 };

    // enqueue_task_locked
    skip;

    // Release GRQ
    grq_lock = 0;
    // Release bucket
    bucket_lock = 0;
    // preempt_enable
    preempt_cnt = preempt_cnt - 1;
    // irq_restore
    irq_lock = 0;
}

// Process 3: Futex wait path
//   irq_save -> preempt_disable -> bucket_lock -> waiter_slot
//   Then drop bucket -> schedule (GRQ lock)
proctype FutexWait()
{
    // irq_save
    atomic { (irq_lock == 0) -> irq_lock = 1 };
    // preempt_disable
    preempt_cnt = preempt_cnt + 1;
    // bucket_lock
    atomic { (bucket_lock == 0) -> bucket_lock = 1 };
    // waiter_slot_lock
    atomic { (waiter_lock == 0) -> waiter_lock = 1 };

    // Initialize waiter, set INTERRUPTIBLE
    skip;

    // Release waiter slot
    waiter_lock = 0;

    // Set state to INTERRUPTIBLE under bucket lock
    skip;

    // Release bucket
    bucket_lock = 0;

    // schedule() acquires GRQ lock (bucket already released)
    atomic { (grq_lock == 0) -> grq_lock = 1 };

    // pick_next_task, context_switch
    skip;

    // Release GRQ
    grq_lock = 0;
    // preempt_enable
    preempt_cnt = preempt_cnt - 1;
    // irq_restore
    irq_lock = 0;
}

// Process 4: Memory management path
//   irq_save -> preempt_disable -> GRQ lock
proctype MmAlloc()
{
    // irq_save
    atomic { (irq_lock == 0) -> irq_lock = 1 };
    // preempt_disable
    preempt_cnt = preempt_cnt + 1;
    // GRQ lock
    atomic { (grq_lock == 0) -> grq_lock = 1 };

    // Zone-level allocation
    skip;

    // Release
    grq_lock = 0;
    preempt_cnt = preempt_cnt - 1;
    irq_lock = 0;
}

// SPIN automatically detects deadlocks (invalid end states) with -a flag.
// No explicit LTL needed — if any await blocks forever, SPIN reports it.

init {
    atomic {
        run Scheduler();
        run FutexWake();
        run FutexWait();
        run MmAlloc();
    }
}
