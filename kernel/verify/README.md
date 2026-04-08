# Rux Kernel Verification Test Suite

Property-based tests for kernel core data structure invariants, using [proptest](https://crates.io/crates/proptest) for randomized input generation.

## Approach

Each test file copies the relevant pure types and functions directly from `kernel/src/` and verifies their invariants. This avoids a shared-crate dependency chain and keeps the kernel source clean. When kernel types change, the copies here must be updated accordingly.

```
kernel/verify/src/       ← Self-contained test files (types copied from kernel/src/)
kernel/src/              ← Kernel binary (single source of truth)
```

## Quick Start

```bash
# Run all verification tests (default 256 proptest cases per test)
cargo test -p rux-verify --target x86_64-unknown-linux-gnu

# Run with more cases for thorough checking
PROPTEST_CASES=10000 cargo test -p rux-verify --target x86_64-unknown-linux-gnu

# Run specific test module
cargo test -p rux-verify --target x86_64-unknown-linux-gnu -- mm::buddy_test
```

## Test Modules

### mm/ (Memory Management)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `page_flags_test` | 6 | `mm/page_desc.rs` | Bitmap set/test/clear, from_raw, clear_all, test_and_set |
| `buddy_test` | 11 | `mm/buddy_allocator.rs` | Alignment, buddy involution, pair contiguity, size_to_order, get_buddy_idx |
| `vma_test` | 9 | `mm/vma.rs` | Non-overlap, adjacent VMAs, overlap rejection, find, remove, split, contains, overlaps, can_merge |
| `refcount_test` | 6 | `mm/page_desc.rs` | Never negative, get/put symmetry, underflow protection, try_get |
| `list_test` | 8 | `mm/list.rs` | Circular list integrity, add/del, FIFO/LIFO, forward/backward symmetry, for_each |
| `buddy_alloc_test` | 11 | `mm/buddy_allocator.rs` | Order calculation, buddy involution, addr roundtrip, alloc+free conservation, merging |
| `zone_test` | 13 | `mm/zone.rs` | Newton's method int_sqrt, pfn/phys roundtrip, GFP→zone mapping, watermark formula |

### sync/ (Synchronization)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `spinlock_test` | 4 | `sync/spinlock.rs` | try_lock/unlock, lock/unlock, unlock_unlocked, contention |
| `seqlock_test` | 8 | `sync/seqlock.rs` | Initial state, write mutates, locked state, try_write, sequence increments, read consistency, struct atomicity |

### net/ (Networking)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `route_test` | 9 | `net/ipv4/route.rs` | Longest-prefix match, host route, default route, masking, add/remove, interleaved ops |
| `arp_test` | 12 | `net/arp.rs` | LRU eviction, cache capacity, update/remove, packet parsing, MAC/IP extraction |
| `checksum_test` | 10 | `net/ipv4/checksum.rs` | RFC 1071 ones-complement, zero-length, complement identity, carry fold, pseudo-header |
| `tcp_test` | 16 | `net/tcp.rs` | RFC 6298 RTT estimator, RTO clamping/backoff, RFC 5681 congestion (slow start/CA/timeout), seq_before, TCP header flags |
| `ethernet_test` | 7 | `net/ethernet.rs` | MAC address classification: unicast/multicast/broadcast mutual exclusivity, addr_eq |
| `ipv4_udp_test` | 9 | `net/ipv4/mod.rs`, `net/udp.rs` | IPv4 header version/IHL, big-endian field roundtrips, UDP port/length/protocol accessors |

### fs/ (Filesystem)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `cmdline_test` | 15 | `cmdline.rs` | get_param, has_param, get_all_params, root device, init program, debug mode |
| `stat_test` | 11 | `fs/stat.rs` | File type mutual exclusivity, set/get mode roundtrip, type/mode independence |
| `path_test` | 14 | `fs/path.rs` | Path normalization, dot/dotdot handling, root escape prevention, component splitting, parent/file_name |
| `ext4/indirect_test` | 10 | `fs/ext4/indirect.rs` | Direct/indirect block mapping, block iterator count, max_file_size, indirect level monotonicity |

### security/ (Security)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `capability_test` | 18 | `security/capability.rs` | POSIX capability bitmask: set/has/clear, boolean algebra (AND/OR/XOR/complement), De Morgan, subset, lo/hi halves |

### signal/ (Signal Handling)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `signal_test` | 16 | `signal.rs` | Signal bitmap add/has/remove, first/first_unmasked, SigAction classification, signal mask ops |

### process/ (Process Management)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `pid_test` | 9 | `process/pid.rs` | PID bitmap allocator: reserved range, uniqueness, free+realloc, exhaustion, double-free safety, nr_allocated |

### sched/ (Scheduler)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `fair_test` | 18 | `sched/fair.rs` | CFS weight/wmult table monotonicity, LoadWeight, calc_delta_fair vruntime arithmetic, sched_slice proportionality, check_preempt |
| `deadline_test` | 16 | `sched/deadline.rs` | DL bandwidth clamped to 100%, consume/replenish runtime, deadline advancement, monotonicity |

### sync/ (Synchronization) — cont.

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `futex_test` | 16 | `sync/futex.rs` | FutexKey private/shared matching, futex_hash distribution, futex_to_flags, bitset intersection |

### fs/jbd2/ (JBD2 Journaling)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `types_test` | 16 | `fs/jbd2/types.rs` | Journal header magic/block_type/sequence roundtrip, tag size calculation, feature flag power-of-2, tags_per_block |

### mm/ (Memory Management) — cont.

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `vmscan_test` | 14 | `mm/vmscan.rs` | nr_to_scan priority-shift formula, ScanControl reclaim target, priority loop termination, LRU index bounds |
| `compact_test` | 16 | `mm/compact.rs` | CompactResult enum, scanner convergence, MAX_SCAN_PAGES limit, migration filter predicate (free/reserved/dirty/refcount) |
| `rmap_test` | 16 | `mm/rmap.rs` | Sv39 VPN extraction/reconstruction roundtrip, addr_to_vpn bounds, page_mapped/mapcount guards |

### fs/ext4/ (ext4 Filesystem) — cont.

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `allocator_test` | 12 | `fs/ext4/allocator.rs` | Bitmap scanner: start offset, max_bits, single free bit, all-ones/all-zeros, byte boundary |

### arch/riscv64/mm/ (RISC-V MMU)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `pagetable_test` | 13 | `arch/riscv64/mm/pagetable.rs` | PTE flag bits, user/kernel/ro pages, is_leaf, ppn extraction, Satp fields |

**Total: 374 tests across 32 modules**

## What Gets Verified

- Pure data structure invariants (bitmap ops, refcount safety, VMA non-overlap, PID allocation)
- Mathematical properties (buddy allocator alignment and involution, Newton's method int_sqrt, watermark formula)
- Hardware format correctness (RISC-V Sv39 PTE and Satp register encoding)
- Network protocol correctness (RFC 1071 checksum, RFC 6298 RTT estimator, RFC 5681 congestion control, longest-prefix match, ARP cache LRU, MAC address classification, IPv4/UDP header encoding)
- Synchronization primitives (SeqLock protocol, spinlock mutual exclusion)
- Boot parameter parsing (cmdline key=value extraction, boolean flags, defaults)
- Security primitives (POSIX capability bitmask algebra: De Morgan's law, subset, complement involution)
- Signal handling (bitmap add/remove/first, mask operations, SigAction classification)
- Filesystem structures (ext4 indirect block mapping, file type/permission modes, path normalization)
- Memory management (zone allocator arithmetic, pfn/phys roundtrip, GFP flag→zone mapping, reclaim scan control, compaction termination, reverse mapping VPN extraction)
- Scheduler theory (CFS vruntime/weight tables, deadline bandwidth clamping, consume/replenish state machine)
- Futex primitives (private/shared key matching, hash distribution, opcode-to-flags conversion, bitset intersection)
- JBD2 journaling (big-endian header parsing, tag size/feature flag arithmetic, tags_per_block)
- Bitmap allocation (free bit scanner with start offset, max_bits, 0xFF fast-path skip)

## What Does NOT Get Verified Here

- Kernel-specific code (raw pointers, inline asm, GlobalAlloc impls)
- Runtime behavior (concurrency, interrupt handling)
- Full system integration (requires QEMU boot)

## Relationship to Kernel Unit Tests

This verification suite and the kernel's own `#[test]` unit tests serve different purposes and complement each other.

| | kernel/verify (proptest) | kernel/src unit tests |
|---|---|---|
| **Environment** | std, host machine, `cargo test` | no_std, may require QEMU/real hardware |
| **Code under test** | Copied pure logic from kernel/src | Actual kernel code, no copies |
| **Input strategy** | Property-based, randomized, exhaustive | Fixed, hand-written test cases |
| **Best for** | Data structure invariants, mathematical properties, edge cases | Integration correctness, type compatibility, behavior verification |
| **Drift risk** | Copies may diverge from kernel source (sync check detects this) | None — tests the real code directly |

**In short:** verify answers "is the algorithm correct?" and unit tests answer "is the implementation correct?"

### When to Add Tests Here vs kernel/src

- **Add to kernel/verify** when you need deep invariant checking with randomized inputs: buddy allocator math, list circularity, route table longest-prefix match, checksum RFC compliance, etc.
- **Keep in kernel/src** when you need to test actual kernel types, inter-module integration, or code that depends on no_std primitives that can't run in std.

### Maintenance

Copied types must stay in sync with kernel source. Run the sync check script to detect divergences:

```bash
python3 scripts/verify_sync_check.py           # check all modules
python3 scripts/verify_sync_check.py -v        # verbose (show diff context)
python3 scripts/verify_sync_check.py mm/list   # filter to specific module
```

If a kernel unit test is fully covered by a verify test (e.g., `list.rs` basic add/del tests), the kernel unit test can be removed to avoid duplication.
