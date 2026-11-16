# newmount lane, round 2 — session of 2026-11-16 (resumed after the 02:20 quota wall)

Branch: `newmount2` @ e6cce7ef4 (3 new commits on top of c62dc92a2).
Mission from handoff: "run the finish-line boot — the five blockers'
state after your fixes ... FINISH LINE: serial login prompt on 15+ min
boot". Predecessor's last state: mounts clean in the lab, finish-line
boot starting.

> ROUND 3 UPDATE (same day, afternoon): **FINISH LINE REACHED** on the
> integrated tree — see the bottom section. Branch now @ 05d0d2b2c.

## What actually happened (short version)

The finish-line boot still ends with `dev-ttyS0.device` timing out and
`serial-getty@ttyS0.service` dead — but the blocking layer moved twice
this session. Two real kernel ABI gaps sat UNDER the whole udev chain
and are now fixed and verified piecewise; the one thing left is a
kernel-internal stall family that other lanes own (fdreap-close / RCU
grace monopolist convoy under the udevd worker fork storm), which only
bites under full-boot concurrency.

## The two ABI gaps found and fixed (with the evidence chain)

### 1. sysfs statfs did not report SYSFS_MAGIC (commit 2078af973)

`udevadm trigger /sys/class/tty/ttyS0` answered "No such device".
Debug run (`SYSTEMD_LOG_LEVEL=debug udevadm info`):

```
sd-device: the syspath "/sys/devices/platform/serial8250/tty/ttyS0"
is outside of sysfs, refusing.
```

systemd 255 `device_set_syspath()` runs `fd_is_fs_type(fd,
SYSFS_MAGIC)` (SYSTEMD_DEVICE_VERIFY_SYSFS default-on) and returns
ENODEV for any other f_type. Our `fill_statfs_for_inode` had branches
for proc/tmpfs/debugfs/tracefs/mqueue/... but NOT sysfs — the rootfs
fallback magic leaked. Effect: EVERY sd-device consumer failed; udevd
could not open any device from its coldboot-replay "add" uevents, so
no udev db, no processed-device "change" broadcast, dev-ttyS0.device
never plugged. After the fix: `udevadm info /sys/class/tty/ttyS0`
returns P:/M:/U:tty/D:c 4:64/N:ttyS0/E:* correctly.

### 2. netlink receive ignored MSG_PEEK/MSG_TRUNC — the size probe ATE every uevent (commit e6cce7ef4)

After (1), udevd still processed nothing; `udevd --debug` showed:

```
sd-device-monitor(manager): Invalid message length (0), ignoring message.
```

705,498 times in one quiet boot. systemd's `next_datagram_size_fd()`
calls `recv(fd, NULL, 0, MSG_PEEK|MSG_TRUNC)` before EVERY monitor
receive and sizes the buffer from the return. Our netlink receive
popped the front datagram, copied 0 bytes, returned 0 — each event was
destroyed by its own size probe. Fixes in the commit:

- `netlink_recv_ext()`: peek keeps the datagram queued; report_full
  returns skb->len (reference datagram recvmsg rule); scatter copies
  only staged bytes.
- recvmsg/recvfrom AF_NETLINK arms plumb both flags.
- `recv(fd, NULL, 0, ...)`: NULL buffer + len 0 is a legal probe shape
  (no more blanket EFAULT; netlink answers the size from the head).
- FIONREAD on netlink reports the FRONT datagram size (Linux SIOCINQ
  per-message contract — sd-device's fallback).

Verified end-to-end in the quiet sh-boot (INIT=/bin/sh):
`systemd-udevd --daemon` now processes every coldboot device —
/run/udev/data fills (11 entries incl. c4:64), ttyS0's db entry is
`I:54025436 / E:ID_MM_CANDIDATE=1 / G:systemd` — the tagged state
dev-ttyS0.device needs to plug. Also verified the socket-activation
side: udevd adopting an inherited kernel-socket fd (LISTEN_FDS via
exec, SO_PASSCRED set) spawns workers and processes (this is the shape
systemd uses via systemd-udevd-kernel.socket `ListenNetlink=
kobject-uevent 1` + `Sockets=`).

## Five blockers — state on the final tree (boots serial-093658 / -093952)

1. **libmount mount units — partial.** sys-kernel-debug.mount PASSES.
   dev-mqueue.mount and sys-kernel-tracing.mount FAIL, and this boot
   finally shows the reason line: "Failed to drain libmount events:
   Invalid argument" (libmount's utab monitor). Manual repro in the
   sh-boot is CLEAN (`mount -t mqueue mqueue /dev/mqueue -o
   nosuid,nodev,noexec` → rc=0, entry visible in /proc/self/mountinfo
   with correct fields), so the mount(2)/mountinfo side is fine; the
   failing piece is libmount's userspace table bookkeeping. Two known
   gaps feed it: (a) statx has NO STATX_MNT_ID — libmount's
   create_fs hook does `statx(fd_tree,"",AT_EMPTY_PATH,STATX_MNT_ID)`
   and takes `st.stx_mnt_id` as the fs id (falls back to 0);
   (b) /run/mount/utab monitor inotify. → **ub2404-login lane's
   STATX_MNT_ID work is exactly this** (see Coordination below).
   The stray `mount_setattr(dfd=-9, path=NULL) → EINVAL` seen between
   the mount and the unit verdict is a probe+fallback shape (libmount
   retries with path "" and succeeds) — harmless.
2. **dev-ttyS0.device — still times out at 90 s** under the full
   systemd boot, killing serial-getty@ttyS0 (the finish-line blocker).
   All syscall-contract layers under it are now fixed and verified
   (see above). What remains: under the full boot, udevd's workers
   spawn but never complete events — every worker sits in epoll_wait
   while `RCU-GP-BOUND site=fdreap-close ... no full quiescent round`
   storms (15+ prints per boot; journald itself observed monopolizing
   a CPU inside fdreap-close; global RCU gen thousands ahead of the
   per-CPU gens). Workers get SIGKILLed at the 180 s event timeout.
   This is the documented monopolist family (kernel/src/fs/fdreap.rs
   comments: fork-window allocator monopolies + preempt-count leaks —
   explicitly cross-lane). SMP=4 and udev.children_max=1 do NOT dodge
   it. The quiet-boot repro processes everything, so the trigger is
   the full boot's concurrent fork/close churn.
3. **credentials 243/CREDENTIALS — unchanged** (sysusers,
   tmpfiles-setup-dev-early/setup: "Failed to set up credentials:
   Protocol error"). Not touched this session.
4. **RuntimeDirectory 233 — unchanged** (timesyncd, resolved:
   "Failed to set up special execution directory in /run: Operation
   not permitted").
5. **clone flags 0x100004100 — FIXED.** Zero "unknown flags" lines in
   every boot since d11867be9/c62dc92a2 (CLONE_CLEAR_SIGHAND et al.).

Boot high-water mark on the final tree: multi-user.target AND
graphical.target reached (via the timeout cascade), getty.target
reached with serial-getty dead on the dev-ttyS0 dependency.

## Coordination

- **ub2404-login lane (STATX_MNT_ID)**: statx is wired (x86_64 332 →
  generic 291 → sys_statx in kernel/src/syscall/file.rs) but the mask
  handling drops STATX_MNT_ID (0x1000): stx_mnt_id stays 0, mask bit
  clear. Mount IDs already have permanent per-mount parity from
  c62dc92a2; the statx wiring is the missing piece. Consumers:
  libmount create_fs (fs->id → /run/mount/utab), and the
  "Failed to drain libmount events: EINVAL" (dev-mqueue/tracing unit
  failures) is the next symptom expected to clear once mnt_id flows.
- **Monopolist/RCU lanes (gb-opt, mmforensics, sched)**: the udevd
  worker storm is a strong, repeatable reproducer for the
  fdreap-close grace-stall family — full ub2404 systemd boot, watch
  `RCU-GP-BOUND site=fdreap-close` + per-CPU gen divergence; or the
  sa3.py socket-activation recipe (in this run/'s probe scripts:
  bind group-1 netlink socket + SO_PASSCRED, exec udevd with
  LISTEN_FDS) on a busy system.

## Gates (binding)

- Triple builds on the final tree: x86_64 rc=0 (build-x64-final4.log),
  riscv64 rc=0 (build-rv64-final4.log), aarch64 rc=0
  (build-a64-final4.log).
- riscv64 `make test`: **1328/1328 ALL TESTS PASSED**
  (run/maketest-rv64-final5.log; first run needed the procfs
  capability-matrix expectation updated for the new poll slot —
  commit "tests: procfs file caps matrix — poll slot is now true").
  ROOTFS_IMG was copied to run/rootfs-test.img because the shared
  main-repo rootfs.img was write-locked by another lane (the 1.3 GB
  scratch copy is deleted after the run; re-copy the same way).
- No push (per resource rules). Boot evidence: serial-093658.log,
  serial-093952.log (finish-line attempts), probe1..23-out.txt in
  run/ (chain verification), probe15b/16/18/19b/22/23
  (socket-activation repros).

## Adopted-fd udevd completes cleanly on a quiet system (final check)

The exact shape systemd uses (kernel socket created by pid 1's
socket-unit, SO_PASSCRED set, LISTEN_FDS adoption via exec — probe22/23,
INIT=/bin/sh, SMP=4): `systemd-udevd` adopted the fd, forked **11
workers, 0 "is taking a long time"**, and filled /run/udev/data for
every device. That closes every layer of the dev-ttyS0 chain this lane
can reach: sysfs magic → sd-device acceptance → uevent delivery →
size probe → creds → worker spawn → rules → db + `G:systemd` tag →
(the group-2 processed-event multicast to pid1 was wired in gdmr22/23).
What still kills the real boot is only the fdreap-close/RCU monopolist
convoy under full-boot concurrency (see blocker 2).

## Suggested next pass (in order of finish-line impact)

(Superseded — see the round-3 finish-line section below.)

---

# Round 3 (2026-11-16 afternoon): FINISH LINE REACHED

## Verdict

The serial login prompt is LIVE on a clean integrated boot of this
worktree: `ub2404 login:` (serial-144248.log line 2902), with
`dev-ttyS0.device` FOUND (line 1082, no 90 s timeout),
`serial-getty@ttyS0.service` Started OK (line 2729), and
`getty.target - Login Prompts` REACHED (line 2730). Zero panics, zero
OOM prints through the whole boot window.

## What the reconciliation found (the login lane's boot vs mine)

The login lane's prompt boot (their ser-103809.log, x86_64 q35 KVM,
-m 8G -smp 8, ub2404.img) did NOT have main's runq-strand/heal-gate —
their kernel was ub2404-login @ 31b1f5834 + uncommitted WIP. Their
boot still shows 20 RCU-GP-BOUND / 24 fdreap lines, and mine (26/28)
does too: the monopolist convoy STILL PRINTS but is SURVIVABLE — it
was never the hard blocker. The real missing pieces were the login
lane's own commits and one uncommitted fix:

1. `1ad066966` statx STATX_MNT_ID fill (via the shared mountinfo
   mount-id model) — clears "Failed to drain libmount events:
   EINVAL" (0 occurrences in my finish boot; was every boot before).
2. `31b1f5834` devfs /dev/shm dir + namespace-teardown umount
   restore — unblocks the credentials chain (243/CREDENTIALS: 0
   occurrences in my finish boot).
3. (their uncommitted WIP, adopted clean here as 05d0d2b2c) xattr
   `system.*` (POSIX ACL) acceptance for owner/CAP_FOWNER —
   sd-mkdcreds' base-ACL setxattr was the EPROTO child kill.
   Their exit.rs/dispatch.rs/file.rs WIP is debug-print-only, NOT
   adopted.

With those on top of this lane's round-2 work (sysfs statfs magic,
netlink MSG_PEEK/MSG_TRUNC, mount_setattr pair-validation, permanent
mnt_ids), the whole chain closes: udev db fills → dev-ttyS0 plugs →
serial-getty starts → prompt. RuntimeDirectory 233 is also gone
(0 "special execution directory" lines).

## Integration verified on this tree (newmount2)

Commits on top of the round-2 head bc217f701:

- `e4c98a6c4` merge ub2404-login @ 31b1f5834 (2 conflicts, both
  parallel implementations of the same fixes: netlink.rs recv — kept
  this lane's full MSG_PEEK/report_full version over their pop+debug
  print version; network.rs len==0 probe — kept this lane's version
  which additionally handles MSG_TRUNC without MSG_PEEK per the
  reference skb rule; their UERECV print dropped in the resolution).
- `bb6527e02` merge main @ 9ca190d6d (runq-strand preemptible TTAS,
  heal-gate, ltp-tail, nohz-full, cotask-decode; clean merge, both
  statx-mnt_id and sysfs-statfs-magic verified present post-merge).
- `05d0d2b2c` xattr: system.* (POSIX ACL) setxattr accepted for
  owner/CAP_FOWNER (clean adoption of the login lane's functional
  WIP; their debug prints not carried).

## The login lane's combination (documented for the switch)

- Image: /home/william/rux-images/ubuntu/ub2404.img (boot with
  -snapshot + file.locking=off)
- Kernel: x86_64 bzImage from the integrated tree above
- QEMU (verified shape, both lanes):
  `qemu-system-x86_64 -M q35 -m 8G -smp 8 -accel kvm -cpu max
  -snapshot -display none -drive file=<ub2404.img>,if=none,id=rootfs,
  format=raw,file.locking=off -device virtio-blk-pci,disable-legacy=on,
  drive=rootfs -netdev user,id=net0 -device virtio-net-pci,
  disable-legacy=on,netdev=net0 -device virtio-gpu-pci,xres=1280,
  yres=800 -device virtio-keyboard-pci,disable-legacy=on -device
  virtio-tablet-pci,disable-legacy=on -device virtio-mouse-pci,
  disable-legacy=on -kernel <bzImage> -append "root=/dev/vda rw
  init=/sbin/init console=ttyS0"`
- Serial: `-serial file:...` (or a socket chardev to interact)
- Verified at 8G/8cpu. 4G not re-tested this round.

## Finish-line evidence (this worktree, no smoke tricks)

- run/serial-144248.log — full systemd boot of the integrated tree;
  the lines cited above; **0 panicked / KERNPANIC / OOM lines across
  the complete 25-minute window** (QEMU exited at the driver
  TIMEOUT=1500 kill, 672 KB of serial; driver run/boot-ub2404.sh,
  finish1-launch.log). Last targets reached: network.target,
  network-online.target, getty.target (Login Prompts); the gdm/
  logind tail rides its 90 s job timeouts to the end, same as the
  login lane's prompt boot.
- Remaining known failures (all non-fatal, off the finish-line
  chain): dev-mqueue.mount / sys-kernel-debug.mount /
  sys-kernel-tracing.mount fail with a real mount errno (the
  "drain libmount" EINVAL is gone; next decode is the mount(2)
  errno itself); gdm/logind/power-profiles-daemon jobs ride their
  90 s timeouts in the tail (same on the login lane's boot);
  NetworkManager.service fails after its children die (SIGABRT
  family, separate hunt).
- Console noise: the login lane's committed hunt probes (af4758125/
  31b1f5834) still print — NOBLEDBG 1413, UNIXCONNECT 375,
  mount_setattr 86, EPPROBE 80, SFDDBG 73, umountfail 72, SIGDEATH
  29 lines in this boot. They are budgeted and harmless, but should
  be quieted (or gated behind a dfx switch) before any user-facing
  `make ubuntu-run`; UERECV is already gone (merge resolution).

## Gates (binding) on the final tree (05d0d2b2c)

- Triple builds: x86_64 rc=0 (cargo, 14:42), riscv64 rc=0
  (run/build-rv64-finish.log), aarch64 rc=0 via
  `make build PLATFORM=aarch64` (run/build-a64-finish.log). Both
  cross-arch binaries verified to carry the merged-tree symbols
  (nohz) — not stale artifacts.
- riscv64 `make test`: **1329 PASS lines, ALL TESTS PASSED**
  (run/maketest-rv64-finish.log; up from 1328 — main's ltp-tail
  added tests). ROOTFS_IMG was copied to run/rootfs-test.img (the
  shared main-repo rootfs.img stays untouched) and the 1.3 GB
  scratch copy is deleted after the run.
- No push (per resource rules). No perf-gate tiers run by this lane
  (coordinator's gate; note this round touches the syscall surface —
  statx mnt_id via the merge — but no scheduler/mm/IRQ hot path
  beyond what main already gated).

## Declaration for the coordinator (make ubuntu-run switch)

The finish line is reached and reproducible: newmount2 @ 05d0d2b2c
(= this lane's round 1+2 + ub2404-login @ 31b1f5834 + main @
9ca190d6d + the xattr ACL commit) boots ub2404.img to a live serial
login prompt under x86_64 KVM. Suggested switch: point
`make ubuntu-run` (or a `make ub2404-run` sibling) at the documented
QEMU shape above with INIT=/sbin/init; the existing GNOME/runit flow
stays untouched. Merge order for main: this branch already carries
main @ 9ca190d6d, so a single merge of newmount2 lands everything
(login lane's two commits + this lane's five) — after their uncommitted
debug-print WIP is either committed separately or dropped (only the
xattr change was functional, and it is already here, clean).
