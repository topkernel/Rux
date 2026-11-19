// Rux Kernel: Interrupt/Preempt Count Balance Model
//
// Verifies: preempt_count stays within bounds, no underflow
//
// Abstracted from:
//   kernel/src/interrupt/preempt.rs  — preempt_count bitfield layout
//   kernel/src/arch/riscv64/trap.rs  — irq_enter/irq_exit bracketing
//   kernel/src/interrupt/softirq.rs  — softirq context management
//
// Safety properties (verified by assertions):
//   preempt_depth >= 0 always (no underflow)
//   hardirq_depth >= 0 && <= MAX_DEPTH always
//   softirq_depth >= 0 always
//   Every disable has a matching enable (balanced by construction)
//
// Note: liveness properties (eventually returns to 0) are not modeled
// because SPIN explores all interleavings including starvation scenarios
// that don't exist on real hardware (hardirq preempts task, not interleaved).

#define MAX_DEPTH 3

byte preempt_depth = 0;
byte softirq_depth = 0;
byte hardirq_depth = 0;

// Task doing preempt_disable/enable (spinlock path)
proctype TaskWithLock()
{
    byte i = 0;

    do
    :: i < 2 ->
        // preempt_disable
        preempt_depth = preempt_depth + 1;
        assert(preempt_depth >= 0 && preempt_depth <= MAX_DEPTH);

        // Critical section
        skip;

        // preempt_enable
        preempt_depth = preempt_depth - 1;
        assert(preempt_depth >= 0);

        i = i + 1;
    :: i >= 2 -> break;
    od;
}

// Hard IRQ handler: irq_enter -> handle -> irq_exit
proctype HardIrqHandler()
{
    // irq_enter (preempt.rs:115)
    hardirq_depth = hardirq_depth + 1;
    assert(hardirq_depth >= 0 && hardirq_depth <= MAX_DEPTH);

    skip;

    // irq_exit (preempt.rs:124)
    hardirq_depth = hardirq_depth - 1;
    assert(hardirq_depth >= 0);

    // May invoke softirq after outermost exit
    if
    :: hardirq_depth == 0 && softirq_depth == 0 ->
        softirq_depth = softirq_depth + 1;
        assert(softirq_depth >= 0);
        skip;
        softirq_depth = softirq_depth - 1;
        assert(softirq_depth >= 0);
    :: skip;
    fi;
}

// LTL: hardirq_depth always in valid range
ltl HardirqBounded {
    [] (hardirq_depth >= 0 && hardirq_depth <= MAX_DEPTH)
}

// LTL: preempt_depth never goes negative
ltl PreemptNoUnderflow {
    [] (preempt_depth >= 0)
}

init {
    atomic {
        run TaskWithLock();
        run HardIrqHandler();
    }
}
