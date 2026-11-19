# ext4 Write Operations Implementation Plan

## Overview

This document outlines the plan to implement write operations for the ext4 filesystem driver in Rux OS.

## Current Status

### Already Implemented (Read-only)
- Superblock reading and parsing
- Block group descriptor reading
- Inode reading and parsing
- Directory entry reading
- File content reading (via extents and indirect blocks)
- Path lookup

### Missing (Write Operations)
- `mkdir` - Create directory
- `create` - Create regular file
- `unlink` - Delete file
- `rmdir` - Remove directory
- `rename` - Rename file/directory
- `write` - Write to file
- `setattr` - Change inode attributes

## Implementation Phases

### Phase 1: Journal System (JBD2-lite) - Foundation

**Estimated Complexity: High**

The journal system is essential for ext4 data integrity. Linux uses JBD2 (Journal Block Device 2).

**Files to create:**
```
kernel/src/fs/jbd2/
├── mod.rs          - Journal core
├── journal.rs      - Journal structure
├── transaction.rs  - Transaction management
├── commit.rs       - Commit logic
└── recovery.rs     - Journal recovery (optional for now)
```

**Key structures:**
```rust
pub struct Journal {
    pub j_dev: *const GenDisk,       // Block device
    pub j_blocksize: u32,            // Block size
    pub j_maxlen: u32,               // Journal size in blocks
    pub j_head: u32,                 // Head sequence
    pub j_tail: u32,                 // Tail sequence
    pub j_transaction: Option<*mut Transaction>,
}

pub struct Transaction {
    pub t_tid: u64,                  // Transaction ID
    pub t_state: TransactionState,   // Running, Committing, etc.
    pub t_buffers: Vec<BufferHead>,  // Modified buffers
}

pub struct Handle {
    pub h_transaction: *mut Transaction,
    pub h_buffer_credits: u32,       // Buffer credits
}
```

**Key functions:**
```rust
pub fn journal_start(journal: &Journal, nblocks: u32) -> Handle;
pub fn journal_stop(handle: &Handle) -> Result<(), i32>;
pub fn journal_get_write_access(handle: &Handle, bh: &BufferHead) -> Result<(), i32>;
pub fn journal_dirty_metadata(handle: &Handle, bh: &BufferHead) -> Result<(), i32>;
```

**Linux reference:** `fs/jbd2/journal.c`, `fs/jbd2/transaction.c`

### Phase 2: Block Allocation

**Estimated Complexity: Medium-High**

Need to implement block allocation for new files and directories.

**Files to modify:**
```
kernel/src/fs/ext4/allocator.rs  - Add allocation functions
kernel/src/fs/ext4/balloc.rs     - New file for block allocation
```

**Key functions:**
```rust
// Simple block allocation (bitmap-based)
pub fn ext4_new_meta_blocks(handle: &Handle, inode: &Inode, goal: u64, count: u32) -> Result<Vec<u64>, i32>;

// Multi-block allocation (mballoc) - optional, can use simple allocation first
pub fn ext4_mb_new_blocks(handle: &Handle, ar: &Ext4AllocationRequest) -> Result<u64, i32>;
```

**Linux reference:** `fs/ext4/balloc.c`, `fs/ext4/mballoc.c`

### Phase 3: Inode Allocation

**Estimated Complexity: Medium**

Allocate new inodes for files and directories.

**Files to modify:**
```
kernel/src/fs/ext4/ialloc.rs     - New file for inode allocation
```

**Key functions:**
```rust
pub fn ext4_new_inode(handle: &Handle, dir: &Inode, mode: u32, name: &str) -> Result<Arc<Inode>, i32>;
pub fn ext4_free_inode(handle: &Handle, inode: &Inode) -> Result<(), i32>;
```

**Tasks:**
1. Find free inode in inode bitmap
2. Mark inode as used in bitmap
3. Initialize inode structure
4. Update group descriptor free inode count
5. Update superblock free inode count

**Linux reference:** `fs/ext4/ialloc.c`

### Phase 4: Directory Operations

**Estimated Complexity: Medium**

Implement directory entry manipulation.

**Files to modify:**
```
kernel/src/fs/ext4/dir.rs        - Add directory write functions
kernel/src/fs/ext4/namei.rs      - New file for name operations
```

**Key functions:**
```rust
// Add entry to directory
pub fn ext4_add_entry(handle: &Handle, dir: &Inode, name: &str, ino: u32) -> Result<(), i32>;

// Remove entry from directory
pub fn ext4_delete_entry(handle: &Handle, dir: &Inode, de: &Ext4DirEntry) -> Result<(), i32>;

// Find entry in directory
pub fn ext4_find_entry(dir: &Inode, name: &str) -> Option<Ext4DirEntry>;
```

**Linux reference:** `fs/ext4/namei.c`

### Phase 5: Implement mkdir

**Estimated Complexity: Medium**

Create directory operation.

**Implementation steps:**
1. Start journal transaction
2. Allocate new inode with S_IFDIR mode
3. Initialize directory data (add "." and ".." entries)
4. Add entry to parent directory
5. Increment parent directory link count
6. Mark inodes and blocks as dirty
7. Stop journal transaction

**Key function:**
```rust
pub unsafe fn ext4_mkdir(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, i32>;
```

**Linux reference:** `fs/ext4/namei.c:ext4_mkdir()`

### Phase 6: Implement create

**Estimated Complexity: Medium**

Create regular file operation.

**Implementation steps:**
1. Start journal transaction
2. Allocate new inode with S_IFREG mode
3. Add entry to parent directory
4. Mark inode as dirty
5. Stop journal transaction

**Key function:**
```rust
pub unsafe fn ext4_create(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, i32>;
```

**Linux reference:** `fs/ext4/namei.c:ext4_create()`

### Phase 7: Implement unlink/rmdir

**Estimated Complexity: Medium**

Delete operations.

**Implementation steps for unlink:**
1. Start journal transaction
2. Find directory entry
3. Delete entry from directory
4. Decrement inode link count
5. If link count is 0, free inode and blocks
6. Stop journal transaction

**Implementation steps for rmdir:**
1. Verify directory is empty
2. Same as unlink, but also decrement parent link count

**Linux reference:** `fs/ext4/namei.c:ext4_unlink()`, `fs/ext4/namei.c:ext4_rmdir()`

### Phase 8: Implement write

**Estimated Complexity: High**

File write operations.

**Implementation steps:**
1. Start journal transaction
2. Allocate blocks if needed
3. Write data to blocks
4. Update inode size
5. Mark blocks and inode as dirty
6. Stop journal transaction

**Key functions:**
```rust
pub fn ext4_write_begin(file: &File, pos: u64, len: u32) -> Result<Page, i32>;
pub fn ext4_write_end(file: &File, pos: u64, len: u32, copied: u32) -> Result<u32, i32>;
```

**Linux reference:** `fs/ext4/inode.c:ext4_write_begin()`, `fs/ext4/inode.c:ext4_write_end()`

## Implementation Order (Recommended)

```
Phase 1: JBD2-lite     [==========] High complexity, essential
    ↓
Phase 2: Block alloc   [========  ] Medium-high complexity
    ↓
Phase 3: Inode alloc   [=======   ] Medium complexity
    ↓
Phase 4: Dir ops       [=======   ] Medium complexity
    ↓
Phase 5: mkdir         [======    ] Uses phases 1-4
    ↓
Phase 6: create        [======    ] Uses phases 1-4
    ↓
Phase 7: unlink/rmdir  [======    ] Uses phases 1-4
    ↓
Phase 8: write         [========= ] High complexity
```

## Simplified Implementation (Without Full JBD2)

For a simpler initial implementation, we can skip the full journal system:

### Option A: No Journal (mount with noload option)
- Directly write to disk without journaling
- Risk of corruption on crash
- Simpler implementation

### Option B: Simple Write-back Cache
- Cache writes in memory
- Periodically flush to disk
- Not crash-safe but functional

## Estimated Timeline

| Phase | Complexity | Estimated Lines of Code |
|-------|------------|------------------------|
| 1. JBD2-lite | High | ~1500-2000 |
| 2. Block alloc | Medium-High | ~500-800 |
| 3. Inode alloc | Medium | ~300-500 |
| 4. Dir ops | Medium | ~400-600 |
| 5. mkdir | Medium | ~200-300 |
| 6. create | Medium | ~150-200 |
| 7. unlink/rmdir | Medium | ~200-300 |
| 8. write | High | ~800-1200 |

**Total estimated: ~4000-6000 lines of new code**

## Testing Strategy

1. **Unit tests** for each component
2. **Integration tests** with actual ext4 images
3. **Comparison tests** against Linux behavior
4. **Stress tests** for concurrent operations
5. **Crash recovery tests** (with journal)

## References

- Linux kernel source: `fs/ext4/`
- ext4 documentation: `Documentation/filesystems/ext4/`
- JBD2 documentation: `Documentation/filesystems/journalling.rst`
