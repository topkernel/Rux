# review-func round findings (2026-10-31)

Scope this round: `kernel/src/syscall/network.rs` (4451 lines, full
line-by-line read; previous status: todo).

Verification for every fix: `make build` (riscv64) and `make build-x86`
rc=0; `make test` guest suite green; behavior A/B via the
`sockaddr_clamp` probe (`test/sockaddr_clamp.c`, driver
`test/sockaddr_clamp_boot.py`, riscv64 boot): BEFORE 6/6 fixable cases
FAIL on the pre-fix kernel, AFTER 6/6 PASS (serial logs kept in
`run/sockaddr-clamp-before-riscv64.log` / `run/sockaddr-clamp-serial.log`).

## Fixed

- **F1 recvmsg msg_namelen was read as a pointer (ABI)** —
  `kernel/src/syscall/network.rs:3198` (old code): `msg_namelen` is a
  socklen_t VALUE field at msghdr offset 8; the code dereferenced offset 8
  AS a pointer (namelen+pad bytes reinterpreted, a bug older than the
  uaccess refactor — raw-deref form 3c8ae4b4). Effects: (a) the
  `access_ok(msg_namelen_ptr, 4)` gate usually evaluated a garbage
  pointer and silently skipped the whole source-address write-back
  (raw/netlink recvmsg never saw msg_name), (b) when the garbage landed
  in the user range the length store went to a random user address
  (4-byte stray write), (c) `msg_namelen` was NEVER updated to the actual
  address length on any path. Fixed: the length word lives at
  `msg_ptr+8` itself. A/B: `udp-recvmsg-namelen` FAIL (namelen stayed
  128) -> PASS (namelen=16); short-hint variant FAIL (stayed 8) -> PASS
  (16).

- **F2 sockaddr write-backs ignored the caller's buffer length (ABI +
  memory overwrite)** — `put_sockaddr_in`/`put_sockaddr_in6` (and the
  accept/getsockname/getpeername/recvmsg/recvfrom call sites) always
  wrote the FULL family struct and stored klen, never clamping to the
  caller's `*addrlen`/`msg_namelen` (reference `move_addr_to_user`
  copies `min(*ulen, klen)` bytes then stores klen). Worse, recvfrom
  computed the clamp explicitly and the helper then overwrote both the
  stored word and bytes past a short caller object. Fixed: `limit`
  parameter plumbed through all three helpers and five inet call sites;
  bytewise copy of `min(limit, klen)` bytes, store klen (null
  `addrlen_ptr` skips the store). A/B: `udp-recvfrom-shortlen` FAIL
  (canary after an 8-byte sockaddr object CLOBBERED, sl=16) -> PASS
  (canary intact, sl=16). The accept site's clamp is the same mechanism
  and helper (see F5 for why accept has no direct guest A/B).

- **F3 AF_INET6 address write-backs ran past the access_ok-validated
  range** — getsockname/getpeername/recvmsg validated only 16 bytes of
  the caller's `addr`/`msg_name` buffer, then a v6 socket took
  `put_sockaddr_in6` writing 28 bytes (12 bytes through an unvalidated
  range, and past a 16-byte caller object). Fixed: family-sized
  `want` (16/28) validation before the write, plus F2's clamp. A/B:
  `v6-getsockname-16b-obj` FAIL (28-byte write CLOBBERED 12 bytes after
  a 16-byte object, sl=28) -> PASS (16 bytes copied, canary intact,
  sl=28).

- **F4 unix datagram sendto/recvfrom silently truncated at RW_CHUNK
  (message corruption)** — the AF_UNIX branches of `sys_sendto` and
  `sys_recvfrom` staged `len.min(RW_CHUNK)` (64KB): a larger SOCK_DGRAM
  / SOCK_SEQPACKET record was silently cut to 64KB and reported as a
  successful (partial) send — a unix datagram is all-or-nothing; the
  receiver got a corrupted record with no MSG_TRUNC signal, and
  sendto/sendmsg disagreed (sendmsg already stages 4*RW_CHUNK whole).
  Fixed (R37): DGRAM/SEQPACKET stage the entire record bounded at
  4*RW_CHUNK (256KB, above the default SO_SNDBUF 212960), EMSGSIZE
  beyond; STREAM keeps partial-send staging (POSIX-legal). recvfrom
  stages min(len, 4*RW_CHUNK) for datagram kinds so a whole large record
  can land. A/B: `unix-dgram-100k` FAIL (send=65536 recv=65536) ->
  PASS (send=102400 recv=102400 whole-record compare); `unix-dgram-300k`
  FAIL (send=65536, silent corruption) -> PASS (-1/EMSGSIZE).

## Filed (not fixed this round)

- **F5 loopback TCP connect wedges against accept (pre-existing, net/
  layer)** — repro: `sockaddr_clamp` probe's accept case. A forked child
  connecting to a loopback TCP listener never returns from the blocking
  `connect()` (kernel trace: "CHILD-CONNECT-START" then silence), and a
  nonblocking-accept poll loop (50ms x 160) never sees a connection —
  on riscv64 MTTCG smp2 AND x86_64. UDP loopback works in the same boot.
  The connect engine's 8-round loopback drain (`Socket::connect`,
  `kernel/src/net/socket.rs:493`) is not enough here. Root cause belongs
  to the net/tcp.rs review round (still todo). The accept-side clamp
  fix (F2 mechanism, same helper) could not get direct guest A/B because
  of this; proven via the identical helper path in recvfrom + inspection.

- **F6 put_sockaddr_un / put_sockaddr_nl do not clamp to the caller's
  length** — same move_addr_to_user shape as F2 but in
  `kernel/src/net/unix.rs:674` and `kernel/src/net/netlink.rs:1204`
  (write the full struct, ignore the caller's buffer size). Lower
  severity (sockaddr_un is the object callers pass anyway); fix belongs
  to those files' rounds.

- **F7 MSG_PEEK is ignored on inet sockets** — only the AF_UNIX receive
  path honors MSG_PEEK; `socket_recv_ctl` has no flags parameter, so a
  UDP/TCP recvfrom(MSG_PEEK) consumes the datagram/bytes (reference:
  peek returns the data without consuming). Needs plumbing through
  `Socket::recv` -> udp/tcp engines (net/ layer round).

- **F8 sendmmsg/recvmmsg stop on a zero-length message** — the AF_UNIX /
  AF_NETLINK batch loops break after a 0-return (`sys_sendmmsg` /
  `sys_recvmmsg`); the reference counts the 0 and continues (sendmmsg);
  edge-case only.

- **F9 sendmmsg per-message msg_name parse drops bad families** —
  `Err(_) => None` (silently unaddressed) vs the reference's EAFNOSUPPORT
  for a msg_name whose family does not match the socket (sendmsg's path
  does return EAFNOSUPPORT); errno divergence on an error path.

- **F10 shutdown() on an unconnected TCP socket returns 0** — the
  reference returns ENOTCONN for shutdown on a socket that was never
  connected; the Rux path flips the state to Closing regardless.
  Kept unfixed: unwind of `socket_create_accepted`'s fd was judged out
  of scope; no LTP case on the 1869 list exercises it.

- **F11 netlink bind ignores nl_pid** — documented in-tree simplification
  (`kernel/src/net/netlink.rs:523`: no portid-routed unicast table);
  processes that bind a chosen nl_pid and expect unicast delivery to it
  cannot work. Needs a portid table (netlink round).

## Retracted

- (none this round)
