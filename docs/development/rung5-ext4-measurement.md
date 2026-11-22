# Rung 5 measurement — rux-ext4 (2026-11-23, L7)

Method per crate-plan §3 step 1 (measure → decompose by class →
price), same shape as rung-3 (docs/architecture/
rung3-fs-svc-measurement.md). Pure code-reading; NO code changed.
Measured on main tip 16c20e362. ext4 = 12 files, 20,097 lines
(fs/ext4/*). jbd2 measured alongside (5 files) because it is ext4's
journal half and the pair must price together.

## (1) Framework→ext4 reverse surface: 84 refs across 25 files

The reverse surface is SMALL for a filesystem this central — the
mount-table seam plus five narrow families:

| Framework file | Refs | Items consumed | Family |
|---|---:|---|---|
| fs/vfs.rs | 16 | EXT4_INODE_OPS identity x3 (incl. unwrap_or default!), Ext4FileSystem field surgery (metadata_csum/read_only), ext4_lookup fn-ptr compare, fallocate constants+call+inode cache, sync_flushed_data/sync_inode_tables | identity-check + fallocate |
| fs/bio.rs | 12 | exthunt_* census hooks x7 shapes, namei::get_current_handle x4 (journal auto-dirty inside with_block_mut) | diagnostics + journal hook |
| fs/mount.rs | 10 | mount_ext / mount_loop_instance / create_root_inode / get_ext / is_mounted | THE ENTRY SEAM (partially table-shaped: string-compare dispatch at line 233, loop-instance special case) |
| printk.rs | 6 | get_ext/create_file (panic-path fs sync) | panic path |
| fs/page_cache.rs | 6 | file::ext, resolve_writeback_inode | writeback probe |
| arch/{x86_64,riscv64}/mm/page_fault.rs | 8 | file::is_ext4_file, ext4_fault_populate_cache | FAULT SEAM |
| fs/superblock.rs | 2 | is_mounted/unmount_ext4 | root unmount |
| fs/file.rs, fs/fsctx.rs | 4 | EXT4_FILE_OPS identity, ext4_file_write_at_vfs, mount_loop_instance | ops-table + O_DIRECT seam |
| lib.rs/init.rs | 6 | init/mount_ext4/get_ext4_fs/create_root_inode/read_file | boot mount (the R3 table's ext4 row) |
| sync/futex.rs, process/{exit,reboot}.rs, drivers/{loop_dev,virtio}.rs, dfx/taskdump.rs, procfs/{mounts,sysrq}.rs | 9 | EXT4_BIG_LOCK release/census, shutdown_sync, loop_instance_invalidate, demand_read_stats, is_mounted | lifecycle/diagnostics |
| tests/* | 5 | direct probes | exempt (test) |

**Family decomposition + predicted relocations:**

| Family | Refs | Shape | Relocations |
|---|---:|---|---|
| inode/file OPS identity (EXT4_INODE_OPS, EXT4_FILE_OPS, ext4_lookup cmp) | 6 | ptr-eq against a leaf table | 0: already the R3 ops-table shape — replace ptr-eq with a fs-id field on the table (the tmpfs/devpts alias solution) |
| fallocate (vfs) | 7 | constants + ext4_fallocate_on + inode cache deref | 1: fallocate via INodeOps slot (ops-shape already exists for other inode ops); the Ext4Inode cache deref moves behind the ops dispatch |
| Ext4FileSystem field surgery (vfs metadata_csum/read_only) | 2 | framework writes leaf-fs fields | 2: fs-feature flags DOWN into the generic superblock (fs-generic field, set at mount) |
| journal auto-dirty (bio::with_block_mut → get_current_handle) | 4 + 13 | framework block-edge calls leaf journal resolver + leaf calls jbd2 dirty | 1: the handle becomes a bio-layer "journal hook" slot (register at ext4 init) — inverts the dependency ext4↔bio forever |
| sync/flush (vfs sync_flushed_data/sync_inode_tables) | 2 | framework sync calls leaf fs syncs | 0: generic super_op (sync_fs) — the classic VFS slot |
| mount/boot (mount.rs, lib/init, fsctx, superblock unmount) | 22 | name→mount dispatch + root lifecycle | 0: the R3 registration table (ext4 row) + root-mount helper DOWN |
| FAULT SEAM (page_fault.rs x2 arches) | 8 | is_ext4_file + populate_cache | 0: FileOps.fault slot (populate is exactly a fault op); is_ext4_file via the same ops-id |
| lifecycle/diagnostics long tail (BIG_LOCK census/release, shutdown_sync, loop invalidate, demand stats, is_mounted) | 15 | scattered single calls | ~5: shutdown_sync → super_op; BIG_LOCK release_if_owner → the generic "fs big lock" protocol already half-exists (EXT4_BIG_LOCK is ext4's only because ext4 is the only journaled fs); stats/census behind dfx registry slots; loop_instance_invalidate stays loop-dev-side (it owns loop disks) |
| panic path (printk) | 6 | emergency fs sync | 0: panic-path hook slot (one registration) |

**Predicted relocation total: ~9 moves + 4 hook-table extensions
(mount table, journal-handle slot, super_op sync/fs, FileOps.fault)
+ 1 ops-id field.** Nothing un-decomposable found.

## (2) ext4→framework consumption (the legal direction)

322 qualified refs + 29 `use` lines. Dominant: fs::bio=75+
15 uses, fs::inode=35, fs::file=34, drivers::blkdev=17, safety::bytes=14,
jbd2=13(+2 uses), fs::page_cache=9, fs::inotify=12 (notify_file on
writes), mm::gup=6 (O_DIRECT pin), io_completion=4, readahead=3.

**The with_block family**: 13 ext4 call sites (allocator 8, extent 5)
consume bio::{with_block, with_block_mut, with_block_deferred_mut} —
the typed block-update edges whose INV-BIO-BLOCKUPD contract (pin,
dirty, journal, sync) is exactly the three-edge block-access family.
BUT ext4 ALSO uses the raw bio surface directly: bread/brelse/
BufferHead appear 307 times across 8 files (file.rs 102, mod 79,
inode 30, namei 23, extent 18, htree 16, journal 15, allocator 10).
The with_block conversion covered the ORDER-CRITICAL writers; the
remaining raw surface is the read-side (bread + brelse windows) and
the journal transaction bodies. For the flip, bio stays framework and
ext4 consumes its pub API — both shapes already legal; the census
work is reducing the raw bread sites onto typed edges (see §4).

**jbd2 coupling is BIDIRECTIONAL and small**: ext4→jbd2 13 refs
(dirty_metadata x6 sites, revoke, types, init), jbd2→ext4 14 refs —
12 of them are `ext4::csum::crc32c` (a pure function: the checksum
primitive lives on the wrong side) + EXT4_BIG_LOCK x2 +
init_journal x1. Relocation: crc32c moves OUT of ext4 into a shared
checksum spot (or jbd2 takes a seed fn at init); BIG_LOCK becomes the
generic journal lock owned by jbd2; ext4::journal::init_journal moves
behind the mount seam. ~3 moves price the pair; jbd2 then flips WITH
ext4 (one crate boundary between them, zero other users — bio's
get_current_handle hook is the only framework touch).

## (3) Entry seams (framework→ext4, beyond the R3 mount table)

1. MOUNT: partially table-shaped today — mount.rs:233 still
   string-compares ext2/ext3/ext4 and special-cases /dev/loop. The R3
   registration row covers the boot mount; the loop-instance arm
   needs the "multi-instance per fs" extension (mount fn receives the
   disk, returns per-instance root — the table entry becomes
   name → (mount_fn: fn(&GenDisk, flags) → Inode, INodeOps)).
2. FAULT: FileOps.fault (page_fault's populate hook) — arch
   framework reaching into ext4's demand-paging today.
3. JOURNAL HANDLE: bio's with_block_mut must call back for "is this
   block journaled" — becomes a registered journal hook (owned by
   jbd2), severing bio→ext4 AND bio→ext4::namei.
4. SUPER_OPS: sync_fs / shutdown / feature-flags / big-lock-protocol
   — the classic superblock operation set, none of which exists as a
   slot today.
5. PANIC PATH: one emergency-sync hook registration (printk must not
   name ext4).

## (4) Pre-flip unsafe census of ext4: 186 regions / 839 dual lines

Scanner: unsafe_census.py over fs/ext4 (run/ext4-census.json).
Kinds: 171 blocks, 13 unsafe-fns, 2 unsafe-impls. Categories:
ptr-ops 64, raw-construct 13, volatile 7.

Family bucketing (by the primitive each region wraps):

| Family | Regions | Conversion path |
|---|---:|---|
| F: journal-handle derefs (get_current_handle → &mut Handle) | 32 | one typed JournalHandle edge (the same hook as seam 3 above) |
| B: extent node-windows (node_extents/indices/header, read_node, gather/splice) | 21 | the four view fns are ALREADY safe-typed (&[u8]→&[Ext4Extent]); the regions wrap their CALLS — de-sign is closure hygiene, not new edges |
| C: disk-struct decode (from_bytes/read_unaligned/from_raw_parts) | 18 | safety::bytes family (landed primitive) |
| A: buffer-head windows (bread→(*bh).b_data, mark/sync) | 20 | BhRef-style typed pin edge (extend bio's with_block; the three-edge family this rung is named for) |
| E: raw cast/deref (as *const/*mut, &raw) | 29 | mostly inode-private-data downcasts → fs::unsafe_edges::privdata_ref (landed), the rest gup/O_DIRECT frame builders |
| G: residual (write_node wrappers, copy_nonoverlapping page copies, uaccess DIO) | 66 | ~20 are unsafe-fn signatures de-signing to safe bodies over the above; ~30 are copy/DIO families onto copy_kva-style edges; the rest one-offs |

**Verdict on the 415-dual figure**: the earlier 415 counted
unsafe-fn bodies that have since been split; today's honest number is
839 dual lines / 186 regions, and it decomposes 100% onto FOUR
conversion families + residuals — (1) the three-edge block-access
family with_block/A-bucket ≈ 33 sites, (2) the journal-handle edge
32, (3) bytes/decode 18, (4) typed views de-signing 21, with the E/G
tail mapping onto landed primitives (privdata_ref, copy edges). No
irreducible beyond the shared mm/asm floor ext4 does not own.

## Price summary

| Item | Count |
|---|---|
| framework→ext4 refs to sever/relocate | 84 (25 files) |
| predicted relocations (moves) | ~9 |
| new/extended seams | mount-table row + multi-instance, FileOps.fault, journal hook, super_ops set, panic hook |
| jbd2 pair-price moves | ~3 (crc32c, BIG_LOCK home, init_journal) |
| ext4 unsafe conversion families | 4 + residuals (186 regions) |
| un-decomposable | none found |

Rung 5 is PRICED. The reverse surface (84) is two-thirds the size of
rung-3's (128) despite ext4 being the deepest consumer — the R3
ops-table design already absorbed the worst of it. The dominant risk
is not the seam count but the journal: bio's with_block_mut callback
and jbd2's crc32c back-reference are the two places the dependency
inverts, and both have clean hook shapes.
