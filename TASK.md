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

### SWITCH_ROOT MOUNT HUNT — RESOLVED (2026-10-31 15h session, commits a3e6e27e + 88d92fa9 + 261e3325)

The "per-component resolution" theory was WRONG — the mounts were fine. The real
chain, fully proven on boot lb10 (serial run/oh-lb10-serial.log, shots oh-lb10-shot-*):

1. **The walk never reached the mounts because pid 1 was JAILED.** OH's
   `make sandbox system` (DoMkSandbox) = InitDefaultNamespace(open
   /proc/self/ns/mnt) + unshare(CLONE_NEWNS) + PrepareSandbox (the ~30 binds +
   pivot_root(sandbox, sandbox) + umount2(".", MNT_DETACH)) + **EnterDefaultNamespace
   (setns back to the pre-pivot ns)**. On Rux ALL THREE legs failed:
   a. pivot_root is JAIL-style (fs root prefix on the shared dentry tree) —
      after it, init's root = /mnt/sandbox/system.
   b. open("/proc/self/ns/mnt") from the jailed task ENOENTed — the ns magic
      link resolves via the STRING walk ("mnt:[N]" is not a path) and the
      syscall-layer ns shortcut skips chrooted tasks. So g_defaultNs was -1.
   c. setns(CLONE_NEWNS) never reset the jail root. Init stayed in the sandbox
      FOREVER → /storage,/mnt/... resolve inside the sandbox (system-sandbox.json
      has NO storage bind) → mkdir /storage/media ENOENT → StartUser fail.
2. **FIXES (a3e6e27e)**: ns magic links are TERMINAL in follow_symlink (like fd
   links) and file_open builds the ns fd from the (pid,kind) inode — works
   jailed; MountNamespace.root now records the pivot target; setns(CLONE_NEWNS)
   re-anchors: pivoted task → non-pivoted ns = root back to "/", entering a
   pivoted ns = root to the ns pivot root (root_is_pivot flag; plain chroot
   keeps its jail); CLONE_NEWNS inherits the parent's pivot anchor.
3. **FIXES (88d92fa9)**: MS_MOVE source walk now crosses mounts on intermediate
   components (/mnt/data move was ENOENT); vfs_mkdir/mknod/symlink decide EEXIST
   BEFORE the EROFS guard (reference order) — OH's MakeDirRecursive over the ro
   /vendor bind needs EEXIST.
4. **PROVEN on lb10**: setns-mnt taps show pid 1 reset to '/' after BOTH sandbox
   pivots; chipset sandbox now prepares (it failed at InitSandbox before);
   service children re-anchor into the sandbox ns; mkdir /storage/media SUCCEEDS;
   StorageDaemon StartUser(100) completes ("SD_DURATION: START USER: delay time
   = 106 ms" on lb14); PrepareUserDirs(100) runs; bootanimation plays and
   finishes; bootevent.appfwk.ready set. Ladder steps 1-3 of the charter: DONE.

### CURRENT BLOCKER (lb10-lb14): the PERF wall + two OH-side holes under it
- **lb10** (loud, smp2): install stream ok; foundation #1 (pid 976) runs the
  account phase, launcher start in flight → at 385s foundation is KILLED BY ITS
  OWN DFX watchdog ('OS_DfxIpcCheck' group-kills 41 threads — IPC stall >6s
  under the 26x slowdown). Restarted foundation #2's ABCM restore loop queries
  BMS with **userId 0** (foreground user never set: GetForegroundOsAccountLocalId
  fails — 249 hits) → 8521220 forever → **the launcher app is NEVER spawned**
  ("restart ability" x134, no comm='com.ohos.launcher' process ever exists).
  The userId-0 is OH-side (AMS restart path defaults to foreground user); on the
  reference kernel foundation never dies so the path never runs.
- **lb12** (quiet loglevel=3, smp4): WORSE — black screen past 544s. 4 CPUs on
  the GRQ kernel = contention regression. Do not use smp4 for OH boots.
- **lb13/lb14** (quiet, smp2): ~1.7-2x faster early (bootanimation.ready 226s
  vs 240s, finished 236s vs 294s, appfwk.ready 328s vs 401s, StartUser(100)
  completes at 352s in 106ms). THEN the state-dependent IPC collapse: SLOW-TXN
  foundation→samgr code 7 grow to 28-30s exec each (lb14 486-855s), WAKE-STRANDED
  1-2.6s with incumbents at preempt_count 1-2, PARKED-LEAK prints (pids 314/546
  park with pc=1). account.ready never lands; screen keeps the frozen logo.
- The quiet-console lever is real but bounded: loglevel=3 kills ~120k tap lines
  (cow 40k, hilog echoes 39k, MREMAP 15k, FXRS 8k) pre-format; the heavy taps
  are now pr_debug (261e3325) so LOGLEVEL=6 boots keep [kmsg] markers + kill
  traces without the flood. lb-boot.py takes SMP= and LOGLEVEL= env now.
- NEXT for this lane (or perf lane): the post-StartUser stall is the census's
  "deviceauth 129s / asset_service 135s DATA_SHARE_READY / futex 17→108ms"
  wall — it is NOT serial tax (quiet boot still collapses). Suspects named by
  the taps: futex wakes stranded behind preempt_count>0 sections (1-2.6s!),
  PARKED-LEAK (park with pc=1), samgr 30s serial txns. This is guarded-domain
  sched work; any fix needs the perf gate before main.

### Boot recipe (current)
`LOGLEVEL=6 python3 run/lb-boot.py <tag> 2400 run/system-lb.img target/x86_64-unknown-none/debug/bzImage`
(smp2 default; do NOT smp4). Markers + kill traces visible, tap flood demoted.
test/rootfs.img must exist for `make test` (copy from the main repo if missing).

### The userId-0 launcher hole (OH-side, decoded on lb10/lb14 — for whoever kills it next)
- The launcher start that creates the ability RECORD uses **userId 0** (lb10:
  first "AppMS: [AMSI1640]userId: 0, bundleName: com.ohos.launcher" at 310.5s,
  BEFORE foundation's watchdog death; the record's user stays 0 forever, so every
  later BMS query is 8521220 = NOT_EXIST and ABCM's restart loop can never
  succeed — "restart ability" x134, no com.ohos.launcher process EVER spawns).
- userId 0 traces to the foreground user never being set:
  GetForegroundOsAccountLocalId fails 249x (foregroundAccountMap_ empty —
  the activation chain had not reached the foreground-set step when the start
  was issued). On the reference kernel the whole activation completes in
  ~10s so the ordering holds; Rux's account phase takes 300-800s and the
  start races ahead of it.
- Candidate kills (UNTESTED): (a) make the boot faster (see the perf suspects
  above); (b) image-side: patch/param OH so the launcher start waits for
  account activation; (c) kernel-side DFX: none needed — the chain is healthy.
- systemui DOES spawn from StartUser (m.ohos.systemui, lb10 441s; it later
  THREAD_BLOCK_6S but survives) — only the launcher start is mis-usered.

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

### Session close (2026-10-31 ~16:30)
- lb15 (quiet smp2, same kernel): fastest boot yet — bootanimation.ready 212s,
  finished 226s, appfwk.ready 217s, StartUser(100) 146ms, appspawn
  lockstatus_100 set — but the account SA never loaded (no AccountMgrService
  lines at all by 1838s): the wedge moved EARLIER into the scan phase. The
  collapse point moves boot-to-boot ⇒ generic load collapse, not one bug.
  Screen stays the frozen logo frame (md5 06b933b4) on every lb1x boot.
- x3 builds re-verified rc=0 after the tap demotions; make test green

## THIRD-SUCCESSOR SESSION (2026-10-31 15:2x-17h, branch oh-launcher @ beb59243+)
Base: main e8cc17ec merged FIRST (the branch was 67 commits behind and did NOT
carry the samgr lane's rotation-proof PREEMPT-HEAL c354069a+047dbb7d — every
lb1x boot ran WITHOUT it; that alone explains lb13/14/15's zero heals).

### Landed (all pushed)
1. merge main e8cc17ec (rotation-proof heal, 2s bound, cumulative bar).
2. **WAKE-HEAL** (sched.rs, wake_up_enqueue_opt tail): a task about to be
   woken, GRQ held, owned by no CPU slot (verified), not mid-switch-out
   (with 50-jiffy stale-marker confirmation — a stale SWITCHING_OUT entry
   was the silent forever-skip for the carrier tasks), not PF_EXITING,
   carries NO legal guard by construction (sc265's conviction) → its
   phantom preempt_count is zeroed AT WAKE so the monopoly never forms.
   Skip-reason census: `why=e/s/c` budgeted 60; heal report budgeted 300.
   lb18 measured: fires 268/boot, strands 13 (vs 21-36 before), lb20
   early: 0 strands/34s. ALL observed skips are why=c (task in its own
   slot — legit pre-sleep window OR lying busy slot; the lying-slot
   adjudication needs the frozen-exec-clock check, NOT done).
3. **tick-heal now sets need_resched** after clearing base bits (the zero
   alone left the incumbent running to user-return — sc267).
4. **kpreempt=1 cmdline arm of the IRQ-exit kernel-preempt hook**
   (trap.rs, default OFF, syscost-style cmdline verdict cache).

### VERDICTS (evidence in run/oh-lb19b-serial.log)
- **kpreempt is NEGATIVE in the current tree**: DEADLOCK spinlock-stuck
  prints on BOTH cpus from 54s, strands EXPLODED to 40. Mechanism: the
  2s tick-heal false-heals REAL guards held >2s (holders are themselves
  collapse victims) → preemptible() lies → the hook preempts a lock
  holder mid-critical-section → the DEADLOCK lock 0xffff88807d2876d0 is
  the same slab-region instance the underflow drops name every boot.
  Full kernel preemption needs the false-heal problem solved first.
- **The leak is a steady-state ENGINE, not one-off**: lb17 tail shows the
  same pids (0x13e/0x219/0x24c/0x425 = OS_IPC worker threads, e.g. pid
  318 = first OS_IPC worker after key_enable) re-healed every ~200ms
  with was=0x10001, underflow at spinlock.rs:461/rwlock.rs:457 drops
  (lock instances 0xffff88807d28a2d0/0xffff88807df62290 heap/slab
  region — per-inode/per-socket Spinlock family per sc73), re-leak
  immediately. Producer remains the double-run/wild-write family.
- The lock at 0xffff88807d2876d0: slab-region heap object embedding a
  Spinlock (inode.state / socket state family), deterministic address
  across boots, contended by both CPUs for seconds at a time.
- SLOW-TXN exec times up to 41.7s (device_manager code 14 during the
  install stream) — exec-time class, not queue-time: the SERVER thread
  itself runs 41s (crypto+syscalls under the collapse), same family as
  the deviceauth 129s / asset_service 135s census stalls.

### lb20 ENDGAME (read before next boot) — the wedge is now NAMED
lb20 (full 2400s, WAKE-HEAL + need_resched insurance): the healthiest boot
yet, AND the account-chain wedge is now a concrete starvation cascade:
1. accountmgr SA LOADED for the first time ever ("SAFWK: StartSystemAbilityTask
   accountmgr SA:200 init finished, 252 ms" @221.9s, pid 976; SACB callbacks
   DELIVERED to tid 976 x16 @268-311s — the samgr->client direction works).
2. softbus_server (pid 724) registered @228.8s then went SILENT — its worker
   never answers. Its pick claim was reset once (STALE-CLAIM-RESET by_cpu=0
   pid=724 @841.7s) with no recovery. This is the R85 "linked but starved /
   pick-skip wedge" family (orphaned on_cpu mark / stale claim), NOT a leak.
3. Consequence cascade, live in the log: device_manager (pid 568) code-3
   handler waits ~5.24s (softbus LNN timeout) PER CALL, x102 SLOW-TXNs from
   distributeddata's DB_TaskExecutor in a 5.75s retry loop; 104 stale-binder-
   buffer reclaims of pid 568 (its own waiters' reply buffers age out);
   distributed account queries (SAREG code=4 ohos.account.f...) spam accountmgr
   x dozens/second from 355s on; bootevent.account.ready never SET (the
   param_watcher wait at 211s hangs); useriam watchdog-killed at 641s (own
   OS_DfxWatchdog) and restarts (687->1064->1135); screen stays md5 06b933b4.
4. NO PARKED-LEAK on any softbus thread; last WAKE-STRANDED at 315s (pid 318
   behind incumbents pid 684 pc=0x102 and pid 1 pc=1) — the strand/heal
   machinery QUIETED after 315s. The final state is a stable starvation
   equilibrium, not a collapse in progress.
### Session close (third successor, 2026-10-31 ~17:50) — LADDER POSITION
Ladder 1 (foundation survives) NOT yet held; the wall moved from "generic
collapse" to ONE concrete cluster. Facts at close:
- lb21 (dfx=wwcensus, same kernel): strand volume COLLAPSED to 6/boot
  (strand_max 10.4s; lb15/16 class was 17-36) — the heal stack (rotation-
  proof 2s tick heal + WAKE-HEAL + need_resched insurance) works. All my
  QEMUs are dead; screen md5 06b933b4 (frozen logo) on lb20/lb21.
- accountmgr SA loads (both boots); samgr->client callbacks work; the
  remaining wall is the softbus/device_manager/accountmgr distributed-
  cluster handshake: every call in that cluster runs multi-second (5.25s
  device_manager timeouts on lb20; softbus->accountmgr code-14 SLOW-TXNs
  on lb21), so activation never lands inside the watchdog windows.
- Stale-claim resets keep regenerating (lb20: pids 497x2/457; lb21: 532x2/
  584/1092) — the phantom-mark producer is alive but bounded.
- Branch oh-launcher pushed through 7c850d6d + TASK commits; x3 builds
  rc=0 (x86_64/riscv64/aarch64); make test ALL PASSED twice (lb18/lb21
  maketest logs) on the final tree.

### FOURTH-SUCCESSOR ENTRY POINTS (in order)
1. The softbus starvation (lb20 evidence above): first boot with
   `dfx=wwcensus` (lb21's recipe), watch pid 7xx softbus_server threads'
   W3-RUNWAKE-REFUSED / W3-FROZEN-EXEC-SLOT lines + the PICK-SKIP reason
   codes in fair.rs's pick decision legs. If a softbus worker is linked-
   but-skipped with why=on_cpu, the R85 stale_oncpu heal's relink leg is
   the fix site (it fired STALE-CLAIM-RESET on 724 at 841s on lb20 without
   recovering the task).
2. The re-leak engine (unchanged): OS_IPC worker carriers re-acquire
   phantom +1 every ~200ms (lb17 tail); drops underflow at spinlock.rs:461/
   rwlock.rs:457 with heap lock instances in the slab region
   (0xffff88807d28a2d0/0xffff88807df62290/0xffff88807d2876d0 — the last
   one DEADLOCKED both cpus under kpreempt). Root remains the double-run/
   wild-write family (sc265/266/73 convictions; r83hunt is the tool).
3. kpreempt=1 is armed-but-NEGATIVE until guards-held->2s disappear
   (the false-heal preempts real lock holders; lb19b DEADLOCK proof).
   Prerequisite: cond_resched at lock-free boundaries of the long kernel
   loops (fork copy / exit unmap / rmap batches) OR the producer fix.
4. timerfd_create storm caller still unnamed (4k/s; the 10-line TFDC
   budget only caught boot-start callers: bluetooth/audio).
=> OH-side note: even with accountmgr up, the DISTRIBUTED account path
   (ohos.account.foreign/distributed) drags device_manager/softbus into the
   critical path at boot. The hmdfs-disable image edit killed hmdfs, but
   distributeddata's device_manager dependency remains. If the kernel
   starvation is fixed, this chain should complete on its own.

### Boot inventory this session
lb16 (merged heal): anim.ready 221s, distributeddata watchdog-killed
  (OS_DfxIpcCheck KOT-KILL 15 members) at 260s, wedge same class.
lb17 (+WAKE-HEAL v1 silent): anim.ready 254s; console stalled ~300s;
  steady re-leak engine first captured. NOTE: lb17's "launcher.ready
  marker" was a FALSE match on "launcher.ready not set" — lb-boot.py now
  rejects "not set" lines.
lb18 (+skip census +stale-marker confirm): anim.ready 226s, heals 268,
  strands 13, accountmgr started 303s, killed at 650s mid-account-phase.
lb19/lb19b (kpreempt=1): the DEADLOCK verdict above (killed early).
lb20 (running at write time): full 2400s, current branch.

### Successor entry points (in order)
1. lb20's endgame: did the account phase complete / foundation survive /
   launcher spawn? (watch AccountMgrService / ActivateDefaultOsAccount /
   StartUser / AMSI1640 userId / appspawn com.ohos.launcher).
2. The remaining strand class is why=c lying busy slots: add the
   frozen-exec-clock adjudication (W3-FROZEN-EXEC-SLOT machinery) to the
   WAKE-HEAL's curr_on skip — a slot naming a task whose sum_exec_runtime
   is frozen for 200ms+ while its state is SLEEPING is lying.
3. The per-op wall (26x): syscall wall-times include wait-behind-
   incumbents; after the monopoly fixes the residue is COW/fault cost
   (rmap 28% of kernel ticks) + the timerfd_create storm (4k/s caller
   STILL unnamed — bump the TFDC budget behind a dfx switch and boot once).
4. The false-heal vs kpreempt dependency: real guards held >2s exist;
   bounding them (cond_resched at lock-free boundaries of fork copy /
   exit unmap / rmap batch loops) is the prerequisite for arming kpreempt.
  (1096/1096); branch pushed through fa31bfb3.
- Successor entry points, in order: (1) the stranded-wake/preempt-count hunt
  (WAKE-STRANDED 1-2.6s at pc=1-2 + PARKED-LEAK pc=1 prints — leak suspects,
  needs a preempt-count origin shadow like the leakorigin hook); (2) the
  userId-0 launcher hole (see above) — image-side or perf-side; (3) once a
  boot keeps foundation alive through activation, the launcher should spawn
  (systemui already does) and the GL/scanout chain is proven.

## FOURTH-SUCCESSOR SESSION (2026-10-31 17:00-18:xx, branch oh-launcher @ 49df47b8+)
Base c23045fe + this session. THE WALL WAS MOSTLY A MISSING /data.

### THE BIG FINDING — every boot from lb16 through lb26 ran WITHOUT /data
1. a6b01fd9 (main, cr2 csum review, 11:49) tightened superblock_csum_ok
   from a 16-bit to the FULL 32-bit compare (correct per e2fsprogs).
2. run/userdata-fresh.img (cooked 08:26 by the PRE-fix kernel whose sb
   sealer wrote only the low u16, snapshot=on ever since so it looked
   pristine) fails the new check: stored le32 0x4e8183c6 vs computed
   0x5b8283c6 — low 16 bits match (why lb1-lb15 mounted it), high 16
   are the stale old-seal. Verified on-host with a crc32c recompute.
3. Effect: EVERY boot lb16..lb26 (the whole third-successor campaign,
   INCLUDING lb20/lb21) logged `mount[10] vdf -> /data: -5` x3 +
   "ext4: superblock checksum invalid". /data never mounted; the
   distributed cluster (softbus LNN store, distributeddata RDBs,
   accountmgr osaccount DB) ran on an unmounted read-only empty /data.
   The "softbus starvation wall" was largely SOFTBUS HAVING NO DATABASE
   DIRECTORY — its LNN init can never finish, so every device_manager
   LNN query burned its 5.24s timeout (x102 on lb20).
4. FIX (this session): run/userdata-fresh2.img = fresh mke2fs 16G ext4
   (T3 recipe; sb csum passes the full-32-bit check bit-for-bit) +
   USERDATA env in lb-boot.py. NEVER boot the old userdata-fresh.img
   again on a post-a6b01fd9 kernel — recook instead.

### lb27 PROOF (release kernel + fresh /data; the healthiest boot ever)
- /data mounts rc=0 first try; no dsoftbus mkdirfails.
- SetBundleQuota for /data/app/el2/100/base/com.ohos.launcher @138s;
  StorageDaemon StartUser(100) COMPLETES in 64ms @237s;
  startup.appspawn.lockstatus_100 SET; USER_ADDED + USER_UNLOCKED fire;
  bootevent.bms.main.bundles.ready @237.9s; wms.fullscreen.ready TRUE;
  ZERO device_manager code-3 timeout chains (lb20: 102); 8521220 GONE
  (the userId-0 hole closed itself once /data existed); SLOW-TXN total
  18/boot. The account ACTIVATION completes.
- NOT yet landed: the AMS launcher START (no AMSI1640, no
  com.ohos.launcher process, screen still the frozen logo md5
  06b933b4 through 911s). bootevent.account.ready is watched (pid 1
  msgId 26) but never SET — find what sets it on the reference kernel.
- lb28 (dfx=ams46): StartUser oneway LATCHED (node=12710 pid=876
  foundation @283s) — this boot's SA-load phase ran the slow samgr
  code-7 family again (29-75s exec), unlock not reached by 663s.
  Boot-to-boot variance in the huks/account keygen path decides it.

### Scheduler work landed (commits 49df47b8 + 0fa06de4 + bb82cac2)
1. CLAIM-SWEEP (fair.rs claim_sweep + scheduler_tick hook): the pick-side
   stale-claim heal was ORDER-DEPENDENT (walks stop at the first allowed
   leftmost; a mid-tree victim with frozen vruntime is never examined —
   lb20's pid 724 lost 579s between resets). Once per 100 jiffies one
   CPU sweeps the whole timeline under the GRQ with the pick-side
   adjudication: {on_cpu=false, claim!=-1} owned by no slot/switch-out
   -> reset claim; linked+on_cpu orphaned-mark shape (ownerless +
   50-jiffy exec-frozen orphan-watch confirm) -> full-qword release.
   Census line (dfx=wwcensus) names pids of both classes. lb22-lb25
   validated: zero false heals (every oncpu sighting was owner-backed);
   no cluster thread was ever stale-claimed on those boots — the claim
   stranding class was NOT the softbus killer (the missing /data was).
2. tfdcensus (dfx=tfdcensus): windowed TFDC tap (first 16 calls per 5s
   window). Storm callers named: companion_device (42), useriam (20),
   time_service (10), OS_IPC workers, usb_host. Entry point 4 CLOSED
   (naming); the storm is fd-churn COST, not a timerfd leak — timeouts
   expire on schedule.
3. TXN-HANG lens (lb24, dfx=txnhunt+poolcensus): binder delivery is
   CLEAN (zero queued todo across the whole boot; no stuck inbound
   sync txn into softbus). The 30-55s waits are userspace-INTERNAL
   futex waits inside executing handlers (accountmgr tid 976 sys=202
   FUTEX_WAIT 31s; foundation main thread 55s; nested BWR reads 50s) —
   exec-class compute/condition waits, NOT kernel scheduling strands.
4. leakhunt (lb25, dfx=leakhunt): the phantom base-bit (+1) acquire
   sites on the carriers (pids 318/512/537/601): spinlock.rs:292,
   mm/slab.rs:628, mm/small_alloc.rs:283, mm/buddy_allocator.rs:857,
   sync/rwlock.rs — the ALLOCATOR-lock family. Producer still at large
   (r83hunt territory; the wild-write/replayed-acquire split unknown).
5. Release kernel: target/x86_64-unknown-none/release/bzImage
   (build-release target) boots the image fine — 6x wake throughput,
   appfwk.ready 132-169s (debug: 217-328s). Same wall without /data
   (lb26), clean with it (lb27) — proves the wall was not compute.
   NOTE: release strand tail exists (stranded=70, strand_max=24s class
   on lb27) — worth one syscost look before relying on release boots.

### FIFTH-SUCCESSOR ENTRY POINTS (in order)
1. THE LAUNCHER START: boot release kernel + USERDATA=run/userdata-
   fresh2.img + dfx=ams46,txnhunt; watch AMSI1640/SwitchToUser/appspawn
   spawn of com.ohos.launcher after lockstatus_100. Variance: lb27-class
   boots unlock at ~237s; lb28-class get stuck in the samgr code-7 30s
   exec family — if an ams46 boot shows the StartUser oneway delivered
   but not executed (the walled-draw shape the arsenal was built for),
   that print names the drop site.
2. bootevent.account.ready is never SET (only watched): compare with the
   reference kernel (14.4s boot) which service/param job sets it; likely
   an init condition job that depends on a service which is slow/dead.
3. The re-leak producer (unchanged): allocator-lock acquire sites above;
   r83hunt + leakhunt together on one boot.
4. Perf-gate note for the coordinator: this session's sched changes
   (CLAIM-SWEEP = once-per-second GRQ-held timeline walk) are Tier-2
   guarded-domain (sched/); needs syscost + pbench gate before any
   merge to main.

### Boot inventory this session
lb22 (dfx=wwcensus,schedtrace, debug): claim sweep live, 17 pick-side
  resets, cluster threads never stale-claimed; timeout loop present.
lb23 (+full sweep): softbus pid 731 REPLIES code-8 in 2.6s (not silent);
  TFDC windowed census first data; timeout loop still present (no /data).
lb24 (poolcensus+txnhunt): TXN-HANG futex-wait picture; no queued todo.
lb25 (leakhunt+futextrace): phantom acquire sites named; no key-mismatch
  prints (no lost futex wakes observed).
lb26 (RELEASE kernel, no /data): same wall → wall is not compute.
lb27 (release + fresh /data): MILESTONE boot — see above.
lb28 (release + fresh /data + ams46): slow SA-load variant, evidence in
  run/oh-lb28-serial.log.
