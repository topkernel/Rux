# test-supplement lane — coverage rationale (2026-10-31)

Mission: turn the week's real fixes into permanent regression coverage;
audit existing tests for assertions weaker than the contract. Worktree
`/home/william/rux-agents/test-supplement`, branch
`feature/test-supplement`.

The lane found three latent bugs in the fixes it was covering — each
pinned by a test after the kernel fix (details below). All kernel
changes are confined to `kernel/src/net/tcp.rs`.

## Kernel fixes made by this lane (in tcp.rs, with pinning tests)

1. **RST sequence validation compared wire byte-order against host
   order** (regression in 295743fa, finding 1). `handle_packet`'s RST
   acceptability check used `tcp_hdr.ack_seq` / `tcp_hdr.seq` raw — the
   header fields hold big-endian wire values, every other consumer
   converts with `from_be`. Net effect on main: in every state a
   legitimate RST's sequence landed far "outside" the window and was
   dropped — an ESTABLISHED connection could no longer be reset by a
   real peer, and in SYN_SENT the R34 ECONNREFUSED path was dead (the
   SYN/RST ping-pong against QEMU user networking would return).
   Fixed with `TcpSeq::from_be` in both arms; pinned by
   `tcp-rst in-window RST resets`, `tcp-rst RST at rcv_nxt resets`,
   `tcp-rst reset clears send/retrans buffers`,
   `tcp-rst SYN_SENT valid RST aborts (ECONNREFUSED)`.

2. **The finding-8 by-value `TcpHdr` refactor broke SYN option
   parsing.** `TcpHdr::from_bytes` (238e4370) began returning the
   header BY VALUE, but `tcp_parse_mss/wscale/tsval` still read the
   option bytes as memory FOLLOWING the header pointer — i.e. past the
   caller's stack copy, not the packet. MSS/window-scale/timestamp
   adoption on real traffic read stack garbage (usually parsing to
   None → silently no WS/TS and default MSS). Fix: the parsers take the
   option-byte SLICE; `tcp_option_slice(skb, header_len)` produces it
   from the skb, `handle_packet` carries it as a parameter. Pinned by
   `tcp-synopts LISTEN adopts SYN options (MSS/WS/TS)` — the test
   drives handle_packet with the exact RX plumbing (by-value header +
   buffer-backed options).

3. **The finding-10 OOO trim inverted its arithmetic.** The first
   version kept `data[len-cut]` with `seq+(len-cut)` instead of
   `data[cut]` with `seq+cut`: a retransmission straddling rcv_nxt
   (already-received prefix) silently dropped the stream range
   `[rcv_nxt, seq+len-cut)` — a permanent hole. Fixed the arithmetic
   and, per RFC 793 §3.9, drain immediately when the trimmed segment
   starts at rcv_nxt (it is in-order by then). Pinned by
   `tcp-straddle trimmed remainder delivers in place` +
   `tcp-straddle delivered bytes are the exact stream range`.

## New kernel-side coverage (kernel/src/tests/, guest `make test`)

- **`tcp_semantics.rs` (new, 44 rows)** — the RFC-semantics sweep of
  2026-10-31 as deterministic state-machine tests (drives
  `handle_packet`/`process_ack` with crafted segments; reorder and
  duplicate scenarios the loopback wire cannot produce):
  - finding 1 (RST validation): 7 rows incl. SYN_SENT ACK==SND.NXT
    rule and buffer clearing;
  - finding 2 (FIN gating): out-of-order FIN dropped without touching
    rcv_nxt, in-order FIN+data → CLOSE_WAIT with exact stream bytes,
    duplicate FIN re-ACKed in CLOSE_WAIT, FIN_WAIT2 transitions;
  - finding 3 (single-shot fast retransmit): fires exactly at
    dup-ACK #3 (retries 0→1, FastRecovery, recover_seq), dup-ACKs 4-5
    only inflate cwnd, data-carrying ACKs not counted, fresh ACK exits
    recovery;
  - finding 4 (Karn + partial-ACK trim): retransmitted segment's ACK
    samples no RTT (with a fresh-segment control that does), partial
    ACK trims the retransmit-queue head to (ack, len-covered, data);
  - finding 9 (SND.WL1/WL2 gate): reordered old ACK with w=0 does not
    retract snd_wnd; newer segment adopts; equal-seq older-ack vs
    equal-ack discrimination;
  - finding 10 (OOO caps): total OOO bytes bounded by the advertised
    window (3rd 2000-byte segment dropped at rcv_wnd 5535), gap fill
    drains the coalescible head, fully-delivered duplicates not
    re-buffered; the straddle-trim rows above;
  - graceful close (finding 6): FIN defers behind queued data at zero
    window (fin_pending, no FIN in the retrans queue), the reopened
    window sends data then FIN at exactly seq 1400 behind it;
  - finding 5 (SYN options handshake-only): LISTEN/SYN_SENT adopt
    MSS 536/WS 7/TSval, a retransmitted SYN-ACK in ESTABLISHED adopts
    nothing;
  - option parsers on wire-shaped bytes (EOL list, truncated option,
    WS clamped to 14);
  - finding 8: `TcpHdr::from_bytes` parses an UNALIGNED header
    field-exact (buffer at offset 1) and rejects short buffers.
- **`ipc_sysv.rs`** — semid64_ds field offsets for the 88-byte
  asm-generic contract restored by 44b70b26: `sem_otime@48`,
  `sem_ctime@56`, `sem_nsems@64` plus the IpcPermUapi prefix offsets
  (key@0 … seq@24, size 48) that pin them. The kernel-side `_high`
  padding had moved ctime to 64 where musl reads 56.
- **`mm_pcp.rs` (new, 10 rows)** — INV-PCP-2 as a hard assertion:
  under IRQs off, N order-0 allocs drop `free_pages` by exactly N and
  the free cycle restores it bit-for-bit (pcp-cached frames stay
  counted free); PCP_HITS+PCP_MISSES advance by exactly N; no
  PCP_DOUBLE_FREE taint; `cached_pages_total ≤ MAX_CPUS·PCP_HIGH`;
  the second cycle is served from the cache and hands back the very
  frames the test freed (no leak).
- **`mem_mmap.rs`** — mprotect coverage-walk span bound (1f2f18cb): a
  span across a punched HOLE fails ENOMEM (the walk must run to
  addr+length, not stop at the first covering VMA's end), each side
  alone succeeds, overrun past the mapping fails. (Skipped in the
  unit-test boot where the task has no user VMAs — the row activates
  wherever mmap rows do.)

## Guest probe coverage (tarball: test/probes.tar.gz)

- **`test/nettest.c`** — new `tcp_close_delivery_test` leg (port
  15002): client writes 8 KiB pattern and closes IMMEDIATELY; server
  must read the exact pattern then a clean EOF. End-to-end pin of the
  close()-drains-before-FIN fix (295743fa finding 6) and FIN/data
  ordering on the wire. NOTE: the standing nettest probe (and this
  leg) cannot go green right now — the loopback TCP echo stalls right
  after the `tcp-listen` trace on the CURRENT MAIN TIP, reproduced on
  a pristine main build (stash + rebuild) on both -smp 2 (with the
  PREEMPT-UNDERFLOW/KTHREAD_MAP print flood) and -smp 1 (clean serial,
  same stall). Pre-existing, preempt-hunt lane territory; the leg
  gates the verdict the moment that wedge is fixed. The kernel-level
  pin of close-drain semantics is in tcp_semantics meanwhile.
- **`test/uafault.c` + `test/uafault_boot.py` +
  `test/uafault-x86-probe.sh` (new leg)** — the x86_64 SMAP +
  dropped-identity-map user-pointer contract (bdc9234e + e22f72a8):
  prlimit64 new/old, setrlimit/getrlimit, write, read, fstat, getcwd
  all handed user pointer VA 1 and each must return -EFAULT (14), not
  success; valid-buffer controls prove the AC-bracketed uaccess still
  copies (a permanently-shut window fails the controls). The kernel
  must survive (no oops / SMAP-VIOL beacon).

## Existing tests corrected (assertions weaker than the contract)

- **`syscall_signal.rs`** — 8 tautologies `test_assert!(ret == 0 ||
  ret < 0, ...)` (true for every value a syscall can return) replaced
  with exact pinned values: rt_sigaction rows assert -EINVAL (the
  unit-test boot task has no signal_struct; the row documents that a
  real process would see 0 and must be re-pinned if the boot task ever
  gains one), rt_sigprocmask rows assert -EFAULT (kernel-pointer
  set/oldset → access_ok failure; the NULL-set query additionally
  asserts the sentinel oldset buffer was NOT written).

## Kernel fixes made by this lane (with pinning tests)

4. **Discarded `put_user` results: getrlimit / getitimer / setitimer
   returned 0 for bad user out-pointers** (found by the new uafault
   probe's first run). `sys_getrlimit` and the itimer old-value copies
   used `let _ = put_user(...)` — a syscall handed an unmapped user
   pointer reported success while writing nothing. Linux reports the
   failed copy_to_user as -EFAULT. Fixed in
   `kernel/src/syscall/process.rs` (getrlimit) and
   `kernel/src/syscall/time.rs` (getitimer, setitimer old_value);
   pinned by the probe's `getrlimit`, `getitimer`, `setitimer-old`
   rows (all EFAULT) plus the valid-buffer controls. Remaining members
   of the same bug class (timer_create's timerid out-param,
   clock_gettime32 len_ptr at time.rs:1136/1838) are noted for the
   time-syscall owner — no probe coverage yet, so not touched here.

## Fixes verified, not unit-testable (and why)

- **per-mm PTE split on all arches (44b70b26)** — a locking topology
  change; semantics identical by design, covered by the full existing
  suite running under it. The concurrency property is the soak fleet's
  domain, not a unit row.
- **RCU QS on user-return (c312944d)** — the hookup lives in the trap
  tails, unreachable from the test harness; on the single-CPU unit
  boot the pre-existing QS paths (__schedule/idle) already complete
  grace periods, so a unit row cannot distinguish. Its regression
  surface is pbench p2/p4 latency — owned by the perf gate (Tier 2).
- **page_cache batch fill outside locks (49f40d2d)** — lock-hold-time
  change only; data-path correctness remains covered by the ext4 /
  page-cache rows. Perf gate Tier 2 (pbench fs rows) owns the timing.
- **prlimit64 new_rlim EFAULT x86_64 row** — verified green on this
  branch's x86_64 `make test` (the identity window is gone from
  main); no stale skip/waiver logic exists around it — the only
  remaining skip is the honest "valid user buffer" row, which needs a
  user mm the unit-test boot task does not have.

## Gate results (this branch)

- riscv64 `make test`: **1267/1267, ALL TESTS PASSED** (was 1217 on
  the branch point; +50 rows from this lane, one pre-existing row
  count adjusted by the sigprocmask sentinel check).
- x86_64 `make test`: **1263/1263, ALL TESTS PASSED** — includes the
  prlimit64 new_rlim EFAULT row, green with the identity map gone.
- `test/uafault-x86-probe.sh`: **PASS** (11/11 EFAULT rows + 3
  valid-buffer controls).
- `test/nettest-probe.sh`: FAIL — pre-existing on the main tip (see
  the nettest note above); reproduced on a pristine main build via
  `run/ts-nettest-smp1.sh` (single-CPU isolation harness, lane-local).
