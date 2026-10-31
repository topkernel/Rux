# Review findings — cr2 session (code-review lane, 2026-10-31)

Scope (coordinator assignment): the review-sprint delta merged to main
since the lane's last run (82059759..40e925fd, 118 commits) — the NEW
shared surface: ext4 metadata_csum write path, the DRM/GEM handle
lifecycle fix, the swap/migration arch leaf primitives, and the SysV
IPC contract alignment. Plus the standing known-gap-4 priority (gate
the oh-lane hunt taps).

Method: same three gates as the lane charter — reference contract
(e2fsprogs 1.47.2 + reference kernel source fetched verbatim where
memory was uncertain), in-repo/in-host verification, no upper-layer
breakage. The ext4 verdicts are EMPIRICAL: a host harness
(run/cr2-csum-harness) compiles the kernel's own csum.rs against real
mke2fs images and leaves the verdict to `e2fsck -fn`.

## Fixed

1. **ext4: superblock checksum sealed as u16 — s_checksum is le32**
   (csum.rs superblock_csum_ok/set; commit a6b01fd9)
   Reference `ext4_superblock_csum` / e2fsprogs `calc_sb_csum` store
   `cpu_to_le32(crc32c(~0, sb[0..1020]))` — the FULL 32-bit running
   state at offset 0x3FC, no masking. We wrote only the low u16 and
   left [1022..1024) holding the previous write's high half. Kernel
   self-verify passed (a u16 compare of the low half is true on both
   mkfs and self-written images — that false compatibility is how the
   bug survived), while e2fsck compares all 32 bits: "Superblock
   checksum does not match superblock" → backup sb with divergent
   counters → the known-gap-1 cascade.
   Gate evidence: harness `sb` class (old seal) reproduces e2fsck's
   refusal verbatim; `sb32` class (full le32) leaves only the expected
   free-count semantic delta. Pristine mkfs image: stored le32 ==
   full crc32c bit-for-bit.

2. **ext4: directory tails sealed with the wrong size, span and seed**
   (csum.rs dir_block_csum/set + DIR_TAIL_SIZE + dir_csum_seed;
   namei.rs dir_block_seal / find_entry_space / add_entry_to_block /
   create_initial_entry; commit a6b01fd9)
   Reference `struct ext4_dir_entry_tail` is 12 bytes — reserved_zero1
   (le32 0), rec_len (le16 = 12), reserved_name_len (le16 = 0xDE00),
   checksum (le32) — with the crc over block[0..blocksize-12] and the
   seed chained crc32c(crc32c(fs_seed, ino), i_generation)
   (e2fsprogs ext2fs_dirent_csum). We modeled an 8-byte markerless
   tail with an ino-only seed over blocksize-8. e2fsck's
   get_dirent_tail then sees "no checksum" / "fails checksum" on
   every kernel-written dir block.
   dir_block_seal now also NORMALIZES the dirent chain to end at
   blocksize-12 (the tail lives inside the last entry's rec_len
   slack; legacy 8-byte-tail blocks included) and reads the
   generation from the on-disk inode; the three entry-space sites
   reserve the tail so entry bytes cannot land inside it (the old -8
   bound let a short entry's name bytes cross into the 12-byte tail).
   Gate evidence: harness `reseal` of a pristine root block is
   BIT-IDENTICAL to mke2fs output; `dirfix` (split + reference seal)
   passes e2fsck with zero checksum complaints. Bitmap csums (lo+hi
   split, ipg/bpg byte spans), GD and inode checksum formulas were
   verified CORRECT as-is by the same harness classes.

3. **ext4: write_group_descriptor reverted fresh disk-side GD state**
   (namei.rs; commit a6b01fd9)
   It serialized the whole in-memory Ext4GroupDesc over the on-disk
   descriptor. The in-memory copy is a mount-time view:
   set_bitmap_csum_in_gd and the allocator's free-block counter
   updates write the DISK copy directly, so the full-struct overlay
   reverted a freshly written bitmap checksum (and stale free-block
   counters) moments after it landed — kernel-self-consistent (next
   mount verifies only bg_checksum) but e2fsck-dirty. Now a
   read-modify-write publishing only the free-inode count fields.

4. **dfx: oh-lane hunt taps gated behind dfx=ohhunt (known-gap 4)**
   (switches.rs + io.rs + memory.rs + drm/mod.rs + signal.rs; commit
   95094dd2)
   MAP_DUMB/failing-ioctl traces (io.rs), the drm mmap
   resolve-failed print (memory.rs — was UNBUDGETED) and
   placement-ENOMEM tap, the prime-import compose-content probe (a
   full ~3.7MB volatile walk per import on the hot path), and the
   SIGABRT first-raise dump (exec-VMA map + stack scan, fired on
   every aborting process including glibc assertions in LTP runs).
   New switch `ohhunt`; all off by default. Clears the perf-gate
   blocker for merging the drm/GEM work.

## Reviewed — no defect (delta surfaces)

- **DRM/GEM handle lifecycle (3df40258 + 0732f884)**: the
  strong_count-retention proxy for per-file handle namespaces is
  sound — I re-derived the refcount geometry through
  prime_handle_to_fd (Box<Arc> in the file's private_data) and
  prime_fd_to_handle (re-register-on-miss): the captured
  gralloc-DESTROY_DUMB-while-RS-holds-the-prime-fd case lands count>1
  exactly as the commit claims, the strong_count read fails only in
  the keep-entry direction (leak, never UAF), and the 8-byte
  GEM_CLOSE encoding reads handle@0 correctly for both layouts.
  Two residual PROXY limitations (ledgered, not fixed — per-file
  tables are the declared end-state divergence): (a) a closed handle
  remains VALID for the closing file too (reference kills the closer's
  handle); (b) if the last foreign ref drops after a retained close,
  the entry leaks until someone re-closes. Neither breaks well-behaved
  clients (gralloc/RS close on teardown).
- **Swap/migration leaf primitives (f2cc5e06 + 936e3c8d)**: walk
  shapes correct per arch (Sv39 vpn 30/21/12; x86_64 and aarch64
  4-level 39/30/21/12 with correct root-not-leaf handling);
  expect_pfn re-validation under the PTE lock present on all three;
  flag masks correct per arch (0x3FF riscv64/x86_64; !0x0000FFFF_FFFF_F000
  aarch64); swap/migration entry encoding internally consistent
  (signature bits 62/61 above the PPN field, symmetric type/offset
  codec, 2-bit type == 4 devices); rmap holds the VMA read lock
  across walk+replace (page-table lifetime, F03-07) and issues the
  cross-CPU shootdown on all three arches.
- **SysV IPC contract alignment (2b9ce2a4/1c2e8bf0/76857369/2fdd61b0/
  e8504e03)**: spot-checked twelve contract points against the man
  pages — ipcperms flag derivation (verbatim (flag>>6)|(flag>>3)|flag),
  semget EEXIST-before-nsems + nsems==0-only-on-create + ENOENT bare
  lookup, semctl EINVAL/ERANGE ordering, msgrcv selection semantics
  for all msgtyp/MSG_EXCEPT/MSG_COPY branches (MSG_EXCEPT ignored for
  msgtyp<0 conforms to the man's "used with msgtyp greater than 0"),
  E2BIG-before-removal, shmget size paths, shmat EIDRM race,
  SHM_LOCK CAP_IPC_LOCK. All conform; the ipc lane's citations hold
  up. No new findings.

## Verified clean by harness

superblock (post-fix), group descriptor, inode, inode bitmap, block
bitmap csum classes: e2fsck reports only the intentional semantic
deltas for each harness mutation class.

## Not covered this session

posix_mq attrs (2fdd61b0 was reviewed by the ipc lane's own ledger),
the remaining namei.rs paths, ext4 journal write coherence (jbd2
interaction with the fresh csum sealers — the sealers are correct
per-class, but a full cooked-image e2fsck after a REAL boot still
needs the guest-side run; the harness cannot exercise the kernel's
buffer-cache write ordering).

## Predecessor WIP preserved

The pcp pageset re-introduction (mm/pcp.rs + 5-file wiring) found
dirty in this worktree was stashed untouched (stash@{0},
"predecessor WIP: pcp pageset reintroduction (unreviewed)"). A sibling
lane (worktree /home/william/rux-agents/code-review-perf, live during
this session) is continuing exactly that work in its own worktree —
the stash is a redundant copy and was deliberately NOT popped; drop it
only after the pcp work lands on main.
