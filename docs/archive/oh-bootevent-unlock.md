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
      Minimal reproduction attempts (test/faultlat*.c, this kernel,
      q35 + virtio-blk-pci + KVM): single reader 54 us/page (with or
      without journal), 6 concurrent processes on separate files
      0.3–1 ms/page (max 26 ms), read() 1723 MiB/s — the raw kernel
      read path is FAST. The OH-case slowness therefore needs the real
      workload mixture (hundreds of tasks, /data writes + journal,
      binder traffic) to reproduce; it is a fresh instrumented hunt
      (dfx=memwatch + bio tracing on the OH image), not a raw-I/O bug.
      Side finding: CLOCK_MONOTONIC in-guest jumped ~700 s during one
      probe run (faultlat "total=701485ms" vs 54 us avg × 8192 pages) —
      timer-lane bug, untracked so far.
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

## faultlat addendum (2026-10-18, feature/faultlat): the page-cache side of the 1.5 s/fault family

Lane: `feature/faultlat` @ cc7a66cc (from main 26e61516). Target: the
`SA:3002 OpenSo 802166ms` stall (ranked blocker #1 above).

### Root causes found in kernel/src/fs/page_cache.rs (all fixed on the branch)

1. **Hard 32 MB cap** (`MAX_CACHED_PAGES = 8192`) vs a multi-GB `.so`
   working set: every fault was miss + eviction + refetch + ext4-biglock
   round trip. The cap is now half of managed RAM (min 32 MB), computed
   lazily from zone `managed_pages`.
2. **Anti-LRU eviction, twice over.** Fresh pages linked at the LRU
   *tail* — the same end eviction scanned from — and the scan followed
   `lru_next` from the tail, which is the list TERMINATOR direction: each
   `evict_one` examined exactly one page, the newest. Concurrent fillers
   evicted each other's freshest pages. File pages now enter at the young
   end (head), the scan walks tail→head via `lru_prev`, Referenced pages
   activate onto the young end of LRU_ACTIVE_FILE (mm/lru.rs additive
   helpers; anon/vmscan untouched).
3. **No active-list demotion** (`lru_deactivate` had zero callers): every
   page ever served accumulated on LRU_ACTIVE_FILE, eviction stalled, and
   inserts fell through to the hash-order BTreeMap fallback = random
   eviction. `evict_one` now demotes the oldest ACTIVE_FILE pages back to
   inactive (second chance) before the fallback.
4. **`insert_batch` allocated frames UNDER the inodes lock** (the
   R-WRITESTORM-1 self-deadlock `insert()` was fixed for, resurrected):
   GFP_KERNEL slowpath → try_to_free_pages → shrink → inodes.lock held by
   self. At RAM-scale cache pressure this wedged a CPU per fill batch.
   Batches now allocate each 64-frame chunk before the lock.
5. Two amplifier fixes on the reclaim side: `balance_pgdat` now reclaims
   32-page batches (was exactly `1<<order` = ONE page per full pass), and
   the allocator's `IN_DIRECT_RECLAIM` gate is acquired with a bounded
   fastpath-retry + `yield_cpu()` wait instead of skipping reclaim and
   failing the allocation while another task reclaims.

### Numbers (test/faultlat3.c: 6 mmap loaders + 2 journal-commit writers, KVM q35 SMP2)

| regime | baseline 26e61516 | fixed cc7a66cc |
|---|---|---|
| under-cap (6×16MB, 2G) | wall 49.2 s, worst avg 11.9 ms/page, max 0.9 s | wall 1.79 s, worst avg 360 µs/page |
| at-cap (6×128MB vs 720MB cap, 2G) | wall 509 s, avg 13–15 ms/page, max 9.4 s | wall 58 s, avg 1.0–1.6 ms/page, 6/6 loaders |

`/proc/meminfo` gained a Rux-extension `PageCacheOps:` line
(hits/misses/evictions/demotions/cap_pages) — the hit/miss ratio is the
direct thrash readout on any boot.

### OH boots b1–b6 (this window): the SA-load chain is dead on the BASE kernel — not a faultlat regression

Every OH boot on this kernel family (fixed AND a pristine-base control,
KVM SMP2/SMP1 4G, `dfx=periodic,taskdump`) reaches 55 services started
and then ZERO `OpenSo` lines ever complete; sa-host services (foundation
pid ~511 among them) go ZOMBIE and are not restarted; clients wait
forever. The one working reference, oh-display's b155 soak (same images
via run/imgt, OLDER kernel build), had 12 OpenSo lines before
media_service exec. Kernel delta base↔b155's build: the anim-block merge
family (softdog node, biglock census/handoff-recovery, missstat) — worth
a bisect from whichever lane owns that merge.

Also decisive: **the userdata image matters**. With
`oh-img/userdata-seeded.img` (md5 17c656…) every boot hard-wedges at
~13.7k serial lines; with `userdata-tok.img` (md5 bb4be5…, what b155
uses — nativetoken-pre-seeded) the boot stays alive and keeps processing
(any kernel). The two images are NOT interchangeable despite the name.

Serial logs: run/oh-b{1..6}, oh-bctl (base control) -serial.log in the
faultlat worktree. Residual known-issue from the at-cap probe: one
uniform ~17 s stall per loader run start (all loaders, once) — first
contact of the six files + journal writers + reclaim settling; avg after
it is healthy. Next steps for OH pixels: (1) bisect the base-kernel
SA-load regression (b155's build vs 26e61516), (2) rerun the decisive
boot on a base that completes OpenSo — the faultlat fix should then turn
the OpenSo seconds into sub-second times.

## basewedge addendum (2026-10-18, feature/basewedge): bisect closed — no base regression; seeded.img re-tested clean on merged main

Verdict of the base-kernel bisect this worktree was opened for: **there
is no base regression to bisect.** The section above ("SA-load chain is
dead on the BASE kernel") does not reproduce. Measured with
md5-verified images (run/oh-boot.sh: KVM q35 SMP2 4G, loglevel 7, six
disks, virtio-gpu 720x1280, no dfx overrides; logs
run/oh-{C1,C1seed,Wseed}-serial.log):

| boot | kernel | userdata | outcome |
|---|---|---|---|
| C1 (600 s) | dd9b5b9b pristine main | tok bb4be52e | 79 services, 78 OpenSo, SA:3002 **22.8 s**; bootevents SET: bootanimation.started/.finished, composer_host/renderservice/wms, bms bundles ready; alive at window end (samgr+bootanimation vruntime still advancing) |
| C1seed (900 s) | dd9b5b9b pristine main | seeded 17c656 | **no wedge**: 80 services, 81 OpenSo, SA:3002 **16.2 s**, same SET chain plus appfwk.ready; foundation SAs (6200/6105/403) init-finished ~35 s; foundation never dies |
| Wseed (600 s) | 26e61516 pre-merge base | seeded 17c656 | no wedge either — slow-flow family: 78 services, 46 OpenSo in the window, per-SA times 39–305 s (SA:3301 236 s, SA:3008 306 s), SA:3002 not reached before the window closed (= the b153 802 s stall, not a wedge); composer_host.ready set |

The b1–b6/bctl shape (55 services, ZERO OpenSo, foundation ZOMBIE at
~13.7k lines) reproduced with **none** of these cells — including the
exact kernel+image pair bctl claimed (26e61516 + seeded-family image,
which it reached with 55 services/0 OpenSo at 20.7k lines versus
Wseed's 78/46). Those boots ran faultlat-worktree builds with a local
`userdata-mine.img` (since deleted), so the 0-OpenSo wedge belonged to
the mid-branch build family and/or that image copy — not to main's
base, and not to `userdata-seeded.img` itself.

Faultlat fix validated in the OH boot path: SA:3002 .so load 802.2 s
(b153) / 729 s (b155 soak) → 16–23 s on dd9b5b9b, on either image. In
the one seeded-vs-tok pair measured, seeded was the faster boot of the
two (16.2 s vs 22.8 s) — its pre-populated /data tree cuts first-boot
churn; single sample, not a claim.

What the two images actually differ by (debugfs -c, read-only):

- **tok**: near-blank ext4; only seeded content is
  `/service/el0/access_token` (0775 1000:1000) — the nativetoken
  preseed; live first boot creates everything else.
- **seeded**: snapshot of a partially-booted /data — root 0771 1000:1000
  (tok root 0755 0:0), full `service/el0..el5` and `app/el1..el5`
  trees root:root 0711, plus `samgr/ system/ local/ log/ misc/
  updater/ …`. First boot scans/rewrites that whole stale tree — far
  more page-cache pressure than the blank image, which is exactly the
  load the mid-branch page-cache defects turned into a hard wedge.

**Guidance (replaces the quarantine):** keep tok as the reference
userdata for comparability with b-series numbers; seeded.img is NOT
poisoned on dd9b5b9b and later — usable as a heavier (and in one
sample, faster) variant. Do not use it to judge pre-merge kernels: it
amplifies their page-cache stall (Wseed per-SA minutes vs b153's
single 802 s stall on tok).

Unchanged frontier + one new observation for the next lane:
`bootevent.boot.completed` stays unset on both images (watcher
registered, never set). And in C1 only, foundation (pid 792) is
SIGKILLed (SIGDEATH sig=9, ppid=1) at ~57k lines — AFTER
wms.ready/bootanimation.finished — with appspawn kickdog spinning
"App enable -1" thereafter; a post-completion userspace/watchdog
question, not a kernel wedge. C1seed shows no foundation death.

Kernels: run/kernels/{A-c6b7c79a,C-dd9b5b9b,W-26e61516}-bzImage (A
unbooted — the bracket was mooted once C flowed). No kernel changes
required; this branch carries only the boot script and this doc.
