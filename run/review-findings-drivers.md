# Full-speed review: drivers (rv-drivers lane, 2026-10-31)

Scope: kernel/src/drivers/** — virtio (blk/net/gpu/input), pci, timer,
intc (plic/gicv3/apic), block layer, drm. Base: main @ 2e5e7c2d.
Verification per kernel change: `make build`, `make build PLATFORM=x86_64`,
`make build PLATFORM=aarch64` — all rc=0 on the final committed tree.
Targeted tests: `test/drm_probe_boot.py` PASS 31/31 (x86_64, virtio-gpu +
full DRM ioctl surface incl. prime import/export and the atomic flip);
`test/boot-soak-fast.py` minimal riscv64 boot (LOOP/CHURN/ARITH all DONE,
no panic attributable to the lane — see F8).

## FIXED (commits on rv-drivers)

- **F1 (fixed, f93243d6) virtio/queue.rs — `wait_for_desc_completion`
  missed the R36-B2 runqueue re-queue compensation.** The re-check path
  after `prepare_to_wait` (descriptor completed before sleeping) called
  `finish_wait` but not `sched::dequeue_task`, unlike its siblings
  (`wait_for_used_interruptible`, `wait_event!`): a racing
  `wake_up_all` between prepare and re-check enqueues the still-running
  task and leaves nr_running inflated until the next context switch.
  Gates: sibling-function contract in-tree; 3 builds rc=0; no behavior
  change unless a racing wake fired (dequeue is a no-op otherwise).

- **F2 (fixed, 95778e17) gpu/virtio_gpu.rs — three command-transport
  defects.**
  1. Notify address `queue_index * multiplier * 2` — used the queue
     index instead of queue_notify_off and scaled the byte-valued
     multiplier a second time (the exact "notify ×2" review BUG the
     tree fixed in `VirtIOPCI::get_notify_addr`; latent only because
     QEMU sets multiplier=0). Now routes through `get_notify_addr`.
  2. Completion poll `used.idx as usize >= idx + 1` breaks at the u16
     wrap: at idx=0xFFFF the completed value 0x0000 never satisfies the
     comparison, so every command from the 65536th submission times out
     after 10 s. A GUI session (2-3 commands per flip) reaches the wrap
     in hours. Now `used.idx != idx` (wrap-safe).
  3. Timeout path freed the DMA staging block while the chain was still
     submitted — a late device response DMAs into reallocated memory.
     Now leaks on timeout (R21-N2 discipline).
  Gates: virtio 1.0 spec 4.1.4.4 / vring wrap semantics + in-repo
  R-numbered discipline; drm_probe 31/31 PASS exercises this exact path
  (get_display_info through resource_flush) on the fixed tree.

- **F3 (fixed, 244250d8) drm/mod.rs — kernel OOB read in the
  bring-up content probes.** The prime-import and flip probes sampled
  fixed pixel offsets from hardcoded 720p/3072-pitch geometry (word
  offsets up to ~492k) on every imported/flipped GEM regardless of
  size; any GEM smaller than ~1.97 MB (imported textures, small FBs)
  read past the heap allocation on a user-triggerable path. All sample
  offsets now clamped to the object's word count. Gates: memory-safety
  by construction; probe PASS 31/31 (incl. prime import path).

- **F4 (fixed, 18eaa659) drm/mod.rs — flip-path oracle painter.** The
  compose-content oracle painted a banded checkerboard over the flipped
  GEM's visible pixels on flips 1..=60 and every 5th flip forever.
  Bring-up tooling that, as permanent code, corrupts the composer's
  real frame content on a cadence (breaks the GNOME goal and any client
  verifying its own output). Removed; read-only nz/paint counters kept.
  Gates: user-buffer integrity (no upper-layer breakage); the drm probe
  paints and verifies its OWN pattern — PASS 31/31 without the oracle.

- **F5 (fixed, db035444) net/virtio_net.rs — riscv64 net-pci INTx
  registration not shared.** init_pci_block_devices claims the gpex
  swizzled lines (32..35) first with IRQF_SHARED; the net function then
  registered with flags=0, request_irq refused ("IRQ already in use and
  not shared"), and `.ok()` swallowed it — no top half, RX interrupts
  never fire, on any config where virtio-net-pci shares a line with a
  virtio-blk disk. Now passes IRQF_SHARED (handler already demuxes by
  its own ISR read). Gates: shared-IRQ contract of the in-tree IRQ
  framework (irqdesc.rs line 213) + blk's documented swizzle; build-only
  change on configs without line collisions.

## FILED (not fixed — outside lane or design-level)

- **F6 virtio_pci.rs `VirtIOPCI::read_block`/`write_block` (per-call
  vrings) are dead code** and were never register-programmed
  (`setup_queue` is not called for their ad-hoc vrings — a device never
  learns those ring addresses). No caller reaches them (only
  `read_block_using_configured_queue`/`write_block_using_configured_queue`
  are used). Left alone: removal is cleanup, not a defect.

- **F7 plic.rs `trigger_ipi` writes the PLIC pending array**, which is
  read-only per the PLIC spec (writes are ignored by QEMU). Harmless
  no-op in practice; flagging for whoever owns the IPI path.

- **F8 (pre-existing, reproduced on this lane, NOT caused by the lane's
  changes)** riscv64 fork/exec churn occasionally executes a freed
  Task: `KERNPANIC pfault epc=0xe ra=0xf` with pid=0xDEADBEEF,
  preempt=0xc3c3c3c3, comm='[' — the sc186/sc189 thread-exit family
  (context switch rets through a reused thread.ra; symbolized to
  arch/riscv64/context.rs switch path + thread.rs vstate save).
  Evidence: 2 panics in 2 boots, then a full clean pass
  (LOOP-DONE/CHURN-DONE/ARITH-DONE) on the same fixed kernel, and the
  unmodified base also passes — a racy sched/task-lifetime bug. None of
  this lane's four changed files execute in the failing configuration
  (PCI virtio-blk only; no virtio-mmio, gpu, drm, or net devices, so
  `wait_for_desc_completion`, virtio_gpu, drm, and virtio_net code
  paths are all unreachable). Belongs to the sched/exit lane.

## FALSE-POSITIVES retracted during review (suspected own reading first)

- **virtio_mmio `enable_device_interrupt` flags=0 (not shared)** — MMIO
  IRQs are one-per-device-slot (IRQ 1-8), never shared; no fix needed.
- **`flush_block_once` allocating with `alloc_desc_chain(2)` under a
  3-wide window check** — conservative overlap check on purpose
  (comment says so); 2-desc chain inside a 3-wide window is safe.
- **`wait_for_desc_completion` sleeping without a deadline timer** —
  analyzed: its callers (MMIO read/write) run their own bounded
  late-drain after the iteration budget; not the 10s-strand shape of
  `wait_for_used_interruptible` (whose callers sleep unboundedly).
  Improvement possible but not a defect; left as is.
- **`submit_event_buffers`/`read_event` (virtio_input) poking rings
  directly, one non-volatile `avail.idx` store in `read_event`** — the
  store cannot be elided (pointer escapes into the shared queue) and
  the following SeqCst fence + MMIO notify orders it; cosmetic only.
- **`pci_pending_slot_reservable` vs `pci_publish_sync_chain` gap** —
  both run under the same BLK-lock hold; the documented invariant
  (only abandon_pending_completion can touch the table between them,
  and it never frees a slot) holds on re-derivation.
