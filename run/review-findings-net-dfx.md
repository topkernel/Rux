# rv-net-dfx review findings (2026-10-30)

Scope: kernel/src/net/** (tcp/udp/ip/arp/route/netlink/unix/raw/ipv6) +
kernel/src/dfx/** + console.rs + printk.rs + init/main.
Watch items: netlink multicast semantics, timer softirq contracts,
printk recursion.

Verification for every fix: `make build` (riscv64), `make build
PLATFORM=x86_64`, `make build PLATFORM=aarch64` — all rc=0 on the branch
tip containing all fixes (full guest suites run centrally at merge).
No QEMU runs (no targeted guest test exists for these paths; the guest
unit suite has zero v6/netlink-uevent coverage, see V1).

## Fixed

- **N1 netlink (watch item: multicast semantics)** — `uevent_broadcast`
  delivered kernel uevents to EVERY NETLINK_KOBJECT_UEVENT socket
  regardless of the bind-time group mask; `netlink_multicast_send`
  (userspace forward) already filtered. Raw kernel events landed on
  udev-group (2) monitor sockets that must only see the daemon's
  processed forward. Fixed: deliver only to sockets subscribed to group
  1 (reference netlink_broadcast subscription rule). udevd (group 1)
  unaffected. Commit 9e8a8992.

- **V1 ipv6 (severe)** — `ipv6_rcv` trimmed `skb.len` to Payload Length
  *before* pulling the 40-byte base header: transport payloads < 40
  bytes (NS/NA 32, SYN 20) were dropped at the skb_pull; everything
  longer lost its last 40 bytes and failed transport validation.
  Inbound IPv6 never delivered a correct segment (no guest v6 test
  coverage existed to catch it). Fixed: trim to `IPV6_HDR_LEN +
  payload_len` (RFC 8200 + reference ip6 input). Commit 358aa400.

- **S1 socket/TCP** — after a nonblocking connect() returned EINPROGRESS,
  nothing promoted the VFS `Connecting` state on handshake completion:
  send()/write() returned EPIPE on an ESTABLISHED connection (reference:
  success; EAGAIN while still connecting), poll(POLLOUT) never fired for
  it, and poll(POLLOUT) never fired for UNCONNECTED UDP sockets either
  (reference datagram_poll is unconditionally writable). Fixed: send
  falls through `Connecting` to the protocol layer + promotes to
  Connected once the slot is ESTABLISHED; TCP POLLOUT consults
  tcp_send_ready; UDP POLLOUT unconditional. Commit 8c32fec0.

- **N2 ipv4** — ip_rcv verified the header checksum over a fixed 20
  bytes; the checksum covers IHL*4 (options included, RFC 791), so
  every options-bearing datagram was dropped. Fixed to verify over the
  validated IHL. Companion guard: ip_defrag now drops fragments with
  IHL > 5 (the 20-byte rebuild buffer cannot carry options; a rebuild
  advertising absent options would mis-parse in ip_dispatch).
  Commit c428a12b.

- **U1 udp** — (a) zero-length sends returned 0 without transmitting;
  zero-length datagrams are legal and now go out as header-only frames
  on both wire paths. (b) send() without destination on an unconnected
  UDP socket returned ENOTCONN; reference udp_sendmsg returns
  EDESTADDRREQ. Commit c1a1bc66.

- **T1 tcp** — fully out-of-window segments were dropped silently;
  RFC 793 §3.9 (and the reference dupack path) answers unacceptable
  non-RST segments with an ACK carrying the current window — the
  response a peer's zero-window persist probe waits for. Fixed.
  Commit 82b90118.

## Filed (not fixed — judged below the fix bar this pass)

- **arp.rs ArpCache::remove/update accounting**: `remove` decrements
  count on invalidate; a subsequent `update` appends at
  `entries[count]`, which can overwrite a still-valid entry (one lost
  mapping per remove cycle; self-heals via re-ARP). No memory-safety
  impact; `arp_remove` has no caller that a boot exercises. Low.
- **udp.rs broadcast heuristic**: `(dest_ip & 0xFF) == 0xFF` treats any
  host address ending in .255 as broadcast (EACCES without
  SO_BROADCAST). Documented in-file (no netmask model); kept.
- **udp_parse_packet** requires UDP length == skb.len exactly; the
  reference trims trailing padding. ip_rcv already trims to IP
  tot_len, so only padded-IP payloads (rare, non-conforming senders)
  are affected. Low.
- **netlink recv source group**: recvmsg/recvfrom on uevent sockets
  always report source nl_groups=1, also for messages the daemon
  forwarded to group 2. libudev only distinguishes unicast (0) vs
  multicast, so behavior matches every consumer; per-message group
  tracking is not worth the queue surgery. Cosmetic.
- **tcp window scale**: snd_wnd stays u16; a scaled peer window is
  capped at 65535 bytes. Documented in-file (P2). Throughput
  limitation, not correctness.
- **defrag total_len as u16**: a crafted MF=0 fragment can claim a
  total above 65535 (illegal datagram); the completion path would
  truncate tot_len on rebuild. Reachable only from a non-conforming
  peer; the datagram is already over every legal limit. Very low.
- **ip_rcv destination check**: packets are dispatched regardless of
  daddr (single-homed host; ethernet is_for_us filters at L2). Left as
  designed.

## False positives — retracted after re-derivation

- *"icmp_echo_reply / icmp_send_dest_unreach leak the skb on the early
  error returns"* — no: SkBuff owns its allocation and Drop frees it
  (review NET-C2 / M1 in buffer.rs).
- *"tcp timer tick frees CLOSE corpses a live fd still wraps"* — no:
  the sweep requires user_refs == 0 AND (parent_fd.is_some() ||
  orphaned); R21-N4/R32-B7 guards hold.
- *"netlink_recv should fail on truncation"* — no: datagram-truncation
  semantics (copy what fits, return the copied count) match the
  reference for a too-small buffer; MSG_TRUNC full-length reporting is
  a separate opt-in this callers' set does not use.
- *"route_output ignores the route it looked up"* — yes, but that is
  the documented W3 leftover (the route table feeds gateway resolution
  in ethernet_send; route_output is a thin alias). Not a defect.
- *"printk recursion window via the syscost probes"* — no:
  note_printk_* are gated atomic counters only, no locks/allocs; the
  per-CPU IN_PRINTK guard plus the bounded CONSOLE_EMIT_LOCK cover the
  recursion watch item.

## Reviewed clean (no action)

buffer.rs, ethernet.rs, icmp.rs, arp pending-queue (R35 discipline),
tcp_timer.rs (softirq contract: staging reserved outside
TCP_TABLE_LOCK, emissions after the drop — verified intact),
netlink.rs rtnetlink/ioctl set, unix.rs (namespace, SCM paths, cork,
poll), raw.rs, icmpv6.rs, printk.rs (per-CPU reentrancy, bounded emit),
dfx syscost printk probes (atomic-only, gated), softirq registration
surface (interrupt/softirq.rs + timer/timer_softirq_handler).
