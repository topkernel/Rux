# LAUNCHER-ERRNO — the 8-17s errno-census diff: seven kernel-side divergences fixed, the freeze wall re-named

Lane: `launcher-errno` @ main dd0f74536 + 8 commits (branch `launcher-errno`).
Rig: the why-slow ws rig (KVM q35 SMP8 8G, LOGLEVEL=4, NOSNAP first-boot
population, per-boot reflink copies of `userdata-f16g-fresh3.img` (the
"fresh3" vehicle) or `userdata.img` (the "V0" factory vehicle),
`dfx=pctlmon syscost=1`, 420 s windows). Reference: OH 6.6.101 bzImage,
serial wsref4 (factory vehicle) for the error-set side.

## 0. TL;DR

1. **The method worked**: the why-slow kmsg error-stream diff was extended
   with a per-(syscall, errno) CENSUS (new syscost table: count,
   first-seen boot-time, first comm, first a0, and — for path syscalls —
   the first-seen PATH STRING, snapshotted at cell creation). It caught
   what kmsg cannot: silent errno answers.
2. **Seven kernel-side divergences convicted and FIXED** (all on
   reference evidence): tracefs chmod/chown EPERM (~494 kmsg rows),
   the /sys/kernel/debug/tracing alias unreachable (~259 rows, TWO
   stacked causes), cgroup v1 controller mounts succeeding (EINVAL on
   reference), /dev/memcg v1 legacy files missing, the boot-interface
   node batch (configfs + /dev/{fuse,tun,bbox,sched_rtg_ctrl} +
   /proc/{slabinfo,vmallocinfo} + /sys/power/resume*), **recvmsg
   EMSGSIZE on large receive iovecs (a 7.4K/s retry-forever spin,
   3.16 M calls per boot)**, and **ext4 readdir panicking on huge
   (100K+ entry) Linux-written directories** (the V0-vehicle kernel
   death at t=40 s).
3. **Two suspects ACQUITTED with evidence**: foundation's stat-EACCES
   storm on the settingsdata RDB path is PARITY (the 0770 10007:10007
   dir is on-disk identical on both vehicles; foundation has no
   DAC_OVERRIDE on either kernel), and the ioctl -512 (ERESTARTSYS)
   census cells are the binder restart sentinel counted before the
   trap-exit conversion — never observed by userspace.
4. **The launcher wall stands, now with a sharper name**: after all
   fixes the ladder still freezes (le4: bootanimation.finished 12.9 s,
   then nothing; le1/le3/le6 same face). The census leaves TWO named
   live kernel-adjacent chains and one image-side verdict: (a) init
   spins ~1K/s opening /dev/__parameters__/param_selinux (228 K calls
   by t=240 s) because the SELinux-labeled param workspace never
   materializes — the creation chain runs inside libparaperm_checker/
   libselinux against our minimal selinuxfs and dies silently (needs
   guest-side hilog visibility or an strace-class init tracer); (b)
   selinuxfs class discovery (class/<name>/index) answers ENOTDIR —
   the reference's selinuxfs carries the loaded-policy class tree;
   (c) the freeze's binder face (foundation handler threads exiting
   owing sync replies) produced NO kernel errno divergence in-window
   in any census — the remaining wall is image-side timing/decision,
   consistent with neverword's per-boot-varying frozen chains.
5. **V0 crawl confirmed + one hard bug killed**: our kernel on the
   factory vehicle: bootanimation.ready 31.6 s vs 5.3 s on fresh3
   (~6x) and then the readdir PANIC (fixed; the remaining crawl is
   unmeasured — the panic killed the boot before any syscost dump).

## 1. Method

Same-vehicle boots ours vs reference; two diffs:

- **kmsg ERROR rows** (why-slow's sieve, refreshed on current main
  with le1/le4): normalize numbers/pids, set-diff. Reference rows carry
  their own guest timestamps (second bracket); ours are driver-stamped.
- **the errno census** (new): `syscost=1` boots print
  `SYSCOST t=Ns errno: nr= err= n= first=s comm= a0=` plus
  `errno-path: ... path=` rows. EAGAIN excluded (idle-park storm);
  ERESTARTSYS-family rows flagged as pre-conversion artifacts.

## 2. Verdict table (all evidence = le-series boots + wsref4)

| # | family | ours (before) | reference | fix (commit) |
|---|---|---|---|---|
| 1 | tracefs chmod/chown | EPERM on every existing node (494 kmsg rows; ~612 chmod fails at t=1.1-1.6 s) | chmod/chown SUCCEED on every existing node (its failures are all ENOENT for nodes its kernel lacks) | TraceFsEntryData iattr overrides, kernfs parity (b02cef0e5) |
| 2 | /sys/kernel/debug/tracing alias | whole tree ENOENT (259 rows) | alias exists (kernel automounts tracefs under debugfs); init's later `mount debugfs /sys/kernel/debug` answers EBUSY | (a) debugfs single-instance EBUSY (eaf0c2c89); (b) create the inode-bearing debugfs 'tracing' node before the alias mount — vfs_mount's bare-dcache child is invisible to path_lookup's inode-ops walk (5bfdb58d2). Verified on the minimal rootfs: ls/chmod/cat through the alias all rc=0 |
| 3 | cgroup v1 controller mounts | /dev/{cpuctl,cpuset,pids,frz} mount SUCCEEDS → follow-up chmod/chown rows diverge (EPERM on real cgroupfs vs ENOENT on reference's plain dirs) + /dev/pids subgroup write paths diverge | all four mounts answer EINVAL (err 22); /dev/memcg memory SUCCEEDS | do_mount: only the memory controller / name= hierarchies / generic tokens mount on -t cgroup; other controller tokens EINVAL (c2eed7357) |
| 4 | /dev/memcg legacy files | memory.oom_control / memory.pressure_level / cgroup.event_control ENOENT (chmod/chown rows) | files exist (memmgrservice.cfg chmods all three successfully) | cgroupfs control files: oom_control read/toggle, event_control registration-form write, pressure_level empty read (c2eed7357) |
| 5 | boot-interface node batch | /config mount EINVAL (→ hmdfs/sharefs chown ENOENT); /dev/{fuse,tun,bbox,sched_rtg_ctrl} chmod ENOENT; /proc/{slabinfo,vmallocinfo}, /sys/power/{resume,resume_offset}, /sys/hibernate/resume ENOENT | all exist and accept chmod/chown (configfs mounted: its later remount answers EBUSY err 16) | CONFIGFS_MAGIC tmpfs instance with pre-created hmdfs/sharefs dirs; null-misc dev stubs; slabinfo v2.1 header + empty vmallocinfo; resume '0:0\n' / resume_offset '0\n' (faaad8b6c) |
| 6 | **recvmsg large-iovec** | **EMSGSIZE for aggregate iovec > RW_CHUNK*4 — sysevent_source spun 8K/s from t=5.2 s, 3,155,708 calls in one boot (census, le3/le4), the datagram never drained** | Linux NEVER errors on a large RECEIVE buffer (it truncates); no such spin possible | guard moved to the STAGING size (unix direct path stages nothing; staged paths allocate min(total, RW_CHUNK*4)) (eaf0c2c89) |
| 7 | **ext4 readdir on huge dirs** | **handle_alloc_error PANIC (ALLOCTHROW 4 MiB) in ext4_readdir via getdents64 — le5 (V0 vehicle) kernel death at t=40.4 s** | Linux streams entries into the user buffer; no whole-dir kernel Vec | list_dir/ext4_readdir try_reserve per entry, return PARTIAL list + kernel error line (27675d98f); full fix = streaming readdir (open) |

### Acquittals (evidence recorded)

- **foundation stat EACCES x4228+ (in-window, from 9.9 s)** on
  /data/app/el1/0/database/com.ohos.settingsdata/entry/rdb/... — the
  blocking component is `com.ohos.settingsdata` itself at 0770
  10007:10007, IDENTICAL on both vehicles (verified by mounting both
  userdata images offline); foundation holds no DAC_OVERRIDE on either
  kernel (foundation.cfg caps = SYS_PTRACE, KILL). The reference's
  foundation gets the same EACCES. PARITY.
- **ioctl -512 census cells (n=97)** — binder's -ERESTARTSYS sentinel
  on signal-touched readers, counted at syscall exit BEFORE the
  trap-exit restart/EINTR conversion. Not observable by userspace.
- le2's silent kernel death at 5.4 s (pristine kernel + syscost=1, zero
  SYSCOST dumps = timers dead) did not reproduce in le3/le4/le6
  (same class as the early-face flake; load was 9-13 during le2).

## 3. Ladder (same rig, fresh3 vehicle, load noted)

| milestone | le1 (main dd0f74536, pristine) | le4 (+fixes 1-6) | le6 (+all 8) | why-slow ws3 | reference (abref1) |
|---|---|---|---|---|---|
| bootanimation.ready | 5.35 | 5.29 | 5.80 | 5.02 | 5.14 |
| bootanimation.finished | 10.59 | 12.89 | 11.08 | 10.53 | 11.25 |
| wms.fullscreen.ready | never | never | never | 10.89 | 8.30 |
| lockscreen.ready | never | never | never | 11.19 | ~9 |
| launcher.ready | never | never | never | never | 11.45 |

Face note: le1-le6 all froze at/below bootanimation under host load
5-22 (other agents' QEMUs) — the ladder face is load-sensitive; ws3's
lockscreen face was measured on a quieter evening. The fixes removed
the error rows (le4's ours-only kmsg set dropped to the reference
residue plus the alias family) but did not lift the freeze in these
conditions. le6 census: recvmsg -90 cells = ZERO all boot (the fix
holds); init's param_selinux open-ENOENT spin = 196,967 by t=360 s
(~700/s sustained) — the remaining top spin, see §4.1.

le6 alias note: the debug-alias chmods STILL fail ENOENT on the OH
boot even with 5bfdb58d2, while the identical kernel passes the
minimal-rootfs probes (ls/chmod/cat through the alias all rc=0). The
distinguishing OH stage is switch_root (init's mount-move/pivot at
~0.7-1.2 s): boot-time submounts under /sys/kernel/debug apparently
do not survive the re-homing. Same root-cause class as the sc36
namespace-discipline notes. Open item — name it "alias vs
switch_root".

## 4. The wall, named (what remains)

1. **The param_selinux spin** (init, ~1K open/s, 228 K by t=240 s):
   /dev/__parameters__/param_selinux never appears. The creation chain
   is init's SELinux param workspace: param_selinux.c dlopens
   libparaperm_checker.z.so → libselinux (setfilecon/class discovery
   against selinuxfs) → AddWorkSpace writes the file. Some link in that
   chain fails silently on ours. NOT directly visible in the census
   (the census sees the poll, not the creation failure). Next
   instrument: a budgeted kmsg trace of init's failing syscalls
   (pid-filtered errno census) or an image-side hilog echo.
2. **selinuxfs class tree**: class/<name>/index probes answer ENOTDIR
   (our selinuxfs stub has empty dirs; the reference carries the loaded
   policy's class tree). samgr's SA-gate currently passes via
   deny_unknown=0 (b51), but every CheckPerm still probes-and-fails
   (n=1483 opens). A synthetic class/<name>/index returning a stable
   id (plus an `access` node answering allow) would be full parity —
   deferred: no observed gating failure.
3. **The binder freeze face**: zero kernel errno divergence in the
   8-17 s window attributable to foundation's frozen sync chains in
   ANY census boot. Combined with neverword's per-boot-varying chain
   shapes, the launcher wall's remaining layer is image-side decision
   under our timing — the f2-class "handler thread exits owing a
   reply" reproduced in ch2 as a whole IPC-pool teardown at 77.4 s
   owing txn 4771 (an OH WorkGroup destructor path), always AFTER the
   spawn window failed to complete.

## 5. V0 vehicle (family 3) status

le5 (fixed kernel through eaf0c2c89, factory V0 vehicle):
bootanimation.ready 31.6 s, finished 36.9 s (vs 5.3/12.9 fresh3, ~6x
crawl — WHY-SLOW's 5x number confirmed), then the ext4 readdir PANIC
at 40.4 s (fixed by 27675d98f — the panicked getdents was walking a
Linux-written 100K+ entry directory). No syscost dump survived (the
panic killed the boot at t=40 s, before the t=60 s dump), so the
per-op crawl anatomy is STILL UNMEASURED; the fixed kernel + one V0
boot with syscost=1 is the follow-up. Static audit of the htree
reader found full hash support (legacy/tea/half_md4 + unsigned) and a
careful dx parse; the crawl's cause is not the htree fast path.

## 6. Gates

- x86_64 build rc=0 (debug + release), riscv64 build rc=0, aarch64
  build rc=0.
- `PLATFORM=riscv64 make test`: first run rc=2 was a lane artifact
  (the run raced this lane's x86 rootfs rebuild; test/rootfs.img was
  absent mid-rebuild). Clean re-run: **1329/1329 tests passed, 0
  failed** ("ALL TESTS PASSED", le-gate-riscv-maketest2.log line
  25116); the make rc=2 is the post-halt QEMU wrapper exit — the same
  artifact as the recorded why-slow baseline (gate-riscv-maketest.log:
  "suite verdict green; QEMU wrapper rc=124").
- Guarded domains touched: fs/ (tracefs setattr, cgroup files, mount
  parity, ext4 readdir degrade) and one syscall (recvmsg). pbench fs
  rows + ipc pipe/unix roundtrips owed on merge review per the gate;
  the recvmsg change only REMOVES an error return (the staging cap
  preserves the old allocation bound).

## 7. Results log

- le1 (pristine dd0f74536, fresh3): freeze after bootanimation.finished
  10.59 s; current-main error rows still carried the tracefs-EPERM and
  node families (fix #5 of why-slow had killed cgroupfs chmod EPERM).
- le2 (pristine + syscost=1): silent kernel death at 5.4 s (no dumps);
  did not reproduce later. Census not in pristine — the boot was the
  pristine release by accident (target rebuilt by the stash dance).
- le3 (fix1 batch + census): census landed. Top cells: recvmsg -90
  491 K@60 s → 3.16 M@420 s; init open ENOENT 188 K; readlink EINVAL
  12 K; stat EACCES 4228 by OS_IPC_1_1129 from 9.93 s (acquitted);
  ioctl -512 (acquitted). Ladder froze at bootanimation.finished 10.8.
- le4 (all fixes through eaf0c2c89): recvmsg spin GONE (no nr=47
  cells); the debug-alias family still ENOENT (both alias causes
  fixed after le4); ladder froze at 12.9 s; markers otherwise equal to
  le1.
- le5 (V0 vehicle, fixes through eaf0c2c89): 6x crawl +
  ext4_readdir panic at 40.4 s (convicted fix #7).
- le6 (bzImage-fix3 = all 8 commits, load 8→22): freeze at
  bootanimation.finished 11.08 s; recvmsg -90 cells ZERO all boot
  (fix holds at boot scale); init open-ENOENT spin 196,967 by t=360 s
  (param_selinux poll — the named §4.1 chain); the debug-alias chmods
  still ENOENT on the OH boot despite passing the identical kernel's
  minimal-rootfs probes — "alias vs switch_root" open item (§3 note).
- Minimal-rootfs alias verification (riscv TCG, 5bfdb58d2):
  `ls /sys/kernel/debug/tracing` lists the tree; chmod events → 0;
  chmod binder enable → 0; cat id → 0.

## 8. Commit index (branch launcher-errno)

- b02cef0e5 tracefs: kernfs iattr parity for chmod/chown
- c2eed7357 cgroup: v1 controller mount parity + /dev/memcg legacy files
- 37f7f0915 syscost: per-(syscall, errno) census (the instrument; path
  snapshots added in eaf0c2c89)
- faaad8b6c fs: boot-interface node parity — configfs, /dev misc stubs,
  proc/sys power files
- eaf0c2c89 net: recvmsg large-iovec parity (the sysevent_source spin)
  + debugfs single-instance EBUSY + census path snapshots
- 27675d98f ext4: readdir degrades instead of panicking on huge dirs
- 5bfdb58d2 main: create the debugfs tracing node before the alias mount
