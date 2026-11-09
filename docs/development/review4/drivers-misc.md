# Round 4 review — r4-drivers-misc (drivers, interrupt, debug arsenal, top-level core, build/test infra)

Scope: Tier A deep (drivers/**, interrupt/**, timer, security, module, perf, trace, debug/**,
top-level kernel files, signal.rs) + Tier B quality pass (build.rs, Makefile, Cargo, Kernel.toml,
kernel/src/tests/**, kernel/verify/**, test scripts). Deduped against
docs/development/review-coverage.md and git log since 2026-10-14.

Findings appended incrementally. Severity scale: Critical / High / Medium / Low / Info.

Dedup: the MMIO-twin allocs+notifies inside `virtqueue.lock_irqsave` (arch2 round
FILED), the b125 PCI fix, GEM_CLOSE (2b2a4a52), PCI modern IDs (eaefd06f),
softirq budget (ddfa8abc), ohhunt gating (95094dd2) are treated as known.

---

## drivers/virtio

### [High] queue.rs:875-980 — wait_for_desc_completion sleep path has no wake-source deadline; 5000-wake budget is the class its sibling had fixed
`wait_for_desc_completion` (used by the MMIO `read_block`/`write_block` sync
paths, mod.rs:558/753 — the riscv64 boot-disk I/O when no PCI disks) sleeps via
`prepare_to_wait` + `schedule()` with NO deadline timer. If the completion's
interrupt is lost (masked line, IRQ-race, wedged softirq), no context ever
re-checks the used ring for this waiter — `schedule()` never returns and the
task sleeps forever holding whatever locks its caller held. The sibling
`wait_for_used_interruptible` fixed exactly this class: it arms
`add_timer_wakeup` (10 s) with a 1-jiffy fallback re-arm and documents why a
bare iteration budget is unsafe ("a 5000-wake cap could expire in well under a
second of false wakes... AC-2 leaked multi-block read failure family",
queue.rs:597-607). `wait_for_desc_completion` still uses `max_iterations = 5000`
(queue.rs:897) for the sleep path and arms nothing. Fix: port the deadline +
re-arm + waiter-side rescue (`pci_rescue_lagged_completions` equivalent for the
MMIO ring) from the sibling.

### [Medium] virtio_pci.rs:1467-1471/1723-1727/1935-1939 — PCI sync late-drain treats ANY used.idx advance as own completion; frees io_buf under a possibly in-flight chain — ✅ FIXED (`834f2fca`)
The Phase-3 late-drain in `read_block_once`/`write_block_once`/`flush_block_once`
breaks out when `idx != prev_expected` — i.e. when ANY chain's used-ring entry
landed, not necessarily THIS one. The file itself documents that QEMU virtio-blk
routinely completes out of submission order (`VIRTIO_PCI_REORDER_EVENTS`,
mod.rs:2245-2255): a later chain's completion during the 2M-iteration spin
makes `late = true` while our response byte is still 0xFF; the code then reads
status 0xFF, returns Err, and deallocates the 64-byte io_buf whose resp byte
the device may still DMA-write — the exact R21-N2 UAF class the surrounding
discipline exists to prevent (compare mod.rs:578-604, whose MMIO twin scans the
ring for `entry_id == submitted_desc_id`). The caller then retries into
descriptor slots while the abandoned chain may still be at the device.
Fix: gate `late` on our head descriptor appearing in the ring (or resp byte
!= 0xFF), not on the raw index delta.

### [Medium] virtio_pci.rs:889-1203 — dead one-shot read_block/write_block build an unregistered vring; a call would always time out and leak — ✅ FIXED (`0843bb2a`)
`VirtIOPCI::read_block`/`write_block` (per-call `VirtQueue::new(8,...)` +
`setup_queue` never called on it) have zero callers — all live paths go through
`read_block_using_configured_queue`/`write_block_using_configured_queue`. If
ever called, the fresh vring is never published to common_cfg (the device still
reads the probe-configured ring), so the submission is invisible to the device:
guaranteed `wait_for_completion` timeout, leaked vring on the timeout arm
(vring only freed on success), and freed header/resp while descriptors may be
consumed. ~315 lines of dead, wrong-by-construction code that contradicts the
file's own lifecycle rules. Delete.

### [Low] queue.rs:1060-1093 — in-flight admission guard trusts the corruptible shared avail.idx the submit path deliberately stopped trusting — ✅ FIXED (`2ceb0fe7`)
`alloc_desc_chain` computes `in_flight = avail_idx.wrapping_sub(used_idx)` from
`get_avail()` — a volatile read of the heap-backed shared ring — while
`submit_quiet` derives the published index from `avail_shadow` precisely
because a stray write into `avail.idx` "made the device walk garbage heads"
(queue.rs:992-995). A corrupted-low avail.idx under-reports in-flight chains
(saturating to 0), letting a third chain wrap onto a live chain's descriptors;
corrupted-high over-reports and refuses all I/O (the permanent "-5 forever"
family this guard's saturating math already defends against for leaks). Read
`avail_shadow` instead of the ring for the driver-side term.

### [Low] probe.rs:668-671 — leftover "AUDIT:" println in the hotplug rescan path — ✅ FIXED (`2ceb0fe7`)
`println!("AUDIT: rescan cfg_base=...")` fires on every `/sys/bus/pci/rescan`
with no debug gate; the codebase elsewhere went to lengths to keep the serial
log structured ("serial-clean" comments at probe.rs:439-443, 515-520). Drop or
gate it.

### [Info] queue.rs:84-87/187 — VirtQueue interrupt_status/interrupt_ack fields are write-only state
The two addresses are stored at construction (probe.rs passes `common_cfg_bar`
as a dummy for both) and never read anywhere; MMIO/PCI ISR handling reads the
device registers directly through their own paths. Dead fields; deleting them
removes the dummy-address plumbing.

### [Info] queue.rs:484-487 — POLLSTALL print labels preempt_count as "pc=0x" but prints decimal
`pdec(preempt_count())` after emitting `"pc=0x"` — decimal value under a hex
prefix; only affects forensics readability.

## drivers/intc + drivers/pci

### [Low] plic.rs:204-214 — trigger_ipi is dead code that RMWs the read-only PLIC pending array — ✅ FIXED (`2ceb0fe7`)
Zero callers. The PLIC pending registers are read-only per the spec (the write
is silently dropped or UNPREDICTABLE), and the RMW also races hardware setting
adjacent bits. IPIs go through SBI (`clint.rs send_ipi` → MSIP), and the
IRQ 11-13 PLIC handlers (arch/riscv64/ipi.rs:419-429) are the legacy compat
path. Delete `trigger_ipi` (and `read_pending`/`Plic::read_pending_word` if
their diagnostics consumers are gone too).

### [Low] plic.rs:223/301-305 — hart-count literals ignore config::MAX_CPUS in two places while the rest of the file uses it — ✅ FIXED (`2ceb0fe7`)
`static PLIC: Plic = Plic::new(PLIC_BASE, 4)` and `init()`'s
`for hart in 0..4 { for ipi_irq in 11..14 ... }` hard-code 4 harts, while
`plic_mask`/`plic_unmask` loop `0..crate::config::MAX_CPUS`. If MAX_CPUS ever
changes, contexts 4..MAX_CPUS-1 never get enables cleared at init and the IPI
enables miss harts — the same silent-truncation class clint.rs's comment
("Review CONS") already polices on its side. Derive from config::MAX_CPUS.

### [Info] pci/mod.rs:481-514 — every device class runs its own full 2048-function ECAM scan
`find_ecam_devices` (8 buses x 32 slots x 8 functions, 2 MMIO reads each) is
invoked separately by the blk, net, drm, and input probes — 4-5 complete walks
of the same config space per boot (~16K+ MMIO reads under TCG before any
device is even touched). One shared cached scan (vendor→ecam list) at boot
would remove the duplicate traffic. Functionally correct as-is.

### [Info] pci/mod.rs:520-574 — enumerate_virtio_devices duplicates the scan shape but covers bus 0 only
The riscv64-only legacy counter walks `RISCV_PCIE_ECAM_BASE + slot*0x8000 +
func*0x1000` with no `bus << 20` term (bus 0 only), diverging from
find_ecam_devices' 8-bus coverage; nothing validates the two stay in sync.

APIC (apic.rs) and GICv3 (gicv3.rs) read end to end: x2APIC probe/SVR
verification, MADT parse + BSP renumber, PIT-ch2 two-window calibration with
sanity clamps, byte-packed IPRIORITYR word building, redistributor
affinity-match walk — no defects found this round; cr3's timer_arm_oneshot
re-verification stands (PASS).

## drivers/drm

### [High] drm/mod.rs:2479-2494 — prime-fd mmap offset is unbounded: any process can map arbitrary physical memory past the GEM
`gem_mmap_resolve`'s prime branch returns `(gem.phys + offset, len.min(gem.size))`
with NO validation of `offset` against `gem.size`. The DRM-node branch right
below bounds it (`off = offset & 0xfff`, `len.min(size - off)`), but the prime
branch accepts any u64 offset: mmap on a prime fd with offset 0x1000000 maps
16MB past the object's physical page, RW, for gem.size bytes. Both /dev/dri
nodes are 0666, and any process can mint its own prime fd (CREATE_DUMB +
PRIME_HANDLE_TO_FD), so this is an arbitrary-physical-read/write primitive.
The reference kernel's dma_buf_mmap rejects `offset + len > dmabuf->size`
with EINVAL before calling the exporter's mmap. Fix: mirror the DRM-node
branch's bound (reject `offset >= gem.size` or clamp length accordingly).

### [Medium] drm/mod.rs:881-974/1998-2011/2058-2117/2285-2334/2476-2513 — six ungated bring-up trace families run on every boot; one scans the whole framebuffer every 10th flip forever — ✅ FIXED (`f88ff011`)
The known-gap-4 sweep (95094dd2) moved the prime-import content probe behind
debug=ohhunt, but left these unconditional: (a) `trace_seq` — first 128
ioctls per boot printed verbatim (pr_warn); (b) `trace_ioctl` — first-use +
every-100th-atomic/every-50th-dumb counters; (c) `add_fb_common` — 60 pr_warn
per boot; (d) `present_fb` — the content probe walks EVERY word of the GEM
(~3.7MB volatile reads, "a few ms under KVM") on the first 40 flips AND every
10th flip with no upper bound — at 60 fps that is 6 full-buffer scans per
second for the life of the session, on the presentation path; (e)
`mode_atomic` — first 60 commits + every 25th, building a String per traced
commit; (f) `gem_mmap_resolve` — first 30 maps. This violates the project's
own "debug infrastructure cost when OFF must be zero-ish" rule and pollutes
the printk ring (pr_warn level: every line also hits consoles with
loglevel<=4). Gate them behind debug=ohhunt (or a drm-specific switch) like
their sibling probes.

Fix shape: six new `drm:` tracepoints (addfb/flip/atomic/prime_import/
prime_mmap/open, events 62-67) now carry the families — (c)(d)(e)(f) gate
their expensive work on `EVENT_*.armed()` and emit the event, (a)(b) ride
`debug=ohhunt` (which also arms the drm: family via the OH_HUNT trace
alias), the VERSION first-8 pr_warn became `drm:open`. When nothing is
armed the cost is one atomic load per ioctl.

### [Low] drm/mod.rs:2410-2412 — sleep_until_ns busy-spins when the timer table is exhausted — ✅ FIXED (`2ceb0fe7`)
The `timer_id == 0` fallback calls `schedule()` with the task still RUNNING —
schedule returns immediately and the loop spins on
monotonic_ns_since_boot() until the vblank deadline (up to 16.6 ms of pure
spin), with no yield. Same discipline as wait_for_used_interruptible's
poll_fallback would suit (bounded spin + IRQs enabled is at least documented
there).

GEM_CLOSE shared-keep semantics verified as the r3b fix (2b2a4a52) — not
re-reported. ioctl size/direction tables (GETCRTC 104, GETENCODER 20,
GEM_CLOSE 4/8, PRIME 12/16, RMFB/PAGE_FLIP dual-encoding, pre-4.0 atomic
56-byte layout) re-checked against the file's embedded provenance comments —
consistent.

## drivers/gpu

### [Medium] virtio_gpu.rs:827-852 — used-ring completion poll reads device DMA memory through a non-volatile reference — ✅ FIXED (`6f42cc7b`)
The send_command wait loop does `let used = &*queue.used; ... if used.idx != idx`
— a plain (non-volatile, non-atomic) load of the used-ring index that the
DEVICE writes via DMA. Every other consumer of this ring in the tree uses
`read_volatile((used_ring + 2) as *const u16)` (queue.rs, virtio/mod.rs — 12+
sites). The per-iteration `fence(SeqCst)` happens to force a reload under
current codegen, but the load is not guaranteed to re-read by the abstract
machine; the same file's own comment history (the 65536-wrap fix directly
above) shows this loop is load-bearing for long sessions. Same issue on the
`avail.idx` read at line 781. Convert both to the read_volatile pattern.

### [Low] virtio_gpu.rs:863-875 — post-timeout commands can consume the leaked chain's late used-ring entry and accept a stale response
On timeout the io_buf is deliberately leaked (correct, R21-N2), but the next
command's `used.idx != idx` check treats the leaked chain's late completion as
the NEW command's completion: its snapshot `idx` equals the leaked ordinal's
used-ring position, so a late completion of the old chain returns immediately
with the fresh block (allocated with `alloc`, not zeroed) never written by the
device. The comment claims "a zeroed response, which callers reject" — the
alloc is uninitialized, and because the recycled heap block very plausibly
holds a PREVIOUS response of the same command type, the hdr_type check can
pass and the caller consumes a stale response. Match by descriptor id (the
used-ring entry's id field, as the blk walkers do) or zero the resp half.

### [Low] virtio_pci.rs:588-609 — set_status/get_status/reset_device access device_status as u32, overwriting queue_select — ✅ FIXED (`2ceb0fe7`)
virtio_pci_common_cfg's device_status is a u8 at offset 20; config_generation
(u8, offset 21) and queue_select (u16, offset 22) follow. The 4-byte write in
`set_status(ACKNOWLEDGE|DRIVER|FEATURES_OK|DRIVER_OK)` also writes 0 into
queue_select — harmless in today's call order (queues are selected explicitly
afterwards), but any future caller that sets status after selecting queue N
silently de-selects it. virtio_gpu.rs's init correctly uses u8 accesses; make
virtio_pci.rs match.

### [Info] framebuffer.rs + fb_simple.rs — the FrameBuffer drawing API and simple-fb probe are dead code with a footgun
No callers of `get_framebuffer()`, `put_pixel`, `fill_rect`, `clear`,
`probe_simple_framebuffer`, or `create_framebuffer` outside the gpu module.
`FrameBuffer::new` stores the PHYSICAL address as a raw dereferenceable
pointer ("assume identity mapping") — if anyone ever calls put_pixel on a
system where phys != identity-VA, it writes to arbitrary physical memory.
~330 lines; delete or convert to phys_to_virt at construction.

## drivers/input

### [Low] virtio_input.rs:426/441-442 — avail.idx store and has_event's used.idx load skip the volatile discipline used everywhere else — ✅ FIXED (`2ceb0fe7`)
`read_event` writes `avail.idx = ...wrapping_add(1)` as a plain store (line
426) while the sibling code at line 384 and the whole of queue.rs use
`write_volatile` for the same field; `has_event` (line 442) reads
`used.idx` — DMA-written memory — with a plain load, where `read_event`
itself correctly uses `read_volatile` four lines below. Functionally
tolerated today (each call re-enters through a lock; read_event re-checks
volatily), but the plain used.idx read is exactly the pattern that lets a
future LTO pass cache the DMA-visible load across the poll loop's
iterations. Make both volatile for uniformity with the rest of the vring
code.

### [Low] virtio_input.rs:571-604 + input/mod.rs:274-310 — standalone probe and legacy poll APIs are dead, and one drops pointer-only results — ✅ FIXED (`5512b361`)
`probe_virtio_input_devices`, `probe_virtio_input`, `get_keyboard_event`,
`get_pointer_event`, and `input::poll_events` have no callers (the live path
is init_virtio_input's own probe + the tasklet BH + evdev's
poll_virtio_events). The dead `probe_virtio_input_devices` additionally has
a latent bug: `Some((keyboard?, pointer))` discards a pointer-only probe
result (the `?` propagates None when no keyboard was found even though a
tablet was). Delete the dead surface.

### [Info] evdev.rs:343-388 — blocking read returns exactly one event per call and sleeps on a 10 ms timer, not the poll wait queue
Linux evdev_read fills the caller's buffer with as many complete
input_events as fit and blocks on the device wait queue; Rux returns one
24-byte event per read() and its blocking loop re-arms a 10 ms timer per
sleep_ms_interruptible round (poll_waitq exists and push_event wakes it, but
the read path never registers). Correct-but-chatty for libevdev-style
readers; input latency bounded at ~10 ms instead of event-driven.

InputEvent layout (24B uapi), event code tables, EVIOCGBIT/CGABS/CGKEY
families, GRAB/LED semantics, sysfs bitmap formatting (input_print_bitmap
word order) all verified against the Linux uapi — no defects. ps2.rs is an
explicitly inert stub.

## drivers/timer + kernel/src/timer.rs

### [Medium] timer/x86_64.rs:283-314 — resync_tsc_against_hpet's ">25 s lead" rebase arm is unreachable: the else-if chain shadows it — ✅ FIXED (`6f42cc7b`)
The chain is: (1) `hpet > tsc` → step forward; (2) `else if tsc > hpet &&
tsc - hpet > 2_500_000` → REPORT ONLY; (3) `else if tsc - hpet >
250_000_000` → pull back TSC_CPU_OFF. Any lead > 250M ticks also satisfies
condition (2) (> 2.5M), so arm (3) — the "vCPU TSC smash" last-resort
correction the comment promises ("a lead >25 s is pulled back as a last
resort") — can never execute. In the exact scenario the code describes (a
real vCPU TSC-offset jump), the clock would only be reported (budgeted at 8
prints per boot via resync_report) and never corrected. Fix: bound the
report arm to `<= 250M` (`> 2_500_000 && <= 250_000_000`) so the rebase arm
becomes reachable.

Timer drivers otherwise read end to end (riscv64.rs, x86_64.rs, hpet.rs,
aarch64.rs, timer.rs facade, wheel.rs collect/cascade/advance/min_hres):
jiffies-from-timebase derivation (multi-CPU idempotent), grid-glued
re-arming, HPET read hardening (validated base pair, DIV/offset complements,
per-CPU monotonic clamp with small-backward vs correction split), the
multiply-then-shift TSC mapping, and the wheel's DUE-carry advance +
budgeted collect + in-place periodic re-arm are all sound. The softirq
heartbeat's debug census calls are switch-gated or anomaly-gated as
documented.

## interrupt/

All eight files read end to end (irqdesc, irqchip, domain, preempt, softirq,
tasklet, ksoftirqd, mod) — no new defects this round. The previously-fixed
items re-verified in place: per-vector 2 ms budget with leave-pending on
budget break (ddfa8abc), the SOFTIRQ_OFFSET mid-window task-switch repair
(166e4516, softirq.rs:346-367), the tasklet SCHED-clear/RUN-set CAS pair
(reviewround 8), ksoftirqd's state-first sleep with NEW-C2 dequeue
compensation, and irq_exit's base-count deferral with the 20 ms starvation
escape hatch (preempt.rs:383-402). Shared-chain dispatch holds the action
lock across handlers (acceptable: handlers are top-half-only by design, and
free_irq's synchronize relies on it).

## drivers/net (device side)

### [Medium] virtio_net.rs:357-517 — every TX packet synchronously spins for device completion under tx_queue lock_irqsave — ✅ FIXED (`6a593a60`)
xmit() holds `tx_queue.lock_irqsave()` across submit → notify →
wait_for_completion_max(200K iters) → 500K late-drain — external interrupts
disabled for up to a few ms of TCG time PER PACKET on the sending CPU
(acknowledged in-code: the budgets were only reduced from 10M/50M). The
Linux shape is asynchronous: start_xmit posts the chain and reclaims
completed descriptors on the NEXT xmit/interrupt (free_old_xmit_skbs), so no
TX ever spins with IRQs off. A UDP/TCP send loop is a train of IRQ-blackout
windows; the sync design also caps effective TX to one in-flight frame. The
R21-N2 leak-on-timeout discipline and the R22/R23/R24 buffer-leak fixes are
all correctly in place — this is purely the wait architecture.

### [Low] space.rs:227-252 — netdev lookup registry only knows "lo"; eth0 is unfindable through it — ✅ FIXED (`5512b361`)
`get_netdevice_by_index` maps only ifindex 0 → loopback and
`get_netdevice_by_name` only "lo"; the eth0 NetDevice registered by
publish_eth0 (which merely bumps DEV_COUNT and assigns ifindex 1) is
invisible to both. Today no live caller uses these lookups (the ethernet
layer dispatches directly to loopback_send/virtio xmit), so this is latent:
the first ioctl consumer (SIOCGIFCONF/getifaddrs shaping) would enumerate a
loopback-only world. Either maintain a real device list or delete the
registry.

### [Info] virtio_net.rs:547-561 — bogus-descriptor-id path pops the newest rx_buffers entry, not the consumed one
On a device-returned bogus UsedElem.id, the handler pops the LAST entry of
rx_buffers and frees it — that is a different, possibly still-posted buffer
(desc armed with its phys) while the actually-consumed buffer's slot is
left accounted. Count stays right; address-level mismatch only matters if
the device is already returning garbage ids. Fine as a degraded-device
recovery path; worth a comment.

virtio_net MMIO+PCI init (feature negotiation, queue-role split rx0/tx1,
split-address programming, IRQF_SHARED INTx), poll()'s
recycle-on-every-exit discipline, the stats/rx_queue lock-order fix, and
loopback's backlog + single-consumer poll design all verified — sound.

## blkdev + misc drivers

### [Low] loop_dev.rs:105-157 — two distinct GenDisk objects exist for loop 0; the buffer cache can key the same blocks twice — ✅ FIXED (`ae7a0f7a`)
init_loops leaks one GenDisk per loop (loop_disk(idx), major 7,
first_minor=idx) AND registers a separate representative GenDisk (major 7,
minor 0) in the block manager for i==0. Anything resolving the disk through
blkdev::get_disk(7) gets a DIFFERENT object than loop_disk(0) — and the
buffer cache keys on the GenDisk pointer, so the same physical block read
through both paths gets two unrelated cache entries (coherency hazard for
any future get_disk(7) consumer; today mount/mkfs go through loop_disk, so
latent). Register the per-idx disks and drop the representative, or make
get_disk(7) return loop_disk(0).

### [Low] loop_dev.rs:306-348 — request path caps the transfer START at the sizelimit boundary but not the LENGTH — ✅ FIXED (`2ceb0fe7`)
loop_request_fn clamps byte_start to offset+limit but never bounds
req.buffer.len() against the limit: a block read/write past the sizelimit
edge reads/writes backing-file bytes beyond it (Linux enforces lo_sizelimit
through the device size). The loop_file_read/write paths are also
unbounded beyond what the short-read zero-fill patches. Edge-case semantics
only (LTP always uses limit 0), worth a clamp for parity.

rtc.rs (goldfish tear-free two-half read, alias discipline, plausibility
floor), watchdog.rs (non-firing softdog uapi), access_tokenid.rs (OH
bookkeeping ioctls with registry mirroring), ashmem.rs (pin/unpin range
algebra port, SysV-style pool refcounts, atomic-reserve mmap), blkdev/mod.rs
(R8-M2 error plumbing, iostat/tracepoint bracketing), and iostat.rs read
end to end — no defects beyond the two above.

## security/ + module/ + perf/ + trace/

security/ (capability, lsm, cap_lsm, mod): the 41-cap table re-verified
against the UAPI numbering; LSM chain dispatch, the capability-only fast
gate, and can_send_signal's kill_ok_by_cred shape — no defects.
module/mod.rs: registration-only kmod port (ET_REL validation, .modinfo
name extraction, sysfs/proc exposure, careful not-twice-negated errnos) —
no defects.
perf/mod.rs: perf_event_open software-clock slice read end to end — attr
head layout (type/size/config/sample_period/sample_type/read_format/flags1
at the ABI offsets), paranoid-2 CAP_PERFMON gate, lock-free hard-IRQ tick
stash + softirq charging, lost-record-first drain, register/unregister fd
error unwinding — sound.
trace/ (mod.rs 2954 lines + files.rs): the tracepoints! macro table (61
events, verbatim reference print fmts/field rows, slot packing with the
SLOTS<=TRACE_ARGS compile assert), per-CPU rings with documented
lock-free-tearing trade, one-relaxed-load disabled-path cost, snapshot/pipe
surface — sound; the recently-grafted lock:/tcp:/sock: wave (ids 52-61)
keeps the packing discipline. No defects.

## debug/ (the arsenal)

### [Info] debug/ — architecture verdict: disciplined, growing, but not yet debt
Coverage this round: switches.rs, mod.rs, pstore.rs, oops.rs, ksyms.rs,
softlockup.rs read end to end; leakorigin (prior cr3 ✅), and the remaining
watch-family files (memwatch, scribble, syscost, taskdump, schb, fsw,
forkprof, mmprof, cctx, wildwatch, metawatch, allocwatch, r83hunt,
monowatch, driftwatch, lostpark, pathstash, scmtap, slowtask, treehunt,
ledger, filerd, backtrace, taint, bug, hexdump, hung_task, sbi_debug,
cctx/unixrd/lockdep) structurally surveyed (gating + entry points + print
budgets). The design question the round asked — is the watch family
accumulating tech debt? — answer: the load-bearing discipline holds. The
single-source switch table (63 switches, name-keyed, macro-derived, debugfs
knob per switch, trace-alias arming), the armed()-first call contract, the
direct-console putchar_no_lock escape for wedged contexts, and the pstore
magic-last capture discipline are all coherent; the just-fixed mmprof
always-on thread regression (debug/mod.rs:146-166) shows the failure mode
that discipline exists to prevent, and the DRM trace families (reported
above under drivers/drm) are currently the main ungated offenders. Sizing
trend is the real risk: 18.6K lines / 63 switches grows by 2-4 instruments
per hunt; retire the closed hunts' probes (the ledger's do-not-delete rule
protects them, but a debug=retired archive directory would keep the tree
navigable).

Findings detail: pstore's DTB-anchored carve (initrd-overwrite limitation
documented), oops' re-entrancy latch + policy split, ksyms' target-triple
guard + gap-aware binary search, and softlockup's idle/no-work skips + R57
IRQ-liveness table all verified — no defects. bug/hexdump/taint/sbi_debug
trivially clean. The debugfs provoke-oops knob is properly gated behind
debugfs (root-only in practice).

## Top-level kernel files

### [Low] printk.rs:1281-1299 — /dev/kmsg read heap-allocates the record header while holding the printk ring irqsave lock — ✅ FIXED (`2ceb0fe7`)
kmsg_file_read holds `RING_BUFFER.lock_irqsave()` across the whole record
build, and the header line uses `alloc::format!` (line 1289) — a heap
allocation under an IRQs-off spinlock on every /dev/kmsg record read. Under
the measured hilogd reader load (92K reads/s, printk.rs:1196-1203) that is
alloc + lock traffic on the printk path every record. Every sibling path
already does it right: format_syslog_header and persistent_log::Line format
into stack buffers, and proc_kmsg_read holds the lock only for plain copies.
Use the stack header formatter and keep the lock hold copy-only.

signal.rs: the do_signal selection loop, restore_saved_sigmask ordering,
ptrace interception, unkillable-init filter (incl. ns-local pid 1 +
same-namespace-kill discard + SIGSTOP/SIGCONT piercing), per-arch frame
builders (riscv64 SigContext with no sc_status ABI break, aarch64
fpsimd-record + 0x120/176 layout asserts, x86_64 pretcode+retcode), restart
sentinel handling on both handler and no-handler paths, group-stop
park/resume with WCONTINUED bookkeeping, and the SI_USER/SI_KERNEL sender
split re-verified — no new defects beyond the note that the budgeted
DEQUEUE (120) and SIGKILL-SEND (80) prints (signal.rs:1343/2810) are
ungated bring-up traces in the same family as the DRM ones; their budgets
are small enough to keep, but they belong behind a switch like their
siblings. console.rs: RX ownership (IRQ-armed vs poll fallback), SPSC ring
with CAS consumer, ISIG record-then-deliver, TX stall latch with 32-byte
probe batching, pre-arm FIFO rescue drain — clean, matches cr3 PASS.





main.rs (boot tail, panic path), init.rs (init-process load), initrd.rs
(gzip/cpio unpack end to end: puff-style DEFLATE with fuzz-verified
DIST_EXTRA, CRC32/ISIZE checks, bounded output, traversal rejection,
deferred hardlink resolution), cmdline.rs (API surface), panic_policy.rs,
print.rs, sbi.rs, list.rs, errno.rs, config.rs (generated) — all read;
no new defects. The RUX-RISCV-LAYOUT image/heap overlap guard and the
PFN-span (vs sum) sizing with the phantom-drift comments are exactly the
kind of boot-time cross-checks a review wants to see.

## Tier B — build/test infrastructure (quality pass)

### [Info] kernel/Cargo.toml features — debug-lock-owner and friends documented, zero-cost claims verified by cfg
The compile-time debug features (debug-lock-owner, debug-taskdump-bt,
debug-futex-trace, debug-aa64-uaccess-selftest) are all opt-in with
documented costs; the runtime switch table carries the rest. No gap found
between Kernel.toml keys and build.rs consumption (MB→bytes conversions at
build.rs:659-670 with per-arch heap clamps; platform feature-vs-config
consistency guards at build.rs:186-204 catch the stale-.config class).

### [Info] kernel/src/tests/** — real harness, real assertions
The unit-test harness (tests/mod.rs) keeps pass/fail/skip counters with a
failed-test list and named assertions (test_assert!/test_assert_eq! record
instead of panicking — the right shape for a guest-side runner). Spot
checks (virtio_queue layout asserts, scheduler vruntime, syscall_memory)
show live assertions, not dead stubs. kernel/verify/ is a genuine proptest
+ Kani + SPIN harness with per-domain model files and a documented
"copied from kernel source" discipline policed by
scripts/verify_sync_check.py (run by make verify/miri/kani). scripts/test/
holds 150 entries (86 shell scripts) — inventory only, per scope.

### [Info] .cargo/config.toml — `-Z ub-checks=no` on kernel targets
Disables rustc's UB-check instrumentation on the two kernel targets
(standard for a no_std kernel that does its own MMIO); noted because it
means unsafe-code UB inside the kernel is not caught by builds — the
review-level discipline (SAFETY comments + verify/Miri on copied models)
is the only check. Acceptable as-is; keep Miri in the routine.

---

# Totals

Findings this round: 25 total.
- High: 2 (drm prime-mmap physical-memory exposure; virtio queue
  wait_for_desc_interruptible wake-source hole)
- Medium: 5 (virtio PCI late-drain false-completion UAF window; dead
  one-shot read/write_block; TSC resync rebase arm unreachable; virtio_net
  sync-TX-under-irqsave; DRM ungated trace family)
- Low: 9 (PLIC dead trigger_ipi; hart-count literals; virtio_input
  volatile discipline + dead probe API; avail.idx corruptible admission
  guard; probe.rs AUDIT print; loop_dev dual-GenDisk + sizelimit length;
  netdev registry lo-only; bogus-desc rx pop; evdev single-event read;
  sleep_until_ns spin; printk /dev/kmsg alloc-under-lock — 12 items,
  counted within the per-file sections above)
- Info: 9 (incl. the debug-arsenal architecture verdict)

# Three worst

1. drm/mod.rs:2479-2494 — prime-fd mmap offset unbounded (High, security):
   any process can map arbitrary physical memory past a GEM by passing a
   large mmap offset on a prime fd it minted itself (CREATE_DUMB +
   PRIME_HANDLE_TO_FD on the 0666 /dev/dri nodes). The DRM-node branch
   right below bounds the same input; the prime branch forgot. Fix is one
   bounds check.
2. queue.rs:875-980 — wait_for_desc_interruptible has no wake-source
   deadline (High, hang): the MMIO sync read/write path's wait can sleep
   forever on one lost IRQ; the sibling function solved exactly this with
   add_timer_wakeup + kick/walk recovery, and this one was never ported.
3. virtio_pci.rs:1467-1471 — late-drain treats any used.idx advance as
   own completion (Medium, UAF window): out-of-order completion (routine
   on QEMU, counted by the driver's own VIRTIO_PCI_REORDER_EVENTS) plus a
   timeout race frees the 64-byte io_buf while the device may still DMA
   its response byte into it — the exact R21-N2 class the file polices
   everywhere else.

# Coverage appendix

Tier A (deep, end to end unless noted):
- drivers/virtio/: queue.rs, mod.rs, virtio_pci.rs, probe.rs, offset.rs —
  full ✅
- drivers/intc/: plic, apic, gicv3, clint, mod — full ✅
- drivers/pci/mod.rs — full ✅
- drivers/drm/mod.rs — full ✅
- drivers/gpu/: virtio_gpu, fbdev, framebuffer, fb_simple, virtio_cmd,
  mod — full ✅
- drivers/input/: evdev, event, mod, ps2, virtio_input — full ✅
- drivers/timer/: mod, riscv64, x86_64, hpet, aarch64 — full ✅;
  kernel/src/timer.rs full ✅; timer/wheel.rs collect/cascade/arm/remove/
  min_hres/overdue read, first 760 lines (types/traits/link plumbing)
  structure-passed 🔶
- drivers/net/: virtio_net, loopback, space, mod — full ✅
- drivers/blkdev/: mod ✅, iostat ✅ (prior-round authored, re-skimmed)
- drivers/ misc: rtc, watchdog, access_tokenid, ashmem, loop_dev,
  mod ✅
- interrupt/: all 8 files full ✅
- security/: all 4 full ✅; module/ full ✅; perf/ full ✅
- trace/: mod.rs full ✅ (2954 lines incl. all 61 event declarations);
  files.rs 🔶 (prior-round authored; switch/alias surface re-checked via
  switches.rs)
- debug/: switches, mod, pstore, oops, ksyms, softlockup full ✅;
  leakorigin ✅ (cr3 PASS re-confirmed via preempt.rs hooks); remaining
  20 files structural survey 🔶 (gating, entry points, print budgets —
  see the debug/ section)
- Top-level: printk ✅, console ✅, signal ✅ (selection/disposition/frame
  build/restore/send-path families; the 3 per-arch restore_sigcontext
  bodies spot-verified against their frame builders), initrd ✅, main ✅
  (boot tail + panic; middle device-init block 🔶 read once), cmdline 🔶
  (API + init/get_param paths; FDT parser structure), init ✅,
  panic_policy ✅, print/sbi/list/errno/config 🔶 (trivial/generated).

Tier B (quality pass): build.rs ✅ (full), Makefile ✅, .cargo/config ✅,
kernel/Cargo.toml ✅, Kernel.toml ✅ (sections + memory/smp/drivers keys
cross-checked against build.rs), tests/mod.rs harness ✅ + spot checks,
kernel/verify 🔶 (lib.rs + structure), scripts/test inventory only
(150 entries), build/ scripts 🔶 (Makefile/gen-ksyms referenced through
build.rs reading; not independently audited).

Dedup honored: PCI modern IDs (eaefd06f), GEM_CLOSE (2b2a4a52), softirq
budget (ddfa8abc), ohhunt gating (95094dd2), MMIO-under-lock FILED item
(arch2), mmprof thread gating (97c95c30 fix verified in tree), SOFTIRQ
bracket repair (166e4516), switch index collision split (historical note
in switches.rs) — none re-reported.
