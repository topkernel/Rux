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
