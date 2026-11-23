# BUG REPORT: read_volatile alignment panics in the reduced-unsafe kernel

Filed by: coverage campaign (cov-metrics worktree), 2026-11-21
Owner: unsafe-slim lane (the new volatile primitives in kernel/src/safety/
and the converted call sites)
Severity: blocks the userspace kselftest coverage layer entirely
Status: OPEN — two independent signatures, repro 100%

## Symptom

`ptr::read_volatile requires that the pointer argument is aligned` —
kernel panic ("unsafe precondition violated", the compiler-inserted
alignment check in debug builds) during the userspace coverage battery
boot (`init=/test/kselftest`).

The printed Sepc/registers are the PANIC HANDLER's own state (the
handler calls debug::backtrace::save_regs itself), so the original
offending site cannot be symbolized from the log — the panic overwrites
it. Two independent signatures observed:

1. fork signature: `kselftest: start` -> park walk -> panic. The first
   fork() in the battery (futex wake pair) reaches fork -> dup_mm ->
   MmStruct::saved_auxv() (mm_struct.rs:1291) — the only saved_auxv
   caller on this path. The suite never panics because it never execs
   or forks a real process.
2. socketpair signature (fork tests gated out): battery passes pipe,
   memfd, mmap/mprotect, then panics at the AF_UNIX socketpair cluster.
   Original site lost to the same handler-overwrite.

Boot logs (both signatures):
- cov-metrics worktree: run/coverage-kselftest/boot.log (latest)
- repro: scripts/coverage-kselftest.sh (builds the cov-dump kernel,
  boots init=/test/kselftest, extracts the profile)

## Why the in-kernel suite does not catch it

The unit-test boot runs without unit-test-independent codegen differences
AND never forks a real userspace process; the battery is the first real
fork/socketpair driver on the reduced kernel. The kernel suite (1598
tests) passes on the same tree.

## What the coverage campaign needs

The kselftest program is the coverage driver for the syscall/ABI layer
(syscall/ipc/sync at 5-10% vs the suite's fs 30%). Until the alignment
regressions are fixed, the battery cannot complete a boot, so those
subsystems cannot be measured or improved.

## Suggested fix direction (unsafe-slim lane)

The new volatile primitives (kernel/src/safety/ — CopyBytes/KernelStack
Window/edges read_volatile_u64/u32) and their converted call sites
should audit pointer alignment at the CALLER (the INV contracts the
primitives already document assume aligned inputs). The two signatures
implicate at least: the fork/dup_mm path (signature 1) and the AF_UNIX
socketpair path (signature 2). A debug-build panic IS the desired
behavior for a misaligned volatile read — the bug is the caller passing
a misaligned pointer, not the check firing.
