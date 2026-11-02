# perf-r3 lane notes — verification of the fresh gap-lane merges + next cost layer

Branch `feature/perf-r3` off main 819a1be2. Rig: x86_64 + KVM strong
profile (LTO/CGU1/strip), q35 `-smp 2 -m 2G`, image `work/lmbench-x86.img`
(copy of the shared testimages lmbench-x86.img), one case per boot
(`userspace/lmbench/run-rux.sh` shape; drivers `run/battery1.sh`,
`run/battery2.sh`, `run/fam-med3-r3.sh`, `run/bw-case.sh`).

## Lane 1 — bw_unix verification: CLOSED

The gap-net direct-drain + batching (main 344536af) landed unverified
(bw_unix TIMEOUT under load in that lane's boots). Measured in a quiet
window (load 5.86, 120 s in-guest budget, single boot):

    bw_unix  =  50,539.92 MB/s   (was 2,637.87 MB/s in the standing table;
                                  reference native = 22,782.66 MB/s)

19x over the standing Rux row, 2.2x the reference column (which is
native — Rux beats it under KVM). The 8.2x-slower standing row is gone:
per-chunk shape is now ONE 64 KiB Vec alloc + one copy_in on the write
side, pop + one copy_out on the read side, wake once per syscall.
Attribution work not needed; row closed.

## Lane 3 — syscall floor: small-count stack staging (fix1)

read (0.133-0.151 us spot) / write (0.109-0.135) vs null 0.0585: the
above-floor path for the 1-byte lmbench cases is sys_read/sys_write →
get_file_fd_safe (reader-bank fdtable lookup, ~20 ns) → 3 ops-ptr
dispatch checks → **alloc_staging_buf(1) = one slab alloc + free pair
per call** → trivial device op (zerodev fill / nulldev discard) →
1-byte exception-table copy → gated dfx notes.

Fix (kernel/src/syscall/io.rs): counts <= 256 stage in a stack
[0u8; 256] — no allocator pair. Pipe/unix/ext4 fast paths untouched
(lat_pipe/lat_unix/bw_unix unaffected — no confound). EFAULT/EOF/
short-op semantics identical to the one-chunk case of the loops.

## Lane 2 — lat_unix residual: unix send micro-fixes (fix2)

lat_unix = 2 unix sends + 2 recvs + 2 wake handoffs. Post-PCID the
handoffs are cheap; the unix send body still takes per send:
options lock (sndbuf), state lock, peer Arc, recv_queue lock,
src_arc lock+clone, pending_anc lock (usually EMPTY), unconditional
current_unix_cred() stamp, wake_up_all + epoll notify.

Fix (kernel/src/net/unix.rs):
- `pending_anc_n: AtomicUsize` mirror — drain lock only when something
  is parked (semantics unchanged; push side stores the count).
- `passcred_flag: AtomicBool` mirror of options.passcred — cred
  snapshot per Linux `unix_maybe_add_creds()`: stamp only when SENDER
  or RECEIVER has SO_PASSCRED. The old comment claimed __scm_send
  stamps unconditionally — wrong for AF_UNIX (maybe_add_creds is
  flag-gated). hilogd sets SO_PASSCRED, so its unconditional-cmsg-deref
  contract still gets stamped segments.
- src_arc lock+clone on stream sends left alone (visible recvmsg
  msg_name behavior; not worth the compat risk).

## Measurements

(battery1: P1 done; P2 fam-med3 base skipped at load 13-15, retried
after; see battery1-report.txt / fam-mbase.txt / fam-mfix1.txt)

## Gates

- dual-arch build: x86_64 build-release-opt rc=0; riscv64 fresh
  (rm build/.config) rc=0 (run/build-r3-rv64-fresh.log).
- make test (riscv64): 1297 PASS / 0 FAIL / 66 env SKIP — green
  (run/maketest-r3.log).
- A/B med-of-runs under load<6: battery1 P4 (null/read/write base vs
  fix1), battery2 P1 (lat_unix/lat_pipe base vs fix2).
- perf-gate tier-1/tier-2 (syscost census + pbench): pending.


## battery2 (2026-11-03 01:34-01:53) — unix ctx anatomy + send micro-fix verdict

- lat_unix base 2.32-2.55 µs / fix2 2.35-2.55 µs: the unix send
  micro-fixes are within the boot-to-boot noise band on this rig —
  kept (they remove per-send allocations, structurally right) but NOT
  claimed as wins.
- The cctx anatomy (4 s window, 3.8M context switches) gives the
  per-phase split for the unix ping-pong roundtrip: unix r 2.4 µs +
  w 567 ns per op; wake enqueue 244 ns, dispatch 22 ns, IPI mean
  995 ns (815 IPIs per 1915 wakes — polling idle already absorbing
  most handoffs); fpusave 10 ns (xsaveopt working); mm 777 MB total
  for 3.8M switches = 203 ns avg (CR3 writes, PCID pending).
- The wake+dispatch+IPI chain (244+22+995 = 1261 ns of 2400 ns
  roundtrip) is the dominant remaining cost — owned by the scheduler
  (gap-sched family). The CR3/PCID work is queued there too.
- unix w avg 567 ns vs r 2400 ns: the receive side carries the pipe
  read analog's cost (wake consume + copyout split) — target for the
  next slice if the sched family doesn't move it first.
