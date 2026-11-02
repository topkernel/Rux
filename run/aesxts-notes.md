# AES-XTS diagnosis notes — geekbench2 lane (2026-11-02)

Question (from the 11-01 standing): why was single-core AES-XTS 1219 vs
the 4-CPU-pinned reference's 7643 (16%) while multi-core AES-XTS sat at
103%?

## TL;DR

Not AES-NI, not XCR0, not FPU state, not the memory path. The 11-01
kernel printed per-operation diagnostics (`MPROTECT-RW`, `BIGMUNMAP`,
`thead-exit` syscall-history dumps, `PREEMPT-*` warnings) from the
mprotect/munmap/thread-exit hot paths to the polled byte-at-a-time UART
console, at ~0.4-0.7 ms+ per print. Geekbench's AES-XTS workload issues
exactly those syscalls around every iteration (buffer mprotect between
phases, >=512K munmap on every thread exit). Each measured iteration
absorbed tens to hundreds of milliseconds of console time — bimodal
runtimes [0.082, 0.051, 0.026, 0.081] s vs the reference's flat 0.009 s.
Fixed on main before this lane by 4ce0cda5 (print purge, message text
preserved behind opt-in gates) and e2717687 (exit-path prints). Rerun on
main@e6401a4a: **8047 (105% of the pinned reference), per-iteration
runtimes flat at 0.009 s — native profile.**

## Evidence

### 1. Kernel-side FPU/AES enablement is correct (current kernel)

- Boot line, all 4 CPUs, both the 11-02 standing run and probe boots:
  `fpu: OSXSAVE+XCR0=0xe7 AVX (cpu feature mask 0xe7)` — x87|SSE|YMM|
  OPMASK|ZMM_Hi256|Hi16_ZMM. AES-NI is SSE-domain: needs CR4.OSFXSR +
  CR4.OSXSAVE + XCR0.SSE only. All present. (Host XCR0 reads 0x2e7 —
  bit 9 PKRU, which Rux intentionally does not enable; irrelevant to
  AES.)
- Probe CPUID inside the guest: `cpuid1_ecx aesni=1 xsave=1 osxsave=1`,
  `cpuid7 vaes=1` (VAES visible too; GB 5.5.1's bundled OpenSSL 1.0.2m
  predates VAES and uses xmm AES-NI assembly + llvm.x86.aesni
  intrinsics, so only the xmm path matters).
- Probe `aesenc` one-shot executes (`PROBE aesenc_exec OK`, SIGILL
  guard armed) — no trap/emulation path.

### 2. Raw crypto + memory rates are at reference-kernel parity in-guest

Probe: static musl binary (`userspace/geekbench/aes_probe.c`), PID 1,
same boot shape as the rig (`q35/kvm/-cpu host/-smp 4/-m 4G`). Three
environments, same binary, run back-to-back on 2026-11-02 morning
(host load 6.5-8, recorded per-run in the serial logs):

| metric | native host (taskset 0-3) | reference kernel in-guest (7.0.0-38-generic, no initrd, VIRTIO_BLK=y) | Rux in-guest (e6401a4a strong) |
|---|---|---|---|
| aesenc 8-chain, M rounds/s | 1311 | 1244 | 1268 |
| xts_walk 128 MB (aesenc+store, 4 passes) GB/s | 19.7-20.2 | 20.1-20.4 | 19.3-19.5 |
| stream read 256 MB 1-thr GB/s | 44-49 | 48-54 | 47-54 |
| stream write (memset) GB/s | 27.1 | 27.6 | 27.4 |
| stream read 4-thr aggregate GB/s | 53.2 | 52.3 | 53.0 |
| first-touch 256 MB GB/s | 5.4 (65535 minflt) | 2.1 | 9.9 |
| malloc+touch+free 128 MB cycle | 27-28 ms | 19-198 ms | 18-21 ms |
| clock_gettime ns/op | 18.2 (vDSO) | 18.7 (vDSO) | 81.9 (syscall) |
| spin 2 s: nvcsw/nivcsw | 0/6 | 0/2 | 0/0 |

Rux in-guest is 96-97% of the reference-kernel in-guest on every
crypto/memory number. There is no residual AES-XTS deficit to fix. The
4K-only user mappings (no THP — MADV_HUGEPAGE is a no-op, MAP_HUGETLB
rejected) were my prime suspect mid-investigation and are DISPROVEN by
these numbers: 4K pages under KVM stream at reference parity; the
hardware page walkers are not the wall at these rates.

### 3. What actually broke the 11-01 run (forensics)

- 11-01 serial log (`perf-geekbench/run/geekbench/rux-serial.log`,
  198 KB for the whole boot): inside the AES-XTS section window the
  kernel printed `MPROTECT-RW pid N addr...` (3x), `BIGMUNMAP pid N...`
  (2x), five `thead-exit` lines each carrying a ~40-entry syscall
  histogram, plus `PREEMPT-UNDERFLOW`/`PREEMPT-HEAL` diagnostics. Run
  phase total: 656 such prints, ~39 KB of console bytes.
- 11-02 serial log (same workload set): 13 KB whole boot, 8 matching
  lines total — the purge in action.
- Commit 4ce0cda5 (2026-11-01 19:40, on main): "console TX is polled
  and byte-at-a-time (console.rs putc/puts_batch), so every pr_err/
  warn/info that fires per-op costs ~0.4-0.7 ms of UART per hit";
  it specifically gates MPROTECT-RW ("fired ungated on every
  PROT_WRITE mprotect — thread create, malloc growth, JIT"), BIGMUNMAP
  ("every >=512K munmap — every thread exit"), and the signal-path
  TKSIG print ("on EVERY tkill/tgkill") that the task brief called the
  tgkill 1317x class. e2717687 did the same for fork/exit prints.
- Score arithmetic: AES-XTS SC per-iteration mean went 0.060 s ->
  0.009 s; +51 ms/iteration ~= the console time of the per-op prints
  that section generates. Bimodality matches which iterations carried
  the mprotect/munmap/thread churn.

### 4. Residual divergences found while probing (follow-ups, not AES blockers)

- `clock_gettime` has no vDSO on Rux x86_64: 82 ns/op vs 19 ns on the
  reference kernel (syscall round trip). Matters for timing-loop-heavy
  userspace, not for Geekbench (hundreds of calls per section).
- `getrusage` reports `ru_minflt = 0` on Rux (native and
  reference-kernel guests report 65535 for the same first-touch test).
  Minor-fault accounting is missing somewhere in the fault path.
- `AT_HWCAP` auxv is 0 on x86_64 (kernel/src/process/exec.rs:1050);
  reference kernels report CPUID feature bits. glibc-internal IFUNC
  dispatch on x86_64 does its own CPUID so nothing observed broke, but
  it is an ABI divergence (musl/glibc `getauxval` consumers).
- One `WW: TIMER-LATE-FIRE pid=1 late=44ms (timer fired late; softirq
  starvation?)` fired during the 11-02 standing run (visible in
  rux-serial-3.log). Did not distort the sections materially; the
  timer guarded domain should keep an eye on late-fire classes during
  CPU-saturated boots.

## Rig incidents on this lane (for whoever runs next)

- The 11-01 lane's capture server (root, :443, /tmp/gb-ca certs) was
  still running but its working directory had been deleted — captures
  landed in an unrecoverable location. Killed exact PIDs 3973065
  (sudo wrapper) + 3973089 (python3), recorded here. Replacement
  server started from THIS worktree
  (userspace/geekbench/gb-capture-server.py, PIDs 25057/25060), same
  certs; it writes captures.log in this worktree.
- The banked testimages/geekbench-x86.img carried the pre-22:28 CA
  (CN=GB Capture CA) while /tmp/gb-ca now holds CN=Rux Geekbench
  Capture CA; the reflink copy used for the standing run had its
  /etc/ssl/certs/ca-certificates.crt swapped via debugfs to the
  current CA (fingerprint 83:87:0C:B6:71:17:BC:21:63:35:FB:E8:B6:ED:
  90:90:C6:10:6E:3F) — upload path verified with curl before the run.
- First standing attempt completed the suite but the upload vanished
  into the dead server's black hole (serial rux-serial-2.log); the
  scored attempt is rux-serial-3.log + captures.log entry
  2026-11-02T07:23:15, the confirm run rux-serial-4.log + entry
  07:47:04 (SC 2297 / MC 7458, AES-XTS SC 8140 — both uploads' parsed
  rows live in run/geekbench/scores-11-02.txt).
- Reference-kernel probe boot: use the host's
  vmlinuz-7.0.0-38-generic WITHOUT initrd (VIRTIO_BLK=y builtin; the
  host initrd dracut waits for the host root UUID and drops to
  emergency mode). Hard-killing that boot after PID-1 exit leaves the
  probe image's ext4 dirty (unknown ro_compat) — e2fsck -fy before
  reuse.
- Probe binary history: first build stored an xmm register back to the
  read-only code page (SEGV; the kernel's USEGV-FOR ripbytes forensics
  `66 0f 7f 00` identified it in one look); second build's throughput
  loop lost its iteration count. Final build is what's committed.
