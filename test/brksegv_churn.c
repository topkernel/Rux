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
#include <sys/syscall.h>
#include <errno.h>

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
        // NOTE: brk(2) is driven via the raw syscall with its raw return
        // (the new break). musl's brk() LIBC WRAPPER is a permanent
        // -ENOMEM stub, and glibc's returns 0/-1 — neither can express
        // "did the kernel move the break where I asked" (round 1 of this
        // probe exited 42 on every child through the musl stub and never
        // faulted). SYS_brk = 214 on riscv64.
        uintptr_t b0 = (uintptr_t)sbrk(0);
        uintptr_t top = b0 + 0x3000;
        uintptr_t got = (uintptr_t)syscall(214, top);
        if (got != top) {
            printf("BRKSEQ-GROW-FAIL b0=%p want=%p got=%p\n",
                   (void *)b0, (void *)top, (void *)got);
            _exit(42);
        }
        *(volatile char *)top = 1;   // touch the new page
        got = (uintptr_t)syscall(214, b0 + 0x1000);
        if (got != b0 + 0x1000) {
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
    if (getpid() != 1) {
        // Running as a SHELL child (pid != 1; note the shell itself can BE
        // pid 1 under init=/bin/sh, so ppid is not a discriminator): exit
        // immediately — parking on stdin here would deadlock the
        // shell's sequential command list. The console-park stage is
        // for the init context only.
        printf("BRKSEQ-EXIT\n");
        return 0;
    }
    // The ltp lane's wedge shape: after the SIGSEGV deaths, /bin/sh
    // parks reading the console and NEVER responds to the driver's next
    // input line. Park on stdin exactly like the shell's read loop; the
    // driver feeds a line after CHURN-DONE and the verdict wants the
    // echoed response within the window.
    printf("BRKSEQ-READY-FOR-INPUT\n");
    char line[64];
    while (fgets(line, sizeof(line), stdin) != NULL) {
        printf("BRKSEQ-GOT: %s", line);
        if (strncmp(line, "quit", 4) == 0) {
            break;
        }
    }
    printf("BRKSEQ-CONSOLE-CLOSED\n");
    // Park so the kernel's tick-side leak tripwire has its observation
    // window; the driver verdicts from the serial stream.
    for (;;) {
        sleep(5);
        printf("BRKSEQ-IDLE\n");
    }
    return 0;
}
