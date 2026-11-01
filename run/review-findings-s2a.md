# review-s2-a findings ledger (syscall/ + ipc/ + signal.rs + sync/futex.rs)

Worktree: /home/william/rux-agents/review-s2-a, branch feature/review-s2-a, base 9150c6f7.
Round: full-speed sweep, 2026-10-31. Focus order: perf, functional vs reference,
lock discipline, ABI shapes.

## Fixed (commit 0e96200e)

| id | file:line | class | A/B |
|----|-----------|-------|-----|
| F1 | kernel/src/sync/futex.rs:1412-1460 | FUTEX_WAKE_OP operand decode diverged from the reference ABI (kernel/futex/waitwake.c futex_atomic_op_inuser): oparg/cmparg read as UNSIGNED 12-bit (0xfff acted as +4095, reference sign-extends to -1); op nibble masked with 0xf so FUTEX_OP_OPARG_SHIFT never worked and nibbles 8-15 were unreachable; invalid op/cmp returned EINVAL instead of ENOSYS; uaddr2==0 returned EINVAL instead of the natural EFAULT. Decode split into futex_wake_op_decode() | in-kernel rows tests/futex_test.rs::test_futex_wake_op_decode (sign-ext, shift, ENOSYS x2, GT boundary); suite 1294/1294 |
| B1 | kernel/src/ipc/binder.rs binder_transaction fixup loop + resolve_parent_ptr | offsets-array entries are sender-controlled; bounds checks `obj_off + 4 > data_size` / `parent_off + size > data_size` / `rel + len > bound` wrap at u64::MAX and pass, pointing object reads at kvirt + block_off - 4 (kernel OOB read inside the guest). All three now checked_add | build + reasoning (guest binder parcel probe deferred); no behavior change for in-range offsets |
| B2 | kernel/src/ipc/binder.rs BinderAlloc::new_buf sync arm | recycled sync block did not reset reply_done/txn: a replied-then-BC_FREE'd block (the standard server flow) carried reply_done=true into its next life and sweep_stale_async (stale = txn None && reply_done, NO grace window) reclaimed the buffer of an IN-FLIGHT transaction after >=1s — parcel data corruption across txns | unit row tests::binder_alloc_recycle (lifecycle: alloc -> deliver+reply+free -> realloc must come back clean); FAILed before the fix by construction, PASS after; suite 1294/1294 |
| B3 | kernel/src/ipc/binder.rs binder_close CLOSE probe | `CLOSE_PROBE_BUDGET.fetch_sub(1) > 0` wraps at 0 → after budget exhaustion + 1 the gate inverts and prints on EVERY service close forever | fetch_add monotone gate; hotpath_print_check stays green |

## Filed (not fixed this round)

| id | file:line | class | note |
|----|-----------|-------|------|
| P1 | kernel/src/syscall/misc.rs:617 sys_poll | perf: alloc per call | `Vec<PollFd>` of nfds allocated EVERY loop iteration (and again on every event wake) — poll is the GLib/Xorg/dbus hot path. Reference uses a stack bounce for small nfds. Fix shape: `[PollFd; 16]` stack fast path, Vec fallback. Needs x86_64+KVM A/B before merging. |
| P2 | kernel/src/syscall/misc.rs:1948 sys_epoll_wait | perf: alloc under lock | `ready_events` Vec allocated per wait round WHILE HOLDING epoll.entries (heap alloc extends the spinlock hold; ~1 alloc per ready event). Fix shape: deliver directly into small stack batches. Needs KVM A/B. |
| P3 | kernel/src/ipc/sysv_sem.rs:1239 try_apply_semops | perf: alloc per call | 3 heap Vecs per semop (work/new_vals/orig_vals) even for the uncontended single-op case (lmbench lat_sem path). Fix shape: stack arrays for small nsops/nsems, orig_vals only when SEM_UNDO present. Needs KVM A/B. |
| P4 | kernel/src/ipc/binder.rs binder_thread_read | perf: alloc per call | staged `Vec<u8>` per read (documented design divergence); plus thread.hist(EV_BWR) costs one extra WORLD acquire per write-side BWR. Low priority; OH throughput not gate-bound. |
| M1 | kernel/src/ipc/sysv_msg.rs IPC_SET | functional (minor) | perm uid/gid/mode updated BEFORE the qbytes CAP_SYS_RESOURCE EPERM — a refused call leaves metadata modified. Reference checks capability first. Not LTP-visible; fix is a 3-line reorder. |
| M2 | kernel/src/ipc/posix_mq.rs mq_timedreceive:797 | errno shape | copy_to_user return ignored: a partial user copy still returns copy_len (success). Reference returns EFAULT (message consumed either way). Same for the MSG_COPY put_user. |
| M3 | kernel/src/ipc/sysv_msg.rs msgsnd queue-full check | divergence (minor) | reference also fails when `1 + q_qnum > q_qbytes` (message COUNT cap); Rux only checks cbytes. Unreachable with default limits unless qbytes is set tiny. |
| D1 | signal.rs KSIG/TKSIG prints | print debt (pre-existing) | budget-4000 pr_err on every kernel/tkill SIGKILL+SIGTERM send; DEQUEUE(120)/SWI(120) on the delivery path. Not new (ratchet green); candidate for dfx-switch retirement once the gdm hunt closes. |

## Retracted

(none — every candidate that failed verification against the reference was dropped
before claiming; notably: futex_requeue's wake-outside-bucket-lock looked like the
R12-2 UAF class but is sound — pid_hash_remove -> synchronize_rcu -> free ordering
means the straight-line pid revalidation + wake cannot meet a freed Task; and the
binder fd/sg fixup writes were verified to run strictly outside WORLD.)

## Files read end to end this round

sync/futex.rs (4122), ipc/binder.rs (8789), ipc/sysv_sem.rs (1446), ipc/sysv_msg.rs
(1096), ipc/sysv_shm.rs (1188), ipc/posix_mq.rs (1325), ipc/access_token.rs (401),
ipc/util.rs (471 re-read), ipc/mod.rs, signal.rs (3790), syscall/signal.rs (774),
syscall/misc.rs poll/ppoll/pselect/select/epoll_wait-head/eventfd2 regions.

## Not reached (left todo in the map, honestly)

syscall/process.rs clone/wait/ptrace/reboot ladders, syscall/file.rs remainder,
syscall/io.rs remainder, syscall/time.rs remainder, syscall/sched.rs,
syscall/dispatch.rs deep pass, syscall/memory.rs remainder, syscall/mod.rs,
misc.rs signalfd4_impl remainder. These keep their map status.
