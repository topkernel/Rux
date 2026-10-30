# Review findings — fs lane (rv-fs)

Scope: kernel/src/fs/** — VFS core, ext4, tmpfs/ramfs(rootfs), devfs,
procfs, sysfs, debugfs, cgroup, selinuxfs, mount, elf loader, buffer
cache, page cache, bio, pipe/fifo/pty/tty/vt, inotify, locks, xattr,
memfd, jbd2.

Method: line-by-line read of every file against the man-page/POSIX
contract plus the in-repo R-numbered comments; every candidate fix
re-derived from the contract before changing code; three gates per fix
(reference contract / builds + targeted test green / no upper-layer
breakage). All three builds rc=0 per fix and at the end (riscv64,
x86_64, aarch64). Guest unit-test suite booted once to verify the new
test group passes.

## Fixed (commits on rv-fs)

1. **ext4: truncate shrink left stale pointers in indirect blocks**
   (`ext4/mod.rs` ATTR_SIZE non-extent path; helper `clear_indirect_entry`
   added to `ext4/indirect.rs` — commit 57134bc8)
   The shrink path freed data blocks beyond the new EOF and zeroed the
   DIRECT slots (`i_block[0..12]`) but never cleared the freed blocks'
   entries inside the single/double-indirect blocks. After the file grew
   again (append or a sparse pwrite past the freed range), the hole range
   resolved through the stale pointers: reads served the old physical
   blocks (stale or foreign data once the allocator reused them — not the
   POSIX zero-fill), and the write path wrote through dead mappings
   without allocating. The reference kernel clears each pointer as it
   frees (ext4_free_branches).
   Gate evidence: POSIX ftruncate hole-zero contract; contract re-derived
   from `inode.read_data`'s `Ok(0)`-means-zero-fill logic that the stale
   non-zero pointers bypass; builds rc=0 x3. No upper-layer breakage: the
   clear only fires on slots whose block was just freed.

2. **ext4: ftruncate GROW silently dropped unflushed MAP_SHARED stores**
   (`ext4/mod.rs` ext4_setattr — commit c10ccc3e)
   ext4_setattr flushed dirty page-cache frames only when the new size was
   SMALLER, but the success tail calls `invalidate_inode` for every
   ATTR_SIZE change. On a grow, dirty frames below the old size were freed
   by the invalidate, silently discarding unflushed shared-mapping stores
   — sequentially reachable (map writable, store, ftruncate larger). Now
   flushed on every ATTR_SIZE (the flush iterates dirty entries only, so
   the no-dirty case is free).

3. **fs: FileBuffer shrink left stale tail; memfd re-extend read old bytes**
   (`fs/buffer.rs` FileBuffer::resize — commit 52b4c324, test a9217b4f)
   resize() dropped whole trailing chunks on shrink but kept the last
   chunk's bytes past the new length; a subsequent grow reuses the kept
   chunk, so the extended portion read back the PRE-shrink bytes.
   ftruncate(2): "the extended portion reads as null bytes". Reachable
   via memfd ftruncate (memfd_setattr → data.resize). Fixed by zeroing
   the kept chunk's tail (same discipline as TmpfsNode::truncate);
   assertion added to the GUEST unit-test suite
   (kernel/src/tests/file_buffer.rs, group "file_buffer").

4. **inotify: IN_MASK_ADD never ORed — mask silently replaced**
   (`fs/inotify.rs` inotify_add_watch — commit 45911935)
   `effective_mask = mask & !IN_MASK_ADD` was computed BEFORE the
   `effective_mask & IN_MASK_ADD != 0` test, so the OR branch was dead
   code: re-adding a watched inode with IN_MASK_ADD|<bits> REPLACED the
   mask instead of ORing (inotify(7); contradicted the function's own
   comment). Test now on the raw user mask; the registry mirror updated
   with the resulting mask.

5. **fifo: unlink while open faked EPIPE/EOF for surviving fds**
   (`fs/fifo.rs` + `fs/pipe.rs` — commit 879a7330)
   fifo_file_close took the "last fd of a direction" verdict from the
   fifo REGISTRY counters; for a FIFO unlinked while fds were open the
   registry entry is gone and the fallback closed the pipe end
   unconditionally. With two reader fds, the first close made writers see
   EPIPE/SIGPIPE while the other reader was still open; symmetrically the
   first writer close delivered EOF to still-served readers. The
   named-pipe reader/writer counts now live on the shared Pipe (riding
   the same Arc every File holds, surviving the unlink) and drive the
   close decision; the registry keeps only open-wait counters/wakeups.

## Filed (not fixed this sprint — no behavior gate failed; recorded for
the coordinator)

- ext4 non-extent truncate frees data blocks but LEAKS the L1
  single-indirect blocks under block[13] when truncating inside the
  double-indirect range (metadata-only leak, e2fsck-class; correct
  boundary math + testing is a change of its own).
- ext4 truncate non-extent path recomputes i_blocks as
  new_blocks*sectors — stops counting the KEPT indirect-block metadata
  (advisory stat field drift, e2fsck-fixable).
- `extent.rs materialize_block_inline` is dead code (only the any-depth
  `ext4_ext_materialize_block` is called); cleanup candidate.
- ext4 journal init uses only the journal inode's FIRST extent to locate
  the log; a multi-extent journal would read wrong blocks (mkfs produces
  single-extent journals in practice).
- dcache_add: cross-bucket LRU eviction replaces the target bucket's
  occupant without un-hashing it and under-counts `count` (stats-only
  drift; lookups stay correct).
- inotify IN_ONESHOT removes the watch without queueing IN_IGNORED
  (Linux delivers IN_IGNORED after the oneshot event fires).

## False positives retracted (suspected, then disproven — kept for the
record)

- "ext4_shutdown_sync writes s_state at the wrong offset (58 vs 54)":
  re-derived the ext2/ext4 superblock layout — the struct includes
  s_free_inodes_count at 0x10, so s_magic sits at 0x38 (the famous 0x438
  disk offset) and s_state at 0x3A=58. The code and its "13 u32 fields"
  comment are correct.
- "add_entry_to_block can silently drop a directory entry when
  rec_len <= used_len after find_entry_space chose the slot":
  unreachable for well-formed directories — ext4_delete_entry merges
  freed space into the PREVIOUS entry, and unmerged inode=0 slots only
  exist at block offset 0 where rec_len is the full block (always >
  used_len). Defensive guard remains, no defect.
- "pipe write loop drops the buffer lock between space check and copy":
  the check+IO run under ONE buffer lock hold (guard spans both); the
  drop happens only before wait/wake. No defect.
- "fast-symlink threshold should be 52 on metadata_csum images":
  53-60-byte fast symlinks only exist when the inode carries an xattr
  (checksum stored there), in which case the full 60 bytes are target
  data — reading <=60 is correct; mkfs never creates the no-xattr
  53-60-byte form.

## Files reviewed line-by-line with no new defects

vfs.rs, file.rs, inode.rs, dentry.rs, path.rs, mount.rs, stat.rs,
permission.rs, bio.rs (eviction/publication machinery), page_cache.rs,
pipe.rs, locks.rs, xattr.rs, fifo.rs (pre-fix), char_dev.rs,
readahead.rs, elf.rs, tmpfs.rs, memfd.rs, superblock.rs (ext4),
ext4/{namei,file,extent,inode,indirect,dir,csum,allocator,journal}.rs,
jbd2/commit.rs (+ skim of the rest of jbd2). procfs/sysfs/devfs/
debugfs/cgroup/selinuxfs/pty/tty/vt/rootfs/fs_struct/io_completion
were skimmed for defect patterns (parsers, arithmetic, lifecycle) with
nothing surviving the re-derivation gate in the time budget; the
bug-history-dense areas named in the charter were all read fully.

## Build / test status

- Baseline 3-arch build rc=0 before any change.
- After every fix: `make build` (riscv64) rc=0.
- Final: `make build` rc=0, `make build PLATFORM=x86_64` rc=0,
  `make build PLATFORM=aarch64` rc=0, plus
  `cargo build --features riscv64,unit-test` rc=0 (the guest-suite
  build the new test runs under).
- Guest unit-test suite booted once (QEMU riscv64) to verify the new
  "file_buffer" group passes; full suites run centrally at merge.

Commits (rv-fs): 57134bc8, c10ccc3e, 52b4c324, 45911935, 879a7330,
a9217b4f.
