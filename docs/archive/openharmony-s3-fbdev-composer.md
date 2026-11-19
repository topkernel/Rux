# Spike S3: fbdev HDI composer — is Route A viable?

Date: 2026-10-07
Source trees inspected (OH master line, matching the oh-robot 6.1 dev / API 23 target):

- `drivers_peripheral` display HDI: `/home/william/workspace/oh-src/drivers_peripheral_x/drivers_peripheral-master/display/`
- render service: `/home/william/workspace/oh-src/graphic_x/graphic_graphic_2d-master/`
- EDU x86_64_virt board/vendor: gitee `open-harmony-edu-dist/{vendor_edu,device_soc_edu,device_board_edu}` (branch `OpenHarmony-5.0.2-Release`; structure matches the oh-robot 6.1 tree per the port plan)

## Verdict

**Route A is viable, and cheaper than the plan estimated.** The composer
backend is pluggable by design at *two* `dlopen` seams, neither of which
touches DRM inside the reusable HDI service layer:

1. `composer_host` (an HDF devhost process) loads
   `libdisplay_composer_driver_1.0.z.so` per hcs config; that driver builds
   `DisplayComposerService`, which `dlopen`s the **vendor VDI library**
   `libdisplay_composer_vdi_impl.z.so` and resolves ~60 **flat C symbols**
   (`dlsym("Commit")`, `dlsym("CreateLayer")`, ...). A vendor drop-in never
   links libdrm unless it wants to.
2. The buffer (gralloc) side has the same seam:
   `libdisplay_buffer_vdi_impl.z.so` exporting `CreateDisplayBufferVdi()`.

OH even ships an **in-tree fbdev backend precedent**
(`display/hal/default_standard/src/display_device/fbdev/`, plus a software
vsync thread and CPU layer composition) — it is the older HAL layer rather
than a VDI, but the mode/capability/vsync/composition logic is directly
reusable as a template.

One significant caveat reshapes the plan: the *rendering* side, not the
composer side, is what pulls DRM in. On the reference image, render_service
and bootanimation both use EGL/llvmpipe (`kms_swrast` over
`/dev/dri/card0`). Route A therefore only reaches the Phase-3 gate if the
OH image is rebuilt with **`graphic_2d_feature_ace_enable_gpu = false`**
(CPU/Skia raster path, an in-tree supported configuration — bootanimation's
EGL code is `#ifdef ACE_ENABLE_GL`-guarded). With GPU kept on, Route A does
not avoid DRM, because mesa has no DRM-less EGL platform on this target.

Estimated effort (agent-mode): ~1.5–2.5k lines of userspace C++ in two `.so`
files, no kernel DRM, no kernel changes beyond what Phase 2 already needs
(binder fd passing, memfd wiring). The 1–2k estimate in the port plan holds;
the extra few hundred lines are the gralloc VDI, which the plan did not
itemize.

## 1. How the composer HDI actually loads (the pluggability mechanism)

Boot chain on the reference image:

1. `hdf_devmgr` reads the uhdf `device_info.hcs` (in the image at
   `/vendor/etc/hdf_config/...`; source in the vendor tree, see §5) and
   spawns one `hdf_devhost` process per `host` node.
2. The display entry is (EDU virt hcs, confirmed from gitee):

   ```
   display_composer :: host {
       hostName = "composer_host";
       priority = 40;
       processPriority = -8;
       threadPriority = 1;
       caps = ["SYS_NICE"];
       uid = ["composer_host"];
       gid = ["composer_host", "graphics", "vendor_mpp_driver"];
       composer_device :: device {
           device0 :: deviceNode {
               policy = 2;
               priority = 160;
               moduleName = "libdisplay_composer_driver_1.0.z.so";
               serviceName = "display_composer_service";
           }
       }
   }
   allocator :: host {
       hostName = "allocator_host";
       ...  moduleName = "liballocator_driver_1.0.z.so";
   }
   ```

3. `composer_host` dlopens `libdisplay_composer_driver_1.0.z.so` (built from
   `display/composer/hdi_service/src/display_composer_driver.cpp`; the
   `HDF_INIT` entry has `moduleName = "display_composer"`). Its `Bind()`
   calls `IDisplayComposer::V1_3::Get(true)` →
   `DisplayComposerImplGetInstance()` → `new DisplayComposerService()`.
4. `DisplayComposerService`'s constructor (`display_composer_service.cpp`):
   - `LoadVdiSo()`: `dlopen("libdisplay_composer_vdi_impl.z.so", RTLD_LAZY)`.
     With the GN arg `drivers_peripheral_display_vdi_default = true` it first
     tries `libdisplay_composer_vdi_impl_default.z.so` (the in-tree
     `composer/vdi_base` DRM implementation) and falls back to the plain
     name. Either way the *vendor* name is the fallback — a board that ships
     its own `libdisplay_composer_vdi_impl.z.so` needs no hcs or build
     change in drivers_peripheral.
   - `LoadVdiAdapter()` dlsyms ~60 flat C symbols (see §2) and **requires
     the 40 v1_0 symbols to be non-null** or `Bind()` fails and composer_host
     dies.
   - Sets `bootevent.composer_host.ready=true` when up.

**Plugin point for Route A: ship our own `libdisplay_composer_vdi_impl.z.so`
+ `libdisplay_buffer_vdi_impl.z.so` in the image (same paths the EDU board
uses, chipset/vendor lib dirs) and rebuild nothing else in the HDI stack.**
Everything above the VDI — IDL stubs, SMQ command responser, buffer cache
manager, hidumper — is reused verbatim from OH source.

Note on symbol convention: the 6.1-dev hdi_service loads flat C symbols
(`extern "C"` functions named `Commit`, `CreateLayer`, ...). Older branches
(and `hal/default_standard`, and the `CreateComposerVdi` factory still
declared in `idisplay_composer_vdi.h`) use a C++ factory. Target the flat
convention, since oh-robot tracks 6.1 dev.

## 2. Interface surface: what must be implemented vs stubbed

### Composer VDI (`IDisplayComposerVdi`, flat symbols)

All 40 v1_0 symbols must exist (non-null); unsupported ones may simply
return `HDF_ERR_NOT_SUPPORT` — the in-tree default VDI does exactly that for
10 of them. Practical split for an fbdev backend:

**Real implementation (~15 functions):**

| Function | fbdev implementation |
|---|---|
| `RegHotPlugCallback(cb, data)` | store cb; call `cb(0, true, data)` immediately for the one connected display (this is what triggers RS screen creation; see `HdiSession::RegHotPlugCallback` pattern) |
| `GetDisplayCapability` | name="rux-fb", `type=DISP_INTF_PANEL`, phyW/H from `FBIOGET_VSCREENINFO`, supportLayers≥1, propertyCount=0 |
| `GetDisplaySupportedModes` / `GetDisplayMode` / `SetDisplayMode` | one 60Hz mode from var_screeninfo; id 0 |
| `GetDisplayPowerStatus` / `SetDisplayPowerStatus` | track + echo state (see `FbDisplay::SetDisplayPowerStatus` template) |
| `SetDisplayVsyncEnabled` | arm/disarm soft vsync thread |
| `RegDisplayVBlankCallback` | store cb |
| `Commit(devId, &fence)` | blit current client/layer buffer(s) to fbdev mmap (format convert if needed), `FBIOPAN_DISPLAY`, return `fence = -1` |
| `CreateLayer` / `DestroyLayer` | allocate layer-id slot, record LayerInfo |
| `PrepareDisplayLayers(devId, &needFlushFb)` | `needFlushFb = true`, mark all layers `COMPOSITION_CLIENT` |
| `GetDisplayCompChange` | return layer list with `COMPOSITION_CLIENT` |
| `SetDisplayClientBuffer` | store BufferHandle (+fence fd, can be -1) for next Commit |
| `SetLayerBuffer` | store layer buffer (client-comp: RS keeps FB in the client buffer; layer buffers still arrive in divided-render mode — support both) |
| `SetLayerRegion` / `SetLayerCrop` / `SetLayerZorder` / `SetLayerCompositionType` / `SetLayerBlendType` | record geometry (needed if we compose layer list rather than just the client buffer) |

**Accept-and-ignore or NOT_SUPPORT (~25 functions):** `SetDisplayClientCrop`,
`SetDisplayClientDamage` (store damage, optional optimization),
`Get/SetDisplayBacklight`, `GetDisplayProperty`, `GetDisplayReleaseFence`
(return `-1` fences per layer — legal, `SyncFence(-1)` is the always-signaled
fence), `Create/Destroy/SetVirtualDisplay`, `SetDisplayProperty`, and the
per-layer setters not listed above (`SetLayerAlpha`, `SetLayerPreMulti`,
`SetLayerTransformMode`, `SetLayerDirtyRegion`, `SetLayerVisibleRegion`,
`SetLayerMaskInfo`, `SetLayerColor`...). The default VDI already returns
`HDF_ERR_NOT_SUPPORT` for `SetLayerVisibleRegion`, `SetLayerMaskInfo`,
`SetLayerColor`, `SetDisplayClientCrop`, `SetDisplayClientDamage`,
`CreateVirtualDisplay`, `DestroyVirtualDisplay`, `SetVirtualDisplayBuffer`,
`SetDisplayProperty`, `GetDisplayProperty` — proof these paths tolerate it.

**Optional (dlsym'd but not null-checked; omit entirely):** all v1_1–v1_3
entries — `RegSeamlessChangeCallback`, `GetDisplaySupportedModesExt`,
`SetDisplayModeAsync`, `GetDisplayVBlankPeriod`,
`Set/GetSupportedLayerPerFrameParameterKey`, `SetDisplayOverlayResolution`,
`RegRefreshCallback`, `GetDisplaySupportedColorGamuts`, `GetHDRCapabilityInfos`,
`RegDisplayVBlankIdleCallback`, `SetDisplayConstraint`, hardware cursor
functions, `FastPresent`, `SetDisplayActiveRegion`, `Clear*Buffer`,
`SetDisplayPerFrameParameter`, `GetDisplayIdentificationData`,
`RegHwcEventCallback`, `GetSupportLayerType`, tunnel-layer functions,
`GetDumpInfo`/`UpdateConfig` (hidumper).

Optional-but-nice: `GetDisplayProperty(DISPLAY_PROPERTY_ID_SKIP_VALIDATE)`
returning 1 lets RS take the skip-validate fast path (fewer IPCs per frame);
RS already tolerates its failure (result ignored in
`HdiDeviceImpl::GetScreenCapability`).

### What render_service actually calls (evidence)

`rosen/modules/composer/hdi_backend/src/hdi_device_impl.cpp` wraps
`IDisplayComposerInterface` V1_3; the per-frame path
(`RSHardwareThread::CommitAndReleaseLayers` → `HdiBackend::Repaint` →
`HdiOutput`) uses:

- init/hotplug: `RegHotPlugCallback`, `RegDisplayVBlankCallback`,
  `GetDisplayCapability`, `GetDisplaySupportedModes`, `GetDisplayMode`,
  `SetDisplayPowerStatus`, `RegRefreshCallback`, `RegHwcEventCallback`
- layer setup: `CreateLayer`, `DestroyLayer`, `SetLayerAlpha/Size/
  TransformMode/VisibleRegion/DirtyRegion/Buffer/CompositionType/BlendType/
  Crop/Zorder/PreMulti/Color`, `SetClientBufferCacheCount`
- frame: `SetDisplayClientDamage`, `SetDisplayClientBuffer`,
  `Commit`/`CommitAndGetReleaseFence`, `GetDisplayCompChange`,
  `SetDisplayVsyncEnabled`
- power/backlight: `Get/SetDisplayPowerStatus`, `Get/SetDisplayBacklight`

Everything is satisfied by the v1_0 set plus returning -1 fences.

## 3. RS hard dependencies vs a DRM-less kernel

- **vsync**: delivered by the in-process VBlank callback
  (`VBlankCallback(sequence, ns)`) — the default DRM VDI gets it from
  `drmWaitVBlank` events, but an fbdev VDI just needs a ~16.7 ms timer
  thread passing `clock_gettime(CLOCK_MONOTONIC)` ns (nonzero — RS drops
  `ns == 0`). OH's own `SorftVsync` (`hal/default_standard/.../sorft_vsync.cpp`)
  is exactly this. RS's `VSyncSampler` consumes timestamps and drives the
  frame loop; apps/bootanimation get vsync via RS (`VSyncReceiver`), not
  from the composer.
- **fences**: acquire fences arrive as plain ints (usually -1 for CPU
  rendering); Commit/GetDisplayReleaseFence may return -1
  (`HdiDeviceImpl::Commit` explicitly wraps `fenceFd < 0` into
  `SyncFence(-1)`). No sync_file/DMA-BUF fence kernel support needed.
- **buffer/fd passing**: `BufferHandle` carries an `fd` (dma-buf on the
  reference; for us a memfd) and crosses processes via `HdifdParcelable`
  over binder. This requires **binder fd arrays (BINDER_TYPE_FDA)** — part
  of the Phase-2 binder UAPI, a prerequisite of Route A regardless.
- **hotplug**: the VDI synthesizes connect at callback registration (see
  above); the default VDI additionally watches `NETLINK_KOBJECT_UEVENT`
  (`hdi_netlink_monitor.cpp`, `nl_groups=1`) for drm hotplug — optional for
  us, and Rux already has uevent netlink if we ever want real hotplug.
- **EGL/GLES — the real DRM dependency**: RS is built with
  `RS_ENABLE_GL` (`graphic_2d_feature_ace_enable_gpu = true` default) and
  initializes an EGL context (llvmpipe via mesa kms_swrast → `/dev/dri`
  card0/renderD128). bootanimation does the same under
  `#ifdef ACE_ENABLE_GL` (`frameworks/bootanimation/src/
  boot_animation_operation.cpp:211-230`). With
  `graphic_2d_feature_ace_enable_gpu = false`: RS uses the Skia raster
  path (`RSBaseRenderEngine::Init()` body compiles empty without
  `RS_ENABLE_GL`; `RenderContextOhosRaster` exists in
  `render_service_base`), bootanimation draws via
  `RSSurfaceFrame->GetCanvas()` (CPU). **Route A therefore requires an OH
  image rebuild with GPU disabled** — no kernel DRM, no mesa. Verify early
  that ArkUI (ace_engine) raster mode is acceptable on standard system for
  the launcher gate (biggest non-kernel risk of Route A; screenless-class
  products do run this configuration).

## 4. Buffer management: gralloc VDI is also pluggable, also needs a backend

Yes — `IGralloc`-equivalent (`IDisplayBufferVdi`,
`display/buffer/hdi_service/include/idisplay_buffer_vdi.h`) needs a custom
backend too. Loaded per-process (mapper is passthrough-indirect):
`mapper_service.cpp` dlopens `libdisplay_buffer_vdi_impl.z.so`, resolves
`CreateDisplayBufferVdi`/`DestroyDisplayBufferVdi` factory symbols; the
`allocator_host` process does the same for allocation.

Interface core is 6 methods — `AllocMem`, `FreeMem`, `Mmap`, `Unmap`,
`FlushCache`, `InvalidateCache` (+ ~8 optional stubs that already default
to no-op/NOT_SUPPORT in the header). The default backend is GBM over DRM
(`buffer/vdi_base/src/display_gralloc_gbm.cpp`, hi_gbm, dumb buffers,
`drmPrimeHandleToFD`). An fbdev-compatible backend is trivial:
`memfd_create` + `ftruncate` + `mmap`; `handle.fd = memfd`; caches are
no-ops on uncached/normal memory. Rux's memfd is implemented-but-unwired
(`kernel/src/fs/memfd.rs:95`) — wiring the syscall is a Phase-2 line item
that Route A depends on.

In-tree fallbacks exist but don't fit Rux as-is:
`hal/default_standard/.../framebuffer_allocator.cpp` allocates *from* the
framebuffer (grows `yres_virtual`, needs `smem_start` + a vendor
`FbGetDmaBuffer` export) and `dmabufferheap_allocator.cpp` needs
`/dev/dma_heap/system` (Linux DMA-BUF heaps). A ~250-line memfd allocator is
simpler than adopting either.

## 5. x86_64_virt (EDU) actual configuration

- `display/display_config.gni` (in-tree): only three flags —
  `drivers_peripheral_display_community`, `hicollie`, and
  `drivers_peripheral_display_vdi_default` (adds
  `COMPOSER_VDI_DEFAULT_LIBRARY_ENABLE`/`BUFFER_VDI_DEFAULT_LIBRARY_ENABLE`,
  making hdi_service prefer `..._default.z.so`, and makes
  `display/composer/BUILD.gn` build `composer/vdi_base` as that default
  library).
- The EDU x86_64_virt product does **not** use `vdi_default`; it ships its
  own vendor VDIs from `device_soc_edu` (`virt/hardware/display/BUILD.gn`,
  branch OpenHarmony-5.0.2-Release):
  - `libdisplay_composer_vdi_impl.z.so` from
    `src/display_device/` — the same drm_connector/crtc/device/display/
    encoder/plane/vsync_worker/hdi_* source set as
    `drivers_peripheral/display/composer/vdi_base`, linked against
    `third_party/libdrm` (factory-style `CreateComposerVdi` export on that
    branch).
  - `libdisplay_buffer_vdi_impl.z.so` from `src/display_gralloc/` —
    GBM/hi_gbm gralloc (`-DGRALLOC_GBM_SUPPORT`), libdrm dumb buffers.
  - `display_gfx.z.so` (2D blit for CPU composition).
  - Prebuilt mesa (`virt/hardware/gpu/` — libglapi etc., i.e. llvmpipe).
- So the reference stack is uniformly DRM: virtio-gpu kernel DRM driver →
  `/dev/dri/card0` + render node → libdrm → composer VDI + GBM gralloc →
  mesa llvmpipe for EGL. `vendor/edu/virt/device.gni` wires
  `display_device_hal = "soc/edu/virt/hardware"`;
  `device/board/edu/virt/device.gni` declares `is_support_graphic = true`,
  `is_support_boot_animation = false` (on that branch; the oh-robot 6.1
  image does run bootanimation).
- hcs: as quoted in §1 (from `vendor_edu/virt/hdf_config/uhdf/device_info.hcs`).

## 6. Route A workload list

Userspace (new code, shipped in the OH image):

1. **`libdisplay_composer_vdi_impl.z.so`** (~1–1.5k LoC C++): flat v1_0
   symbol set; single-display state machine; fbdev open/mmap/pan; soft
   vsync thread (timerfd/clock_nanosleep); Commit-time blit with format
   conversion (see below); layer bookkeeping. Templates:
   `composer/vdi_base/src/display_composer_vdi_impl.cpp` (wrapper shape),
   `hal/default_standard/src/display_device/fbdev/fb_display.cpp` (modes,
   capability, power, vsync-enable),
   `.../vsync/sorft_vsync.cpp` (soft vsync),
   `.../fb_composition.cpp` + `hdi_gfx_composition.cpp` (composition; note
   its `FbFresh` path calls a vendor adapter ioctl we replace with plain
   mmap+memcpy+FBIOPAN).
2. **`libdisplay_buffer_vdi_impl.z.so`** (~250 LoC): `CreateDisplayBufferVdi`
   factory; memfd allocation; mmap/unmap; cache no-ops; minimal
   `IsSupportedAlloc`.
3. Image/config: place both `.so` in the chipset/vendor partition where the
   EDU ones live; set `graphic_2d_feature_ace_enable_gpu = false` (and keep
   `graphic_2d_feature_rs_enable_uni_render` as the product default — both
   render modes work with an all-client composer; uni-render means only the
   client buffer needs blitting).

Kernel (Rux): **no new display code**. Existing fbdev
(`kernel/src/drivers/gpu/fbdev.rs`: FBIOGET_VSCREENINFO/FSCREENINFO,
FBIOPAN_DISPLAY, mmap, 32bpp) suffices. Dependencies already in the plan:
binder with fd passing (Phase 2), memfd wiring (Phase 2), timerfd/clock
(present).

Pixel format note: Rux fb advertises 32bpp with bitfields
R@[15:8] G@[23:16] B@[31:24] (byte order X,R,G,B per pixel;
`fbdev.rs:create_var_screeninfo`). OH gralloc RGBA_8888 is R,G,B,A bytes —
Commit does a per-pixel byte shuffle (cheap at 360x720 ≈ 0.26 MP; on x86_64
with SIMD this is negligible). If it ever matters, the gralloc VDI can
allocate in the fb-native layout and advertise the matching OH pixel
format, making Commit a straight memcpy; stride handling via
`fix_screeninfo.line_length`.

## 7. Minimal verification path (no full OH boot needed first)

1. **VDI unit probe (host or Rux)**: a small program that dlopens
   `libdisplay_composer_vdi_impl.z.so`, resolves the 40 symbols, calls
   `RegHotPlugCallback` (expect immediate connect callback),
   `GetDisplayCapability`/`GetDisplaySupportedModes`,
   `CreateLayer`, writes a test pattern into a memfd BufferHandle via
   `SetDisplayClientBuffer`, then `Commit`. Verify the pattern on
   `/dev/fb0` with the existing screenshot/pixel-check tooling from
   `test/ubuntu-gui/verify.py`. This validates the whole fbdev path on
   riscv64 main with zero binder/HDF involvement.
2. **composer_host smoke**: after Phase 2 binder, boot the OH image up to
   `bootevent.composer_host.ready` and confirm the VDI so loads (hilog
   "composer load vendor vdi library") and `render_service` gets past
   `RegHotPlugCallback` (screen-create path).
3. **Gate check**: bootanimation frame visible + `launcher.ready` +
   pixel-verification script.

## 8. Route B (for comparison): the DRM surface a kernel would need

If Route A were rejected (e.g. ArkUI-raster proves unusable), Route B's
kernel-side ioctl set, derived from what the default VDI stack actually
issues (libdrm calls enumerated across `composer/vdi_base/src/*.cpp`,
`buffer/vdi_base/src/{display_gralloc_gbm,hi_gbm}.cpp`) plus mesa
kms_swrast/llvmpipe probes:

- Core: `DRM_IOCTL_VERSION`, `DRM_IOCTL_GET_CAP` (client caps: UNIVERSAL_PLANES,
  ATOMIC, and whatever llvmpipe asks), `DRM_IOCTL_PRIME_FD_TO_HANDLE` /
  `HANDLE_TO_FD`
- Mode objects: `MODE_GETRESOURCES`, `MODE_GETCONNECTOR`, `MODE_GETENCODER`,
  `MODE_GETCRTC`, `MODE_GETPLANE_RESOURCES`, `MODE_GETPLANE`,
  `MODE_OBJ_GETPROPERTIES`, `MODE_GETPROPERTY` (properties for connector/
  crtc/plane objects)
- Framebuffer/composition: `MODE_ADDFB2`/`MODE_RMFB`,
  `MODE_CREATEPROPERTBLOB`/`DESTROYPROPERTBLOB`, `MODE_ATOMIC_COMMIT`
  (pageflip + modeset paths)
- Buffers: `MODE_CREATE_DUMB`/`MODE_MAP_DUMB`/`MODE_DESTROY_DUMB` (gralloc
  and llvmpipe)
- Events: vblank/pageflip events on the fd (`drmHandleEvent`) +
  `MODE_WAIT_VBLANK`; render node (`/dev/dri/renderD128`) separation for
  mesa; plus the uevent netlink emission drm class devices expect.

That is the "4–8k lines + long tail" estimate from the plan — Route B buys
GPU rendering and zero OH rebuild, at much higher kernel cost and risk
(mesa's ioctl probing is exactly the kind of long tail the plan flags in
R3). Route A and B are not mutually exclusive: A first for the gate, B
later if GL becomes a requirement.

## 9. Risks / open items for Route A

| # | Risk | Mitigation |
|---|---|---|
| A1 | ArkUI/ace_engine raster-only mode on standard system unproven (all reference images run GPU on) | Earliest possible check in Phase 3 entry: rebuild graphic_2d with GPU off on x86_64 and confirm launcher UI renders; this is the go/no-go for Route A |
| A2 | CPU render+compose throughput (360x720@60) | ~0.26 MP frame; Skia raster + one blit is well within one x86 core; measure in the VDI probe |
| A3 | Flat-symbol vs factory VDI convention drift if oh-robot re-syncs to a newer OH | oh-robot tree is pinned per plan R5; if re-synced, re-check `LoadVdiFuncPart1/2/3` symbol names |
| A4 | `FBIOPUT_VSCREENINFO` echo semantics (fbdev HAL fallback allocators rely on growing virtual Y) | Not needed — our gralloc is memfd-based; fbdev stays single-buffered with pan |
| A5 | binder fd-array passing correctness (HdifdParcelable) surfaces late | Already a Phase-2 binder acceptance test item (S1); add an fd-roundtrip case |

## 10. Answer to the plan's S3 question

> "Is Route A viable? Inspect oh-robot display HDI interface definitions and
> composer_host; check whether the composer backend is pluggable without DRM"

Yes: the composer backend is a `dlopen`ed vendor library behind a flat C
symbol table, with all DRM usage confined to replaceable code; the buffer
backend has the same seam; fences can be -1 everywhere; vsync can be a
software timer; and OH ships fbdev + soft-vsync + CPU-composition reference
code in-tree. The condition is rebuilding the OH image with
`graphic_2d_feature_ace_enable_gpu = false` so render_service and
bootanimation stay off EGL/llvmpipe/DRM. Route A stands, with risk A1
(ArkUI raster on standard system) as the one thing to falsify first.
