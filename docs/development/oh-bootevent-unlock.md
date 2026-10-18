# OH display: bootevent unlock (SO_PEERSEC) and the pixel frontier

Date: 2026-10-18. Lane: `feature/oh-display` @ c6b7c79a (on top of main
3d7f2b7d: biglock fairness + printk bounded + prior fixes).

## Summary of this window

1. **Fixed (kernel, c6b7c79a)**: `getsockopt(SO_PEERSEC)` on AF_UNIX —
   OH's param service needs it for `getpeercon(3)`; its absence made init
   FORBID every `bootevent.*` set, which wedged bootanimation in
   `WaitRenderServiceInit()` forever (b151: 225k serial lines, black
   screen, healthy park). After the fix every bootevent sets cleanly and
   the boot proceeds through the real graphics setup.
2. **Proven (kernel DRM)**: `test/drm_probe_boot.py` on this kernel —
   **PASS 31/31**, screendump of the named virtio-gpu console
   (720x1280, 65536 colors, 100% non-black): the whole
   composer-init + gralloc dumb/prime + ADDFB2 + MODE_ATOMIC path puts a
   test pattern on screen with no OH userspace involved
   (`run/oh-drmprobe-shot1.ppm`, entry in `run/oh-shotmon-MILESTONE`).
   Combined with the anim-block lane's source cross-check, the kernel DRM
   side is armed and pixel-correct; the remaining gate is userspace.
3. **New frontier (userspace-side blockers, kernel-caused)**, in order:

   a. **SA 3002 `.so` load takes 802 s**: `SAMGR: SA:3002 OpenSo
      802166ms` (b153). media_service (pid 564) sits in
      UNINTERRUPTIBLE sleep for the whole window; ~537 file-backed page
      faults ≈ 1.5 s per fault. Every large-library load on the OH image
      rides the same slow file-read path (launcher's ArkUI will hit it
      too). This is now the single common gate for both remaining pixel
      sources (bootanimation video, launcher UI).
   b. **hilogd SIGSEGVs early** (b154 line ~4774: rip in a file-backed
      lib, `Segfault at 0x18`, rax=0, right after an EFAULT on syscall
      240/mq_open-shaped args). With hilogd dead there is no hilog
      anywhere (serial, disk, or otherwise), which blinded every
      LOGI/LOGE diagnosis of the graphics failure modes this window.
   c. bootanimation reaches the real render setup: display node commit
      (COMMIT_TRANSACTION, oneway 480 B) + surface-node create
      (CREATE_NODE_AND_SURFACE "BootAnimationNodeExtra" → surface id 5
      reply) all work over binder; then it takes the VIDEO path
      (image ships `bootvideo.mp4`) and polls `CheckSystemAbility(3002)`
      until its duration timer stops the runner — no frames, no EGL
      context, allocator_host never wakes, composer VDI never lazy-opens
      card0 (#2 never appears).

## The SO_PEERSEC fix (what and why)

Call chain (oh-robot sources):

```
render_service: SystemParameter("bootevent.renderservice.ready", true)
  → init param service: CheckParamPermission_ → SelinuxCheckParamPermission
    → libparaperm_checker.z.so SetParamCheck (param_checker.c:90)
      → getpeercon(sockFd)                    ← FAILS EINVAL on Rux
        → getsockopt(SOL_SOCKET, SO_PEERSEC)  ← not implemented; also the
                                                optlen cap (128) rejected
                                                libselinux's 256-byte buffer
  → "Forbid to set parameter bootevent.*"
bootanimation: WaitRenderServiceInit() (boot_animation_controller.cpp:65)
  polls GetBoolParameter("bootevent.renderservice.ready") forever
```

On the Linux reference the identical chain succeeds (getpeercon returns
the peer's label; no bootevent Forbids; launcher.ready at 10.1 s).

Rux answer (kernel/src/syscall/network.rs):

- AF_UNIX `getsockopt(SO_PEERSEC)` returns a synthesized
  `"u:r:<peer comm>:s0"` string (NUL-terminated, length reported
  including the NUL, ERANGE + needed-size on a too-small buffer,
  ENOTCONN without a peer — Linux stream semantics).
- The getsockopt optlen cap rises 128 → 256 (libselinux's
  INITCONTEXTLEN+1).

Why a fake context is sufficient: the image's libselinux
(oh-robot third_party/selinux) `sidtab_context_to_sid` is pure string
keying (no context parsing), and `selinux_check_access` hits
`string_to_security_class("parameter_service") == 0` → unknown class →
`security_deny_unknown() == 0` (our selinuxfs stub) → ALLOW. Same
permissive pattern the samgr SA gate already rides.

Verification (b152/b153): zero `getpeercon failed` lines, zero
`Forbid to set parameter bootevent.*`; 17 distinct bootevents set
(wms.ready, appspawn.started, bms.main.bundles.ready, ...);
bootanimation passes the wait and proceeds into the render setup.

## What to watch in the next boots

- `drm: open card0 (#2) by pid <composer_host>` — anim-block's lazy-open
  canary: composer VDI opens card0 on the FIRST client HDI call. Today
  zero HDI calls are delivered because no client reaches a
  buffer-request.
- `SAMGR: SA:XXXX OpenSo <n>ms` — any value in minutes points at the
  slow file-read path (1.5 s/page fault); fixing that is the highest-
  leverage move for OH pixels now (both video and launcher paths).
- `USEGV-FOR: pid=<hilogd>` — the hilogd crash (b154 ~line 4774) blocks
  all userspace log visibility.
- Missing bootevents vs the Linux reference after this fix:
  launcher.ready, lockscreen.ready, account.ready, wms.fullscreen.ready,
  bootanimation.ready — all downstream of the same stalls.

## Boot index (this window)

| boot | kernel | config | result |
|---|---|---|---|
| b151 | pre-fix | KVM seeded, dfx=periodic,futextrace,taskdump | 225k lines; SA chain up; all bootevents FORBIDDEN; bootanimation spins in WaitRenderServiceInit; died ~12:50 on its own |
| b152 | +SO_PEERSEC | same | ALL bootevents set; bootanimation started+finished instantly (no render); killed after evidence |
| b153 | +SO_PEERSEC | dfx=periodic,taskdump,bindertrace | full IPC story: node+surface create work; video path polls SA 3002 (registered 802 s late); killed after evidence |
| b154 | +SO_PEERSEC | persistent userdata (no snapshot) | /data/log/hilog empty — hilogd SIGSEGV'd early; killed |
| drm_probe | +SO_PEERSEC | drm_probe as init | **PASS 31/31; 720x1280 pattern on screen (100% non-black)** |

## Reproducing

```bash
# SO_PEERSEC boot: standard oh-boot-seeded.sh tag / test/run-oh.sh
# DRM end-to-end proof:
python3 test/drm_probe_boot.py --timeout 300   # PASS 31/31 + screendump
python3 test/debug/shotstat.py work/drm_probe_shot-gpu0.ppm
```
