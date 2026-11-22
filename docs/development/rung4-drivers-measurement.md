# Rung 4 (drivers) — crate-separation measurement (2026-11-22)

Method: per the crate-plan (docs/architecture/crate-plan.md), measure the
REVERSE surface of `kernel/src/drivers/` — what the framework tree
consumes FROM drivers and what drivers require back — to predict the
relocation cost of the drivers crate split. Zero QEMU: pure code reading
+ grep census over the L3 worktree at 3dc8c534e.

## 1. Raw size

- 40 `.rs` files, 26,105 lines total.
- Unsafe dual-census residue: 376 lines (post round-8/9; the F-rows for
  the protocol edges are registered at source).

## 2. Framework → driver references (the reverse surface)

**780 references from 127 framework files** (everything outside
`kernel/src/drivers/`). By family:

| Family | Framework files | Refs | Dominant items |
|---|---:|---:|---|
| timer | 90 | 480 | `get_jiffies` (324), `read_time` (45), `monotonic_ns_since_boot` (41) |
| blkdev | 12 | 51 | `GenDisk` type consumed by fs/ext4, mm/swap, fs/bio, fs/jbd2 |
| virtio | 20 | 48 | `pci_blk_kick(_all)`, `get_pci_gen_disk`, completion/flush |
| intc | 15 | 47 | `apic::eoi`, timer-event probes, irqchip ops |
| gpu | 11 | 24 | `get_framebuffer_info` (15) |
| net | 11 | 27 | `get_device` singletons (virtio_net, loopback) |
| rtc | 9 | 26 | `wall_secs` (19) |
| drm | 7 | 20 | drm ioctl entry from sysfs |
| loop_dev | 7 | 18 | |
| input | 6 | 16 | event injection from console/evdev paths |
| ashmem | 4 | 9 | IPC ioctl |
| pci | 0 | 0 | (pure driver-internal; nothing outside references it) |

**Key finding: 62% of the reverse surface is the timer family** — and
90% of that is three pure functions (`get_jiffies`, `read_time`,
`monotonic_ns_since_boot`) that read an AtomicU64 static the arch timer
driver maintains. These are clocksource *reads*, not device operations:
in the reference kernel they live in the TIMEKEEPING layer
(`kernel/time/`), not in drivers — the drivers crate only provides the
tick that increments them. **Pre-split move: hoist the jiffies/clock
statics + readers into `kernel/src/timer.rs` (framework) and let
drivers call `timer::tick_update()` — this alone removes ~440 refs
(56%) from the reverse surface and drops the touching-file count from
127 to ~60.**

## 3. Entry seams (how a driver registers)

| Seam | Shape | Already table-driven? |
|---|---|---|
| **IRQ** | `interrupt::request_irq(irq, fn(u32,usize)->IrqReturn, flags, name, dev_id)` — IrqAction list per IrqDesc, RT-threading opt-in | **Yes** — the reference's action-list shape; drivers register at probe, no framework code names a driver |
| **Block** | `GenDisk { ops: Option<&'static BlockDeviceOps>, request_fn, async_read_fn, async_write_fn, async_*_sg_fn ... }` registered via `fs::sysfs::register_block_disk_at` — **ops-table** | **Yes** — function-pointer table in a registered struct (INV-BLKDEV-2); framework (fs/bio, ext4) calls through `disk.ops`, never names virtio directly |
| **Net** | `NetDevice { netdev_ops: &'static NetDeviceOps }` + `drivers::net::register_netdevice` (in-tree `net/space.rs`) — index assignment only | **Mostly** — ops table exists; the registration helper itself lives under drivers/ (should move to net/ with the split) |
| **Input** | `INPUT_KEYBOARD/INPUT_POINTER: Spinlock<Option<VirtioInputDevice>>` statics + event ring | **No — singletons**; consumers (console, evdev) name virtio_input directly. Needs an `input_event` framework hook (the reference's input core) |
| **GPU/fb** | `GPU_DEVICE: Spinlock<Option<VirtioGpuDevice>>` + `set_framebuffer_info` + 15 framework reads of `get_framebuffer_info` | **No — singleton + global info struct**. The `FrameBufferInfo` lookup is the console/OOM-forensics path; needs a `framebuffer` framework hook |
| **Probe/init** | `drivers::probe::init_block_devices/init_pci_block_devices/init_network_devices` called from lib.rs boot + arch boot calls (intc::init, timer::init, rtc_init, input::init, gpu probe, drm init, watchdog) | **Call-graph, not table** — the reference has the same shape early (initcall levels); a `device_initcall` table is optional polish, not a split blocker |
| **tty/serial** | console.rs uses the UART directly (no driver file — the ns16550 driver IS console.rs's bottom); fs/tty.rs is the NTTY layer with no hardware references | Already clean (framework tty; console bottom is arch) |
| **Interrupt controller** | `drivers::intc::init()` per-arch + apic/plic/gicv3 back-call into `interrupt::` (softirq raise, irqdesc) | Registration seam exists (`irq_set_chips`); arch dispatch tables remain arch-owned — correct |

**Pre-flip seam work (in dependency order):**
1. Hoist timer statics (see finding above) — zero new seams needed.
2. `framebuffer` framework hook: one `FrameBufferInfo` registry
   (`console::set_framebuffer_info`) — removes the gpu singleton reads
   from OOM/forensics/sched paths.
3. `input` framework hook: an event-injection entry
   (`input::report_event`) replacing INPUT_KEYBOARD/POINTER singleton
   reads — the evdev/console consumers become driver-agnostic.
4. Move `register_netdevice` from drivers/net/space.rs to kernel/src/net
   (it only assigns ifindex — pure framework logic in the wrong tree).

## 4. Driver → framework back-references (what the crate boundary must provide)

Total 329 `crate::{sub}` references from drivers (excluding drivers::
self-refs and config):

| Framework subsystem | Refs | Items drivers consume |
|---|---:|---|
| `fs::` | 151 | `file::File` (45), `io_completion` (27), `dev_t` (21), devfs mknod/registry, sysfs register_* |
| `interrupt::` | 83 | `softirq::raise_softirq_irqoff` (12+15), `request_irq` (6), tasklet, preempt |
| `sched::` | 37 | `schedule`, `cond_resched`, task lookup |
| `process::` | 36 | current task, pid |
| `mm::` | 23 | page alloc for DMA, page_desc |
| `net::` | 8 | stack tx/rx entry |
| `ipc::` | 3 | ashmem ioctl entry |
| (also) `arch::` | 295 | register/CSR bottoms, page-table edges — stays crate-internal to the kernel crate |
| (also) `safety::` | 104 | the primitives themselves |

**The fs:: consumption is the split's hard edge**: `io_completion`
(27) is the async block-completion callback type, and `File` (45) is
the mmap/ioctl target for gpu/input/net ioctl paths. The crate boundary
needs a `driverlib` facade exporting: `IoCompletion`, `File` handle
(opaque), `DevT`, devfs-mknod, sysfs-register, request_irq,
raise-softirq, schedule/cond_resched, current-pid, dma-alloc, and the
safety primitives. That is a bounded, enumerable list (~25 items) —
the facade is writable in one pass because the items are stable.

## 5. Safety-primitive adoption audit (pre-flip conversion work)

**MmioRegion adopted: 14 of the 21 unsafe-bearing driver files**
(virtio mod/probe/pci, queue via DmaBlock, pci, gpu ×3, intc apic/plic/
gicv3, hpet, net virtio_net, input mod/virtio_input, fb_simple).

**DmaBlock adopted: 1 file** (virtio/queue.rs — the vring windows,
7 `read/write_volatile_at` sites).

**Raw volatile MMIO remaining (the pre-flip work items): 28 sites**

| File | Sites | Class |
|---|---:|---|
| virtio/queue.rs | 12 | vring used-ring/event reads + notify doorbell store — **candidates for the DmaBlock window API extension** (the notify store is F-VIO-NOTIFY, width-is-protocol, stays raw) |
| drm/mod.rs | 5 | GEM-window content probes — **F-DIAG-DRM (gated diagnostics; stays)** |
| virtio/mod.rs + virtio_pci.rs | 4 | resp.status reads (config-space) — MmioRegion-convertible |
| intc/apic.rs | 2 | MMIO xAPIC mode reg read/write — MmioRegion-convertible (x2apic MSR path stays F-row) |
| input/virtio_input.rs | 1+ | event buffer read — DmaBlock window |
| rtc.rs | 1 | CMOS read — port-IO shape (safe-fn candidate like INV-XE-PIO-1) |
| gpu/framebuffer.rs, virtio_gpu.rs | 2 | fb write path bottom — the FrameBuffer's own window (partially MmioRegion already) |

**Pre-flip conversion count: ~12 genuinely convertible sites** (the
12 queue vring reads + 4 config-space reads + 2 xAPIC MMIO + 1 input +
1 CMOS); the remaining 16 are F-row'd protocol/diagnostic edges.

## 6. Predicted relocation cost

| Metric | Count |
|---|---:|
| Framework files flipping `drivers::` imports | 127 (→ **~60 after the timer hoist**) |
| Total import-site edits | 780 refs (→ ~340 after hoist) |
| New framework seams to write | 3 (timer hoist, framebuffer hook, input hook) + 1 move (register_netdevice) |
| driverlib facade items | ~25 enumerable items |
| Driver-internal unsafe at split time | 376 dual lines, all F-row'd or single-audited |

**Verdict: the drivers crate is SPLITTABLE after three preparatory
moves** (timer hoist = 56% of the reverse surface; framebuffer hook;
input hook) plus the driverlib facade. No framework file names a driver
through anything but the seams after those moves except: the
`drivers::virtio::pci_blk_kick` family (48 refs — these are the
completion/kick paths from fs/bio+jbd2 and belong BEHIND the GenDisk
ops table, i.e. the block completion path needs its kick routed through
`disk.ops` — one more seam item) and the drm ioctl entry (20 refs, one
dispatch function).

Recommended split order: (1) timer hoist [pure mechanical, 480 refs],
(2) block kick behind ops [semantic, 48 refs], (3) framebuffer + input
hooks [two small registries, 40 refs], (4) driverlib facade + crate
boundary, (5) `forbid(unsafe_code)` at the new crate's leaf files.
