# Geekbench on Rux — Rig, Scores, and Findings (2026-11-01)

Lane `perf-geekbench`, first full Geekbench run on the Rux x86_64 kernel.
Geekbench 5.5.1, AMD Ryzen 9 9950X, KVM (`-M q35 -accel kvm -cpu host
-smp 4 -m 4G`), strong-profile kernel (`build-release-opt`).

Two kernel bugs were found and fixed on this lane just to make the run
complete; both are in the diff of this branch with serial/packet
evidence below.

## Headline

| | Rux (4 vCPU guest) | Reference, 4-CPU pinned | Reference, 32T full |
|---|---|---|---|
| **Single-Core Score** | **1941** | 2165 | 2549 |
| **Multi-Core Score** | **7139** | 7027 | 20402 |
| **vs 4-CPU reference** | 90% single / 102% multi | 100% | (topology differs) |

All 21 workloads complete in both the single-core and the multi-core
section. No section dies. The reference columns are the same Geekbench
5.5.1 build run natively on the host: once with `taskset -c 0-3`
(topology-matched to the guest's 4 vCPUs) and once unconstrained.

Note on the reference columns: the guest's 4 vCPU threads float over all
16 host cores, while the 4-CPU pinned run is stuck on cores 0-3 — so the
pinned column is the fair-ish comparison and the 32T column is the host
peak. Single-core scores varied ~15% between the two native runs alone
(2549 vs 2165); treat single-core deltas under ~15% as noise.

## Why Geekbench 5.5.1 and not 6.4.0

Geekbench 6.4.0 runs its workloads but, without a Pro license, exits
255 at the result-upload step and prints **no scores** — locally or on
failure (`native-full.log`: ` unknown error (internal code 35)`,
rc=255). Geekbench 5.5.1 "Tryout" also prints no scores locally, but
its upload payload contains the complete JSON result. That upload is
the rig's data path (below), so 5.5.1 is the suite of record; 6.4.0
stays out (228 MB tarball for an unusable build).

## The rig (userspace/geekbench/)

Same protocol as `userspace/lmbench/`: tarball committed, extracted
binaries gitignored, static-musl PID-1 runner, one boot per run,
serial console is the log.

- `build-rootfs.sh <out.img>` — reflink copy of the shared
  `work/ubuntu-x86.img` (glibc userland; the Geekbench binaries are
  glibc-dynamic), grown +1G, payload injected via debugfs:
  `/test/geekbench/{geekbench5,geekbench_x86_64,geekbench.plar}` and
  `/test/geekbench_run` (init). With `GB_CAPTURE_CA=<ca.pem>` set it
  also installs that CA as the guest's `/etc/ssl/certs` store and pins
  `browser.geekbench.com` to `10.0.2.2` in `/etc/hosts` (see capture
  path).
- `geekbench_run.c` — PID 1. Brings eth0 up (10.0.2.15/24, gw
  10.0.2.2, dns 10.0.2.3) via SIOCSIFADDR/SIOCADDRT, runs
  `geekbench5 --cpu` with output through a pipe to the serial console,
  kills the process group after `gtimeout=` seconds (default 900).
  `geekbench=sysinfo` and `geekbench=netprobe` (resolve+connect
  diagnostic) are smoke modes.
- `run-rux.sh` — boots `qemu-system-x86_64 -M q35 -accel kvm -cpu host
  -smp 4 -m 4G`, waits for `GEEKBENCH DONE`/panic/timeout, kills only
  its own QEMU PID.
- **Result capture**: Geekbench only exposes scores inside the HTTPS
  POST to browser.geekbench.com, and that page is bot-guarded (403 to
  every fetcher available here). So the rig redirects the upload to a
  local TLS capture server: `make-capture-ca.sh` mints a CA +
  browser.geekbench.com cert, `gb-capture-server.py` (root, :443) logs
  the POST bodies, `parse-capture.py` prints the score tables from
  `captures.log`. The guest reaches it because QEMU user-network's
  10.0.2.2 is the host; the guest's CA store contains only the capture
  CA. The native reference runs use the same server via a hosts entry
  on the host.

### Reproduce

```bash
cd userspace/geekbench
./make-capture-ca.sh /tmp/gb-ca                      # once
sudo cp /tmp/gb-ca/ca.pem /usr/local/share/ca-certificates/gbcapture.crt
sudo update-ca-certificates                          # host trust
echo '127.0.0.1 browser.geekbench.com' | sudo tee -a /etc/hosts
sudo python3 gb-capture-server.py /tmp/gb-ca/srv.pem /tmp/gb-ca/srv.key 443

# reference (native host)
./geekbench5 --cpu            # after unpacking the tarball
taskset -c 0-3 ./geekbench5 --cpu

# Rux
GB_CAPTURE_CA=/tmp/gb-ca/ca.pem ./build-rootfs.sh ../../work/geekbench-x86.img
./run-rux.sh "" ../../work/geekbench-x86.img ../../run/geekbench/rux-serial.log 2700 1200
./parse-capture.py captures.log
```

Artifacts: `run/geekbench/{native5-*.log, rux-serial.log,
captures-native.log, captures-rux.log}`.

## Per-workload scores

### Single-Core

| Category | Workload | Rux (4 vCPU) | Ref 4-CPU pinned | Ref 32T full | Rux/4CPU |
|---|---|---|---|---|---|
| AES-XTS | AES-XTS | 1219 | 7643 | 8525 | 16% |
| Integer | Text Compression | 805 | 1536 | 1885 | 52% |
| Integer | Image Compression | 2001 | 1813 | 2030 | 110% |
| Integer | Navigation | 1364 | 1753 | 1695 | 78% |
| Integer | HTML5 | 2262 | 1651 | 2269 | 137% |
| Integer | SQLite | 1974 | 1480 | 2026 | 133% |
| Integer | PDF Rendering | 2035 | 2074 | 2133 | 98% |
| Integer | Text Rendering | 1945 | 1456 | 2101 | 134% |
| Integer | Clang | 1964 | 2161 | 2217 | 91% |
| Integer | Camera | 2105 | 1861 | 2140 | 113% |
| Float | N-Body Physics | 2341 | 1956 | 2466 | 120% |
| Float | Rigid Body Physics | 1913 | 1737 | 1962 | 110% |
| Float | Gaussian Blur | 2344 | 2149 | 2332 | 109% |
| Float | Face Detection | 2675 | 2770 | 2711 | 97% |
| Float | Horizon Detection | 1904 | 1459 | 1978 | 131% |
| Float | Image Inpainting | 4055 | 4092 | 4703 | 99% |
| Float | HDR | 4414 | 3614 | 5048 | 122% |
| Float | Ray Tracing | 2772 | 2042 | 3043 | 136% |
| Float | Structure from Motion | 1889 | 1661 | 1977 | 114% |
| Float | Speech Recognition | 1737 | 1741 | 1979 | 100% |
| Float | Machine Learning | 2277 | 2028 | 2513 | 112% |
| | **Overall Single-Core Score** | **1941** | **2165** | **2549** | **90%** |

### Multi-Core

| Category | Workload | Rux (4 vCPU) | Ref 4-CPU pinned | Ref 32T full | Rux/4CPU |
|---|---|---|---|---|---|
| AES-XTS | AES-XTS | 9278 | 8972 | 9500 | 103% |
| Integer | Text Compression | 5792 | 3851 | 21062 | 150% |
| Integer | Image Compression | 7552 | 7481 | 25079 | 101% |
| Integer | Navigation | 4033 | 3546 | 7884 | 114% |
| Integer | HTML5 | 7227 | 7151 | 27313 | 101% |
| Integer | SQLite | 7012 | 6829 | 23579 | 103% |
| Integer | PDF Rendering | 7253 | 7632 | 20130 | 95% |
| Integer | Text Rendering | 4569 | 5924 | 11853 | 77% |
| Integer | Clang | 7300 | 7823 | 30224 | 93% |
| Integer | Camera | 7137 | 7477 | 19010 | 95% |
| Float | N-Body Physics | 6484 | 7501 | 22036 | 86% |
| Float | Rigid Body Physics | 7501 | 7589 | 31340 | 99% |
| Float | Gaussian Blur | 7592 | 7694 | 26057 | 99% |
| Float | Face Detection | 8936 | 10296 | 31220 | 87% |
| Float | Horizon Detection | 7380 | 7616 | 19467 | 97% |
| Float | Image Inpainting | 15136 | 12349 | 37152 | 123% |
| Float | HDR | 18577 | 18962 | 44236 | 98% |
| Float | Ray Tracing | 13410 | 11370 | 43875 | 118% |
| Float | Structure from Motion | 7231 | 7486 | 26772 | 97% |
| Float | Speech Recognition | 5869 | 4537 | 16598 | 129% |
| Float | Machine Learning | 5005 | 5635 | 5833 | 89% |
| | **Overall Multi-Core Score** | **7139** | **7027** | **20402** | **102%** |

## Compatibility bugs found (both fixed on this branch)

### 1. mprotect of a mid-VMA sub-range returned ENOMEM — killed the run at workload 1

Evidence (`rux-serial.log`, first attempt): Geekbench died inside its
first workload —

```
MPROTECT-FAIL pid 308 addr 0x7ff017230000 len 0x800000 prot 0x7 -> -12
stderr[0] pid 308: geekbench_x86_64: src/concurrency/thread_pool.cpp:92:
  ThreadPool::~ThreadPool(): Assertion `stopped_.load() == true' failed.
GEEKBENCH EXIT status=255
```

Cause: `sys_mprotect_inner`'s coverage walk iterated
`vma_mgr.iter_after(addr - 1)`, which yields only VMAs **starting** at
or after `addr` — the VMA *containing* `addr` never appears, so any
strict sub-range of a larger VMA (Geekbench's 8 MB RWX arena carve)
read as a gap and failed with ENOMEM despite no gap existing.

Fix: seed the cursor from the containing VMA via the new
`VmaManager::last_at_or_before()` (kernel/src/mm/vma.rs,
kernel/src/syscall/memory.rs). After the fix all 42 sections ran.

### 2. TCP RX checksum verified against stack garbage — SYN-ACKs dropped, no outbound TCP at all

Evidence (`netprobe` boot + `-object filter-dump`): the guest SYN went
out, slirp's SYN-ACK came back **eleven times**, and the guest never
ACKed one; only the final RST (bare 20-byte header) reached the TCP
layer (`tcp-in[0]` trace). A temporary diagnostic at the checksum gate
showed the computed sum varying (0x1b8…0x7b8) between **byte-identical**
retransmissions — impossible unless the summed bytes are not the
packet's.

Cause: `tcp_parse_packet` returns the header **by value**;
`tcp_checksum` then read `header_len()` bytes through `&TcpHdr` —
for RX that is a stack copy, so the option bytes ("MSS" in slirp's
SYN-ACK, header_len 24) were adjacent stack garbage. Every inbound
segment with options failed verification; option-less segments (RST)
passed. The same shape poisoned option parsing
(`tcp_parse_mss`/`wscale`/`tsval` read options off the struct pointer
too — they usually failed to parse, which is why loopback got away with
it).

Fix: `tcp_checksum` now takes the wire header slice; RX builds it from
`skb.data`; `tcp_option_bytes` and the three option parsers take wire
bytes; `handle_packet` receives `wire_hdr` from the demux (v4 and v6).
After the fix: `GEEKBENCH NETPROBE select=1 soerr=0` and the result
upload POST landed on the capture server.

## Optimization / compatibility queue (ranked)

1. **Single-core AES-XTS at 16%** (1219 vs 7643; multi-core AES is
   103%). A 6x single-thread-only regression on the most
   vector-register-heavy workload points at first-crossing costs on the
   SIMD/xstate path or timekeeping skew in the earliest-measured
   section. Highest single headroom in the table.
2. **Single-core Text Compression at 52%** (805 vs 1536) while its
   multi-core is 150% — same early-section signature as AES. Suspect
   the whole single-core phase pays an early-run cost (cold path,
   scheduler placement of one worker among 4 vCPUs, or TSC-derived
   timing early after boot).
3. **Multi-core Thread-ish rows 77-93%** (Text Rendering 77%, N-Body
   86%, Face Detection 87%, Machine Learning 89%, Clang 93%) — these
   are the heaviest thread-create/join workloads; wake/futex/scheduling
   latency under concurrency is the likely tax. Matches the scheduling
   guarded domain (wake→pick deltas, pbench P2).
4. **Guest clock reads 1970-01-01** in the result JSON (no RTC/time
   set for init tasks). Harmless to scores, but the rig should set
   time; also `sysinfo` reports topology "1 Core, 4 Threads" —
   core/thread topology reporting to guests is off by one level.
5. **TCP TX checksum path** still computes over an in-buffer header
   (correct today); worth folding into one wire-bytes helper with RX so
   the class cannot regress.

## Gate status for the kernel fixes on this branch

Pre-verified on this lane (2026-11-01):

- Dual-arch build: x86_64 `build-release-opt` rc=0 (the measured
  kernel), riscv64 `make build` rc=0.
- riscv64 regression boot (scripted serial, minimal rootfs):
  `cd /` → `cd proc` → `ls` → `echo BOOTCHECK-OK` → poweroff, rc=0,
  zero panics.
- `make test` (riscv64): **ALL TESTS PASSED** (full suite green).

The changes are in the mm (mprotect path, one extra BTreeMap lookup
per call) and net/tcp guarded domains — merge should run the Tier-2
gate (pbench + syscost quiet boot); this lane measured Geekbench
before/after instead (the run completing at all is the functional
signal; per-workload parity above is the performance signal).

---

# 2026-11-02 standing — rerun on main@e6401a4a (lane geekbench2)

Same rig, same protocol, same reference columns (banked native runs of
2026-11-01; single-core reference noise band ~15% applies). Kernel:
strong-profile `build-release-opt` off main e6401a4a — i.e. AFTER the
serial-print purge (4ce0cda5, e2717687), the per-call allocation purge
(cbfe975e), SleepMutex realignment, the lost-park fix, and the path-walk
fast paths landed. One scored run (rux-serial-3.log; upload captured at
07:23:15, host load ~6.8-7.4 including the guest itself).

## Headline

| | Rux 11-02 | Rux 11-02 confirm | Rux 11-01 | Reference, 4-CPU pinned | vs 11-01 | vs 4-CPU ref |
|---|---|---|---|---|---|---|
| **Single-Core Score** | **2274** | 2297 | 1941 | 2165 | +17% | 105% |
| **Multi-Core Score** | **7114** | 7458 | 7139 | 7027 | -0.4% | 101% |

Confirm run (rux-serial-4.log, 07:47, host load ~7.4-8.1) repeats the
scored run within 1% single-core (2297 vs 2274) and +4.8% multi-core;
AES-XTS SC repeats at 8140 (vs 8047) with the same flat 0.009 s
iterations — the fix is stable, not a lucky run.

## Top movers vs 11-01

| Section | Workload | 11-01 | 11-02 | delta | vs 4-CPU ref |
|---|---|---|---|---|---|
| SC | **AES-XTS** | 1219 | **8047** | **+560%** | 105% |
| SC | Text Compression | 805 | 1427 | +77% | 93% |
| MC | Text Rendering | 4569 | 5563 | +22% | 94% |
| MC | Speech Recognition | 5869 | 6757 | +15% | 149% |
| MC | N-Body Physics | 6484 | 7259 | +12% | 97% |
| MC | Machine Learning | 5005 | 5576 | +11% | 99% |
| MC | AES-XTS | 9278 | 9962 | +7% | 111% |

The two former early-section laggards (AES-XTS 16%, Text Compression
52%) are gone: their per-iteration runtimes went bimodal/warmup-shaped
([0.082, 0.051, 0.026, 0.081] s and [2.185, 1.083, 0.682, 0.652] s) to
flat at reference speed ([0.009 x4] s and [0.64-0.66] s). The
multi-core thread-heavy rows (Text Rendering, N-Body, ML, Speech
Recognition — the create/join/futex-heavy family) moved up 11-22%,
consistent with the scheduling/futex/lost-park work that landed.
Remaining below-90% rows: SC Navigation 77%, SC Clang 86%, MC Camera
76%, MC Face Detection 79%, MC SQLite 86% — all single-run numbers
under load 7-8; the 11-01 flake guidance (rerun in isolation before
calling regression) applies, and several other SC rows softened 2-13%
in the same band, so treat sub-15% SC deltas as noise until re-run.

## The AES-XTS 16% row: root cause (kernel-side, already fixed on main)

The 11-01 kernel printed per-operation diagnostics from the exact
syscalls Geekbench's AES-XTS section issues between iterations:
`MPROTECT-RW` (every PROT_WRITE mprotect — buffer re-protection,
thread create, malloc growth), `BIGMUNMAP` (every >=512K munmap —
every thread exit), `thead-exit` lines with full syscall histograms,
plus `PREEMPT-*` warnings — 656 prints / ~39 KB of console output
during the 11-01 run phase alone. The console TX is polled and
byte-at-a-time: ~0.4-0.7 ms+ per print (commit 4ce0cda5's census).
+51 ms of that per iteration turns the reference's flat 0.009 s into
the observed bimodal 0.026-0.082 s — a 6.2x score drop isolated to
whichever sections carried the mprotect/munmap/thread churn.

AES-NI itself was never broken. From-our-side evidence (probe
`userspace/geekbench/aes_probe.c`, booted as PID 1 in the same QEMU
shape; full notes in `run/aesxts-notes.md`):

- Guest CPUID exposes AES-NI/VAES/AVX2/AVX512F; Rux XCR0=0xe7 (SSE
  domain on — AES-NI needs no further enable); `aesenc` executes
  untrapped.
- aesenc 8-chain issue rate: 1268 M rounds/s in-guest vs 1244
  reference-kernel-in-guest vs 1311 native.
- AES-XTS-shaped 128 MB walk: 19.3-19.5 GB/s in-guest vs 20.1-20.4
  reference-kernel-in-guest vs 19.7-20.2 native — 96-97% parity.
- Streaming read/write/memset and 4-thread aggregates: parity with the
  reference kernel in the same QEMU. (This also disproves the
  4K-page/EPT-amplification theory for this workload: Rux's 4K-only
  user mappings stream at reference rates; no THP needed at these
  working sets.)

No kernel change was required on this lane for AES-XTS — the purge
that fixed it is already on main; this lane supplies the evidence and
the quantified before/after (1219 -> 8047, 105% of the 4-CPU pinned
reference).

Residual divergences noticed while probing (logged as follow-ups in
`run/aesxts-notes.md`, none block Geekbench): no vDSO clock_gettime
(82 ns vs 19 ns per read), getrusage ru_minflt stays 0, AT_HWCAP
auxv is 0 on x86_64 (reference kernels report CPUID feature bits),
and one TIMER-LATE-FIRE late=44 ms during the standing run.

## Rig notes for this run

- The 11-01 capture server had outlived its working directory (deleted
  — captures unrecoverable). Replaced from this lane's worktree
  (PIDs of the old one, 3973065/3973089, killed individually and
  recorded in run/aesxts-notes.md; the new one serves the same
  /tmp/gb-ca certs).
- The banked image carried a stale capture CA; the reflink copy used
  here had /etc/ssl/certs/ca-certificates.crt swapped to the current
  CA via debugfs before the scored run.
- New rig pieces: `aes_probe.c` + `run-probe.sh` (probe boots, Rux or
  reference kernel — the host's 7.0.0-38-generic boots the probe image
  without initrd since VIRTIO_BLK=y).

## Artifacts

`run/geekbench/`: rux-serial-2.log (first attempt, upload lost to the
dead server), rux-serial-3.log (scored), rux-serial-4.log (confirm),
probe-rux.log, probe-rux2.log (final binary), probe-ref.log,
captures.log (entries 2026-11-02T07:23:15 scored and 07:47:04 confirm),
scores-11-02.txt (both uploads' per-workload scores + per-iteration
runtimes, committed), build-gb2.log. Worktree: work/bzImage (the
measured kernel), work/geekbench-x86.img, work/probe-x86.img.
