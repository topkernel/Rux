# phantom-bits lane — KILL the phantom preempt-bit producer

Mission (coordinator, 2026-11-01): the OH launcher's last kernel bug.
Sharpest signature: lb60 dump @169.4s — woken FFRT pair (340/341,
vruntime ~1e6) QUEUED tens of seconds while incumbent pid 552
(os_ds_service, preempt=0x100, vruntime ~1.05e9) ran; one SOFTIRQ-bit
monopolist strands the whole ladder.

## VERDICT (first successor, 2026-11-01) — the "phantom bookings" were
## LIVE brackets; two real engines found and killed

1. The 0x100 monopolist face = a task stuck INSIDE an inline
   __do_softirq window. Entry asm keeps IF=0 through the whole trap
   bracket (trap.S has no sti) → the window is UNTICKABLE (no tick →
   no PREEMPT-HEAL, no orphan scan, no preemption). Two engines:
   (a) irq_exit dispatched softirqs inline over ANY spinlock holder —
       the handler spins on the same lock the interrupted task holds,
       same CPU, unbreakable (the lb55/56 both-CPU UNIX_TABLE DEADLOCK
       family, census pc=0x101/0x102);
   (b) ethernet_poll's per-handler drain was UNBOUNDED (loopback
       request/response feeds itself during the drain; the 2 ms budget
       only checks BETWEEN vectors) — seconds-long windows (lb60 552:
       22 s+ at pc=0x100 pure).
   ZERO sub=0x100 underflows + silent SOFTIRQ-BRACKET-LEAK all boot
   proved the release never MIS-LANDED — the window never finished.

2. The +1 "phantom base counts" (was=0x10001 x674/boot heals,
   sub=+1 underflows) = the tick-side PREEMPT-HEAL's CUMULATIVE bar
   false-adjudicating LIVE guards of lock-hot tasks. pb3 evidence:
   887 heals/boot, 96% on 4 lock-hot pids (os_ds_service, mmi_service,
   ...), each healed every ~2 s ALL BOOT, every HEAL-ORIGIN line with
   oage_jif=0-1 (a FRESH 0->1 acquire inside the "2 s hold" — the base
   drains between ticks; the 100 Hz sampler never catches the gaps).
   Each false heal stripped a live guard + set need_resched → the
   holder preempted MID-SECTION → its lock stranded → spinners on both
   CPUs (the DEADLOCK cascade) → their own windows matured → cascade.
   The underflows are the stripped guards' drops landing on zero.

## Fixes (all on origin/phantom-bits)
- 29a27887 dfx: softirq-bit shadow (ssite) + tick-side BIT-ORPHAN scan
  + PREEMPT-HEAL-SOFTIRQ site line (dfx=leakhunt-gated).
- 2efa8981 dfx: per-CPU softirq window record (SOFTIRQ_WIN/T0) +
  WAKE-STRANDED census sq= line for windows >50 ms.
- 1854ec4d fix: irq_exit inline dispatch now requires base==0 (defer
  to ksoftirqd otherwise); ethernet_poll bounded at 64 pkts/source
  with NetRx re-arm; ksoftirqd yields on need_resched between windows.
- d2381bf9 fix: heal contiguity bar — per-task ti_pc_zero_jif stamps
  every REAL outermost base drain (sub path); the heal re-arms on a
  recent drain. Direct zeroings don't stamp → genuine phantoms still
  heal after a contiguous 2 s.
- (hatch) fix: 20 ms starvation bound on the deferral (SOFTIRQ_WIN_T0
  = last-dispatch stamp; force inline when starved) — bio/queue
  waiters busy-poll with lock_irqsave held and their completions ARE
  the deferred softirqs (make-test VW-WEDGE disk=0 irqs=0 caught it).
- (halt-drain) fix: make-test halt path drains pending softirqs before
  the WFI loop (ksoftirqd can never run in a pure WFI loop).

## Verification (OH image, x86_64 KVM, USERDATA=userdata-fresh2)
- pb1/pb2 (taps only): reproduced the wedge (43-55 s txn hangs,
  125 s strand, census softirq incumbents pc=0x102).
- pb3 (softirq fixes): softirq monopolist faces GONE (no sq= windows
  >50 ms, no DEADLOCK); heals/underflows UNCHANGED (273/82 @180 s) —
  the +1 family was independent → decoded as false heals (see 2).
- pb4 (ALL fixes, 1500 s full boot): heals 887→0, underflows →0,
  WAKE-STRANDED 1 (2.1 s boot transient), DEADLOCK 0, BIT-ORPHAN 0.
  The phantom-bit layer is DEAD. NOTE: the samgr/installs 30 s-class
  txn hangs PERSIST (avg 81 s @300 s window) with ZERO heals — the
  remaining hang carries NO preempt-count/monopolist face; it is a
  different (userspace futex chain / fair-pick) family for the OH lane.

## Gates
- make test: ALL TESTS PASSED (riscv64). x3 builds rc=0 (x86_64
  debug+release, riscv64 debug+release, aarch64).
- KNOWN COSMETIC: post-verdict halt-time VW-WEDGE spam on riscv64
  make-test boots (device completed, used ring full, PENDING_LAST=0,
  irqs=0 — halt races the last flush; verdict green, run.sh accepts
  the timeout kill; the halt-drain fix above addresses the deferred-
  ksoftirqd half). Base commit does not show it; 4/4 tap-carrying
  boots did pre-halt-drain.

## Rig
python3 run/lb-boot.py pbN 1500 run/system-lb.img \
  target/x86_64-unknown-none/release/bzImage \
  " dfx=wwcensus,leakhunt,txnhunt LOGLEVEL=6 SMP=2"
with USERDATA=run/userdata-fresh2.img. Images are SYMLINKS into
/home/william/rux-agents/oh-launcher/run (never copy image bytes).
One QEMU at a time; kill only ours (identify via /proc/<pid>/cmdline
bzImage path — serial is stdio so no tag in cmdline); pkill FORBIDDEN.

## Next lane notes
- The monopolist/phantom layer measured dead; OH ladder still blocked
  by the installs/samgr futex-chain hang (TXN-HANG-OUT exec>1.3e6 ms
  on pb4) — hunt from the TXN-HANG-STK taps (oh-ladder), not from
  preempt counts.
- WW3's 1 Hz census always shows pc=256/0x10000 — SELF-OBSERVATION
  artifact (the heartbeat runs inside a softirq window). Do not read
  it as monopolist evidence.
- Stretch (kpreempt=1 re-eval): with false heals dead, the false-heal
  deadlock objection (lb19b) is gone; re-trial kpreempt=1 next session.

## NINTH-SUCCESSOR SESSION (2026-11-01 13:00-, branch oh-ladder @ 618bf480)

### lb61 (main a8fae0b7, release, fresh2): THE EMPTY-BUNDLE WALL IS DEAD
With ALL kernel monopolists dead, the ladder advanced further than every
prior boot in campaign history:
- 116.868 CreateNewUser(100) start; 116.875 **"create user:100 bundle size:34"**
  — the set is NON-EMPTY (the first boot ever to print this line; the
  preinstall rdb read worked: the 8th successor's empty-set early leg
  never fired). The 3x10s futex chain did NOT recur in this window.
- 117.2 SetBundleQuota com.ohos.launcher uid 20010010
  /data/app/el2/100/base/com.ohos.launcher — per-user install stream ran.
- 135.709 OnCreateNewUser wait complete (34 bundles, 18.9s); bundles.ready;
  135.730 CreateNewUser end SUCCESS.
- 135.973 USER_UNLOCKED services start; 135.876 lockstatus_100 SET.
- Storage: /data/service/el2/100 restorecon + quotas all clean.
NOTE (8th successor's warm-play postmortem): the lbwarm population boot
never persisted — userdata-warm.img mtime == its copy time; the boot ran
with snapshot=on (NOSNAP not exported). The warm play remains UNTESTED.

### The NEW wall: foundation's IOsAccount proxy death-mark (lb61, bracketed)
- 135.984 startUser in service:100 -> 135.986 "userId not exist" (2ms).
  AMS46 ripple: between the StartUser READ (135.982) and the failure
  callback (135.994, txn14525 oneway ohos.aafwk.UserCallback) foundation
  sent ZERO osaccount queries — CheckUserParam's IsExistOsAccount never
  left the process = SendRequestInner's IsObjectDead early-return on a
  PERMANENTLY death-marked proxy (6th successor's decode, confirmed).
- The mark: accountmgr (pid 922) authored a storm of status-32
  TF_STATUS_CODE replies 100.0-102.1s (right after SA 200 publish at
  99.98); 101.927 the first "failed, handle:10 error:32 desc:*.IOsAccount"
  print (the client's marking moment, IsPrint first-error rule).
- Server-side IsOsAccountExists is a FILE check (user_info_base/<id>/
  user_info.json) returning ERR_OK always — the 32s come from OTHER
  stub methods (see lb62 decode below).

### lb62 (p32tap kernel: POISON32-DELIVER names the 32's client) — the poison map
New tap (commit 618bf480) prints at the reply-origin site: server,
CLIENT pid/tid, and the ORIGINAL request code of every status-32 reply:
- 133.8-135.2: auth hosts (face 558/556, pin 772) poison USERIAM's
  executor proxies (client 677, req_code=0x1).
- 139.0-140.4: accountmgr (959) replies 32 to NINE clients — and at
  **140.187/140.188 client pid=887 (foundation) req_code=0x42
  (COMMAND_GET_FOREGROUND_OS_ACCOUNTS) + req_code=0x35
  (COMMAND_QUERY_ACTIVE_OS_ACCOUNT_IDS)** — foundation's osaccount-kit
  polling queries hit accountmgr mid-init, got 32, and the proxies were
  permanently death-marked. 30-100s later the ladder dies on the same
  marked proxy. The cascade: auth hosts -> useriam -> accountmgr ->
  foundation (each level's stub propagates its dependency's 32 up).
- lb62 was the slow variant (no CreateNewUser by 253s), killed early;
  the poison map was complete.

### The fix (image-side, private copy): system-lb3.img
- Patched /system/lib64/platformsdk/libipc_single.z.so (the framework lib
  every OH process loads; verified the image copy is BIT-IDENTICAL to
  oh-robot's out-tree build): in IPCObjectProxy::SendRequestInner the
  `if (status == ERR_DEAD_OBJECT) SetObjectDied(true);` leg is REMOVED —
  a TF_STATUS_CODE reply authored by a server stub error must not
  permanently death-mark the caller's proxy. The REAL death path
  (SendObituary, kernel death notice) still marks. OH-source edit was
  rebuilt via `ninja -w dupbuild=warn ipc_single` (prebuilt python on
  PATH) and REVERTED after extraction (oh-robot tree left pristine;
  out-tree .so restored to the original md5 fb4a215b...).
- system-lb3.img = system-lb.img + ONLY this .so (reflink copy + debugfs
  write; md5-verified round trip 072dbb39...).
- lb63 = system-lb3.img + p32tap kernel, 2400s. Watch: no permanent
  poison, CheckUserParam passes, SwitchToUser, appspawn com.ohos.launcher,
  pixel md5 != 5f8b704d.
