# oh-src lane: source-path analysis of the samgr +0x800194 wedge

Static analysis only (no QEMU, no instruments). Every claim carries a
file:line or a binary offset. Binaries extracted to `run/ohlibs/`
(debugfs from `run/system-lb5.img` of oh-launcher): samgr, libffrt.so
(/system/lib64/ndk/), libipc_single.z.so, libhicollie.z.so, libc++.so,
libeventhandler.z.so and 20 more under `run/ohlibs/sweep/`.

## 1. What the censused "+0x800NNN" offsets actually are

The A/B census recorded futex parks as (uaddr, op=0x80, val=0x2).
Decoding the notation:

- op 0x80 = `FUTEX_WAIT|FUTEX_PRIVATE_FLAG`, val 0x2 = the futex word
  holds 2 at park time.
- The uaddr is inside an 8MiB+4KiB anonymous VMA (`uvma` in the
  FUX-STUCK records; size 0x801000). These are musl pthread stacks:
  OH musl `src/thread/pthread_create.c:288-296` maps
  `guard + ROUND(stacksize + libc.tls_size + __pthread_tsd_size)`
  (16 default workers => 0x2000 guard + 0x801000 RW stack; both
  OS_IPC threads (`pthread_create(.., NULL, ..)` at
  `foundation/communication/ipc/ipc/native/src/core/framework/source/ipc_workthread.cpp:175`)
  and FFRT CPUWorkers (`pthread_attr_setstacksize` at
  `foundation/resourceschedule/ffrt/src/eu/cpu_worker.cpp:45-48`)
  get this shape on OH's musl).
- **The futex word is musl's condvar waiter node, which lives ON THE
  WAITING THREAD'S OWN STACK**: `third_party/musl/src/thread/pthread_cond_timedwait.c:80-115`:
  `struct waiter node = {0};` (automatic storage, :73), then
  `seq = node.barrier = 2; fut = &node.barrier;` (:88-89) and
  `__timedwait_cp(fut, seq, clock, ts, !shared)` (:107). That is
  literally "FUTEX_WAIT_PRIVATE on a stack word with value 2".
- Therefore **A+0x800NNN = the frame depth of that waiter node,
  measured from the waiter's own 8MiB stack mapping base**. Same code
  path => same depth => same offset (that is why common classes match
  exactly between the two kernels — identical allocation history).
  Different call chain => different depth => different offset.
- Cosmetic difference between kernels: the reference places these
  stack mappings at 0x7ff0xxxxxxxx, ours lower; only the A+ offset is
  compared, and it is kernel-independent. Not causal.

## 2. The A/B pair named

### 2.1 +0x800714 (reference-benign common; the 60s/10s cycle)

Caller: `libhicollie.z.so+0x91b7` (u2v base+off, e.g. lb399
`tgid=329 tid=348`, fto=59s+997ms / 9s+995ms). libhicollie exec page
is 0x12000, so vaddr = 0x12000+0x91b7 = **0x1b1b7**, which is exactly
the return address after `call pthread_cond_clockwait@plt` at
0x1b1b2 (verified by disassembly). This is the WatchdogInner main
loop's timed condition wait (`base/hiviewdfx/hicollie/frameworks/native/watchdog_inner.cpp`,
the `Start()` tick loop; ~60s and ~10s = different tasks' check
intervals). Every OH service runs it on both kernels — the universal
benign idle cycle.

### 2.2 +0x800194 (ours-only, THE wedge signature)

The FUX-STUCK records for samgr show (lb399 tgid=329 tid=352;
lb436 tid=889 and tid=340): `fto=0x0` (infinite), `fval=0x2`, and
`u2v=<base>+0x32e02`. The +0x32e02-class anchor is **NOT in
libffrt** (see §5 correction): the only library in the whole image
whose exec segment has a call-return boundary at +0x32e02 is
**libc++.so** (exec page 0x96000 => vaddr 0xc8e02; `test eax,eax` at
0xc8e02 immediately follows the wait call inside
`std::__1::condition_variable::wait(unique_lock<mutex>&)` — the
function spans ~0xc8d90..0xc8e2c, symbol present in .dynsym).

So the wedge frame is `std::condition_variable::wait(lock)` called
from userspace C++ code. The ONLY predicate-less (infinite)
`condition_variable::wait` reachable in every wedged process lives in
**libipc_single.z.so**, function at 0x4f905..0x4fa3e:

```
4f941: call mutex::lock@plt            ; lock(mutex_)
4f952: mov  [r14+0x64], eax            ; numWaitingForThreads_++
4f9c0: mov  ebp, DWORD PTR [r14+0x60]  ; numExecuting_
4f9c4: call 5a900                      ; threadPool_->GetMaxThreadNum()
4f9c9: cmp  ebp, eax
4f9cb: jl   4fa25                      ; numExecuting_ < max -> skip
4f9d3: call condition_variable::wait@plt   ; THE PARK
4f9d8: loop back to 4f9c0
```

This is `IPCProcessSkeleton::BlockUntilThreadAvailable()`,
`foundation/communication/ipc/ipc/native/src/core/framework/source/ipc_process_skeleton.cpp:428-446`
(the `cv_.wait(lock)` is at **:441**). Semantics: **"park forever
while every IPC worker thread of this process is busy"**.

Who calls it: exactly one importer in the entire image,
**libhicollie.z.so** (verified: no other extracted lib imports
`IPCDfx::BlockUntilThreadAvailable`). The call site is
`base/hiviewdfx/hicollie/frameworks/native/handler_checker.cpp:39-45`
— the "XCollie IpcCheck Task" probe: the watchdog posts a task that
calls `IPCDfx::BlockUntilThreadAvailable()` (libhicollie 0x12c23,
complete with the musl hook-flag dance of handler_checker.cpp:40-42
at 0x12be6-0x12c20 in the binary). It is a DFX probe whose purpose is
to detect IPC-thread-pool exhaustion; when it stalls, XCollie would
report "IPC full". In our boots it stalls indefinitely.

Full waiter-side chain (all file:line):

```
XCollie watchdog tick (watchdog_inner.cpp tick loop — the +0x800714 parker)
  -> WatchdogTask::RunHandlerCheckerTask (watchdog_task.cpp:294-299)
  -> HandlerChecker::ScheduleCheck (handler_checker.cpp:28-55)
     posts "XCollie IpcCheck Task"
  -> checkTask (handler_checker.cpp:36-47)
  -> IPCDfx::BlockUntilThreadAvailable (ipc_skeleton.cpp:366-372)
  -> IPCProcessSkeleton::BlockUntilThreadAvailable
     (ipc_process_skeleton.cpp:433-446; cv_.wait at :441)
  -> std::condition_variable::wait (libc++.so, ret addr 0xc8e02 = u2v +0x32e02)
  -> musl __pthread_cond_timedwait (pthread_cond_timedwait.c:107)
     parks on stack-local node.barrier==2  <-- censused uaddr, A+0x800194
```

Thread identity: lb399 parked thread tid=352 = `OS_IPC_3_352`
(comm from SLRD records; it read binder txns until 10.355s). lb436
parked tids 889 and 340 with byte-identical stack chains; tid 340 =
`OS_IPC_0_340`, a real binder worker (260 SLRD records). Note for
future censuses: FFRT CPUWorkers do NOT prctl-rename (no PR_SET_NAME
in `ffrt/src/eu/cpu_worker.cpp`), so they inherit the creator
thread's comm — "OS_IPC"-named threads in any census can be either
real IPC workers or FFRT workers created by one; tid 889 is likely
the latter.

### 2.3 The sibling ours-only offsets = the same probe, same cascade

- telephony +0x8001a4 (0x10 deeper than samgr's), foundation/softbus
  +0x800b44, mmi_uinput +0x8009c4: same `BlockUntilThreadAvailable`
  wait, different stack depth per process (different dispatch chain
  into the probe). Each of those processes' IPC pool is exhausted the
  same way. mmi is notable: it registered its SA with samgr at
  10.644s (SAREG txn660, lb399) and wedged in the same window.
- bluetooth +0x800824 (COMMON on both kernels) fits the same probe
  too: on the reference it parks and RECOVERS (pool drains), so it
  never appears stuck in the census windows. Same code, different
  recovery behavior — consistent with "exact-offset matches on
  common pairs prove identical allocation history".

## 3. Producer chain: who should wake the +0x800194 waiter

- Waker: `IPCProcessSkeleton::UnlockForNumExecuting()`
  (ipc_process_skeleton.cpp:447-491; `cv_.notify_all()` at **:483**).
- Who calls it: `BinderInvoker::StartWorkLoop`
  (`foundation/communication/ipc/ipc/native/src/core/invoker/source/binder_invoker.cpp:535-592`)
  brackets every dispatched command:
  `LockForNumExecuting()` at :561 -> `HandleCommands(cmd)` at :563 ->
  `UnlockForNumExecuting()` at :565. `numExecuting_` is incremented
  NOWHERE else (verified across the ipc tree).
- Therefore **the probe parks forever <=> every OS_IPC worker of the
  process is simultaneously inside `HandleCommands` and none of them
  ever finishes its transaction**. Pool size:
  `IPCWorkThreadPool::GetMaxThreadNum()` (ipc_thread_pool.cpp:160-164,
  `maxThreadNum_/PROTO_NUM + IDLE_SPAWN_ACTIVE_NUM`, with
  `DEFAULT_WORK_THREAD_NUM = 16`,
  ipc_process_skeleton.h:218) — i.e. the ceiling is ~16-21.

Crucial census blind spot: while the probe thread parks in a futex,
the stuck workers themselves are NOT in futex parks — a handler that
blocks on an outgoing sync IPC sits inside a second
`ioctl(BINDER_WRITE_READ)` waiting for `BR_REPLY`; the futex census
(and the reference-side /proc poller) cannot see those. This explains
why "only 1-2 samgr threads" show parks while the whole pool is
consumed.

## 4. The fork in one sentence

Our kernel lets samgr-family transactions stall indefinitely (some
dispatched txn never completes because a kernel-visible input — a
binder reply, a wakeup, an fd event — never arrives or arrives
wrong), the IPC pools exhaust, and hicollie's exhaustion probe parks
at A+0x800194; on the reference kernel every txn completes, pools
drain, the probe never stalls, and samgr idles only in the watchdog
tick (+0x800714).

## 5. Correction to the 34th's call-site table

- "PollOnce+0x802 hosts the infinite parks" is WRONG: libffrt+0x32e02
  (vaddr 0x98e02) is mid-instruction in the NDK libffrt (bytes of the
  rel32 of `call ReportSysEvent@plt` at 0x98e00). The 0x...8e02 stack
  words are libc++'s `condition_variable::wait` return address
  (libc++.so+0x32e02), not libffrt. Same for +0x91b7: not
  "SaveTheBbox+0x1b7" — that address is mid-instruction in libffrt
  but a real `pthread_cond_clockwait` return in libhicollie (§2.1).
- The verified libffrt anchors: WorkerIdleAction+0x157 = vaddr
  0xa1807, return after `call pthread_cond_clockwait@plt` at 0xa1802
  (sexecute_unit.cpp:156 deep-sleep wait; the 10s `wait_for` of
  :139 shares the frame) — these produce the SHORT timed cycles
  (A+0x8007d4/0x7e4), common and benign.
- `IOPoller::WaitFdEvent` (io_poller.cpp:219-227,
  `task->waitCond_.wait(lck)` at :222) is a real infinite std-cv wait
  but in libffrt's own code (would decode inside libffrt); it is NOT
  the wedge caller.

## 6. Ranked kernel-visible candidates for the OH lane

The probe's gate is pure userspace; the kernel-visible fork is
whatever makes `HandleCommands` not return. Ranked:

1. **samgr outgoing sync SendRequests that never get a reply**
   (highest value). At wedge time, dump every samgr thread's current
   syscall + in-flight txn: the stuck `ioctl(BINDER_WRITE_READ)`
   threads name the FIRST wedged peer (target proc/node + code). The
   in-tree traces to extend: SAREG (incoming), SYNCSEND (outgoing —
   lb399 already prints `SYNCSEND-EARLYRET pid=334 ... txn543 code0x1
   to pid547`) — add per-txn reply-arrival (`BR_REPLY` delivery to
   the sending thread) and a "no-reply >Ns" reporter. Kernel objects:
   binder_thread/binder_transaction; the specific syscall is
   BINDER_WRITE_READ with BINDER_SYNC_WAIT semantics on the caller
   side.
2. **The txn samgr was dispatched when the stall began.** lb399
   window: OS_IPC_3's last SLRD at 10.355 (code 6 =
   SUBSCRIBE_SYSTEM_ABILITY, samgr_ipc_interface_code.h:30); SAREG
   txn660 = multimodalinput AddSystemAbility (code 3) at 10.644;
   probe park starts ~11.06 (FRESH park=6s at 17.058). Instrument
   per-thread txn DEQUEUE (not just slow reads) for tgid=samgr in
   10.3-11.1s and resolve which handler runs >1s. AddSystemAbility
   itself only PostTasks (system_ability_manager.cpp:1264) — if it
   stalls, the stall is inside something it calls synchronously
   (`abilityStateScheduler_->SendAbilityStateEvent` /
   `RegisterDistribute` -> dbinder -> softbus, or a death-recipient
   `AddDeathRecipient` ioctl).
3. **hdf_devmgr (and peers) pool state at first-stall ordering** —
   the 32nd's TXN-HANG "txn2-from-hdf_devmgr" shape: if hdf_devmgr's
   own pool wedges first (its probe park = one of the sibling
   offsets), every caller into it hangs, cascading pools. One
   wedge-time snapshot of ALL services' in-flight BWR ioctls
   (thread -> target proc) gives the cascade root directly.
4. **fd-side inputs** for handlers that block on devices (poll/epoll
   on /dev/hdf_* nodes, /dev/binder): readiness notifications our
   kernel fails to deliver (epoll_wait wakeups, POLLIN on binder fd
   after txn enqueue). Only if (1)-(3) come back clean.
5. Cosmetic, non-causal, already explained: the stack-VMA placement
   difference (ours lower than reference) and FFRT-worker comm
   inheritance (census may over-count "OS_IPC" threads).

## 7. Method notes / reproduction

- Extraction: `debugfs -R "dump <path> run/ohlibs/<name>"
  run/system-lb5.img` (do not write to the image; it is shared).
- u2v convention check: for a lib with exec LOAD at vaddr V, a u2v
  "base+off" decodes to vaddr `(V & ~0xfff) + off`. Verified on three
  anchors: libffrt 0x66000+0x3b807 = 0xa1807 (real boundary),
  libhicollie 0x12000+0x91b7 = 0x1b1b7 (real boundary), libc++
  0x96000+0x32e02 = 0xc8e02 (real boundary); the same offsets are
  mid-instruction in libffrt, proving the per-lib base matters and
  the 34th's single-lib assumption was wrong.
- Binary facts cited above re-verify with:
  `objdump -d -M intel run/ohlibs/sweep/libipc_single.z.so` (0x4f905-0x4fa3e),
  `... sweep/libhicollie.z.so` (0x12b90-0x12c6f, 0x1b1b2),
  `... sweep/libc++.so` (0xc8d90-0xc8e2c).
- Our-side evidence: run/oh-lb399-serial.log (FUX-STUCK-FRESH
  tgid=329 tid=352; SLRD timeline; SAREG txn660; SACMP 0x5f504e47),
  run/oh-lb436-serial.log (tids 889+340 parked at A+0x800194 with
  identical chains). Reference-side:
  oh-launcher/run/fuxref-fx6/{fast.tsv,parks.tsv,maps-*.txt}.
