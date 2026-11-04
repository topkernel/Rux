# arch-s1 lane: S-1 model-checking pilot (Kani/loom on kernel concurrency primitives)

## Charter (from the survey shortlist, docs/architecture/os-kernel-survey.md S-1; user go 2026-11-03)
Pilot, 1-2 sessions: evaluate whether Kani (verification) and loom
(concurrency model checking) can run against Rux kernel concurrency
primitives in a HOST-side harness, and whether they pay. This is the
methodology adoption the survey recommends (seL4-lineage verification
methods applied to Rust-native primitives).

## Known constraint going in
The kernel crate cannot link std (no_std; unit tests collide with
panic_impl — recorded house lesson). loom/kani need host builds. The
pilot's core question is EXTRACTION cost: can the primitive's logic be
built for the host (cfg-gated, or extracted to a small inner crate)
without semantic drift?

## Targets (in order)
1. kernel/src/sync/mutex.rs SleepMutex (the R84 handoff state machine:
   waiters/tokens/anti-starve — historically bug-dense, now the
   canonical primitive).
2. kernel/src/sync/spinlock.rs + the preempt-count interplay (if time).
3. The futex wait-queue key resolution (physical-word keys from the
   LTP fix) if S-1 finishes early.

## Deliverables
- A host-side harness (kani/loom configs in-repo, English comments)
  with the extraction pattern documented.
- Verdict per target: invariants checked, bugs found (loom explores
  interleavings; kani proves bounds/overflow), false-positive load,
  runtime cost, and whether the extraction changed the code shape.
- A do/don't recommendation for CI integration (GitHub smoke.yml runs
  kani on nightly already for some crates — check .github/workflows).
- NO kernel-behavior changes: extraction shims must be cfg(test)-like
  and default-off; if a real bug is FOUND, fix it through the normal
  lane discipline (separate commit, gates).

## Session log (append per session)

### 2026-11-02, round 1 — rs suite ported, 2 kernel bug families fixed, 2 stressors re-run clean

**Ported** (stress-ng stressor logic as standalone static musl probes,
per charter method; sources archived in userspace/tests/probes.tar.gz
per the 394c16ce policy, host driver test/stressng/run-class-x86.sh):
14 classes — malloc/mmap/bigheap (memory churn), fork/exec, fsmeta
(metadata storm with read-back verify), pipe/sock/sem/msg/shm (IPC),
futex/sched (wakeup storms), timer (create/settime/delete + CLOCK_
MONOTONIC discipline + nanosleep latency), plus --spawn passthrough
for minimal reproducers. Vehicle: the x86_64+KVM LTP rootfs (copy of
the permanent-store image in run/, rs/rsrun/renameco injected via
debugfs); PID-1 rsrun runs cmds.txt lines with per-class budgets.

**Round-1 campaign** (baseline 819a1be2, one family per boot,
dfx=allocwatch,taskdump,watchdog + syscost=1, logs in
run/stress-runs/): ipc / sched / timer / memory PASS (0 panics, 0
wedges, 0 probe errors); fsmeta FAIL (SP-2); forker survived but lit
up the SP-1 storm. Verdicts + census numbers: run/stress-runs/
findings.md.

**SP-1 (fixed, verified)**: PREEMPT-HEAL false-fires stripped live
guard bits of fresh fork/exec tasks (15,577 heals + 169 underflows per
120 s of fork churn). Two producers: (1) stale_200's j==0 arm treated
never-stamped fields as instantly stale — fixed with arm-on-first-
observation (sched.rs); (2) new_task_at did not zero the four
ti_pc_*_zero_jif stamps on recycled Task pages — same class as R68a
(task.rs). Verification boot (fork 8 + exec 4, full 100 s budgets):
**0 heals, 0 underflows, 0 panics** (was 15,577/169/0).

**SP-2 (fixed, verified)**: ext4 setattr corrupted freshly-written
files — the success tail ran page-cache invalidate_inode for EVERY
attr flavor (chmod dropped dirty unflushed pages) and metadata-only
setattrs wrote back the disk-read inode's stale i_size (0 for pending
writeback). renameco minimal reproducer isolated chmod (3/300 fails,
reads returned EOF; rename alone clean). Fix: invalidate gated to
ATTR_SIZE, metadata setattrs carry the live VFS size (ext4/mod.rs).
Verification boot: renameco 0/300 on every sequence; fsmeta 4-worker
class full 120 s rc=0 (was: all 4 workers corrupt on cycle 1).

**Driver bug found+fixed in-flight**: rsrun killed classes at an
absolute 60 s (RES rc=137 truncation in the first campaign's memory/
forker boots); now parses --timeout per class.

**Tier-1-under-load evidence** (syscost, memory boot): IF=0 cpu0
17.6 s/60 s (rest 17.2 s, hard 408 ms, uart 3 ms), top rest-site
sched.rs:2568 (37.4M ops avg 834 ns); no ALLOCTHROW / DEADLOCK /
KERNPANIC classes in any boot. Follow-up hunts queued: IRQ-OFF-WINDOW
idle-wake prints 65-142 ms under load (pre-existing, guarded domain
interrupts); WAKE-STRANDED 118 s face under memory churn (SP-1
family; re-check on fixed kernel in round 2).

**Round 2 queue**: dirstorm-style shared-tree churn + fsync pressure;
pipe/socketpair under CPU contention (2 vCPU); LTP robustness subset
long-loop (fork12, mmapstress, mmap-corruption); brk-churn probe via
glibc-static (musl pins brk); make test + full LTP spot-scan on the
fixed kernel to guard the setattr/preempt changes.

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

### lb57 (ladder rig, release+spinlock fix): the healthiest fail ever — POISON EVENT CAPTURED
- SA storm COMPLETE (78 inserts, 0 DEADLOCK), accountmgr alive ALL BOOT (SA:200
  @174.3, NO group-suicide — KOT-KILL lines 173 -> 35 vs lb54), CreateNewUser
  222.9->262.9 end, lockstatus_100 SET 263.1. SLOW-TXN n=27 p50=27.5s max=72.9s
  (variance-dominated; still the samgr code-7 30s family).
- 263.218: same one-shot fail as lb54 — BUT WITHOUT any watchdog kill or restart:
  startUser -> "userId not exist" (2097193) 0ms apart, zero kernel traffic.
- THE POISON MOMENT (bracketed to the millisecond): 263.000/263.001
  `IPCObjectProxy PrintErrorDetailedInfo: failed, handle:26 error:32
  desc:*.IOsAccount proxy:4920816` x2 — error 32 IS ERR_DEAD_OBJECT
  (c_utils errors.h: BASE_ERR_OFFSET+EPIPE). SendRequestInner marks the proxy
  dead (`status == ERR_DEAD_OBJECT -> SetObjectDied(true)`) AT THIS PRINT —
  every later osaccount query (CheckUserParam at 263.218 included) fails
  instantly and silently. THE PERMANENT MARK IS MADE HERE, NOT BY A DEATH
  NOTICE: no death notice fired for it (DRS/notice net silent), accountmgr
  alive, and the kernel SENDFAIL net did NOT fire -> the send returned +32
  WITHOUT any kernel error verdict.
- SOURCE AUDIT (oh-robot, this image's tree): the ONLY way a sync send returns
  +32 with zero kernel traffic is HandleReply's TF_STATUS_CODE leg
  (binder_invoker.cpp:1026): a BR_REPLY whose flags carry TF_STATUS_CODE (0x08)
  has its FIRST INT32 read as the send status. Servers author that shape via
  BinderInvoker::SendReply -> WriteTransaction (binder_invoker.cpp:1241):
  an error return with an EMPTY reply parcel becomes TF_STATUS_CODE + errcode.
  => A TRANSIENT server-side error 32 from accountmgr's osaccount stub
  PERMANENTLY death-marks foundation's IOsAccount proxy. Kernel BR_DEAD_REPLY
  (raw 0x7205) does NOT mark (verified against this tree's invoker source);
  kernel ioctls return -errno, also not +32.
- Our kernel relays reply flags verbatim (construction flags:tr.flags,
  delivery flags:t.flags) and never fabricates status-code replies — the 32
  is authored by the SERVER userspace. TAP LANDED (oh-ladder @ +1):
  STATUSCODE-REPLY WARN on every BC_REPLY with TF_STATUS_CODE + nonzero
  status (names server pid/tid, code, poison value). lb58 runs it.
- OH-side: the fix belongs upstream (a stub error must not equal ERR_DEAD_
  OBJECT, or SendRequestInner must not treat HandleReply status as liveness);
  kernel-side there is nothing to fix on this leg — but the SERVER error's
  own origin (why accountmgr's osaccount handler returns 32 under load)
  is the next hop, and THAT one is perf-family.

### lb58: a REAL KERNEL BUG found and fixed — pcp refill race (ON MAIN TOO)
- lb58 (ladder rig + first STATUSCODE tap) panicked at 133.8s:
  `PANIC: index out of bounds: the len is 32 but the index is 32
   Location: kernel/src/mm/pcp.rs:153` under net-softirq alloc pressure
  (stack: ipv4 defrag dispatch -> unix registry path). Console dead after.
- Root cause: alloc_order0's miss check and its refill STORE run in two
  SEPARATE irq-off windows; a nested irq-context alloc between them
  refills the row on its own, so row.count is not the zero seen at the
  miss — the unconditional store then indexed past list.len() (PCP_HIGH
  = 32). Classic check-then-act across critical sections.
- FIX (oh-ladder): cap the stored frames at remaining-room-minus-one
  (never park count exactly ON PCP_HIGH — that would arm the next free
  push), surplus batch frames returned via the zone free path
  (INV-PCP-2 net-zero preserved). Same file is IDENTICAL on current
  main (md5-verified) — coordinator should cherry-pick the oh-ladder
  commit to main (mm guarded domain; needs its perf-gate rows).
- Also on lb58: the first tap iteration burned its 200 budget on
  samgr's boot-time status-7 storm in ~3s (samgr answers hundreds of
  GetSystemAbility-null ERR_NULL_OBJECT status replies while SAs load);
  budgets split now (poison-32 gets its own 400).
- Poison(32) sightings on lb58 BEFORE any osaccount event: samgr itself
  sent a status-32 POISON reply at 4.342 (tid 332), and device_manager's
  IOsAccount proxy (handle:3) was marked at 133.17 right after an
  accountmgr reply. The osaccount 32-authors are multiple processes.

## EIGHTH-SUCCESSOR STATUS SNAPSHOT (for the next successor / coordinator)
1. RIG: use oh-ladder (evidence-premerge + 5f0537a8 + taps + pcp fix),
   release bzImages saved in run/ (bzImage-ladder-*). main release AND
   debug both stall/wedge on the OH image (lb55/lb56) — UNIX_TABLE /
   binder::WORLD DEADLOCK family, pre-existing, NOT cured by the
   spinlock fix.
2. lb57 (the healthy-boot shape): accountmgr SURVIVES with the spinlock
   fix (no watchdog suicide); the ladder now dies ONLY at the osaccount
   proxy poison: a TF_STATUS_CODE reply carrying 32 from the SERVER
   side death-marks the caller's IOsAccount proxy permanently
   (SendRequestInner marks on status==32; verified in the image's own
   ipc source). Server-side authors are OH userspace (kernel relays
   reply flags verbatim; kernel BR_DEAD_REPLY/errno never produce +32).
3. NEXT: lb59 (sctap2+pcpfix kernel) — read STATUSCODE-REPLY POISON
   lines at the ladder moment: names the exact server pid/tid and time;
   correlate with accountmgr hilog to find which osaccount handler
   returns ERR_DEAD_OBJECT (OH bug: transient server error -> permanent
   client proxy death). If the server-side 32 is rare on a healthy
   boot, the ladder may complete by luck: watch for appspawn
   com.ohos.launcher + pixel md5 change; report IMMEDIATELY.

### lb59 — THE LADDER'S REAL ENGINE DECODED (SwitchToUser reached; silent empty user)
With the pcp fix + tap kernel (healthy boot, 76 SAs, 0 DEADLOCK/panic):
- LADDER ADVANCED PAST THE OLD WALL: CheckUserParam PASSED (no osaccount
  poison at the decision on this boot), foregroundUserId printed,
  SwitchToUser(old=0,new=100,isAppRecovery=1) EXECUTED at 251.6 —
  further than any boot in campaign history.
- Then the lb40 shape live: `ImplicitQueryInfoByPriority: can't find
  target AbilityInfo` x154 + `waiting query highest priority ability
  info completed` x151, spinning from 251.6s past 900s (no exit).
- ENGINE (source + log bracketed):
  1. CreateNewUser(100) start 189.054, end 218.643 (29.6s) — and the
     binder call itself is SLOW-TXN txn14922 accountmgr->foundation
     exec 29610ms.
  2. The handler NEVER printed `create user:100 bundle size:N`
     (LOG_NOFUNC_I INFO — INFO lines DO flow through the hilog tap:
     "startUser in service" I0 came through; key 'bundle size:' is in
     AMS_KEYS and the budget was alive at [274]) → the line never ran.
  3. OH source (bundle_user_mgr_host_impl.cpp): OnCreateNewUser's
     early leg — if GetAllPreInstallBundleInfos(...) returns FALSE:
     LOGE + `return ERR_OK` — CreateNewUser still prints "end" SUCCESS.
     That function's ONLY other false: `return !preInstallBundleInfos
     .empty()` — the filtered preinstall set came out EMPTY.
  4. The set comes from dataMgr->GetAllPreInstallBundleInfos() ->
     LoadAllPreInstallBundleInfos -> preInstallDataStorage_ (the
     distributeddata KVS). bootevent.bms.main.bundles.ready only set at
     217.2s — the first-boot scan was still running when CreateNewUser
     read the store. Under our kernel the KVS read inside the 29.6s
     exec never produced records (the ~30s exec = the KVS timeout
     class; 5th successor's distributeddata KV coin-flip, same family).
  => USER 100 IS CREATED WITH ZERO BUNDLE RECORDS. The launcher's
     AbilityInfo cannot exist for user 100; the AMS isBoot retry ladder
     (no attempt limit) spins forever. Screen stays frozen logo.
  5. Image-side verified (debugfs on system-lb2.img + hap unpack):
     Launcher.hap's home ability is an extensionAbility
     (com.ohos.launcher.MainAbility, type service, action.system.home),
     module.json declares NO u1enable -> NOT skipped by the U1 filter
     (U1==1 anyway) — the miss is the EMPTY SET, not the filters.
- PIXEL PLAY (in flight): a WARM /data. Real devices boot this way
  after first provisioning: boot once on a writable copy
  (userdata-warm.img, NOSNAP=1 added to lb-boot.py) so the first-boot
  scan PERSISTS the pre-install records; the next boot's CreateNewUser
  reads a populated local store (fast path, no first-population) ->
  per-user install stream runs -> AbilityInfo for user 100 exists ->
  appspawn spawns com.ohos.launcher -> pixels.
  lbwarm (population boot) running at write time; lb60 = warm+snapshot.
- GATES for the pcp fix + taps: make test ALL PASSED (lb59-maketest.log);
  x86_64 release + riscv64 + aarch64 builds rc=0.

### The 30s CreateNewUser exec — dissected per user directive (bug, not perf)
Two independent samples (lb59 tid 912, lbwarm tid 930 — same window, same
image state) agree exactly:
- The handler parks in sys=202 (futex) with a1=0x80 (FUTEX_WAIT |
  FUTEX_PRIVATE_FLAG — process-INTERNAL sync, NOT a binder reply wait:
  a binder client parks in ioctl sys=16 as the neighboring samples show).
- The uaddr chain is DETERMINISTIC across boots: 0x7fffffffc8f4 -> c844
  -> c654, each held ~10s (2 audit samples) — a CHAIN of three
  sequential ~10s stack-based waits (condvar/future wait_for), NOT one
  5s loop: phase1 c8f4, phase2 c844, phase3 c654, then give-up-empty.
- ZERO installd install-stream traffic in the window (only 2 tiny SACB
  oneways to installs) -> the per-user install loop never ran; the
  early-empty-set leg is confirmed from the kernel side too.
- The same 1Hz audit names the wedged SERVERS concurrent with the
  window: samgr code-7 handlers at 30-55s exec whose server threads sit
  in sys=45 (brk!) for the whole window — an ALLOCATION storm inside
  samgr's handler, plus netmanager->samgr 54.7s. Working hypothesis
  (lb60 proves): foundation's KVS-open needs a worker (FFRT/IPC pool);
  the pool is exhausted (workers parked as sync-IPC CLIENTS against the
  30s samgr handlers); the open never runs; main thread wait_for x3 x10s
  gives up -> silent empty user 100 -> no launcher, forever-spin ladder.
- TAP LANDED (oh-ladder): one-shot dump_all_tasks("txn-hang-20s") when
  any txn's exec crosses 20s — prints EVERY task's state + parked
  syscall mid-hang. lb60 = fresh-data boot with this kernel
  (bzImage-ladder-hangdump): the dump names the leaf's producer thread.

## TENTH-SUCCESSOR SESSION (2026-11-01 18:00+, branch oh-ladder @ af5577c5)
Mission: verify the poison-patched image end-to-end. The predecessor's
lbpop2 (population, NOSNAP) + lb64/lb65 (snapshot boots, system-lb3.img +
userdata-warm.img) got FURTHER THAN EVER — and then froze on a NAMED engine.

### lb64/lb65 decode (evidence: oh-lb64/65-serial.log + task dumps)
1. The empty-bundle wall is dead and the poison ladder advanced:
   lb65 `CreateNewUser user(100) start` @40.2 -> `create user:100 bundle
   size:34` — the per-user install stream RAN (lb64: SetBundleQuota for
   /data/app/el2/100/base/com.ohos.launcher @119.78s; hmdfs/sharefs
   launcher mkdirfails @119.3 = install stream touching the bundle).
   systemui + render_service (llvmpipe/CompThread) SPAWNED on lb64/65 —
   the GUI stack is loading.
2. THE WALL: phantom-preempt monopolists own every CPU and the tick-side
   PREEMPT-HEAL never fires (ZERO PREEMPT-HEAL lines on both boots; lb59
   pre-contiguity-bar fired 1784). lb65 dump @64.5s: cpu slots 1232
   (m.ohos.systemui, preempt=0x10000) / 1067 (render_service watchdog,
   0x100) / 319 / 1177 (tgid 313 OS_IPC/FFRT, 0x10000) — all four busy,
   nr_running=165; installs tid 1138 state=RUNNING on_rq=1 NEVER picked
   (its mkdir = txn13201 exec 2350s; the CreateNewUser txn9711 itself
   2355s). lb64: systemui main wedged in exit_group (lastsys=231) at
   0x100 from ~118s through kill @660s.
3. Root cause: d2381bf9's contiguity bar re-arms the heal window on any
   completed PREEMPT pair — correct for the base field, but it shields
   stranded SOFTIRQ/HARDIRQ bits on lock-hot carriers (OS_IPC threads
   live in spinlock loops), AND the pure-field faces (base==0) were
   never reachable by the base!=0 gate (keep_tick_nesting existed but
   had NO call site). preemptible()==0 for ANY bit, so these monopolize
   exactly like the base face.
4. FIX (af5577c5): per-field contiguity — Task ti_pc_sirq_zero_jif /
   ti_pc_hirq_zero_jif stamped whenever the tick observes that field
   ABSENT; a field present at 200 consecutive ticks is phantom (lock
   traffic never touches these fields). Stale field -> skip the re-arm
   -> window matures -> new heal leg (base==0): keep_tick_nesting +
   need_resched + PREEMPT-HEAL-FIELD print with softirq/hardirq origin
   sites; the base leg now collapses stale hardirq extras too.
   Base-face (pc=1) policy deliberately untouched.
5. Build note for successors: `build-release` does NOT regen ksyms
   (only the debug `build` target does). A release build after a
   `make build` for ANOTHER arch embeds wrong symbols. Two-pass release:
   cargo build --release; gen-ksyms.py <elf> x86_64-unknown-none
   kernel/src/dfx/ksyms_generated.rs; cargo build --release; mkbzimage.
   LTO byte-loop-transforms the tick prints into immediate stores —
   `strings` on the release ELF is NOT evidence a print was stripped;
   disassemble for the heal's andl sites instead (0xff0000 = clear_base,
   0xf10000 = keep_tick_nesting).
6. lb66 (verification boot): bzImage-ladder-fieldheal + system-lb3.img +
   userdata-warm.img + dfx=ams46,txnhunt, SMP=2. Watch: PREEMPT-HEAL*
   lines (the fix firing), the install stream completing (installs
   code-4 execs bounded), startUser in service:100 -> CheckUserParam
   (poison leg: POISON32-DELIVER then NO marking = patch works) ->
   SwitchToUser -> appspawn com.ohos.launcher -> pixel md5 change
   (frozen-logo set: 5f8b704d/06b933b4/82f0a25f/23312e5b).

### Tenth-successor session close (2026-11-01 ~19:45, branch @ e8b9a969)
1. HEAL STACK, three rounds, honest final form (8387c00b): per-field
   tick-observed absence stamps for ALL THREE count fields
   (ti_pc_{p,s,h}irq_zero_jif); a field PRESENT at 200 consecutive
   ticks is phantom, a field observed ABSENT refreshes its stamp. This
   replaces both the sc42 cumulative window (false-healed live
   softirq windows — round 2's make-test wedge) and d2381bf9's
   PREEMPT-pair re-arm (shielded stranded S/H bits behind lock churn —
   the lb61-lb66 zero-heal freeze). make test GREEN 1267/1267 (the
   VW-WEDGE prints after "All tests passed! Halting..." are POST-HALT
   writeback noise, NOT failures — verify the suite verdict line before
   believing a wedge; 3112b576 also passes, sometimes without the noise).
2. THE 441s INSTALLS STALL, decoded to its wait site (coordinator
   redirect; lb66 tid984 stack, symbolized against the af5577c5
   two-pass rebuild): sys_mkdir -> vfs_mkdir -> ext4_mkdir_wrapper ->
   ext4_journal_stop (the mkdir's journal commit) -> blkdev_write ->
   submit_request -> VirtQueue::wait_for_used_interruptible — the
   POLL path (caller holds a preempt guard: the VFS mutation lock over
   the journal write), 5M laps, NO deadline (poll_only skips the 10s
   deadline entirely!), caller retried for 441s+. NOTE: Rux's internal
   dispatch uses asm-generic numbers: nr 83 = FDATASYNC; the x86 native
   83 = mkdir maps to internal 34 — the lastsys=83 prints on the mkdir
   stacks are the NATIVE number. The wait site is the same either way.
3. Instruments landed for the capture (e8b9a969): VW-POLLSTALL (the
   poller's own ring snapshot at 2M laps: resp byte, used idx,
   prev_used, pc — splits delivery-loss vs kick-loss vs descriptor-
   window in one line) + re-armable txn-hang-20s dump+vw_report (the
   old global one-shot latch spent itself on an early 20s samgr txn
   and went dark at the real hang — lb70 lesson; now once per 600s).
4. OH boots this session (all bzImage + system-lb3.img + userdata-warm,
   dfx=ams46,txnhunt, SMP=2): lb66 (r2 kernel) froze at the monopolist
   wall AGAIN pre-install; lb67 (cumulative heals, 185 heals/70
   underflows) passed the wall, SA storm 88, then the accountmgr
   userspace-futex stall (txn16360 code66, tid977 in FUTEX_WAIT with
   txn16423 QUEUED behind it) — the fifth successor's pool-exhaustion
   family, NOT kernel scheduling; lb70/71 (r3 heals, 34-119 heals,
   28-60 underflows at spinlock.rs:210 — the samgr-brk/mm-unmap real
   2s+ hold false-heal cost) stalled pre-install (lb71: accountmgr
   txn19304 code53 exec 445s). The wall ROTATES: monopolists /
   accountmgr futex / samgr code-7 storms / installs fdatasync —
   boot-to-boot variance decides which one bites.
5. NEXT (in order): (a) roll boots until one reaches the install
   stream with the r3i kernel — VW-POLLSTALL names the fdatasync
   engine directly; the fix is then targeted (candidates: give the
   poll_only path a deadline + kick+walk like the sleep path's; the
   caller retry loop in submit_request; the descriptor window). To
   reproduce lb66's no-heal shape exactly, boot 3112b576 + cherry-pick
   e8b9a969. (b) The accountmgr futex family (tid parked with queued
   txn) is userspace-internal — the samgr/installs 30s chain on the
   campaign map. (c) The r3 heal false-positive cost on REAL 2s+ holds
   (spinlock.rs:210 underflows) wants the leakorigin outermost-age
   discriminator if it grows.
6. Gates this session: x3 builds rc=0 (x86_64 release two-pass +
   riscv64 + aarch64), make test GREEN (lb69-maketest.log: verdict
   green, 1267/1267). Branch pushed through e8b9a969.

## THIRTEENTH-SUCCESSOR SESSION (2026-11-02 03:30-08:30, branches oh-ladder @ 79cb96cb + biofix-fieldpoll @ 6e5afaae)
Mission per charter: population race -> pixels. Coordinator redirected mid-session
(07:00): STOP image patches, hunt the kernel stall. Outcome: THE 550-1100s
STORE-LOAD STALL IS DECODED AND FIXED; a second, OLDER blocker (the allocator
freeze) is now named to its exact victim signature and blocks verification.

### The bio write-plan closure burn (root cause of the crawl's 550-1100s loads)
1. lb229 evidence (252 identical 5s TXN-HANG samples): distributeddata's
   OS_IPC_0_912 frozen at fs::bio::WritePlan::push_entry for the WHOLE 1092s
   window; bundles.ready landed 1091.6s. The kernel stack never changed.
2. Engine: plan_add_constraints (fs/bio.rs) rescans EVERY pending publication
   against the whole plan per round; the plan grows a few blocks/round ->
   thousands of quadratic rounds = 18+ min kernel CPU in ONE fsync/mkdir
   durability point. Every service pays it behind accumulated churn; the KV
   load (many fsyncs) crawls 550-1100s vs ~40s healthy. This is the O(batch^2)
   class the fs lane fixed for jbd2 — the SAME family in the bio WritePlan.
3. Fix iteration 1 (index+worklist closure) ALLOCATED from journal/eviction
   spinlock contexts -> allocator wedges (lb234/235: samgr workers frozen in
   RawVecInner::finish_grow; REVERTED). Fix iteration 2 (79cb96cb, FINAL):
   allocation-free round cap (3 rounds) + conservative linear fallback
   (schedule every record's pres+entries for the device — strict superset,
   same pre->entry phase discipline, one linear pass). Gates: make test ALL
   PASSED; x3 builds rc=0 (x86_64 two-pass release, riscv64, aarch64).
4. Also landed while chasing the dark family: FXDV2 param-arena futex tracer
   ran ALWAYS-ON (500 console prints from do_futex's hot path in the first
   seconds of every boot) — gated behind dfx=futextrace (6e5afaae).

### THE REMAINING BLOCKER (older than this session, now precisely named)
Every boot since ~07:45 wedges before SA inserts (samgr-dark; 10+ consecutive,
all kernel classes/all images; ~50% flowed earlier tonight; load-correlated).
Victim signature EXACT: parking threads sit in unix_write_slice_owned ->
RawVecInner::finish_grow (a HILOG write's allocation), epoll_create1, or
finish_grow directly — i.e. every allocating syscall parks on the small_alloc
class lock held by a thread frozen mid-section. That is the mm guarded-domain
PHANTOM-PREEMPT-INSIDE-ALLOCATOR family (leakhunt's named acquire sites:
small_alloc.rs:283, slab.rs:628, buddy_allocator.rs:857). NOT kernel-class
specific, NOT my bio fix (allocation-free now), NOT FXDV2 (gated, still dark).
This freeze ALSO explains the population boots' bimodality all along.

### Verification state (blocked)
run/verify-roll.sh (bzImage-ladder-fx2fix = fieldpoll+biofix+futexgate,
system-lb5 unmodified, warm snapshot) re-rolls for a boot that reaches the
store-load window: measure bundles.ready vs the 550-1100s baseline; expect
the fast (~40s) class. Kernels in run/: bzImage-ladder-fx2fix (verification),
bzImage-ladder-biofix{,2} (superseded iterations).

### Population-path decodes (committed for the successor, image work stopped)
1. Creation trigger (OH source): accountmgr's OnAddSystemAbility runs
   CreateOsAccount(100) once storage(5003)+AMS(180)+BMS(401) SAs registered;
   subscription via samgr's CheckListenerNotify (late subscribers notified).
2. User-100 resurrection: GetOsAccountInfoById falls back to the
   distributeddata KV when the file is absent — stripping only
   /data/service/el1/public/account is INSUFFICIENT; the store lives at
   /data/service/el1/public/database/os_account_mgr_service/ (discovery boot
   lb216 listed the paths). IsOsAccountExists = presence of
   account/<id>/account_info.json (valid JSON).
3. Race margins (creation start vs bundles.ready): lb59 -28s, lbpop2 -239s,
   lb219 -38s, lb226 -0.4s (read at exec start ~155s loses), lb229 lost by
   12ms (retry budget 900s burned, ready at 1091.616, giveup 1091.604). With
   the bio fix compressing the load, the natural race should win on a
   healthy boot. Image-side: system-lb6/8 (strip stubs), lb9 (condition,
   bypassed by dynamic SA loads), lb10 (ondemand flips), lb11-13 (patched
   libbms retry) — ALL SUPERSEDED, do not boot; system-lb5 remains the
   campaign baseline; lb7 = discovery (prints paths to console).
4. FIRST MOVE when the allocator freeze is fixed: verify the bio fix
   (verify-roll), then the strip-population (system-lb8 recipe: strip account
   dir + os_account_mgr_service dir at 4.3s via sdc stub, NOSNAP fresh warm
   copy) -> creation reads the populated store -> bundle size:34 -> install
   stream persists -> GOLDEN copy -> snapshot ladder -> pixels.

### Boots this session
lb200 (roller aborted), lb210 (strip v1: KV resurrection), lb216 (discovery),
lb217-231 (golden rolls: samgr-dark mix + race losses; lb219/222/223/225/226
created user-100-empty = copies deleted; lb227/229 = the retry-budget
boundaries), lb232-245 (verification attempts: the allocator-freeze dark
streak). Gates: make test ALL PASSED (biofix-maketest-full.log), x3 rc=0.

### Why the heal machinery never fires during the allocator freeze (for the sched lane)
The frozen class-lock holder sits inside lock_irqsave (IF=0 + preempt off):
its OWN CPU's tick cannot fire, so every tick-side observer (PREEMPT-HEAL
per-field stamps, claim-sweep census) is blind to it — ZERO heal lines on
every dark boot while the machine is wedged. Only an observer on the OTHER
CPU (GRQ timeline walk / cross-CPU dump) can see and name the holder. A
holder-naming DFX needs to be cross-CPU by construction; also note the
waiters cannot self-report (printing allocates — they are parked IN the
allocator). Boot-side, the frozen holder's identity is recoverable from a
cross-CPU task dump (the txn-hang dump_all_tasks machinery) — none armed on
the dark boots (the 20s-latch had not fired before the wedge).

## FOURTEENTH-SUCCESSOR SESSION (2026-11-02 10:45-13:00, branch biofix-fieldpoll @ 0508ff20+)
Mission: verify the bio fix end-to-end + hunt the allocator freeze. Outcome:
THREE RIG BUGS and TWO KERNEL PANIC CLASSES fixed; the "freeze" did NOT
reproduce on the quiet host — tonight's reproductions were INSTRUMENT
RACES plus the roller killing healthy boots. The allocator itself
measures at MICROSECOND class. The pixel wall is back to the
store-load/ladder userspace family.

### Rig bugs found (these poisoned the 13th's dark-streak evidence)
1. verify-roll.sh READY grep matched the NEGATIVE "not set" warning line
   (lb246's false READY at 169s = foundation exit; fixed: positive
   marker only).
2. verify-roll.sh never set USERDATA -> every verify boot (lb233-lb246,
   the WHOLE dark streak) mounted the checksum-poisoned userdata-fresh
   symlink: /data mount FAILED (-5, superblock checksum invalid). The
   13th's "freeze" boots were entangled with the no-/data trap (the
   fourth successor's lb16-lb26 lesson). Fixed: USERDATA pin. Baselines
   lb229 etc. were gold-roller reflink copies (vdf mounted rc=0), so the
   550-1100s class numbers stand; the DARK-BOOT diagnosis does not.
3. poproll.sh killed the VM the moment an ALLOC-* print appeared —
   combined with (see below) a racy stamp table, healthy boots were
   murdered at 29-360s and read as "the freeze kills the machine".

### Kernel fixes (committed, pushed; make test green x3; x3 builds rc=0)
1. mm/pcp.rs: free_order0's entry store bound-checked (lb250 PANIC
   "index 32" at 22s: the 35336eb1 refill guard can leave count ==
   PCP_HIGH exactly; a full row + this store = out of bounds. Full row
   now spills oldest batch + sends the incoming frame straight to
   buddy).
2. Print-path PANIC class (lb266 @209s): from_utf8_unchecked on task
   comm (userspace-writable via prctl) panicked inside the KOT-KILL
   group print's Debug fmt ("end byte index 13 not a char boundary")
   during do_exit_group — one print killed the kernel. Fixed: checked
   UTF-8 at all 10 comm sites (exit.rs/signal.rs) + netdev get_name
   (SIOCSIFNAME names). lb250's StrSearcher-shaped trace = same family.
3. mm/small_alloc.rs: both class page-list walks capped at 8192 pages
   (cycle defense; walk_cycles counter). DEFENSIVE — no cycle was ever
   observed (see meter below).

### The allocwatch DFX (dfx=allocwatch) — and the mirage it created then killed
- Final design: DEDICATED per-key slots (site base + class; 24 slots),
  cross-CPU tick scan reports holds >300ms with holder identity/state/
  preempt/ti_cpu/buddy-word + kernel-stack PCs; ALLOC-STUCK lock-word
  detector (200ms); per-class slow-path section meter (max TSC cycles,
  >10ms count, 10s heartbeat).
- Iterations 1-3 used a SHARED 16-slot scan table — two different class
  locks' enters raced the slot choice and MIXED pid/epoch/jif between
  stamps; the mixed stamp survived its exit and read "held 300ms-1s".
  The prints named real pids (os_ds_service) that were NOT stuck.
- The meter is the arbiter: healthy-boot sections measure 76-1500
  CYCLES (<= 0.6us), over10ms=0 all boot. NO slow section ever existed.
- lb253 (the one boot never killed on a print) ran its FULL 1500s
  healthy with /data mounted (userdata-warm2). The 13th's
  finish_grow-park freeze is HOST-LOAD-CORRELATED and did NOT bite in
  single-agent quiet-host conditions (tonight's host: 3 lmbench loops
  from other lanes' worktrees, load 7/32, VM at 199% = both vcpus
  healthy).
- ABBA hypothesis (coordinator ask): STRUCTURALLY ABSENT — small_alloc
  class sections are pure mm (alloc_locked -> buddy only; no
  fs/journal/reclaim calls; buddy sections O(1) helpers, bounded
  loops). The REVERSE nesting EXISTS (bio LRU lock -> Box free ->
  class-lock flush) = latency amplifier only, not a cycle.

### The bio fix's end-to-end measurement (honest numbers)
- lb253 (fx3fix kernel = fieldpoll+biofix+pcp1, system-lb5,
  userdata-warm2, /data rc=0): bundles.ready NEVER SET in 1500s; ladder
  spun (186 waiting-query). The WritePlan burn is GONE (tid877's stack
  = nanosleep/poll retry ladder, userspace-internal), but the window
  did NOT compress on warm2's shape. The 550-1100s "class" measured by
  the 13th was on STRIPPED warm copies — warm2 (unstripped, stale
  user-100) is a DIFFERENT, slower shape. No clean A/B landed yet.
- POPULATION STATE (lb260/lb275, warm.img): startUser at 101s with NO
  CreateNewUser — warm.img ALSO carries a stale user-100 (the ninth's
  NOSNAP lbpop wrote it). Both warm images have the
  user-100-per-user-records question open; the ladder misses
  (ImplicitQueryInfoByPriority) on every boot. PIXELS need userdata
  with NO user-100 + populated store: the strip recipe (image-side,
  FORBIDDEN now) or FRESH provisioning (userdata-fresh2.img — creation
  retry budget vs first-scan; the bio fix should flip it; UNTESTED
  this session).

### Boot inventory this session
lb250 (pcp panic 22s), lb251/252 (dark, poisoned userdata-fresh),
lb253 (fx3fix, warm2, full 1500s, healthy, no ready), lb260 (fx4fix,
warm, ALLOC print @360s = stamp race + roller kill), lb261 (same @29s),
lb262/264 (not-found stamp face), lb263 (fx5fix, stack=creation-sp,
freeze print @301s), lb266 (fx6fix, UTF8 PANIC @209s), lb267 (killed
for fx7), lb268 (fx7fix, 300ms-stamp w/ clean preempt+buddyword ->
meter demanded), lb270 (fx8fix, meter live: 1500 cycles), lb275+
(fx9fix, clean instrument, no-kill roller — ROLLING at close).

### FIFTEENTH-SUCCESSOR ENTRY POINTS (in order)
1. lb275+ verdicts (pop14.log): with the no-kill roller these boots
   LIVE — read bundles.ready timing, the ladder outcome, and any
   ALLOC-* prints (now trustworthy). If the ladder spins on warm.img's
   stale user-100 (expected), switch the roller's USERDATA to
   run/userdata-fresh2.img (fresh provisioning — the kernel-only pixel
   path) and watch the creation-vs-ready race.
2. The store-load window: instrument-decode WHERE the 1500s+ goes on
   warm2 (lb253's tid877 nanosleep ladder — the distributeddata retry
   chain; SLOW-TXN census on a live boot names the servers in its
   wait chain). This is the bio-fix A/B that never landed.
3. The 13th's freeze: reproduce by LOADING the host (the correlation
   is the clue) with allocwatch armed — the fixed-slot instrument + the
   meter will now name a real holder (if any) vs host starvation. If
   NO stamp ever fires under load, the freeze is vcpu starvation or a
   stuck word (ALLOC-STUCK) — the mm fix follows from which.
4. Gates reminder: everything is on origin/biofix-fieldpoll; the mm
   guarded-domain rows (pcp, small_alloc) need syscost/pbench numbers
   before main-merge; the UTF8 fix is print-path-only (no gate).

### FOURTEENTH-SUCCESSOR ADDENDUM — the coordinator's ext4-biglock steer, decoded
1. (a) THE HOLD HYPOTHESIS IS DEAD: maxheld instrument (release-side
   max hold + holder) reads **4 jiffies (40 ms)** as the boot maximum —
   NOBODY holds the biglock for 5 s. The waited=500j episodes are QUEUE
   STARVATION: the anti-starve timer (5 s) is the only visible episode
   length. STALL-FREE lines (owner=none, waiters=5) show a FREE lock
   with parked waiters — the handoff wake fires but the woken waiter
   cannot be SCHEDULED: phantom-preempt monopolists own the CPUs (the
   campaign's open producer, Face B). The queue protocol itself is
   sound.
2. FAIR-SPINNER FIX LANDED (real mitigation): task-context preempt-bit
   callers on lock_fair previously degraded to the UNGATED barging
   acquire — a phantom-bit chmod walker (chmod_ros pid 524 storm)
   barged past every parked waiter per op. Now they spin on the GATED
   acquire (2 s BARGE_ESCAPE fallback). Stall count unchanged
   (lb275: 7 vs lb280: 6) because the root is scheduling, not barging.
3. (b) SENDFAIL taps: the storm onset (80.3-81.6 s) shows samgr REPLY
   failures — why=6-reply-empty-stack, why=17-reply-target-gone — then
   why=3-owner-proc-dead (pid 333->handle 23): REAL process deaths
   after the stall era (OH watchdogs firing), so the poison is a MIX:
   real deaths + late-reply/empty-stack on stalled callers.
4. (c) SA:401 (BMS) DID insert (and SA:4010); bundles.ready=0 is the
   BMS scan never completing (the store-load wall), not registration.
5. NET: the poison storm's kernel-side window is the STALL ERA
   (5-78 s) = scheduling starvation under phantom monopolists. The
   producer hunt (r83hunt / preempt bits) remains THE wall; the
   biglock fairness fix removes one amplifier. EXT4-BIGLOCK-HOLDER-STK
   + maxheld instruments are in for the next stall read.

### FOURTEENTH-SUCCESSOR ADDENDUM 2 — monopolist snapshot + the storm's true author
1. Snapshot instrument (fx11/fx12fix): dump_all_tasks at BOTH stall
   variants (free-strand + locked-churn) + the waiter's explicit
   on_rq/preempt/ti_cpu line. lb285 (fx11) ran the CHURN shape
   (maxheld=3j) with NO free-strand — snapshot moved to both branches.
2. lb290 (fx12fix, 445s): the POISON STORM HIT WITH ZERO BIGLOCK STALLS
   — 2372 POISON32 lines, no STALL prints at all. The stall->storm
   causality is NOT universal.
3. THE STORM'S AUTHOR IS NAMED: pid 961 = accountmgr (started 94.5s)
   authored 2389/2407 POISON32-DELIVER, victims = foundation (pid 370,
   tids 988/1203 x~600 each). This is the eighth-successor decode
   exactly: accountmgr's osaccount stub returns error-32 (ERR_DEAD_
   OBJECT as TF_STATUS_CODE), which PERMANENTLY death-marks foundation's
   IOsAccount proxies → every osaccount query fails silently → no
   user-100 creation → no launcher. The 32's own origin = accountmgr's
   internal KV read failing under the store-load wall (the campaign's
   standing userspace-internal timeout family).
4. NET for the launcher: the chain is store-load wall -> accountmgr
   handler errors -> 32-poison of foundation -> creation dead. The
   kernel-side lever remains the STORE-LOAD COST (bio fix removed the
   WritePlan burn; the residual is the distributeddata/samgr retry
   ladder — fifth-successor territory), NOT the biglock (holds are
   3-4j; the fair-spinner fix landed as hygiene).

### FOURTEENTH-SUCCESSOR ADDENDUM 3 — the retry ladder's never-true condition, named
lb253's window (116-146s), kernel-side dependency chain from the 1Hz audit:
1. tid877 (distributeddata OS_IPC_1, pid 873) executes asset_service's
   txn12462 parked in sys=35 NANOSLEEP — the FFRT/DB_TaskExecutor
   wait_for (userspace-internal; the check between sleeps is not
   kernel-visible).
2. The FFRT pool worker tid874 (OS_IPC_0) is parked as a sync-IPC
   CLIENT in sys=16 against USERIAM tid 654, txn10152 code 12, exec
   5530ms -> 10530ms+ (the 30s-class exec family — crypto + syscalls
   under the per-op wall; useriam is the historical watchdog-kill
   victim on earlier boots).
=> NEVER-TRUE CONDITION: tid877's wait_for returns only when a pool
   worker frees; the pool's worker is blocked on useriam's code-12 for
   10-30s+ each; every dependent (asset_service, powermgr, drm_service,
   usb_host — 235+ hung txns each) queues behind distributeddata.
   Combined with the accountmgr 32-poison (addendum 2): accountmgr's
   own KV handlers ride the same pool starvation, time out, return 32,
   and permanently death-mark foundation's osaccount proxies.
=> KERNEL LEVER: useriam's exec cost = the per-syscall wall (the perf
   lane's p1_null 4.5us class x millions of calls) — cross-lane. No
   correctness bug in this hop; the kernel-side fixes this session
   (pcp, UTF8, walk bounds, fair-spinner) stand as hygiene + panic
   kills. Biglock stalls: 7 (lb275) -> 0 (lb290) with the fair-spinner
   fix in — recorded, variance-caveated (lb290 had no chmod-era
   contention at all).
lb290 bundles.ready: NEVER fired (87 SAs, storm on) — the store-load
residual is stall-independent, routing through the pool/useriam chain.

### SESSION CLOSE (14th successor, 2026-11-02 ~14:10)
Branch pushed @ 4420bb92+this. Rollers: poproll continues (fx12fix,
snapshot-armed; PIDs owned by lb-boot.py lb29X drivers — kill only
those). Merge queue: coordinator takes biofix-fieldpoll. Successor
entry points in the addenda above + the main close section.

## SIXTEENTH-SUCCESSOR SESSION (2026-11-02 19:30-21:0x, branch biofix-fieldpoll @ c8d1b9f7)
Mission: verify the 15th's binder sync-send parity find end-to-end. Outcome:
THE UPSTREAM DIVERGENCE IS FOUND AND FIXED — the why=7 orphan's root cause is
our NON-DEFERRED sync-send TRANSACTION_COMPLETE; the reference-parity fix
(deferral, 963b3e95) landed, gates green, and lb313 took the fresh-provisioning
boot FURTHER THAN EVER (bundles.ready 238.8s + CreateNewUser raced to a 1s
loss). The race is now a coin-flip, not a wall.

### The root cause (source-verified against the reference tree)
1. REFERENCE SEMANTICS (linux-6.6 binder.c, the reference bzImage is 6.6.101):
   a sync (!oneway, !reply) send queues the sender's tcomplete DEFERRED
   (binder_enqueue_deferred_thread_work_ilocked — NO process_todo), and
   binder_has_work_ilocked consults process_todo (plus looper_need_return /
   eligible proc work) — NOT todo-emptiness. Net: the sync client stays PARKED
   INSIDE its BWR ioctl until the reply (or error resolution) flips
   process_todo. The ioctl cannot return in between: the kernel itself makes
   send-over-send from one thread impossible. Reply delivery then returns
   [BR_TRANSACTION_COMPLETE, BR_REPLY] together. OH's WaitForCompletion loop
   is built for exactly this.
2. OUR divergence: we pushed the tcomplete with process_todo=true — the read
   returned immediately, userspace regained control between send and reply,
   and OH (written against reference semantics) drove the shapes the
   reference structurally forbids: RSS tid458 resent LOAD_SYSTEM_ABILITY 1ms
   over undelivered txn15580 (lb311, caught by the guard why=20); samgr
   workers' 60-1900-deep legal-interleave txn stacks (lb308/310 OWE); the
   out-of-order reply pops (reference BUG_ONs at binder_pop_transaction_
   ilocked when top != replied txn — we retain-anywhere, so old replies
   surface mid-nest: lb313 tid981 REPLY_PUSHED(14793) while 14791 sat above).
3. FIX (963b3e95): sync-send tcomplete queued WITHOUT process_todo +
   thread_has_work drops its `!th.todo.is_empty()` clause (audited: every
   other thread-todo push site sets process_todo=true). All error-resolution
   paths (dead_reply_to_sender etc.) already set process_todo → the parked
   client wakes exactly like the reference.

### Verification boots (system-lb5, SMP=2, LOGLEVEL=6, dfx=ams46,txnhunt,syncguard)
- lb311 (guards ON, warm userdata, cg16c): why=20 fired ONCE (176.3s, RSS),
  boot UNWEDGED (the 15th's lb304 wedge fear did not reproduce), startUser
  + SwitchToUser reached at 176.5s, ladder spun on warm's stale user-100
  (known data shape). POISON32 collapsed 2372 (lb290) -> 16.
- lb313 (cg16e = deferral, FRESH userdata-fresh2): healthiest fresh boot ever
  — bootanimation 166s, CreateNewUser(100) START 199.3s, bundles.ready SET
  238.8s, CreateNewUser end 239.77s (lost the race by ~1s: the KVS read gave
  up 67ms after ready — the lb229 12ms-margin family, now compressed from
  700s to ~1s), startUser 240.4s, SwitchToUser 240.5s, then the empty-set
  ladder spin (no per-user records — SetBundleQuota=0). The store-load wall
  (550-1100s class, bio-fix A/B never landed) is GONE on this kernel.
- lb314 (same recipe): the samgr-buffer-exhaustion death spiral (8x why=12,
  428 KOT-KILLs, OS_IPC_0 workers of codec/composer/hdf/accesstoken killed)
  -> init stopAllServices -> DELIBERATE guest sysrq-c panic at 36.9s. A
  pre-existing variance class (lb313 same kernel clean), not deferral-caused.
- lb315 (same recipe): crawling variant (txn2 samgr-internal futex stall at
  7s — the storage_manager->samgr early call), running at close.

### The parity guard verdict (mission question a/b)
- (a) why=20/21 rejections DO replace the why=7 orphan at exactly the rotating
  victim moment — BUT each hit names an OUR-BOOKKEEPING divergence (the
  immediate tcomplete), not reference-illegal userspace: with the deferral
  fix the illegal shapes become structurally unreachable.
- (b) OH userspace on rejection: the RSS hit did NOT wedge (1 rejection, boot
  continued); the lb304-class wedge was the EARLY samgr code-20/3 sends under
  the pre-fix bookkeeping. Post-fix boots (lb313) show ZERO guard prints —
  the reference discipline holds without rejections, exactly like the
  reference kernel.
- Guards stay dfx=syncguard-gated (default off) until the coordinator decides;
  syncwatch (log-only companion) landed for divergence hunting (55b5f6d6).

### Git reconciliation (c8d1b9f7)
- origin/main merged twice (819a1be2 gap-*; 394c16ce test-archive rule).
- origin/biofix-fieldpoll (the 14th's addendum lineage) merged with ext4/mod.rs
  taken ENTIRELY from main's SleepMutex EXT4_BIG_LOCK — this drops the
  rejected 7954908e fair-spinner content by construction (user directive
  2026-11-02) and its old-lock diagnostics.
- Gates on the final tree: x3 builds rc=0 (x86_64/riscv64/aarch64 — NOTE:
  the aarch64 build rewrites arch/aarch64/dtb_blob.S's .incbin path to this
  worktree; restore it before committing if it shows dirty), make test ALL
  TESTS PASSED (cg16-maketest2/3.log).

### Successor entry points (in order)
1. ROLL THE RACE: system-lb5 + USERDATA=run/userdata-fresh2.img + cg16e-class
   kernel + dfx=ams46,txnhunt,syncguard. The race is now ~1s-class; a boot
   where the KVS read lands AFTER bundles.ready wins: watch "create user:100
   bundle size:34" (NOT the empty-set end) -> SetBundleQuota -> per-user
   install stream -> ImplicitQuery EXITS -> appspawn com.ohos.launcher ->
   PIXELS. Screenshot to run/oh-rux-launcher-*.png, report IMMEDIATELY.
2. If the race keeps losing by ~1s: the CreateNewUser KVS read's 30-40s
   internal timeout (userspace-internal futex wait, the DB_TaskExecutor
   family) is the margin to shave — kernel-side it is the distributeddata
   exec-class residual (bio fix landed; the samgr Executor futex interleave
   at boot, txn2-shaped, is the current visible stall).
3. The OS_IPC_0 worker watchdog-death family (lb314's spiral trigger,
   lb315's audio_host): pre-existing, load-correlated; if a spiral boot
   sysrq-c's again, just reroll.
4. The out-of-order reply-pop decode (lb313 tid981): with the deferral the
   mid-nest reply delivery (reference BUG_ON site) can still occur if a
   parked client receives NEW incoming work before its reply — reference
   delivers BR_TRANSACTION reentrantly there too; if a why=7/-6 family
   resurfaces, RPOP-DIVERGE + the guard hist dumps (006921c1) name it.

### TASK.md note (16th successor, aa43f52e merge)
origin/main's TASK.md became the stress lane's 101-line document (the
coordinator repurposed it). This worktree keeps the oh-launcher lane
history; the merge resolution is ours-by-intent. SP-1/SP-2 intel is in
the stress lane's TASK.md on main.

### SIXTEENTH-SUCCESSOR CLOSE (2026-11-02 22:05-22:10)
1. THE STEER ADJUDICATED (lostpark on a stall boot): lb319 (futextrace+
   lostpark armed) hit the samgr-internal futex deadlock class (samgr main
   tid 325 + Executor tid 345 parked in PRIVATE futexes from ~6s, 630s+).
   lostpark printed ZERO lines all boot — no idle-loop lies, no refused
   wakes through a 10-minute full-system stall. The parked uaddrs
   (0x7ff082aa94394 / 0x7fffffffcab4 — process heap + main-thread stack)
   are NOT the param-arena family (FXDV2 covers 0x7ff0_0000_0000..100000
   only; its 500 budget burns by 5.5s AND its hot-path prints are a
   documented samgr-dark hazard — lb243). VERDICT: the post-creation
   stall web is USERSPACE-INTERNAL with instrument backing; kernel wake/
   park machinery is clean through it. futextrace dropped from rolls.
2. USERIAM (mission leg 3, now adjudicated): the code-12 handler park is
   on EVERY boot's critical path (lb317 596s, lb318 424s, lb320 248s+),
   parked USERSPACE-SIDE (OWE: thtodo=0, no outstanding txn, thread idle
   in BWR read) — useriam's FFRT executor never completes the task
   (task-dump posture: OS_FFRT_2_0 in futex_wait, OS_FFRT_Delay epoll-
   polling). NOT the kernel orphan (that died with the tcomplete fix).
   NEXT INSTRUMENT (successor): a futex-stuck census — stamp park time
   per futex wait, 1Hz report of waits >N s with uaddr/tgid/tid — names
   the exact condvar and its non-running producers; park it next to the
   TXN-HANG census in the same tick.
3. Boot classes this session (cg16e/cg16f, all system-lb5+fresh2,
   dfx=ams46,txnhunt,syncguard): lb311 guards-on warm (1 why=20, boot
   unharmed, POISON32 2372->16), lb312 pre-fix kernel crawl (no ready in
   1355s), lb313 RACE LOST BY ~1s (ready 238.8 vs giveup 239.7),
   lb314/316 samgr-buffer-exhaust watchdog spiral -> init sysrq-c
   (variance), lb315/319 samgr-internal futex deadlock class (6s onset),
   lb317 RACE WON (bundle size:34 @183.4s, install stream ran, ladder
   0-spin — then stalled PRE-startUser on the foundation tid-725 futex
   anchor + 1522 poison at distributeddata), lb318 startUser fired ->
   accountmgr "userId not exist" fast-fail (the KVS coin-flip),
   lb320 late creation (689s, host-loaded) -> empty-set loss, ladder
   spin. lostpark silent on every armed boot; zero PANIC on cg16f.
4. PUSHED through c6659188 (main aa43f52e merged; SP-1/SP-2 in every
   roll since cg16f). Gates: x3 rc=0 + make test ALL PASSED on the
   final tree (cg16-*-build4/maketest4 logs).
5. PIXELS: not yet. The chain now needs ONE boot with: no early spiral +
   race won + accountmgr coin-flip won + post-creation chain unstarved.
   lb317 had 3 of 4. The two standing roots (samgr Executor futex
   interleave; useriam FFRT completion) are userspace-internal under the
   whole-system per-op wall — the perf lane's syscost/pbench territory
   per the merge-gate philosophy, or the futex-stuck census above.

### lb320/lb321 final verdicts (session close 22:27)
- lb320: late creation (689s, host-loaded) -> empty-set loss; ladder spun
  87x then the whole AMS chain stalled (last ImplicitQuery a MISS at
  839s); appspawn alive but idle ("App enable -1" kickdog, no spawn);
  bootanimation.finished fired; screen stayed frozen-logo md5 06b933b4.
- lb321: deep slow class (no bootanimation by 933s under host load 12.9).
- Pattern across the session's rolls: the race needs a quiet-host window
  (the two fastest boots — lb313/317 — ran when load was ~7; every roll
  at load 12+ crawled or spiraled). The successor should check `uptime`
  before rolling and prefer quiet windows.

## SEVENTEENTH-SUCCESSOR SESSION (2026-11-02 23:00-00:0x, branch biofix-fieldpoll @ ca58fc80+)
Mission: sync main (128KB stacks + ext4 size hold), roll the race in quiet windows,
build the 16th's futex-stuck census, re-check the long-handler parks. Outcome:
THE CENSUS IS IN (dfx=fuxstuck, two commits), the cg17 kernel DOUBLED boot speed,
the account side of the race now WINS every roll (startUser+SwitchToUser on all
three boots — first fresh-boot boots ever where creation completed and the account
existed), useriam's 248-596s FFRT parks are GONE, but the BMS per-user install
query is now a STRUCTURAL loss (fires ~73s, data lands ~104s) — pixels still frozen.

### Merge + gates
- ca58fc80 = origin/main merged (c05f13ad 128KB x86_64 stacks, e9029b74 ext4
  published-size hold, SP-1/2/3, sec-r1/compat-r3/review-r3a). TASK.md conflict
  resolved ours-by-intent (lane history; main's copy is the stress lane's).
- Gates: x3 builds rc=0 + make test green (cg17/cg17b/cg17c log sets in run/).
  TRAP: a finished make test leaves its QEMU alive under `timeout 600` holding
  test/rootfs.img's write lock — the NEXT make test fails with "Failed to get
  write lock" until the leftover self-reaps (or you kill your own leftover).

### The futex-stuck census (mission leg 2) — landed, verified on live boots
- 45a3b60b: Waiter::park_j stamped at chain insertion; futex_stuck_census() on
  the 1Hz heartbeat slot next to txn_hang_audit; dfx=fuxstuck-gated; one burst
  per 5s; >=10s threshold; bounded (32 sampled, 8 printed, 1200-line budget).
  Line: uaddr/mm/tgid/tid + park span + anywoken + entry-frame sysno/epc + the
  owning tgid's thread histogram [run/futex/int/unint].
- e60fd08b: fresh-onset bucket — lb323 taught that a pure oldest-8 top was
  monopolised by 155s-class peripheral parks (bluetooth/audio) and NEVER showed
  the critical-path park that decided the race. Now: top-4 oldest anchors +
  top-4 newest-onset groups (FUX-STUCK-FRESH).

### Roll results (system-lb5 + userdata-fresh2 + SMP2 + LOGLEVEL6, host load ~6.6-7.1)
- lb322 (cg17, dfx=ams46,txnhunt,syncguard): bundles.ready 104.5s (lb313:
  238.8s — 2.3x faster), CreateNewUser start 73.1 / end 104.6, startUser
  104.9, SwitchToUser 105.0. ACCOUNT SIDE WON. But the BMS "create user:100
  bundle size:34" print NEVER came -> empty-set ladder spin -> frozen logo.
  POISON32 176 (12 into foundation).
- lb323 (cg17b + census): same shape (72.6 / 104.0 / 104.1). useriam tgid 683:
  ZERO >10s futex parks ALL BOOT — the 248-596s FFRT executor parks are GONE on
  the cg17 kernel (mission leg 3 answered; 128KB stacks + merges correlate).
  Steady state: stuck_total=65 constant, ALL peripheral — bluetooth(385) 6
  threads, audio_server(373) 11 futex+10 int, resource_schedule(451),
  vibrator(567), deviceauth(614), sharing(742), codec_host(558),
  storage_daemon(326) — every tgid run=0. The critical path (foundation /
  distributeddata / accountmgr) is NEVER futex-stuck >10s.
- lb324 (cg17c + slowtask): accountmgr(904) SEGFAULTED at 71.9s (kernel
  USEGV-L forensics printed; NOT the usb_host crash-loop site) -> restarted as
  1105 -> POISON32 1233 (844 into distributeddata(579), 47 into
  foundation(865)). Same race shape (73.4 / 103.9 / 104.1). SLOWTASK in the
  73-104s window: NO fs-family slow entries at all (only idle sys=16
  binder-read parks) — there is NO kernel-side stall in the loss window.

### The race, re-decoded on cg17
1. The trigger: accountmgr's OnAddSystemAbility CreateOsAccount fires once
   storage+AMS+BMS SAs register — cg17's speed moved that from ~183s (cg16f)
   to ~73s. BMS's bundle-data completion (bundles.ready) still lands ~104s.
2. The loss: CreateNewUser start ~73s, its internal data wait resolves at
   ready+0.1-0.6s on every boot (end tracks READY, not start+30 — it waits for
   the data), yet the per-user install-list query STILL returns empty (the
   size print is never reached). Structural ~30s early trigger + a query that
   fails even when resolved late.
2b. SHARPER (lb324 decode): "ready" on cg17 boots is a FAILED-scan
   completion, not data-landing — the el2/0 SetBundleQuota stream NEVER ran
   (0 lines on lb322/324 vs 84 on lb317): BMS's scan dies before the
   per-bundle phase. The failure prints are right there in the window:
   foundation's *.IOsAccount proxies get error:32 AND error:29189, and the
   *.IKvStoreDataService proxy gets error:29201 (the OH KV-store error
   family — service-not-serving/DB-not-ready) at 65-104s. The KVS service
   (distributeddata) is not serving BMS at trigger time; the whole boot's
   SLOWTASK output has ZERO fs-family entries (only sys=16/281/202 idle
   parks) — NO kernel slowpath feeds it. Userspace-internal readiness
   mismatch, instrument-backed on both sides.
3. NOT the poison: lb317 WON with 2034 total POISON32 (zero into foundation);
   lb323 LOST with 16 total (zero into foundation). No correlation.
4. NOT a cg17 regression in the hang shapes: lb323's camera->distributeddata
   code-25 337s hang has its 766s-class analogues on lb317 (cg16f).
5. Kernel-actionable inventory from the census: render_service (tgid 864 on
   lb324) is the ONLY stuck tgid with runnable producers (run=2 futex=12,
   waiter tid 948 parked 263s on uaddr 0x7ff08d38d804) — if that waiter is on
   the pixel path, it is the scheduling-lever candidate. samgr itself parks
   (run=1). Everything else is run=0 deadlocked userspace (no kernel lever).

### Successor entry points (in order)
1. THE 73s-vs-104s GAP: in-window kernel cost is nil (slowtask empty, no
   futex-stuck critical path) — the gap is userspace work (FFRT scan churn).
   Perf lane territory (pbench/syscost on a standard boot), OR find whether OH
   re-triggers CreateNewUser on accountmgr restart AFTER ~105s: lb324's
   restart (71.9s) re-triggered creation but too early; a restart landing
   after 105s should hit a populated store and WIN — cheap to watch on the
   next crash-boot (grep CreateNewUser + accountmgr SIGDEATH ordering).
2. THE ACCOUNTMGR SEGFAULT (lb324 71.9s, DECODED): fault rip=0x7ff085175144,
   cr2=0x7ff20051c6c2 — the faulting address sits INSIDE the 2GB PROT_NONE
   guard map (PROTNONE-MAP 0x7ff0020cb000 len 0x80000000 printed 30ms before,
   serial lines 8271-8283 of run/oh-lb324-serial.log). That is userspace
   dereferencing a failed-map/guard result — the reference kernel faults the
   same way; NOT kernel-actionable on its face (unless the map that produced
   the garbage pointer failed on OUR kernel but succeeds on the reference —
   would need an mmap-failure audit around 71.7s). One crash in three boots.
3. RENDER_SERVICE WAITER (tid 948, uaddr 0x7ff08d38d804): decode whether it is
   the compositor's surface/vsync wait (unblocks with pixels) or a producer
   the pixel path needs (starved signaler — scheduling lever). The census
   prints its park on every fuxstuck boot now.
4. Roll discipline unchanged: quiet windows only (the three boots above all
   ran at load 6.6-7.1 and all reached SwitchToUser — the fastest, healthiest
   fresh-boot class yet). Recipe: system-lb5 + USERDATA=run/userdata-fresh2.img
   + cg17c-class kernel + dfx=ams46,txnhunt,syncguard,fuxstuck.

### lb324 final verdict + session close (00:0x)
- lb324 ran to 1500s: markers = appspawn 5.1s / bootanimation.ready 22.7s /
  finished 32.0s; screen frozen-logo md5 06b933b4 on all 9 shots. Driver and
  QEMU exited clean (no leftovers).
- Session pushed through 070c2218. Branch state: ca58fc80 (main merge) +
  45a3b60b (census) + e60fd08b (fresh bucket) + TASK docs. Gates on the final
  tree: x3 rc=0 + make test green (cg17c logs).
- The three-boot class (lb322/323/324) is the fastest, healthiest fresh-boot
  family of the campaign (all reached SwitchToUser ~105s; lb313's class was
  ~240s). The single remaining blocker for pixels: BMS's KVS-dependent scan
  fails empty when the creation trigger (73s, SA registration) meets a
  not-yet-serving KV store (error 29201 family) — userspace-internal,
  instrument-backed (zero kernel stall in-window), with the successor levers
  named in the entry points above.

## EIGHTEENTH-SUCCESSOR SESSION (2026-11-03 00:17-.., branch biofix-fieldpoll @ 714d767b+)
Mission: sync main (d89cdce4), THE RETRY QUESTION (reference-kernel timeline on
the same image), render_service waiter decode, quiet-window rolls. Outcome:
retry question ANSWERED (reference never needs to retry; the store-serving gap
is the lever), render_service DECODED (victim not lever), a cg18 wedge
root-caused live via QEMU monitor (heartbeat pid-hash self-deadlock) and FIXED,
and lb325 became the FIRST FRESH BOOT EVER to print "create user:100 bundle
size:34" + run the per-user SetBundleQuota stream (the BMS race WON via the
watchdog-restart path) — then wedged at 107s (the fix's target).

### Merge + first roll
- 714d767b = origin/main d89cdce4 merged (LTP r22 census 1042/1869, tmpfs
  MAP_SHARED coherence, shared-futex physical keys, 128KB x86 stacks lineage).
  TASK.md auto-resolved to main's copy (stress charter + our tail — a
  superset, kept as-is; no ours-restore needed this time).
- cg18 build x3 rc=0 (x86_64 first, others before push). lb325 rolled with
  slowtask ADDED to the census recipe (dfx=ams46,txnhunt,syncguard,fuxstuck,
  slowtask + syscost=1) on userdata-fresh2 at load ~7.5.

### THE RETRY QUESTION — answered (mission leg 1)
- Reference kernel, SAME image set incl. userdata-fresh2 (refu9, this
  session): accountmgr 4.3s, distributeddata start 4.9s, BMS scan start
  5.9s, creation (GenerateUserKeys user:100) 8.3s, per-user SetBundleQuota
  el2/100 launcher 8.5s, launcher.ready 11.7s. ZERO PrintErrorDetailedInfo
  lines. Every prior reference boot (refu1 6.4s / refu2 8.2s / refu7 6.5s /
  refu8 11.6s) likewise: creation succeeds FIRST attempt.
- Conclusion: the reference NEVER enters the failure regime — its store
  serves ~1.4s after distributeddata exec, long before the SA-registration
  trigger. There is no reference retry machinery exercised to diverge from.
- Our kernel's restart-retrigger machinery is PROVEN working: lb324's
  accountmgr crash-restart re-triggered creation; lb325's watchdog SIGKILL
  wave (allocator_host/composer_host/foundation/resource_schedule/inputmethod
  all signal:9 at 30-31s) restarted foundation at 32.3s, whose re-run scan
  landed AFTER the store (SA:1301 init finished 46.3s / 31.8s duration) and
  SUCCEEDED: "create user:100 bundle size:34" at 65.5s — the first
  non-empty per-user query on a fresh boot of the campaign — SetBundleQuota
  el2/100 stream 66.5s+, CreateNewUser end 102.5s, startUser 103.1s,
  SwitchToUser 103.1s.
- VERDICT: branch B of the mission — the lever is the STORE'S TIME-TO-SERVE
  (ours ~40-104s class vs reference ~1.4s), a per-op cost problem for the
  perf queue, NOT a retry-semantics divergence. Handoff table below.

### The perf handoff table (lb325 syscost=1 + lb322/323/624 vs refu8/refu9)
- distributeddata SA:1301 init: 28,341 ms (lb322) / 28,761 ms (lb323) /
  31,759 ms (lb325) vs reference max SA init 1,829 ms (memmgrservice; the
  reference prints only 3 init-finished lines total, all <2s).
- ~20 foundation SAs each report 20-22s init, ALL completing at ~40s —
  gated on the store serving (the pile releases the moment SA:1301 lands).
- KVS-family binder txns to distributeddata (lb323 TXN-HANG): 13 distinct
  >5s; max exec 61,570 / 56,570 / 51,570 / 31,570 / 16,570 / 11,570 ms —
  server thread IDLE, reply deferred userspace-side (FFRT pool churn).
- lb325 syscost t=60s: ticks u/k/i c0=1205/12076/247 c1=1266/11307/734 —
  ~90% KERNEL-mode, ~2% idle; IF=0 tot 16.5s/60s cpu0 + 14.1s/60s cpu1
  (max closed seg 7-16ms); rest-sites: percpu.rs:126 n=33.0M avg 216ns,
  slab.rs:640 n=21.1M avg 313ns, sched.rs:2568 n=288k avg 6.5us (!),
  binder.rs:1586 n=259k avg 4.3us; UART if1 n=11581 avg 719us max 10ms
  (70% multi-poll — console writes alone cost 8.3s of cpu1's first 60s at
  loglevel 6); hilogd.server 384,443 console writes in 60s.
- BMS scan: ours 40->64s (restarted foundation; first attempt 40->104s
  fails); reference 5.9->8.5s (2.6s, succeeds).
- SLOWTASK threshold is >=8s-same-entry: per-op costs in the 4ms-8s band
  are invisible to it — that blind band is exactly where the store lives.

### render_service waiter (mission leg 2) — decoded, NOT a lever
- tid 948 (lb324 tgid 864) parks on 0x7ff08d38d804 in TWO long episodes
  (onset ~33s->~80s and ~166s->500s+). Through every census the tgid
  histogram OSCILLATES run=0/1/2 with futex=12<->13, int=5<->6 — the
  "runnable producers" are picked, run, and re-park continuously (classic
  worker-pool churn). A stranded-on-runqueue bug (stress-lane
  WAKE-STRANDED class, 1-2.6s faces) would PIN run>=1 across consecutive
  censuses; it never pins. VERDICT: throughput/dependency-bound VICTIM —
  the compositor waits for surfaces from the launcher that never spawns.
  No scheduling lever; it heals the day the launcher spawns.

### THE lb325 WEDGE — root-caused live, fixed (the session's kernel change)
- At 107.072s (last serial: TXN-HANG txn20418 file-op=sync on
  /mnt/sandbox/system/data/service/el1/public/account/100/account_avatar.tmp,
  server state=0x0) the serial went silent; QEMU burned 198% CPU for 8+
  minutes; screen still frozen-logo md5 06b933b4 (oh-lb325-wedge-shot.ppm).
- QEMU-monitor conviction: BOTH vCPUs at the IDENTICAL RIP inside
  core::sync::atomic::atomic_compare_exchange<u8> with RDI =
  0xffffffff814de05e on BOTH (pid_hash::BUCKET_LOCK+22), RFL=0x97 (IF=0!)
  on both; a full scan of the 256-byte BUCKET_LOCK array found byte 22 the
  ONLY one held. Stack walks: CPU0 = futex_stuck_census -> for_each_task
  -> pid_hash_for_each_task -> lock_bucket(22); CPU1 = dfx::slowtask::audit
  -> for_each_task -> pid_hash_for_each_task -> lock_bucket(22).
- Mechanism (the sc207/sc215 class, re-convicted): both audits run on the
  1Hz timer-softirq heartbeat with IF=0. A task-context pid-hash writer
  (fork insert / exit remove / kill for_each — the install stream at its
  pid-churn peak) was interrupted mid-critical-section holding bucket 22;
  the heartbeat walker on that CPU reached bucket 22 and block-spun; the
  interrupted holder can never resume (its CPU spins non-preemptible), the
  other CPU's heartbeat walker joins — whole box dead, heartbeat included
  (why no census ever printed). stallwatch_tick was already converted to
  the _try walk in sc215 for exactly this; slowtask::audit (lb324 recipe
  added slowtask) and futex_stuck_census (17th) were NOT — arming BOTH
  doubled heartbeat-side blocking walks and the 107s install churn hit the
  window.
- FIX (cg18b): new sched::for_each_task_try wrapper (documents the
  conviction); slowtask::audit, futex_stuck_census phases B1/B2, and
  treehunt::tick (heartbeat member, same hazard when dfx=treehunt is
  armed) switched to it; slowtask reports partial-snapshot rounds (8-line
  cap). Blocking walks remain for preemptible callers (task context,
  khungtaskd) where the holder always resumes. Kernel-semantic change:
  NONE — diagnostics only, default-off switches.
- Per-nr table (lb325 t=60s window): nr=9 mmap n=179,154 avg 72us max 722ms
  tot 12,968ms (3k mmap/s — the FFRT arena churn at 50-70x the reference's
  ~1-5us class); nr=10 mprotect n=31,596 avg 53us tot 1,681ms; nr=7 poll
  avg 1.56s (waits); nr=77 futex avg 15ms (condvar waits). The vm-op family
  (mmap+mprotect: 210k ops, ~15s wall per CPU-minute) is the concrete
  per-op lever candidate behind distributeddata's 27-32s SA init.

## NINETEENTH-SUCCESSOR SESSION (2026-11-03 01:30-.., branch biofix-fieldpoll @ 60e223dd)
Mission: the perf handoff — WHY is mmap 72µs when the reference is 1-5µs? Outcome:
ATTRIBUTED TO ITS LAST LAYER and FIXED (mm guarded domain); the per-op mmap wall
dropped 72->30µs under OH load (fixmun 210->40-45µs, -79%); P8 A/B and the full
stage table below. Same image, same accel (KVM), reference vs ours, same binary
(work/pbench + test/pbench.c P8 case).

### Merge + instrumentation landed (all pushed)
- 3587fd4b = origin/main merged (d2be598d: the dfx-trace lane — tracefs +
  reference-exact set_event grammar; TASK.md resolved ours-by-intent per the
  standing rule; main's copy is the dfx-trace charter).
- f3bf6d8b dfx(mmprof): cmdline `mmprof=1` (forkprof pattern, zero cost when
  off) — TSC stage brackets: mmap {tot, vma_write lockwait, find_free_area,
  VmaManager::add, MAP_FIXED's munmap+install}, munmap {body, surgery, walk,
  tlb, cond_resched yields}, mprotect {tot, lock, coverage+EACCES, pte walk,
  tlb}; scalars {fixed/hint/anon0 placement, unmap pages+empties, mprotect
  pages, vma adds, munmap chunks}; mmap length buckets; per-comm issuer table.
  Dumps from a 60s cadence.
- f1606bb8: scalar-slot namespace fix (the first boot panicked at 1.0s —
  bump() indexed the phase array; bounds-tolerant counters now).
- 1feb7e20 + 60e223dd: console discipline — the printk emit-lock gives
  softirq emitters 100µs while the OH hilog flood holds the console ~17ms
  per 256B line (emulated baud): a 6-line burst lost all but 1-2 lines
  (lb332/333), and even one 60s line raced (lb336). Final form: ONE compact
  line per dump rotating A/B/C, emitted by a task-context kthread "mmprofd"
  (2s emit budget — always gets through).

### THE ATTRIBUTION (the 18th's handoff question, answered layer by layer)
lb335 t=60s (pre-fix, fresh2, syscost window): mmap tot=84µs avg (n=154k,
max 749.9ms); **fixmun=210µs avg (n=51k) carries everything**; the non-fixed
path is CHEAP (vma_write lockwait 0µs at 3k mmap/s — the RwSpinlock TTAS is
NOT the story; find=7µs; add=2µs); mprot-tot=47µs (n=26.6k).
Inside munmap (lb333): body=177µs = **yld 132µs (the per-chunk cond_resched
ACTUALLY SCHEDULES — need_resched is set at ~3.5k chunks/s under the churn,
each yield a full re-queue: the 722ms-class mmap max is a yield that waited
behind the storm) + walk 23µs (a full 4-level PageTableWalker::walk PER PAGE
— 4 dependent memory refs per 4KB) + surg 4µs + tlb 2µs**.
Reference A/B (pbench P8, quiet, same rootfs/binary/KVM): ref 0.22 (mmap0 4k/
128k) / 1.24 (fixed-cold) / 3.42µs (fixed-hot) vs ours 6.10 / 24.4 / 64.3µs;
mprotect 0.51/0.53 vs 3.77/5.84µs. The mission's 50-70x load class = these
19-28x quiet per-op ratios x the yield amplification under load.

### THE FIX (f1daca15, mm guarded domain, reference-shape semantics)
1. unmap_pages: resolve the upper chain ONCE per 2MB block and iterate the
   leaf PTEs directly (the reference's walk shape). Per-page semantics
   bit-exact with the old body (swap/P=0-marker/U-bit/foreign-frame/rmap/
   refcount/PCACHE-pin/device-window-restore discipline); upper-level-leaf
   shapes fall back to the old per-page walk (unmap_pages_slow). Also kills
   the old double walk (walk None -> read_pte_raw).
2. munmap: the per-chunk cond_resched is GONE — the reference does not
   schedule inside munmap; every chunk boundary still drops vma_write (the
   same-mm racer handoff points, mm-audit invariant intact); ONE yield at
   the operation boundary.
3. mprotect: the EACCES pre-check walks the RANGE only (seed containing-VMA
   + iter_after, break past span) instead of every VMA of the mm under the
   write lock (reference mprotect_fixup shape).
Gates: x3 builds rc=0 (x86_64/riscv64/aarch64), make test ALL PASSED
(cg19-maketest.log; the tail VW-WEDGE prints are post-halt noise).

### Verification numbers
- pbench P8 quiet A/B (same binary/rootfs/KVM): fixed-cold 24.4->17.0µs,
  fixed-hot 64.3->59.3µs, mmap0/mprotect unchanged (untouched paths; their
  residual is BTreeMap+split surgery + 3 lock acquires + IPI+CR3 pair).
- Under OH load (lb337/lb338 t=60s): mmap tot 84->22-30µs, fixmun
  210->40-45µs, mprot-tot 47->32µs, mpcov 1µs, mplock 0. syscost cross-check
  (lb336): nr=9 n=270,912 avg 45µs (lb325: n=179,154 avg 72µs — -37% per-op,
  +51% ops throughput). The vm-op family wall: ~15s/CPU-min -> ~5s/CPU-min.
- distributeddata SA:1301 init did NOT compress (lb336: 29,443ms vs the
  28.3-31.8s family) — the SA init wall is NOT the vm-op per-op cost; the
  KVS txns' deferred-reply/FFRT-pool family (17th's decode) stands as the
  next candidate. The issuers table (C-line) on the next healthy boot names
  who churns the 100-270k mmaps.

### Boot classes this session (system-lb5 + fresh2, SMP2, LOGLEVEL6)
lb330 (panic 1.0s = my slot bug), lb331/332/333 (pre-fix: race lost, ladder
spin; lb333 wedged 181->473s then recovered; lb332 bootanimation.ready 51s),
lb334/337/338 (ExecReboot panic 27-56s: lb334 storage_manager, lb338
hdf_devmgr crash + audio_host watchdog SIGKILLs — the 16th's lb314 spiral
class, ~50% rate, NOT attributed to the fix; lb337's dump thread landed its
line from the SECOND boot after reboot), lb335/336/338-second (healthy,
evidence above), lb339 rolling at close. NO PIXELS yet: the two structural
losers remain (BMS trigger ~40-82s vs store ready ~104-113s; the early-death
spiral class).

### Successor entry points (in order)
1. The remaining vm-op gap (fixmun 34µs vs ref ~3µs): the fixed path runs 3
   vma_write acquires + 2 BTreeMap removes + 1 add + pin/unpin + 2 Vma::new
   + IPI+CR3 pair per re-map. A single-hold unified MAP_FIXED surgery (the
   reference mmap_region shape) would chase it — but the chunked-munmap
   discipline is load-bearing (mm-audit invariant comments); design first.
   The present-page teardown path inside unmap walks runs ~1.2µs/page
   (rmap_alt_remove + page_remove_rmap + put_page_site + free) — 7.4M
   present pages per boot through munmap — a second-order lever.
2. THE CHURNERS ARE NAMED (lb341 issuers, eviction working): sysevent_source
   93,308 mmaps by t=180s (32% — the hiSysEvent boot scan), sa_main 22,137,
   appspawn 11,555, hdf_devhost 7,695, processdump 3,316. NOT
   distributeddata's DB_TaskExecutor threads directly. The 18th's "FFRT
   arena" attribution was the family; the top comm is the sys-event scan.
3. ROLL THE RACE (mission 2): the four coin-flips need one clean boot.
   Recipe unchanged (system-lb5 + fresh2 + dfx=ams46,txnhunt + syscost=1).
   Any frame change from frozen-logo md5 06b933b4: driver auto-writes
   run/oh-<tag>-shot-*.ppm — convert + report IMMEDIATELY.
4. The early-death spiral (3/8 boots tonight, 27-56s ExecReboot panic:
   storage_manager/hdf_devmgr crashes + watchdog SIGKILL waves) — pre-exist-
   ing class (16th's lb314 family, ~50% rate then as now); lb340/341 (the
   no-yield kernel) both ran healthy 800s, so the yield removal is not the
   trigger.

### lb339-341 addendum — the yield lesson and the final numbers
- lb339's B-line exposed the flaw in the first fix: with ~1.05 chunks per
  munmap call, moving the cond_resched from per-chunk to end-of-call fired
  it just as often (need_resched is ~always set under the OH churn — 1-2k
  munmap calls/s, each schedule 132µs behind ~165 runnable tasks). The
  REFERENCE kernel never schedules inside munmap; preemption is handled at
  syscall exit. ee42c599 removes the yield entirely.
- lb340/341 (no-yield kernel) t=60s: mmap tot=27µs (n=160k, MAX 2.1ms —
  the 749ms tail is GONE), fixmun=34µs (was 210), munmap body=52µs (was
  177-185), yld absent, mpcov=1µs, mplock=0; >16M mmap median 5.6ms ->
  415-427µs; mprotect 47µs (variance band 32-47 across boots — the cover
  fix is in (mpcov 1µs) but the whole-body write-lock hold under flood
  dominates the residual; an A/B candidate for the same surgery shape).
- syscost cross-check lb340 t=180s: nr=9 n=269,567 avg=32µs max=3ms
  (lb325: 179k avg 72µs max 722ms). The vm-op family wall: ~15s/CPU-min ->
  ~5s/CPU-min (-67%).
- unpg=7.4M present pages per boot through munmap at ~1.2µs/page of
  descriptor work (rmap/put/free) — the next mm-lever if the store window
  still matters after the userspace-internal residual.
- Final P8 quiet row on the no-yield binary (p8d, perf-gate re-bank):
  fixed-cold 16.8µs (ref 1.24), fixed-hot 58.8µs (ref 3.42), mmap0
  6.3-6.4µs (ref 0.22), mprotect 3.9/5.9µs (ref 0.51/0.53). The mmap0 and
  mprotect quiet residuals (BTreeMap+split surgery, 3 lock acquires, the
  per-call full TLB flush) are the successor's single-hold surgery target.
- mprotect load residual (lb341 A-line): tot=47µs = lock 0 + cov 1 + pte 4
  + ~42µs in the split surgery (Vec alloc + 3x Vma::new + 2-3 BTreeMap ops
  per overlapping VMA) + the per-call full flush — the same single-hold
  surgery shape applies.
- lb341 ran the full 800s healthy (race lost on the 83->115s trigger-vs-
  ready gap as every fresh boot this session; the ladder spin + POISON32
  trickle class, no crash).

## TWENTIETH-SUCCESSOR SESSION (2026-11-03 03:3x-.., branch biofix-fieldpoll @ 5ddc03d2)
Mission: decode distributeddata's SA:1301 init 29.4s on a post-fix kernel
(fuxstuck+txnhunt+ams46+mmprof), reference A/B for the same window, quiet-window
rolls. Outcome so far: THE 31s-CLASS WALL DECOMPOSED into a userspace
on-demand LOAD cascade with a kernel-measured 5.06s param-watcher stall at its
root gate — full evidence below; the binder deferral (mission ending (c))
audited CLEAN at all four tcomplete sites + error paths; lb342 instrument boot
banked; lb343 (syscost + 3s census bar) rolling at write time.

### Merge + instrument boots
- 5ddc03d2 = origin/main 113cd2b3 merged (docs; clean).
- ea6992a6 dfx(futex): stuck-census bar 10s -> 3s (the decisive cascade parks
  are 4-5s class — invisible at the old bar; diagnostics only, 8-line cap
  bounds volume). x86_64 build rc=0.
- lb342 (mmprof=1 + dfx=ams46,txnhunt,syncguard,fuxstuck,slowtask, LOGLEVEL=6,
  fresh2, load ~5.5): HEALTHY boot, race lost by the usual margin (CreateNewUser
  71.8s vs bundles.ready 103.2s = 31.4s), SA:1301 init 28,532ms (11.5->40.0),
  59 POISON32 (ladder-spin family), no panic/ExecReboot. mmprof: mmap tot=24us
  (n=178,799), fixmun=33us — the vm-op wall stays dead; issuers: sysevent_source
  165,050 / sa_main 22,137 / appspawn 11,373 / hdf_devhost 8,505 /
  DB_TaskExecutor **2** (distributeddata is NOT an mmap churner).

### THE DECODE (lb342 evidence, all timestamps host-side serial)
1. distributeddata (581) shows ZERO futex parks >10s all boot (fuxstuck: not a
   single tgid=581 line), ZERO binder txns >4s TO it (vs 13 distinct >5s on
   lb323/cg17) — the 17th's reply-deferral family and any condvar-starvation
   story are BOTH absent on the post-fix kernel, yet the wall persists at
   28.1-31.8s across every kernel since cg17 (28.3/28.8/31.8/29.4/28.5).
2. THE SHAPE IS A LOAD CASCADE WITH 4s METRONOMES. DB_TaskExecutor threads
   re-send SAMGR_LOAD(4802=device_manager) at 11.98/16.5/20.6/24.6/28.7/32.9/
   36.9 — 4.05s apart, replies 6-17ms except one 4.0s samgr-side outlier.
   softbus_server (exec 15.5) likewise re-LOADs 1120/1123
   (wifi_manager) at 18.96/24.97/28.75/32.47/36.39. Each waiter's loop =
   the 14th's sys=35 NANOSLEEP poll discipline; the loop releases when the
   dependency's SA registers.
3. THE CASCADE TIMELINE: samgr+hilog+param_watcher start 5.3-5.5; hdf_devmgr
   SETS bootevent.hdf_devmgr.ready at 5.433 but INIT'S PARAM SERVICE DOES NOT
   HANDLE IT UNTIL 10.496 (**5.06s in the param channel** — PCTL-RECV #2/#4
   land in one burst at 10.489-10.505; the FIRST param handle of the boot).
   bootevent.boot.completed fires 3ms later (10.505) — it is gated on
   hdf_devmgr.ready. The reference handles the same set same-millisecond
   (refu9: set ~3.52 handled 3.52).
4. boot.completed releases init's on-demand list: distributeddata execs 11.4
   (ref 4.5); device_manager loads via samgr (LOAD txn1198 11.98 -> spawn
   12.42 -> publish 13.0, 0.6s in-stage = reference-class speed);
   wifi_manager loads 20.0 -> publishes 30.7 (10.9s in-stage); softbus (exec
   15.5) registers only at 40.02 after its wifi/1123 waits; distributeddata's
   OnStart (28.1s) releases the moment softbus registers (40.02->40.03).
   SA:1301 and SA:4700 both print spend:28144ms — the SAME cascade, not a
   shared release.
5. CFG NOTE: device_manager.cfg (start-on-demand: BOOT_COMPLETED) is
   byte-identical between system-lb5 and the stock image — the cascade shape
   is the image's own semantics on BOTH kernels; the reference just runs
   every stage 5-25x faster, so the cascade compresses into ~2s there.
6. Mission ending classification: NOT (a) waker-runnable-starved (no parks to
   starve), NOT (c) binder deferral (audited below) — (b)-shaped, with the
   chain's ROOT = init's 5.06s param-service stall + every stage's per-op
   costs (the 4ms-8s SLOWTASK-blind band). The parks are <10s poll loops,
   kernel-clean; the per-op wall inside the stages is the lever.
7. BINDER DEFERRAL AUDIT (ending (c), code): four tcomplete sites — BC_REPLY
   (immediate, process_todo=true), ONEWAY (immediate), SYNC (deferred per
   963b3e95) — all balanced; dead_reply_to_sender pushes ReturnError with
   process_todo=true + exact-thread wake; the send-fail paths return err_cmd
   BEFORE any tcomplete queue (no dangling deferral). Two SYNCSEND-GUARD
   prints this boot (pid 485 telephony, 39.05/39.17, why=stack-top-not-mine/
   todo-head-txn at the 29201 ICommonEvent error) — noted, peripheral to the
   race path.
8. Also observed: txn11484 accountmgr(915)->foundation(871) exec 30s+ (server
   main thread in futex wait on its OWN stack condvar, thtodo=0, OWE says
   reply deferred userspace-side) — the osaccount chain riding the same
   stage-cost wall. W2-WAKE-REFUSED-SLEEPING pid=976 x3 (28.8-31.3s,
   state=0x0 on_cpu=false pk=1) — the wake-refusal-on-running shape, 3
   prints, no strand confirmed (needs the 3s-bar census to re-adjudicate).

### Mission-2 A/B numbers (same window, refu9 vs lb342)
- boot.completed: ref ~3.6s / ours 10.5s (the 5.06s param stall is 5s of it).
- distributeddata exec: ref 4.5 / ours 11.4. store serves: ref 5.9 (1.3s
  after exec) / ours 40.0 (28.6s after exec). creation: ref 8.3 / ours 71.8.
  SetBundleQuota stream: ref 8.5 / ours none (race lost). launcher.ready:
  ref 11.7 / ours never (frozen logo).
- Reference runs NO dynamic-proc loads (0 "Start dynamic proc" lines); ours
  routes device_manager/wifi/distributeddata through samgr LOADs — both
  kernels run the same on-demand cfg semantics; the reference's cascade
  simply finishes inside the first 2s after boot.completed.

### lb343/lb344 (3s-bar census + periodic taskdump + syscost) — the cascade under the microscope
- lb343: param-watcher stall present but variable (first param RECV 8.36s vs
  sends from 5.4s; lb344: sent 5.8 handled 14.1 = **8.3s**; lb342 5.06s). The
  stall is a per-boot variance band of the same shape: init's param service
  does not drain its queue while the service-start storm runs; boot.completed
  (gated on hdf_devmgr.ready) fires only after it drains — 10.5-14.5s on ours
  vs ~3.6s on the reference. ~7-11s of the structural gap sits HERE.
- THE SAMGR-INTERNAL INTERLEAVE (both boots): SLOW-TXN rows "queue 0 ms,
  exec Ns" — samgr's own threads call EACH OTHER (Executor->OS_IPC_2,
  OS_IPC_0->OS_IPC_1, OS_IPC_8->OS_IPC_9, OS_IPC_13->OS_IPC_7...) and every
  handler takes seconds with ZERO queue wait: code 2 x1.5-6.6s, code 3
  x2.2-6.0s, code 12 x9.0-9.6s, code 30 x2.3-26.3s, one code-20 x113s
  (audio_service->samgr), time_service->samgr x95s+. The handler threads show
  state=0x0 (RUNNING) sys=16 while "executing" for seconds — the handler is
  in USERSPACE code (past the BWR ioctl), either burning CPU at the per-op
  wall or spinning on a userspace lock whose holder is descheduled. The 12-deep
  OWE stacks on tid 339 (txn71/110/116/119/129/132 code20>328.458) are the
  nested-sync chain riding these per-hop seconds. samgr is the boot's
  serialization point; per-hop seconds compose into every SA registration,
  LOAD reply (the 4.0s outlier), and the 4s metronomes.
- vruntime CAVEAT for the successor: the periodic taskdump's vruntime field is
  STALE (mmprofd's dumps landed at 61/121/181s while its dumped vruntime stayed
  byte-identical) — do NOT use frozen vruntime as a not-picked proof. The
  strand adjudication needs sum_exec_runtime/last-pick in the dump line (the
  stress lane's W3-FROZEN-EXEC-SLOT shape).
- syscost t=240s (only clean window; 60/120/180 rows lost to the doubled-
  console interleave — R34 corruption eats our own telemetry, mmprofd-style
  task-context emitter is the fix): **nr=2 open() n=193,537 avg=85us
  tot=16,470ms per 60s** (the top syscall — the sysevent-scan class), nr=83
  mkdir n=844 avg=3,646us tot=3,077ms, nr=257 openat n=12,791 avg=181us,
  nr=77 futex n=92 avg=19ms. open() at 85us vs reference 1-5us class = the
  next per-op lever family (mkdir 3.6ms vs ~50us class).
- SPIRALS (mission-4 note): lb343 accesstoken_service exit 270.8s -> KOT-KILL
  waves (storage_manager group kills 294s) -> init sysrq-c 326.2s; lb344
  ExecReboot at 25.0s then crash-looped (3 ExecReboot/PANIC). lb342 healthy.
  2/3 this session vs the 19th's ~50% rate — same pre-existing class, not
  chase-attributed.
- PIXELS: none (md5 unchanged where shots landed). Race not reached on
  lb343/344 (both spiraled before/inside the cascade); lb342 lost by 31.4s
  as usual.

### Successor entry points (20th, in order)
1. THE SAMGR HANDLER BLACK BOX: instrument ONE samgr-internal slow handler
   end-to-end (code 12/30, queue-0/exec-seconds): is it userspace CPU at the
   open()/mkdir() wall, a userspace spin, or a chain? A per-tgid syscall-tap
   (scmtap exists) behind dfx, or fsw counters, inside the 15-40s window.
   The open()/mkdir() per-op rows above are the named kernel lever family.
2. init's param-service drain stall (5-8.3s variance): catch init's param
   thread posture DURING the stall with the 3s-bar census (its parks are
   <3s/cycle or non-futex — a 1s bar or a taskdump at t=8-12s would name it);
   kernel-side check the param socket queue wakeup path (unix socket
   readability for the param channel).
3. Roll the race (recipe unchanged + periodic): the four coin-flips still
   need one clean boot; 2/3 boots spiral early — the spiral is the dice.
4. mmprofd-style task-context emitter for syscost (the 60/120s rows are
   lost to console interleave today).
5. WAKE-RUN-LEAK "kicked-2x" threads + the W2 refusals: low-rate, but the
   census's own kick contaminates its snapshot — decouple (report first,
   kick next round).
- lb345 final: foundation SIGKILL wave at 48-51s (watchdog class) ->
  bundles.ready never fired -> no CreateNewUser -> ladder-spin POISON32
  trickle to 940s; bootanimation.ready 54.2s (2.4x lb342's 22.3s — this roll
  ran 3 own QEMUs deep, load-inflated). SA:1301 init reproduced at 29,016ms
  (12.9->41.7) with the identical cascade (device_manager OpenSo spend 23ms,
  wifi 12.1-14.5s, softbus 41.6, 1301 releases at softbus). Frozen-logo md5s
  unchanged. Race tally this session 0/4 (lb342 lost by 31.4s, lb343/344/345
  spiral-class).

### 21st session (2026-11-03) — THE PARAM STALL DECODED + THE OPEN() WALL CUT 3x (104->33us)

Synced origin/main ee332f51 (tree identical to the 20th's tip; no code delta).

**Lever-2 decode: the param drain stall is NOT a lost wakeup — it is init's
single main-loop thread BUSY for ~5s in the boot-job file-command storm.**
lb342 serial, timestamps host-side: last `le_epoll.c:122 timeout:-1` (wait
entry) at 5.036s; hdf_devmgr SETS ready at 5.433 (PCTL-SEND #3); init's
epoll loop does NOT return until 10.144-10.160 (next timeout:-1 line), and
the signalfd SIGCHLD reaps + PCTL-RECV #0..#5 drain in ONE burst right
after. In between, pid=1 prints are a continuous per-file command stream:
`boot job post-fs-data start` 5.605 -> finish diff **2,306ms** (reference
refu9: **275ms**, 8.4x), `early-boot finish diff 981ms` (ref 85ms, 11.5x),
`pre-init 2,033ms` (ref 733ms), then the `boot` job runs to ~10.1s. The
param messages land at 5.36-5.49s — just as the loop leaves for the job
chain — and queue behind it. Reference runs the same jobs in 0.36s
total, so its param set is handled same-millisecond. WAKE PATH IS CLEAN
(this round's named kernel question answered: the drain waits because
the reader is BUSY, not because a wake is lost — the 5.06/8.3s variance
= job-storm variance). Lever-2 collapses into Lever-1: cut the per-op fs
wall and the storm compresses.

**Lever-1 attribution (lb346, fsw=1 anatomy — infra from the perf-path
lane + new brackets, commit fd950765):** at t=330s: open n=218,469 wall
avg **99.6us** (matches syscost 104us — no contradiction, digits
re-read); walk loop wall 51.7s of which **miss=44.2s (85%)** over 339k
misses (130us each), **misseo=177,610** (52% of misses = ENOENT
existence probes, each paying the FULL ext4 dir scan: get_data_blocks
Vec + per-block extent walk + every dirent parsed — ENOENT was never
cached, the old no-negative-cache objection was the unbounded-heap
leak); walk PROLOGUE **66.6us/walk over 781k walks = 52s wall** — every
OH service is chrooted and paid make_absolute_rooted +
get_process_root(String) + normalize_with_root + dentry_walk_no_symlink
RE-WALK of the jail prefix PER LOOKUP; openexecavg (lb346 lacked PC2 —
truncation) — fixed by the PC1/PC2 split.

**Two kernel fixes (b21e126b, both pushed):**
1. **Bounded negative-lookup memo** (cmdline negmemo=1): ext4 ENOENT
   misses insert a NEGATIVE dentry child stamped with the parent
   INODE's dir_mut_gen; the 8 VFS mutation sites (create/mkdir/unlink/
   rmdir/rename/link/symlink/mknod) bump parent_inode.dir_mut_gen.
   Stamp on the INODE (not the dentry) because bind-mount aliases are
   separate dentries over one shared inode — a sandbox-view memo must
   invalidate when the file appears host-side. Ext4-only (ops.lookup
   fn-pointer gate; dynamic fs change without VFS mutations — procfs
   pid reuse). Cap 32 negatives/parent, first-found eviction. A stale
   memo (gen mismatch) is detached and re-resolved (NEGSTALE).
   Reference parity: the reference kernel caches negative dentries in
   its dcache with the same mutation-invalidation semantics.
2. **Chroot fast base**: with the jail root recorded as a dentry
   (set_root_with_dentry) and an absolute path, path_lookup STARTS at
   the jail-root dentry and feeds raw components to the loop ('.'/'..'
   /trailing-slash arms carry the semantics; '..' clamps at the same
   dentry). Kills the root-string allocation train AND the jail-prefix
   re-walk. Reference parity: nd->root IS a resolved dentry there; and
   the old code permission-checked the whole jail prefix per lookup,
   which the reference does not.

**A/B (lb346 off vs lb347 on, same recipe, fsw=1+syscost=1):**
- syscost open() t=240s: n=200,012 avg=**104us** tot=20.8s/min ->
  n=249,477 avg=**33us** tot=8.3s/min (3.2x; volume UP 25% = the boot
  gets more done per minute).
- walk prologue: 66.6us -> **2.0us/walk** (1.89s total vs 52s).
- negmemo: neghit=50,522 probes served from the memo, negstale=**90**
  (invalidation scheme stable), misseo 177,610 -> 163,891.
- boot.completed: 11.8s -> **9.2s** (hdf_devmgr.ready handled 11.5 ->
  9.09s); post-fs-data 2,843 -> 2,081ms (-27%); early-boot 1,291 ->
  938ms (-27%); pre-init 1,321ms. Remaining vs reference: post-fs-data
  7.6x — the residue is REAL-file cold-dcache misses (the found-miss
  half of the 130us miss class), mkdir/jbd2, and the sync-exec storm.
- **openexecavg = 29.6us exec vs 33us wall (PC2)** — after the fixes
  the open row is ~90% real kernel CPU (was ~70% waiting). The wall is
  no longer scheduling-inflated; the remaining sink is the miss path
  (ext4 dir scan) — next fs lever.
- open max 179ms/165ms class remains (blocking open tail —
  VFS_MUTATION_LOCK/jbd2-commit family, unattributed this round).

**Rolls: lb346 (off) + lb347 (on) BOTH wedged in the pre-existing
300-900s TXN-HANG exec family** (lb346: 334s exec 778->336, killed;
lb347: 333s exec 822->337 + POISON32 wave, killed). NOT a regression:
lb342 maxed 741s, lb345 941s on the 20th's kernels — the samgr
nested-call wall class. Race tally this session 0/2 so far. lb348
(same kernel, negmemo=1) rolling at write time.

**Successor entry points (21st, in order):**
1. **The miss path is now THE fs sink**: 40.8s walk-loop wall over 329k
   remaining misses (found-misses ~19.5s + unmemoed ENOENT ~21.3s).
   The ext4 lookup pays get_data_blocks (Vec alloc + FULL extent walk
   per block index) + every dirent parsed + bread per block, per miss.
   Cache the dir-block list per directory inode, or parse the on-disk
   htree index. fs is a guarded domain: pbench fs rows + boot-job A/B
   after any change.
2. The 165-179ms blocking open tail: bracket VFS_MUTATION_LOCK wait vs
   jbd2 commit vs bio wait on the O_CREAT path (the mkdir 3.6ms row's
   big sibling).
3. Roll the race on the nm2 kernel (recipe: SMP=2 LOGLEVEL=6
   USERDATA=run/userdata-warm2.img lb-boot.py <tag> 1500
   run/system-lb5.img run/bzImage-ladder-nm2 " syscost=1 fsw=1
   negmemo=1 dfx=fuxstuck,txnhunt,ams46,slowtask"). The wedge dice are
   the odds (pre-existing family); boot.completed now fires ~9.2s —
   1.3s earlier than the 20th's best gate.
4. PC2's openexecavg is the health check for any fs change: exec should
   track wall within ~20%; a widening gap = a new wait crept in.
5. The 70M slab.rs:640 rest-site n at t=180 (19.1s) — allocator
   class-lock volume 2.8x the 25M/boot band figure, worth a glance
   from the memory-domain owner (rest-site charging may differ from
   the allocwatch counter; do not merge the two numbers blindly).

**lb348/lb349 final: the fixes REPRODUCE, the wedge dice do not clear.**
lb348: hdf_devmgr.ready handled 9.17s, boot.completed 9.18s (stable
across boots now; was 10.5-14.5s), post-fs-data 2,209ms, early-boot
755ms, startUser/SwitchToUser at **46.9s** (lb342's creation was
71.8s) — then the pre-existing wedge: txn exec 772s (668->874, started
t=20s, BEFORE any memo churn — not negmemo-caused), no bundles.ready.
lb349: startUser 38.1s (25-34s earlier than the 20th's best), second
startUser retry 69.8s, max exec 505s — wedged again; frozen-logo md5
06b933b4 unchanged through t=540. Race tally 0/4 this session; ALL
four rolls in the pre-existing 300-900s TXN-HANG exec family (lb342
741s / lb345 941s on the 20th's kernels — same class). The gate is now
unambiguously the samgr nested-call handler wall (userspace poll-loop
interleave per the 20th's decode; syscost nr=35 poll tot=592s/min),
NOT the fs per-op wall this session fixed.

### 23rd session (2026-11-03) — THE WEDGE SEED NAMED (reply-vs-nested-call race), probes landed, 22nd's ext4 work completed+hardened

Synced nothing new from origin/main (d2b2672a was already merged; tip
6ab1b0e5 was the 22nd's, never rolled before this session).

**THE 22ND'S UNCOMMITTED ext4 WORK — completed, verified, pushed.**
The uncommitted mod.rs diff + untracked htree.rs were the miss-path
lever (21st's successor entry 1): a read-only parity port of the
reference dx_probe/ext4_dx_find_entry/__ext4fs_dirhash (commit 9cc6188f)
plus a bounded (32, replace-oldest) dir data-block-LIST memo keyed
(ino, i_size) serving lookup()/list_dir() without the per-block extent
walk. Review findings fixed before commit: parse_index_block now
enforces count<=limit (the reference's ERR_BAD_DX_DIR check; without it
the binary search could read past the block buffer on a corrupt count).
x3 builds rc=0, make test 1291/1291. SECOND fix (cbc6db61): the
(ino, size) key does NOT protect against inode REUSE — a deleted dir's
ino reallocated to a new dir of equal size would be served the old
list (the exact reuse class namei free_inode already purges for page
cache + icache). dir_blocks_purge() now runs at ext4_add_entry_locked,
ext4_delete_entry_locked (both under ENTRY_MUTEX — covers every
create/mkdir/link/symlink/mknod/rename/unlink/rmdir parent) and at
free_inode itself.

**IMAGE FACT that reframes the lever: system-lb5 has ZERO indexed
directories.** Superblock has DIR_INDEX (compat=0x38 bit 0x20 — note
0x4 is the WRONG bit, first read said no-dir_index, corrected), but a
full inode-table scan shows 379 dirs, ALL >1 block, NONE with
EXT4_INDEX_FL (verified two ways: kernel-independent Python scan of the
image + run/htree-verify.py). e2fsprogs never converted them. So the
htree port NEVER ENGAGES on the OH campaign images (it stays correct
parity code for dir_index images); the live miss-path lever here is the
BLOCK-LIST MEMO alone. Successor fs A/B should read PC2 miss/misseo vs
lb347's t=90s miss=23.5s misseo=161k with that in mind.

**MISSION-1 DECODE: the 300-900s TXN-HANG wedge has a NAME — the
reply-vs-nested-call race, lost.** From lb348/lb349's existing prints
(no new instrumentation needed to see it):
1. samgr -> foundation code-5 txn (lb349 txn8148 j=3637; lb348 txn9879
   — same code path both boots).
2. foundation's handler thread (OS_IPC_0_881/883) issues a nested sync
   call (code 6, the ohos.samgr.accessToken family per the SAREG
   parcels) ~10ms later: enq_j=3640, pick_j=0.
3. ~960ms later the DEFERRED BC_REPLY for the code-5 fires while the
   nested txn is still UNPICKED on the target proc todo. Our kernel
   rejects it (why=7 reply-stack-top, BR 0x7211) — and the rejection is
   REFERENCE PARITY (in_reply_to->to_thread != thread = BR_FAILED_REPLY
   in the reference's binder_transaction reply path).
4. The OH ipc lib does NOT retry the failed reply (AMS46-SENDFAIL,
   h=-1) -> the code-5 txn is NEVER replied -> samgr's caller thread
   parks forever -> the whole dependency tree above it polls forever
   (the "userspace poll-loop interleave"). lb349: txn8148 exec 505s at
   kill; lb348: txn9879 exec 776s. This IS the wedge family.
5. WHY the reference never wedges here: (a) source — the reference
   parks a sync sender INSIDE its BWR until the reply (the 16th's
   963b3e95 parity work), so userspace never regains control between
   send and reply to fire the early reply; AND (b) empirics — the
   archived reference-kernel serials (oh-refu8/9) show ZERO
   reply-stack/bad-transaction-stack binder user-errors on the same
   image (only benign early-boot "cannot find target node" and
   kill-time "undelivered death notification" classes), because the
   nested call is picked+replied in sub-ms there, so the deferred reply
   always loses the race SAFELY (fires after the nested call popped).
6. The kernel-side lever is therefore the NESTED CALL'S PICK LATENCY
   (ours 1.25s in lb349 = the race window) — NOT the rejection (parity)
   and NOT the poll loops (they are downstream symptoms).

**Wedge-window CPU sink (rip census, t=240s lb349, symbolized on a
rebuilt d2b2672a ELF in /tmp/nm2elf):** >50% of kernel-text tick
samples = core atomics/spinlock pages (~30%: 0x804a2-0x804a4xxx
load/store/cxchg + preempt), fmt/log/console pages ~20%
(0x8084e/0x8084f/0x8085/0x8086/0x802f4 UartRxBuf), syscost self-cost
5% (0x806fa), cli/sti page 20% (0x807e9). The wedged userspace's kmsg
flood + our own telemetry prints burn both vCPUs in the print path and
contended locks — that is the AMPLIFIER that keeps pick latencies at
seconds once the wedge seeds. (Console itself is already batched +
stall-bounded; volume is the cost.)

**Probes landed for the next wedge (bab4fe06 + 31dd5388, all ams46-
gated, print-only):** PICK-LATE (sync pickup with enq->pick >= 250
jiffies, names server/sender/lateness/enq_elig), SYNCSEND-EARLYRET (a
BWR returning rc<0 or staging NOTHING while an unresolved outgoing
sync txn sits on the stack — every hit is a park-divergence instance
the reference structurally forbids), R7-STACK entries now carry
to_pid. enq_elig (eligible-reader count at proc-todo enqueue) decides
lost-wake (elig>0, late pick) vs all-readers-busy (elig=0).

**Early-boot pick data (lb352):** PICK-LATE fired at t=8s for txns
4-7 (code 0x5f504e47, late=2.85s, enq_elig=1 then 0) — the pre-samgr
window; no late-boot hits yet (all four rolls died in the 60s spiral
before the wedge window).

**The samgr-FFRT segfault scare — exonerated, with evidence.** All
four rolls this session died at ~60s in the storage_manager kill-wave ->
init sysrq-c spiral, and lb351/352/353 each showed ONE samgr FFRT
segfault (3/3 vs 0/4 on the 21st's kernel — alarming). Chase: (a) the
22nd's htree port CANNOT be the cause (zero indexed dirs, above); (b)
the memo-purge kernel (lb353) still segfaulted; (c) the fault context
(USEGV-FOR, 22nd's forensics working): rip in ld-musl
`mov rax,[rax+0x88]; mov rax,[rax+rcx*8]` with rax=0 — a NULL object
after a dlopen-family failure; the new-kernel-only ENOENTs were all
libzmq.so.5 / libBulletSoftBody.so.3.25 probes and an on-image walk
PROVES those files are genuinely absent (walker now validates against
known-present files: ld-musl 2447, sandbox.json 1564, bin/init 285);
(d) baseline recount: samgr segvs exist on old kernels too (lb332,
lb334, lb343 — 3/15 boots ~20%). Verdict: pre-existing userspace dice
(NULL-handle on missing optional libs), not a kernel regression; the
session's 4/4 spiral rate rode host load 8.8-11 (17 foreign QEMUs).

**Rolls: 0/4 (lb351/352/353 sysrq-c at ~62s; lb354 light-telemetry
roll launched at session close).** No pixel change (frozen-logo md5
06b933b4 where shots landed).

**Successor entry points (23rd, in order):**
1. ROLL UNTIL A WEDGE with the h23c-class kernel + dfx=ams46,txnhunt:
   the three probes decide the wedge-seed fix in one boot —
   PICK-LATE.enq_elig=0 => nested-call tower capacity (chase the
   handler CPU), enq_elig>0 => wake-path bug (chase wake_proc_one);
   SYNCSEND-EARLYRET>0 => our park has a hole (fix per reference);
   R7 with to_pid => the exact server whose slow pick opened the race.
2. The pick-latency lever itself: the wedge window's CPU is
   print-path+spinlock (rip census). Cheapest kernel-side cuts: rate-
   cap the per-txn SAREG/SACB parcel dumps (100+ hex chars per txn in
   a txn storm), and consider a quiet-mode for txnhunt prints during
   rolls (the mission-4 recipe could drop fsw/fuxstuck/slowtask —
   lb354 runs this config).
3. If a clean race boot happens: uptime check at 38-47s (startUser
   window) — pixels may light; report immediately.
4. The memo A/B (PC2 miss/misseo vs lb347) still needs a surviving
   boot past t=90s; the htree port is dead weight on THIS image — do
   not spend on it until an indexed image matters.

## Session 1 (2026-11-03, first session)

### What ran

Tooling: kani 0.68.0 (host install was a broken empty bundle — the
`cargo kani setup` download from github.com times out on this host;
fetched the release tarball via the api.github.com asset redirect
(checksum-verified) and re-created the `toolchain` symlink the setup
normally makes). loom 0.7.2 from crates.io (reachable; only github.com
itself is blocked).

Extraction (target #1, SleepMutex): `kernel/verify/src/sync/
mutex_model.rs` — the R84/R85 state transitions copied text-faithfully
with environment hooks as parameters (jiffies -> `now`, wake_up_one
outcome -> bool at the release site, ANTI_STARVE_JIFFIES -> param).
NOT a cfg-gate inside the kernel crate: the house mirror-crate pattern
(kernel/verify) already solves the no_std/panic_impl collision, so the
pilot extends it. Drift control: new `sync/mutex_model` mapping in
scripts/verify_sync_check.py (env-hook transforms + anchor-based
region comparison for the recheck/release/epoch blocks inline in
lock()/release()); `python3 scripts/verify_sync_check.py
sync/mutex_model` green. release_if_owner is mirrored but not
text-compared (unit-tested instead — its was_held wrapper does not
survive normalization).

### Target #1 verdict: SleepMutex handoff state machine

- Invariants checked: INV-M1..M4 (locked/owner/depth coupling) after
  every state section; INV-M5 token conservation (waiters == counted
  tasks [+ leaked units]); INV-M6 no underflow (kani arithmetic).
- loom (feature `loom`, RUSTFLAGS --cfg loom, LOOM_MAX_PREEMPTIONS=3,
  release): 8 scenarios — S1 two-task pure fair (clean, strand-free),
  S2 three-task (timer-rescued class), S3 barging ungated spinner
  (timer-rescued), S4 recursion+do_exit, S5 anti-starve override, and
  P1/P2/P4 planted bugs (all CAUGHT: P1 via loom deadlock detection —
  the pre-R85 b148 wedge shape; P2/P4 via INV-M5 in the spending
  section itself).
- kani: `verify_mutex_ops_invariants` (every disciplined 6-step op
  sequence keeps INV-M1..M5, no arithmetic panic) and
  `verify_mutex_exit_leak_is_bounded` — both SUCCESSFUL, 2.2 s total.
  The two counterexamples kani produced on the way were HARNESS
  call-discipline gaps (pid-0 waiter aliasing no-task ownership; a
  counted waiter also taking ungated) — now encoded as documented
  kani::assume discipline; neither is producible by a kernel lock()
  call site. False-positive load: LOW and each was decidable in
  minutes from the concrete playback.

### REAL BUG FOUND AND FIXED (kernel commit on this branch)

loom's FIRST run (90 s in) violated INV-M3 (`!locked => depth == 0`)
on the anti-starve scenario: the acquire transitions' recursion arms
tested only `owner == pid`, and a FREE lock carries owner == 0 — the
same value `get_current_pid()` returns for the no-task context
(early boot / secondary harts — the exact contexts the degrade path
serves; lock()'s adaptive fair-try runs BEFORE the can_sleep check).
A pid-0 caller hitting a momentarily-free lock behind a counted waiter
(the handoff window) "recursed into" the free lock: depth=1 on
locked=false; the bogus guard's release can then free a lock another
task genuinely holds. Fix: `st.locked &&` on the four recursion arms
(commit da166982 — behavior unchanged for every real pid; owner==pid
implies locked for pid != 0). Reachability is narrow (no-task caller
racing a contended handoff) but this is the canonical coarse lock.
Regression test pins the sequence (mutex_model_test).

Second finding (design confirmation, no code change): the epoch
recovery alone does not cover multi-waiter strands — with 3+ waiters
(sole-waiter term dead, epoch term spent) both S2-pure-fair traces and
the barger scenarios park against a FREE lock; the R85 anti-starve
timer is the designed bound (<= 5 s). The loom harness must model that
timer (park-time deadline check) and DOCUMENTS the interpretation
rule: loom deadlock + lock free + all waiters parked = bounded-latency
state, not a kernel bug. Two-task contention is strand-free without
the timer (S1 proves it exhaustively).

### Cost numbers (this host)

- Fast suite: 16 model tests, <0.1 s (1118 pre-existing tests 0.2 s).
- loom suite (8 scenarios, preemption bound 3, release build):
  59.5 s wall, all green — under the survey's 5-minute exit criterion,
  so even per-push is affordable while the suite stays this size
  (growth is exponential in threads/rounds; keep scenarios small).
- kani mutex harnesses: 2.2 s wall (0.07 s + 2.13 s).
- Kernel builds: x86_64/riscv64/aarch64 all rc=0 with the fix.
- make test (riscv64, private rootfs copy): ALL TESTS PASSED, rc=0;
  my QEMU swept by cwd match (other lanes' instances untouched).

### Code-shape drift

Kernel source: +`st.locked &&` on four arms and expanded comments —
no structural change. The extraction is a parallel copy kept honest by
the sync check (which caught nothing beyond the intentional hooks
after the transforms were written once). The LOOM DRIVER (mutex_loom.rs)
is new model code (~450 lines) — the queue/park/condvar environment;
it is NOT sync-checked (it models, not copies); its fidelity risks
found during the pilot are documented inline (pop-on-wake equivalence,
phantom-token stale flag — fixed via the dequeue_task analog, W1
refused-wake shapes out of scope).

### CI recommendation: DO (staged)

1. KEEP the kani nightly (kani.yml) — and note it was BROKEN on main
   (vma_kani missing can_merge/merge — fixed in this branch); the
   mutex harnesses add ~2 s.
2. ADD loom (new .github/workflows/loom.yml in this branch): sync-check
   + mutex scenarios, LOOM_MAX_PREEMPTIONS=3, release — 59.5 s on this
   host, so it could ride per-push; recommendation is nightly +
   workflow_dispatch first (cheaper to keep honest), promoting to
   per-push only if a sync/ regression class ever escapes it. The
   interpretation rule (timer-rescued deadlocks need a human on first
   failure) also favors nightly.
3. Rule for writers: changes to kernel/src/sync/mutex.rs must update
   kernel/verify/src/sync/mutex_model.rs in the same commit (the
   smoke-time sync check enforces it); loom scenario growth should
   stay at 2-3 threads, <=2 rounds, bound 3.
4. The survey's exit criteria: (a) planted-bug reproduction — P1/P2/P4
   all caught; (b) clean on current main — yes, after the pid-0 fix;
   (c) runtime — fast suite 0.1 s, kani 2.2 s, loom minutes (fits
   nightly, not make-test adjacency); (d) the written rule — item 3
   here plus the sync-check CI wiring.

## TWENTY-FIFTH-SUCCESSOR SESSION (2026-11-03 19:00-.., branch biofix-fieldpoll @ d463189b+)
Mission: re-roll the race on the post-IRQ-fix kernel (origin/main e264a64b
merged as d463189b; TASK.md ours-by-intent, dtb_blob.S repointed to this
worktree), verify whether the dead-tick poison fix (d3700224) took the
wedge family's teeth, chase the 23rd's seed if it persists. Pair 1 of the
staggered relaunch (pair 2 waits on this closure).

### KERNEL BUG FOUND+FIXED FIRST: 3f377d13 negstale NULL-deref panic (ON MAIN)
- lb357 (cg26 = merge tip, negmemo=1): KERNPANIC pfault badaddr=0x98 rip=
  path_lookup+0x1c5c at 3.3 s, sys_mkdirat -> vfs_mkdir -> path_lookup,
  pid 1. Instruction: movzbl 0x98(%r12) with r12=0 = the dcache-hit arm's
  `unsafe { &*child_raw }; c.is_negative()` after the stale-memo arm
  returned core::ptr::null().
- Root cause: the lock-free dentry-walk refactor 3f377d13 (perf-path lane,
  on main) converted the stale-memo normalization's Option-return into a
  raw null, which falls past the match into the dcache-hit deref — the
  pre-refactor code returned None and took the fs-lookup arm. ANY
  negmemo=1 boot panics on its first negstale. lb347's "negstale=90 stable"
  predates 3f377d13.
- FIX (52067191): pre-step rewrites `found` to None on stale; the match
  keeps a single Some arm. x3 builds rc=0, make test green. Coordinator:
  cherry-pick to main — the 21st's negmemo lever is unusable on main
  until this lands.

### MISSION-1 VERDICT: the wedge family did NOT lose its teeth
- lb358 (cg26b = fix in): early-death spiral (storage_manager kill wave
  5.6 s -> init sysrq-c 42.8 s) — the pre-existing ~40-50% dice, unrelated.
- lb359 (cg26b, fresh2, dfx=ams46,txnhunt,negmemo=syscost=1, load ~4-5):
  HEALTHY through the cascade — SA storm 67+, CreateNewUser start 69.95,
  bundles.ready SET 101.36, CreateNewUser end 101.37, startUser 101.6,
  SwitchToUser 101.6 — and the clock is ALIVE: syscost ticks advance at
  full rate through the load window (c0 2711/14000/273 @60s ->
  7468/33614/389 @180s — ~233 ticks/s/cpu sustained; the d3700224 fix
  works under OH load; every historical census-silence reading is
  confirmed poison-suspect).
- BUT the race lost again (empty-set: zero "bundle size" prints, 451
  waiting-query ladder lines; no SetBundleQuota stream) AND the wedge
  seeded at ~8 s: txn pairs exec 998s-class by 1140s. Race tally 0/2
  (lb358 spiral, lb359 wedge). Pixels: all 7 shots md5 82f0a25f
  (frozen-logo class, no change).

### MISSION-2 DECODE: the lb359 wedge, step by step (the cleanest yet)
Victims: hdf_devmgr (pid 337) OS_IPC workers <-> power_host (611) /
composer_host (623) main threads. FOUR txns, all code 1, all seeded ~8.0 s:
- txn1240: power 611.611 -> hdf 337.555 (picked by 555; 555's inline
  handler parks in PRIVATE futex 0x7ff000a19af4 a1=0x80 — FFRT arena).
- txn1287: hdf 555 -> power 611 (611 received it while owing 1240's
  reply; parked in BWR).
- txn1386/txn1388: same shape one hop over (composer 623 <-> hdf 633).
- Stack evidence (TXN-HANG-OWE): BOTH sides hold BOTH txns — 611's stack
  = [1240(sent), 1287(received)], 555's = [1240(received), 1287(sent)].
- Cycle: 555 handles 1240 inline -> futex-waits on an hdf-internal
  condvar; 611 parked awaiting 1240's reply; 555's own outgoing 1287
  needs 611 (parked). 633/623 mirror it with an async dispatch (633 back
  in read while 1386 still stacked). By 601 s EVERY pid-337 thread is
  INTERRUPTIBLE(sleep) (task dump) — the proc is a closed wait graph.

### The kernel-side exoneration (source-verified against fetched 6.6 binder.c)
1. The delivery mechanism IS reference-parity: composer 623's nested
   1386 was delivered to parked originator 633 by the from-walk — 6.6
   binder_transaction() lines ~3099-3132 keep `while (tmp) { if
   (tmp->from && tmp->from->proc == target_proc) { target_thread =
   tmp->from; break; } tmp = tmp->from_parent; }` — first-match-break,
   same as our .rev()+break (binder.rs ~8032). The wake is
   binder_enqueue_thread_work_ilocked = push to thread todo +
   process_todo=true — our 8052-8055 matches; the reference read loop's
   `wait_for_proc_work=false` thread consumes its own todo identically
   to our from_thread branch (8712-8718).
2. has_work/available predicates match 6.6 (process_todo || need_return
   || eligible-proc-todo; available = empty stack+todo+looper flags).
3. The one SYNCSEND-EARLYRET hit (6.379 s, pid 501.509, txn552 code12 to
   samgr, enq_j=585 pick_j=0, c1=BR_CLEAR_DEATH_NOTIFICATION_DONE) is
   the multi-command-write shape: BC_CLEAR pushes ClearDeathDone to the
   CALLER's own todo with process_todo (parity: binder_enqueue_thread_
   work_ilocked on the write path) — the read returns with the done
   notice while the sync txn sits unanswered. The reference returns the
   same way (has_work(thread,false)=process_todo). OH re-enters the
   ioctl and parks. NOT a divergence; the probe's c1 table should treat
   ClearDeathDone like BR_TRANSACTION_COMPLETE (benign).
4. The R7 rejection (11.735 s accountmgr 825.840 depth=2) is parity
   (23rd already established) and did NOT grow into a stranded family on
   this boot.
=> The wedge is NOT a binder-delivery divergence. The from-walk delivered
the nested call correctly; the RECEIVER'S HANDLER (hdf_devmgr userspace,
inline on 555 / dispatched for 633) parks on an FFRT-arena PRIVATE futex
and never completes, closing a cross-process wait cycle the reference
completes in milliseconds (same image, refu8/9: whole boot 12 s).

### Open fork for the successor (lb360 adjudicates): (a) vs (b)
- (a) hdf's handler waits on a condvar whose signaler depends on the
  outer call's completion = logical OH deadlock (no kernel lever; needs
  the reference's speed to never enter it — per-op perf lane).
- (b) the signaler exists but is stuck in a KERNEL object (lost futex
  wake / unfinishable syscall) = kernel bug, fuxstuck's anywoken field
  decides it (a never-woken uaddr with runnable producers).
lb360 RAN (cg26b + dfx=ams46,txnhunt,syncguard,fuxstuck,slowtask +
negmemo=1). Verdict:

### lb360 — the (a)-vs-(b) fork, adjudicated partial: the family ROTATES
- Race lost again (CreateNewUser 60.3 / ready 91.0 / startUser 91.4 —
  the compressed structural loss; no ladder spin this boot). Pixels:
  both shots md5 06b933b4 (frozen logo, no change).
- THE WEDGE PAIR REPRODUCED AND RESOLVED: power 597.597 <->
  hdf 334.554 txn1121/1180 seeded ~7.2 s (identical four-stack shape),
  audited at 4.3 s exec (554 in BWR with deferred tcomplete; 597's
  handler alternating nanosleep/BWR = retry-ladder posture), and
  COMPLETED between 11.5-16.5 s (audit stops; pair gone). lb359's
  same-shape pair persisted 700 s+. So the cycle is NOT a hard logical
  deadlock — a throughput-bound stall; boot variance decides whether a
  given pair completes. Later strand at kill (346 s): txn16151 power
  OS_IPC -> powermgr 686 exec 96 s+ server-parked (same family).
- Census: ZERO futex parks >3 s in tgid 334 (hdf) all boot — this
  boot's role rotation put no inline-handler futex park in hdf. The
  standing anchors: tgid 385 x2 parked 278 s anywoken=0 with tgidthr
  run=3/futex=13 (churning pool, not lost-wake); run=0 pools =
  userspace-deadlocked peripherals; EVERY printed uaddr anywoken=0 —
  the futex wake side stays clean (losthunt verdict re-confirmed).
- SYNCSEND-EARLYRET: the SAME deterministic hit every boot (pid 50x,
  txn552 code0xc to samgr 328, c1=BR_CLEAR_DEATH_NOTIFICATION_DONE,
  enq_j~555) — benign per the 6.6 parity read (multi-command write
  buffer; reference returns the same way; userspace re-parks). The
  probe's c1 table should classify ClearDeathDone WITH
  BR_TRANSACTION_COMPLETE (own-todo notice), not a park hole. R7: 1
  parity hit (accountmgr class).

### NET for the campaign (25th successor's whole-lane read)
1. The store-load chain is compressed and stable (ready ~91-101 s,
   creation tracks it, startUser lands). Remaining race losses: the
   BMS per-user query returning empty on fresh userdata (17th's
   structural decode stands) + the early-death spiral dice (~30-50%).
2. The wedge family is CLOCK-INDEPENDENT (full-rate ticks through a
   wedged window) and BINDER-PARITY (every hop source-checked against
   fetched 6.6: from-walk first-match-break, process_todo wake,
   has_work/available predicates, ClearDeathDone earlyret). The strand
   pairs are userspace-internal waits (FFRT futex parks, usleep retry
   ladders), throughput-bound. Kernel lever = the per-op wall (perf
   lane: open 33 us, mkdir 3.6 ms); an hdf-side OH fix is out of scope.
3. The 23rd's seed is ADJUDICATED: no kernel park hole found in two
   probe boots; the observable earlyret shapes are reference-legal.

### Housekeeping this session
- Orphan cleanup: killed the 24th's leftover lb355 QEMU (335042,
  reparented to systemd, cwd this worktree); stress-port lane's QEMU
  (370172, live timeout wrapper) left alone. My own lb357/358/359/360
  QEMUs killed by exact pid.
- Kernels: run/bzImage-ladder-cg26 (pre-fix, panics on negstale),
  run/bzImage-ladder-cg26b (WITH fix — the roll kernel).
- Gates on the final tree (d463189b + 52067191): x3 debug builds rc=0
  (re-run AFTER the vfs fix), make test green (riscv64, "All tests
  passed", suite verdict line checked), x86_64 two-pass release
  (bzImage-ladder-cg26b).

### Successor entry points (25th, in order)
1. COORDINATOR ITEMS: (a) cherry-pick 52067191 (negstale NULL-deref)
   to main — every negmemo=1 boot on main panics at its first stale
   memo until then; (b) the perf handoff stands (per-op wall is the
   wedge family's only kernel lever; syscost/pbench rows for the fs
   fixes already banked by the 21st/22nd need re-verification on a
   quiet host if the coordinator wants fresh numbers).
2. THE RACE (unchanged recipe, cg26b-class kernel + the fix):
   system-lb5 + USERDATA=run/userdata-fresh2.img + SMP=2 LOGLEVEL=6 +
   " dfx=ams46,txnhunt negmemo=1" (+syscost=1 when measuring). The
   account side wins every healthy boot; pixels need (i) no early
   spiral, (ii) the BMS KVS query to hit a serving store — the 17th's
   29201-family readiness gap. Watch "create user:100 bundle size:34"
   then SetBundleQuota -> appspawn com.ohos.launcher; screenshot
   md5 leaving {5f8b704d, 06b933b4, 82f0a25f, 23312e5b} = PIXELS:
   convert (PPM->PNG pure python) + report IMMEDIATELY.
3. THE WEDGE, if chased further: it is userspace-internal and
   throughput-bound (this session's adjudication). The un-tried kernel
   angle: WHY does hdf's inline handler futex-park 700 s on some boots
   (lb359 555) while the same pair resolves in 9 s on others (lb360)?
   A boot with BOTH fuxstuck AND periodic taskdump through the wedge
   window would name the futex's owner histogram at park time (the
   census prints only park-time state, not the signaler's identity).
4. The early-death spiral (storage_manager/hdf_devmgr kill waves ->
   init sysrq-c, ~30-50%): unresolved for many sessions; a forensic
   boot with dfx=taskdump from t=0 + hilog keys for the FIRST crash
   (not the cascade) would name the origin service crash.

## TWENTY-SIXTH-SUCCESSOR SESSION (2026-11-03 20:00-.., branch biofix-fieldpoll @ 491ebc09)
Mission (user-overridden stagger, launch priority): the launcher wall is
NOT performance (microbench ahead: ctx 3.4x, pipe 2.6x, exec 1.39x) —
decode the parked-poll signature (lb360 tid 749 sys=16 40+s), prime
suspect binder fd poll-wake semantics, fix per reference, roll.

### 1. THE PARKED POLLS DECODED (mission item 1, complete)
- The parks are NOT poll() syscalls. sys=16 on x86_64 = ioctl, a0=0x5,
  a1=0xffffffffc0306201 = BINDER_WRITE_READ on /dev/binder fd 5. AMS46-T
  reads the parked task's entry frame (orig_rax/rdi) — tid 749 was in
  ONE blocking BWR read from j=92 through j=172+ (80+s), state=0x1
  (INTERRUPTIBLE). SLOWTASK: ~a dozen services (el5_filekey_man,
  storage_manager/daemon, OS_IPC_0_*, av_session, deviceprofile) parked
  in the same BWR shape, ages 92-104s — all entered ~t=5.5s (idle).
- Timeout argument: none — BWR read parks until work or a signal.
- Wait queue: the thread's per-thread binder wait queue (thread.wait via
  wait_event_interruptible, mirroring binder_wait_for_work's
  prepare_to_wait(&thread->wait)) with waiting_for_proc_work membership.
- fd class for actual poll()/epoll_wait() users (init le_epoll, ffrt,
  netstack, dsoftbus): timerfd/eventfd/sockets — and OH NEVER polls the
  binder fd (oh-robot sources checked: ipc framework is pure blocking
  BWR; ipc_process_skeleton.cpp has a leftover epoll include, zero uses).

### 2. THE DISCRIMINATION (mission item 2): wake path EXONERATED by data
- Every tracked txn pick in lb360: SLOW-TXN queue=0 ms (only 4 SLOW-TXN
  all boot, all queue 0) — work-to-wake-and-dispatch is 0 ms-class on
  the exercised path. The 32 sys=16 TXN-HANG samples all show
  wait-proc-work=false with a userspace-deferred reply or an FFRT
  futex-parked handler (the adjudicated wedge family) — zero
  "parked with pending proc work" shapes. The reference cannot beat
  0 ms-class by a margin that explains 31 s; the poll-wake channel is
  NOT the launcher wall. (25th's binder-parity adjudication stands.)

### 3. REFERENCE-PARITY FIXES LANDED ANYWAY (mission item 3) — 491ebc09
Source diff vs the fetched 6.6 binder.c found four divergences in the
suspect family; all fixed (reference wins):
1. Availability semantics: binder_available_for_proc_work_ilocked is
   empty txn stack + empty own todo, NO looper flags. Removed the
   LOOPER_ENTERED/REGISTERED gates from thread_has_work, the read drain
   arm, waiting_threads membership, and wake_proc_one's pick. The old
   gates (b85-era) could park a proc's only unflagged reader forever
   with work queued (wake_proc_one found no pick; has_work hid the proc
   todo) — a never-woken class the reference cannot have. sc161's
   work_readable_by routing KEPT (narrower divergence preventing the
   sc156 daemon-reader black hole); wake pick now prefers readers that
   can actually read something queued (avoids routing-shaped wake eats).
2. binder_poll ported: polling tid's binder thread state (created on
   first poll, binder_get_thread parity), LOOPER_POLL (0x20) state,
   has_work readiness predicate.
3. binder poll_wait registration: binder_poll_wait/unwait in the
   sys_poll wait registry (poll_wait(filp, &thread->wait) parity) —
   poll()/epoll_wait() over a binder fd now wakes on the per-thread
   queue instead of the 10 ms re-check slice (was: binder absent from
   poll_wait_impls → NotWired fallback).
4. binder_wakeup_poll_threads_ilocked parity: no-candidate wake now
   wakes POLL-mode available threads.
enq_elig probe predicate updated to match. x3 builds rc=0, make test
green, pushed. Roll kernel: run/bzImage-ladder-pollwake (two-pass
x86_64 release).

### 4. ext4 wave (mission item 4): 9cc6188f + cbc6db61 verified intact
on the branch after the origin/main merge (a31e9a4a; merge touched only
docs/test/lmbench-report.md). The wave lands on main when this branch
merges — no rebase needed; coordinator's earlier cherry-pick abort was
main's diverged ext4 structure, and the branch lineage is the fix.

### 5. ROLLS: lb361+ with the pollwake kernel (results appended below)

### 6. ROLL RESULTS (pollwake kernel, bzImage-ladder-pollwake @ 491ebc09)
- lb361 (load 2->5): LOST, wedge boot. Early cascade crawled
  (foundation start 32.5s vs 8.4-8.8s on lb359/360), hiview exit-255 at
  28.9s + write_updater-255; never reached bootanimation.ready;
  appspawn->607 strand txn1737 exec 1467s at close.
- lb362 (load 5->7): LOST, no-ladder boot. foundation 12.9s,
  bootanimation.ready 14.9s / finished 22.4s, SetBundleQuota stream at
  18s (the BMS store serving EARLY - a good sign), but zero
  create-user/bundle-size ladder and no launcher.ready by 1500s (the
  osaccount error:32 readiness family, 16.8s).
- Pixels: 20 shots across both boots, all frozen classes (06b933b4 x9,
  82f0a25f x11) - no change. Race tally with the parity kernel: 0/2
  (25th was 0/3 on cg26b-class) - same dice, no outcome regression.
- Regression watch on the fix: SLOW-TXN queue=0ms on BOTH new boots
  (lb361 n=10, lb362 n=4 - the pick/wake path stays clean); USEGV
  counts stable (lb358:0 - it spiraled before the segv window, lb359:62,
  lb360:61, lb361:61, lb362:60 - usb_host same-rip segv present pre AND
  post); hiview
  exit-255 is 1/boot UNIVERSAL on historical boots (lb302-355 all have
  it; lb358/359/360 were the exceptions) - NOT a fix artifact; lb361's
  32.5s foundation crawl is within the known load-variance family
  (lb345: 29s walls under 3-QEMU load) but watch it on the next rolls.
- A/B wake-latency verdict (mission item 2, banked): our work-to-wake
  is 0ms-class on every boot pre AND post fix (queue=0ms on all
  SLOW-TXN across lb359-362); the reference boots the same image in
  ~12s end-to-end and cannot beat 0ms-class picks by any margin that
  explains a 31s gap. The binder poll/wake channel is EXONERATED as
  the launcher wall. The wall remains: (a) the BMS/osaccount readiness
  gap (error:32/29201 family - empty query ladder), (b) the early
  spiral/wedge dice (userspace-internal FFRT waits, adjudicated).

### Successor entry points (26th, in order)
1. THE RACE: same recipe, quiet window (load <4): system-lb5 +
   USERDATA=run/userdata-fresh2.img SMP=2 LOGLEVEL=6 " dfx=ams46,txnhunt
   negmemo=1". Watch lb362's good sign (SetBundleQuota at 18s) repeat;
   the account side (create user:100 bundle size:34) is the deciding
   half. Pixels: md5 leaving {5f8b704d,06b933b4,82f0a25f,23312e5b} =
   convert + report IMMEDIATELY.
2. The osaccount error:32 (GetOsAccountProxy) family at ~16s is the
   sharpest un-hunted structural loss visible on healthy boots - the
   17th's empty-set decode's direct predecessor print. A probe boot
   naming WHY the osaccount proxy fails at 16.8s (which SA load/order)
   would close the last named gap on the losing side.
3. lb361's early crawl (foundation 32.5s) under load 5: if the next
   quiet-window roll shows foundation >15s, bisect between the parity
   fix (491ebc09) and load noise by rolling the cg26b kernel once.
4. Coordinator: 491ebc09 is pushed on biofix-fieldpoll (contains the
   negstale fix lineage + ext4 htree/memo wave + this parity fix) -
   merging the BRANCH (not cherry-picks) lands everything cleanly;
   run the merge gate's tier-2 check (binder/ is a hot path).

## TWENTY-SEVENTH-SUCCESSOR SESSION (2026-11-03 21:20-.., branch biofix-fieldpoll @ 3a40c3df = merged main be56334a)
Mission: quiet-window rolls on the pollwake+parity kernel; re-decode the
readiness gap with 491ebc09 live (never-woken class gone?); track the
wedge/spiral dice; bank entry points.

### 0. Gate audit (mission item 2, "verify no other gate remains") — verified against the FETCHED 6.6 source
Reference: /home/william/workspace/oh-robot/kernel/linux/linux-6.6/drivers/
android/binder.c (read directly this session):
- binder_available_for_proc_work_ilocked (line 606) = `!transaction_stack
  && todo empty` — NO looper flags. The 26th's fix matches the reference
  exactly. The flag-gated variant was pure b85-era local patch.
- binder_wait_for_work (line 4358): waiting_threads membership =
  do_proc_work (availability), no flag check. Our fixed membership
  (binder.rs ~8793) matches.
- binder_thread_read (line 4467): an unflagged proc-work waiter only
  gets a binder_user_error WARNING (the wait_event on
  binder_stop_on_user_error is a no-op at the default 0) — it still
  parks and consumes. Our drain path is parity.
- Remaining LOOPER_ENTERED/REGISTERED sites in our binder.rs are NOT
  availability gates: (a) post_txn_to_child_thread's sc165 routing
  PREFERENCE (has a proc-todo fallback), (b) BR_SPAWN_LOOPER emission
  decisions (~4200/9289/9307 — reference has its own looper condition
  there). The never-woken class is CLOSED; the only kept availability
  divergence is sc161 work_readable_by tgid routing (deliberate,
  prevents the sc156 daemon-reader black hole).
- Stale comments (docs-only, no behavior): binder.rs ~8777-8782 still
  describes proc_work as "+ entered/registered looper" for the
  wait_for_work tracepoint words, and the wake_proc_one doc block
  ~1748-1755 still says the predicate "must match ... ENTERED/REGISTERED
  looper" from b85 days. Successor may clean up when touching the file.

### 1. lb363 (roll 1, POLLUTED): load 2.95 at launch, rose to 5.5 within
90 s (two TCG QEMUs from the stress/LTP lanes). Early crawl -> no
bootanimation.ready by 350 s; NEW PRINT CLASS for the campaign: 175x
`DEADLOCK: spinlock stuck cpu=0/1 lock=0xffff8880xxxx ra=Task::rlimit`
(per-task rlimits locks, waiter in tick/IRQ context; kernel/src/sync/
spinlock.rs fires per 100M spin iterations). NOT a binary regression:
lb363 used the byte-identical pollwake kernel as the 26th's clean
lb361/362 rolls, and zero prior boots (lb344-362) show this shape —
it is the KVM vCPU starvation class (holder descheduled mid-hold by
host load; watchdog threshold ~100-500 ms per print). Killed my own
QEMU at 350 s. Successor note: treat sustained rlimits DEADLOCK storms
as host-pollution signature, re-roll, do not bisect the kernel on them
unless they recur at load <3.

### 2. lb362 forensic re-decode (26th entry point 2, from the existing
serial log — no new boot needed)
- The osaccount error:32 burst (14.3-14.4s) is TRANSIENT and benign in
  shape: accountmgr proc starts 13.07s, SAREG txn3317 at 14.28s, SA:200
  "init finished" at 14.34s — the error:32s land INSIDE that
  registration window, and GetOsAccountProxy failures STOP at 16.8s.
  The proxy stabilizes. NOT the boot's real loss.
- The REAL lb362 loss: distributeddata OnStart-SA:1301 NEVER finishes.
  Healthy lb359: OpenSo 8.6s -> "Start OnStart-SA:1301 finished, spend:
  27815 ms" -> insert SA:1301 at 36.4s. lb362: OpenSo 10.2s, then NO
  insert/Publish/init-finished print for the whole 1500s. Consequently
  zero KV-store service: no insert SA:1301, BMS SetBundleQuota stream
  stops after 1 bundle (2 lines at 18s vs 84 on a won boot), and
  lb362 also never registered storage_manager SA:5003 / foundation
  180/182/3203/3704/403/501/6105/6200 / softbus 4700 / deviceprofile
  6001 / telecom 4005 — the whole downstream SA cascade of the KV
  store froze with it.
- lb362's taskdump at 602s: tgid 581 main thread lastsys=192
  (semtimedop) with GARBAGE args (frame stale — several threads print
  nonsensical lastsys; don't trust taskdump lastsys without sane args).
  Our semtimedop arms a deadline timer (sysv_sem.rs ~1165 add_timer_
  wakeup) so a timed op cannot forever-park by construction; treat the
  192 reading as stale, not as a SysV-sem kernel lead.

### 3. lb364 (roll 2, CLEAN quiet-window launch at load 2.6, full probe
set dfx=ams46,txnhunt,fuxstuck,slowtask + negmemo=1) — WEDGE boot, and
the readiness gap's mechanism CAUGHT LIVE:
- distributeddata (pid 560) starts 8.16s, OpenSo 8.49s. Main thread
  (tid 560) parks on FUTEX_WAIT_PRIVATE at stack uaddr 0x7fffffffd004
  (FFRT sync wait) from ~9s and NEVER moves: census shows park=55s ->
  96s+ monotone, anywoken=0 (NO wake ever fired on that uaddr, all
  boot). OnStart-SA:1301 never completes; no other tgid-560 thread
  parks >=3s (the DB executor churns sub-3s) — the DB machinery moves,
  the main thread's FFRT wait simply never resolves.
- THE WEDGE, this boot's role rotation: SAMGR itself. At 12-18s samgr's
  serving workers (main 328, OS_IPC_0_338/2_349/5_555/6_556, OS_FFRT_
  2_5=443) all futex-park with PENDING txns; every TXN-HANG from 18s on
  targets pid 328 (accountmgr, telephony, bootanimation CompThread,
  hiview, powermgr, samgr itself; execs 226-340s+ at close). Last SA
  registration on the whole boot: 17.7s (media_service). Census
  histogram for tgid 328 at 365s: run=1 futex=24 int=3 — near-closed
  wait graph (not fully run=0).
- UNIFICATION: lb362's distributeddata-main park and lb364's samgr
  collapse are the SAME FFRT private-futex wait-graph family, different
  role rotation. Whichever of samgr/distributeddata/hdf-power it lands
  on, the boot loses (total freeze if samgr, readiness loss if
  distributeddata). Kernel channels instrumented clean on both: futex
  anywoken=0 everywhere (wake never fires — not lost), SLOW-TXN
  queue=0ms picks, binder parity audited above. The 25th's
  userspace-internal adjudication EXTENDS to the readiness gap leg.
- Census steady-state peripheral family unchanged: bluetooth(384) 171
  lines, user_auth_host, telephony, sensors, softbus, wifi — all
  anywoken=0 run=0-class.
- Pixels: frozen class only (82f0a25f).

### 4. Race tally on the pollwake+parity kernel: 0/3 this session
(lb363 crawl excluded as host-polluted), 0/5 with the 26th's. The dice:
wedge (lb361, lb364, lb365, lb367), distributeddata-hang/no-ladder
(lb362). No outcome regression from 491ebc09 on the clean legs
(SLOW-TXN queue=0ms held on lb364; no new panic classes; USEGV stable).

### 5. lb366 — the cg26b A/B roll (isolates 491ebc09: cg26b has the
ext4 wave 9cc6188f/cbc6db61 but LACKS only the parity fix)
- OnStart-SA:1301 COMPLETED, spend 27509 ms, published 38.3s — the same
  27.5-27.8s as lb359/360. With lb361 (pollwake, crawl boot) also
  completing OnStart at 73.5s, the parity fix is NOT proven to cause
  the distributeddata OnStart hang (pollwake clean-cascade samples:
  1 hang / 1 complete; cg26b: 3/3 complete). The lb362-class hang
  stays in the FFRT dice family.
- HEALTHIEST cascade of the campaign: appspawn 6.4s, bootanimation
  ready 18.8s / finished 34.5s; KVS serving from 38.3s; creation
  start 67.3 / bundles.ready 98.7 / end 98.7 / startUser 98.9; i18n
  (USER_ADDED) at 113.7s. The 31s readiness gap CLOSED — creation met
  a long-serving store. AND STILL LOST: zero "create user:100 bundle
  size" prints, zero SetBundleQuota, installs service parked in an
  EMPTY binder read 108+s (nobody ever sent it work).
- Pixels: 5 shots all 06b933b4 (frozen).

### 6. THE SCAN-LEG DECODE (the session's main structural find, from
oh-robot sources + lb317/lb362/lb366 logs)
- The loss hierarchy on healthy fast boots is NOT the KVS timing. With
  the store serving 29+s before creation (lb366), CreateNewUser still
  produced nothing: source chain = CheckInitialUser parks on a
  BundlePromise until the INITIAL USER-0 scan completes
  (bundle_user_mgr_host_impl.cpp:520), then GetAllPreInstallBundleInfos
  returns !empty() (:383) — empty set -> "GetAllPreInstallBundleInfos
  failed" -> return ERR_OK SILENTLY (no size print, no error on
  serial). The ROOT is upstream: the initial user-0 preinstall stream
  never installs ANYTHING.
- The scan lifecycle IS observable via the param channel:
  bms.scanning_apps.status 0 (BeforeBmsStart) -> 1 (scan end).
  lb366: 0 at 34.5s -> 1 at 74.6s, ZERO SetBundleQuota in between —
  the scan "completed" WITHOUT INSTALLING (the 17th's "failed-scan
  completion", now instrument-named). lb362: 0 at 16.5s -> 1 at
  565.9s with exactly 2 quota lines at 18s (systemres) — the install
  loop STARTED then stalled ~549s on the not-yet-published KVS
  (SA:1301 never came up that boot). lb317 (the campaign's only scan
  win, cg16f): scan 164.5 -> quota stream 167.7-204.4 (84 lines) ->
  CreateNewUser at 183.4 found bundle size 34 INSTANTLY.
- SetBundleQuota census across ALL fresh rolls: lb317=84; lb322/324
  (cg17) = 0; lb358-361, 364-366 = 0; lb362 = 2. THE SCAN LEG HAS BEEN
  DEAD SINCE cg17 (2026-11-02) — since the kernel got 2x faster, NOT
  since the ext4 wave (9cc6188f landed 11-03 08:07, after lb322-324
  already showed 0) and NOT since the parity fix (lb366 = cg26b-class,
  no parity fix, still 0).
- lb317's win mechanics: init SIGKILLed the first foundation at 152.2s
  (watchdog/critical-service kill) + installs at 154.5s; the RESTARTED
  foundation (pid 1009) re-ran BMS with the KVS long-serving (up since
  ~36s) and its scan WON. No boot since got the restart, and the
  first-pass scan on fast kernels never installs. The 17th's entry
  point 1 ("a restart landing after ~105s should hit a populated
  store and WIN") is confirmed as the win mechanism — but it needs the
  first foundation to die, which is itself dice.
- ams46 tap extension (e304836f): added the BMS boot-scan verdict keys
  (BmsStartEvent / OnBmsStarting / first boot / no space / ReInstall
  all haps / Guard against / QueryAllData failed / persistent data is
  empty / GetAllPreInstallBundleInfos failed / Bms initial user).
  lb367 verdict: the image LEVEL-GATES those I-lines out of the hilog
  socket the tap reads (not even lb317 shows them, while its W-level
  and the size-print I-line DID flow) — the tap cannot name the branch.
  Next instrument for this leg must be fs-side (opendir/readdir probe
  on the preinstall roots) or perf-side (make the scan's dependencies
  reference-fast). The preinstall list itself is a LOCAL RDB
  (PreInstallDataStorageRdb), not the KVS; the KVS dependency is
  LoadInstallInfosFromDb (bundleInfos persistence) which blocks the
  pipeline until SA:1301 serves or times out.

### 7. lb367 (scanverdict kernel = pollwave + tap): samgr wedge AGAIN,
seeded ~27s (right at the scan start), fresh parks 211s+, no
OnStart/creation. WEDGE-RATE TABLE for the coordinator (pollwake
family = 491ebc09+ lineage vs cg26b = everything else same):
- pollwake-family rolls: lb361 crawl+wedge, lb362 healthy-but-KVS-hang,
  lb364 wedge, lb365 wedge, lb367 wedge => 4 wedges / 5 rolls.
- cg26b rolls: lb358 early spiral, lb359 healthy, lb360 healthy, lb366
  healthy => 1 bad / 4 rolls.
NOT statistically decisive at these n, but the direction (samgr-
centered total-freeze wedges clustering on the parity kernel) plus
the fact that main has 491ebc09 merged warrants a targeted A/B:
5 cg26b vs 5 pollwake rolls counting wedge-class outcomes. The parity
fix is reference-correct (gate audit above); if the A/B confirms a
wedge-rate shift, the divergence to hunt is WHICH unflagged reader
class our kernel parks in BWR that the reference would not (the 26th's
own decode showed OH never polls the binder fd; the parked sys=16
shape with looper=0x0 taking proc work — lb364 tid 443 — is new
post-fix behavior that matches the reference's LETTER; check it
against the reference's actual thread states on a reference boot).

### 8. lb368 (cg26b, final roll, IN FLIGHT at session close — driver
self-terminates at 1500s) — THE SCAN LEG FIRED, then stalled mid-stream
- FIRST fresh-boot install stream since lb317 on a cg26b-class kernel:
  scan start (param 0) 19.3s; installs uid 10000-10005 at 21.3-26.5s
  (PRE-KVS — the install loop does not need the KVS to start); stream
  PAUSED 20s during distributeddata OnStart (10.9-45.9s window);
  resumed uid 10006-10008 at 46.7-50.3s once the KVS published; then
  DIED at 50.3s on distributeddata serving-capacity collapse (direct
  evidence: powermgr txn15601 -> distributeddata OS_IPC_0_1332 exec
  287s+, reply deferred userspace-side — the FFRT family's THIRD role:
  store-side serving wedge under install load).
- CreateNewUser start 52.6s READ THE PREINSTALL LIST: "create user:100
  bundle size:34" PRINTED (the campaign's first since lb317) — the
  deciding half of the race WON on this boot. But the per-user install
  loop needs the (now wedged) KVS/installs pipeline: no CreateNewUser
  end, no startUser, stream frozen at 18 SetBundleQuota lines.
  Pixels: 5 shots all 82f0a25f (frozen).
- Net: lb366+lb368 together decompose the scan leg completely:
  (a) whether the first-boot install branch fires at all is timing
  dice (lb368 fired with scan at 19.3s; lb366 didn't with 34.5s —
  earlier BMS start correlates with firing), (b) the stream's knees
  are BOTH distributeddata (its OnStart window pauses the stream; its
  serving collapse kills it), (c) with the store nominally up the
  per-user installs still ride the same store. The single lever that
  moves every knee: distributeddata speed/robustness under load — the
  per-op wall (perf lane) plus its 27-35s OnStart (fs-bound DB init).

### 9. Housekeeping
- Killed only own QEMUs (lb363/364/365/366/367 + their drivers by
  exact pgrep tag match). make test's suite QEMU self-reaped (rc=124
  post-halt kill is the wrapper's normal shape; verdict line green).
- Kernels: run/bzImage-ladder-scanverdict (e304836f, two-pass x86_64
  release; ksyms regenerated — gen-ksyms ran between the two cargo
  passes; x3 debug builds rc=0; make test green).
- Pushed: 08ae518b (TASK banking), e304836f (ams46 tap). Branch
  biofix-fieldpoll.

### Successor entry points (27th, in order)
1. THE SCAN LEG (the named root): on a healthy-cascade boot (cg26b
   class gives them most often), the initial user-0 preinstall never
   installs. The branch verdict is invisible to hilog taps (image
   level-gating) — the discriminating instrument is FS-SIDE: a gated
   probe counting opendir/getdents on the preinstall roots
   (/system/app*, cfg dirs) by tgid, plus the installs-service inbound
   txn count. If the scan dir-list runs and finds the bundles, the
   failure is in ProcessSystemBundleInstall's proxy legs (installd
   proxy acquisition); if the dir-list is EMPTY, it is our
   readdir/sandbox-pivot path — a kernel bug class. Either answer
   closes the leg's biggest fork.
2. THE WEDGE-RATE A/B (coordinator-relevant, main has 491ebc09): 5x
   cg26b vs 5x pollwake rolls, count wedge-class outcomes (samgr
   total-freeze with pending txns). 4/5 vs 1/4 is the current
   uncontrolled estimate.
3. THE RACE (unchanged recipe, quiet window load <4):
   system-lb5 + USERDATA=run/userdata-fresh2.img SMP=2 LOGLEVEL=6
   " dfx=ams46,txnhunt,fuxstuck,slowtask negmemo=1" on
   run/bzImage-ladder-scanverdict. Pixels: md5 leaving
   {5f8b704d,06b933b4,82f0a25f,23312e5b} = convert + report IMMEDIATELY.
   lb366 shows the healthy fast cascade reaches startUser at ~99s with
   the store serving — the account half is there; the scan half is
   entry point 1.
4. Host-load discipline for rolls: a 30-core stress burst (shm_test
   class) mid-roll produced the 175x rlimits spin-stuck storm on
   lb363 — check `uptime` AND `ps aux --sort=-%cpu` for >200% single
   processes right before and (if possible) during the first 60s of
   every roll; kill own roll early if the host jumps above ~5.


## TWENTY-EIGHTH-SUCCESSOR SESSION (2026-11-03 23:00-.., branch biofix-fieldpoll @ 9094970b = merged main 91fed16d)
Mission: the fs-side scan probe (the 27th's top entry point), the
distributeddata serving-collapse profile, quiet-window rolls. Outcome:
THE SCAN LEG IS DECODED END-TO-END — the preinstall store DIRECTORY
never exists on our boots (no mkdir ever fires on the chain; every
bmsdb.db create-open dies on the missing parent), while the reference
kernel on the SAME image populates it to 43 preinstalled rows in the
first 8.5s. Plus one NEW kernel-side divergence candidate: our /data
image after a hard kill cannot journal-recover (e2fsck clears inodes),
where the reference's image replays its journal clean.

### Merge + instruments (all committed, pushed at close)
- 9094970b = origin/main 91fed16d merged (TASK.md ours-by-intent).
- BMSSCAN fs-side probe (dfx=ams46 cohort, 4 commits): openat attempts
  on the scan's watched paths (bms RDB store chain, /data/app/el1,
  /system/app, /system/etc/app install_list) with R/W/C intent +
  PARENT-DIR state; per-directory first-getdents outcome; the
  access(bmsdb.db, F_OK) is-new-db probes WITH THE FILE SIZE (live
  divergence meter); mkdir attempt watch on the same set. Budgets
  800/100(sysapp)/500/64/128, OFF unless dfx=ams46.
- Rig: run/bmsdump.sh (debugfs extraction + sqlite3 row counts — NOTE
  the image root IS the /data content: paths are /service/el1/..., not
  /data/service/...); run/sp-ref-n.py (reference kernel, NOSNAP
  writable userdata, for post-boot /data state extraction).
- Rig bug fixed: mmprof=1 belongs on the KERNEL CMDLINE (env var is a
  no-op — bmsours boot 1 lost its mmprof data to this).

### MISSION 1: the reference-vs-ours preinstall store A/B (decisive)
- REFERENCE (bzImage 6.6.101, system-lb5 + fresh2 reflink copy, NOSNAP,
  150s, booted to GUI): /data/service/el1/public/bms/bundle_manager_
  service/ EXISTS (uid 5523:1000 = foundation-created via
  BundleUtil::CreateDir's recursive ForceCreateDirectory), bmsdb.db =
  1,093,632 B, preinstalled_bundle = 43 rows (com.ohos.launcher +
  com.ohos.systemui verified), installed_bundle = 42 rows. Asset kept:
  run/bms-ref.img + run/bmsref-bmsdb.db (the golden reference store).
- OURS (cg26b-class 52067191 + probe, bzImage-ladder-bmsscan26, 3
  healthy boots bmsours3/4/5): the store DIRECTORY NEVER EXISTS. Probe
  timeline on every healthy boot: RDB ctor storm at ~33-35s (access x4
  + RDWR+CREAT + RD, ~40 cycles = BMS OnStart's RdbDataManager table
  creation), scanning_apps.status 0 at the same moment, then a ~40s
  park (LoadInstallInfosFromDb's KVS read), status 1 at ~73-75s — and
  between 33s and kill: ZERO mkdirs on the bms chain, ZERO installd
  ScanDir calls (installs pid idle since ~15s, "waiting for work (no
  pending txn)" at 180s), ZERO /system/app walks, ZERO install sends.
  bmsdb.db stays ENOENT through t=105s+ (size prints), parent MISSING
  at every open. The late mkdirs at 70.7s are storage_daemon's user-100
  dirs (StartUser(100) prep) — still not the bms chain.
- ROOT: OnBmsStarting's post-KVS legs never execute their fs actions —
  the scan flow that would CreateDir the store (recursively, chown
  5523:1000 — exactly the reference's dir ownership) and install 34+
  bundles dies at/after the KVS-read leg. NOT a kernel fs bug: the dir
  is missing because USERSPACE never created it (lb368 proved this
  kernel family runs the install stream when the branch fires; the
  reference kernel creates it on the same image). NOT mount ordering,
  NOT negmemo/htree (nothing to hide — the dir was never made).
- The branch mechanism (source + timing): LoadInstallInfosFromDb's
  answer decides the flow. lb368's scan FIRED at 19.3s because BMS
  reached the KVS read while the SA:1301 proxy was still unobtainable
  (instant false -> first-boot branch -> installs). Tonight's boots
  reach it at ~33-35s with the proxy obtainable-but-slow (SA:1301
  publishes at 41-43s after its 28.5s OnStart), the read waits ~40s,
  and the flow then completes EMPTY without touching the fs (the
  40s-park + fail-fast-everything shape). The 27th's "earlier BMS start
  correlates with firing" is now instrument-confirmed as the proxy-
  obtainability race at the KVS read.
- NET: pixels still need either (a) the lb368-class early branch dice,
  or (b) the store serving fast enough that the KVS read resolves
  FALSE-FAST or with real data before OnBmsStarting gives up — both
  route through distributeddata's time-to-serve (mission 2 / perf).

### MISSION 2: distributeddata under the scan window (banked data)
- Wedge boots (bmsours 1/2): OnStart-SA:1301 NEVER finishes; main
  thread parked on a stack/FFRT-arena futex (0x7ff0007b2c00 class),
  DB pool churning sub-minute re-parks, tgidthr run=0 — the lb362/364
  family; SA inserts freeze at 12-33s; BMS parks in LoadInstallInfos-
  FromDb forever (zero BMSSCAN lines = the scan never even starts).
- Healthy boots (bmsours3/5): OnStart 28.5s (12.6->41-43s publish),
  then the store serves CLEAN (zero SLOW-TXN targeting it, threads
  idle "no pending txn" by 180s). The 33->73s cost is NOT store
  serving — it is the CLIENT-side wait (foundation's KVS read parked
  in userspace; the store was up at 43s and the read still resolved
  ~30s later). No serving-collapse reproduced tonight (the lb368
  collapse needs the install stream actually running).
- The 4s-metronome DB_TaskExecutor samgr LOAD polls (the 20th's
  cascade) present on every boot — unchanged.

### NEW: ext4 crash-consistency divergence candidate (fs guarded domain)
After a hard-killed NOSNAP boot: the REFERENCE image's e2fsck runs
"recovering journal" and lands clean (only free-count fixes); OUR image
has NO journal-recovery line, block-bitmap checksum mismatches at
debugfs open, inode checksum mismatches, and e2fsck -fy CLEARS the el1
subtree ("inode contains invalid data") — i.e., our committed writes do
not survive a hard kill the way the reference's do. This poisoned two
forensic extractions tonight (evidence destroyed by my own fsck) and
matters for ANY NOSNAP population work (written state may not survive
resets). FOLLOW-UP (controlled, cheap): minimal-rootfs boot, write+fsync
files, hard-kill, e2fsck — if a cleanly-fsynced fs still fails journal
recovery on our kernel, that is a real jbd2/metadata-csum bug class.
Suspects: journal commit not covering metadata_csum updates, or bitmap
checksums never rewritten on allocation.

### Boots this session
bmsours1 (spiral->sysrq-c 31.7s: hdf_devmgr+hiview exit-255 cascade),
bmsours2 (early samgr wedge, SA inserts froze 12.9s — scan never
started), bmsref (reference, GUI, golden store extracted), bmsours3
(healthy, scan-empty decode), bmsours4 (healthy, size-meter boot),
bmsours5 (healthy, parent-state+mkdir watch: the decisive boot),
bmsrace1 (mission-3 roll, in flight at write time). Coordinator's
ab9cg-1 cg26b A/B roll ran concurrently from this worktree (not mine —
left alone; it wedged in the DEADLOCK class at ~1319s).

### Successor entry points (28th, in order)
1. THE KVS-READ BRANCH: the sharpest un-instrumented hop is WHAT
   LoadInstallInfosFromDb RETURNS after its ~40s wait (empty-TRUE ->
   BundleRebootStartEvent no-install path vs FALSE -> first-boot path
   that would CreateDir+install). The branch verdicts are I-lines the
   image level-gates from the hilog tap; the fs-side discriminator is
   now cheap: IF the first-boot branch ever fires on a roll, BMSSCAN
   shows the /system/etc/app/install_list.json opens + the /system/app
   walk + the store-dir mkdirs. Roll until a boot shows status-0 BEFORE
   SA:1301's OpenSo (the lb368 ordering) and the probe will narrate the
   whole install stream.
2. STORE TIME-TO-SERVE (the lever both knees share): everything above
   routes through distributeddata's 28.5s OnStart + the client's 40s
   wait. The OnStart window is the 20th's load cascade (samgr/softbus/
   wifi chains) — perf-lane territory (syscost rows for the window are
   in bmsours3/5 serials).
3. EXT4 CRASH-CONSISTENCY (above): the controlled fsync-kill-fsck test
   on the minimal rootfs; if it reproduces, it is a correctness bug
   ahead of every NOSNAP population plan.
4. Race rolls: recipe unchanged (system-lb5 + fresh2 + SMP2 LOGLEVEL6
   dfx=ams46,txnhunt negmemo=1, quiet window load <4-5); the probe
   kernel (bzImage-ladder-bmsscan26) narrates the scan leg for free.
   Pixels: md5 leaving {5f8b704d,06b933b4,82f0a25f,23312e5b} ->
   convert + report IMMEDIATELY.

### Session close (28th, 2026-11-04 00:4x)
- bmsrace1 (mission-3 roll, load 4.7-6.4): WEDGE — the DEADLOCK
  spinlock-stuck class from 265s (same lock word as the coordinator's
  ab9cg-1 wedge — 0xffff88807a44f4b8-class), SA inserts frozen early;
  race tally 0/1 this session (host never gave a load<4.5 window; the
  three healthy forensic boots all told the same scan story).
  Foundation started EARLY on this boot (6.2s) but its own boot-job
  mkdirs failed err=2 at 6.5s (/data not staged yet — including
  /data/service/el1/public/database/bundle_manager_service) — the
  early-BMS-start face of the /data-staging timing, worth remembering
  for the branch dice.
- Gates on the final tree (9094970b + probe commits): x86_64 debug +
  two-pass release, riscv64, aarch64 all rc=0 (dtb_blob.S clean);
  make test ALL TESTS PASSED (run/cg28-maketest.log, PRIVATE rootfs
  copy test/rootfs-mine.img per the 17th's pattern — LTP r24's suite
  QEMU left untouched; my leftover suite QEMU killed by exact match).
- Pushed: branch biofix-fieldpoll (probe commits 923399da.. + TASK).
- Assets: run/bms-ref.img + run/bmsref-bmsdb.db (golden reference
  preinstall store), run/bzImage-ladder-bmsscan26 (cg26b+probe, the
  roll kernel), run/bzImage-ladder-bmsscan (mainline-tip+probe),
  run/bmsdump.sh + run/sp-ref-n.py (rig).

## THIRTIETH-SUCCESSOR SESSION (2026-11-04 07:00-.., branch biofix-fieldpoll @ 90083ddd+)

Mission: land/verify the 29th's uncommitted extent.rs work, verify the
jbd2 write-through campaign against the fsync->kill->fsck repro, race
rolls with the free discriminator. Outcome: THE EXTENT FIX IS THE ROOT
CAUSE OF "EVERY /data MOUNT RAN JOURNAL-LESS" — verified, committed
(90083ddd), pushed, and the OH /data now mounts with a live journal.

### The 29th's uncommitted work (recovered, verified, landed)
The 29th died after D-19 (renumbered D-20, see below) + ccprobe
archive but left extent.rs UNCOMMITTED with no TASK notes. Intent read
from its doc comments + verified against reference v6.6 ext4_extents.h
VERBATIM: ext4_ext_is_unwritten is ee_len STRICTLY > 0x8000; ee_len ==
0x8000 is a WRITTEN extent of exactly 32768 blocks. Our bit-15 test +
0x7FFF length mask turned that extent into an unwritten hole of length
0. TRIGGER CONFIRMED TWO WAYS: (a) debugfs on run/userdata-fresh2.img:
journal inode 8 = ONE extent (0-32767):2129920-2162687, Size 134217728
(128MB journal = exactly 32768 blocks — mke2fs emits the maximal
written extent); (b) new rig golden run/ccrust/cc-test-max.img (1.3GB,
feature-matched, -J size=128) reproduces the identical inode-8 shape.
Every pre-fix boot printed 'ext4: journal inode empty, disabled' AT THE
/DATA MOUNT (bmsours3/4/5: 1 line each) — init_journal's block-0
resolve hit the 0-length extent.

### Rig A/B (x86_64 debug, KVM, run/ccrust)
- PRE-fix kernel (bzImage-prefix-dbg, stash-built): cc-test-max boot
  prints 'journal inode empty, disabled'; postfsync kill -> e2fsck
  rc=4 (block-bitmap diff, group free counts, BITMAP CHECKSUM class),
  no journal recovery. THE 28th's POISONED-/data DIVERGENCE, minimal.
- FIXED kernel (bzImage-fixext-dbg): journal silent-live; postfsync
  kill rc=0 30/30 markers; midloop 2.5s rc=0 44/44; midloop 6.5s rc=0
  111/111; replay pass rc=0 (journal clean at kill — D-20 write-through
  leaves s_start 0, no replay line, expected).
- Small-journal golden (cc-test.img, 4MB journal — under 0x8000 blocks,
  the old bug never fired there): postfsync rc=0 30/30 (no regression);
  6.5s midloop rc=0 111/111 this roll; 9.0s rc=4 KEEPING the D-20
  residual (one fixable class, 157/157 markers survive replay).
- ccrun.py grew --img (override golden). ccanalyze.sh unchanged.

### In-situ OH verification (the campaign image set)
Roll boots (system-lb5 + fresh2 reflink, SMP2 LOGLEVEL6
dfx=ams46,txnhunt negmemo=1, kernel bzImage-fixext-dbg): journal-empty
line 0 per boot (was 1); /data mounts at ~2.26s. The jbd2 write-through
+ parity fixes (c917a4b6) are now actually ACTIVE on /data — the 28th's
NOSNAP-population plans inherit real journaling for free.

### Housekeeping found on the way
- config_test.rs x86_64 stack assertion was stale (81920 vs the
  deliberate 131072 of c05f13ad/e336143a): every x86_64 suite run
  failed 1095/1096 since. Fixed (7af48a38): x86_64 suite now ALL PASSED.
- The 29th's divergence entry duplicated "D-19" (timer wheel owns it in
  both companion ledgers): renumbered D-20 + max-journal evidence line
  (c71af344).
- Gates on the final tree: x86_64 debug + release, riscv64, aarch64
  all rc=0 (dtb_blob.S clean); make test ALL PASSED on riscv64 AND
  x86_64 (private rootfs copies rootfs-mine/-mine2; LTP r26's suite
  QEMU untouched; my own leftover suite QEMU killed by exact match).

### Race rolls (r30roller = run/r30roll.sh, lb380+)
Free discriminator: BMSSCAN install_list opens + bms-chain mkdirs =
branch FIRED; RDB storm ~34s + 40s park + zero mkdirs = empty
completion. TALLY (this session, kernel WITH extent fix):
- lb380: EARLY WEDGE (lb362/364 family: txn1038 hiview->OS_IPC_6_602
  hung from ~11s, SA inserts froze at 10.0s/17 inserts, BMSSCAN froze
  at 14.8s; serial alive only via txnhunt repeats). Same class as the
  28th's bmsours2/bmsrace1/ab9cg wedges (pre-fix) — not new.
- lb381: HEALTHY, EMPTY completion (RDB ctor storm 34.4s, parent
  MISSING, SA:1301 at 40.7s, 68 SA inserts).
(roll tally completed at session close below)

### Successor entry points (30th, in order)
1. THE JOURNAL IS NOW LIVE ON /data — re-run any NOSNAP population
   plan and CHECK e2fsck after hard kills: the 28th's 'image poisoned'
   extraction hazard should be gone (write-through journaling). If a
   /data image still fails journal recovery, that is a NEW class (the
   jbd2 slow path / revoke records never exercised by the cc rig).
2. Race rolls: continue run/r30roll.sh (it self-classifies; kills only
   its own QEMUs by userdata-r30-N path). Branch-fire rate so far 0/1
   healthy boots (lb381 empty; lb380 wedge). The dice unchanged: need
   status-0 BEFORE SA:1301 OpenSo (lb368 ordering) for the install
   branch.
3. The D-20 residual (mid-commit write-through window, 9.0s-class
   kills rc=4 one fixable class) is the WAL follow-up — unchanged.
4. cc-test-max.img + ccrun.py --img are the standing rig for any
   maximal-extent/journal-shape question.

### Session close (30th, 2026-11-04 08:2x)
- ROLL TALLY FINAL (r30roller, lb380-385, all kernel WITH extent fix):
  6/6 NON-HEALTHY, 0 branch-fires, 0 healthy boots.
  * lb380 WEDGE txn1038 hiview->OS_IPC_6_602, SA froze 10.0s (17)
  * lb381 INIT-CASCADE at 53.2s: /log/startup mkdir err=2 ->
    stopAllServices -> DoUmount /data -> sysrq-c (bmsours1 family,
    died mid-KVS-park after a healthy start: SA:1301 at 40.7s, 68 SAs)
  * lb382 WEDGE txn572 OS_IPC_0_382->OS_IPC_5_518, SA froze 8.2s (13)
  * lb383 WEDGE txn2247 (SA 32)
  * lb384 WEDGE txn820 (SA 18)
  * lb385 WEDGE txn1349 (SA 18)
  Host load 3.3-6.9 the whole session — never a <4.5 quiet window
  (identical to the 28th's last session). All wedges are the KNOWN
  lb362/364 early-samgr-IPC family, seen pre-fix (bmsours2, bmsrace1,
  ab9cg-1); NO new wedge class with the journal-live /data, but also
  no healthy-boot timing sample — branch-fire rate this session:
  0/0-healthy (no denominator). journal-less-lines=0 on ALL SIX boots
  (the extent fix held on every boot, wedge or not).
- Branch pushed after gates: 90083ddd (extent fix) + 7af48a38 (stale
  x86_64 stack assertion) + c71af344 (D-20 renumber + evidence); this
  TASK update closes the session. Gates: x86_64 debug + release,
  riscv64, aarch64 rc=0; make test ALL PASSED riscv64 AND x86_64
  (private rootfs copies; other lanes' QEMUs untouched throughout).
- Assets added: run/ccrust/cc-test-max.img (128MB-journal golden),
  run/ccrust/bzImage-prefix-dbg + bzImage-fixext-dbg (A/B pair),
  run/ccrust/cc-run-{prefix-max1,fixext-max1,fixext-sj1,fixext-max-ml25,
  fixext-max-ml65,fixext-sj-ml65,fixext-sj-ml90}.* (the evidence set),
  run/r30roll.sh (the roller).
- Successor priorities: entry points above stand; the dice need a
  quiet host window (roll r30roll.sh again when load <4 sustained);
  pixels unchanged (frozen classes checked every boot, 0 changes).

## THIRTY-FIRST-SUCCESSOR SESSION (2026-11-04 08:39-.., branch biofix-fieldpoll @ 164fbf4b)

Mission: NOSNAP population re-run (journal-live /data, e2fsck after hard
kills as the acceptance), race rolls on populated stores, pixels. Outcome
so far: ACCEPTANCE PASSED (twice over), the JBD2 CSUM_V3 PARITY FIX
landed (164fbf4b) — e2fsck now REPLAYS our journal instead of resetting
the jsb — and the populated-store series runs (all wedge/cascade dice
under load 2.4-6.4, no healthy sample yet).

### 1. The population substrate and the acceptance (mission 1)
- userdata-warm.img is POISONED (pre-fix era: "Inode checksum does not
  match inode" at the bms chain) — NOT usable; the reference-populated
  bms-ref.img (fsck-clean, user-100 fully provisioned, store 43 rows,
  /app/el2/100 populated, launcher installed) is the golden substrate.
- lb386 (first NOSNAP populated boot, bzImage-fixext-dbg): early wedge
  (txn574 from 11s, samgr), hard-killed, e2fsck -fy: store INTACT (43
  rows), account_list.json identical, NO el1 destruction — the 28th's
  poisoning class is GONE with the journal-live kernel. Residual fix
  classes: jsb "corrupted" + orphans + zero-dtime + one extent-block
  checksum (inode 262337 = time.db-wal, same block every kill) +
  bitmap/count accounting. ALL samples (3 boots) show the IDENTICAL fix
  list — deterministic per-boot state, not torn-write luck.
- RIG BUG (fixed in poproll2b.sh): the 30th's r30roll/poproll kill step
  used `pgrep -x qemu-system-x86_64` — the name is 17 chars, pgrep -x
  matches comm (15) and NEVER matches, so QEMUs survived their verdicts
  and two fsck samples ran against LIVE writers (tainted; the pop1 and
  pop31-0 first-fsck logs are that shape). poproll2b.sh kills via
  /proc/*/cmdline scan with verify-dead, and fscks only quiescent images.
- lb388's untainted post-kill fsck: same classes (jsb corrupt, orphans,
  262337 extent csum, bitmap). lb389 (init-cascade sysrq-c after DoUmount)
  adds an xattr-block refcount off-by-one. NET: crash consistency of the
  populated store PASSES (nothing the mission needs is lost), but the jsb
  class named a real format bug — see 2.

### 2. JBD2 CSUM_V3 parity — the jsb class root-caused and FIXED (164fbf4b)
- The OH /data journal carries s_feature_incompat 0x13 = REVOKE|64BIT|
  CSUM_V3 (mke2fs pairs metadata_csum with journal csum v3). Our jbd2
  wrote NONE of the v2/v3 checksums: write_journal_superblock mutated
  s_start/s_sequence without recomputing s_checksum; tags were emitted
  through the V1 struct regardless of feature (for v3, a 12-byte packed
  struct copied as 16 bytes — OOB stack read AND wrong layout); no
  descriptor/revoke tail csums; no commit-block h_chksum[0].
- e2fsck semantics (fetched upstream journal.c): e2fsck_journal_sb_csum_
  verify fails on the stale jsb checksum -> "journal superblock
  corrupted" -> reset_super (this was the 'one fixable class' residual
  of the cc campaign and the 28th's divergence shape). Layout verified
  bit-exact against the clean image BEFORE implementing: s_checksum
  @252 = crc32c(~0, jsb[0..1024], field zeroed); j_csum_seed =
  crc32c(~0, s_uuid) = 0x2a90e450 on this image.
- Implemented per reference 6.6 (fs/jbd2 + jbd2.h): seed + jsb verify at
  init (permissive journal-disable on mismatch — documented divergence;
  the reference fails the mount), jsb csum on every jsb write AND on
  write_clean_sb, descriptor/revoke tail csums (revoke entry capacity
  now reserves the tail), commit-block csum, per-tag csum with correct
  V3/V2/V1 layouts — data blocks are copied into the journal area FIRST
  and the tag csum runs over the exact bytes that landed (write order is
  recovery-neutral; commit block stays the atomic point). Recovery now
  verifies descriptor tails / commit blocks / per-tag csums (variant-
  aware tag parse too — the old V1 t_flags@+6 read missed LAST_TAG on
  v3 tags and overcounted every v3-journal scan).
- VERIFIED on the populated /data: csum1 boot (170s, commits 926->945,
  init-cascade sysrq-c kill): jsb bytes post-kill carry a bit-exact
  VALID checksum (host crc32c check), and e2fsck -fy RUNS JOURNAL
  RECOVERY ('正在恢复日志', s_start=70) — first time our log is
  reference-consumable. Remaining classes: orphans/zero-dtime/accounting
  ONLY. Store 43 rows intact. This closes the 28th's crash-consistency
  divergence at the format level.
- Known residuals banked: (a) extent-tree-block tails (ext4_extent_csum)
  are still unwired — csum.rs has the helpers (extent_block_csum_set)
  with ZERO call sites; namei.rs:778-791 shows the wiring pattern
  (generation-chained seed); the 262337/time.db-wal class is this gap;
  extent.rs is bug-dense, do it carefully. (b) commit timestamps stay 0
  (recovery-neutral). (c) init's jsb-csum-mismatch policy is permissive
  (reference fails the mount).

### 3. Populated-store series (poproll2b.sh, lb387+, all pre-rlimits-fix kernel)
- TALLY: lb386/387/390/391/392/393/394 wedge (SA plateaus 15-18/35/57),
  lb388 stall, lb389 init-cascade (see below), lb395 in flight at write
  time. createNewUser=0 on ALL (user-100 found — the warm path is taken,
  no recreation race). journal-less=0 on all (extent fix holds).
- lb389 decode (the cascade face, lb381/bmsours1 family): render_service
  (pid 811) exits CODE 0 silently at 38.8s -> init resets the graphic
  stack (render/allocator/composer restart) -> appspawn reaped ->
  restart-limit -> init sysrq-c deliberate crash at 47.7s. Reference
  boots this image to GUI (render_service survives); our healthy fresh
  boots (bmsours3/5, lb366) also keep it — exit(0) happens ONLY on
  cascade-family boots (lb381, lb389): symptom, not root cause. No
  hilog verdict visible (level-gated); render_service stderr shows only
  benign config warnings before the exit.
- The healthy-boot ceiling so far: 57 SA inserts + graphic stack up.
  The two userspace walls on the populated store: the samgr wedge dice
  and the render_service cascade. DFX watchdog kills leaf services
  (inputmethod, useriam) but NEVER foundation/samgr — no image-side
  restart lever exists for the wedge; kernel timers demonstrably fire
  through wedges (TXN-HANG rescans on schedule) — userspace-internal
  adjudication extends to the populated series.
- PIXELS: every shot across all boots is the frozen set (82f0a25f /
  06b933b4 classes); ZERO changes.

### 4. Cross-lane (coordinator, this session)
- Wedge-rate A/B verdict (pollwake worse, 0/5 vs 4/5) was measured
  pre-3b352cf6 (rlimits self-deadlock fix) — my populated + fresh rolls
  move to the post-fix kernel after the main sync (merge not rebase,
  per coordinator; dtb_blob.S watched) and double as the A/B re-run.
- 1784be54 (optimistic-spin holder guard) explains ftest03; ext4/htree
  wave exonerated.

### 5. Housekeeping / gates
- Gates for 164fbf4b: x86_64 debug + fresh-config rebuild, riscv64,
  aarch64 all rc=0 (dtb_blob.S clean); make test on private rootfs
  (rootfs-mine3) GREEN — "suite verdict green", wrapper rc=124 post-halt
  (the normal shape; run/cg31-maketest.log).
- Assets: run/poproll2b.sh (fixed roller), run/userdata-csum1.img +
  run/csum1-fsck.log (the fix's evidence pair), run/pop31-*-fsck*.log,
  run/pop1-fsck.log (tainted-marked), /tmp/e2fsck-journal.c (upstream
  reference used for the jsb verdict semantics).

### Successor entry points (31st, in order)
1. AFTER the main sync lands (r10: 3b352cf6 rlimits + 1784be54 spin
   guard): rebuild, swap the roller kernel (poproll2b KERNEL=env), and
   count wedge-class outcomes on the populated store — the coordinator's
   A/B re-run rides these boots. Same for a fresh-store r30roll leg
   (branch-fire tally still 0-fires/0-healthy-denominator this era).
2. The healthy populated boot should take the warm ladder (user-100
   exists -> no CreateNewUser -> activate -> startUser(100) -> launcher
   AbilityInfo in the store -> appspawn -> pixels). Watch: startUser /
   SwitchToUser / appspawn com.ohos.launcher / launcher-ready (use the
   FIXED pattern 'Handle set param.*launcher\.ready' — bare
   'launcher.ready' false-matches init's "not set" warnings).
3. EXTENT-TAIL WIRING (fs residual): csum.rs extent_block_csum_set has
   no call sites; mirror namei.rs:778-791 (read i_generation, dir_csum_
   seed, set tail before the block goes dirty) at every extent.rs
   mutation site; acceptance = the 262337-class vanishes from post-kill
   e2fsck logs.
4. The D-20 ledger entry should gain the csum-v3 line (this fix) when
   the coordinator merges; divergence to record: permissive
   journal-disable on jsb-csum mismatch (reference fails the mount).

## THIRTY-SECOND-SUCCESSOR SESSION (2026-11-04 12:45-.., branch biofix-fieldpoll @ 9096ae0d+)

Mission: roll the race on the FULLY-FIXED kernel (journal LIVE + CSUM_V3
+ rlimits + spin-guard + wake_up contract), populated-series re-run,
mission-2 instruments if wedges persist, pixels. Outcome: THE WEDGE DICE
ARE NOT DEFANGED — 8/8 populated wedges, 6/6 fresh wedges, 0 branch
fires — but the kernel side is now FORMALLY ACQUITTED as the producer:
the stress lane's pick-generation ledger returns pickgen=retired on
every lying-slot observation (R49 pick-published-never-switched NOT the
OH producer), the lying-slot census proves boot-onset transient (healthy
boots show the identical 58-60 why=c budget exhaustion by t=13-22s), and
WAKE-HEALs keep firing continuously THROUGH the wedge (lb421: 103 heals
past 180s; healthy boots go silent after ~60s) — the wake machinery is
alive and repairing while the OH userspace stays wedged. The wedge is
userspace-internal (samgr worker waits), matching the 4th successor's
TXN-HUNT adjudication, now with the pick side convicted-free.

### 0. Sync (the d453519e-class content the briefing promised)
- 428c8155: merge origin/main (ltp r26: 3cd911bd fd-close task_work
  parity + census docs). TASK.md conflict resolved --ours (lane doc
  convention, same as 661249ba).
- 02e3418e: merge origin/main AGAIN when d453519e (wake_up of a RUNNING
  target returns false — reference contract) + the coordinator's CSUM_V3
  landing (51ab2963) appeared. The first roll kernel (bzImage-fullfix-dbg,
  md5 165d1852) lacked d453519e; lb404/405 rode it (banked as the
  intermediate A/B point); bzImage-fullfix2-dbg (103670dc) carries it.
- 9096ae0d: cherry-pick of the stress lane's c6d74e01 (pick-generation
  ledger, origin/stress-port) — their instrument, used not re-invented;
  dfx-only, default-off; dedupes when stress-port merges to main.

### 1. Roll tallies (all x86_64 KVM SMP2, NOSNAP populated / fresh2,
system-lb5, dfx=ams46,txnhunt negmemo=1)
- POPULATED (bms-ref reflink), intermediate kernel (rlimits+spinguard,
  NO wake_up fix): lb404 WEDGE SA=61 (first-hang txn500
  OS_IPC_2_527->OS_IPC_1_383 @12.0s), lb405 WEDGE SA=60
  (bundles.ready fired, account phase never starts; missing SAs incl
  1301 the account/AMS family — the wedge halts samgr SA registration
  wholesale at insert ~#61).
- POPULATED, FULL fix set: lb406 WEDGE SA=1 — face=INIT-CASCADE
  (StopAllServices @78s, /log/startup mkdir err=2, deliberate sysrq-c;
  the lb381/lb389 family), lb407 WEDGE SA=0 (dark; storage_manager ->
  OS_IPC_1_349 first hang @7.0s; samgrClient regObject=null retries to
  the end), lb408 WEDGE SA=45, lb409 WEDGE SA=0 AT LOAD 2.35 (quiet
  window — load does not explain the early faces), lb410 WEDGE SA=57,
  lb411 WEDGE SA=0. TOTAL 8/8 wedge, 0 healthy, 0 startUser.
- FRESH (userdata-fresh2 reflink), FULL fix set: lb430-435 ALL WEDGE
  (el 241-270s), install_list opens 0/6, bms mkdirs 0/6 — the race
  never reaches the branch decision; branch-fire 0/6 with 0 healthy
  denominators (same shape as the 30th/31st eras).
- journal-less=0 on every boot (extent fix holds everywhere); loads at
  boot start 2.35-5.56 — wedges at ALL loads.
- PIXELS: 17 boots, every shot the frozen 82f0a25f class (dark boots
  lb406-class produce no lit shot). ZERO changes.

### 2. Mission 2 — the instruments' verdict (lb421/lb422/lb423)
- lb421 (populated, dfx+=lostpark): 58 why=c lying-slot refusals; the
  lostpark census names pid 318 x45 (naming_cpu=0/1, slotw=1 = PICK
  writer, idlebit=0 = CPU not idle) from t=6.8s — the wedge onset.
- lb422 (dfx+=leakhunt,taskdump): WAKE-SKIP-ORIGIN sites are
  wait.rs:384/199 brackets (the benign mid-schedule park class per the
  stress lane's r10 adjudication) for pid 318 x25; pid 329 = SAMGR x19
  with origin sched.rs:2677. taskdump produced NOTHING without
  dfx=watchdog (the periodic dump rides the watchdog timer — the
  AGENTS.md pairing is load-bearing).
- THE DISCRIMINATOR: healthy boots (bmsours3/5, lb366) show the SAME
  58-60 why=c refusals with the same early-exhaustion (last skip at
  t=13-22s on every boot, healthy or wedged) — the lying-slot census is
  BOOT-ONSET TRANSIENT, not the wedge producer. Dominant victim is one
  pid per boot (318/319/320 — the ±1 slot holder).
- lb423 (populated, pickgen kernel 16b6dab5): pickgen=retired on ALL
  59 LP observations, pickgen=LOST = 0 — per the ledger's design
  ("retired acquits the pick path and re-points the hunt"), R49 is
  ACQUITTED for the OH wedge.
- WAKE-HEAL time distribution: healthy bmsours5 = 1 heal <60s then
  SILENT; wedged lb410 = 25 heals spread to 300s; wedged lb421 = 177
  (103 past 180s). The wake machinery runs and repairs all through the
  wedge.
- VERDICT: the OH wedge producer is NOT in the kernel scheduler's
  pick/wake path (with these instruments' reach). Everything still
  points at samgr's userspace-internal worker waits (TXN-HANG exec
  waits, sys=202 futex, server stack = plain scheduler park). The next
  instrument that can advance this is a userspace-side wait-site census
  (name the rip/ret addr of the futex the stuck OS_IPC workers sleep
  on) — tracepoint-shaped, or re-test after the stress lane's R49 hunt
  lands a producer conviction on their own workload.

### 3. Crash consistency on real kills (the 31st's acceptance, re-run)
- All 8 populated post-kill fscks rc=1 with SMALL deterministic fix
  lists (4-11 lines: orphans, zero-dtime, bitmap/count accounting, the
  KNOWN 262337/time.db-wal extent-tail csum gap — csum.rs
  extent_block_csum_set still has zero call sites, entry point 3 of the
  31st stands).
- THE JOURNAL REPLAYS on real kills (pop31-1-fsck.log: 'superblock
  lacks needs_recovery but journal has data -> run journal: yes') —
  CSUM_V3 works against e2fsck outside the lab rig.
- Re-fsck of an already-fixed image: rc=0 clean.
- createNewUser=0 on every boot — user-100 survives every hard kill.
- Rig note: the two pop chains shared the pop31-N-fsck.log names, so
  only the last per-index file survived; the per-boot tallies live in
  run/roll-logs/poproll31.log (the authoritative record).

### 4. Gates (for the push)
- x86_64 debug rc=0 x3 (fullfix 165d1852, fullfix2 103670dc, pickgen
  16b6dab5); riscv64 fresh-config rc=0; aarch64 rc=0, dtb_blob.S clean
  (git tree clean after all builds).
- make test: ALL TESTS PASSED (private rootfs copy test/rootfs-mine5.img;
  wrapper rc=124 post-halt, the normal shape — run/cg32-maketest.log).

### 5. Assets
- run/bzImage-fullfix-dbg (md5 165d1852), run/bzImage-fullfix2-dbg
  (103670dc), run/bzImage-fullfix2-pg-dbg (16b6dab5 + pickgen).
- run/cg32-decode.sh (per-boot classifier), run/cg32-diag.sh (+ the
  inline diag2 variant: add leakhunt,taskdump), run/cg32-diag2.sh.
- run/r30roll.sh fixed (wedge detect + proc-cmdline kill).
- Serials: run/oh-lb{404..411,421,422,423,430..435}-serial.log.

### Successor entry points (32nd, in order)
1. The kernel is acquitted at the instruments' reach; the wedge hunt
   needs the USERSPACE wait-site census (rip of the futex the stuck
   OS_IPC workers sleep on) — a tracepoint in the unified framework
   (kernel/src/trace/), NOT a new bespoke dfx switch (frozen). The
   TXN-HANG sampler already walks the server's kernel stack; the
   userspace return address of the FUTEX_WAIT is one frame further.
2. If the stress lane convicts an R49 producer on their workload,
   re-run the pickgen diagnostic here (run/cg32-diag.sh KERNEL=run/
   bzImage-fullfix2-pg-dbg lbNNN) — the hook is already in our tree.
3. The dice: SA=61/60 came on the intermediate kernel; the full-fix
   set sampled SA={1,0,45,0,57,0} — no load correlation (wedge at 2.35
   and at 5.56). A true sustained-quiet window (<2) is still untested;
   roll poproll2b again if one appears.
4. The 262337 extent-tail csum gap (entry point 3 of the 31st) is the
   remaining fs residual — visible again in every post-kill fsck here.

### 36th-successor relay note for the comprehensive-sweep agent (2026-11-04 22:1x)

DYNAMIC SIDE CONFIRMATION OF THE FROM-WALK SHAPE (lb416, kernel
bzImage-cg36-bwrsnap-dbg, first bwr_snap wait-for graph; serial
run/oh-lb416-serial.log):

- Wedge-time edges: hdf_devmgr(336) --txn112412 code3--> samgr
  OS_IPC_0_339 (exec 100s+); sensors(754) --txn133993
  code0x5f504e47--> samgr main 329; ROOT inside samgr: OS_IPC_0_339
  and OS_IPC_1_349 hold interleaved EXEC/AWAIT txn stacks
  (339: [EXEC 112412, AWAIT 112415->349, EXEC 112418-from-349,
  AWAIT 112420->349...]; 349 mirror-image), ages FROZEN 100-152s.
- Mechanism (kernel/src/ipc/binder.rs ~line 8380, the
  "origin-thread" leg of the sync-send path): a nested same-proc
  sync call is delivered to the ORIGINATOR thread's own todo with
  process_todo=true, so the parked reply-waiter takes it despite a
  non-empty txn_stack. The walk iterates th.txn_stack.iter().rev()
  and BREAKS at the FIRST (innermost/newest) frame whose from_proc
  == target proc.
- AUDIT REQUEST: compare that selection rule against the reference
  binder.c from-walk (6.6) — specifically whether the reference
  breaks at the first match or walks the whole from_parent chain
  (last match = outermost originator wins), and what that means for
  the 339<->349 ping-pong unwinding. My 4-edge cap cut the
  innermost frame; bwrsnap2 (12 edges + depth + *INNER* marker) is
  rolling now (lb417+) for the deep-graph confirmation.
- Also on lb416: at wedge 51 samgr threads sit in idle proc-work
  reads; every frozen txn is thread-targeted (never touches proc
  todo), so the idle pool cannot drain it — matches the src lane's
  census-blind-spot prediction verbatim.
- VERIFIED (torvalds/linux v6.6 binder.c ~3100-3130, fetched 22:2x):
  the reference from-walk ALSO breaks at the FIRST (innermost/newest)
  match — our first-match break is parity, NOT the divergence. The
  more interesting reference guard sits directly above the walk:
  `tmp = thread->transaction_stack; if (tmp->to_thread != thread)
  { binder_user_error("got new transaction with bad transaction
  stack"); return_error = BR_FAILED_REPLY; }` — a thread whose stack
  TOP is an OUTGOING (awaiting-reply) frame gets its new sync send
  REJECTED with BR_FAILED_REPLY. Worth auditing whether our sync-send
  path enforces exactly this (the 26th-session comment mentions a
  rejection guard; verify it fires on the same predicate and returns
  the same error).

## THIRTY-SIXTH-SUCCESSOR SESSION (2026-11-04 20:5x-.., branch biofix-fieldpoll @ 83b4d6e6+)

Mission delivered on all three fronts + the src-lane queue. The
round's arc: mission-1's literal target banked (comm-inline census
rows), then the source lane's src-path.md landed mid-round and the
lane pivoted to its instrumentation queue — and the FIRST bwr_snap
wait-for graph (lb416) cracked the wedge shape open dynamically.

### 1. Instruments landed (all committed, x3 x86_64 rc=0, make test
### ALL PASSED twice: cg36-maketest.log 1310/1310 on rootfs-mine7,
### cg36-maketest2.log on rootfs-mine8)
- comm inline in FUX-STUCK rows (1533d8c9): phase B copies the parked
  task's comm; one line now names owner thread + offset + u2v caller.
- bwr_snap (queue item 3): wedge-time wait-for graph over every
  BWR-blocked binder thread (edges EXEC/AWAIT with to/from pid/tid +
  ages; idle reads summarized). Three hardening passes in-session:
  latch precedence fix (the cg35 first-fire bug made the 20s dump fire
  every 5s — 1019 task dumps on lb416), newest-12 edge window (a
  first-12 window kept the OLDEST frames and hid the blocker), depth
  + exec/await counts + *INNER* marker.
- samgr-sig early verdict in poproll2b.sh (record-bounded python re:
  serial records concatenate without newlines).

### 2. MISSION-1 BANKED — the wedge signature row, owner named
lb413: `FUX-STUCK-FRESH tgid=328 comm=OS_IPC_2_501 ... A+0x800194
fto=0x0 u2v=0x7ff000e16000+0x32e02` (samgr=pid328 via ServiceStart).
Dynamic + static agree: libc++ cv::wait = BlockUntilThreadAvailable
(src lane §2.2). Neighbor +0x8001d4 banked on lb410 (OS_IPC_9_559,
same u2v). mmi +0x8009c4 owner thread = comm=mmi-inject (lb415/417/
418/420/421, all u2v +0x32e02, fto=0). foundation +0x800b44 banked
with comm inline on lb423 (powermgr same slot, u2v +0xe998 variant).
telephony +0x8001a4: still no sample (0 hits lb396-425).

### 3. THE WAIT-FOR GRAPHS (bwr_snap first light)
- lb416: samgr OS_IPC_0_339 <-> OS_IPC_1_349 interleaved EXEC/AWAIT
  stacks frozen 100-152s under hdf_devmgr's txn112412 (code3);
  sensors -> samgr main code 0x5f504e47 30s+. 51 idle readers at
  wedge; every frozen txn is thread-targeted (from-walk), proc todo
  empty — idle pool cannot drain it. Census blind spot confirmed.
- lb418: samgr MAIN depth=3452 (main<->OS_IPC_x_350 code-3 ping-pong,
  base hdf_devmgr txn59425); ALL frames age-identical (21700ms) — the
  whole stack built within ~100ms then froze.
- lb421: main depth=1681 vs OS_IPC_0_340 depth=1680, mirror images,
  newest window alternating EXEC/AWAIT (txn68566..68577), all 21210ms;
  main's innermost = EXEC delivered-unreplied while parked in BWR.
- lb423 (SA=56): av_session -> samgr OS_IPC_0_340 code6 exec 97s
  (downstream pile-up), probe parks on powermgr/foundation.
- Verdict: the wedge is a FROZEN same-proc recursion inside samgr
  (builds 1.7-3.5k frames in ~100ms, deadlocks at the bottom), which
  exhausts the IPC pool; hicollie's probe parks (the A+0x800NNN
  family); every dependent hangs behind samgr.

### 4. Coordination with the parallel lanes
- oh-src src-path.md consumed in full (mission-1's caller named; its
  queue items drove this round's instruments).
- Comprehensive-sweep agent: my from-walk first-match hypothesis
  REFUTED (verified parity vs 6.6 myself: torvalds v6.6 binder.c
  ~3100-3130 breaks at first match too — relayed via TASK.md). ITS
  root-cause: the reference REJECTS same-process binder transactions
  at two doors (context-manager-own-process + WARN_ON proc==target
  "self transactions not allowed" -> BR_FAILED_REPLY); our lb122-era
  port allowed both. The crosswait/recursion class is structurally
  unbuildable on the reference. Its rejection fix batch lands on this
  branch; THIS lane's verification job: fixed kernel shows zero
  samgr same-proc EXEC edges, probe-park family gone/transient, SA
  progression past the ~58-62 stall, pixel watch.
- fx8 reference census (mission-2): COMPLETE (fuxref-fx8/: parks
  1.29MB, fast 28k rows, tscen 18.7k rows, 12 maps snapshots, wedge
  window covered; samgr OS_IPC = binder-read posture, matches fx7).
  RIG TRAP found: run/oh-img/vendor.img dangled (oh-svc worktree
  cleaned) — fuxref silently dropped the 6th disk and the reference
  boot crashed at 34s via init's sysrq path; symlink repointed to
  /home/william/rux-images/oh/vendor.img (md5-identical to
  oh-robot's). Other lanes using the oh-svc path will hit this.

### 5. Boots this round
lb409-425 (poproll2b on bms-ref-derived userdata, private copies;
kernels cg36-comm/cg36-bwrsnap/2/3; DFX=ams46,txnhunt,fuxstuck).
Verdicts all wedge-class (SA 0-58 variance as before); zero panics;
pixels unchanged (frozen md5 set — no PIXEL-CHANGE lines).

### 6. Successor entry points (37th, in order)
1. SYNC the sweep agent's same-proc rejection batch when it lands on
   origin/biofix-fieldpoll (it had NOT landed at this session's close;
   check git log origin/biofix-fieldpoll), rebuild, and run the
   verification boot set above (§4 bullet 2). If OH userspace chokes
   on BR_FAILED_REPLY somewhere (new crash-loop shapes), that service
   is the next fork to compare against the reference.
2. telephony +0x8001a4 u2v sample still open (grep new boots' serials
   via run/u2vbank.py).
3. The frozen BOTTOM of the recursion (why main's innermost EXEC never
   replies) stays unexplained — moot if the rejection fix lands, but
   the pre-fix serials (lb416/418/421) hold the evidence.

## COMPREHENSIVE-FIX SESSION (2026-11-04 21:5x-.., branch biofix-fieldpoll @ 4f955ec2+)

Mission (user directive "一次把所有问题搞定"): systematic one-pass audit of the
whole divergence surface — syscall census diff, binder lifecycle side-by-side,
fd readiness, timers/signals — fix everything as ONE batch, verify once.
Static work only (the 36th rolled bwr_snap dynamically in parallel).

### 1. THE ROOT DIVERGENCE (source-verified both sides, vmlinux-confirmed)
The reference bzImage (OH 6.6.101, built from oh-robot tip 19bfc7552 —
vmlinux strings verified present) REJECTS same-process binder transactions
in TWO places our kernel left open:
- Context-manager arm: "got transaction to context manager from process
  owning it" -> BR_FAILED_REPLY. Our lb122-era removal was calibrated
  against VANILLA mainline (no check there); the OH reference HAS it.
- WARN_ON(proc == target_proc): "self transactions not allowed" ->
  BR_FAILED_REPLY for any handle-targeted same-proc send (b85-era comment
  claimed 5.10 parity; the reference is 6.6 with the check).
Dynamic conviction: our wedge serials are FULL of exactly this traffic —
samgr pid 329 workers executing ADD_SYSTEM_ABILITY handlers re-entering
the registry in-process via SystemAbilityManagerClient's handle-0 proxy
(codes 2/3/12/20/30, SLOW-TXN rows pid 329 -> 329; lb430 txn6307
353->355 + txn781 355->353 = the mirror ping-pong). The from-walk then
delivered each nested call to the parked originator, building the
crosswait graph the 36th's bwr_snap caught (lb416: [EXEC from peer,
AWAIT->peer]* mirror stacks, ages 100-152s). Reference: those sends die
at the door; OH tolerates it (boots to GUI ~12s, zero WARN splats, zero
PrintErrorDetailedInfo on refu8/9). The 0x5f504e47 oneway the lb122
removal was blamed on is CROSS-PROC (lb421 SACMP pid 757/760/761->329)
plus some same-proc sends (cgf1 printed BINDER-REJECT ctxmgr-own-process
code=0x5f504e47 @5.4s) — on the reference BOTH shapes are refused at the
door and the boot still succeeds.
The from-walk selection rule itself was AUDITED PARITY (theirs: stack
head=newest via from_parent, break at first match; ours: Vec tail=newest,
.iter().rev() breaks at first match — both pick the NEWEST originator;
the "deepest" hypothesis is refuted).

### 2. Fixes landed (one batch, all committed on biofix-fieldpoll)
1. 4f955ec2 fix(binder): reference-parity same-proc rejection — BOTH
   doors restored (ctx-mgr-own-proc why=22, self-transaction why=23), in
   reference order (before the sync-send guards), with tgid refinement
   for the sc161 shared-proc model (a forked child is a fresh binder proc
   on the reference: only sender-tgid == node reg_tgid counts as self;
   replies exempt — a same-proc reply cannot exist on the reference).
   ALSO ungated the two sync-send protocol guards (todo-head-txn,
   stack-top-not-mine) — the reference rejects them unconditionally; the
   lb304-era gating existed because the same-proc sends violated the
   shape, and those now die earlier.
2. 2daa78b3 dfx(binder): bounded (40/boot) BINDER-REJECT verdict prints
   for why=22/23 so rolls can count the rejections.
3. 94c48416 fix(epoll): eventfd poll_wait registration — the ONE fd
   class missing from poll_wait_impls (unix/pipe/fifo/socket/inotify/
   binder/evdev/timerfd/signalfd/perf all present). OH's FFRT worker
   signaling rides eventfd+epoll; every wait fell back to the 10ms
   re-check slice (latency + full fd-set rescan per waiter — the same
   tax the 26th removed for binder).

### 3. Audit coverage (the rest — verified parity, no fix needed)
- Syscall surface: reference TSCEN census posture classes are
  futex_wait/BWR/epoll_pwait(281)/poll(7)/nanosleep/recvmsg(45/47)/
  devkmsg-read/rt_sigtimedwait(128). All implemented; rt_sigtimedwait's
  consume+park+lost-wakeup guard+high-res deadline verified in source;
  epoll_pwait sigmask swap LTP-covered; timerfd blocking read + poll_wait
  wired; signalfd wired.
- BC/BR command inventory matches; failed-send pushes ReturnError and
  BREAKS the write loop (reference: return_error.cmd stops the loop);
  BR_SPAWN_LOOPER gate mirrors done: (sc164 child handling documented);
  BC_FREE_BUFFER parity incl. async-slot drain; BC_INCREFS-on-0 already
  guards mgr.proc_id != proc.id; binder_close clears mgr_node on owner
  death; BR constants bit-identical.
- OH userspace BR_FAILED_REPLY handling verified in ipc source:
  OnDeadOrFailedReply -> error return, NO retry loop, NO proxy
  death-mark (only TF_STATUS_CODE 32 marks) — the rejection produces a
  clean fast error exactly like on the reference.
- NOT fixed (documented divergence): binder priority inheritance
  (binder_set_nice on delivery/reply-restore) is absent — a fairness
  feature, not a wedge producer (the crosswait cannot be broken by
  priority; the 32nd acquitted our pick path). Candidate for a later
  parity commit if wedge-throughput work wants it.

### 4. Gates
x3 builds rc=0 (x86_64/riscv64/aarch64, dtb clean); make test x86_64
ALL PASSED (private rootfs test/rootfs-cgf1.img; rc=124 post-halt =
normal wrapper shape — run/cgf1-maketest-x86.log). Roll kernel:
run/bzImage-cgf1-dbg (md5 c72ed210).

### 5. Verification (run/cgf-roll.sh, fresh2 reflinks, SMP2 LOGLEVEL6,
dfx=ams46,txnhunt negmemo=1) — rolling at write time
- cgf1 (manual verdict, v1 roller murdered cgf2/cgf3 at 15s on a
  '/proc/sysrq-trigger' chown-noise false match — v2 fixed): WEDGE at
  392s, SA frozen 53 since ~20s, TXN-HANG wifi->samgr OS_IPC_7 exec
  369s (server parked userspace futex, reply deferred userspace-side).
  **THE FACE IS CHANGED**: ZERO same-proc SLOW-TXN rows (was the
  dominant class), no mirror stacks, no DEADLOCK, no PANIC — the
  binder crosswait is dead on this kernel. Residual = the monopolist
  face (RCU-GP-BOUND flood, CPU0 qs counter frozen = non-quiescent
  CPU; fdreap grace stalls) + samgr FFRT-internal futex waits — the
  25th/32nd userspace-internal adjudication family, pre-existing
  (lb421 identical flood pre-fix).
- The 36th's parallel rolls on ITS OWN sameproc build
  (bzImage-cg36-sameproc-dbg) agree: lb424/425/426 all wedge in the
  same residual family.

### 6. TWO MORE ROOT-CAUSE FIXES from the verification loop (the 36th's puzzles)

**22fdd846 — the POOL-DRAIN fix (ERESTARTSYS parity).** OH's
BinderInvoker::StartWorkLoop aborts the work loop on ANY
TransactWithDriver error other than ECONNREFUSED/EBADF ("returned
unexpected error, aborting") and the thread EXITS. Our BWR park path
eagerly converted the wait_event_interruptible sentinel to -EINTR, so
EVERY signal touch on a parked reader killed that worker (the 36th's
"196 idle readers -> pool exhausted" puzzle: the readers were
signal-aborted out of their loops). The reference propagates raw
-ERESTARTSYS; its syscall exit RESTARTS the ioctl transparently (no
handler: rewind rip + restore orig_rax) or converts to EINTR only when
a handler frame is built. Our trap-exit machinery (signal.rs
restart_syscall_no_handler + setup_frame) already implemented both arms
for the other ERESTARTSYS users — the binder site just never fed it
the sentinel. Fixed: return -512 raw. Cross-check: the reference's
samgr pool PERSISTS (fuxref tscen fx8: 23-24 threads in EVERY 1s sweep
31s-275s). The 36th confirmed post-fix: 17 OS_IPC workers + main
persist at wedge.

**The numExecuting_ probe question (36th's second suspect) — answered
from source, no kernel bug.** GetMaxThreadNum = (16*2)/PROTO_NUM(=2) +
1 = 17 (NOT small). The pool is created in
IPCProcessSkeleton::GetCurrent() at INSTANCE CREATION (before samgr's
main enters its work loop), so OH's only true leak shapes —
LockForNumExecuting increments before its threadPool_==null return
while UnlockForNumExecuting returns before the decrement (and both
CHECK_INSTANCE_EXIT early-returns) — cannot fire in samgr's live
config. The single increment/decrement pair around HandleCommands is
unconditionally balanced. Therefore the probe parking forever means 17
handlers GENUINELY stuck inside HandleCommands in userspace FFRT
waits; the "15 idle readers" are sync-CLIENT parks. Residual wedge
engine = FFRT-internal handler waits (the 25th's userspace-internal
adjudication family; fuxstuck anywoken=0) — kernel levers: the per-op
wall (perf lane) + the pid-1 monopolist face below.

**ea7e4218 — the residual's instrument.** The OTHER residual face
(cgf1-v1 boot): WAKE-STRANDED incumbent = PID 1 (init) holding
pc=1/2/65537 for 345s+ from the SIGCHLD-reap moment (~40.6s), starving
RCU grace (RCU-GP-BOUND flood, CPU0 qs counter pinned; fdreap close
stalls snowball). Landed: one-shot dump_all_tasks("rcu-gp-frozen-cpu")
after 32 consecutive failed grace rounds — the healthy-CPU waiter
names the frozen CPU's incumbent stack on the next boot that hits this
face.

### 7. Verification state (bzImage-cgf3-dbg = all five fixes)
- make test x86_64 ALL PASSED again on the final tree
  (run/cgf3-maketest-x86.log); riscv64 + aarch64 builds rc=0.
- Roll (run/cgf-roll.sh, fresh2 reflinks, SMP2 LOGLEVEL6
  dfx=ams46,txnhunt negmemo=1): cgf1 SA=52 wedge@383s (load 9.56,
  HEALTHY-noladder shape), cgf2 SA=60 wedge@383s (load 11.83, samgr
  handlers in FFRT waits: softbus/foundation code-6 exec 358-367s,
  RCU-GP sparse not frozen — no monopolist). Both boots show ZERO
  same-proc txns, ZERO mirror crosswaits, pool persisting, guards
  firing single digits. Roll continues to cgf8 — next agent reads
  run/roll-logs/cgf-roll.log for the final rate table.
- Host load 5.8-11.8 the whole evening (36th's rolls + other lanes) —
  no quiet window; the dice numbers are load-inflated.

### 8. Final verification tally (evening session, load 3.9-11.8 — no quiet window)
Complete kernel (bzImage-cgf3-dbg): cgf1-cgf5 ALL wedge (SA 52-60
frozen 240s+), but the FACE is uniformly the post-fix residual:
- ZERO same-proc transactions on every boot (was: the dominant
  SLOW-TXN class, pid 329->329 codes 2/3/12/20/30).
- ZERO mirror crosswaits / deep stacks (the 36th confirmed: lb426/427
  zero recursion).
- Pool persists (17 workers + main; the ERESTARTSYS fix).
- The stuck engines are now uniformly: samgr handler threads parked
  INSIDE HandleCommands in userspace FFRT waits, thtodo=0, reply
  deferred userspace-side (cgf5: THREE workers on code-12
  CHECK_SYSTEM_ABILITY_IMMEDIATELY from softbus/EventRunner/netmanager,
  exec 376s+; cgf3: the hdf<->power code-1 pair; cgf2: code-6s) —
  the adjudicated userspace-internal family. Rate same as the old
  kernel's rolls under the same load, face completely changed: every
  kernel-authored wedge engine found by the audit is dead.
NEXT AGENT'S SINGLE TARGET (per the campaign method): the FFRT-handler
completion wall — why samgr's code-12 handler (and siblings) never
returns on our kernel. Levers: (a) the per-op wall inside the handler's
own syscall mix (syscost the window), (b) FFRT's own wake/queue
machinery (eventfd-poll now prompt — verify FFRT's condvar signalers
run), (c) the pid-1 monopolist face (rcu-gp-frozen-cpu dump armed,
zero fires so far this evening — that face did not recur on the
complete kernel).

### 36th session CLOSE (2026-11-05 00:0x) — sweep fixes verified; unbiased wedge rate

The comprehensive-sweep agent landed 5 commits in this shared worktree
during the round (4f955ec2 same-proc rejection, 94c48416 eventfd
poll_wait, 2daa78b3 BINDER-REJECT verdict prints, ea7e4218 rcu-gp-
frozen-cpu dump, 22fdd846 BWR -ERESTARTSYS pool-drain fix). This
lane rebuilt on the tip (bzImage-cg36-poolfix-dbg) and verified:

1. SAME-PROC DOORS FIRE: BINDER-REJECT pid=samgr tid=OS_IPC_*
   ctxmgr-own-process code=0x5f504e47 (lb427/428); the 1681-3452-deep
   frozen recursion class (lb416/418/421 graphs) is unbuildable now —
   zero same-proc EXEC edges on every fixed-kernel boot.
2. POOL PERSISTS: samgr = 17 OS_IPC workers + main alive at wedge
   (lb432 task-dump census; was 8 workers pre-ERESTARTSYS on lb428);
   BWR-SNAP idle-reads 157/271 threads at the 20s latch.
3. UNBIASED WEDGE RATE on the COMPLETE kernel, THIS lane's substrate
   (bms-ref userdata, poproll2b boots): 10/10 wedge-class verdicts
   (lb432-437 early-kill batch: wedge/samgr-sig mix; lb438-441
   SIGKILL=0 long-mode: 4/4 standard wedge detector, SA scatter
   15-62, zero milestones, zero pixel changes from the frozen set).
   The sweep's cgf1 healthy-noladder boot used its own rig — on THIS
   substrate the wedge is still certain; its remaining face matches
   the 25th's userspace-internal family (single stuck handler per
   boot kernel-side — security_guard code3 97s on OS_IPC_5, wifi/
   distributeddata code6 90-99s, render_service->composer_host
   code1 95s, ui_service probe park 166s — with the pool healthy and
   the probe tripping userspace-side; source lane's numExecuting_
   adjudication: 17 handlers genuinely inside HandleCommands in FFRT
   condvar waits; the per-op wall / FFRT handler slowness lever is
   the named suspect).
4. make test on the complete tree: ALL PASSED rc=0
   (run/cg36-maketest3.log, private rootfs-mineA); x3 x86_64 builds
   rc=0 across the round's four kernel snapshots.
5. Assets: run/bzImage-cg36-{comm,bwrsnap,bwrsnap2,bwrsnap3,sameproc,
   poolfix}-dbg; run/u2vbank.py (record-bounded offset-bank
   extractor); run/system-lb5r.img (private quiescent copy of
   system-lb5 for reference boots — recook from a QUIESCENT lb5,
   never while a NOSNAP=1 roller boot is live); fx8 census under
   run/fuxref-fx8/. The 37th's first move: read the sweep agent's
   full session note (75a90b05) + this one, then chase the per-op
   wall lever (timerfd_create storm caller STILL unnamed — entry
   point 3 of the 4th-successor notes) if the source lane confirms
   the FFRT-slowness adjudication.

## 38TH SESSION (2026-11-05 00:19-.., branch biofix-fieldpoll @ 555ec7d3+)

Mission: (1) complete+commit the 37th's uncommitted u2v2/u2v3 caller-chain
decode; (2) THE SYSCALL-BUDGET EXPERIMENT — measure the wedged handler's
syscall volume per class across its exec window, compare vs the fx8
reference census, adjudicate retry-loop vs notify-chain.

### 1. 37th's work completed and committed (x3 builds + make test green)
555ec7d3 = u2v2/u2v3 caller-chain decode + wedge-full burst. The 37th died
mid-compile: its diff compared u64 uret words against usize VMA fields
unconstrained — completed the casts, verified x3 rc=0 + make test ALL
PASSED, pushed. Also banked its run/u2vbank.py (record-bounded extractor).

### 2. Boot u2v1 (kernel 555ec7d3) — the wedge face extracted
Same substrate (system-lb5 + fresh2 userdata reflink, SMP2, KVM).
DFX=txnhunt,fuxstuck,taskdump,periodic syscost=1; new driver run/u2v-boot.py
(drives the uart-RX "DUMP!" magic via QEMU stdin — dumps fire from the RX
IRQ, the only path guaranteed to run while everything is parked; DUMP!s at
131.9/192.3/252.3/310.8s banked dump_all_tasks + binder_dfx_dump each).

**RETRY-LOOP REFUTED for the wedged handler** (the mission's question):
samgr tid 341 (OS_FFRT_2_0) executing code-6 from el5_filekey_man (txn40,
read 5.783s, reply never): TXN-HANG posture at EVERY 5s sample for 300s =
sys=202 futex_wait, stable uaddr (occasional 16B-neighbor alternation);
fuxstuck park ages grow monotonically (294s..304s), anywoken=0 always; the
uart DUMP! task series shows state=INTERRUPTIBLE(sleep) at all four dumps
and lastsys FROZEN from 131s to 310s = ZERO syscalls in the last ~180s of
its 300s exec. The handler parked ONCE and was never woken. Volume ≈
reference (which completes the whole code-6 in ms). Mission branch (a)
(RETRY LOOP as the engine) is DEAD for the parked handlers.

**The wedge face is SYSTEM-WIDE FFRT-mutex waits.** u2v2 boot (below) with
the caller chains names them: EVERY wedged handler park decodes as
libc++ std::condition_variable::wait (u2v anchor +0x32e02) called from
**ffrt::mutexPrivate::wait()** (u2v2 anchor libffrt LOAD 0x66000 +0x680c7
= vaddr 0xce0c7, the return right after cv_.wait — objdump-verified in
run/ohlibs/sweep/libffrt.so; source ffrt/src/sync/mutex.cpp:219-240,
thread-park arm for task==nullptr || Block()==BLOCK_THREAD). 480/788
continuation rows across ALL services are this class (u2v2). samgr at
wedge: SEVEN parkers on SEVEN distinct stack-node uaddrs (each waiter
parks on its OWN musl waiter-node barrier — per src-path.md the censused
uaddr IS stack-local; the A+0x8006xx/7xx/8xx "arena offsets" are per-thread
STACK tops of ~8MB anon thread stacks, NOT FFRT sync-arena slots), 15
binder threads holding in-flight txns (binder-dfx stack=1), 26 queued,
ALL sleeping except the periodic RefCountCollect (which legitimately
cycles — vruntime grows every 30s window, not a strand).

**Wake semantics**: mutexPrivate::wait waits on ctx->wn.cv; the waker is
mutexPrivate::unlock() -> wake() (mutex.cpp:212-217) — a direct futex WAKE
on the waiter's own node. anywoken=0 for 300s => the holder NEVER
unlocked. The holders are themselves asleep (all samgr threads
INTERRUPTIBLE(sleep) at the uart dumps) — holders parked mid-critical-
section in the SAME mutexPrivate family, or invisible to fuxstuck (a
BWR-parked holder is in ioctl, not futex — the census blind spot).

### 3. The syscall-volume table (u2v2, clean after the phase-shift fix)
syscost dumps were garbled on u2v1 (every dump collided with the 30s
periodic task dump; only 2 garbled headers survived). Fix 8ccf6ed6
phase-shifts the dump to 45s-mod-60 — u2v2 tables clean. Steady-state
deltas (per 60s at the wedge, 105/165/225/285s):
- recvmsg(45) +1.9M..+2.2M/60s (~35k/s, avg 28-58us = EAGAIN spin),
  recvfrom(47) ~+2M/60s (~32k/s, avg 9us = pure EAGAIN spin),
  writev(20) ~+2.3M/60s (~25k/s) — a ~90k/s socket-spin storm, STEADY
  across the wedge window; eats ~93% of accounted kernel ticks
  (ticks u/k/i 630/13464/244, 726/13464/509; softirq busy 10s/105s).
- futex(202) +~2k/60s (the parked system barely futexes — no re-park
  cycles, consistent with anywoken=0), ioctl/BWR(16) +~50/60s (binder
  frozen), nanosleep(35) +~85/60s (a few holders sleep mid-handler —
  txn545's server sys=35).
- REFERENCE CONTRAST: fx8 tscen hilogd posture = futex_wait/do_sys_poll/
  __skb_wait_for_more_packets (BLOCKING socket reads) / devkmsg_read —
  NO spin posture. OURS spins recvmsg/recvfrom EAGAIN at 60-70k/s. THE
  SOCKET-READ SEMANTICS DIVERGENCE (blocking on the reference, EAGAIN
  spin on ours) is the one 10-100x-class volume diff found. The spinners
  (task dumps: sysevent_source/os_ds_service/mmi_service RUNNING
  constantly, plus hilogd) need naming: next boot arms dfx=sysring (the
  hung-task path at 120s+ dumps the 2048-entry ring with (pid,nr,a0,ret)
  — the spin's fd + EAGAIN return names the fd and the thread).

### 4. Two kernel bugs found and fixed this round
1. fd689f3d fix(x86_64): SCRIBBLE2-GUARD diagnostic printer overflowed on
   a freed-task pid — (v >> 28) as u8 keeps bits 28-35 (not a nibble);
   garbage pid digit made b'a'+n-10 overflow u8 and the guard's own
   diagnostic PANICKED the context switch (u2v1 @316.1s during the
   KOT-KILL storm: SWITCHIN-DEAD-TASK task=0xffff888040688000 interleaved
   right before 'PANIC: attempt to add with overflow' context.rs:364).
   A wedge converted into a kernel panic by a debug printer — the boot
   died ~5s after DUMP!4.
2. 8ccf6ed6 dfx(syscost): phase-shift the dump off the taskdump boundaries.
3. 2a6358e0 dfx(futex): FUX-STUCK-U2V continuation row (the main row was
   already at the 256-byte printk record ceiling — u2v2 anchors truncated
   mid-field on u2v1) + sample cap 32->64 / walk cap 64->192 (u2v1's
   blind spot: 357 FUX-STUCK-FULL rows for sharing_service, ZERO for
   samgr tgid=329 — the samples filled with whichever buckets hashed
   first).
Gates: x3 builds rc=0, make test x86_64 ALL PASSED twice (pre- and
post-64-sample tree), pushed through 2a6358e0.

### 5. u2v2 boot (kernel 225d24f5 = 2a6358e0 tree) — chain extraction
DFX=txnhunt,fuxstuck,taskdump,periodic syscost=1, DUMP!x8. SA stuck 26-27
by 16.8s (early wedge), stuck_total=137 at 748s, hung-task fired 183.4s,
rcu-gp-frozen-cpu fired once at 32.6s (CPU0 incumbent = accountmgr pid
908 RUNNING — the monopolist face, transient here; gone by 60s).
Pixels: every shot = frozen set 82f0a25f (no change).
Park-chain classes across all services (continuation rows): 480x
ffrt::mutexPrivate::wait, 79x ffrt WorkerIdleAction (+0x3b946, the
timed deep-sleep), 88x lib+0x25b6e, 73x executables' own waits
(0x555555558000+0x1b1b), 102x lib+0x26502, 88x lib+0x3579 — per-lib
bases identified by matching the known anchors (libc++ LOAD 0x96000,
libffrt LOAD 0x66000, libhicollie 0x12000+0x91b7).

### 6. Next agent entry points (in order)
1. **dfx=sysring boot (u2v3)**: name the socket-spin threads (fd + EAGAIN
   + pid from the hung-task ring dump) and adjudicate the blocking-vs-
   EAGAIN recvmsg divergence (reference blocks in __skb_wait_for_more_
   packets; ours spins). Candidate kernel fork: O_NONBLOCK flag handling
   or our sock-wait returning EAGAIN where the reference blocks. Fixing
   the spin may also restore the CPU/kernel-tick budget the holders'
   critical sections need.
2. **The mutex holders**: samgr's 7 parkers wait on FFRT mutexes held by
   asleep mid-critical-section threads. Sharpest lead: the code-6 path's
   listenerMapLock_ critical section contains AddDeathRecipient = an
   ioctl BWR INSIDE the lock (system_ability_manager.cpp:915-941); a
   holder parked in that BWR is invisible to fuxstuck. TXN-HANG sys=16
   rows (1 on u2v2: power_host->pid338 tid340, exec only 1.4s) — extend
   TXN-HANG to print EVERY stack=1 thread's current syscall (not only
   >20s txns) so the BWR-parked holders surface.
3. The earliest-stuck-txn identification per boot (patient zero): u2v2's
   first TXN-HANG = txn545 code64 (pid449 OS_IPC_2_484 -> pid382, server
   sys=35 nanosleep). Diff our nr-35/16/45 semantics for the exact OH
   usage if it recurs.
4. accountmgr rcu-gp-frozen-cpu face fired once (32.6s, pid 908) — keep
   the ea7e4218 dump armed; it self-cleared by 60s.

### 7. Addendum (01:xx-02:0x) — the ABI decode + the named spinners

**Syscall-nr ABI decode (critical for reading every serial line):** the OH
x86_64 image calls NATIVE x86_64 numbers; x86_compat::translate maps them
onto the generic table (futex 202->98, recvmsg 47->212, recvfrom 45->207,
clock_gettime 228->113, epoll_pwait 281->22, timerfd_settime 88->86,
writev 20->66). Consequences: TXN-HANG "sys=202" = native futex (the
lastsys latch sits BEFORE translation — the lb59 "futex retry loop"
reading stands); the syscost per-nr + FLOOD tables record NATIVE nrs
(their FLOOD_NRS list is native); the SYSCOST-RING / HUNG-SYSRING rows
record the POST-TRANSLATE generic nr. The sysring has TWO record sites:
entry (ret=u64::MAX = the -1 rows — an in-flight marker, NOT a failure)
and exit (real result) — every "ret=-1" row pairs with the next row of
the same (pid, nr, a0). The u2v4 clock_gettime "50% failures" were just
entry rows; all exits were ret=0.

**HUNG-SYSRING (u2v4 @181s, first-hung latch):** fired too early (boot
storm, pre-wedge) — the one-shot latch spent itself; fixed by moving the
replay into the syscost periodic dump (de217e70, SYSCOST-RING at every
60s dump). u2v4's tail still named the FFRT timer machinery: pids
693/694 loop [epoll_pwait(epfd4) + timerfd_settime(fd11) + futex(FPRT
word)] — THE TIMERFD-STORM CALLER the 4th successor left unnamed
(entry point 3). u2v5's wedge-phase rings (285/345s): pid 588 = the same
timer loop with epoll_pwait ret=1 (INSTANTLY readable — the timerfd
rearm churn), pid 934 = writev/prctl-class traffic. Exits all 0/1 — the
loop WORKS, it is churning at loop rate (~100-300/s in the ring window),
not failing.

**The hilogd.server flood is BOOT-VARIANCE, not a constant:** u2v2
reached 6.25M recvmsg+recvfrom (28k/s each, steady across 105->285s,
kernel ticks 93%, softirq busy 10s/105s); u2v5 (same kernel family,
dfx=sysforensic added) ramped to only 78,864 and STOPPED (the reader
drained the backlog and the producers stopped). The flood = a PRODUCER
storm (writev 2.3-2.6M/60s on u2v2) consumed by the reader — consume
rate (28k/s) ~= produce rate (25k/s). The flood table's 16 comm slots
fill FIRST-SEEN, so the writev producer comms are NOT visible in it
(u2v2's visible writev slots sum to ~4k vs the 7.1M real calls) — the
producer identity needs either a bigger flood table or the ring at the
flood phase (u2v2's flood predates the ring instrument; u2v5's flood
never ramped). The reference contrast stands: fx8 hilogd.server posture
= __skb_wait_for_more_packets 72/72 (BLOCKED reader), ours = 28k/s
EAGAIN-spin when the storm branch is taken.

**Working model (all measurements in this session):** the wedge face =
FFRT-mutex wait cascade (handlers parked in ffrt::mutexPrivate::wait,
holders parked mid-critical-section or unreachable); DOWNSTREAM of a
kernel-capacity face: when the boot takes the log-storm branch, ~90k/s
kernel-bound socket ops (per-op 9-85us — the 17-72x per-op wall family)
eat >100% of kernel capacity, starving the cooperative userspace into
the park cascade. The parked handlers themselves are clean (no retry).

### 8. Next-agent entry points (38th, revised)
1. **Name the writev flood producers** (the log-storm branch): raise the
   FLOOD table slots (16 -> 64, cost = one atomic store per tracked nr)
   or dump the per-task last_syscall counters in the periodic task dump
   (the task struct already carries last_calls[96]). The flood phase
   needs catching mid-ramp (u2v2-style boot: early wedge, SA<30).
2. **The timerfd rearm-churn**: verify our timerfd_settime CLEARS the
   POLLIN readiness on rearm (Linux does: the expiry is armed fresh; a
   timerfd only reports when the CURRENT armed deadline expired). Our
   pid-588/693/694 loop with epoll_pwait ret=1 every iteration is
   consistent with a stale-readable timerfd. LTP timerfd family is
   green, so check the REARM path specifically (settime while the old
   expiry is still pending).
3. The FFRT-mutex holders: samgr's 7 parkers + 15 stack=1 binder threads
   (binder-dfx at every uart DUMP!). Extend TXN-HANG to print EVERY
   stack=1 thread's current sys (not only >=20s txns) so the
   BWR-parked holders (the futex-census blind spot) surface.
4. Candidate kernel levers for the per-op wall (the machine-capacity
   face): the fs/socket op families the perf lane named (open 85us,
   recvfrom 9us spin cost) — any fix there shrinks the storm's kernel
   share and deflates the cascade.
Assets: run/u2v-boot.py (DUMP! driver), run/bzImage-u2v{1,2,3,5}-dbg,
run/oh-u2v{1..5}-serial.log; kernels: 555ec7d3 (u2v1),
225d24f5 (u2v2), c77280f6 (u2v3/4), aaabf556 (u2v5). Gates: x3 rc=0 +
make test ALL PASSED x3 this round; pushed through de217e70.

### 9. u2v6 (timerfd-fix kernel ca4a8fbb) — verification + the quantified engine

The fix is correct parity but does not stop the wedge on this substrate:
SA stuck 26-27 (early-wedge branch again), txn2138 code-6 held 510s
(sys=202 futex park, same FFRT-mutex face), pixels = frozen set 82f0a25f
(no change). The timer churn loops no longer dominate the ring (pid 588's
loop is gone from the u2v6 tail; the 538/588 clock rows remain at modest
rate) — but the hilogd.server flood RAGED HARDER (8.72M recvmsg+recvfrom
by 285s vs u2v2's 6.25M — boot variance on the producer side).

**The u2v6 ring finally caught the flood consumer verbatim** (pid 320 =
hilogd.server): recvfrom(peek) entries+exits ret=2 (the 2-byte length
peek SUCCEEDS — records ARE queued), recvmsg entries without visible
exits in the 64-entry tail (34 entries vs 7 exits ret=60) = the consume
side runs ~0.7-1ms per record — 28k/s x ~0.7ms ~= 1 full CPU in the
recvmsg kernel path alone. Combined with writev 25k/s (the producers),
the epolls, and the clocks, the storm accounts for the measured 93%
kernel-tick saturation (ticks u/k/i 638/14212/228, 731/13874/329 at
105s — unchanged from pre-fix u2v2/u2v5).

**The quantified engine (this session's decisive answer):** the boot's
log/scan storm x OUR per-op kernel costs (recvmsg 28-58us->~0.7ms-class
on the passcred path, writev 22us, open 42-85us — the 17-72x per-op wall
family) saturates both CPUs; the cooperative FFRT-mutex userspace
starves; handlers park in ffrt::mutexPrivate::wait behind holders that
never complete their critical sections; SA progression freezes. The
parked handlers are clean (no retry — lastsys frozen, anywoken=0, park
ages monotonic). The wedge is DOWNSTREAM of the per-op wall + the
producer storm; the reference runs the same flood at ~2-5us/op and
reaches the GUI.

### 10. Handoff state (39th)
- Branch pushed through ca4a8fbb (timerfd ticks-reset fix, make test ALL
  PASSED x4 this session, x3 builds rc=0 every round). TASK.md is the
  memory; run/u2v-boot.py drives the DUMP! uart-magic boots.
- The two instruments this session added: FUX-STUCK-U2V continuation
  rows (caller chains per park) + SYSCOST-RING (fd+ret per dump, joins
  the flood table's comm attribution). REMEMBER: dfx=sysring needs
  dfx=sysforensic to actually record (the ring sites sit behind the
  SysForensic gate); the OH image uses NATIVE x86_64 nrs (translate
  maps them; TXN-HANG/flood print native, SYSCOST-RING prints generic).
- Next levers, in order: (1) the recvmsg/SO_PASSCRED per-op cost (the
  ~0.7ms-class consume) — profile our unix_recvmsg cred path; the
  reference delivers ucred at connect time and re-serves it from the
  socket, no per-record lookup; (2) name the writev producer comms
  (FLOOD table slots 16->64); (3) the FFRT-mutex holders need TXN-HANG
  to print every stack=1 thread's current sys (the BWR-parked holders
  are the futex-census blind spot); (4) the ffrt::mutexPrivate::wait
  parks are the wedge face — any CPU-capacity fix deflates them.
- Pixels: every shot every boot = frozen set 82f0a25f (no change).
- Wedge verdicts this session: u2v1 (SA 43, panic @316s = the guard
  printer overflow, fixed), u2v2 (SA 26), u2v3/u2v4 (SA 21/14, ring
  rigs), u2v5 (SA 54), u2v6 (SA 26). Unbiased rate unchanged: every
  boot wedges; the face is the FFRT-mutex park cascade under CPU
  saturation.

## 39TH SESSION (2026-11-05 02:0x-.., branch biofix-fieldpoll @ 9ca8efe6+)

Mission: the last kernel-side wall — the hilogd socket per-op cost.
(1) profile the recvmsg/writev per-op cost (the 28-58us -> ~0.7ms-class
consume vs the reference's ~2-5us), (2) fix per reference semantics
(net guarded domain), (3) verify post-fix per-op rows, wedge rate,
pixels.

### 1. The instrument: dfx unixrd=1 (c33129a1)

A filerd-shaped monotonic-ns phase probe around the three AF_UNIX hot
entries, dumped on the syscost 60s cadence + shutdown cascade:
- UNIXRD-RM (sys_recvmsg AF_UNIX): outer / iov / alloc / recv / cout /
  cmsg / addr phases + n/ok/ea/err/peek/cred counts + alloc byte sum.
- UNIXRD-RF (sys_recvfrom AF_UNIX, the hilogd 2-byte length peek):
  outer / alloc / recv / cout + peek count.
- UNIXRD-WV (sys_writev gathered-dgram, the producer side): outer /
  gather / send + ok/ea counts + gathered byte sum.
The iov snapshot + bounce alloc sit ABOVE the family dispatch (shared
code) — their sums cover all families; the storm boot is unix-dominated
so the attribution reads as an upper bound. Gated behind the unixrd=1
cmdline param (cctx tri-state pattern), zero cost when off.

### 2. THE PROFILE (u2v7: system-lb5 + fresh2, SMP2 KVM, syscost=1
### unixrd=1 dfx=txnhunt,taskdump; probe armed at the first dump t=45s)

Per-op walls, cumulative but stable across 105s -> 765s (wedge era):

- UNIXRD-RM n=5.0M (24k/s, ok=n, ea=0, cred=n — EVERY recvmsg delivers
  SCM_CREDENTIALS; hilogd sets SO_PASSCRED): outer 4.9us = cmsg 3.3us
  (68%) + alloc 1.0us (bounce buffer, allocb=73B avg) + recv 0.9us
  (engine incl. the queue lock!) + iov 0.1us + cout 0.08us + addr
  0.07us. The 38th's SO_PASSCRED lever CONFIRMED as the dominant
  consume-side phase — but the cost is OUR HEAP STAGING, not a cred
  lookup: deliver_unix_cmsgs allocated three heap objects per call
  (new_fds Vec + cmsg staging Vec with a grow + a .concat() payload
  Vec) and touched the fd table even with zero SCM_RIGHTS files.
- UNIXRD-WV n=5.9M (25k/s, ok 87%, ea 13% — producers DO hit the full
  queue): outer 17.8us = send 12.9us + gather 4.6us. The send phase is
  dominated by OUR HILOG DIAGNOSTIC TAP: every record is scanned for
  ~125 key/tag shapes (49 AMS + 23 KEYS + 12 TAGS + 10+9 spawn + 7+8
  acct + 1+6 installd + flood probe) = 8-20k byte compares per record
  x 25k/s = seconds of CPU per second. The reference kernel has no
  such per-record kernel-side tap at all.
- UNIXRD-RF n=5.2M (peek, 24k/s): outer 38-110us, recv phase 36-110us
  — but ea=1 across the WHOLE boot: the peek BLOCKS (waits for the
  next record; inter-record gap at 25k/s ~= 40us). The 38th's u2v2
  "EAGAIN spin" face and this boot's blocking face are the SAME call
  on boot-variance regimes; on THIS substrate our hilogd posture is
  already the reference's blocking posture, and the peek wall is
  sleep-to-arrival, NOT kernel burn.
- Reconciliation with the syscost per-nr table (t=165s): nr=45
  (recvfrom/peek) avg 74us (includes the blocking waits), nr=47
  (recvmsg) 9.5us, nr=20 (writev) 23us, nr=228 clock 3us at 54k/s,
  nr=281 epoll_pwait 6.3us. Kernel ticks 88-90% (c0 3295u/31525k,
  c1 4373u/30525k at 165s). rest-sites: sched pick 23.3s/165s (60k/s
  picks — the wake churn of ~90k/s socket events), percpu 14.1s,
  slab 10.8s (39M allocs), fd lookup 6.3s (21M), fdreap 3.5s.
- Wedge face on u2v7: standard (SA stuck, fdreap close completion
  stalled x7k by 820s); pixels = frozen set (no change).

### 3. THE FIXES (9ca8efe6, reference-shaped, semantics unchanged)

1. deliver_unix_cmsgs cred-only fast path: no SCM_RIGHTS files ->
   write the IDENTICAL 32-byte chain (hdr 16 + struct ucred 12 + pad
   4, cmsg_len 28, controllen reported 32) from the stack with ONE
   copy_to_user; skip the fd-table access and the three per-call heap
   objects. No-room/EFAULT semantics byte-identical to the staged
   path.
2. hilog diagnostic tap gated behind hilogtap=1 (cctx tri-state,
   default OFF — the filerd discipline "OFF by default like every DFX
   probe"): the tap is instrumentation, not kernel semantics; the
   125-scan-per-record cost on the storm path was ours alone. Boots
   needing HILOG-AMS/slog/alog/ilog/PCTL evidence arm it explicitly
   (it also carries the PCTL-SEND/RECV param tap — same class).
3. sys_writev gathered-dgram pre-reserve: one try_reserve_exact walk
   over the (already-validated) iovec lens before the copy loop —
   kills the per-iovec realloc+memcpy growth rounds (3 per hilogd
   writev).

### 4. Verification (u2v8, kernel 9ca8efe6, COMPLETE)

Same substrate, same cmdline minus hilogtap (production shape).
Matched-window A/B (u2v7 -> u2v8, per-op cumulative avgs):

- RM (consume recvmsg): outer 4944 -> 1962ns at t=165s (1874ns at
  885s); cmsg phase 3354 -> 353ns (89% cut, holds through the wedge);
  alloc 1.0us / recv 0.9us unchanged (the next levers).
- WV (producer writev): outer 17823 -> 10171ns; send phase 12932 ->
  4775ns (the tap scans WERE the send-phase fat); gather 4580 ->
  3488ns (pre-reserve; residual = the 3 copy_from_user + reserve
  bookkeeping).
- syscost per-nr: nr=47 recvmsg 9.5 -> 6us; nr=20 writev 23 -> 14us;
  nr=45 peek 74 -> 59us (still sleep-dominated).
- THROUGHPUT: the consumer now keeps pace with the storm instead of
  falling behind — u2v7 processed 5.1M recvmsg cumulative by 765s
  (~7k/s avg); u2v8 processed 31.1M by 885s (~37k/s avg, 6x). The
  per-op wall was throttling the drain rate; with it cut, the storm
  flows through at producer rate.
- WEDGE LEDGER: u2v8 still wedges — SA ceiling 53 (insert SA:3301_53),
  TXN-HANG txn3482 code12 exec 555s + txn5616 code6 547s (samgr
  workers parked sys=202 futex, thtodo=0, reply deferred userspace-
  side — the same ffrt::mutexPrivate::wait cascade), fdreap close
  stalls late. Wedge rate: 11/11 wedge-class on this substrate.
- PIXELS: every shot every boot = frozen set 82f0a25f (no change).
- Gates: x3 builds rc=0 (riscv64 fresh config, aarch64, x86_64);
  make test x86_64 ALL PASSED (run/u2v8-maketest-x86.log). Pushed
  through 9ca8efe6.

Verdict: the per-op wall is down 40-60% per op and the socket path now
drains the storm at producer rate (6x consume throughput), but the
wedge persists — the FFRT park cascade has upstreams beyond the per-op
wall (the wake/sched churn of ~90k/s events at 60k/s picks, and the
holders themselves). The mission's 10x per-op target needs the bounce
buffer removed too (alloc 1.0us + one copy: 4.9 -> ~1us = the 10x);
the wedge needs the queue-split/wake shape or the samgr-internal FFRT
hold chain broken. Both are queued below.

### 5. Handoff levers (next agent, in order)

1. The send-side wake shape: ~90k/s socket events each take
   wait_queue lock + wake_up_all on a queue that MIXES readers and
   parked senders (sched pick 60k/s at 2.3us = 23s/165s). The
   reference wakes ONE exclusive waiter per event (sk_data_ready);
   our WaitQueue already has wake_up_one + exclusive prepare_to_wait
   support — but reader/writer waits share ONE queue, so a naive
   exclusive switch risks wake cycling. The reference splits them
   (sk->sk_receive_queue.wq vs peer_wait): a two-queue split on
   UnixSocket is the correct parity move (send/recv/close sites
   enumerated in unix.rs: 1079/1113 accept, 1917 send, 2125/2166
   recv, 2213-2291 teardown).
2. The bounce buffer (1.0us/op): the recvmsg path still allocates
   vec![0u8; total] + a scatter copy (two copies per record). The
   reference copies skb->user directly (single copy). A 1-iovec
   direct-to-user unix_recv variant is the structural fix; measure
   first (the alloc is only 1.0us now, and the probe rows above are
   the A/B baseline).
3. If hilogtap=1 boots are ever needed for AMS decodes, remember the
   tap now costs ~5-8us/send while armed — arm it only on
   evidence-collecting boots, never on per-op measurement boots.
4. u2v7 artifacts: run/oh-u2v7-serial.log (probe tables at
   105/165/225/285/405/525/585/645/705/765s), run/bzImage-u2v7 source
   c33129a1. u2v8 verdicts append below.

## 41ST SESSION — CONTINUOUS ROLL (2026-11-05 04:30-, branch oh-roll @ 02e3c900)

Mission: roll OH boots continuously on the LATEST main (the 40th's three
net/unix fixes 9fe9b754 wake-budget-exclusive + 89d6df0e queue-split +
02e3c900 direct-recv were never booted), hunt pixels vs the frozen set
{82f0a25f, 06b933b4}, bank wedge/SA/phase data. Roller: run/r41-roll.sh
(100-boot loop, lb-boot.py shape, system-lb5 + userdata-fresh2, SMP2
KVM, 900s window, cmdline "dfx=ams46,txnhunt,fuxstuck unixrd=1
syscost=1"). Ledger: run/r41/ledger.csv + per-boot run/r41/<tag>-summary.txt.
Kernel: run/r41/bzImage-current = x86_64 debug @ 02e3c900.

### Boot 1 (r41v1, 04:30-04:45) — SA NEEDLE UP, WEDGE FACE CHANGED

- PIXELS: all shots frozen 82f0a25f (no change; no alert fired).
- SA: ceiling 58 (SA inserts 57, distinct 57, last insert t=34.3s) —
  vs u2v8's ceiling 53 / 52 inserts / last 28.7s. The 40th's fixes
  moved the ladder FURTHER (5 more SA registrations, 5.6s longer
  ladder). Still no CreateNewUser (wedge before provisioning, same
  class as u2v8 — which also never reached it).
- DIRECT-RECV CONFIRMED (unixrd RM): alloc 1028 -> 0ns, cout 88 -> 0ns
  (bounce gone, allocb 73B -> 1B); recv absorbs the user copy
  (964 -> 1557ns); outer 2021 -> 2544ns at the early window — net wash
  on per-op wall, one slab alloc/record dropped as designed.
- NEW WEDGE FACE (the important finding): total socket-plane freeze at
  t~60s. UNIXRD counters frozen from t=60 through t=840 (RM n=174,991;
  RF 175,085; WV 198,897) vs u2v8's continuously-flowing storm (31.1M
  recvmsg by 885s). Both CPUs 100% kernel-busy with idle FROZEN at
  314/411 ticks from t~35 to t=840.
- The burn: syscost rest-sites t=240: sched.rs:2782 (pick) n=155.9M
  tot=236s avg=1516ns — 650k picks/s (u2v8: 60k/s) — paired 1:1 with
  fdreap.rs:132 n=155.9M. fdreap.rs:132 is the `schedule(); continue`
  yield in retire_complete's completion loop: a close() caller spins
  there because PROCESSED_SEQ never reaches its ord — the retire
  reaper took the batch and never finished it. One RCU-GP-BOUND
  site=fdreap-close print at t=12.3 (gen=9419 qs=9418,9419 — cpu0 one
  quiescent report behind), then silence; the burn continues to end.
- FUX-STUCK census (first boot with fuxstuck armed): 180 lines, 7
  distinct tids over the boot, terminal singleton storage_daemon
  tgid=326 tid=348 uaddr=0x7ff0831396b4 parked 831s+ with anywoken=0
  and fto=59.99s — A 60s FUTEX TIMEOUT THAT NEVER FIRED. Park grows
  unbounded past the timeout; the timer wake for the timed wait is
  lost. (u2v8 could not see this: fuxstuck not armed; its "samgr
  workers parked sys=202" TXN-HANG face is likely the same class.)
- TXN-HANG: n=1390, codes {12:692, 1:177, 2:173, 8:173, 49:173,
  64:1, 6:1}, max exec 865.9s; comms dominated OS_FFRT_2_0 (519) +
  bootanimation/render_service/resource_schedu/wifi (173 each) — the
  FFRT park cascade, now with an explicit lost-timer-wake member.
- WEDGE LEDGER: 12/12 wedge-class (r41v1 adds one).
- READ: the 40th's wake changes plausibly interact with (a) the
  fdreap reaper's completion (schedule-yield spin needs the reaper to
  finish; the reaper's own wait/wake shape changed) and (b) timed
  futex waits (60s fto not firing). Both are for the futex/sched and
  verification lanes — this lane keeps rolling for frequency data.

### Gate notes (this session)

- x86_64 build from stock main 02e3c900: rc=0. `make test` gate on
  riscv64 (default platform) FAILS ON STOCK MAIN with a deterministic
  link error: "rust-lld: kernel image outgrew the boot.S 20MB early
  window: extend the window (both PMD tables) and move heap+
  reservation together" (run/r41-maketest-x86-main.log, log line
  21256; tree was clean at 02e3c900). The riscv64 debug+test-cfg image
  now exceeds the 20MB boot window — the merge-gate riscv64 suite run
  needs either the window extended or the test image slimmed.
- x86_64 make test verdict (run/r41-maketest-x86.log): 1313 tests,
  1312 PASS, 1 FAIL — and the 1 failure is a STALE CONFIG ASSERT on
  main, not a functional failure: kernel/src/tests/config_test.rs:55
  still asserts KERNEL_STACK_SIZE == 81920 (the interim 80KB packed
  slab), but Kernel.toml kernel_stack_size_x86_64 = 131072 since the
  nftw01 stack-overflow fix (2026-11-02; the Kernel.toml comment block
  documents the 80KB->128KB move). The failing assert then trips the
  harness panic at main.rs:1117 ("1 test(s) failed!"), which is what
  the wrapper reports as suite failure. ONE-LINE FIX for whoever owns
  it: update the assert to 131072. All 1312 functional tests green.
- BWR-SNAP (pool health) rows are ZERO on main by construction: the
  bwr_snap probe (53d2c6e8) lives on biofix-fieldpoll and is NOT
  merged. Do not read 0 as "pool healthy" on main boots.

### Rolling bank (kernel 02e3c900, updated per boot)

- r41v1: SA 58/57@34.3s | ladder NONE (wedge pre-provisioning) | FUX
  180L/7tids/terminal tid348 fto-lost 831s | TXN 1390 {12:692} max
  865.9s | sched-burn 650k/s fdreap-paired | pixels 82f0a25f x6 | 12/12
