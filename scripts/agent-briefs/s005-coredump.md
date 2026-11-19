# Agent s005-coredump (worktree /home/william/rux-agents/s005-coredump, branch feature/s005-coredump)
Read /home/william/rux-agents/prompts/COMMON-X86.md first.

Mission: fix BUG-S005 (test/stress/BUGS.md) — after 2-3 children die by
core-dumping signals (SIGABRT with core), the PARENT shell dies with
SIGILL (even PID 1). Deterministic repro (~3 min, riscv64):
  boot rootfs (init=/bin/sh, -cpu rv64,zbb=true,zba=true,zbs=true, smp1 ok):
  i=0; while [ $i -lt 8 ]; do /test/linux-ltp/testcases/bin/abort01 >/dev/null 2>&1; echo "iter $i rc=$?"; i=$((i+1)); done
  -> iter0/iter1 ok, shell dies at iter2. abs01 (no core) loops clean 5+.

You own: kernel/src/process/{coredump.rs,exit.rs,wait.rs}, kernel/src/signal.rs,
kernel/src/fs (only if the coredump file-write path leads there), and the
riscv64 arch signal-frame files if the corruption is in frame restoration.
Do NOT touch the x86 exec path or drivers (other agents own them).

Investigation directions (ranked):
1. coredump.rs: the core writer reads child memory via phys_of/user pages
   and writes to a file — a wrong page mapping could WRITE into parent
   memory instead (the parent then executes garbage = SIGILL). Check the
   write destination and any phys_to_virt/virt_to_phys inversions; look
   for the file write path borrowing the PAGE CACHE page of the ELF
   binary being dumped (riscv rootfs is on ext4: the parent's /bin/sh
   text pages live in the page cache — if the core dump path dirties the
   file-backed page of the PARENT's text, the parent SIGILLs on next
   fetch!). That is the leading hypothesis: check
   coredump write / page-cache interaction, and whether dumping an
   anon-only child still corrupts (write a C test that aborts with no
   file-backed pages, run it in a loop from the shell).
2. The exit path's page-table teardown freeing a frame still mapped in
   the parent (shared file page freed then reused).
3. wait/SIGCHLD path scribbling the parent's signal state.

Verification: repro loop reaches iter7 with the shell alive, then rerun
the LTP runner (userspace/linux-ltp/output is built, inject via
`make rootfs`) and confirm run_ltp.sh proceeds past abort01 — report the
PASS/FAIL counts of the first ~50 tests as the S005-fix evidence.
Gates: riscv64 build+boot clean. NEVER merge to main/feature/x86-64.
