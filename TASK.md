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

## Handoff note (coordinator, 02:3x model switch)
- run/oh-userdata-rw.img is a PRIVATE 16GB rw copy created by the last run (first-boot state repro; boot with USERDATA=run/oh-userdata-rw.img + snapshot off IF you need the mutated /data state — the tracked symlink userdata.img points to the central store seed and must stay READ-ONLY).
- Your predecessor's last boots: run/oh-sprint3-serial.log + sprint3 shots (still all black at 120/200s). The oracle-painter history question is still open in the charter.

## SCANOUT lane findings (2026-10-31, scanout sprint)
1. ORACLE QUESTION ANSWERED (was the charter's open question): every prior non-black
   screenshot was the kernel-side oracle checkerboard (commit 4276946e "scanout binding
   PROVEN" = kernel-painted banded checker via kva into the composer's GEMs; painter
   removed in 18eaa659). run/oh-shotmon-MILESTONE shows 8 oracle colors = 100% pixels.
   Userspace has NEVER produced a single non-black pixel through the whole project.
   Reframe: kernel binding is fine; the gap is userspace render content.
2. Sprint4-7 early wedge (~pid 310, PREEMPT-UNDERFLOW spam) NOT reproducible on the
   current tree: r1 (run/oh-r1-serial.log) boots deep (ServiceStart 117+, pid 650+,
   5MB log). Likely the stale run/oh-img/userdata.img symlink (relinked 03:00). No
   kernel regression from 18b7c9cb/85230725/d30e1c81.
3. r1 milestone: bootanimation RAN its full 150-frame sequence ("play sequence frames
   end" x2 — Draw() executed; in sprint3 it never appeared = vsync never fired that
   boot). Vsync chain (kernel WAIT_VBLANK -> HDI DrmVsyncWorker -> RS sampler ->
   software generator) works, at least sometimes. All 4 r1 shots still pure black =>
   Skia/EGL draws land in NO visible memory (co2 verdict "CONTENT, not binding" stands;
   "clear-only buffers" now confirmed on a healthy animation run).
4. scanpaint (run/scanpaint.c + run/oh-boot-paint.sh, private ramdisk run/ramdisk-paint.img):
   static musl dumb-buffer painter injected as pid-1 side task, PAGE_FLIPs color bars
   at 30fps. Purpose: prove/kill the userspace->kernel scanout path with zero GL/Skia involved.
   Ladder: painter pattern visible => scanout OK from userspace, gap is OH render stack
   (EGL/GBM/Skia draw->buffer); painter pattern NOT visible => kernel dumb-buffer/flip
   path has a hole the composer path doesn't exercise.

## SCANOUT VERDICT (2026-10-31 04:0x) — PIXELS PROVEN; BLOCKER IS OH's MISSING GL DRIVER
1. **paint6/paint7: userspace pixels ON SCREEN.** run/oh-paint6-shot-*.ppm and
   run/oh-paint7-shot-*.ppm: 100% non-black (921600/921600) at 25/45/70/100/130s,
   distinct md5s = the painter's moving stripe = LIVE userspace presentation.
   Chain: card0 -> GETRESOURCES/GETCONNECTOR -> CREATE_DUMB(720x1280@32) -> MAP_DUMB
   -> mmap -> paint -> ADDFB2 -> PAGE_FLIP. No oracle, no OH render stack.
2. Injection mechanics (for reuse): rdinit=/rpaint where /rpaint is a **static ELF**
   shim (run/paintshim.c: fork painter + exec /init) — a #! script does NOT work
   (kernel init loader has no binfmt_script: "ELF load failed err=InvalidHeader
   first8=23212f62", silent fallthrough to /init). Painter logs via /dev/kmsg
   (klog() in scanpaint.c) — that is how its traces reach the serial log.
3. **Kernel ABI fix landed (this branch):** Rux encoded MODE_PAGE_FLIP as IOWR(dir 3)
   and MODE_RMFB as IOWR — upstream libdrm uses _IOW(dir 1) / _IOR(dir 2); real
   libdrm clients got ENOTTY (painter proved it: _IOW-encoded flip -> errno 25).
   Now both encodings dispatch (kernel/src/drivers/drm/mod.rs, *_LIBDRM constants).
   paint7 re-validated with the painter back on the upstream encoding: all 5 shots
   non-black, 150+ frames presented. Builds rc=0 on x86_64 + riscv64 + aarch64.
4. **The content root cause, precisely:** r1 log — MUSL-LDSO fails to load
   `libGLES_mali.so` (errno=2/11, file ABSENT from system.img AND vendor.img;
   verified with debugfs). OH's Mesa egl_dri2 (dri2_get_proc_address lines) loads,
   but the GLES driver does not exist => render_service and bootanimation have NO
   GL context => Skia GPU draws silently no-op => every surface stays at its
   clear color (opaque black, alpha 0xff — matches the co2 nz/clear=921600 probe).
   Vsync is FINE (r1: bootanimation ran its full 150-frame sequence; sprint3's
   stuck case was boot-state-dependent). Scanout binding was NEVER the blocker.
5. Routes to launcher-first-frame, ranked:
   a. OH-image: ship a working GL (Mesa llvmpipe/swrast built OH-style, installed
      as /vendor/lib64/chipsetsdk/libGLES_mali.so; impl symlinks already exist).
      Needs a private system/vendor.img copy (debugfs -w replace, ext4).
   b. OH-source: force RS + bootanimation onto CPU/Skia-raster surfaces (no GL).
      Needs oh-robot rebuild (out/x86_64_virt exists) + image repack.
   c. Kernel: virtio-gpu venus/virgl 3D contexts so Mesa's virtio driver renders —
      the "real" fix, large.
   d. Bridge (this lane, done): scanpaint doubles as a boot-splash stand-in — keep
      it flipping until RS produces real frames, if a visible screen is needed early.
6. Other kernel findings to ledger (not fixed here):
   - binfmt_script missing for the pid1 init path (see 2).
   - mksh-as-pid1 (rdinit=/bin/sh): interactive rescue shell hard-wedges the boot on
     a user-mode cow pagefault after a fork (run/oh-repl1-serial.log tail).
