# Framekernel Layer Map

Status: active long-term lane (user directive 2026-10-29). This document is
the normative classification of every kernel module into framekernel layers.
The machine-readable source of truth for numbers is
`scripts/gen_tcb_ledger.py` (same rule table); regenerate the ledger with:

```bash
python3 scripts/gen_tcb_ledger.py            # text report
python3 scripts/gen_tcb_ledger.py --markdown # tables for docs
```

## Goal

Use language-level isolation to give the monolithic kernel
microkernel-adjacent safety: shrink the `unsafe` TCB to a small privileged
**Framework** layer, and make every other module Safe Rust enforced by
`#![forbid(unsafe_code)]` at the file level.

North star:

- every non-Framework file carries `#![forbid(unsafe_code)]`;
- every Framework `unsafe` block carries an auditable `// SAFETY:` invariant
  comment.

**Honesty caveat** (carried in every ledger report): Rust's unsafe
discipline covers accidental memory unsafety only. It is not a proof against
logic bugs, ABI/spec violations, or compromised build tooling. The ledger
measures the language-level TCB shape, nothing more.

## Layers

| Layer | Meaning | unsafe policy |
|---|---|---|
| `framework` | Hardware/privilege TCB: trap entry, page tables, context switch, raw MMIO/DMA, uaccess, low-level sync primitives, physical memory machinery | permitted; every block needs a `// SAFETY:` invariant comment (coverage tracked) |
| `service` | Everything userspace-facing or policy-level: fs, net, ipc, syscall wrappers, sched policy, process management, security, driver logic above the DMA core | `#![forbid(unsafe_code)]` — target state 100% |
| `diagnostics` | `dfx/` debug tooling that deliberately reads raw memory, gated by `dfx=` boot flags | permitted like framework, reported separately so tool churn does not hide inside the framework bucket |
| `test` | `kernel/src/tests` (compiled only under the `unit-test` feature) | excluded from the headline metric |
| `unbuilt` | cfg'd-off on both gate targets (currently `arch/aarch64` skeleton) | excluded — the dual-arch gate cannot verify it |

## Classification (per module)

Longest-prefix match; unmatched paths default to `service` (new files start
life in the safest bucket and must be explicitly promoted).

### Framework

| Path | Why it needs `unsafe` |
|---|---|
| `arch/` (riscv64, x86_64, interface) | trap entry/exit, page tables, context switch, pt_regs, uaccess, SBI/early boot |
| `mm/` | page-table management, buddy/slab/small_alloc physical machinery, zones, vmemmap, page structs |
| `sync/{spinlock,rwlock,seqlock,percpu,rcu,lockkey}.rs` | raw atomic/per-CPU primitive implementations |
| `interrupt/` | IRQ dispatch, softirq, tasklet machinery |
| `drivers/virtio/` | virtqueue MMIO/DMA core |
| `drivers/intc/` | apic/plic/clint raw MMIO |
| `drivers/timer/` | hpet + arch clock MMIO |
| `drivers/pci/` | PCI config space |
| root: `console.rs`, `printk.rs`, `sbi.rs`, `main.rs`, `init.rs`, `initrd.rs`, `cmdline.rs`, `list.rs`, `timer.rs` | raw console I/O, SBI ecalls, boot arg pointers, intrusive list primitive |
| `module/` | module loader relocation |
| `perf/` | PMU hardware registers |
| `trace/` | tracepoint machinery |

### Service (must become forbid)

| Path | Notes |
|---|---|
| `syscall/**` | the capsule boundary: validate/translate user args at entry, run on kernel-owned types after. Today's raw-pointer casts (esp. uaccess in io.rs) are the first shrink target |
| `fs/**`, `io_uring/` | ext4/jbd2/bio/page-cache logic |
| `net/**` | sockets, tcp/udp, netlink |
| `ipc/**` | pipes, unix sockets, SysV, binder (largest single file: 222 tokens) |
| `sched/**` | policy classes; raw current-task/percpu access in sched.rs is a shrink target |
| `process/**` | task lifecycle, exec, ptrace, coredump, namespaces |
| `security/**` | capability/LSM logic |
| `signal.rs`, `errno.rs`, `panic_policy.rs`, `print.rs` | logic files |
| `sync/{futex,condvar,semaphore}.rs`, `sync/mod.rs` | futex's uaccess + raw futex-word access splits into `arch::uaccess` framework entry; queue logic stays safe |
| `drivers/` (all except virtio/intc/timer/pci) | device logic above the DMA core: gpu/drm/input/blkdev/net drivers/loop_dev/ashmem/watchdog/rtc |

### Diagnostics

`dfx/**` — deliberate raw-memory debug tooling (scribble, wildwatch, syscost
probes...), gated off by default via `dfx=` boot flags.

### Test / unbuilt

- `tests/**` — `unit-test` feature only.
- `arch/aarch64/**` — interface skeleton, does not build; excluded until it
  builds on some gate target.

## Shrink strategy (order of operations)

1. **Forbid sweep** — every file that already compiles unsafe-free gets
   `#![forbid(unsafe_code)]` (own commit per subtree, gates per commit,
   zero runtime change). Build-time enforcement starts immediately; any
   future change that wants `unsafe` in a forbidden file fails the build and
   must either use a Framework primitive or explicitly promote the file.
2. **Unsafe-reduction refactors** — newtyped addresses (VirtAddr/PhysAddr),
   uaccess-bounded syscall entry, audited Framework primitives with
   `// SAFETY:` invariant comments. Behavior-changing refactors wait for
   lane coordination (sched/, fs/ext4, syscall/io|network, ipc/binder, mm/
   are active) and for the in-flight panic hunt on main to land.
3. **Syscall boundary = capsule boundary** — validate/translate all user
   pointers at entry; the body runs on kernel-owned types.
4. **Progressive verification** — `kernel/verify` proptest+SPIN on the
   Framework's smallest critical pieces first (page-table wrapper never maps
   user-writable over kernel text; uaccess bounds). Kani when the toolchain
   unblocks. No whole-kernel proofs.

## Metrics

Headline (must be monotonic down): **unsafe tokens outside Framework**
(service layer), from `scripts/gen_tcb_ledger.py`.

Baseline 2026-10-29 (branch point e3d8236a, comment-stripped code counts):

- total unsafe tokens: 4,603 (raw grep incl. comments: 4,625)
- service: **3,085** | framework: 1,269 | diagnostics: 175 | test: 74
- service files: 178 — forbidden 0, unsafe-free 55, unsafe-bearing 123
- SAFETY-comment coverage: 2,912 / 4,598 constructs (63%)

Top service offenders: ipc/binder.rs 222, syscall/process.rs 171,
sched/sched.rs 131, syscall/network.rs 127, syscall/misc.rs 125,
sync/futex.rs 117, syscall/file.rs 117, fs/ext4/extent.rs 71,
syscall/io.rs 69, fs/bio.rs 64.

Session 1 (2026-10-29, commits 33917c9b..e9cc518f): forbid sweep of every
unsafe-free buildable file — **113 files now carry `#![forbid(unsafe_code)]`**
(46 service, 14 framework, 7 diagnostics, 47 test [kernel unit tests];
plus 0 of the intentionally skipped ones). Token counts unchanged by
design (forbid pins files, it does not remove unsafe): headline still
3,085 service tokens; the number moves only via unsafe-reduction
refactors. Deliberately NOT forbidden, with reasons:

- 14 directory-root `mod.rs` files (arch/, arch/{riscv64,x86_64}/mm/,
  drivers/, drivers/{intc,net,timer}/, fs/jbd2/, interrupt/, ipc/, net/,
  sched/, sync/, syscall/) — an inner forbid propagates to every child
  module declared in the file, and each of these directories still has
  unsafe-bearing children. When a subtree becomes fully clean, its
  mod.rs forbid is the structural finish line.
- `config.rs`, `dfx/ksyms_generated.rs` — regenerated by build steps.
- `arch/aarch64/**` — does not build on any gate target.
