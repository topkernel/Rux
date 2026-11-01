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
