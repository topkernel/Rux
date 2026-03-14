# ext4 64-bit Group Descriptor Fix

**Date**: 2026-03-14
**Issue**: LTP test directory `/test` appeared empty, unable to list `fork_test`, `mini-ltp`, `linux-ltp` files
**Root Cause**: ext4 driver used 32-byte group descriptor struct, but filesystem uses 64-byte group descriptors (64-bit feature)

---

## Problem Description

When executing the following commands in Rux OS, the directory appeared empty:

```bash
root# cd /test
root# ls
root#    # Nothing displayed
```

However, using `debugfs` to inspect rootfs.img confirmed that `/test` directory indeed contains files:

```
$ debugfs -R "ls -l /test" test/rootfs.img
   8194   40755 (2)      0      0    4096 14-Mar-2026 23:01 .
      2   40755 (2)      0      0    4096 14-Mar-2026 23:01 ..
     19  100755 (1)      0      0   534312 14-Mar-2026 23:01 fork_test
     20   40755 (2)      0      0    4096 14-Mar-2026 23:01 mini-ltp
     48   40755 (2)      0      0    4096 14-Mar-2026 23:01 linux-ltp
```

## Debugging Process

### Phase 1: Adding Debug Output

First, added debug output to the `ext4::list_dir` function to trace path resolution:

```rust
crate::debug_println!("[ext4] list_dir: input='{}', resolved='{}'", path, abs_path);
```

Found that path resolution was normal - both `/test` and `/test/.` correctly resolved to `/test`.

### Phase 2: Tracing Data Block Reading

Added more debug output to the `get_data_blocks` function:

```rust
crate::debug_println!("[ext4] get_data_blocks: size={}, remaining_blocks={}", self.size, remaining_blocks);
```

Discovered the key issue:

```
[ext4] get_data_blocks: size=4096, remaining_blocks=1   # Root directory - correct
[ext4] get_data_blocks: returning 1 blocks              # Root directory - returned 1 block
[ext4] get_data_blocks: size=0, remaining_blocks=0      # /test directory - ERROR!
[ext4] get_data_blocks: returning 0 blocks              # /test directory - returned 0 blocks
```

The `/test` directory's inode read returned an empty inode with size=0!

### Phase 3: Tracing Inode Reading

Added debug output to the `read_inode` function:

```rust
crate::debug_println!("[ext4] read_inode: ino={}, group={}, index={}", ino, group, index);
crate::debug_println!("[ext4] read_inode: inode_table_start={}", gd.bg_inode_table);
```

Found the problem:

```
[ext4] read_inode: ino=2, group=0, index=1
[ext4] read_inode: inode_table_start=145    # Group 0 - correct!

[ext4] read_inode: ino=8194, group=1, index=1
[ext4] read_inode: inode_table_start=0      # Group 1 - ERROR! Inode table at block 0?
[ext4] read_inode: on-disk mode=0x0, size=0 # Read garbage data
```

Group 1's `bg_inode_table` was 0, which is incorrect!

### Phase 4: Locating Group Descriptor Reading Issue

Checked the group descriptor reading code:

```rust
let gds_per_block = block_size / core::mem::size_of::<Ext4GroupDesc>() as u32;
//                                                          ^^^^^^^^^^^^^^^^
//                                                          This is 32 bytes!
```

Then checked the filesystem's actual parameters:

```
$ debugfs -R "stats" test/rootfs.img
...
Group descriptor size: 64    # Actually 64 bytes!
...
```

## Root Cause

**Core Issue**: When ext4's 64-bit feature is enabled, group descriptor size changes from 32 bytes to 64 bytes.

The original code used `core::mem::size_of::<Ext4GroupDesc>()` to calculate group descriptor offsets, but:

1. The `Ext4GroupDesc` struct was defined as 32 bytes
2. The filesystem has the 64-bit feature enabled, using 64-byte group descriptors
3. The wrong size was used for offset calculation

**Offset Calculation Error**:

| Group | Correct Offset (64 bytes) | Wrong Offset (32 bytes) | Result |
|-------|---------------------------|-------------------------|--------|
| 0     | 0                         | 0                       | Correct |
| 1     | 64                        | 32                      | Wrong |

Group 1's descriptor should be read from offset 64, but the code read from offset 32, getting the second half of group 0's descriptor (all zeros).

## Solution

Read the actual group descriptor size `s_desc_size` from the superblock and use it for offset calculation:

```rust
// Get descriptor size - read from superblock
// Default is 32 bytes, 64 bytes when 64-bit feature is enabled
let desc_size = if ext4_sb.s_desc_size < 32 { 32 } else { ext4_sb.s_desc_size as usize };

// Use actual size to calculate offset
let gds_per_block = block_size as usize / desc_size;
let gd_offset = gd_index * desc_size;  // Key fix!
```

## Verification Results

After the fix, debug output showed:

```
[ext4] read_inode: ino=8194, group=1, index=1
[ext4] read_inode: inode_table_start=657    # Now correct!
[ext4] read_inode: on-disk mode=0x41ed, size=4096
...
[ext4] list_dir: found 3 entries
fork_test  mini-ltp  linux-ltp
```

## Related Files

- `kernel/src/fs/ext4/mod.rs` - ext4 filesystem main module, fixed group descriptor reading in `init()` function
- `kernel/src/fs/ext4/superblock.rs` - Superblock and group descriptor structure definitions
- `kernel/src/fs/ext4/inode.rs` - Inode reading and data block retrieval
- `kernel/src/fs/ext4/extent.rs` - Extent tree handling

## Lessons Learned

1. **Don't assume structure sizes**: ext4 has multiple optional features (like 64-bit) that change on-disk data structure sizes
2. **Use size fields from superblock**: `s_desc_size`, `s_inode_size`, etc. record actual sizes
3. **Debugging strategy**: Start from user-visible issues and trace down layer by layer (path resolution → directory reading → inode reading → group descriptor reading)
4. **Tool verification**: Use standard tools like `debugfs` to verify filesystem contents and rule out data corruption

## Future Improvements

1. Add support for ext4 64-bit group descriptor high 32-bit fields (`bg_inode_table_hi`, etc.)
2. Check the 64-bit flag in `feature_incompat` during `init()`
3. Consider adding group descriptor checksum verification
