// Rux Kernel: Scheduler Enqueue/Dequeue Consistency Model
//
// Verifies: nr_running consistency — every enqueue increments, every dequeue decrements
//
// Abstracted from:
//   kernel/src/sched/sched.rs — GlobalRunQueue, enqueue_task, dequeue_task
//
// Key invariants (INV-SCHED-1 through INV-SCHED-3):
//   INV-SCHED-1: nr_running >= 0 at all times
//   INV-SCHED-2: enqueue increments nr_running, dequeue decrements it
//   INV-SCHED-3: GRQ lock guards all nr_running modifications

#define MAX_TASKS   4
#define MAX_RUNNING 4

// Global state
byte grq_lock   = 0;
byte nr_running = 0;

// Per-task state: 0=RUNNING, 1=INTERRUPTIBLE, 2=idle
byte t0 = 0;  // idle task, always RUNNING
byte t1 = 2;
byte t2 = 2;
byte t3 = 2;

// Task that cycles between running and sleeping
proctype TaskCycle()
{
    byte state = 2;  // initial: idle
    byte tid;

    // Use _pid to distinguish tasks (2,3,4 since init=0, scheduler=1)
    tid = _pid - 2;  // gives 0, 1, or 2

    do
    :: true ->
        // Wake up: enqueue under GRQ lock
        atomic { (grq_lock == 0) -> grq_lock = 1 };
        state = 0;  // RUNNING
        nr_running = nr_running + 1;
        assert(nr_running <= MAX_RUNNING);
        grq_lock = 0;

        // Run for a while
        skip;
        skip;

        // Sleep: dequeue under GRQ lock
        atomic { (grq_lock == 0) -> grq_lock = 1 };
        state = 1;  // INTERRUPTIBLE
        nr_running = nr_running - 1;
        assert(nr_running >= 0);
        grq_lock = 0;

        // Wait to be woken again
        do
        :: true -> break;
        od;
    :: skip -> break;
    od;
}

// Scheduler: timer interrupt triggers schedule
proctype Scheduler()
{
    do
    :: nr_running > 0 ->
        // Timer interrupt -> schedule
        atomic { (grq_lock == 0) -> grq_lock = 1 };

        // pick_next_task, context_switch
        skip;

        grq_lock = 0;
    :: skip -> break;
    od;
}

// LTL: nr_running is always in valid range
ltl NrRunningValid {
    [](nr_running >= 0 && nr_running <= MAX_RUNNING)
}

// LTL: no permanent starvation (simplified)
ltl NoPermanentStarvation {
    [](nr_running > 0 -> <> nr_running < MAX_RUNNING)
}

init {
    // t0 (idle task) is always running
    nr_running = 1;

    atomic {
        run TaskCycle();  // _pid=2
        run TaskCycle();  // _pid=3
        run TaskCycle();  // _pid=4
        run Scheduler();  // _pid=5
    }
}
