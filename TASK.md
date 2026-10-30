# Lane: OH launcher — storage_daemon registration stall (hot evidence)
Work autonomously; never ask; commit every real step. Worktree: THIS one (already built: x86_64 bzImage; run/oh-boot.sh = battle-tested driver: q35 KVM, -m 4G -smp 2, serial-to-file, USERDATA/EXTRA_APPEND env; run/oh-img/* symlinks the image set from the CENTRAL STORE /home/william/rux-images/oh — READ-ONLY, boot with -snapshot + file.locking=off, NEVER write them).
IMAGES now live in /home/william/rux-images/ (central store, git-invisible). OH source tree: /home/william/workspace/oh-robot (grep freely).

## Evidence chain (verified on run/oh-probe1-serial.log, 25372 lines)
1. Boot goes deep: 185 ServiceStart, 634 SAMGR lines, 1114 appspawn/ability lines — the old seed-userdata hard-wedge is GONE on current main.
2. Boot ABORTED by OH init: "ServiceReap error critical service crashed storage_manager 4" -> "ExecReboot panic" -> StopAllServices -> sysrq 'c'. storage_manager (critical) exits cleanly (exit_group 0) repeatedly; 4 strikes -> panic reboot. LAUNCHER NEVER GETS ITS CHANCE.
3. Why storage_manager exits: StorageDaemonClient::CheckServiceStatus (storage_daemon/client/storage_daemon_client.cpp:58 in the OH tree) polls samgr CheckSystemAbility(STORAGE_DAEMON_SAID) ~15x200ms, then E_SERVICE_IS_NULLPTR -> main gives up -> exit(0). Log: "check storage daemon status 0 times".
4. Why the check fails: storage_daemon (pid 325) is ALIVE the whole boot (EXITGRP code=-9 only at shutdown sweep, line 25018) but NEVER SENDS its AddSystemAbility registration. Last exchange: SAREG txn14 send line 4362 ("ohos.samgr.accessTo..ken" capability query), samgr reply txn18 line 4369 — then SILENCE from pid 325. STUCK between handshake and registration.
5. Red herrings eliminated: "fails: nr29=-4" on every IPC worker's first call = ioctl(BINDER_WRITE_READ) interrupted once by a signal (nr post-translation generic; 0xc0306201=_IOWR('b',1,48)) — normal, retried fine. The sysrq 'c' is OH's own reboot path.

## Mission
Find WHERE/WHY storage_daemon's main parks after the txn18 reply. Tools: EXTRA_APPEND="dfx=taskdump,watchdog" boots (probe3 tried taskdump alone — no periodic dumps appeared; investigate the dump trigger in kernel/src/dfx/ — probe3-serial.log exists); per-task LASTSYS latch (last_syscall_nr/a0/a1) via taskdumps; gdb (-s + gdb-multiarch, attach-then-interrupt, symbols target/x86_64-unknown-none/debug/rux); compare with a HEALTHY registrant (hdf_devmgr pid 335 SAREG'd fine — what did it do next that 325 never reached?); OH source: storage_daemon main = foundation/filemanagement/storage_service/services/storage_daemon — read the startup sequence between the access-token check and AddSystemAbility. Suspects: (a) binder wake lost for pid 325's main (binder.rs thread.hist EV_WAKE_RET instrumentation exists); (b) ioctl/futex return mismatch -> userspace parked in a sleep-forever branch; (c) bounded wait stretched by timer/nanosleep inaccuracy; (d) parcel byte-order/offset corruption of the txn18 reply.

## Ladder
1. storage_daemon completes SA registration live (SAREG send from pid 325 after handshake). 2. CheckServiceStatus succeeds -> no critical-crash cascade -> NO ExecReboot. 3. Boot survives past the panic point; launcher/appspawn chain proceeds. 4. New stalls past that: diagnose with the same toolkit.

## Discipline
One QEMU at a time (check ps first — other lanes run); kill only your own PIDs; pkill FORBIDDEN. All three builds rc=0 before pushing. Small commits, English, NEVER the word "Linux" (use reference-kernel/POSIX). Push: origin/oh-launcher (branch = this worktree's). Quota deaths mid-run: commit early; a successor resumes from this file + git.
Final message: root cause + fix + ladder position (1-4), commits, build status.
