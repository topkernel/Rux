Host-side e2fsck oracle for the ext4 metadata_csum write path.

Usage:
  rustc -O -o harness main.rs          # after: cp ../../../kernel/src/fs/ext4/csum.rs csum.rs
  mke2fs -q -t ext4 -b 4096 -I 256 -O metadata_csum,64bit,has_journal,extent,... -d content t.img 16M
  ./harness t.img inspect|reseal|dirfix|sb|sb32|gd|inode|ibitmap|bbitmap|dirhunt
  e2fsck -fn t.img                     # the verdict

csum.rs is a COPY of the kernel's kernel/src/fs/ext4/csum.rs (refresh it
before use; the cr2 session verified commit a6b01fd9's version).
Classes: inspect dumps mkfs ground truth (sb le32 csum, 12-byte dir tail
markers, generation-chained seed); reseal proves the fixed seal is
bit-identical to mke2fs output; dirfix split+seal passes e2fsck with no
checksum complaints; sb (old u16 seal) reproduces the e2fsck superblock
refusal; sb32 proves the le32 fix. The img/binary artifacts in this
directory are untracked scratch.
