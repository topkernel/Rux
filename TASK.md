Lane: LTP r20 full scan (riscv64)
Baseline r19 = 1,018/1,869. Work autonomously; never ask. Worktree = THIS (main@82059759; test/rootfs.img may need symlink -> /home/william/workspace/Rux/test/rootfs.img).

## Mission
1. Reconstruct deleted infra from git (main + `git branch -r | grep archived`): the LTP runner, the 1,869-case list, the per-round results layout (convention /home/william/rux-agents/ltp-r<N>/results/). test/patrol-runbook.md + test/suite.sh carry conventions. LTP source: userspace/linux-ltp/ltp-20240524.tar.xz (extract-build convention like toybox; musl SDK via `make sdk`; soft-fp overlay discipline userspace/build-softfp.sh — BUG-S005 zcb pin).
2. Build kernel (riscv64) + LTP with the pinned march.
3. Full 1,869 scan (detached/setsid; long runs survive tool timeouts). Known flake families (rerun 2x in isolation before counting new failures): accept02 getrusage04 kill02 genfmod rename/statfs.
4. Results to /home/william/rux-agents/ltp-r20/results/. Deliver r20 vs r19 delta table.
5. New real failures: triage against recent main merges (aarch64 merge b238ebd1 shared diffs: virtio device-ID table, wake floor per-arch, ELF machine checks, boot-table honesty) — compare failing syscall surface vs merge diff.
6. Small kernel fixes welcome (small commits, English, NEVER "Linux"); rerun affected family.

## Discipline
One QEMU at a time (check ps — other lanes live); -smp 2 -m 1G; kill only yours; pkill FORBIDDEN. make test stays green; x3 builds rc=0 if kernel touched. Push origin/ltp-r20.
Final message: r20 number + delta table + flake/real verdicts (2x rerun evidence) + fixes + results path.

---

# Perf review: mm

Focus: MEMORY — p3a anon fault med 0.08µs but p99 581µs (bimodal: what is the slow path paying?); P4 fork+exit 9ms / fork+exec 13ms (reference ~1ms class); TLB shootdown cost; mmap/munmap paths; copy_page_table_cow walk; vma tree operations; the allocator family (small_alloc/buddy/slab — the OH campaign named phantom preempt-count acquire sites there; the REST share, magazine discipline); demand-fill vs fault-around (aarch64 noted anon fault-around deferred; x86 batch fill exists — compare).

Perf-review lane rules (binding)
- Mission: find where Rux is SLOW vs the reference kernel and close the gap. Target: 10x-class improvements on the lagging metrics. The user mandate: every interface metric where we lag the reference must be examined.
- GROUND TRUTH: the reference kernel bzImage is at /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage. test/pbench_boot.py --bzimage <path> boots ANY bzImage with the pbench P1-P7 battery — run the SAME battery on the reference kernel and on your rebuilt Rux kernel to get true A/B columns. This is the primary tool: measure reference, measure ours, fix, re-measure.
- Known instrumented gaps (from syscost/OH campaign, x86_64 debug): futex wake→run tick-quantized 10-108ms (reference: µs); openat avg 252µs; p1 null syscall ~4.5µs; p2 ctxsw 15.4µs (debug); p6_append ~640µs; write_fsync ~12ms; mkdrmdir ~20ms; unlink ~1ms; pipe roundtrip ~19µs; UART print 0.36-0.93ms/line; OH boot to launcher.ready 254-541s vs reference 14.4s (~26x whole-system).
- Every fix ships with NUMBERS: pbench median before/after (same boot config), or syscost per-op average before/after. No numbers = not merged.
- Correctness gate: `make test` green (riscv 1198) + x3 builds rc=0. If you touch aarch64 paths, `make test PLATFORM=aarch64` green too.
- Small commits, English messages, NEVER the word "Linux" in commits/docs — write "the reference kernel".
- Record findings/fixes/numbers in run/perf-findings.md; push origin/perf-<scope>.
- One QEMU at a time (check ps; many lanes run); kill only your own; pkill FORBIDDEN.
- Worktree: THIS one. Never write to /home/william/workspace/Rux.

---

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

### lb29 addendum (clean-config retry, release + fresh /data + wwcensus)
Slow variant: unlock at 746s (lockstatus_100 set), USER_UNLOCKED events
fire 806-821s, NO accountmgr reset this time — and STILL no launcher:
AMSI1640 never prints, and com.ohos.systemui NEVER SPAWNS either (0
mentions; lb10-era boots DID spawn systemui from StartUser). So the
post-unlock gate is upstream of both ability launches: foundation
receives the StartUser(100) oneway (lb28's AMS46 latch @283s) but the
SwitchToUser/user-start ability chain never executes inside foundation
on these boots — the ams46 walled-draw family. Fifth successor: get an
ams46 boot through a 237s-class unlock (lb27 config luck) and read the
AMS46 read/free/retire + census lines for node 12xxx — they name
whether the oneway is parked in the async slot, read and dropped, or
racing a dead reader.

## FIFTH-SUCCESSOR SESSION (2026-10-31 20:00-23:xx, branch oh-launcher @ a7189470+)
Kernel delivery is CLEAN; the wall is INSIDE foundation userspace, and it
has TWO shapes. All boots: release kernel, LOGLEVEL=6, SMP=2,
USERDATA=run/userdata-fresh2.img, dfx=ams46(+txnhunt/periodic).

### The ams46 verdict (lb30/lb32/lb34/lb35/lb36/lb37)
1. **lb30**: ENQ 807.707 (accountmgr→foundation AMS node, oneway 0x11) ->
   READ by foundation MAIN thread 1ms later -> BC_FREE 8ms after. The 30s
   silence-rule drop NEVER fired — kernel delivery is NOT the wall. The
   old "ams46 walled-draw" family is closed: the oneway executes.
2. Instrumentation landed for the follow-window (all pushed):
   - RIPPLE (a7189470): after the traced READ, print every txn whose
     sender/receiver is foundation or the StartUser sender, 120s, budget 640.
   - THREAD-SNAPSHOT (19b195f3): the reader thread's in-flight syscall
     (kernel-stack entry frame) + 16-entry nr-ring, every 10s.
   - HILOG-AMS cohort at WARN in the unix-socket hilog tap (unix.rs): BMS/
     AMS/accountmgr verdict lines on the console at loglevel 6.
   - AMS46-R parcel hex (80B) + ENQ sender euid.

### Shape A — the LOOP (lb34, lb36): DECODED, root cause named
- Chain on the console: `CreateNewUser user(100) start` → `startUser in
  service:100` → `foregroundUserId:0` → `SwitchToUser old=0 new=100` →
  `waiting query highest priority ability info completed` x588+ forever.
- THREAD-SNAPSHOT: reader tid in nanosleep (sys=35) + clock_gettime churn
  = AMS's StartHighestPriorityAbility isBoot=true retry ladder
  (ability_manager_service.cpp:7905): `ImplicitQueryInfoByPriority` keeps
  returning false → usleep → retry, NO attempt limit on the boot path.
- The query needs com.ohos.launcher's PER-USER record for user 100, which
  comes from CreateNewUser(100)'s per-user install stream
  (OnCreateNewUser → InstallByBundleName for EVERY preinstalled bundle →
  installd SA 511). On our kernel that stream's installd loads burn
  15-101s EACH ("Start dynamic proc:installs,511,105_*ms" — samgr's
  ServiceControlWithExtra against init timing out) and repeat.
- FIX LANDED (image): run/lb-mkimage2.sh → run/system-lb2.img flips
  installs.cfg "ondemand" true→false. Result on lb36/37: 0 dynamic
  loads, CreateNewUser(100) COMPLETES (start+end lines). Necessary.
  NOTE: sa_main for an on-demand-profile SA still only registers its
  PROFILE at boot (samgr code 10 = ADD_ONDEMAND); the live SA loads on
  the first LoadSystemAbility(511) — now into a resident process.

### Shape B — the FAST-FAIL (lb35, lb37): errcode pending lb38
- `CreateNewUser end` (success!) → `startUser in service:100` →
  OnStartUserDone oneway (code 0x1, dlen 68) back to accountmgr within
  ~5ms → accountmgr logs "Failed to AbilityManagerService in call back"
  → activation fails, rollback, NOTHING spawns. No foregroundUserId/
  SwitchToUser lines → the fail is INSIDE CheckUserParam (AMS
  user_controller CheckUserParam), and NO IsOsAccountExists binder call
  nor samgr lookup preceded the callback (ripple). Kernel euid stamp at
  ENQ = 3058 (correct, printed). The errcode rides at parcel offset 62
  (int32): 2097177=CHECK_PERMISSION_FAILED(uid leg), 2097193=
  INVALID_USERID_VALUE(IsExistOsAccount false). lb38 (80B parcel lens)
  reads it.

### CLOSE-OUT (00:30): Shape B named to its source line; errcode decoded
- lb38 parcel decode: OnStartUserDone(100, 2097193=INVALID_USERID_VALUE).
- lb40 keys landed: "failed to get os account service" (os_account.cpp:894
  GetOsAccountProxy) fires x137 in foundation 9.9-139.8s — ALL before
  samgr's "insert SA:200" (141.3s): the ohos-account kits' proxy is
  unobtainable until SA 200 registers; every osaccount query in that
  window fails with NO kernel traffic (proxy null).
- lb42 (the clean kill): CreateNewUser end -> startUser at 225.485 ->
  **UN: [user_controller175]userId not exist** at 225.486 -> callback
  error. At 225s SA 200 was long registered and NO proxy-failure line
  fired — the IsOsAccountExists QUERY RAN and accountmgr answered FALSE
  for the user it had just created+unlocked: the osaccount DB read
  (OsAccountControlManager -> distributeddata KV, the DB_TaskExecutor
  SLOW-TXN family) is the boot-to-boot coin flip. Kernel-side that is
  the distributeddata/exec-class perf wall (userspace-internal futex
  waits inside executing handlers — prior TXN-HANG decode), NOT binder.
- lb40 (the loop shape with everything right): CheckUserParam passed,
  SwitchToUser entered, ladder still spinning post-CreateNewUser-end —
  the BMS "can't find target AbilityInfo" verdict key landed too late
  for that boot; UNVERIFIED whether the ladder eventually exits.

### Successor entry points (in order)
1. Boot run/system-lb2.img + dfx=ams46,txnhunt (LOGLEVEL=6, SMP=2,
   USERDATA=run/userdata-fresh2.img). Hope for the lb40 shape and read
   the "can't find target AbilityInfo" verdict: if BMS's
   ImplicitQueryAbilityInfos misses the launcher for user 100 even after
   CreateNewUser end, check GetAllPreInstallBundleInfos' disallowList
   path and the per-user record for com.ohos.launcher.
2. Shape B mitigation candidates (image-side): none clean; kernel-side
   it is the distributeddata KV read reliability under load — the
   guarded-domain perf work.
3. The re-leak producer / samgr service-start timeouts (15-101s) remain
   the deep wall behind everything; the perf lane owns them.

### lb44-lb46 addenda (the fast-fail's engine, still one facet short)
- lb44: errcode 2097193 again; euid=3058 correct; NO IsOsAccountExists
  query ENQ, NO proxy-failure line at the moment, accountmgr did NOT
  restart — the failing call never left foundation's userspace. The
  ipc-skeleton keys ("SendRequest", "work thread") landed after lb44.
- lb45 (dfx=poolcensus): ZERO POOL census lines all boot = no proc ever
  had queued todo at census time — the "queued work undrained" pool
  shape is absent; the exhaustion shape (all workers parked as CLIENTS,
  SendRequest failing locally) is invisible to that census gate.
- lb46: reader freed the code-46 after a 160s SILENT handler (sys=16
  parked in a nested binder ioctl the whole window; no TAG_LOGI even
  printed), then OnStartUserDone(100, 2097193) again. The osaccount
  query chain under load is the engine; its exact failing leg (ipc
  work-thread exhaustion vs a 160s-stuck nested call returning false)
  is the ONE remaining unnamed hop. "SendRequest: Invalid samgr stub
  object" x2 and SendRequestInner cost:1583ms lines appear on these
  boots.
- Screen on every boot this session: the frozen bootanimation frame
  (md5 5f8b704d / 06b933b4 class).

### Artifacts this session (all committed+pushed)
- Instrumentation: AMS46 RIPPLE (a7189470), THREAD-SNAPSHOT (19b195f3),
  HILOG-AMS WARN cohort + account-kits/query-verdict keys, AMS46-R parcel
  hex 80B + ENQ euid, AMS46-R-FAIL (send failures in-window).
- Image: run/lb-mkimage2.sh -> run/system-lb2.img (installs resident).
- Boots lb30-lb42; make test ALL PASSED on the final tree; x3 builds
  rc=0 (x86_64/riscv64/aarch64).
## SIXTH-SUCCESSOR DECODE (2026-11-01 01:3x, branch @ 3ad7c3e7+)
The "160s silent handler" was an INSTRUMENTATION ARTIFACT, not a shape.
Raw-log re-derivation of all three boots:
1. **2097193 = AAFWK INVALID_USERID_VALUE, source-confirmed**:
   ErrCodeOffset(SUBSYS_AAFWK=1) = 1<<21; ability_manager_errors.h
   offset+41 (0x29) with the source comment "Result(2097193) for
   invalid userid". The ONLY return sites are user_controller.cpp:157/
   166/176 (CheckUserParam's three legs).
2. **lb44 @266.819-820 and lb45 @208.830-831 are decisive**: the code-46
   handler prints "startUser in service:100" and "userId not exist"
   (user_controller.cpp:175) 1 ms apart. The ripple window (prints EVERY
   foundation txn) shows ZERO sends between them; R-FAIL never fired;
   AMS46 lifecycle READ->FREE = 4-6 ms. So CheckUserParam's
   IsExistOsAccount leg completed in ~1 ms with NO kernel binder
   traffic and NO "failed to get os account service" log (lb44's
   HILOG-AMS budget was only at [100] then — alive).
3. **The engine, named**: `OsAccount::IsOsAccountExists` pre-sets
   isExist=false; on a cached proxy_ it calls
   `proxy->IsOsAccountExists()`. `IPCObjectProxy::SendRequestInner`'s
   FIRST line: `if (IsObjectDead()) return ERR_DEAD_OBJECT;` — silent
   (ZLOGD only), instant, no kernel contact. And `GetOsAccountProxy`
   returns proxy_ whenever non-null — a DEATH-MARKED proxy is returned
   FOREVER (the death recipient only clears on an actual death
   notification; a failed send's SetObjectDied(true) is never undone).
   So ONE earlier send to the osaccount handle that returned
   ERR_DEAD_OBJECT (kernel BR_DEAD_REPLY; the mark is also set on any
   send whose status maps DEAD) poisons EVERY later osaccount query in
   foundation for the whole boot — CheckUserParam always fails, StartUser
   always returns 2097193, the launcher NEVER spawns.
4. **Why the window looked "silent for 160s"**: the AMS_KEYS "SendRequest"
   matched SA_CLIENT's "SubscribeSystemAbility SendRequest succeed!"
   flood — budget [599] exhausted at 125.6s on lb46, dark from 125s to
   the end; lb44's AMS46-T sys=16 snapshots are all AFTER the fail
   (foundation's main looper parked in BWR — its normal idle posture).
5. **The open kernel question (lb47 hunts this)**: WHICH send failed and
   WHY — our kernel returned BR_DEAD_REPLY/BR_FAILED_REPLY to some
   foundation->accountmgr(-osaccount) handle earlier in the boot. All
   reason prints were pr_info (invisible at LOGLEVEL=6) or
   launcher-family-gated (app_txn_fail_print). Candidates: ref lost
   from proc.refs (invalid-handle), node wrongly marked dead
   (child-node-dead), owner seen dead. New taps (3ad7c3e7):
   AMS46-SENDFAIL always-on WARN (pid tid code handle BR reason,
   budget 400, reason shadow 1-18) + de-flooded AMS keys
   ('failed, handle:' = IPCObjectProxy's error verdict, 'invoker is
   null', 'Invalid samgr stub', 'SendRequestInner cost').
6. OH-source reference points: SendRequestInner early-dead return at
   foundation/communication/ipc/ipc/native/src/core/framework/source/
   ipc_object_proxy.cpp:218-247; the poison-cache at
   base/account/os_account/frameworks/osaccount/core/src/os_account.cpp
   GetOsAccountProxy (proxy_ != nullptr fast path) + IsOsAccountExists
   (isExist=false preset); AMS handler at
   foundation/ability/ability_runtime/services/abilitymgr/src/
   ability_manager_service.cpp:9596 (StartUser -> CheckUserParam ->
   OnStartUserDone on ANY failure).

## PREEMPT-HUNT SESSION (2026-11-01 02:00-07:00, branch preempt-hunt @ 1607f62e+)
The phantom preempt-count producer is NAMED: two faces, one dominant.

### Face A (DOMINANT, killed): the preempt-disabled SPIN-WAIT convoy
1. Instrument chain (commits 88dd4b82→bd4410ff): outermost 0->1 acquire
   shadow + exact caller-site attribution (#[track_caller] threaded
   through Spinlock/RwSpinlock acquire paths via preempt_count_add_at)
   + live ACQUIRE-DIVERGENT report + multiplicative slot hash (the old
   XOR mixes aliased task structs 0x1000000/0x2080000 apart — ph2's 40
   "splits" were hash collisions; ph3 with the Fibonacci hash: 1).
2. ph2 evidence (system-lb.img + userdata-fresh2.img, dfx=leakhunt,
   wwcensus, debug kernel, 1200s): 320 tick-heals, 109 underflows, 321
   strands. The >2s "phantom" incumbents name SHORT-section locks
   (dentry children/vfsmount, unix dgram_peer/recv_queue, inode icache,
   mm vma_files) — because preempt_disable runs BEFORE the TTAS spin,
   a waiter's whole 0->1 booking covers the SPIN. Behind any holder
   running a multi-second section (unix stream drain memcpy under
   recv_queue, build_path under an fs_struct read guard, mm unmap under
   the pte lock — the rmap_alt_remove rip), every waiter monopolized
   its own CPU preempt-off for the whole hold: the pc=1 WAKE-STRANDED
   incumbents, the 2s-heal stripping LIVE spin bookings, the post-heal
   SpinlockGuard::drop underflows (31+/boot), the kpreempt deadlock.
3. FIX (5f0537a8): Spinlock::lock and RwSpinlock::read/write now wait
   PREEMPTIBLY at the outermost depth in task context (TTAS with
   preempt on, disable only around the acquiring CAS, schedule() to the
   holder after a 20k budget); nested/IRQ callers keep the legacy
   preempt-off spin (mid-nesting schedule = the LTP r2 WEDGE family).
   RwSpinlock::read waits on has_writer() only. Also fixes the if-let
   temporary-extension guard in FsStruct::get_cwd/get_root (the #1
   origin family, 46 lines/boot). Holder side: unix stream drain now
   copies OUTSIDE recv_queue (1607f62e; the memcpy was a named >2s
   origin and user-page faults nested the fault path under the lock).
4. VERIFICATION (ph3 vs ph2, full 1200s boots): tick-heals 320->171
   (-47%), strands 321->221 (-31%), underflows 109->96, panics 6->2,
   SLOW-TXN median exec 5780ms->2250ms (count 32->57: fewer forever-
   wedged txns means more complete past the threshold). make test
   riscv64 GREEN (1198/0); x3 builds rc=0 (x86_64/riscv64/aarch64).
   ph4 (drain split + live divergence) REGRESSED vs ph3 (heals 281,
   WAKE-HEAL 26, underflow 136, SLOW-TXN med 7970ms) AND carried an
   exact-fit semantic divergence (it popped one segment beyond the
   buffer, draining its fds without delivering its data) — REVERTED in
   bf53ba41. The holder-side drain bounding needs a per-segment
   pop/drop/relock shape (no heap Vec, no over-pop), not the batch
   shape; future work. ph3's numbers are the branch's verification.

### Face B (residual, ~4 user-return phantoms/boot): the lying sched slot
- The definitive phantom: LEAK-ORIGIN user-return pc=1 (a +1 at user
  return can never be a live guard). Exact osites: dentry.rs:370/395
  (vfsmount/mountpoint locks), always omis=1 — the acquire ran while
  tp named the victim but the CPU's sched slot named ANOTHER task
  (oslot recorded; live ACQUIRE-DIVERGENT lines show slotpid).
- ph4 live data: divergent acquires spread across MANY short-section
  sites (dentry/unix/syscall) — the producer is the SLOT lying (naming
  a task that is not executing), not any one lock. Windows that leave
  slot != tp: pick-publish (sched.rs:2515) -> tp-swap tail on prev;
  the R56c lying-idle-slot heal; abort paths repair via
  republish_current_after_abort (verified present). NEXT HUNT: freeze
  on a live divergent acquire with slotpid, walk WHICH machinery wrote
  that slot last (slot_tag_write tags exist: SLOT_W_SCHED_PICK,
  SLOT_W_R56C — add a tag read to the divergence report).
- WAKE-HEAL/CLAIM-SWEEP/2s-tick-heal remain symptom treatment for this
  face; they are what converts a lying-slot window into a stripped
  guard + underflow.

### kpreempt=1 verdict (stretch NOT taken)
Still blocked: 171 tick-heals/boot remain (mm unmap loops under the pte
lock are the biggest residue — the rmap_alt_remove incumbent family).
Every heal strips a possibly-live guard; with the IRQ-exit hook armed
that means preempting a holder mid-critical-section = the lb19b
DEADLOCK shape. Prerequisite unchanged: bound the unmap-loop holds
(drop/reacquire the pte lock every N pages) or make the heal holder-
aware. The convoy fix removed the WAITER-side blockers only.

### Boots this session
ph1 (tap v1, last-wins sites, 374 user-return lines all pid 568 — the
  rate gate + exact sites landed after), ph2 (exact sites: the site
  distribution table), ph3 (convoy fix: the -47%/-31% numbers),
  ph4 (drain split + ACQUIRE-DIVERGENT live).

### FINAL TREE VERDICT (14d04456, pushed)
ph6 (record-only divergence, 900s, same recipe): the best boot of the
session — at the matched <=600s window vs ph2/ph3: PREEMPT-HEAL
187->114->38, UNDERFLOW 53->57->25, WAKE-STRANDED 233->138->31,
DEADLOCK 0, SLOW-TXN med 1970ms (n=15, full window), appspawn at 5.2s,
serial live to 900s. ph5/ph5b (the two boots carrying the live
ACQUIRE-DIVERGENT print) BOTH wedged on net::unix::UNIX_TABLE
(both-CPU DEADLOCK, stranded-0x10000 carriers) — print disabled
(record-only) in 14d04456; correlation-by-elimination, mechanism
unproven. make test on the final tree GREEN (ALL TESTS PASSED); x3
builds rc=0. The ph5/ph5b UNIX_TABLE wedge shape matches the pre-existing
lb19b/s14 stranded-0x10000 family — the next hunt's Face C candidate.

## EIGHTH-SUCCESSOR SESSION (2026-11-01, branch oh-ladder @ 53f4fd47)
Mission: re-measure SLOW-TXN/watchdog with the preempt-hunt spinlock fix on
main, chase the launcher ladder. Rig findings first (both boots main @ 4a757d6e,
system-lb2.img + userdata-fresh2.img + dfx=ams46,txnhunt LOGLEVEL=6 SMP=2):

### WEDGE CHECK (coordinator question answered): current main still wedges, BOTH profiles
- lb55 (DEBUG main): SA inserts stop at 16 (24.1s); first strand 12.4s at 38.1s;
  59 DEADLOCK prints from 83.7s, ALL on net::unix::UNIX_TABLE (static lock,
  nm-verified). WAKE-STRANDED up to 133s (pid 523) behind incumbents
  appspawn(335)+ueventd(306), both pc=1; earlier incumbents at pc=0x101/0x102
  (SOFTIRQ bit + 1-2 — softirq-context monopolists, NOT 260-deep preempt
  stacks: pc=258 = 0x102). KOT-KILL small ones only; accountmgr never loaded.
  Killed at 747s; screen md5 82f0a25f (frozen logo class).
- lb56 (RELEASE main): SA inserts stop at 17 (17.8s); 105 DEADLOCK prints from
  6.3s — binder::WORLD x29 + UNIX_TABLE x79 (+1 beside sys_openat). Same
  family as lb48/50/51/52. Killed at 437s.
- Conclusion: the preempt-hunt spinlock fix did NOT cure the post-merge
  wedge; lb47b (debug clean) now looks like the coin flip, not a
  debug-vs-release distinction. The preempt-hunt lane's own ph5/ph5b
  boots (TASK tail) ALSO wedged on UNIX_TABLE — the wedge predates the
  spinlock fix. Wedge-fix lane's suspect set stands (977adb51/e1e7d8f9...).
- NOTE for wedge hunters: lb55/56 DEADLOCK ra resolves inside
  RawSpinlock::lock itself (deadlock_warn inlined; caller_ra = return
  address after the warn call) — the LOCK address is the informative
  field, not ra.

### The ladder rig (built this session)
- Branch oh-ladder = origin/evidence-premerge + cherry-picked 5f0537a8
  (sync: bound the preempt-disabled spin in lock/read/write) = 53f4fd47.
  Conflicts were comment-only + the preempt_disable_at site-shadow call
  (leakorigin hook absent on this base) — resolved to plain preempt_disable.
  bzImage saved: run/bzImage-ladder-53f4fd47 (release, 12.9MB).
