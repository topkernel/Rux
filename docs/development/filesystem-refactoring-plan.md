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

### Phase 1: Critical Bug Fixes

**Priority**: P0 — correctness issues that cause data loss or corruption
**Dependencies**: None

| Task | File | Description |
|------|------|-------------|
| Fix RootFS rename | `fs/rootfs.rs:907-916` | Complete rename: remove old → add to new parent atomically |
| Fix VirtIO write_block DMA | `drivers/virtio/mod.rs:544-549` | Add `virt_to_phys()` for all descriptors |
| Add ext4 group_descs lock | `fs/ext4/mod.rs:47` | Replace `UnsafeCell` with `Mutex<Vec<Box<Ext4GroupDesc>>>` |
| Fix VIRTIO_PCI_EXPECTED_USED_IDX | `drivers/virtio/mod.rs:609` | Change to `AtomicU16` |

**Testing**: After each fix, run `make build && make rootfs && echo -e "\n/bin/toybox echo hello" | timeout 10 make run 2>&1 | tail -20` and verify smoke tests pass.

### Phase 2: Dentry Cache + Inode Cache Activation

**Priority**: P0 — largest single performance win
**Dependencies**: Phase 1

Rux already has inode cache infrastructure (`fs/inode.rs:534-833`) but VFS never uses it. Every `path_lookup()` creates a fresh `Arc<Inode>` and every ext4 read re-reads the inode from disk.

**Implementation**:

1. **Activate inode cache in VFS**:
   - `path_lookup()` → call `icache_lookup()` before reading from disk
   - On cache miss → read from disk → call `icache_add()`
   - On cache hit → return cached inode
   - LRU eviction at 256 entries (existing)

2. **Add dentry cache**:
   - New file: `kernel/src/fs/dcache.rs`
   - Hash table mapping `(parent_ino, name) → dentry`
   - Each dentry holds: name, parent pointer, inode pointer, validity flag
   - Negative dentries: cache "file not found" results
   - `lookup_fast()`: check dcache first, skip disk I/O on hit
   - `lookup_slow()`: on miss, read from filesystem, populate dcache
   - Invalidation: on create/unlink/rename, invalidate related dentries

3. **Benefit**: A `stat("/usr/bin/ls")` call currently reads ~4 inode blocks from disk. With dcache + icache, the first call is cached and subsequent calls are O(1).

**Linux reference**: `fs/dcache.c` (`d_lookup`, `d_alloc`, `__d_lookup_rcu`), `fs/inode.c` (`iget_locked`, `find_inode_fast`)

### Phase 3: Page Cache + Read-Ahead

**Priority**: P1 — reduces disk I/O for sequential reads
**Dependencies**: Phase 2

Currently every `bread()` reads exactly one 4KB block from disk. Sequential file reads (e.g., `cat large_file`) issue one block read per 4KB, each requiring a VirtIO round-trip.

**Implementation**:

1. **Page cache**:
   - New file: `kernel/src/fs/page_cache.rs`
   - Per-inode address space backed by pages
   - `page_cache_readahead(inode, offset, size)`: read multiple blocks ahead
   - Pages marked clean/dirty, dirty pages written back periodically
   - Integration: `bread()` checks page cache first, misses go to disk

2. **Read-ahead**:
   - Track access pattern: sequential vs random
   - Sequential: read 16-32 blocks ahead asynchronously
   - Random: no read-ahead
   - Adaptive window based on hit rate

3. **Benefit**: `cat /etc/mrshrc` (one small read) won't benefit, but `cat /test/large_file` will see 10-30x fewer VirtIO round-trips.

**Linux reference**: `mm/readahead.c` (`ondemand_readahead`, `page_cache_sync_readahead`), `mm/filemap.c` (`generic_file_buffered_read`)

### Phase 4: Mount Table

**Priority**: P1 — needed for multiple filesystems, bind mounts, USB drives
**Dependencies**: Phase 2

Replace hardcoded prefix matching in `resolve_filesystem()` (`vfs.rs:178-196`) with a proper mount table.

**Implementation**:

1. **Mount table structure**:
   ```rust
   struct VfsMount {
       mountpoint: Arc<Dentry>,    // Where it's mounted
       root: Arc<Dentry>,          // Root of mounted fs
       parent: Option<Arc<VfsMount>>, // Parent mount
       superblock: Arc<SuperBlock>,
       flags: MountFlags,
   }
   ```

2. **Mount tree**: Global `Vec<Arc<VfsMount>>` with `follow_mount()` to descend into mounted-over directories.

3. **`resolve_filesystem()` → `follow_mountdown()`**: Walk mount tree to find the correct filesystem for a given path.

4. **`sys_mount()`/`sys_umount()`**: Implement mount/umount syscalls.

**Linux reference**: `fs/namespace.c` (`do_mount`, `follow_down_one`), `fs/pnode.c`

### Phase 5: Multi-Lock Bio Cache

**Priority**: P1 — removes the global I/O bottleneck
**Dependencies**: Phase 3 (page cache changes bio integration)

Currently a single `Mutex<BlockCacheInner>` serializes all cache operations. Under concurrent I/O (multiple processes reading files), this is the primary bottleneck.

**Implementation**:

1. **Per-bucket spinlock**: Replace single `Mutex<BlockCacheInner>` with per-hash-bucket `spin::Mutex<HashBucket>`.

2. **Lock ordering**: bucket lock → per-buffer state lock (existing `Mutex<BufferState>`). Never reverse.

3. **Shrinker**: Register a memory shrinker callback that evicts clean buffers when memory is low.

4. **Increase cache size**: From 256 entries (1MB) to dynamic sizing based on available memory (e.g., 10% of RAM, min 4MB).

5. **Background writeback**: Instead of `sync_dirty_buffer()` in the caller's context, mark dirty and let a writeback daemon flush periodically.

**Linux reference**: `fs/buffer.c` (`__find_get_block`, `mark_buffer_dirty`), `mm/vmscan.c` (`shrink_slab`)

### Phase 6: JBD2 Journaling Integration

**Priority**: P1 — crash safety for ext4
**Dependencies**: Phase 5 (bio cache changes needed for journal I/O)

The JBD2 module exists at `kernel/src/fs/jbd2/` but ext4 bypasses it. Writes go directly to bitmap + data blocks. A crash mid-write corrupts the filesystem.

**Implementation**:

1. **Integrate journal into ext4 operations**:
   - Every metadata-modifying operation starts a journal transaction
   - Modified blocks are logged to the journal before being written to their final location
   - On commit: write commit block, then allow metadata blocks to flush
   - On recovery: replay journal from last checkpoint

2. **Transaction wrapping**:
   ```rust
   // In ext4_mkdir, ext4_create, ext4_unlink, etc.
   let handle = jbd2::journal_start(&journal, 3)?; // 3 buffer credits
   jbd2::journal_get_write_access(&handle, &bitmap_bh)?;
   // ... modify bitmap ...
   jbd2::journal_dirty_metadata(&handle, &bitmap_bh)?;
   jbd2::journal_stop(&handle)?;
   ```

3. **Orphan list**: Track inodes that are unlinked but still open. On recovery, free these inodes.

**Linux reference**: `fs/jbd2/transaction.c`, `fs/ext4/namei.c` (every operation wrapped in `ext4_journal_start/stop`)

### Phase 7: Multi-Block Allocator (mballoc)

**Priority**: P2 — allocation performance for large files
**Dependencies**: Phase 6 (journal credits for allocation)

The current block allocator scans block groups linearly (O(n) across all groups). Linux's mballoc uses a buddy allocator with preallocation and locality hints.

**Implementation**:

1. **Per-group buddy bitmap**: Track free extents of power-of-2 sizes within each block group.

2. **Locality hint**: Allocate near the parent directory's block group.

3. **Preallocation**: When allocating for a growing file, pre-allocate extra blocks to reduce future allocation calls.

4. **Extent tree write support**: Allow creating depth > 0 extent trees when files have more than 4 non-contiguous extents.

**Linux reference**: `fs/ext4/mballoc.c` (`ext4_mb_new_blocks`), `fs/ext4/extent.c` (`ext4_ext_insert_extent`)

### Phase 8: Interrupt-Driven VirtIO I/O

**Priority**: P2 — reduces CPU waste during I/O
**Dependencies**: None (can be done independently)

Currently `wait_for_completion()` busy-waits until the device finishes. This wastes CPU cycles that could run other processes.

**Implementation**:

1. **Completion callback**: When the VirtIO device signals completion via interrupt, wake the waiting task instead of spinning.

2. **Sleep instead of spin**: In `read_block()`/`write_block()`, after submitting the request, sleep the current task (via `wait_queue` or equivalent) instead of spinning.

3. **Interrupt handler**: The existing `interrupt_handler_pci()` (line 864) already clears interrupt status. Add logic to check the used ring and wake waiters.

4. **Queue size increase**: From 8 to 64+ for better pipelining.

**Linux reference**: `drivers/virtio/virtio_ring.c` (`virtqueue_kick`, callback mechanism), `drivers/virtio/virtio_blk.c` (`virtblk_done`)

### Phase 9: Async I/O Framework

**Priority**: P2 — enables non-blocking file I/O
**Dependencies**: Phase 5 (multi-lock bio), Phase 8 (interrupt-driven I/O)

**Implementation**:

1. **Bio submission queue**: Decouple I/O submission from completion. Caller submits bio, gets a callback or future when done.

2. **I/O scheduler**: Simple deadline scheduler — prioritize reads over writes, order by deadline.

3. **Non-blocking read/write**: `O_NONBLOCK` on files returns `EAGAIN` if data not in cache, background read-ahead fills cache.

**Linux reference**: `block/blk-mq.c`, `block/deadline-iosched.c`

### Phase 10: VFS Cleanup

**Priority**: P2 — maintainability, code quality
**Dependencies**: Phase 2 (dentry cache), Phase 4 (mount table)

**Implementation**:

1. **Centralize path resolution**: Extract duplicated CWD+path logic from syscalls into a single `resolve_user_path()` function.

2. **Unified file ops dispatch**: Replace `core::ptr::eq()` checks in `file_stat()`, `file_read()` etc. with a proper `struct file_operations` vtable dispatch (the table exists in `fs/file.rs:79-90` but isn't consistently used).

3. **Split `file_open()`**: The 240-line function should be refactored into per-filesystem `->open()` callbacks.

4. **Consolidate ext4 operations**: Merge `ext4/mod.rs` and `ext4/namei.rs` duplicate implementations into a single code path.

5. **Implement missing syscalls**: `symlinkat`, `fchdir`, `statx`, `openat2`.

6. **Increase path buffer**: From 256 bytes to `PATH_MAX` (4096).

---

## 5. File Change Summary

| Phase | Files to Modify | Files to Create | Estimated LOC |
|-------|----------------|-----------------|---------------|
| 1. Bug fixes | `fs/rootfs.rs`, `drivers/virtio/mod.rs`, `fs/ext4/mod.rs` | — | ~100 |
| 2. Dentry+inode cache | `fs/vfs.rs`, `fs/inode.rs`, `fs/ext4/file.rs` | `fs/dcache.rs` | ~800 |
| 3. Page cache | `fs/bio.rs`, `fs/ext4/file.rs` | `fs/page_cache.rs` | ~600 |
| 4. Mount table | `fs/vfs.rs` | `fs/mount.rs` | ~500 |
| 5. Multi-lock bio | `fs/bio.rs` | — | ~400 |
| 6. JBD2 integration | `fs/ext4/namei.rs`, `fs/ext4/allocator.rs`, `fs/ext4/mod.rs` | — | ~300 |
| 7. mballoc | `fs/ext4/allocator.rs` | `fs/ext4/mballoc.rs` | ~800 |
| 8. Interrupt VirtIO | `drivers/virtio/mod.rs` | — | ~300 |
| 9. Async I/O | `fs/bio.rs`, `fs/blkdev.rs` | `fs/io_scheduler.rs` | ~600 |
| 10. VFS cleanup | `fs/vfs.rs`, `syscall/file.rs`, `fs/ext4/mod.rs`, `fs/ext4/namei.rs` | — | ~500 |

**Total estimated**: ~4,900 lines of new/modified code

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
Phase 1: Bug Fixes (P0)
    │
    ├── Phase 2: Dentry + Inode Cache (P0)
    │       │
    │       ├── Phase 3: Page Cache + Read-Ahead (P1)
    │       │       │
    │       │       └── Phase 5: Multi-Lock Bio Cache (P1)
    │       │               │
    │       │               └── Phase 6: JBD2 Journaling (P1)
    │       │                       │
    │       │                       └── Phase 7: mballoc (P2)
    │       │
    │       └── Phase 4: Mount Table (P1)
    │               │
    │               └── Phase 10: VFS Cleanup (P2)
    │
    ├── Phase 8: Interrupt-Driven VirtIO (P2, independent)
    │       │
    │       └── Phase 9: Async I/O Framework (P2)
    │
    └── (Phase 10 also depends on Phase 5)
```

**Recommended starting order**: Phase 1 → Phase 2 → Phase 8 → Phase 4 → Phase 5 → Phase 6 → Phase 3 → Phase 7 → Phase 9 → Phase 10

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
