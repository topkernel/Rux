# Rux OS Filesystem Stack — Refactoring Plan

## Overview

This document provides a comprehensive analysis of the Rux OS filesystem stack, compares it with the Linux kernel implementation, and outlines a phased refactoring plan to achieve feature and performance parity with Linux.

**Scope**: Syscall layer → VFS → ext4 → Buffer cache → VirtIO block driver

**Target**: Feature-complete, crash-safe, performant filesystem stack matching Linux behavior.

---

## 1. Architecture Overview

### 1.1 I/O Stack Layers

```
User space (read/write/open/stat/...)
    │
    ▼
┌─────────────────────────────────────────────────┐
│  Syscall Layer  (kernel/src/syscall/file.rs)     │
│  sys_openat, sys_read, sys_write, sys_close,     │
│  sys_fstatat, sys_getdents64, sys_mkdirat, ...  │
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│  VFS Layer  (kernel/src/fs/vfs.rs)               │
│  path_lookup, file_open, file_read, file_stat,   │
│  file_getdents64, vfs_mkdir, vfs_unlink, ...    │
│  Routes: /dev→devfs, /proc→procfs, /→ext4/rootfs│
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│  ext4  (kernel/src/fs/ext4/)                     │
│  mod.rs    — filesystem mount, lookup_path       │
│  file.rs   — ext4_file_read/write_vfs            │
│  inode.rs  — inode reading, get_data_blocks      │
│  extent.rs — extent tree traversal                │
│  namei.rs  — mkdir, create, unlink, rename, link │
│  allocator.rs — block/inode bitmap allocation     │
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│  Buffer Cache  (kernel/src/fs/bio.rs)            │
│  bread / brelse / sync_dirty_buffer              │
│  256 entries, 64 hash buckets, 4KB blocks        │
│  Single Mutex<BlockCacheInner> + LRU eviction    │
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│  Block Device  (kernel/src/fs/blkdev.rs)         │
│  GenDisk abstraction                             │
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│  VirtIO Block Driver (kernel/src/drivers/virtio/)│
│  MMIO + PCI backends                             │
│  Polling-based completion, 8-entry virtqueue     │
└─────────────────────────────────────────────────┘
```

---

## 2. Layer-by-Layer Comparison: Rux vs Linux

### 2.1 Syscall Layer

| Aspect | Rux (`syscall/file.rs`) | Linux (`fs/` + syscall tables) |
|--------|------------------------|-------------------------------|
| Path buffer | 256 bytes fixed kernel buffer | `PATH_MAX=4096`, user-space `getname()` |
| Path resolution | Each syscall duplicates CWD+path logic | Centralized `user_path_at_empty()` / `filename_lookup()` |
| dirfd support | Mostly ignored (TODO at lines 224, 376, 852, 1029, 1086) | Fully supported via `user_path_at()` |
| `openat2()` | Not implemented | `RESOLVE_*` flags, `O_LARGEFILE` handling |
| `statx()` | Not implemented | Modern stat with `STATX_*` flags |
| `preadv2/pwritev2` | Not implemented | `RWF_*` flags (NOWAIT, DSYNC, SYNC, APPEND) |
| `io_uring` | Not implemented | Async I/O submission/completion rings |
| `splice/tee/vmsplice` | Not implemented | Zero-copy pipe-to-file/file-to-pipe |
| `memfd_create` | Not implemented | Anonymous file via fd |
| `symlinkat` | Returns ENOSYS | Fully implemented |
| `fchdir` | Returns ENOSYS | Resolves fd to path via dentry |
| `futimesat` | Stub — does not update timestamps | Updates atime/mtime |
| `statfs/fstatfs` | Hardcoded values, zeros for ext4 | Real filesystem statistics |

**Key issue**: Path resolution is duplicated across ~15 syscalls. Linux centralizes this in `fs/namei.c` with `user_path_at()` and friends.

### 2.2 VFS Layer

| Aspect | Rux (`fs/vfs.rs`) | Linux (`fs/`) |
|--------|-------------------|---------------|
| **Dentry cache** | None — every path walks from root | RCU-protected hash table, negative dentries, LRU shrinker (`fs/dcache.c`) |
| **Inode cache** | `icache_lookup/add` exist (`fs/inode.rs:534-833`) but never called by `path_lookup()` | Slab-allocated inode cache, per-superblock hash (`fs/inode.c`) |
| **Mount table** | Hardcoded prefix matching (`vfs.rs:178-196`) | Mount tree with `mountpoint` dentries, bind mounts, propagation |
| **Path resolution** | String-based, no caching | `walk_component()` → `lookup_fast()` (dcache) → `lookup_slow()` (disk) |
| **File ops dispatch** | `core::ptr::eq()` on ops pointer, per-fs inline code | Unified `struct file_operations`, `vfs_read()`/`vfs_write()` |
| **Directory iteration** | Full re-list on every `getdents64` call (O(n)) | Cursor-based via `file->f_pos`, per-directory cookies |
| **Locking** | Per-file `Mutex<u64>` for position, no VFS-wide lock | `inode->i_rwsem`, `file->f_lock`, per-superblock `s_umount` rwsem |
| **`file_open()`** | ~240 lines interleaving rootfs/ext4/devfs/procfs | `do_filp_open()` → `path_openat()` → per-fs `->open()` |
| **Filesystem registration** | None | `register_filesystem()` / `kern_mount()` |
| **Permission check** | `generic_permission()` basic implementation | `inode_permission()` with ACLs, capabilities, LSM hooks |

**Critical gap**: No dentry cache means every `stat`, `open`, `access` walks the entire path from root. For ext4, this means reading inode blocks from disk for each path component.

### 2.3 Buffer/Page Cache

| Aspect | Rux (`fs/bio.rs`) | Linux (`fs/buffer.c`, `mm/page_io.c`) |
|--------|-------------------|--------------------------------------|
| **Structure** | Hash table (64 buckets) + LRU list + raw pointers | Address space → `xa_mark` page cache + buffer_head for metadata blocks |
| **Max entries** | 256 (1MB total) | Dynamic, shrunk under memory pressure |
| **Locking** | Single `Mutex<BlockCacheInner>` (line 262) | Per-bucket spinlock, RCU for lookups |
| **Read-ahead** | None — each `bread()` reads exactly 1 block | `ondemand_readahead()` with adaptive window |
| **Write-back** | Synchronous: dirty → immediate `sync_dirty_buffer()` | Dirty pages → per-BDI writeback thread (`flush-8:0`) |
| **Journaling** | Not integrated | JBD2 logs buffer heads before commit |
| **Page mapping** | None — buffers are standalone | Buffer heads backed by `struct page` |
| **Shrinker** | None | `shrink_slab()` reclaims under memory pressure |
| **I/O scheduling** | None — direct block read/write | `blk-mq` with NOOP/deadline/mq-deadline/cfq |

**Critical gap**: Single global lock serializes all cache operations. Under concurrent I/O, this is the primary bottleneck.

### 2.4 ext4 Filesystem

| Aspect | Rux (`fs/ext4/`) | Linux (`fs/ext4/`) |
|--------|------------------|-------------------|
| **Inode caching** | None — `read_inode()` from disk on every access | Inode cache (`ext4_iget()`) with `i_state` flags |
| **Extent tree** | Read-only traversal, max 4 inline extents | Full read/write, tree splitting, extent status cache |
| **Block allocator** | Linear bitmap scan across groups | `mballoc`: buddy allocator with preallocation, locality groups |
| **Inode allocator** | Linear bitmap scan | Group-based with flexible block allocation |
| **Journaling** | JBD2 module exists but ext4 bypasses it | Fully integrated: `ext4_journal_start()` wraps all metadata ops |
| **Orphan list** | Not implemented | Tracks deleted-but-open inodes for recovery |
| **Delayed allocation** | None | `ext4_da_writepages()` allocates blocks at writeback time |
| **Write pattern** | Read-modify-write inode, immediate sync | Copy-on-write pages, writeback daemon |
| **Extent status cache** | None | Per-inode cache avoiding repeated extent tree lookups |
| **fallocate()** | Not implemented | `FALLOC_FL_KEEP_SIZE`, punch hole, collapse range |
| **xattrs/ACLs** | Not implemented | Extended attributes, POSIX ACLs |
| **File encryption** | Not implemented | `fscrypt` |
| **Indirect blocks** | Free only direct blocks (TODO at `namei.rs:1107`) | Full indirect/triple-indirect block freeing |

**Critical code duplication**: `ext4/mod.rs` has `read_file()`, `create_file()`, `add_dir_entry()` etc. AND `ext4/namei.rs` has `ext4_mkdir()`, `ext4_create()`, `ext4_add_entry()` — separate implementations of similar operations.

### 2.5 VirtIO Block Driver

| Aspect | Rux (`drivers/virtio/mod.rs`) | Linux (`drivers/virtio/virtio_blk.c`) |
|--------|-------------------------------|--------------------------------------|
| **Completion model** | Busy-wait polling (`wait_for_completion()`) | Interrupt-driven with virtqueue callback |
| **Queue size** | 8 entries (fixed) | Configurable, typically 64-256 |
| **Multi-queue** | None | `blk-mq` with per-CPU hardware queues |
| **Request allocation** | Heap alloc per request (header + response) | `mempool` + per-request `struct request` |
| **Physical addressing** | `read_block()` correctly uses `virt_to_phys()` (line 389), **`write_block()` uses virtual address** (line 546) — BUG | DMA-mapped scatter-gather via `dma_map_page()` |
| **PCI lock** | Global `VIRTIO_PCI_BLK_LOCK: spin::Mutex<()>` (line 605) | Per-queue lock, no global serialization |
| **Global state** | Mutable statics (`VIRTIO_PCI_EXPECTED_USED_IDX` at line 609 — not atomic) | Per-device `struct virtio_device` with proper allocation |
| **I/O scheduler** | None | `blk-mq` with pluggable schedulers |
| **Feature negotiation** | Basic | `VIRTIO_BLK_F_RO`, `_F_BLK_SIZE`, `_F_FLUSH`, `_F_DISCARD`, `_F_WRITE_ZEROES` |

**Critical gap**: Busy-wait polling wastes CPU cycles. Linux uses interrupt-driven completion, allowing the CPU to sleep or do other work during I/O.

### 2.6 RootFS

| Aspect | Rux (`fs/rootfs.rs`) | Linux (`fs/ramfs/`, `init/do_mounts.c`) |
|--------|----------------------|-----------------------------------------|
| **Node lookup** | Linear scan of children `Vec` (O(n)) | Hash-based dentry cache |
| **Path cache** | 64-entry FNV-1a hash, no chaining, overwrite on collision (line 59-66) | Integrated dentry cache |
| **Rename** | **Data loss bug** (line 907-916): removes old, returns ENOSYS without adding new | Atomic: `lock_rename()` + dentry_move |
| **atime/mtime** | Not updated | Updated on access/modification |
| **Memory pressure** | Not handled | tmpfs can swap to disk |

---

## 3. Critical Bugs

### 3.1 RootFS rename() Data Loss

**File**: `kernel/src/fs/rootfs.rs:907-916`

```rust
// Remove from old parent directory
if !old_parent.remove_child(old_name) {
    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
}

// TODO: Implement complete rename logic
Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
```

The old entry is removed (line 907) but the function returns ENOSYS without adding it to the new parent. The entry is lost permanently.

**Fix**: Implement the full rename: recreate node with new name, add to new parent. Use `Arc::make_mut()` or replace the `unsafe` interior mutation at line 268.

### 3.2 VirtIO write_block() Virtual Address Bug

**File**: `kernel/src/drivers/virtio/mod.rs:544-549`

```rust
// Set data buffer descriptor (read-only, device reads)
queue.set_desc(
    data_desc_idx,
    buf.as_ptr() as u64,  // <-- VIRTUAL address, not physical!
    buf.len() as u32,
    ...
);
```

Compare with `read_block()` at line 392-394 which correctly converts:
```rust
let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
    crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
).0;
```

VirtIO devices use DMA with physical addresses. Using virtual addresses causes silent data corruption or I/O errors depending on the MMU mapping.

**Fix**: Add `virt_to_phys()` conversion for all three descriptors in `write_block()`, matching `read_block()`.

### 3.3 Ext4FileSystem UnsafeCell Without Synchronization

**File**: `kernel/src/fs/ext4/mod.rs:47`

```rust
group_descs: UnsafeCell<Vec<Box<Ext4GroupDesc>>>,
```

All ext4 operations that modify group descriptors (block/inode allocation) use `unsafe` raw pointer access through this `UnsafeCell` with no synchronization primitive. If two threads allocate blocks simultaneously, the free block counts in group descriptors can corrupt.

**Fix**: Add a `spin::Mutex` or `RwLock` around group descriptor access, or use a `Mutex<Vec<Box<Ext4GroupDesc>>>`.

### 3.4 VIRTIO_PCI_EXPECTED_USED_IDX Not Atomic

**File**: `kernel/src/drivers/virtio/mod.rs:609`

```rust
static mut VIRTIO_PCI_EXPECTED_USED_IDX: u16 = 0;
```

This mutable static is accessed from `get_expected_used_idx()` and `increment_expected_used_idx()` without atomic operations. If interrupt handlers and polling code run concurrently, this can race.

**Fix**: Use `AtomicU16` or protect with a spinlock.

---

## 4. Refactoring Phases

### Phase 1: Critical Bug Fixes ✅

**Priority**: P0 — correctness issues that cause data loss or corruption
**Dependencies**: None

| Task | File | Status |
|------|------|--------|
| Fix RootFS rename | `fs/rootfs.rs` | ✅ Fixed cross-directory rename ordering |
| Fix VirtIO write_block DMA | `drivers/virtio/mod.rs` | ✅ Added `virt_to_phys()` |
| Add ext4 group_descs lock | `fs/ext4/mod.rs` | ✅ Replaced `UnsafeCell` with `spin::Mutex` |
| Fix VIRTIO_PCI_EXPECTED_USED_IDX | `drivers/virtio/mod.rs` | ✅ Changed to `AtomicU16` |

### Phase 2: Dentry Cache + Inode Cache Activation ✅

**Priority**: P0 — largest single performance win
**Dependencies**: Phase 1

Dentry cache implemented in `fs/dentry.rs` with `lookup_child()`, `add_child()`, negative dentry support, and parent tracking. Inode cache implemented in `fs/inode.rs` with `icache_lookup()`, `icache_add()`, `icache_remove()` hash table. Both are fully integrated into `path_lookup()` in `vfs.rs`: dentry cache hit skips disk I/O, icache lookup/check/add on every path component, invalidation on create/unlink/rename/rmdir.

### Phase 3: Page Cache + Read-Ahead ✅

**Priority**: P1 — reduces disk I/O for sequential reads
**Dependencies**: Phase 2

Page cache implemented in `fs/page_cache.rs` with per-inode `BTreeMap<u64, CachedPage>`, LRU eviction (512 pages max). Read-ahead implemented in `fs/readahead.rs` with sequential access detection (`ReadAheadState`), activation after 2 consecutive sequential reads, prefetching up to 4 blocks ahead. Integrated into `ext4_file_read_cached()` in `ext4/file.rs` with per-fd read-ahead state. Write path invalidates page cache after writes.

### Phase 4: Mount Table ✅

**Priority**: P1 — needed for multiple filesystems, bind mounts, USB drives
**Dependencies**: Phase 2

Mount table implemented in `fs/mount.rs` with `VfsMount`, `MntFlags`, and unified `do_mount()` entry point supporting ext4, procfs, and devfs. The old hardcoded prefix matching has been replaced with a dentry tree in `vfs.rs`. `vfs_mount()` walks from VFS root to mount point, creates intermediate dentries, attaches `VfsMountInternal` with mount flags. `path_lookup()` calls `follow_mount()` to cross mount points. `vfs_umount()` removes mount point dentries from the tree.

### Phase 5: Multi-Lock Bio Cache ✅

**Priority**: P1 — removes the global I/O bottleneck
**Dependencies**: Phase 3 (page cache changes bio integration)

Replaced single `Mutex<BlockCacheInner>` with per-bucket `spin::Mutex<HashBucket>` (64 buckets). Global entry count uses `AtomicU32` for lock-free capacity checks. LRU list uses independent `spin::Mutex<LruState>`.

Key improvement: eviction (`evict_one`) releases all locks before calling `sync()`, so dirty writeback no longer blocks concurrent cache lookups. `sync_all()` collects dirty buffer pointers under bucket locks, then syncs without holding any lock. `bread_async()` now performs eviction (was missing).

### Phase 6: JBD2 Journaling Integration ✅

**Priority**: P1 — crash safety for ext4
**Dependencies**: Phase 5 (bio cache changes needed for journal I/O)

Full JBD2 module at `fs/jbd2/` (types, journal, transaction, commit, recovery, checkpoint, revoke). Bridge at `fs/ext4/journal.rs` reads journal inode, validates superblock, initializes journal, runs recovery. All ext4 namei operations conditionally use journal transactions when `fs.journal.is_some()`: mkdir (12 credits), create (8), symlink (8), link (6), unlink (8), rmdir (10), rename (16), file write (4). `write_block_from_vec()` registers dirty buffers with `jbd2_journal_dirty_metadata()` when a handle is active.

### Phase 7: Multi-Block Allocator (mballoc) ✅

**Priority**: P2 — allocation performance for large files
**Dependencies**: Phase 6 (journal credits for allocation)

Replaced linear group-0 scan with goal-group spiral search. Added `PreallocState` for per-inode block preallocation (up to 8 extra contiguous blocks). Eliminated bitmap double-read (find + mark + write in single pass). Buddy bitmap scan skips fully-occupied bytes (0xFF fast path). Deduplicated `find_free_bit` between BlockAllocator and InodeAllocator.

### Phase 8: Interrupt-Driven VirtIO I/O ✅

**Priority**: P2 — reduces CPU waste during I/O
**Dependencies**: None (can be done independently)

Both MMIO and PCI VirtIO block devices use interrupt-driven completion via `wait_for_used_interruptible()` (`drivers/virtio/queue.rs`) with `WaitQueueHead` sleep/wake. Interrupt handlers (`interrupt_handler()` for MMIO, `interrupt_handler_pci()` for PCI) call `wake_up_all()` on wait queues. IRQ enabled via PLIC (`enable_device_interrupt()`). Request pattern: submit under queue lock → drop lock → sleep until interrupt fires.

### Phase 9: Async I/O Framework ✅

**Priority**: P2 — enables non-blocking file I/O
**Dependencies**: Phase 5 (multi-lock bio), Phase 8 (interrupt-driven I/O)

Implemented `IoCompletion` primitive (AtomicBool done + AtomicI32 status + WaitQueueHead). Added `blkdev_read_async` → VirtIO `submit_read_async` (fire-and-forget, Phase 1 only) → interrupt handler completion path with `PendingIo` tracking. Added `bread_async`/`bread_wait` to bio layer. Converted read-ahead from synchronous serial (4 bread calls) to async batch submit + single wait.

### Phase 10: VFS Cleanup ✅

**Priority**: P2 — maintainability, code quality
**Dependencies**: Phase 2 (dentry cache), Phase 4 (mount table)

**Completed items**:

1. **Centralize path resolution**: Added `read_user_path()` (zero-allocation kernel stack buffer) and `read_user_str()` helpers. Upgraded `resolve_user_path()` to use `read_user_path()` internally. Refactored 14 syscalls to use these helpers, eliminating duplicated inline CWD+path logic. Fixed `sys_mkdirat` and `sys_faccessat` to respect dirfd (was ignored).

2. **Unified file ops dispatch**: Done in Phase 33.

3. **Split `file_open()`**: Done in Phase 33.

4. **Consolidate ext4 operations**: Done in Phase 33.

5. **Implement missing syscalls**:
   - `sys_fchdir`: reconstructs absolute path from dentry chain
   - `sys_symlinkat`: ext4 fast/slow symlink + VFS `vfs_symlink` + syscall
   - `sys_statx`: Linux ABI `struct Statx` (256 bytes) + mask-based field population + dispatch
   - `sys_openat2`: `struct open_how` parsing + resolve flag validation + delegate to `sys_openat`

6. **Increase path buffer**: From 256 bytes to `PATH_MAX` (4096).

7. **Bug fixes**: Fixed rootfs rename cross-directory data corruption (reorder remove before set_name). Fixed ext4 indirect block leak (recursive `free_indirect_block` for single/double/triple indirect).

**Dead code cleanup**: Removed ext4 standalone `list_dir()` and `path_lookup()` (no external callers). Removed path.rs stubs `path_lookup()`, `follow_mount()`, `follow_link()`.

**Verification**: Smoke test 15/15 passed (3 consecutive runs).

---

## 5. File Change Summary

| Phase | Status | Description |
|-------|--------|-------------|
| 1. Bug fixes | ✅ Done | rootfs rename, virtio DMA, ext4 group_descs, AtomicU16 |
| 2. Dentry+inode cache | ✅ Done | `dentry.rs` + `inode.rs` integrated into `path_lookup()` |
| 3. Page cache | ✅ Done | `page_cache.rs` (512-page LRU) + `readahead.rs` (sequential detect) |
| 4. Mount table | ✅ Done | `mount.rs` + dentry tree with `VfsMountInternal` |
| 5. Multi-lock bio | ✅ Done | Per-bucket spinlock, eviction without I/O under lock |
| 6. JBD2 integration | ✅ Done | Full `jbd2/` module, all ext4 ops wrapped in journal transactions |
| 7. mballoc | ✅ Done | Locality hint, preallocation, buddy bitmap scan, no double-read |
| 8. Interrupt VirtIO | ✅ Done | `wait_for_used_interruptible()` + `WaitQueueHead` + PLIC IRQ |
| 9. Async I/O | ✅ Done | IoCompletion, batch read-ahead, async bio submit |
| 10. VFS cleanup | ✅ Done | Path helpers, symlinkat/statx/openat2, rename fix, dead code removal |

**Completed**: 10/10 phases (all done)
**Remaining**: 0/10 phases

---

## 6. Testing Strategy

### Per-Phase Verification

| Phase | Test Method |
|-------|-------------|
| 1 | `make build && make rootfs && echo -e "\n/bin/toybox echo hello" \| timeout 10 make run` + smoke tests |
| 2 | Benchmark: `time cat /etc/mrshrc` before/after (should be faster on repeated access) |
| 3 | Benchmark: `time cat /test/large_file` (create 1MB file, measure) |
| 4 | Mount tmpfs at `/tmp`, verify `ls /tmp` shows tmpfs contents |
| 5 | Concurrent I/O test: two processes reading different files simultaneously |
| 6 | Crash test: kill QEMU mid-write, reboot, verify filesystem integrity |
| 7 | Large file creation: `dd if=/dev/zero of=/tmp/bigfile bs=1M count=100`, measure time |
| 8 | CPU usage test: during file I/O, verify CPU idle percentage is higher |
| 9 | Non-blocking I/O: `O_NONBLOCK` open returns `EAGAIN` correctly |
| 10 | Regression: all smoke tests pass, all mini-ltp tests pass |

### Regression Testing

After each phase:
1. Run smoke tests: `echo -e "\n/test/smoke_test" | timeout 30 make run`
2. Run mini-ltp tests: `echo -e "\n/test/linux-ltp/run_quick.sh" | timeout 120 make run`
3. Verify `ls /`, `ls /bin`, `cat /etc/mrshrc`, `mkdir /tmp/test_dir` still work

---

## 7. Implementation Order (Dependency Graph)

```
Phase 1: Bug Fixes ✅
    │
    ├── Phase 2: Dentry + Inode Cache ✅
    │       │
    │       ├── Phase 3: Page Cache + Read-Ahead ✅
    │       │
    │       └── Phase 4: Mount Table ✅
    │               │
    │               └── Phase 10: VFS Cleanup ✅
    │
    ├── Phase 5: Multi-Lock Bio Cache ❌
    │       │
    │       └── Phase 6: JBD2 Journaling ✅
    │               │
    │               └── Phase 7: mballoc ❌
    │
    ├── Phase 8: Interrupt-Driven VirtIO ✅
    │       │
    │       └── Phase 9: Async I/O Framework ❌
    │
    └── (Phase 10 also depends on Phase 5)
```

**Recommended order for remaining work**: Phase 5 → Phase 9 → Phase 7

---

## References

- Linux kernel source: https://elixir.bootlin.com/linux/latest/source/
  - `fs/namei.c` — path resolution
  - `fs/dcache.c` — dentry cache
  - `fs/inode.c` — inode cache
  - `fs/buffer.c` — buffer cache
  - `fs/ext4/` — ext4 filesystem
  - `fs/jbd2/` — journaling
  - `mm/readahead.c` — read-ahead
  - `mm/filemap.c` — page cache
  - `block/blk-mq.c` — block multi-queue
  - `drivers/virtio/` — VirtIO drivers
- Rux existing plan: `docs/development/ext4-write-plan.md`
- POSIX standard: https://pubs.opengroup.org/onlinepubs/9699919799/
