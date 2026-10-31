# Lane: gdm three blockers (Ubuntu line)
Work autonomously; never ask; commit every step. Worktree = THIS (merged origin/main already at 8419a9a1; dirty 0). Ubuntu image: work/ubuntu-gui.img in the MAIN repo is now a symlink to the CENTRAL STORE /home/william/rux-images/ubuntu/ubuntu-gui.img (86M stub — if unusable, `make ubuntu-image` rebuilds; large rebuilds' OUTPUT should also go to the store + symlink back, never raw inside a worktree). Campaign knowledge in this worktree's run/ (GNOME-SESSIONS.txt, gdm-* scripts, PAM 89-cycle chain).

## Registered blockers (priority)
1. devfs symlink (CanGraphical gate): gdm's CanGraphical probes a /dev path that must BE a symlink (like /dev/fd -> /proc/self/fd); devfs serves real nodes. Find the exact path (campaign logs run/), give devfs the symlink form (fs/devfs R-numbered prior art).
2. netlink sendmsg multicast: rtnetlink/uevent multicast membership/send shape returns an error udev/systemd treats as fatal (campaign notes in run/ + git log).
3. console-VT: the VT ioctl path gdm needs (campaign notes; syscall/io.rs freshly audited by the aarch64 campaign — check its current state first).

## Ladder
1. Inventory + merge reconciliation (aarch64-era shared changes may have shifted these). 2. Ubuntu boot to login/gdm stage on current main (regression-check the old milestone). 3. Blocker 1 fixed, CanGraphical passes. 4. Blockers 2-3 as allowed; goal line = gdm login prompt (greeter).

## Discipline
-smp 2 -m 1G (SMP4/2G final validation only); ONE QEMU at a time (check ps — 8 lanes live); shared images -snapshot + file.locking=off ONLY; kill only yours; pkill FORBIDDEN. x3 builds rc=0; riscv make test green. Small commits, English, NEVER "Linux". Push to the lane branch.
Final message: per-blocker status + evidence, milestone, commits, build/test status.
