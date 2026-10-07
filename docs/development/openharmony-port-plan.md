# OpenHarmony on Rux — Feasibility Analysis and Port Plan

> **Status**: analysis complete, plan not yet started. This document is the
> working plan for replacing the Linux kernel of the OpenHarmony x86_64_virt
> emulator image with Rux. It was produced by auditing two trees:
> `/home/william/workspace/oh-robot` (OpenHarmony source + built images +
> a full 5,929-line reference boot log `kernel.log`) and the current Rux
> `main` (roadmap v31.0, 2026-10-04). Every factual claim below carries a
> file-path reference into one of those trees.

---

## 1. Executive summary

**Replacing the Linux kernel under this OpenHarmony image is feasible. The
estimated total is 20–35 person-months of traditional engineering effort,
or roughly 6–12 calendar months at the project's current 5-agent parallel
workflow, to reach "boots to launcher/lockscreen in QEMU". Production-grade
(24h stability, full LTP parity, performance) roughly doubles that.**

New code estimate: **25,000–40,000 lines of Rust**, about 20–25% of the
current kernel body (164.9k lines).

The three dominant work items, in order:

| # | Item | Why it dominates | Est. (new code) |
|---|------|------------------|-----------------|
| 1 | **x86_64 architecture port** | Rux is riscv64-only: zero x86 code, 984 direct `arch::riscv64::` references across the kernel, no ACPI/APIC/bzImage support | 12–15k lines |
| 2 | **binder driver** | Every system-service IPC (samgr → foundation → launcher) runs over kernel binder (`/dev/binder`, `/dev/hwbinder`, `/dev/vndbinder`). Nothing boots past init without it | 6–10k lines |
| 3 | **graphics** | render_service and bootanimation use `/dev/dri/card0` + DRM ioctls + mesa llvmpipe; Rux has fbdev only | 1–2k (HDI fbdev route) or 4–8k (in-kernel DRM) |

Two structural facts keep the estimate from being far larger:

- **The reference system tolerates many missing kernel interfaces.** The
  reference boot log shows the current OH image reaching
  `All boot events are fired, boot complete now` at 10.3 s while cpuset /
  cpu / pids / freezer cgroups, `/dev/ion`, `/dev/ucollection`, hungtask,
  zbinder tracing, zram, and the watchdog device are all absent or failing
  (see §4.2). We get to inherit that tolerance.
- **HDF costs the kernel nothing here.** The image runs HDF entirely in
  userspace (`# CONFIG_DRIVERS_HDF is not set` in
  `out/x86_64_virt/kernel/OBJ/linux-6.6/.config`; `hdf_devmgr` is a plain
  userspace process). No `/dev/hdf` kernel interface is needed.

Plan shape: **Phase 0** ports Rux to x86_64 with Ubuntu amd64 as the
verification gate; **Phase 1** stands up the initrd boot chain; **Phase 2**
adds the OH device nodes (binder first); **Phase 3** brings up graphics to
launcher; **Phase 4** is the long tail. Three de-risking spikes start
immediately and run in parallel (§7).

---

## 2. Target system profile

Facts about the system Rux must support, from the oh-robot tree:

- **OS**: OpenHarmony 6.1 dev line, API 23 (`build/version.gni`:
  `sdk_version = "6.1.0.35"`, `api_version = "23"`; runtime string in
  `out/.../system/etc/param/ohos.para`). Standard system level
  (`out/ohos_config.json`: `os_level: "standard"`).
- **Board/product**: `x86_64_virt` (EDU), configs at
  `vendor/edu/x86_64_virt/` and `device/board/edu/virt/` (kernel defconfig
  at `device/board/edu/virt/kernel/configs/x86_64_virt_defconfig`).
- **Reference kernel**: Linux 6.6.101 + OH patches, built by
  `device/board/edu/virt/kernel/build_kernel.sh`.
- **Machine**: QEMU q35, 6 vCPUs, 8 GB, KVM, six virtio-blk disks
  (updater/system/vendor/sys_prod/chip_prod/userdata), virtio-gpu at
  360x720 with `-virgl` (no 3D), virtio-net, **PS/2 keyboard + mouse**
  (q35 defaults; `CONFIG_VIRTIO_INPUT` is off in the defconfig).
  See `qemu_run_user.sh` / `qemu_run_bridge.sh` in the tree root.
- **Boot**: `-initrd ramdisk.img -kernel bzImage`; cmdline
  `root=/dev/ram0 init=init console=ttyS0` plus `ohos.boot.*` and
  `ohos.required_mount.system=/dev/block/vdb@/usr@ext4@...` tokens (parsed
  by userspace init, not the kernel — the kernel only has to expose them
  verbatim via `/proc/cmdline`).
- **Userspace**: x86_64 musl (OH libc), dynamically linked; the ramdisk's
  `/init` symlinks to `bin/init_early` (built from
  `base/startup/init/services/init/standard/`).
- **No kernel modules anywhere**: `build_kernel.sh` only runs `make
  bzImage`; no `modules_install`, no `.ko` in ramdisk or system images, no
  `insmod` in the whole boot log. Rux's registration-only module loader
  (`kernel/src/module/mod.rs`, which deliberately links no module code) is
  sufficient as-is.

### Reference boot chain (what must come up, in order)

From `kernel.log` (times from kernel start):

1. 1.311 s kernel runs `/init` (= `init_early`) from the ramdisk.
   It mounts tmpfs/devpts/proc(hidepid=2)/sysfs/selinuxfs, mknods
   `/dev/{null,random,urandom,kmsg}`, then uses the **embedded ueventd**
   library listening on `NETLINK_KOBJECT_UEVENT`
   (`base/startup/init/ueventd/ueventd_socket.c:42`) to wait for
   `/dev/block/vdb` (system) and `/dev/block/vdc` (vendor).
2. Mounts vdb→`/usr`, vdc→`/vendor`, **switch_root to /usr**, execs
   `/bin/init --second-stage`; second stage mounts vdd/vde, runs
   resize2fs/e2fsck, mounts vdf→`/data` (fstab at
   `device/board/edu/virt/cfg/fstab.virt`), loads SELinux policy
   (`policy.31`, permissive), starts `param_service`.
3. Services: `ueventd`, `watchdog_service`, `hilogd`, **`samgr`**,
   `hdf_devmgr` (userspace), `appspawn`, then ~40 system abilities
   (wifi/audio/mm input/telephony/...), HDI hosts
   (`/vendor/etc/init/hdf_devhost.cfg`), **`render_service`** (3.5 s),
   `foundation` (5.0 s), `bootanimation`, **`launcher`** ready at 8.7 s,
   boot complete at 10.3 s.

---

## 3. Kernel dependency inventory

### 3.1 Hard requirements (system does not boot / no UI without them)

| Requirement | Evidence | Rux status |
|---|---|---|
| x86_64 CPU, q35 platform, KVM-capable | `qemu_run_user.sh` | **zero x86 code** (`kernel/src/arch/mod.rs:12` "x86_64 - Not implemented") |
| initrd (gzip cpio) unpack + `root=/dev/ram0` semantics | ramdisk.img is cpio; cmdline | **none** — no cpio/gzip unpacker, no ramdisk block device, `root=` values are not even read (`cmdline.rs` accessors have no callers) |
| `NETLINK_KOBJECT_UEVENT` broadcast with ACTION/DEVPATH/DEVNAME/SUBSYSTEM/SEQNUM + `/sys/kernel/uevent_seqnum`; ueventd creates `/dev/block/vdX`, `/dev/input/event*` from them | `ueventd_socket.c:42`; log shows ueventd handling block/input events per `system/etc/ueventd.config` | **have** — `kernel/src/net/netlink.rs:889`, `kernel/src/fs/sysfs.rs` (uevent_send_full, seqnum, `/sys/kernel/uevent`), verified end-to-end with busybox mdev |
| ext4 on virtio-blk PCI, 6 disks, `barrier=1` tolerated | required_mount cmdline tokens | **have** ext4+JBD2+virtio-blk-PCI; `barrier=` option is accepted-ignored (mount option parser has no such flag; flush via `VIRTIO_BLK_T_FLUSH` exists) |
| **binder**: misc devices `binder`, `hwbinder`, `vndbinder`, full ioctl/mmap UAPI (BINDER_WRITE_READ, BC/BR commands, node/ref mgmt, death notifications, thread registration; `CONFIG_BINDER_SENDER_INFO=y`) | defconfig lines 5171–5176; log full of `binder: ... transaction` lines | **none** — no binder code anywhere in the tree |
| **ashmem**: `/dev/ashmem` with size/pin/unpin ioctls | defconfig 4893, `ashmem: initialized` at 0.67 s | **none** (SysV shm + `/dev/shm` tmpfs exist; not the same ABI) |
| `/dev/access_token_id` char device + ioctls | defconfig 5214; the most-referenced node in the log (99 hits) | **none** (devfs char-device registry `fs/devfs/registry.rs` is ready for it) |
| memfd_create | OH userspace uses it | **implemented but unwired**: `fs/memfd.rs:95` vs ENOSYS stub at `syscall/process.rs:4219` (NR 279 dispatches to the stub) |
| Graphics: `/dev/dri/card0`, DRM ioctls, dumb buffers, mode-set, llvmpipe (mesa `kms_swrast_dri.so` under `/vendor/lib64/chipsetsdk/`) | render_service/RSRenderThread/bootanimation AVC entries on `dev_dri_file` with ioctl 0x642e/0x64b3 + mmap | **fbdev only** (`drivers/gpu/fbdev.rs`, `/dev/fb0`); no `/dev/dri`, no DRM ioctls |
| Input: `/dev/input/event*` evdev; PS/2 source on q35 | log: AT Translated Set 2 keyboard + ImExPS/2 mouse → event0–2; MMI reads evdev, injects via uinput | **evdev + virtio-input have**; PS/2 is a placeholder (`drivers/input/ps2.rs:116` "ports not available"); uinput missing |
| selinuxfs mounted at `/sys/fs/selinux`, permissive policy load (`policy.31`) | log 548 (policy loaded); all AVC `permissive=1` | **none** — needs at least a minimal selinuxfs or verified failure tolerance |
| cgroup v1 `memory` mounted at `/dev/memcg` (memmgrservice.cfg:13) with `writepid` into `/dev/memcg/perf_*` from render_service/multimodalinput; cgroup2 at `/sys/fs/cgroup` | log; `CONFIG_MEMCG=y` on reference | **partial** — cgroup2 with cpu/memory/pids controllers only (`fs/cgroup.rs`); no v1-style named mounts |
| `/proc` and `/proc/sys` write surface: `kernel.printk_devkmsg`, `randomize_va_space`, `oom_score_adj`, net.* sysctls, `/proc/cmdline` verbatim | `system/etc/init.cfg` chown/write list | partial — procfs has no `/proc/sys` tree of this breadth yet (per feature-gap analysis 2026-09-24) |

### 3.2 Tolerable failures (de-scope list — evidence the system boots without them)

All of the following **fail in the reference boot log** (err=2 / ENOENT /
`Unknown subsys name`) and the system still reaches boot-complete:

- cpuset / cpu / pids / freezer cgroups (`cgroup: Unknown subsys name`,
  log lines ~1000/1003/1255)
- `/dev/ion`, `/dev/ucollection`, `/dev/sched_rtg_ctrl`,
  `/dev/xt_qtaguid`, `/dev/mali0`, `/dev/block/by-name/{misc,bootctrl}`
- `/sys/kernel/hungtask/*` (XCollie logs `can't open hungtask file`),
  `/proc/sys/hguard/*`, `/proc/net/aware/*`
- zbinder tracefs events; debugfs/tracing mounts that fail are non-fatal
- zram (`/sys/block/zram0` absent), hyperhold
  (`open file /dev/by-name/hyperhold failed`)
- `/dev/watchdog` — `watchdog_service` respawns every ~10 s and the boot
  proceeds regardless
- `/dev/hilog` — notably, **even the reference kernel failed to register
  hilog** (`register hilog error -16`, EBUSY on chrdev region), and
  userspace `hilogd` works fine regardless. Rux can skip hilog entirely.
- audio: this boot had "No soundcards found" — no audio needed for the gate
- fscrypt on /data (`fileencryption=software` in fstab) — since we control
  image assembly, we can also simply drop the option; it is not needed for
  an unencrypted emulator image
- hmdfs / sharefs (distributed filesystem): it initializes on the
  reference kernel, but distributed (cross-device) features are out of
  scope for the local gate; stub-or-fail is acceptable initially

Decision: **none of §3.2 goes into Phases 0–3.** They are Phase 4 or
never, gated on real need.

---

## 4. Rux gap analysis (summary)

| Area | Verdict | Evidence / notes |
|---|---|---|
| arch layer | riscv64 only, no trait abstraction; 984 direct refs to `arch::riscv64::` outside `arch/` | `arch/mod.rs`; aarch64 previously existed and was removed (`quickref.md:106`) — dual-arch maintenance cost is real, see §8 R1 |
| boot | OpenSBI + FDT + `-kernel` ELF; root mounting is heuristic (auto-mount first ext4), not cmdline-driven | `main.rs:544-700`, `cmdline.rs` |
| virtio | blk PCI+MMIO, gpu PCI 2D, input PCI; **net MMIO only**; MSI-X registers exist but unused (INTx via `virtio_pci.rs:600` heuristic); single-queue | — |
| PCI ECAM | works, base hardcoded `0x30000000`, bus 0..8 | q35 ECAM is at `0xb0000000` — config, not rewrite |
| serial | MMIO ns16550A console | x86 needs port-I/O 8250 variant (same register semantics) |
| timer/RTC | SBI timer + goldfish RTC | x86 needs TSC/HPET/LAPIC-timer + `rtc_cmos` |
| SMP | SBI HSM hart_start + SSIP IPI | x86 needs SIPI + LAPIC IPIs + per-CPU GS base |
| futex | complete except **all PI ops ENOSYS** (`sync/futex.rs:1226-1279`) | musl OH libc does not require PI futex on the boot path — keep as P2 |
| netlink | ROUTE + KOBJECT_UEVENT | the two protocols ueventd/netd need first |
| AF_UNIX, pty, inotify, epoll, robust futex, loop, seccomp, ptrace | present | battle-tested by Ubuntu/Xorg/D-Bus bring-up |
| process_vm_readv/writev | EOPNOTSUPP stubs | watch for debug tooling (faultloggerd) use; not on boot critical path |
| getrandom, sched_setaffinity, sendmmsg/recvmmsg, copy_file_range, personality, prctl | present | — |
| swap | works (single swap area, root-disk tail) | sufficient; zram is de-scoped |

---

## 5. Workload estimate

| Phase | Content | Traditional | Agent-mode calendar |
|---|---|---|---|
| 0 | x86_64 port + arch convergence | 6–12 p-mo | 2–4 months |
| 1 | initrd + boot chain to second-stage init | 2–4 weeks | 2–4 weeks |
| 2 | binder + ashmem + access_tokenid + memfd wiring + selinuxfs-min | 3–6 p-mo | 1–2 months |
| 3 | graphics to launcher (route A preferred) + input | 1–2 p-mo (A) / 3–6 p-mo (B) | 1–2 weeks (A) / 4–6 weeks (B) |
| 4 | long tail, stability, LTP parity | 3–6 p-mo | ongoing, parallelizable |
| **Total** | | **20–35 p-mo** | **6–12 months to gate** |

Calibration for the agent-mode column: the current kernel body (164.9k
lines, Ubuntu desktop + Xorg + D-Bus + LTP 800) was produced by this
workflow in well under a year; the port plan adds ~20–25% of that body with
the same review/cherry-pick process.

---

## 6. The plan

### Phase 0 — x86_64 architecture port (gate: Ubuntu amd64 desktop)

**Scope.**

1. Arch layer, mirroring `arch/riscv64` (12.7k lines today):
   - boot: bzImage Linux boot protocol (QEMU `-kernel`); 32-bit protected
     mode entry, `boot_params` from QEMU, e820 memory map consumption
   - MM: 4-level page tables, NX, per-CPU CR3/ASID analog, TLB shootdown
     IPIs, fixmap
   - traps: IDT + TSS/IST stacks (NMI/double-fault), `syscall`/`sysret`
     via MSRs, pt_regs layout for the syscall layer
   - SMP: SIPI startup sequence, per-CPU areas via GS base
   - interrupts: LAPIC (xAPIC, x2APIC optional) + IOAPIC from ACPI MADT;
     wire MSI/MSI-X for virtio (replaces the `virtio_pci.rs:600` INTx
     heuristic)
   - time: TSC clocksource + calibration, HPET fallback, LAPIC timer for
     ticks; `rtc_cmos` for wall clock
   - ACPI: RSDP→XSDT→(MADT, HPET, MCFG, FADT) parsing — the FDT
     replacement
   - console: port-I/O 8250 (reuse the 16550 register logic, swap access)
2. Driver adjustments: PCI ECAM base from MCFG/q35 (0xb0000000);
   virtio-net PCI path; PS/2 i8042 keyboard + mouse driver (the existing
   `drivers/input/ps2.rs` framework becomes real on x86).
3. **Arch abstraction convergence** (structural, must land inside Phase 0,
   not after): collapse the 984 direct `arch::riscv64::` references into
   an explicit arch interface so the second architecture is a leaf module,
   not a second copy of call sites.

**Exit gate.** On x86_64 QEMU q35: Ubuntu 22.04 amd64 rootfs boots through
the existing GUI verification (`test/ubuntu-gui/verify.py`, 23/23 pixel
checks) and an LTP full sweep within a pre-agreed delta of the riscv64
baseline (suggest: ≥90% of current riscv64 PASS count; regressions triaged
into x86-specific vs generic).

**Do not start Phase 1 work before this gate.** Everything OH-specific is
unverifiable without the architecture.

### Phase 1 — initrd boot chain (gate: OH second-stage init runs)

**Scope.**

- gzip/cpio initrd unpack into the early rootfs; `root=/dev/ram0`
  semantics (unpack to rootfs ramfs; no ramdisk block device needed —
  Linux's own modern path); make `root=`, `init=`, `console=` actually
  drive boot (the accessors exist in `cmdline.rs`, unused)
- `/proc/cmdline` exposes the full cmdline verbatim (init parses
  `ohos.*` tokens itself; the kernel must not filter)
- uevent payload check: block devices emit `SUBSYSTEM=block` +
  `DEVNAME=vdX` so ueventd creates `/dev/block/vdX`; `/dev/pts` via
  devpts mount; `/dev/kmsg` write path
- verify/write-tolerate the `/proc/sys` surface init touches first
  (`kernel.printk_devkmsg`, `randomize_va_space`, `oom_score_adj` writes
  must at least not wedge init)
- switch_root works (userspace; `pivot_root` already implemented)

**Exit gate.** OH `init_early` completes: mounts /usr + /vendor from
/dev/block/vdb, vdc, switch_root, `/bin/init --second-stage` reaches
`ServiceStart` of ueventd/samgr. Services will crash immediately (no
binder) — that is expected and fine at this gate.

### Phase 2 — OH device nodes + binder (gate: samgr.ready)

**Scope, in dependency order.**

1. **binder** (the critical path): misc char devices `binder`, `hwbinder`,
   `vndbinder` with the Linux Android binder UAPI as extended by OH
   (`BINDER_SENDER_INFO`). Reference implementation: Linux
   `drivers/android/binder.c` + OH tree's patched copy under
   `kernel/linux/linux-6.6/drivers/android/`. Deliverables: mmap buffer
   zone with async/sync offsets, BINDER_WRITE_READ, full BC_* set
   (TRANSACTION, REPLY, ACQUIRE/RELEASE, INCREFS/DECREFS, DEAD_BINDER
   done, ENTER/EXIT_LOOPER, REGISTER_LOOPER), BR_* return set, node/ref
   accounting, death notifications, thread management, freeze/spam
   controls (minimal). Test against OH's own `libbinder` userspace —
   samgr is the acceptance test.
2. **ashmem**: `/dev/ashmem` misc device; ASHMEM_SET_NAME/SIZE/PIN/
   UNPIN/GET_PIN_STATUS ioctls over a tmpfs-backed page pool.
3. **access_tokenid**: small char device with get/set ioctls
   (~300–500 lines; port the semantics from
   `kernel/linux/linux-6.6/drivers/accesstokenid/`).
4. **memfd wiring**: point NR 279 at `fs/memfd.rs`.
5. **selinuxfs minimal**: enough of `/sys/fs/selinux` for `load_policy`
   to succeed permissively (enforce node reads 0, policy load/commit
   accepted). Alternative if Spike S2 (§7) proves tolerance: mount may
   fail. Decide per spike result.
6. hilog: **skip** (reference kernel itself failed to register it).

**Exit gate.** `bootevent.hdf_devmgr.ready` and samgr stability: samgr,
hdf_devmgr, appspawn, storage_manager, hilogd stay up; `foundation`
spawns and reaches its binder-heavy startup path.

### Phase 3 — graphics + input to launcher (gate: launcher.ready + visible UI)

**Route A (preferred): HDI fbdev composer.** Write a display HDI composer
plugin in the OH tree (userspace, IDL-generated interface, loaded via
`hdf_devhost` composer_host) that renders into `/dev/fb0` instead of DRM.
Rux's virtio-gpu 2D + fbdev + pan/flush path is already verified by the
Ubuntu desktop work. Cost: 1–2k lines of userspace C++ in the oh-robot
tree, no kernel DRM. *Feasibility check is Spike S3.*

**Route B (fallback): minimal in-kernel DRM/KMS.** `/dev/dri/card0` +
render node, DRM_IOCTL_VERSION/MODE_*, dumb create/map/destroy, legacy
SET_CRTC + page-flip events, enough for mesa kms_swrast + render_service
at 360x720. 4–8k lines, plus long-tail ioctls as llvmpipe probes them.

**Input.** Two options, pick per convenience: (a) add
`-device virtio-keyboard-pci,-device virtio-tablet-pci` to the qemu run
scripts (we control them; MMI only needs /dev/input/event*) and reuse the
existing virtio-input + evdev stack; (b) the Phase-0 i8042 driver. Start
with (a) for the gate; keep (b) as the real-hardware path. uinput
(injection) is Phase 4.

**Exit gate.** bootanimation renders on screen; `launcher.ready` fires;
keyboard/mouse events reach the launcher; screenshot pixel-verification
script (new, modeled on `test/ubuntu-gui/verify.py`) passes.

### Phase 4 — long tail and stability (parallelizable, no single gate)

- cgroup v1-style `memory` named mount for `/dev/memcg` + writepid
  semantics (memmgrservice/render_service); evaluate actual failure
  tolerance first — the reference boots with several cgroups missing, but
  memcg specifically is in the required set
- PSI (`/proc/pressure/memory`) — read by memmgr; evaluate tolerance
- uinput (MMI injection), `/dev/net/tun` (netmanager), audio ALSA es1370
- zram, hyperhold, hmdfs/sharefs stubs-or-real per distributed-scenario
  needs
- LTP regression parity on x86; 24h soak (`rux-agents/soak-24h/` model);
  boot-time and graphics perf pass

---

## 7. Spikes to start immediately (parallel with Phase 0)

| Spike | Question | Method | Decides |
|---|---|---|---|
| **S1: binder closed loop** | Can we implement the binder UAPI to OH's satisfaction? | On riscv64 main: minimal binder device, a probe doing BINDER_WRITE_READ loopback, then a samgr cross-check against a copied OH userspace | Phase 2 estimate confidence; surfaces ABI depth early |
| **S2: selinuxfs tolerance** | Does OH init survive selinuxfs mount / policy-load failure? | Read `base/startup/init` + `libload_policy` failure paths; if unclear, emulate failure and observe | Phase 2 item 5: implement vs skip |
| **S3: fbdev HDI composer** | Is Route A viable? | **DONE 2026-10-07 — YES** (see openharmony-s3-fbdev-composer.md): VDI is dlopen-pluggable, fences -1, vsync timer; but Route A requires a GPU-off (Skia CPU raster) rebuild of the OH graphics stack — verify ArkUI raster launcher at Phase 3 entry | Phase 3 route selection |

All three are read/prototype work on existing riscv64 main + the oh-robot
tree; none blocks or depends on Phase 0.

---

## 8. Risk register

| # | Risk | Evidence | Mitigation |
|---|---|---|---|
| R1 | **Dual-arch maintenance collapse** (the aarch64 precedent) | `quickref.md:106`: aarch64 removed; 984 scattered arch refs | arch convergence is a Phase 0 deliverable, reviewed like any gate; x86 gets equal LTP/GUI regression from day one |
| R2 | binder ABI depth (transaction flags, scatter-gather offsets, async space exhaustion, sender info) | Android binder UAPI is ~15 years of edge cases | S1 spike; acceptance test is real samgr, not a self-written probe; budget 6–10k lines |
| R3 | render_service hidden deps (memcg writepid, PSI, DRM ioctl long tail) | render_service is `critical` in graphic.cfg | Spike S3 + tolerance experiments in Phase 3 entry; Route B fallback |
| R4 | x86 platform landmines (IST/NMI stacks, TSC sync across vCPUs, APIC quirks, SMIs under KVM) | standard x86 bring-up fare | fixed single platform (q35+KVM, `-cpu host` not required — bring up under TCG first, switch to KVM for perf); the existing THREAD=single deterministic mode maps to `-accel tcg,thread=single` |
| R5 | OH 6.1 master line moves under us | oh-robot tracks master | pin: treat the current oh-robot tree + built images as the frozen target; re-sync only at Phase boundaries |
| R6 | `printk_devkmsg`/sysctl write failures wedge first-stage init | untested | Phase 1 entry experiment #1; cheap to find, cheap to stub |
| R7 | uevent payload mismatches break ueventd device creation | our uevent path was verified against mdev, not OH ueventd | Phase 1 gate explicitly covers `/dev/block/vdX` creation; diff our uevent env against Linux's kobject_uevent_net_broadcast format field by field |

---

## 9. Standing decisions (recorded so we don't relitigate)

1. **Kernel identity stays Linux-compatible** (existing policy): `uname`
   reports Linux-compatible strings; OH checks kernel identity in places
   (e.g. `ohos.boot.hardware` handling is userspace; kernel stays generic).
2. **We own the QEMU invocation and the image assembly.** Allowed
   modifications, used sparingly and recorded here: adding
   virtio-input-pci devices (Phase 3 input), dropping fscrypt from fstab,
   anything required by Spike/Phase gates. Not allowed: changing the OH
   component set, services, or boot chain order — the goal is Rux running
   this system, not a modified system.
3. **Nothing in §3.2 is implemented before it is proven needed.** The
   reference log is the authority; if a de-scoped item turns out to block
   a later phase, it enters that phase's scope explicitly.
4. **Phase gates are hard.** Phase 1+ work does not start before the
   Phase 0 gate passes; this avoids debugging OH symptoms that are really
   architecture bugs.
5. Rux keeps riscv64 as the co-primary platform; x86 regressions the
   riscv64 LTP/GUI suites must not land (CI parity, same review standard
   as the Ubuntu gate).

---

## Appendix A — reference boot milestones (kernel.log, q35+KVM, 6 vCPU)

| t (s) | Event |
|---|---|
| 0.67 | ashmem init; hilog register fails (EBUSY) — tolerated |
| 1.31 | `/init` (init_early) runs |
| 1.75 | ueventd + watchdog_service |
| 2.75–2.83 | hilogd, samgr, appspawn, hdf_devmgr |
| 3.43 | `bootevent.hdf_devmgr.ready` |
| 3.55 | render_service starts |
| 5.0 | foundation (ams/bms/wms/…) |
| 5.10 | bootanimation.started |
| 8.72 | launcher.ready + lockscreen |
| 10.33 | `All boot events are fired, boot complete now` |

## Appendix B — key reference paths

- OH tree: `/home/william/workspace/oh-robot`
- Reference boot log: `kernel.log` (5,929 lines) at the tree root
- Kernel defconfig: `device/board/edu/virt/kernel/configs/x86_64_virt_defconfig`
- Final kernel .config: `out/x86_64_virt/kernel/OBJ/linux-6.6/.config`
- Built images: `out/x86_64_virt/packages/phone/images/`
- Ramdisk staging: `out/x86_64_virt/packages/phone/ramdisk/`
- init sources: `base/startup/init/services/init/standard/`
- ueventd sources: `base/startup/init/ueventd/`
- HDI host definitions: `/vendor/etc/init/hdf_devhost.cfg` (in image);
  composer/audio config under `vendor/edu/x86_64_virt/`
