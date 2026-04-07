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

### sync/ (Synchronization)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `spinlock_test` | 4 | `sync/spinlock.rs` | try_lock/unlock, lock/unlock, unlock_unlocked, contention |

### arch/riscv64/mm/ (RISC-V MMU)

| Module | Tests | Source | Invariants |
|--------|-------|--------|------------|
| `pagetable_test` | 13 | `arch/riscv64/mm/pagetable.rs` | PTE flag bits, user/kernel/ro pages, is_leaf, ppn extraction, Satp fields |

**Total: 49 tests**

## What Gets Verified

- Pure data structure invariants (bitmap ops, refcount safety, VMA non-overlap)
- Mathematical properties (buddy allocator alignment and involution)
- Hardware format correctness (RISC-V Sv39 PTE and Satp register encoding)

## What Does NOT Get Verified Here

- Kernel-specific code (raw pointers, inline asm, GlobalAlloc impls)
- Runtime behavior (concurrency, interrupt handling)
- Full system integration (requires QEMU boot)
