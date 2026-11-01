# review-s2b round findings (2026-10-31 / 11-01)

Worktree /home/william/rux-agents/review-s2-b, branch feature/review-s2-b
(off main 9150c6f7). Domain: kernel/src/fs/ (all) + kernel/src/net/ (all).

Per-commit verification: `make build` + `make build-x86` rc=0; behavior
A/B via guest probes (test/inet_peek.c + test/inet_peek_boot.py driver,
x86_64 boot; the probe sources ride in test/probes.tar.gz — repacked
after each addition). run/s2b-base-bzImage is the clean-main kernel each
A/B "BEFORE" ran on.

## Fixed this round (7 commits)

- **F7 MSG_PEEK ignored on inet sockets** (4edca91e) — recvfrom/recvmsg/
  recvmmsg with MSG_PEEK consumed the datagram/stream bytes on
  AF_INET/AF_INET6 (only AF_UNIX honored it). Peek plumbed through
  socket_recv_ctl -> Socket::recv -> TcpSocket::recv (head copy, no
  window update/ACK) / UdpSocket::peek_packet / the VFS UDP queue arm.
  A/B: 5/5 FAIL -> 5/5 PASS (two peeks return the same datagram,
  truncated peek keeps it queued, TCP peeks return the same prefix).

- **F10 shutdown() ENOTCONN** (219fce00) — shutdown on a never-connected
  TCP socket returned 0 and a SHUT_WR on a listener killed the listener's
  protocol slot (tcp_sock.close default arm -> TCP_CLOSE).
  socket_shutdown_check() encodes the reference inet_shutdown rules.
  A/B: shutdown-unconnected / shutdown-listen-wr FAIL (returned 0) ->
  PASS (-1/107); listener SHUT_RD stays 0.

- **F6 sockaddr_un/sockaddr_nl write-back overruns** (b8a38043) — both
  helpers wrote the full kernel struct ignoring the caller's *addrlen
  (the F2 move_addr_to_user divergence, unix/netlink variants); the
  netlink getsockname arm also EINVAL-rejected declared lengths < 12
  (the reference has no floor). A/B: canary after an 8-byte unix object
  CLOBBERED -> intact, stored lengths 10/12 correct.

- **F11 netlink portid routing** (922e158b) — bind(2) ignored a chosen
  nl_pid; unicast sends to a port id fell into the rtnetlink parser
  (EINVAL). NETLINK_PORTID_TABLE (proto, portid -> Weak socket),
  netlink_bind adopts nl_pid (EADDRINUSE on collision),
  netlink_send_unicast delivers verbatim / ECONNREFUSED on a miss.
  A/B: pid reported 1 -> 5555; unicast 18/18 bytes verbatim; refused
  errno 22 -> 111; collision 0 -> 98.

- **F8/F9 zero-length messages + EAFNOSUPPORT** (6a1e7eb7) — three
  swallow-gates: sys_sendmsg returned 0 for an empty iovec set WITHOUT
  transmitting (musl's sendmmsg fallback loops over sendmsg, so a
  leading empty message in a batch was reported sent and never crossed
  the wire); the sendmmsg/recvmmsg batch loops broke at a 0-length
  message; Socket::recv's UDP arm CONSUMED a real zero-length datagram
  and then reported EAGAIN. Plus sendmmsg's per-message msg_name parse
  silently unaddressed bad families (now EAFNOSUPPORT like sendmsg).
  A/B: an [empty,'x'] sendmmsg pair delivered only 'x' (empty never on
  the wire) -> both delivered in order (recv 0, then 'x', then EAGAIN);
  recvmmsg-zerolen-count FAIL -> PASS.

- **Lock audit: UDP xmit under UDP_TABLE_LOCK** (70a1af5d) —
  udp_send/udp_sendto/udp_sendto6 held the irqsave table spinlock across
  ipv4_send_src_ttl/ipv6_send_hops, i.e. across the virtio-net xmit
  completion wait — the R34/R35 wedge class TCP fixed with TcpTxBatch.
  Build/emit split (udp_build_locked* -> BuiltUdp::emit after the guard
  drops). No wire behavior change; inet_peek probe green on the rebuilt
  kernel.

- **devfs stat/lseek size under MAP_SHARED** (d1275bce) — getattr and
  SEEK_END sized regular files from entry.data, which is deliberately
  stale while a SharedFilePages backing is active (and after a
  shrink-ftruncate under one) — the file had two visible sizes (the mmap
  EOF gate serves the authoritative entry.size). A/B: after a
  shared-mapping write at offset 100, fstat/SEEK_END reported 5 ->
  110/110.

## Perf levers pulled (5542a7b6 follow-ups; uncommitted work-in-progress
## at ledger time — see "status" below)

1. **Lock-free VFS root read** — VfsState.root_dentry is write-once
   (vfs::init + the lazy ensure arm; "/" overlays replace the root's
   INODE, never the dentry), so get_vfs_root() now serves an
   Acquire-load of a deliberately-leaked pinned pointer
   (VFS_ROOT_PINNED) and falls back to the locked slot only before boot
   publishes. Removed the walk's last global-lock pair in the prologue.
2. **Close-side trims** — (a) epoll_purge_closed_fd fast-outs on a live
   EPOLL_INSTANCES counter == 0 (the per-close for_each_file irqsave
   walk was pure overhead for epoll-less workloads — lmbench's open case
   paid it on every close); (b) FdTable::park_and_drain skips
   drain_retired's emptiness re-check when the caller just parked.

   A/B (x86_64+KVM strong profile, interleaved base/fixed boots,
   run/abx-s2b.txt, one case per boot; host load 6-27 during the sweep —
   above the lane's formal <6 validity bound, so directional evidence
   only): lat_syscall open base 1.2685 us vs fixed 1.1246 us med-of-3
   (**-11.3%**, fixed < base in all 5 completed pairs; the two levers
   target exactly this case). stat pairs hover at parity as expected
   (only the root-pin lever touches it, ~2%); fstat control flat.

   NOT pulled: the lookup_child BTreeMap-under-spinlock (~39 ns/comp)
   lever — the lock-free designs considered (RCU-published snapshot,
   seqlock memo) all need deferred Arc reclamation machinery sized
   against the mutation-heavy workloads (tar/pbench fs rows); left for a
   dedicated lane rather than risked inside this sweep.

## Verified already-fixed (filed items)

- **F5 loopback forked TCP connect wedge** — CANNOT reproduce on current
  main (9150c6f7): the sockaddr_clamp accept case (forked child,
  blocking connect, nonblocking accept poll) passes 3/3 x86_64 boots
  (fd accepted, clamp intact). The 8-round loopback drain + blocking
  wait in Socket::connect that landed after the finding resolves it.

## Files read end to end this round (review_coverage.py updated)

net/tcp.rs (4776, done — no new defects), net/unix.rs (2965, done —
put_sockaddr_un fix), net/udp.rs (fixed — lock + peek defects),
net/netlink.rs (fixed — F6/F11), net/socket.rs (fixed — F7/F10/F8),
fs/devfs/mod.rs (2411, fixed — shared-map size). fs/vfs.rs walk/dispatch
re-read around the perf levers (lock-free root read); fs/pipe.rs,
fs/fs_struct.rs, fs/file.rs FdTable, fs/dentry.rs spot-read from the
lock audit. Remaining fs/ + net/ files keep their prior status in the
coverage map (net/* small files spot-read for the audit).

## Lock findings (audit, fs/ + net/)

- FIXED: UDP_TABLE_LOCK-across-xmit (70a1af5d, above).
- Sound: EXT4_BIG_LOCK / VFS_MUTATION_LOCK are SleepMutex (bd9c0881);
  pipe waits use prepare/re-check/schedule/finish outside the buffer
  lock; FdTable reader banks + drain quiescence protocol consistent;
  unix/tcp/netlink wait engines all drop locks before sleeping.
- Notes (no fix, ledger only): (a) Pipe::tee_into allocates the staging
  Vec under the pipe buffer spinlock (bounded by len; R35-discipline
  class); (b) BTreeMap inserts under spinlocks (my NETLINK_PORTID_TABLE,
  VFS_STATE's mount_chain_index) allocate under lock — consistent with
  existing tree practice (mount path), flagged for the alloc-discipline
  lane; (c) udp_rcv's UdpPacket Vec::from(data) allocates under
  UDP_TABLE_LOCK in softirq context (pre-existing, same class).

## Gates

- Dual-arch build rc=0 (riscv64 + x86_64) after every commit.
- inet_peek probe 17/17 PASS on the final kernel; sockaddr_clamp 8/8
  PASS (3 boots); devfsmap probe PASS.
- make test (riscv64 suite): running at ledger time — verdict recorded
  below when finished.
- Coverage: scripts/review_coverage.py --check passes; doc regenerated.

## Retracted

- (none)
