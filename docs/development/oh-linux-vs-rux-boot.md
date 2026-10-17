# OpenHarmony boot: reference Linux vs Rux (milestone-aligned diff)

Date: 2026-10-18. Lane: `feature/oh-linux-ref` (analysis only; no kernel changes).

Question being answered: Rux boots OH deeper than ever (76 services,
`drm_service` exec'd, composer `.so` loading) but never reaches
`drm: open card0`. Is the stall a userspace/token problem, a kernel-interface
gap, or something else?

Method: boot the same image set on OH's own reference Linux kernel
(`oh-robot/out/x86_64_virt/packages/phone/images/bzImage`, Linux 6.6.101
OHOS) under QEMU/KVM, machine/device/cmdline copied verbatim from
`oh-robot/qemu_run_user.sh`, serial captured to a file, shared images
protected by `-snapshot`/`file.locking=off`. Compare against Rux's deepest
run `oh-display/run/oh-b116-serial.log` (1.34M lines; 1.28M of them are the
kernel's `btx` binder trace).

Runs (kept under `runs/`, untracked):

| run | kernel | userdata | result |
|---|---|---|---|
| `runs/oh-ref-r1` | reference Linux | stock populated 17G | UI: `bootanimation.finished` 10.7s, `launcher.ready` 10.1s, launcher pixels on screendump |
| `runs/oh-ref-r2` | reference Linux | blank 4G ext4 (mirrors Rux) | same milestone: `launcher.ready` 9.9s, launcher pixels on screendump |
| `oh-display/run/oh-b116-serial.log` | Rux | 4G with injected `nativetoken.json` | stalls: 0 SA registrations, black screen |

Reference scripts: `test/run-oh-linux-ref.sh` (boot) and
`scripts/qemu-mon.py` (QEMU monitor client for headless `screendump`).
Headless `-display none` + `screendump <file> ohgpu` on the named
virtio-gpu console captures the real scanout (360x720, the reference
geometry) — no SDL display needed.

## Verdict in one paragraph

The reference Linux reaches a rendered launcher in ~10s from the same disks,
with the same init, in a headless VM, with **either** the stock userdata
**or a blank one**. Rux's stall is a single kernel-side defect chain: native
access tokens set by `init` land in a kernel store that the binder driver
never reads, so every binder sender reports a token whose type bits decode
as `TOKEN_SHELL` (2) instead of `TOKEN_NATIVE` (1); samgr then rejects every
`AddSystemAbility` (101 denials observed, 0 SAs ever registered), starting
with hdf_devmgr's SA 5100; `render_service` can never fetch SA 5100 and so
never publishes SA 10; `bootanimation` polls `CheckSystemAbility(10)`
180,135 times, gets `OBJECT_NULL` (7) every time, and the screen stays
black. Everything else on Rux — service spawn, order, mounts, HDF host
loading, composer `.so` dlopen — matches the reference.

## (d) Display-chain timeline on reference Linux — the checklist Rux must satisfy

r1 timestamps (kernel time, KVM):

| t (s) | event | evidence |
|---|---|---|
| 0.64 | kernel virtio-gpu binds at 0000:00:01.0; `/dev/dri/card0` + `fb0` exist | `[drm] Initialized virtio_gpu ... minor 0`, `fb0: virtio_gpudrmfb` |
| 0.74 | `/dev/access_token_id` char device init | `access_token_id: access_tokenid init success` |
| 1.9 | `/dev/block/vdf → /data` mounted | `Mount /dev/block/vdf to /data successful` |
| ~1.9 | init runs `load_access_token_id` (from `/etc/init.cfg`), allocates native tokens for all services | no `gettotkenid failed` lines; 2 benign `failed to set access token` warnings |
| 2.88 | `hdf_devmgr` exec (pid 169) | `ServiceExec hdf_devmgr` |
| **2.90** | **hdf_devmgr publishes SA 5100** (`DEVICE_SERVICE_MANAGER_SA_ID`) — 10 ms after exec | `SAMGR: insert SA:5100_2` |
| 4.56 | `bootevent.hdf_devmgr.ready` | param service |
| 4.84 | `composer_host` exec (pid 567); ready at 4.89 | `bootevent.composer_host.ready` |
| 5.02 | `render_service` exec (pid 717) | |
| 5.25 | `bootevent.renderservice.ready` | |
| 5.25 | `bootanimation` exec (pid 886) | |
| 5.57 | **`render_service` publishes SA 10 (`RENDER_SERVICE`)** | `SAMGR: insert SA:10_58` |
| 5.60–5.62 | `bootevent.bootanimation.ready/.started` | |
| 5.74 | `bootevent.wms.ready` | |
| 10.1 | `bootevent.launcher.ready`, then `lockscreen.ready` | |
| 10.7 | **`bootevent.bootanimation.finished`**; bootanimation exits 0 at 11.8 | `ServiceReap info bootanimation` |
| 68+ | periodic DRM use: `RSRenderThread`/`llvmpipe` ioctls on card0 (`PRIME_FD_TO_HANDLE` 0x2e, `MODE_MAP_DUMB` 0xb3) | audit trail |
| — | launcher on screen: screendump 360x720, 0% black, launcher wallpaper `bdbec2` + blue icons `5da2db` | `runs/oh-ref-r1/shot1.ppm`, `runs/oh-ref-r2/shot-launcher.ppm` |

The exact ordering dependencies that Rux's log disproves today:

```
/dev/access_token_id usable by init  →  services carry NATIVE tokens
  → hdf_devmgr AddSystemAbility(5100) accepted        (Rux: denied, 101x)
    → render_service CheckSystemAbilityImmediately(5100) returns object (Rux: OBJECT_NULL)
      → render_service publishes SA 10                 (Rux: never)
        → bootanimation CheckSystemAbility(10)         (Rux: 180,135 polls, all OBJECT_NULL)
          → composer/render create+map dumb buffers on /dev/dri/card0 (llvmpipe)
            → frames → bootanimation.finished → launcher
```

Note on "drm: open card0": that string is a Rux-side kernel printf. The
reference kernel shows the equivalent milestone only through the audit
trail (`RSRenderThread`/`llvmpipe`/`render_service` ioctling
`/dev/dri/card0`) plus the on-screen pixels — both present on r1/r2, both
absent on Rux.

## (a) Services: order and set

Reference r1: 123 service starts / 93 unique services. Rux b116: 74/74.

- **Order**: effectively identical (same init cfg, same jobs). The first
  ordering difference is parallel-start jitter (`key_enable` at slot 2 vs
  slot 5, `storage_daemon` before vs after `samgr`). No semantic divergence.
- **Set**: Rux starts a strict subset — 0 services exist on Rux that don't
  exist on the reference.
- **19 services exist on Linux but never start on Rux**, all spawned by
  bootevent triggers between 4s and 10s (i.e. downstream of the SA
  registrations that fail on Rux — symptoms, not causes):
  `app_domain_verify_agent asset_service backup_sa clearplay_host
  deviceauth_service deviceprofile dhardware distributedsched
  dlp_permission_service dslm_service i18n_service installs
  intell_voice_host nwebspawn sandbox_manager_service
  security_component_service updater_sa usb_service wifi_manager_service`
- Restart behavior: the reference restarts flaky services
  (`watchdog_service` 3 starts, `usb_host` many) — Rux's log shows
  hdf_devmgr exit(1) (see (c)) and no successful restart cycle.

Common noise, identical on both kernels (NOT divergences):
`Failed to open realPath` cgroup paths 15× each, `hguard` 2× each,
`Failed to mount moving /mnt/data` once each. Rux's 325× `/dev/ucollection`
chmod retries are just a longer-running log, not a different behavior.

## (b) accessToken: who sets the native token, and what the "poll" really is

### What the Rux trace actually shows (decode)

`btxp` parcels from bootanimation (pid 495) to samgr (pid 332), repeated
180,135 times:

```
00010000 00000000                          strictModePolicy(0x100) + workSource(0)
16000000 <utf16 "ohos.samgr.accessToken">  interface token (u16 len 22)
0a000000                                  systemAbilityId = 10
```

`ohos.samgr.accessToken` is **not** an access-token interface — it is
`SAMANAGER_INTERFACE_TOKEN`, the descriptor of `ISystemAbilityManager`
(`if_system_ability_manager.h:392`). Code 2 is
`CHECK_SYSTEM_ABILITY_TRANSACTION`. The 4-byte reply `07000000` is
`OBJECT_NULL = 7` (`ipc_types.h`) — samgr saying "SA 10 not registered",
not a token mismatch. Likewise render_service's code-12 polls
(`CHECK_SYSTEM_ABILITY_IMMEDIATELY`) carry `0x13ec = 5100`.

So the "hdf_devmgr polls samgr getting zero replies" framing was two steps
off: the pollers are bootanimation (SA 10) and render_service (SA 5100),
and the reply is `OBJECT_NULL`, which is merely the downstream symptom.

### Who sets native tokens (source-verified, runs on the reference)

1. `init` executes the `load_access_token_id` command from `/etc/init.cfg`
   (`init_cmds.c:1012` → `LoadAccessTokenId` → `GetAccessToken`,
   `init_service_manager.c:1451`).
2. For **every** declared service, `GetAccessTokenId(&NativeTokenInfoParams)`
   (`interfaces/innerkits/nativetoken/src/nativetoken.c:855`) opens+locks
   `/data/service/el0/access_token/nativetoken.json` **directly on the
   filesystem — no IPC**, reuses or allocates a token id, writes the file
   back. On r2's blank userdata it logs `GetFileBuff: Empty file` and
   regenerates the whole table; first boot needs no populated `/data`.
3. When init forks each service, the child — still root, pre-setuid — calls
   `SetAccessToken` → `SetSelfTokenID(tokenId)`
   (`init_common_service.c:460`), an `ioctl(ACCESS_TOKENID_SET_TOKENID)`
   on `/dev/access_token_id` (`token_setproc.c`). The OH kernel stores it
   per task; OH's binder patch carries it on every transaction.
4. samgr's gate `CanRequest()`
   (`system_ability_manager_stub.cpp:1289`) does exactly one thing:
   `GetTokenTypeFlag(GetCallingTokenID()) == TOKEN_NATIVE`. And
   `GetTokenTypeFlag` is **pure bit decode** —
   `(token >> 27) & 3` from the id itself (`accesstoken_kit.cpp:210`,
   `AccessTokenIDInner` layout in `access_token.h`), no table, no IPC.
   Token id layout (LE bitfields): uniqueID 20b | res 4b | clone 1b |
   render 1b | dlp 1b | **type 2b (27–28: 0=HAP, 1=NATIVE, 2=SHELL)** |
   version 3b (29–31, =1). Real native tokens are `0x28xxxxxx`.

So: **the fix-line's "samgr only checks type bits" belief is confirmed by
source — but the "nothing on this image ever sets tokens" belief is wrong.**

### What actually happens on Rux (b116 evidence)

- Rux log has **zero** `gettotkenid failed` lines and **zero**
  `failed to set access token` warnings: init's token allocation from the
  injected `nativetoken.json` *succeeded* and every service child's
  `SET_TOKENID` ioctl *returned 0*.
- But the binder reported `token 0x140001e9`-style values (version 0,
  type 2 = SHELL, uniqueID = pid) for every sender, and the new registry
  driver's `atid: pid … ioctl` trace appears **0 times** in the 1.34M-line
  log.
- Root cause (kernel, `feature/oh-display` @2c875058):
  - `/dev/access_token_id` is registered **twice** in
    `kernel/src/fs/devfs/mod.rs`: the old skeleton
    (`drivers::access_tokenid::DEV_ACCESS_TOKENID`, writes
    `task.access_token`) is inserted by the `char_nodes` batch **after**
    the new registry (`ipc::access_token::DEV_ACCESS_TOKEN`), so the batch
    insert **wins the node**.
  - `syscall/io.rs` still dispatches to the old
    `tokenid_file_ioctl` (present at line ~979) — init's SETs land in the
    per-task field of the old driver.
  - The binder's `GET_SENDER_INFO` reads
    `ipc::access_token::token_of(pid)` — the new registry, which is never
    written → falls back to the synthetic per-pid token.
  - Net: SET path and GET path are **two disjoint stores**; the real tokens
    init stored are invisible to binder, and samgr sees SHELL type →
  `AddSystemAbilityInner PERMISSION DENIED!` ×101 → hdf_devmgr exit(1).

### Does populated userdata matter?

No. r2 (blank 4G ext4, no `nativetoken.json`) reaches `launcher.ready`
at 9.9s; the nativetoken kit regenerates the file on first boot and ATM's
own table loader (`services/common/json_parse/src/json_parse_loader.cpp`,
same file path) picks it up inside samgr. Rux's blank-4G userdata and the
`userdata-tok.img` variant are both fine; neither is the blocker.

## (c) hdf_devmgr: from token check to composer card0 (reference behavior)

On Linux the whole "assignment" phase is done before serial can even log
it:

1. 2.88s `ServiceExec hdf_devmgr`.
2. 2.90s `SAMGR: insert SA:5100_2` — `AddSystemAbility(5100)` accepted on
   the first try (10 ms after exec). This is the step that never succeeds
   on Rux: there, AddSystemAbility is denied 101 times and hdf_devmgr
   `exit(1)` (`exit_code=256` in b116), taking its `OS_IPC_*` threads with
   it; the devhosts never get driver assignments from the dead devmgr.
3. 4.56s `bootevent.hdf_devmgr.ready`.
4. `composer_host` (a devhost) execs at 4.84s and reaches
   `bootevent.composer_host.ready` at 4.89s — the display composer driver
   (in-proc HDF) is loaded by then. (hdf internal logs go through hilogd,
   not serial, on both kernels — the .so-level detail is only visible in
   Rux's own `exec:`/dlopen traces.)
5. render_service gets SA 5100 from samgr (`CHECK_SYSTEM_ABILITY_IMMEDIATELY`),
   talks HDF to the composer, publishes SA 10 at 5.57s, and the
   render/llvmpipe threads open `/dev/dri/card0` (audit: ioctls 0x642e
   `PRIME_FD_TO_HANDLE`, 0x64b3 `MODE_MAP_DUMB`).

## (e) Kernel interfaces the reference uses that matter to Rux

Proven-in-use on the reference (audit + serial), with Rux status:

| interface | used by | Rux status |
|---|---|---|
| `/dev/access_token_id` misc char (4 ioctls, `u64` payload) | init service children, libbinder (`GetSelfTokenID` on every binder open), ATM | device exists but node is owned by the old skeleton; binder reads a different store — **the blocker** |
| binder sender-token ioctls (`BINDER_GET_SENDER_INFO`, `BINDER_GET_ACCESS_TOKEN`) | samgr (`GetCallingTokenID`) | implemented (`GET_SENDER_INFO` incl. real pid) but fed from the wrong store |
| DRM virtio-gpu → `/dev/dri/card0` (+`fb0`) | render_service, bootanimation (`RSRenderThread`), llvmpipe | Rux has a virtio-gpu/DRM path (`kernel/src/drivers/drm|gpu`); the "drm: open card0" milestone depends on the SA chain above, not on new DRM ioctls |
| DRM ioctls actually exercised | `PRIME_FD_TO_HANDLE` (0x2e), `MODE_MAP_DUMB` (0xb3), i.e. dumb-buffer + fd import (software GL) | matches Rux's fbdev/dumb plan; no virgl/3D needed |
| `TIOCGWINSZ` (0x5413) on `/dev/access_token_id` | fdsan-style isatty probes | harmless; Rux returns success already |
| SELinux (selinuxfs, policy, permissive) | samgr's `CheckGetSAPermission` gate | image runs `WITH_SELINUX`; gate passed on Rux too (no selinux denials in samgr on Rux) — not a blocker |
| cgroup v1 dirs (`/dev/cpuset`, `/dev/blkio`, `/dev/pids`, `/dev/memcg`) | init per-service writes | **absent on both kernels** (identical `Failed to open realPath` counts) — not blockers |
| `/dev/__parameters__/*` param files, `/dev/rtc0`, `/dev/input/event0` | param service, time, input | present on Rux (param files are init-created tmpfs) |

## Top divergences, ranked (for the fix line)

1. **Token store split (kernel, oh-display tree).** `/dev/access_token_id`
   double-registered in devfs; old skeleton wins the node, binder reads the
   new registry, registry never populated. Fix: make one store — either
   route the node to `ipc::access_token` and delete the old
   `drivers/access_tokenid` registration + `io.rs` dispatch, or make
   `GET_SENDER_INFO` read the task's token set by the old driver.
   Expected effect alone: 101 denials → 0, SA 5100 registered, and the
   real `0x28xxxxxx` tokens from `nativetoken.json` flow end-to-end.
2. **If synthesizing tokens anyway**: they must not merely carry NATIVE
   type bits. `samgr`'s `CanRequest` only checks type bits (true), but the
   ATM inside samgr loads the SAME `nativetoken.json` into its permission
   table (`json_parse_loader.cpp`), and later `VerifyAccessToken(tokenId,
   perm)` calls by other services look the id up there. A
   `0x28000000|pid` synthetic id passes `CanRequest` but fails every
   permission check that consults the table. The robust fix is #1; with #1
   the table ids and the reported ids match automatically.
3. **hdf_devmgr restart**: after hdf_devmgr exit(1) on Rux nothing brings
   it back (it is a critical service — the reference restarts criticals;
   verify init's critical-restart path fires on Rux once #1 lands).
4. Not blockers (settled by the reference runs): populated userdata (blank
   works), SELinux absence, cgroup absences, service-order differences,
   the 19 unspawned services (all downstream of #1), virtio-gpu geometry
   (360x720 reference vs 720x1280 Rux — both are valid panel sizes; the
   image adapts).

## Reproducing

```bash
# reference boot (KVM, headless, serial to file, snapshot-protected disks)
test/run-oh-linux-ref.sh runs/oh-ref-r1            # stock userdata
test/run-oh-linux-ref.sh runs/oh-ref-r2 runs/userdata-blank.img

# screendump (headless, named virtio-gpu console)
python3 scripts/qemu-mon.py runs/oh-ref-r2/monitor.sock "screendump shot.ppm ohgpu"
```

Watch for: `SAMGR: insert SA:5100_2`, `insert SA:10_`, and
`bootevent.launcher.ready` — with KVM the whole chain completes in ~11s.
