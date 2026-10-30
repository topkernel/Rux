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

### T1 PARTIALLY NAMED — render_service SIGABRT
- Repro on every boot: abort ~2 hilog lines after `RSEglImageManager::MapEglImageFromSurfaceBuffer` + `RenderFrame: RenderFrame: Canvas` (first composition frame, 720x1280). musl abort prints nothing (stderr=/dev/null).
- Kernel tap added: signal.rs abort_reason_dump() fires on the FIRST sig-6 per tgid (budget 8/boot), dumps rip/rsp + all exec VMAs + 64 stack words classified lib+offset. The observed raise is the FAULTLOGGERD crash-handler RE-raise (handler frames on the stack; libfaultloggerd mapped). Also seen right there: `pagefault: Permission denied at 0x7ff00067c000, rip=0x3fbf08ad30 (musl)` — a write to a read-only mapping near the abort; NOT yet attributed (param-workspace write? CFI shadow?). NEXT STEP: decode that fault + the ABORT-STK chain values (the serial line tears from UART interleave — the [18]=0x7ff08c0ada9a-class values are heap/so pointers, not code returns; the real chain is likely BELOW the handler frames — consider catching the ORIGINAL signal, or reading faultloggerd's dump from /data/log/faultlog/temp (the fresh lineage persists it — host-side reading currently blocked, see known-gap below).
- The mali symlink (coordinator's Mesa finding) is IN (private images, see below) and `dlopen /vendor/lib64/chipsetsdk/libGLES_mali.so` now SUCCEEDS (1126ms init) — the abort is NOT the missing GLES driver.

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
1. render_service abort reason FULLY named (fault attribution at 0x7ff00067c000 or the handler-frame chain). 2. Fixed → render_service survives its first life AND registers (SAMGR). 3. DONE (fresh lineage + csum write path + token OK). 4. With the sibling: bootanimation/launcher pixels.
