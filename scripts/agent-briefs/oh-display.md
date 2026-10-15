# Agent oh-display (worktree /home/william/rux-agents/oh-display, branch feature/oh-display)

Mission: OH Phase 2 display — get the OpenHarmony display composer chain to
render on screen (bootanimation → launcher) on the Rux x86_64 kernel. The
gate is a QEMU `screendump` showing non-black OH UI pixels plus the composer
service alive in `hidumper -s 3301`-equivalent serial evidence.

## Where the work stands (2026-10-15)

Branch has two unmerged WIP commits on top of main (6bf12a15):

- `67a7a03a` — earlier quota-pause snapshot
- `5073e2b5` — user-pause snapshot, the last working state:
  - `kernel/src/drivers/drm/mod.rs` (~1963 lines) — DRM/KMS core:
    /dev/dri/card0 + renderD128, modeset/page-flip path. Verify what
    ioctls the OH composer actually issues vs what we answer.
  - `kernel/src/fs/devfs/mod.rs` — DRM node open tracing: every
    open("/dev/dri/card0" | renderD128) prints `drm: open <name> (#n) by
    pid <p>` — the primary serial visibility into composer/gralloc startup.
  - `kernel/src/syscall/process.rs` — VFS-first exec read path: the old
    raw-ext4 shortcut resolved only against the root superblock, so
    /vendor/bin/hdf_devhost (every display *_host service) got ENOENT.
    Now opens through the VFS in 16K chunks.
  - `Kernel.toml` — kernel_heap_size 128 → 512 MB (DRM ioctls + OH
    service storm exhausted 128MB).

The frontier when work stopped: hdf_devhost exec was just fixed; the next
boot needs to confirm the display *_host services now start and reach
`drm: open card0`. Serial logs of boots 19–23 lived in /tmp and were lost
to a reboot — expect to re-run boots from scratch.

## Environment

- OH images: `/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/`
  (ramdisk, updater, system, vendor, sys_prod, chip_prod, userdata — ~15GB).
  /tmp is wiped on reboot: stage copies (or symlinks) under
  `/home/william/rux-agents/oh-display/run/oh-img/`, NOT /tmp.
- Boot: `KERNEL=<bzImage> ./test/run-oh.sh <imgdir> gui` (q35, 720x1280
  virtio-gpu, all six disks, ohos.* cmdline tokens verbatim).
- Build:
  ```
  echo 'platform_target=x86_64' > build/.config
  cargo build --target x86_64-unknown-none --no-default-features --features x86_64
  python3 build/mkbzimage.py target/x86_64-unknown-none/debug/rux target/x86_64-unknown-none/debug/bzImage
  ```
- Screenshot: pipe QEMU monitor commands (`screendump /path/out.ppm`) via
  stdin to the run script's serial/monitor, or use `-monitor` + socat;
  PPMs can be checked for non-black pixel counts with python.
- Reference: `docs/development/openharmony-phase1-runbook.md` (Phase 1
  gate: 74 services up, stable 300s — already merged to main).

## Likely next steps (ranked)

1. Boot the current branch kernel, confirm hdf_devhost execs and
   `drm: open card0` appears. If the composer opens card0 but stalls,
   capture which DRM ioctl it blocks on (add temporary return tracing).
2. Complete the ioctl set the composer needs: DRM_IOCTL_VERSION,
   SET_CLIENT_CAP, GET_CAP, MODE_GETRESOURCES, MODE_GETCONNECTOR,
   MODE_GETENCODER, MODE_SETCRTC, dumb-buffer create/map, and the
   page-flip (MODE_ATOMIC or MODE_PAGE_FLIP) path.
3. gralloc side: renderD128 open + GBM-style allocation ioctls.
4. Screenshot after each milestone; commit per milestone.

## Rules

- Commit often (English messages); before any long boot campaign, commit
  a `wip(snapshot)` first. If the agent dies to quota exhaustion, the
  branch must hold everything needed to resume.
- NEVER launch detached test loops (`setsid`/`nohup` + `&`) — they
  survive agent death and keep spawning QEMUs for hours. Run long boots
  as tracked background Bash (run_in_background), and kill only QEMU PIDs
  you started (never `pkill qemu`).
- Boot your own image copies; shared images always `-snapshot` +
  `file.locking=off`. Default test runs to `-smp 2 -m 2G`.
- Do NOT touch main or other worktrees. The main session merges.
- `build/.config` holds your platform choice; leave it as x86_64 while
  you work here (this worktree is x86-only).
