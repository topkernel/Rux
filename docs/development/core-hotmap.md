# Core function-level hotmap (p23-hotmap, 2026-11-27)

The sampling campaign behind the phase-2 whole-core port decision (user
directive 2026-11-27: "查清整个 core 的函数级热点，产出阶段二整体搬迁所依据的热图").
Baseline: origin/main `8f0c65857`. All performance numbers come from
x86_64 + KVM (the platform-priority directive).

## Method

The in-tree tick sampler (`kernel/core/src/perf/mod.rs`) is tick-granular:
samples can only be produced at timer-IRQ boundaries, and the effective
period is floored at `TICK_NS = 1e9/KERNEL_HZ`. At the default
`kernel_hz = 100` a 1 ms request coalesces to 100 Hz (documented
behavior), so the sampling boots run a **`kernel_hz = 1000` build** — the
kernel is HZ-parameterized everywhere that matters (RR slice, RT
throttling, `dl_ns_to_jiffies`, TICK_NS), so a build-time override needs
no source change. The committed probe change (`test/perf_sample.c` in
`userspace/probes/probes.tar.gz`) requests `sample_period = 1 ms`; on a
standard HZ=100 kernel that stays tick-rate, on the sampling build it is
honored 1:1.

The dump records carry `PERF_SAMPLE_PERIOD`; decoding it verifies which
kernel actually ran: the reference window shows `period=10,000,000 ns`,
the 1000 Hz windows show `period=1,000,000 ns`.

Rig per window: one QEMU at a time, `-M q35 -accel kvm -cpu host`,
`-smp 2 -m 2G` (geekbench per its rig spec: `-smp 4 -m 4G`), the
recorder probe as PID 1 arming per-CPU cpu-clock sampling events
(kernel-mode only, with callchains), streaming the ring records into a
perf.data container, then `scripts/perf_report.py` per-function
self/total counts against the booted kernel's own ksyms snapshot.

Loads (each one sampling window):

| Window | Load | Kernel | Window | Samples (kernel-mode) | lost |
|---|---|---|---|---|---|
| pb100ref | pbench battery (reference A/B) | HZ=100 (period coalesced to 10 ms) | 35 s | 317 | 0 |
| pb1000 | pbench battery | HZ=1000 | 35 s | 878 | 0 |
| lm1000 | lmbench self-contained case loop (23 cases: syscall family, pipe/unix, ctx, fork/exec, sem, pagefault, mmap, lat_fs, mem_rd, bcopy/bzero, bw_pipe/unix/file_rd, usleep) | HZ=1000 | 60 s | 14,838 | 0 |
| gb1000b | Geekbench 5.5.1 `--cpu` (full CPU suite) | HZ=1000 | 90 s | 1,883 | 0 |
| bt1000 | boot: recorder execs `/sbin/init` (udesk-init, the image's real init) | HZ=1000 | 60 s | 198 | 0 |

Total union input: 17,797 kernel-mode samples across four windows
(pb1000 + lm1000 + gb1000b + bt1000); pb100ref is the methodology A/B,
not part of the union.

Artifacts (uncommitted, lane-local): `run/hotmap/` in the p23-hotmap
worktree — `out-<tag>/` per window (report.txt top-100 total view,
report-full-self.txt, report.json, serial.log, perf.data), the union
(`union.json` / `union.csv` / `union-report.txt`), the rig
(`hotmap_boot.py`, `inject.sh`, `hotmap_load.c`, window confs), the
HZ=1000 kernel (`k1000/bzImage`), and the injected image copies.
Committed rig: `scripts/test/hotmap_boot.py` (injected-image driver),
`scripts/hotmap_union.py` (union/tier/subsystem analysis), the probe
sources in the tarball.

## Threshold rationale

Per window: `hot_threshold = max(window_samples * 0.1%, 30 samples)`;
`warm_threshold = max(window_samples * 0.01%, 3 samples)`. With the
windows above the absolute floors dominate in every window (the largest
window, lm1000, would need >30,000 samples for the 0.1% rule to exceed
30). Meaning:

- **HOT** = a function reached ≥30 total samples (call-chain inclusive)
  in at least one window — i.e. ≥0.2% of that window's kernel time; the
  phase-2 mover sees real per-window cost evidence at function level.
- **WARM** = ≥3 total samples somewhere (0.04%–0.2%): borderline, listed
  for the coordinator to adjudicate per subsystem.
- **COLD** = sampled but never 3 samples in any window.

`total` (call-chain inclusive, deduplicated per sample — the reference
"children" semantic) is the primary ranking because the port cost of a
subsystem tracks where kernel time is *attributable*, and the self view
is distorted by the sampler's own interrupt-shadow bias (below).

## Union result

**619 functions sampled; HOT = 173, WARM = 330, COLD = 116.**

Per-subsystem summary (the direct input for the phase-2 whole-core move;
"samples" = sum of total counts over the four windows):

| Subsystem | HOT | WARM | COLD | samples |
|---|---|---|---|---|
| arch (trap/entry, x86_64 helpers) | 13 | 27 | 13 | 35,929 |
| syscall (dispatch, io, memory, process, time) | 27 | 13 | 8 | 25,623 |
| rust-core (core/alloc intrinsics: atomics, mem, ptr) | 34 | 108 | 32 | 9,095 |
| mm | 25 | 51 | 14 | 6,638 |
| fs (incl. pipe, ext4 rows) | 19 | 34 | 10 | 4,049 |
| core-misc (rux:: api shims, kernel misc) | 15 | 12 | 2 | 2,910 |
| sync | 14 | 31 | 9 | 2,583 |
| sched | 4 | 6 | 3 | 2,463 |
| process (signals, exec, wait) | 12 | 18 | 4 | 2,138 |
| debug (hotpath, trace, seccomp gate neighbors) | 6 | 2 | 5 | 1,142 |
| other (unclassified symbols) | 4 | 19 | 2 | 583 |
| perf | 0 | 2 | 1 | 58 |
| drivers | 0 | 3 | 12 | 50 |
| timer | 0 | 4 | 1 | 46 |

Top-15 union by total (per-window columns bt/gb/lm/pb = bt1000/gb1000b/
lm1000/pb1000):

| total | bt | gb | lm | pb | function |
|---|---|---|---|---|---|
| 16,571 | 195 | 1,866 | 13,702 | 808 | trap_handler |
| 10,446 | 129 | 150 | 9,555 | 612 | rux::arch::x86_64::cpu::enable_irq (BIAS — see below) |
| 6,594 | 194 | 1,803 | 3,952 | 645 | rux::arch::x86_64::trap::handle_syscall |
| 6,214 | 194 | 1,803 | 3,602 | 615 | rux::syscall::dispatch::syscall_handler |
| 5,697 | 194 | 1,803 | 3,097 | 603 | rux::syscall::dispatch::syscall_handler_inner |
| 4,581 | 194 | 1,799 | 2,028 | 560 | rux::syscall::dispatch::table_lookup |
| 1,997 | 2 | 150 | 1,469 | 376 | syscall::io::sys_read (+_inner, read_file_core ~1.9k) |
| 1,412 | 2 | 151 | 1,230 | 31 | core::sync::atomic::atomic_load::<u8> (GS-based per-CPU reads) |
| 1,331 | 0 | 1,327 | 0 | 4 | syscall::memory::sys_munmap (gb-only: mm teardown chain below) |
| 1,008 | 0 | 3 | 946 | 59 | check_and_deliver_signals |
| 919 | 0 | 38 | 810 | 71 | rux::sched::hotpath::current (+current_task_mut 869) |
| 794 | 0 | 790 | 0 | 4 | rux::mm::hotpath::free_pages |
| 691 | 0 | 27 | 610 | 54 | rux::arch::x86_64::cpu::get_thread_id |
| 646 | 0 | 29 | 566 | 51 | rux::arch::x86_64::smp::current_task_ptr |
| 619 | 3 | 5 | 611 | 0 | <rux::fs::types::File>::read_slice (+File::read 592) |

Per-window shape (what each load contributes):

- **lm1000** (60 s, syscall-loop heavy): the syscall entry floor
  dominates — trap/handle_syscall/dispatch/table_lookup ≈ 70% of total;
  read chain, getppid, seccomp gate (`seccomp_syscall_gate` 317),
  `<rux::trace::TraceEvent>::armed` 433 (tracepoint armed-read on the
  syscall path), `atomic_load::<u8>` 1,230 (per-CPU GS reads),
  `zerodev_read` 148 (lat_syscall write lands on /dev/zero's handler).
- **gb1000b** (90 s, real application phases incl. per-workload
  setup/teardown): **the mm teardown chain** — sys_munmap 1,327 →
  mm::policy::munmap → MmStruct::unmap_pages 1,152 → free_pages 790 →
  Zone::free_pages 588, remove_from_free_list 223, pfn_to_page_mut 211,
  page_remove_rmap 161, lru_del_page 148 — plus the mmap write lock
  (`RwSemaphore::down_write` 205, `optimistic_spin` 191,
  `spin_on_owner` 84) and ext4 cached reads (ext4_file_read_cached_dst
  144). This window is the only place mm shows real heat; lmbench
  microbenchmarks barely touch mm (26 samples in the lm1000 60 s).
- **pb1000** (35 s): pipe roundtrip (pipe_read_common/_user 376 each),
  the same entry-floor chain, sys_write 68 (write+fsync rows).
- **bt1000** (60 s): thin but real startup signature — do_execve 59
  (do_execve_elf 49, exec_set_leaf_flags_range 26, phys_to_virt 26),
  uart_write_kernel 38 (console during init), nanosleep 76 (init's
  sleep loop). The image's init idles after bringing the stack up; the
  full x86 desktop boot is not reachable from these test rootfs images
  (the desktop campaign runs on riscv64).

Self ranking excluding the enable_irq bias (the honest "where do cycles
actually sit" list) is headed by: `atomic_load::<u8/u64/usize/u32>`
(the per-CPU GS reads behind current/cpu_id/flags checks — 2,540
combined), `memset` 281, `x86_compat::translate` (syscall-number
translation) 219, `current_task_mut`/`current_task_ptr`/`gs_cpu_id`
(per-CPU task access), `mm::hotpath::nr_descs` 108 (virtio descriptor
window check), `RwSemaphore::spin_on_owner` 84, `TaskExtHandle::deref`
85 — i.e. the per-CPU access + entry-adjacent helpers are the
instruction-level hot spots, and the call-chain totals above are the
structural movers' list.

## Methodology findings (the pitfalls record)

1. **Tick-granularity floor**: the sampler cannot exceed KERNEL_HZ;
   a period below one tick coalesces to tick-rate (module doc says so;
   verified by the PERIOD field). 1000 Hz needs the HZ=1000 build. The
   HZ=1000 build is measurement-only: Kernel.toml is NOT changed on
   main (all gate bands and baselines stay HZ=100).
2. **100 Hz → 1000 Hz A/B (same pbench battery)**: identical process
   sequence (28,693 sideband records both boots), kernel-mode sample
   yield 317 → 878 (2.8x, not 10x — the two boots are not time-identical
   because HZ changes sleep rounding; and the 10 ms sampler's tick-lands-
   in-kernel probability differs from the 1 ms sampler's excursion
   capture). Per-function ranking is stable between them; the union
   consumes per-window relative thresholds, so the absolute yield does
   not affect tiering.
3. **enable_irq self-sample bias (interrupt shadow)**: at 1 kHz the
   LAPIC timer pending after a long IF=0 stretch fires at the first
   `sti`; the interrupted PC is then inside
   `rux::arch::x86_64::cpu::enable_irq`. 56% (pb100ref), 70% (pb1000),
   64% (lm1000) of self samples sit there; their callchains confirm the
   pattern (irq_restore lock-guard drops, schedule's sti, trap exit).
   These are IF=0-exit markers, NOT hot compute — hence total is the
   primary ranking and the self tables exclude enable_irq.
4. **geekbench feasibility**: Geekbench 5.5.1 boots and completes its
   full CPU suite on main (`8f0c65857` build) — all 21 workloads, plus
   the results-upload attempt. First sampling attempt (window 120 s):
   the recorder stalled before its window-end marker (no WINDOW/DONE
   print, no fsync, no dump) while the suite ran to completion; the
   boot then ended in a #GP panic stack (serial preserved:
   `run/hotmap/out-gb1000/crash-serial-gb1000.log`). The 90 s retry
   completed cleanly (DONE rc=0, lost=0). Suspect: the recorder racing
   the suite's saturation/killing phase — reported here as a P2
   sampler-robustness item for the coordinator, not diagnosed further
   in this lane. lost=0 in every completed window (the 128-page rings
   never overflowed at 1 kHz).
5. **lmbench compatibility**: all 23 self-contained cases ran in-loop
   without exec failures on the injected copy of the shared
   `lmbench-x86.img` (16 MiB scratch injected at /tmp/lmbench.file).
   Server-side cases (lat_tcp/udp/connect, bw_tcp) are excluded from
   the loop (they need the runner's server fork); coverage-priority over
   completeness — the syscall/switch/fs/mem families cover the entry
   floor and the fs/pipe paths.
6. **Window/end-of-window artifacts**: lm1000's serial log shows a
   duplicated `reboot: Power down` sequence after DONE (second reboot
   call from a leftover task; cosmetic, dump already fsynced). The
   recorder kills only its direct child at window end — grandchild
   workloads of a load can outlive it into the poweroff path (avoid
   window-end-sensitive loads; geekbench first attempt likely tripped
   here).
7. **Boot-window reachability**: the shared x86 test rootfs init
   (`/sbin/init` → static `udesk-init`) idles after ELF exec — no full
   desktop startup on x86 exists outside the riscv64 gnome image. The
   boot window stays thin (198 samples) and the startup signature comes
   from the exec path + pbench/lmbench battery startup phases.

## Reproduce

```bash
# in the p23-hotmap worktree (or any worktree at main + this tarball)
./scripts/extract-probes.sh                      # restore test/*.c
# sampling kernel: set kernel_hz = 1000 in Kernel.toml, then
PLATFORM=x86_64 make build && cp target/x86_64-unknown-none/debug/bzImage run/hotmap/k1000/bzImage
# (revert Kernel.toml afterwards; the committed default stays 100)
TC=toolchain/x86_64-rux-linux-musl/bin/musl-gcc
$TC -static -O2 -Wall -pthread -o run/hotmap/perfsample test/perf_sample.c
$TC -static -O2 -Wall -o run/hotmap/hotmap_load test/hotmap_load.c
# pbench window (driver builds its own probe rootfs):
(cd scripts/test && python3 ./perfsample_boot.py --arch x86_64 --accel kvm \
   --bzimage run/hotmap/k1000/bzImage --seconds 35 --tag pb1000)
# injected-image windows (lmbench / geekbench / boot):
cp --reflink=auto /home/william/rux-images/testimages/lmbench-x86.img run/hotmap/lm-sample.img
dd if=/dev/zero of=run/hotmap/scratch16m.img bs=1M count=16 status=none
/usr/sbin/debugfs -w -R "write $(pwd)/run/hotmap/scratch16m.img /tmp/lmbench.file" run/hotmap/lm-sample.img
run/hotmap/inject.sh run/hotmap/lm-sample.img 60 run/hotmap/lmbench.conf
python3 run/hotmap/hotmap_boot.py --img run/hotmap/lm-sample.img \
   --kernel run/hotmap/k1000/bzImage --tag lm1000
# union:
python3 scripts/hotmap_union.py run/perfsample-pb1000 run/hotmap/out-lm1000 \
   run/hotmap/out-gb1000b run/hotmap/out-bt1000 --json run/hotmap/union.json
```

Tier lists for the movers: `run/hotmap/union.csv` (every function with
tier, subsystem, per-window self/total columns).
