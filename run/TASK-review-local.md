# Lane: independent line-by-line code review (user-named)
Work autonomously; never ask; commit findings. Worktree = THIS (now at c5fa4dca, lane branch feature/code-review pushed). NEVER run git writes in /home/william/workspace/Rux — this worktree only. Ledger: scripts/review_coverage.py (--check must pass after updates).

## Rules (binding)
Compare against the reference kernel's semantics per surface, but SUSPECT YOUR OWN READING FIRST: re-derive from the man-page contract + in-repo R-numbered comments; file only what survives. Three gates per finding: reference contract / in-tree tests+LTP families / no upper-layer breakage. kernel-crate #[cfg(test)] cannot run on host (std panic_impl clash) — assertions go to guest harnesses, the unit suite, or the kernel/verify host mirror (`make verify`, 1104 tests — mirrors kernel function bodies; a sibling session keeps it in sync automatically).

## Session 2026-10-31 (cr2) — DONE, pushed e395d8cb..c5fa4dca
Coordinator assignment (sprint-delta review 82059759..40e925fd) executed in full:
- ext4 metadata_csum write path: 3 divergences FIXED (a6b01fd9), empirically
  proven against e2fsck via run/cr2-csum-harness (sb csum is le32 not u16;
  dir tail is 12 bytes with markers + generation-chained seed over a
  blocksize-12 span; write_group_descriptor was reverting fresh disk-side
  GD state — now RMW). This closes the FORMULA half of oh-svc TASK.md
  known-gap 1 (a full cooked-image e2fsck after a real guest boot is the
  remaining proof).
- DRM/GEM handle lifecycle: proxy sound, 2 residual proxy limitations in
  the ledger. Swap leaf primitives: clean. SysV IPC: 12 contract points
  conform.
- Known-gap 4 done: all oh-lane hunt taps gated behind dfx=ohhunt
  (95094dd2) — perf-gate blocker cleared.
- Verification: x3 builds rc=0; make verify 1104/1104; riscv make test
  1198/1198 (run/cr2-maketest.log); coverage --check green.
- Findings ledger: run/review-findings-cr2.md.

## Handoff notes
- The pcp pageset WIP that was dirty in this worktree is preserved in
  stash@{0} (NOT popped): a sibling lane (worktree code-review-perf) is
  actively continuing that work in ITS worktree — do not duplicate it.
  Drop the stash only after that work lands on main.
- kernel/src/arch/aarch64/dtb_blob.S gets rewritten by every aarch64
  build (.incbin carries the building worktree's absolute path) — if you
  build aarch64, `git checkout --` it before committing anything.

## Priorities NEXT session
1. The aarch64 campaign's SHARED-code surface (unchanged from previous
   charter, still not covered): syscall/x86_compat.rs (x86->generic
   translation table — audit EVERY entry against both tables; whole
   families were misrouted historically), fs/elf.rs (per-arch machine
   checks + exec paths), process/task.rs (per-arch WAKE_PTR_FLOOR + wake
   paths), mm/vdso.rs, signal.rs cfg arms, process/fork.rs TLS arms,
   drivers/virtio + drivers/pci (device-ID table, BAR self-assignment,
   loud-failure probe), console.rs (aarch64 PL011 arms vs twins'
   ownership discipline).
2. kernel/src/arch/aarch64/** vs its riscv64 twin — twin-divergence
   review (undocumented drift from the riscv64 contract).
3. ext4 csum follow-up (with oh-svc lane): boot the fresh userdata
   lineage on a kernel WITH a6b01fd9, run host e2fsck -fn on the
   persisted image — the formula classes are proven; what remains
   unproven is the buffer-cache WRITE ORDERING under real guest load
   (journal interplay). Any new "checksum mismatch" class there is a
   call-site ordering bug, not a formula bug.
4. Ledger upkeep; --check green before push.

## Verification for fixes
x3 builds rc=0; riscv make test green; make verify green (host mirror);
aarch64 make test green if aarch64 touched (symlink test/rootfs.img;
suite QEMU may hang after verdict — kill only that PID).
## Discipline
One QEMU at a time; kill only yours; pkill FORBIDDEN. Small commits,
English, NEVER "Linux". Push to the lane branch.
Final message: findings (filed/fixed/false-positive-retracted with gate
evidence), coverage delta, commits.
