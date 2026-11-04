# oh-src lane: OH source-path analysis — from wait-site addresses to code paths

## Charter (user directive 2026-11-04)
"通过地址和 OH 的源文件，找到代码路径，判断为什么会走到这里。
OH 源代码我们都有的，直接看不要猜。" Pure static analysis: no QEMU
boots, no instruments — read the actual source and binaries; every
claim cites file:line or a disassembly offset.

## Inputs (all verified present)
- THE ADDRESS TABLE (from the oh-launcher 34th's A/B census, TASK.md
  lines ~3302-3342 of that lane): OURS-ONLY wedge parks (the reference
  kernel NEVER parks at these): samgr +0x800194 (x58, 251s — THE
  wedge signature), telephony +0x8001a4 (x46), foundation +0x800b44
  (x17; softbus same slot), mmi_uinput +0x8009c4 (x25). Reference-
  benign common parks: +0x800714 (the universal 60s/10s cycle),
  +0x7d4/+0x7e4. Call sites already u2v-decoded on our side:
  ffrt::IOPoller::PollOnce+0x802 (infinite parks),
  SExecuteUnit::WorkerIdleAction+0x157 (short cycles) — libffrt.
- OH SOURCE TREE: /home/william/workspace/oh-robot (reference kernel
  6.6.101 + full OH userspace source: base/, foundation/, etc.)
- IMAGE BINARIES: system-lb5.img (debugfs extraction;
  /home/william/rux-agents/oh-launcher/run/) — libffrt.so carries
  symbols; samgr.bin is a stripped PIE.

## Method (in order)
1. EXTRACT the binaries (debugfs from system-lb5.img): samgr.bin,
   libffrt.so, and the peers (telephony/foundation/mmi).
2. DECODE the arena offsets: the futex uaddr +0x8001xx class are
   offsets into each process's FFRT sync arena — derive the arena
   layout FROM LIBFFRT SOURCE (oh-robot foundation/ffrt or
   communication/ffrt: what sync object owns which slot/offset —
   queue condvar, semaphore, worker idle futex). +0x800194 vs
   +0x800714 = two different structures; name both.
3. MAP the call sites: libffrt has symbols — PollOnce/WorkerIdleAction
   to source file:line. For samgr.bin (stripped): disassemble its
   futex call sites (objdump) and match against the oh-robot samgr
   source (base/system/system_ability_manager or
   foundation/communication/samgr — find the OS_IPC worker loop that
   waits; optionally REBUILD samgr from source with symbols and
   pattern-match).
4. TRACE the code path: for the +0x800194 wait — which source loop
   waits there; WHO is the producer that should wake it (which
   enqueue/notify in source); what CONDITION gates the producer
   (system-ability registered? device state? param? fd event?); and
   which of those conditions depends on a KERNEL-VISIBLE input
   (syscall return, fd state, ioctl result, data content, ordering).
5. THE A/B DIFF: decode what +0x800714 (the reference-benign wait) is
   in source — the reference samgr idles THERE; ours idles at
   +0x800194. The structural difference between the two wait sites in
   source IS the fork where our execution diverges.
6. DELIVERABLE (run/src-path.md): the full source file:line chain
   (service init → the divergent fork → the wait), the producer chain,
   and a RANKED candidate list of kernel-visible inputs that could
   steer the fork — each candidate names the exact syscall/ioctl/fd
   the OH lane should instrument next. No speculation without a
   file:line.

## Session log (append per session)

### Session 1 (2026-11-04): wedge wait named; producer chain closed
- Deliverable: run/src-path.md (read it first). Summary:
  A+offsets are musl condvar waiter-node depths on the waiter's own
  8MiB stack (pthread_cond_timedwait.c:107, barrier=2 => fval 0x2).
  +0x800194 = IPCProcessSkeleton::BlockUntilThreadAvailable
  (ipc_process_skeleton.cpp:441), reached via hicollie's XCollie
  IpcCheck probe (handler_checker.cpp:43) through libc++
  condition_variable::wait (u2v +0x32e02 is in libc++.so, NOT
  libffrt — the 34th's PollOnce+0x802 binding was a stale stack word,
  0x98e02 is mid-instruction in libffrt). +0x800714 = the hicollie
  WatchdogInner tick (libhicollie 0x1b1b7, clockwait). Probe parks
  forever <=> numExecuting_ (HandleCommands bracket,
  binder_invoker.cpp:561-565) pinned at pool max; waker is
  UnlockForNumExecuting (ipc_process_skeleton.cpp:483). Ours-only
  sibling offsets = the same probe in pool-exhausted peers
  (telephony/foundation/softbus/mmi). Stuck workers sit in
  BINDER_WRITE_READ ioctls invisible to the futex census. Ranked
  next-instrumentation list in run/src-path.md §6.
- Binaries + sweep artifacts under run/ohlibs/ (extracted read-only
  from system-lb5.img).
