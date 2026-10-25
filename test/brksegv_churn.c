// brksegv_churn.c — deterministic do_exit-hang / preempt-count-leak repro.
//
// Repro target (ltp lane's brkseq handoff, leg-19): a SIGSEGV-killed
// user task never finishes dying cleanly — TERM-DESCHED engages, the
// half-dead entry lingers (W3-RUNWAKE-REFUSED state=0x0 curr_on=true),
// and afterwards a CPU's idle task (or a real task) carries a leaked
// preempt_count (PREEMPT-LEAK pc=0x10000-class, held for minutes; the
// pure-HARDIRQ shape the PREEMPT-HEAL's base!=0 gate never cleared).
//
// Engine, mirroring brkseq exactly: fork a child that grows the break,
// touches the new page, shrinks the break below the touched page, then
// writes through the freed page — the write faults above the current
// break and the default SIGSEGV action kills the child from user mode
// (no handler installed: the kernel's SIGDEATH/do_exit path runs).
// The parent churns N such deaths, then idles so the driver can watch
// the kernel-side leak tripwire: without the fix the PREEMPT-LEAK
// held_nonzero_for keeps growing past the 10 s heal window; with it a
// PREEMPT-HEAL line fires and the leak clears.
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef ROUNDS
#define ROUNDS 12
#endif

static void one_death(void) {
    pid_t pid = fork();
    if (pid < 0) {
        return;
    }
    if (pid == 0) {
        // Child: grow, touch, shrink, then write through the freed page.
        uintptr_t b0 = (uintptr_t)sbrk(0);
        uintptr_t top = b0 + 0x3000;
        if ((uintptr_t)brk((void *)top) != top) {
            _exit(42);
        }
        *(volatile char *)top = 1;   // touch the new page
        if ((uintptr_t)brk((void *)(b0 + 0x1000)) != b0 + 0x1000) {
            _exit(43);               // shrink below it
        }
        // Deterministic SIGSEGV: write above the shrunk break. This is
        // the brkseq fault (their probe faulted at b0+0x4020).
        *(volatile char *)(b0 + 0x4000) = 1;
        _exit(44);                   // never reached
    }
    int st = 0;
    waitpid(pid, &st, 0);
    printf("BRKSEQ-ROUND child=%d status=0x%x\n", pid, st);
    fflush(stdout);
}

int main(void) {
    setbuf(stdout, NULL);
    printf("BRKSEQ-START rounds=%d pid=%d\n", ROUNDS, (int)getpid());
    for (int i = 0; i < ROUNDS; i++) {
        one_death();
        usleep(300 * 1000);
    }
    printf("BRKSEQ-CHURN-DONE\n");
    // Park so the kernel's tick-side leak tripwire has its observation
    // window; the driver verdicts from the serial stream.
    for (;;) {
        sleep(5);
        printf("BRKSEQ-IDLE\n");
    }
    return 0;
}
