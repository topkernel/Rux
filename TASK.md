# Lane: OH boot — SERVICE-SIDE of the black screen (complementary to the oh-launcher graphics lane)
A sibling agent (worktree /home/william/rux-agents/oh-launcher) owns the SCANOUT path. YOU own the SERVICE side: why the graphics services misbehave/die. DO NOT touch drivers/gpu/**, drivers/drm/** or kernel/src/drivers/virtio/virtio_gpu.rs (sibling's files). Everything else kernel-side is yours if a root cause lands there.

## STATE (oh-svc session 2026-10-31, commits 428328dd..a1b147ff)
Ladder positions: (1) render_service abort PARTIALLY named — see T1. (3) /data cascade RESOLVED. (2) token RESOLVED as a consequence.

### T3 RESOLVED — the /data first-boot ENOENT cascade (root causes, all three stacked)
1. **The central-store seed userdata.img is a 16G filesystem truncated into a 1.3G file.** The ext4 mount geometry guard (total_blocks > device_blocks) EIOed EVERY mount attempt — /data was never mounted in ANY sprint boot. The "metadata_csum rw mount downgraded to read-only" warning printed next to it was a red herring (downgrade was non-fatal).
2. **metadata_csum rw mounts were refused by policy.** Fixed: the write path now maintains ALL checksum classes (csum.rs helpers + wiring): sb re-sealed on free-count writes (allocator + namei), GDs re-sealed after every descriptor write, bitmap csums (bg_*_bitmap_csum_lo/hi) refreshed on every bitmap write (allocator + namei's mark_inode_used/free_inode), dir blocks sealed with ext4_dir_tail on every finalized write (add-entry/fresh-block/mkdir/delete/dotdot + mod.rs add_dir_entry/append_dir_block), find_entry_space never hands out a slot overlapping the tail. The rw→ro downgrade is LIFTED (ext4/mod.rs).
3. **The cook lineage (userdata-cook1.img) wedges init forever**: host e2fsck on the truncated seed reconnected THOUSANDS of orphan inodes into lost+found/#<ino>; init walks that directory with fstatat per entry — each lookup scans the whole ~400-block dir (O(N²) walk, looked like a hang; NOT a hang). Do NOT cook the truncated seed.

**THE WORKING LINEAGE: run/userdata-fresh.img = mke2fs 16G ext4 -b 4096 -I 256 -O metadata_csum,64bit,has_journal,extent,huge_file,dir_nlink,extra_isize,flex_bg,dir_index,sparse_super,filetype,large_file,^orphan_file,^metadata_csum_seed.** Boot it with run/oh-boot-cook.sh <tag> /home/william/rux-agents/oh-svc/run/userdata-fresh.img. First boot is SLOW (population, ~8min); second+ boots fast. It persists (no -snapshot on that drive).

### T2 RESOLVED (via T3) — composer_host native token
With /data mounted rw, init creates /data/service/el0/access_token and the nativetoken file; the boot logs `ATM: [InitNativeTokenInfos]Init native token ... composer_host, permSize 0 ok!`. "gettotkenid failed" is GONE on fresh-lineage boots.

### T1 ROOT CAUSE NAMED (2026-10-31, this session) — render_service SIGABRT
The abort chain is FULLY decoded; the fix belongs to the DRM driver (sibling's file — spec below).

1. **The abort is Mesa's llvmpipe assert `jit_tex->base`** (lp_setup.c:1072 at the image's
   built revision — oh-robot mesa3d 99c7104e75f lineage, `lp_setup_set_fragment_sampler_views`).
   For a display-target texture it does `jit_tex->base = llvmpipe_resource_map(res,0,0,READ)`
   and asserts non-NULL. The image's GL driver is `kms_swrast_dri.so` (chipsetsdk symlinks),
   so the map is `kms_sw_displaytarget_map`: `DRM_IOCTL_MODE_MAP_DUMB` then
   `mmap(PROT_READ, MAP_SHARED, node_fd, handle<<12)`. NULL => assert => SIGABRT.
2. **The failing call is CAPTURED** (boot oh-glm5, kernel taps in io.rs/memory.rs):
   `drm: MAP_DUMB pid=945 fd=9 handle=0x3eb ret=-22` two lines before the assert, right after
   `RenderFrame: Canvas` + the previous frame's dt unmap (the 2 BIGMUNMAPs). handle 0x3eb=1003
   is the client buffer RS composited fine one frame earlier (MAP_DUMB ret=0 at 47765).
3. **Root cause (kernel, drivers/drm/mod.rs): the gems table is GLOBAL and `gem_close`
   (line ~1687) removes the entry unconditionally.** DESTROY_DUMB dispatches to the same
   gem_close and its encoding MATCHES OH libdrm (ioc(3,0xb4,4)), so the gralloc/DrmAllocator
   side can (and does) delete the shared entry while render_service still needs it.
   Reference kernels use PER-FILE handle namespaces: the exporter's destroy removes only its
   own file's handle; the object lives while any file/dma-buf references it
   (linux-5.10/6.6 drm_prime.c `drm_gem_prime_fd_to_handle` + drm_gem_handle_delete).
   RS caches EGLImages per buffer key (`RSEglImageManager::MapEglImageFromSurfaceBuffer:
   265993852991569922` repeats with NO re-import in the failing frames), so nothing
   re-registers the entry before the map.
4. **Secondary crash explained (NOT a kernel bug)**: after the abort, faultloggerd's
   in-process handler dies with SIGSEGV — musl `__memcpy_fwd+0x24` writing a RO/PROTNONE page
   (~0x7ff00067c000). Same libdfx_signalhandler 936-byte-context-copy-overflows-the-guard
   family already adjudicated IMAGE-SIDE in leg-31B (commit 5615ff48); mitigation =
   run/patch-ldmusl-guardread.sh (oh-launcher lane) applied to the private images if crash
   reports are ever needed. It does not affect service survival.
5. **GEM_CLOSE is dead for ALL userspace on Rux** (probe + trace proven): kernel expects
   ioc(1,0x09,4) but OH libdrm's `struct drm_gem_close` is 8 bytes => 0xC0086409 => ENOTTY
   (trace: `drm: ioctl-fail req=0x40086409 ret=-25` from the allocator every boot). Same
   encoding-mismatch class the sibling fixed for PAGE_FLIP/RMFB (oh-launcher 0732f884).

### T1 FIX SPEC (for the oh-launcher lane / whoever owns drivers/drm — NOT this lane's files)
Fix 1 (one line class): dispatch GEM_CLOSE with BOTH encodings — add
`DRM_IOCTL_GEM_CLOSE_8 = ioc(1, 0x09, 8)` alongside the 4-byte constant (OH libdrm struct is
{u32 handle; u32 pad}). Same pattern as the PAGE_FLIP upstream-encoding fix.
Fix 2 (the abort): `gem_close` must NOT drop a handle other files still reference. Minimal
global-table proxy for per-file namespaces:
```rust
let Some(gem) = DRM_STATE.lock_irqsave().gems.get(&handle).cloned() else { return -22 };
// Reference semantics: drm_gem_handle_delete removes only the CALLING FILE's handle;
// the object stays alive for every other importer. Global-table proxy: another live
// kernel-side reference (prime file, FB binding) keeps the entry.
if alloc::sync::Arc::strong_count(&gem) > 1 { return 0; }
// else remove as today (drop outside the state lock — GemObject::drop runs virtio unref)
```
Verified geometry: the allocator's DESTROY_DUMB-while-RS-holds-its-dup'd-prime-fd lands
count>1 => entry survives => MAP_DUMB works => assert gone. End-state (record in
design-divergences.md later): per-file handle tables, the reference design.
Fix 3 (FYI, correct as-is): prime_handle_to_fd/fd_to_handle read handle@0/fd@8 — matches
OH's vendored libdrm layout ({handle,flags,fd}) — do NOT "fix" to upstream layout.

### T1 PROOF ARTIFACTS (this session)
- run/glmprobe.c (+ built run/glmprobe): single-process repro of the llvmpipe chain —
  CREATE_DUMB(0xb2! per OH drm.h, NOT upstream 0xb4) -> HANDLE_TO_FD -> FD_TO_HANDLE x2
  (dedupe SAME-HANDLE) -> MAP_DUMB + PROT_READ mmap + read, re-import, RW mmap — ALL PASS,
  so the chain itself is sound; the failure is the cross-process destroy ordering.
  Injected via run/system-glmprobe.img (reflink of system-ohsvc.img, key_enable slot,
  sandbox:0 console:1 cfg). Boot driver: `SECS=600 setsid bash run/oh-boot-cook.sh <tag>
  <userdata> ` with run/oh-img/system.img repointed at the probe image (restore afterwards!).
- Kernel taps committed in THIS lane's files: sys_ioctl MAP_DUMB/fail traces (io.rs),
  drm-mmap placement-ENOMEM attribution (memory.rs). Budgets 64/40/20, ungated, cheap.
- Boot oh-glm5 serial: run/oh-glm5-serial.log (the capture is at line ~49240).


### PRIVATE IMAGES (this worktree's run/, never the central store)
- run/system-ohsvc.img = central system.img copy + `rm /system/bin/resize2fs` (silences the fstab resize attempt; resize2fs/e2fsck cannot open /dev/block/vdf anyway, see known-gaps) + symlink /system/lib64/libGLES_mali.so → /vendor/lib64/chipsetsdk/swrast_dri.so.
- run/vendor-ohsvc.img = central vendor.img copy + symlinks /lib64/libGLES_mali.so and /lib64/chipsetsdk/libGLES_mali.so → chipsetsdk/swrast_dri.so.
- run/oh-img/system.img + vendor.img symlinks POINT AT THESE (svc boots use them automatically).

### KNOWN GAPS (successor queue, in order)
1. **e2fsck-cleanliness of the written fresh image**: guest-side self-consistency HOLDS (the kernel's own mount-time csum verification passes across repeated boots), but host e2fsck 1.47.2 rejects it ("superblock checksum mismatch" → backup; inode/bitmap csum mismatches). Root suspicion: csum.rs's inode_csum span/formula for 256-byte inodes with the extent root tail, and possibly the bitmap-csum byte span. Verify csum.rs against `e2fsck` on a small test image (host loop, no boot needed) before touching anything. Until then debugfs on the image needs `-b 4096 -s 32768` and still trips bitmap csums — extract files via guest-side instead (tarsync over serial, or fix the csum first).
2. **devfs block-device nodes cannot be opened at all** (devfs_get_file_ops returns None for every BlockDevice → ENXIO): resize2fs/e2fsck (fstab check/resize on /data) always fail with "while opening /dev/block/vdf". Real fix = block-device FileOps routed to the registered GenDisk by devno (+ O_EXCL semantics later).
3. **render_service registers no SAMGR ability** on fresh-lineage boots (clients spam "RenderService connect fail"); it also didn't render before aborting. Cross-check with the sibling: display HDI/vsync ARE up (DISP lines + drm import 720x1280 succeed).
4. svcdbg/ABORT taps: hunt instrumentation, budgeted; drop or gate behind dfx= switches before any perf-gate merge.

## Booting
run/oh-boot.sh <tag> (snapshot boots, tok-era flags) — run/oh-boot-cook.sh <tag> [userdata] [kernel] for writable /data. run/oh-boot-cook-gdb.sh adds -gdb tcp::1239. SECS env. One QEMU at a time; check `ps aux | grep qemu` (sibling boots too!); kill only your own; pkill FORBIDDEN (note: `pkill -f <tag>` matches your own compound command — use the ps|kill loop).
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

## LAUNCHER-BRINGUP lane state (2026-10-31 12:xx, session continues)
Worktree on branch oh-launcher @ 773f701e (base main 40e925fd). All boots
q35+KVM x86_64, GL-fixed images (oh-svc system/vendor), snapshot everywhere,
fresh userdata every boot. Driver: run/lb-boot.py <tag> <secs> [sysimg]
[kernel] [extra-cmdline] — per-line wall timestamps, launcher markers, shots.

### Ladder position: launcher SPAWNS and reports ready (boot-dependent race);
screen still shows the frozen bootanimation logo at 780s (sysc1 boot).
1. sysc1 (syscost=1 boot): `bootevent.launcher.ready` at 541.8s — appspawn
   spawned com.ohos.launcher, the ability initialized. Shots 420-780s all the
   frozen OH logo frame (2.5% pixels, md5 06b933b4) — launcher not drawing
   yet (ArkUI first frame slow under the current perf; or no composition).
2. lb1 (clean, no syscost): bootanimation.ready 253.9s, install stream
   FINISHED at ~324s (systemres last), `bootevent.bms.main.bundles.ready`
   331s — then WEDGED: accountmgr (SA 200) never received ANY
   OnAddSystemAbility callback (its first log line, account_mgr_service.cpp:616,
   never printed), so user 100 was never created (QueryActiveOsAccountIds:
   "no accounts"), so AMS never called StartUser → no launcher spawn. Direct
   client→accountmgr binder queries worked (SAREG reply from accountmgr at
   254.2s), so the accountmgr service itself is up; the samgr→client
   NOTIFICATION direction is the suspect. Boot-to-boot race: sysc1 got
   through (ActivateDefaultOsAccount 332.7s, appspawn lockstatus_100 333.4s),
   lb1 did not.
3. 8521220 DECODED: ERR_BUNDLE_MANAGER_BUNDLE_NOT_EXIST — BMS answer while
   the preinstall stream hasn't reached com.ohos.launcher yet. NOT a bug in
   itself; a symptom of the slow install stream.
4. sprux1's "reboot" at 442s: the stockperf pbench injector's probe ran as
   key_enable IN INIT'S PROCESS GROUP (pgid 1; OH init never setpgids
   services) and its cleanup kill(0,SIGTERM) hit init → ExecReboot →
   StopAllServices → samgr died → "no context manager". Same behavior on
   the reference kernel (init blocks SIGTERM for signalfd, so it consumes
   it) — NOT a kernel bug, but the injector MUST setpgid(0,0) before any
   group kill. sprux1 markers before that: bootanimation.ready 228s (ref
   kernel: 8.3s; full ref boot to launcher.ready: 14.4s).

### ROOT CAUSE OF THE LAUNCHER NEVER SPAWNING (lb2, 2026-10-31 12:2x)
mount("hmdfs") → -EINVAL (Rux has no hmdfs filesystem). Chain:
StorageDaemonProvider::StartUser → MountManager::MountByUser →
MountFileSystem → MountHmdfs: SupportHmdfs() reads
const.distributed_file_property.enabled (from
/system/etc/param/distributed_file.para, =true in the stock image) →
mount('/data/service/el2/100/hmdfs/account','/mnt/hmdfs/100/account',
type 'hmdfs') fails → StartUser failed → UnlockUser ends errCode
13600721 → osaccount NEVER calls AbilityManagerAdapter::StartUser(100)
→ AMS never starts the launcher root; the Ext/ABCM load-timeout retry
loop then queries BMS with userId 0 forever (8521220, launcher only
exists for user 100). The reference OHOS 6.6.101 kernel has CONFIG_HMDFS
so the same mounts succeed there.
FIX (image, this lane): run/lb-mkimage.sh builds run/system-lb.img =
GL-fixed system image with that one param flipped to false →
SupportHmdfs()=false → LocalMount path = pure BIND mounts of
/data/service/el2/100/hmdfs/account onto /storage/media/100/local and
/storage/cloud/100 (see storage_mount_info.json hmdfs-mount nodes with
the local_hmdfs option) → StartUser should return E_OK.
(Kernel-side alternative for later: an "hmdfs" fstype alias to bind.)

### Next steps (in order)
1. lb3 (RUNNING, 1800s, system-lb.img + TFDC kernel): watch ~254s for
   "SupportHmdfs ... false", LocalMount bind success, StartUser OK, AMS
   StartUser(100), launcher spawn; then shots past 600s for pixels.
   TFDC prints name the timerfd_create storm caller. (lb2, same class of
   boot without the fix, was killed at ~1000s wedged in the ABCM retry
   loop; its SACB data validated the callback tap: samgr code=1 ONEWAY
   deliveries to accountmgr appear right after subscription on good
   boots.)

### SWITCH_ROOT MOUNT HUNT (lb3-lb9, final state of this session)
The hmdfs image fix was necessary but NOT sufficient. The next layer:
- Kernel-confirmed (lb4-lb9, mkdirfail widened): pid 1's mkdir
  '/mnt/sandbox/system/vendor' -> EROFS at ~4.6s and mkdir
  '/storage/media' -> ENOENT at ~9.8s; storage_daemon (pid 324)
  StartUser(100) mkdirs '/storage/media/100*' fail -2 the same way
  (kernel errno), so StartUser fails → UnlockUser 13600721 → no AMS
  StartUser(100) → no launcher, on EVERY boot regardless of the
  accountmgr-callback race.
- OH switch_root mechanics (lb6 MSMOVE map): first-stage init mounts
  tmpfs on /mnt,/mnt/data,/storage (ramdisk root, ~1.1s), MS_MOVEs
  /dev,/proc,/sys,/run,/mnt,/mnt/data,/storage into /usr/*, then
  MS_MOVEs /usr onto / and chroots. All 8 moves now print (MSMOVE tap;
  MS_MOVE returns before the generic mount tap, which hid them before).
  /proc/mounts ORDER decides parent-before-child; lb5's boot had the
  bad order and the /mnt/data move failed err=2 (mnt-precheck-fail
  would now name it); lb6+ had the good order.
- PROVEN CORRECT (SWROOT-CHILDREN + WALKPROBE taps, lb7/lb9): after the
  /usr→/ overlay the VFS root's children are dev/mnt/proc/run/storage/
  sys, ALL with (M) mounts; task root == VFS root (same pointer),
  procroot '/', vfsroot-storage-mount=true vfsroot-mnt-mount=true AT
  THE MOMENT of the failing walk.
- STILL BROKEN: the per-component resolution inside path_lookup /
  lookup_parent_dir for 'mnt'/'storage' lands on the read-only ext4
  inode instead of crossing the mount. NEXT PROBE: inside path_lookup's
  component loop, for components named mnt/storage at depth 1, print
  lookup_child ptr + has-mount + follow_mount result (inode ptr/fstype)
  + the parent-vpath the mkdir actually used (vfs_mkdir stage trace).
  Suspects: VfsPath/dentry identity mismatch between the walk and
  file_mkdir's parent, or the mount's root dentry inode being swapped
  by the root overlay (root.set_inode(inode) mutates the ROOT dentry's
  inode — mnt.root's dentry is the same object as the pre-overlay /usr
  mount root; check whether the moved mounts' roots got their inodes
  clobbered by rehome/set_inode interplay).
- ALSO still open (independent): the accountmgr OnAddSystemAbility
  race (bad boots: lb1/lb4-style, zero ABCM retries, no user creation;
  good boots: sysc1/lb2/lb3/lb5). SACB tap in place to catch the loss.

### PERF CENSUS (syscost=1 boot sysc1, dumps at t=60/120/180/240s) — handoff
for the perf lane (guarded domains: sched + fs + ipc; raw log:
run/oh-sysc1-serial.log in THIS worktree):
- Whole-boot shape: ref kernel (stock OHOS 6.6.101 bzImage, same images,
  same machine class) reaches launcher.ready in 14.4s. Rux: bootanimation
  ~254s, install stream done ~324s, launcher.ready 541s (on a good boot).
  The wall is NOT compute: init spends 129s stalled after deviceauth,
  135s on `start asset_service` (condition-wait on DATA_SHARE_READY),
  futex avg 17ms→108ms as the boot progresses.
- **Futex wake→run latency is tick-quantized**: slow-ring full of ~10ms
  and ~30ms futex waits; TaskExecutorRDB futex max 30s; WAKE-STRANDED
  pid=1 strand=131s matches init's 135s stall. Wake path has preempt
  checks + idle-CPU IPIs (sched.rs:3300) but under this load woken tasks
  wait whole ticks. THE master multiplier (every IPC handoff pays it).
- **UART console tax**: prints cost 0.36-0.93ms EACH (LSR polling under
  KVM, multi-poll 68%/100% on the two CPUs = console-lock serialization);
  26k prints/60s ≈ 15s CPU per 60s window. The boot's DFX taps (43k
  "pagefault: cow" pr_debug lines at loglevel=7, 9k MREMAP-CFI pr_info,
  KSIG per-send prints, 30k slog/ilog/alog hilog-socket echoes with
  kernel-side UTF-16 decode+match per send) are most of that volume.
  Gating/removing them before any perf-gate run is mandatory anyway
  (known-gap 4 of the svc charter).
- **hilog socket traffic**: hilogd.server 703k recvfrom in 60s, 5.06M by
  240s (21k/s sustained), recvfrom avg 353µs. Every send pays the kernel
  hilog tap decode. mmi_service 185k recvfrom.
- **timerfd_create storm (nr=281 on x86_64 — NOT execve; execfail2 tap
  disproved the failed-exec theory, only 4 benign one-shots)**: 111k
  calls in first 60s, 833k by 240s (accelerating 1.9k→4.3k/s), avg
  ~1.6-1.9ms each — several CPUs of kernel work (rmap/page_record_mapping
  = 28% of kernel-tick rip histogram; slab churn 24M hits fit an
  fd/object create+destroy loop). WHO calls timerfd_create 4k/s is
  unnamed — next step: a budgeted timerfd_create failure/comm tap, or a
  per-comm counter in syscost's flood table (it currently only tracks
  recvmsg-family floods).
- openat avg 252µs (85k/60s), read avg 1.1ms — fs path/read 5-10x slow;
  secondary next to the above.
- ticks at t=60s: u/k/i = 512/5130/352 per CPU — 85% kernel mode.

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

---
# (svc-lane charter section, preserved at merge)
Small commits, English, NEVER "Linux". x3 builds rc=0 before push. Push: origin/oh-svc. Quota/concurrency deaths: commit early; TASK.md persists.
## Ladder
1. DONE (2026-10-31): abort reason FULLY named — llvmpipe `assert(jit_tex->base)` from a
   cross-process GEM-handle destroy (see T1 above + FIX SPEC — the fix is a ~20-line
   drivers/drm/mod.rs change, sibling's file).
2. NEXT: land the T1 Fix Spec (Fix 1 + Fix 2) in drivers/drm/mod.rs, rebuild, boot —
   expect render_service to survive its first life; then watch for SAMGR registration
   (known-gap 3 may still hold) and the next blocker. After the fix boots clean, remove or
   gate the T1 traces (io.rs/memory.rs) before any perf-gate merge (see known-gap 4).
3. DONE (fresh lineage + csum write path + token OK). 4. With the sibling: bootanimation/launcher pixels.
