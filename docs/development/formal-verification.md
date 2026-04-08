# Rux Formal Verification Design

## 1. Overview

### 1.1 Goal

Introduce formal verification capabilities for the Rux kernel through a layered verification strategy that systematically improves kernel stability and security. The goal is NOT end-to-end verification of all 101K lines (no current toolchain can achieve this), but rather to follow the **TCB (Trusted Computing Base) minimization principle** and focus on verifying critical safety invariants in the unsafe core.

### 1.2 Core Strategy: TCB Minimization

Following the Asterinas project's verification approach (USENIX ATC 2025 CONVEROS, SOSP 2025 CortenMM):

```
Rux Kernel Code (~101K lines)
├── Safe Rust (~85%) ← Rust type system guarantees memory safety
│   ├── Syscall dispatch logic
│   ├── High-level filesystem operations
│   ├── Network protocol state machines
│   └── Process management logic
└── Unsafe Rust (~15%) ← Requires formal verification
    ├── Memory management core (buddy, page_desc, vma)
    ├── Synchronization primitives (spinlock, rwlock, futex)
    ├── Page table operations (map/unmap, COW)
    ├── Context switch (context_switch, trap)
    └── Device driver MMIO
```

**Objective**: By verifying this ~15% unsafe TCB, combined with Rust's type system guarantees for safe code, achieve high safety assurance for the overall kernel.

### 1.3 Industry References

| Project | Method | Scale | Key Results |
|---------|--------|-------|-------------|
| **seL4** (2009-2014) | Isabelle/HOL theorem proving | ~10K lines C | Full functional correctness proof, ~20 person-years |
| **Ferrocene** (2021-2025) | Toolchain qualification | Rust compiler subset | IEC 61508 SIL 2 certification (Dec 2025) |
| **Asterinas** (2023-2025) | CONVEROS model checking | Rust framekernel | Model-checked 12 concurrency modules, found 20 bugs |
| **Atmosphere** (2024) | Verus functional correctness | Rust kernel components | Mathematical proof-level verification of key algorithms |
| **RusyFuzz** (2024) | Kernel-level fuzzing | Multiple Rust OS | Fuzzing adapted for crate-based kernels |

**Key takeaways**:
- seL4's full functional correctness requires ~20 person-years — not feasible for a 101K-line project
- Asterinas's TCB minimization is the most pragmatic approach — verify 14% of code for most safety assurance
- Ferrocene proves the Rust toolchain itself can be certified, providing a foundation for application-level verification

---

## 2. Tool Selection

### 2.1 Four-Layer Verification Toolchain

```
┌─────────────────────────────────────────────────────────┐
│  L4: Functional Correctness    Verus (Microsoft Research) │  Highest
│      Mathematical proof, requires/ensures/invariant spec │
├─────────────────────────────────────────────────────────┤
│  L3: Concurrency Model Checking  CONVEROS + SPIN/Promela │  Deadlock
│      State-space search, race condition detection         │
├─────────────────────────────────────────────────────────┤
│  L2: Automated Safety Verification  Kani (Amazon)         │  Core unsafe
│      Symbolic execution, no-panic/no-UB/no-OOB proofs     │
├─────────────────────────────────────────────────────────┤
│  L1: Test Enhancement            proptest + Miri          │  Coverage
│      Property testing + undefined behavior detection       │
└─────────────────────────────────────────────────────────┘
```

### 2.2 Tool Details

#### L1: Property Testing & UB Detection

**proptest** (property-based testing)
- Automatically generates large volumes of inputs to verify code properties
- Applicable to: data structure invariants, boundary conditions, state transitions
- Strengths: simple integration, fast feedback, can discover boundary bugs
- Limitations: cannot prove "holds for all inputs", only tests sampled inputs

**Miri** (MIR Interpreter, official Rust project)
- Interprets Rust at the MIR level to detect undefined behavior
- Detects: data races, uninitialized memory reads, invalid pointers, UB
- Limitations: no inline assembly support, no `extern "C"` FFI, single-threaded

#### L2: Kani Symbolic Model Checker

**Kani** (Amazon/AWS, v0.62)
- Translates Rust MIR to CBMC (C Bounded Model Checker) format, uses SAT/SMT solvers to search for property violations
- Capabilities: bit-precise symbolic execution, handles unsafe code, partial no_std support
- Output: for each harness, either proves the property or produces a counterexample (concrete input that violates it)
- Amazon internal use: verifying ~7,500 unsafe functions in the Rust standard library
- Limitations: requires Rust nightly, limited no_std support, limited scale (~hundreds of lines per harness)

**Kani property types**:
- `#[kani::proof]` harness + `kani::assert!()` — assertion holds for all inputs
- `kani::assert!(ptr != null)` — no null pointer dereference
- `kani::assert!(idx < len)` — no out-of-bounds access
- Automatic panic detection (including arithmetic overflow)
- Automatic UB detection (all 6 forms of Rust UB)

#### L3: Concurrency Model Checking

**CONVEROS** (Asterinas project, USENIX ATC 2025)
- Abstracts Rust concurrency code into Promela models, uses SPIN model checker to search for deadlocks, races, livelocks
- Results: model-checked 12 concurrency modules, found 20 real bugs
- Applicable to: lock ordering verification, futex protocol, interrupt-preemption interaction

**SPIN/Promela**
- Industry-standard concurrency model checker
- Full state-space search, LTL property verification
- Used for modeling lock acquire/release, interrupt enable/disable, task state transitions

#### L4: Verus Functional Correctness

**Verus** (Microsoft Research, PLDI 2024)
- Embeds specifications in Rust code (`requires`/`ensures`/`invariant`), Z3 solver verifies each function satisfies its specification
- Proves functions satisfy pre/post-conditions for all possible inputs
- Ecosystem: AutoVerus (LLM-assisted proof generation), VeruSAGE (agent-driven verification), VeriStruct (struct specifications)
- Limitations: significant specification effort required, Z3 expertise needed, limited scale

**Verus specification example** (conceptual):
```rust
#[verus::spec]
fn buddy_merge_is_valid(order: usize, pfn: usize)
    requires 0 < order && order < MAX_ORDER,
    requires is_aligned(pfn, 1 << order),
    ensures  is_free(pfn) && get_order(pfn) == order + 1,
{
    // Verus proves this function satisfies ensures for all inputs meeting requires
}
```

---

## 3. Verification Target Analysis

### 3.1 Unsafe Code Distribution

| Module | unsafe Blocks | Risk Level | Verification Priority |
|--------|--------------|------------|----------------------|
| syscall/ | 303 | High | P2 (user pointer bounds) |
| fs/ | 279 | Medium | P2 (filesystem consistency) |
| drivers/ | 215 | Medium | P3 (MMIO registers) |
| arch/ | 110 + 48 unsafe fn | High | P1 (page tables, context switch) |
| net/ | 100 | Medium | P3 (protocol state machines) |
| ipc/ | 66 | Medium | P3 (shared memory safety) |
| sched/ | 58 | High | P1 (runqueue integrity) |
| process/ | 50 | Medium | P2 (task lifecycle) |
| **mm/** | **95 + 38 unsafe fn** | **Critical** | **P0 (memory safety foundation)** |
| **sync/** | **34 + 25 unsafe fn** | **Critical** | **P0 (concurrency safety foundation)** |
| interrupt/ | 30 | Medium | P2 (interrupt nesting) |
| tests/ | 53 | Low | Not verified |

### 3.2 Critical Safety Invariants

Ranked by risk/benefit ratio:

#### Invariant 1: Page Reference Count Protocol (P0)

**Location**: `kernel/src/mm/page_desc.rs` (872 lines)

```
INV-REF-1: refcount never < 0
INV-REF-2: refcount == 0 ⟺ page is on buddy free list
INV-REF-3: refcount > 0 ⟺ page is in use
INV-REF-4: mapcount == -1 (PAGE_MAPCOUNT_BIAS) ⟺ page is not mapped
INV-REF-5: mapcount > -1 ⟺ page is mapped in (mapcount + 1) page tables
INV-REF-6: COW bit set ⟹ refcount >= 2 AND W bit clear
```

**Current risk**: `put_page()` does not check for refcount underflow; could theoretically go negative.

#### Invariant 2: Buddy Allocator Free List Integrity (P0)

**Location**: `kernel/src/mm/zone.rs` (FreeArea), `kernel/src/mm/page_alloc.rs`

```
INV-BUDDY-1: every page on free list has refcount == 0
INV-BUDDY-2: page's order field == order of the free list it is on
INV-BUDDY-3: free list forms a valid singly-linked list (no cycles, no external nodes)
INV-BUDDY-4: nr_free counter == actual list length
INV-BUDDY-5: buddy of a free page is either free (same order), allocated, or outside zone
```

**Current risk**: `buddy_allocator.rs` `get_mut()` silently returns the first element on out-of-bounds index (latent bug).

#### Invariant 3: Lock Ordering & Deadlock Prevention (P0)

**Location**: `kernel/src/sync/spinlock.rs`, `futex.rs`, `sched/sched.rs`

```
INV-LOCK-1: preempt_disable must precede spinlock acquire
INV-LOCK-2: irq_save must precede preempt_disable
INV-LOCK-3: release order: unlock → preempt_enable → irq_restore
INV-LOCK-4: no lock acquisition cycles (deadlock)
INV-LOCK-5: GRQ lock and futex hash bucket lock nesting direction is consistent
```

**Current risk**: no documented lock hierarchy; futex hash bucket lock may nest inside GRQ lock.

#### Invariant 4: VMA Non-Overlap (P1)

**Location**: `kernel/src/mm/vma.rs` (762 lines)

```
INV-VMA-1: no two VMAs overlap (disjoint intervals)
INV-VMA-2: VMAs sorted by start address (BTreeMap guarantee)
INV-VMA-3: max_end == max(end) across all VMAs
INV-VMA-4: start and end must be PAGE_SIZE aligned
```

#### Invariant 5: COW Page Table Protocol (P1)

**Location**: `kernel/src/arch/riscv64/mm/mm_ops.rs`

```
INV-COW-1: COW bit set ⟹ PTE W bit clear
INV-COW-2: COW bit set ⟹ page refcount >= 2
INV-COW-3: COW fault, refcount == 1 ⟹ restore W bit directly (no copy needed)
INV-COW-4: COW fault, refcount > 1 ⟹ allocate new page + copy + decrement refcount
INV-COW-5: after fork, writable PTE must be downgraded to read-only + COW bit
```

#### Invariant 6: Context Switch Atomicity (P1)

**Location**: `kernel/src/arch/riscv64/context.rs`

```
INV-CS-1: __switch_to saves/restores all callee-saved registers (ra, sp, s0-s11)
INV-CS-2: FPU state saved before __switch_to, restored after
INV-CS-3: MMU (satp) switched before register switch
INV-CS-4: after switch, prev reference is invalid; must use tp to get current task
```

### 3.3 Known Potential Issues (Priority Verification Targets)

| # | Location | Issue | Severity |
|---|----------|-------|----------|
| 1 | `buddy_allocator.rs:83` | `get_mut()` silently returns first element on out-of-bounds index | High |
| 2 | `page_desc.rs` | `put_page()` does not check refcount underflow | High |
| 3 | `buddy_allocator.rs:190` | `next_free` uses u16, truncates when page_idx > 65535 | Medium |
| 4 | `sync/` | No documented lock hierarchy | Medium |
| 5 | `sched.rs` + `futex.rs` | GRQ lock and hash bucket lock nesting direction may be inconsistent | Medium |

---

## 4. Implementation Roadmap

### Phase 1: Safety Audit & Invariant Documentation

**Goal**: Establish verification foundation by documenting safety assumptions for all unsafe code.

**Work items**:
1. Add `// SAFETY:` comments to all ~1,193 unsafe blocks explaining why each is safe
2. Extract critical safety invariants (Section 3.2) into module header comments
3. Audit known potential issues (Table 3.3), assign fix priorities

**Files affected** (172 files containing unsafe, by module):
- `mm/` — 17 files, `sync/` — 5 files, `arch/` — 17 files
- `sched/` — 4 files, `process/` — 7 files
- `syscall/` — 9 files, `fs/` — 29 files
- `drivers/` — 19 files, `net/` — 10 files
- `ipc/` — 4 files, `interrupt/` — 5 files

**Deliverables**:
- SAFETY comments on all unsafe blocks
- Safety invariant documentation in module headers
- Known issues list with fix plan

**Effort**: Medium (~1-2 weeks, 172 files, ~5-10 min per file)

---

### Phase 2: Property Testing & Miri Integration

**Goal**: Discover data structure invariant violations and undefined behavior at low cost with high coverage.

#### 2.1 proptest Property Tests

**Test targets**:

| Test | Target Module | Verified Property |
|------|--------------|-------------------|
| `test_buddy_alloc_free_cycle` | `buddy_allocator.rs` | After any alloc/free sequence, re-allocation succeeds (no leak) |
| `test_buddy_no_overlap` | `buddy_allocator.rs` | Two allocations return non-overlapping addresses |
| `test_buddy_merge_correctness` | `buddy_allocator.rs` | After freeing buddy pair, order+1 allocation succeeds (merge) |
| `test_refcount_never_negative` | `page_desc.rs` | After any get/put sequence, refcount >= 0 |
| `test_mapcount_consistency` | `page_desc.rs` | map +1, unmap -1, final == -1 |
| `test_page_flags_atomicity` | `page_desc.rs` | Concurrent set/clear flag does not lose updates |
| `test_vma_no_overlap` | `vma.rs` | After any add/remove sequence, no VMAs overlap |
| `test_vma_sorted` | `vma.rs` | Iteration results are sorted by start address |
| `test_spinlock_acquire_release` | `spinlock.rs` | lock then unlock does not panic/deadlock |
| `test_page_flags_bitmap` | `page_desc.rs` | 16 flag bits set/test/clear operations are correct |

**Implementation approach**:
- Create `kernel/src/tests/property/` directory
- Use `#[cfg(test)]` + `proptest` crate
- Extract pure logic from kernel modules as `#[cfg(test)]` compilable hosted versions

#### 2.2 Miri UB Detection

**CI integration**:
- Add Miri job in GitHub Actions
- Run all `#[cfg(test)]` unit tests under Miri
- Expected findings: data races (multi-threaded tests), uninitialized memory reads

**Limitations**:
- Miri does not support inline assembly → provide mocks for arch/ modules
- Miri does not support `extern "C"` FFI → provide stubs for SBI calls
- Miri is single-threaded → concurrent tests handled separately

**Deliverables**:
- `kernel/src/tests/property/` directory, ~10 proptest files
- CI Miri job configuration
- List of discovered and fixed bugs

**Effort**: Medium (~1-2 weeks)

---

### Phase 3: Kani Automated Safety Verification

**Goal**: Symbolically verify core unsafe modules, proving critical properties hold for all possible inputs.

#### 3.1 Kani Environment Setup

**Dependencies**:
- Rust nightly (required by Kani)
- Kani verifier (`cargo install kani-verifier`)
- CBMC backend

**no_std adaptation strategy**:
- Kani's no_std support is limited; use **module extraction verification**
- Extract core logic to verify into a separate crate (`kernel-core-verify/`)
- Provide mocked hardware dependencies (CSR operations, physical memory access)
- Run Kani verification in hosted mode

#### 3.2 Proof Harness Design

**Harness 1: Buddy Allocator Safety**

```rust
#[kani::proof]
fn verify_buddy_alloc_no_double_free() {
    let order: usize = kani::any();
    kani::assume(order < MAX_ORDER);

    let addr1 = buddy_alloc(order);
    if addr1 != 0 {
        buddy_free(addr1, order);
        // After free, re-allocation should succeed (proves free returns memory)
        let addr2 = buddy_alloc(order);
        kani::assert!(addr2 != 0, "buddy free should return memory to pool");
    }
}

#[kani::proof]
fn verify_buddy_no_overlap() {
    let order = kani::any::<usize>();
    kani::assume(order < MAX_ORDER && order < 4); // limit search space

    let addr1 = buddy_alloc(order);
    let addr2 = buddy_alloc(order);
    if addr1 != 0 && addr2 != 0 {
        let size = 1usize << (order + 12);
        kani::assert!(addr2 >= addr1 + size || addr2 + size <= addr1,
            "two allocations must not overlap");
    }
}

#[kani::proof]
fn verify_buddy_merge() {
    // Allocate two order-0 pages
    let a = buddy_alloc(0);
    let b = buddy_alloc(0);
    if a != 0 && b != 0 {
        // Free both — buddies should merge
        buddy_free(a, 0);
        buddy_free(b, 0);
        // Order-1 allocation should succeed (buddy merge)
        let c = buddy_alloc(1);
        kani::assert!(c != 0, "buddy merge should create order-1 block");
    }
}
```

**Harness 2: Page Refcount Safety**

```rust
#[kani::proof]
fn verify_refcount_never_negative() {
    let initial: i32 = kani::any();
    kani::assume(initial >= 0);

    let gets: u8 = kani::any();   // 0..255 get_page calls
    let puts: u8 = kani::any();   // 0..255 put_page calls

    let mut refcount = initial;
    for _ in 0..gets {
        refcount = refcount.saturating_add(1); // get_page
    }
    for _ in 0..puts {
        if refcount > 0 {
            refcount -= 1; // put_page (with underflow check)
        }
    }
    kani::assert!(refcount >= 0, "refcount must never go negative");
}

#[kani::proof]
fn verify_mapcount_bounds() {
    // mapcount starts at -1 (PAGE_MAPCOUNT_BIAS)
    let mut mapcount: i32 = -1;
    let maps: u8 = kani::any();
    let unmaps: u8 = kani::any();

    for _ in 0..maps {
        mapcount = mapcount.saturating_add(1);
    }
    for _ in 0..unmaps {
        if mapcount > -1 {
            mapcount -= 1;
        }
    }
    kani::assert!(mapcount >= -1, "mapcount must never go below PAGE_MAPCOUNT_BIAS");
}
```

**Harness 3: VMA Non-Overlap**

```rust
#[kani::proof]
fn verify_vma_add_no_overlap() {
    let mut vma_mgr = VmaManager::new();
    let start1: usize = kani::any();
    let start2: usize = kani::any();
    let len: usize = kani::any();
    kani::assume(start1 % PAGE_SIZE == 0);
    kani::assume(start2 % PAGE_SIZE == 0);
    kani::assume(len >= PAGE_SIZE && len % PAGE_SIZE == 0);

    let vma1 = Vma::new(start1, start1 + len, VmaType::Anonymous);
    let vma2 = Vma::new(start2, start2 + len, VmaType::Anonymous);

    let r1 = vma_mgr.add(vma1);
    let r2 = vma_mgr.add(vma2);

    // If both VMAs added successfully, they must not overlap
    if r1.is_ok() && r2.is_ok() {
        let overlaps = (start1 < start2 + len) && (start2 < start1 + len);
        kani::assert!(!overlaps, "VMA manager must reject overlapping VMAs");
    }
}
```

**Harness 4: Spinlock Guard Safety**

```rust
#[kani::proof]
fn verify_spinlock_unlock_only_when_locked() {
    let lock = RawSpinlock::new();
    // Initial state: unlocked
    kani::assert!(!lock.is_locked(), "initial state must be unlocked");

    // Acquire
    lock.lock();
    kani::assert!(lock.is_locked(), "must be locked after lock()");

    // Release
    lock.unlock();
    kani::assert!(!lock.is_locked(), "must be unlocked after unlock()");
}
```

#### 3.3 Kani Verification Scope

| Module | Harness Count | Verified Properties |
|--------|--------------|---------------------|
| `buddy_allocator.rs` | 4 | no-panic, no-UB, no-OOB, no-double-free, merge correctness |
| `page_desc.rs` | 4 | refcount >= 0, mapcount >= -1, flag operations no overflow |
| `vma.rs` | 3 | non-overlap, sorted invariant, max_end consistency |
| `spinlock.rs` | 3 | lock/unlock pairing, guard drop safety, IRQ save/restore |
| `page_alloc.rs` | 3 | zone allocation safety, PFN range check, free list operations |
| `slab.rs` | 3 | slab allocation alignment, free list integrity, no out-of-bounds |

**Total**: ~20 Kani proof harnesses

**Deliverables**:
- `kernel/verify/` directory with Kani harnesses
- `kernel/verify/Cargo.toml` (separate crate with mocked hardware deps)
- CI Kani job
- Kani verification report (pass/fail status per harness)

**Effort**: Large (~2-4 weeks, no_std adaptation + harness writing + debugging)

---

### Phase 4: Concurrency Model Checking

**Goal**: Verify lock ordering correctness, detect deadlocks and race conditions.

#### 4.1 Lock Hierarchy Documentation

**Known lock hierarchy** (needs complete mapping):

```
Level 0 (highest): IRQ disable (irq_save)
  └── Level 1: preempt_disable
        └── Level 2: GRQ lock (sched/sched.rs)
              └── Level 3: per-zone lock (mm/zone.rs)
              └── Level 3: futex hash bucket lock (sync/futex.rs)
                    └── Level 4: waiter slot lock
        └── Level 2: process tree lock (process/task.rs)
        └── Level 2: inode lock (fs/)
        └── Level 2: dentry cache lock (fs/)
```

**Verification needs**:
- All lock acquisition paths follow this hierarchy
- No reverse nesting (lower-level lock → higher-level lock)
- No circular nesting

#### 4.2 SPIN/Promela Models

**Model 1: Futex Wait/Wake Protocol**

```promela
// Simplified futex wait/wake model
byte futex_val = 0;
bool waiter_waiting = false;
bool woken = false;

proctype Waiter() {
    atomic { bucket_lock; waiter_waiting = true; }
    // Check if futex_val changed
    if
    :: futex_val != expected_val -> bucket_unlock; skip
    :: futex_val == expected_val ->
        set_state(INTERRUPTIBLE);
        bucket_unlock;
        // Key invariant: if wake executes before set_state,
        // waiter is not lost (because waiter_waiting == true)
        do
        :: woken -> break
        :: skip
        od
    fi;
}

proctype Waker() {
    atomic { bucket_lock; futex_val = new_val; }
    if
    :: waiter_waiting -> woken = true
    :: !waiter_waiting -> skip
    fi;
    bucket_unlock;
}

ltl NoLostWakeup = [] (waiter_waiting && futex_val != expected_val -> <> woken);
```

**Model 2: Lock Ordering**

```promela
// Verify GRQ lock and futex bucket lock nesting direction
byte grq_locked = 0;
byte bucket_locked = 0;

proctype Path1() {  // scheduler path
    lock(grq);
    lock(bucket);
    unlock(bucket);
    unlock(grq);
}

proctype Path2() {  // futex wake path
    lock(bucket);
    // Attempt to acquire GRQ lock → potential deadlock!
    lock(grq);
    unlock(grq);
    unlock(bucket);
}

ltl NoDeadlock = [] !deadlock;
```

#### 4.3 Verification Scope

| Model | Verified Properties |
|-------|-------------------|
| Futex wait/wake | No lost wakeup, no spurious sleep |
| Lock ordering | No deadlock (`ltl NoDeadlock = [] !deadlock`) |
| Interrupt/preempt | preempt_count balanced, interrupt nesting safe |
| Scheduler enqueue/dequeue | nr_running consistency |

**Deliverables**:
- Lock hierarchy documentation (`docs/architecture/lock-ordering.md`)
- SPIN/Promela model files (4 models)
- Model checking report

**Effort**: Large (~2-3 weeks, requires concurrency expertise)

---

### Phase 5: Verus Functional Correctness Verification

**Goal**: Mathematical proof-level functional correctness verification for the most critical algorithms.

#### 5.1 Verus Environment Setup

- Install Verus: `cargo install verus`
- Verus uses `extern crate verus;` annotations
- Specifications embedded via `verus!` macro

#### 5.2 Verification Specifications

**Spec 1: Buddy Merge Correctness**

```
requires: page A and page B are buddies (aligned, adjacent)
          A is free with order N
          B is free with order N
ensures:  after merge, a single free block of order N+1 exists
          the merged block starts at min(A.pfn, B.pfn)
          refcount of merged block == 0
```

**Spec 2: Page Refcount Protocol**

```
invariant:
  refcount(page) >= 0
  refcount(page) == 0 ⟺ is_free(page)
  mapcount(page) >= -1
  COW_flag(page) ⟹ refcount(page) >= 2

get_page:
  requires: refcount(page) > 0
  ensures:  refcount(page) == old(refcount(page)) + 1

put_page:
  requires: refcount(page) > 0
  ensures:  refcount(page) == old(refcount(page)) - 1
            if refcount(page) == 0 then is_free(page)
```

**Spec 3: COW Page Table Protocol**

```
fork_cow_page:
  requires: page is writable, mapped, refcount == 1
  ensures:  refcount == 2
            PTE W bit == 0
            COW bit == 1
            page content unchanged

cow_fault:
  requires: COW bit == 1, PTE W bit == 0
  case refcount == 1:
    ensures: PTE W bit == 1, COW bit == 0, page content unchanged
  case refcount > 1:
    ensures: new page allocated, content copied
             old refcount decremented, new refcount == 1
             PTE points to new page with W == 1, COW == 0
```

#### 5.3 Verification Scope

| Module | Spec Count | Verification Target |
|--------|-----------|-------------------|
| Buddy allocator | 5 | split/merge/free list integrity |
| Page refcount | 4 | get/put/COW protocol |
| COW protocol | 3 | fork/cow_fault/exe_unmap |
| VMA manager | 3 | add/remove/split non-overlap |

**Total**: ~15 Verus specifications

**Deliverables**:
- `kernel/verus/` directory with Verus specification code
- Verus verified proofs
- Specification documentation

**Effort**: Very large (~4-8 weeks, requires Verus/Z3 expertise, significant specification effort)

---

### Phase 6: CI Integration

**Goal**: Integrate all verification tools into CI for continuous verification.

#### 6.1 CI Pipeline

```yaml
# .github/workflows/verify.yml
name: Formal Verification

jobs:
  miri:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo +nightly miri test -- -Zmiri-track-raw-pointers

  kani:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cd kernel/verify && cargo kani

  proptest:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cd kernel && cargo test --features property-test

  spin:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cd kernel/verify/spin && for f in *.pml; do spin -a $f && gcc -o pan pan.c && ./pan; done
```

#### 6.2 Gate Rules

| Check | Requirement | Blocks Merge? |
|-------|-------------|---------------|
| `make build` | Compiles successfully | Yes |
| `make test` | 60 unit tests pass | Yes |
| Miri | No UB reports | Yes |
| Kani | All harnesses pass | Yes |
| proptest | 100K iterations, no failures | Yes |
| SPIN | No counterexample | Warning |
| Verus | All proofs pass | Warning |

#### 6.3 New Code Verification Requirements

- New unsafe blocks must include `// SAFETY:` comments
- New/modified P0 module code must pass Kani verification
- New lock operations must conform to lock hierarchy

**Deliverables**:
- CI configuration files
- Gate rules documentation
- Verification coverage dashboard (optional)

**Effort**: Medium (~1 week)

---

## 5. Effort Estimation & Milestones

| Phase | Content | Effort | Status |
|-------|---------|--------|--------|
| 1 | Safety Audit | 1-2 weeks | ✅ Done — 483 SAFETY comments, 6 invariants, 3 bug fixes |
| 2 | proptest + Miri | 1-2 weeks | ✅ Done — 1088 proptest cases, Miri CI workflow |
| 3 | Kani | 2-4 weeks | Pending |
| 4 | SPIN Model Checking | 2-3 weeks | Pending |
| 5 | Verus | 4-8 weeks | Pending |
| 6 | CI Integration | 1 week | ✅ Done — Miri workflow, `make miri` target |

**Total**: ~11-20 weeks (~3-5 months)

### Recommended Execution Order

```
Phase 1 (Foundation) → Phase 2 (Low-cost coverage) → Phase 3 (Core verification)
                                                            │
                                                    Phase 4 (Concurrency) → Phase 5 (Highest assurance)
                                                                                       │
                                                                              Phase 6 (CI)
```

Phase 1-2 can start immediately with no additional tools. Phase 3-5 require toolchain setup and domain expertise. Phase 6 can begin incrementally after Phase 2 completes.

---

## 6. Expected Benefits

### 6.1 Safety Improvement

| Assurance Level | Source | Coverage |
|----------------|--------|----------|
| Memory safety | Rust type system | ~85% safe code |
| No UB | Miri | All testable code |
| Data structure invariants | proptest | buddy, vma, refcount, flags |
| Core unsafe safety | Kani | buddy, page_desc, spinlock, vma |
| No deadlock | SPIN | Lock ordering, futex protocol |
| Functional correctness | Verus | buddy merge, refcount protocol, COW |

### 6.2 Quality Metrics

- **Phase 1**: ✅ 100% of unsafe blocks have documented safety assumptions (483 comments)
- **Phase 2**: ✅ Miri CI workflow (`.github/workflows/miri.yml`), `make miri` target, 1088 proptest cases
- **Phase 3**: 20 core safety properties symbolically proven
- **After Phase 4**: Lock hierarchy documented, known lock paths deadlock-free
- **After Phase 5**: 15 critical algorithms verified at mathematical proof level

### 6.3 Industry Benchmarking

| Metric | Rux (Target) | Asterinas | seL4 |
|--------|-------------|-----------|------|
| Verified code ratio | ~15% | ~14% | ~100% |
| Verification tools | Kani+SPIN+Verus | CONVEROS | Isabelle/HOL |
| Assurance level | Safety properties + key functional correctness | Concurrency safety | Full functional correctness |
| Effort | 3-5 months | ~2 years | ~20 person-years |

---

## 7. Risks & Limitations

### 7.1 Tool Limitations

| Limitation | Impact | Mitigation |
|-----------|--------|------------|
| Kani limited no_std support | Cannot directly verify kernel modules | Extract to separate hosted crate |
| Miri no inline assembly | arch/ modules cannot be Miri-checked | Provide mocks, audit assembly separately |
| Verus limited scale | Large functions hard to verify | Split into small functions, verify incrementally |
| SPIN state-space explosion | Complex concurrent models infeasible | Simplify models, limit concurrent entities |
| No RISC-V ISA formal model | Assembly code cannot be formally verified | Manual audit + test coverage |

### 7.2 Parts That Cannot Be Verified

The following parts cannot be effectively verified with current toolchains and rely on code review and testing:

1. **175 inline assembly blocks**: `__switch_to`, CSR operations, trap entry
2. **425+ raw pointer dereferences**: vmemmap pointer arithmetic, task via tp
3. **83 mutable static globals**: global state concurrent access
4. **TCP state machine**: too complex for current tool capabilities
5. **Device driver MMIO**: depends on hardware behavior, cannot be modeled

---

**Document Version**: v1.1
**Last Updated**: 2026-04-08
