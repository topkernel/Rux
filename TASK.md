Lane: wedge-format — name the 870k/s format! storm (intermittent RCU wedge)
Base: main@641c8b2c. Worktree = THIS (branch wedge-format). Work
autonomously; never ask. NEVER write to /home/william/workspace/Rux.

## The bug (the last wedge face)
~50% of GNOME full-device boots wedge before the shell: an sz32 Rust
allocation storm (47M alloc/free per minute = 870k/s, 100% in the
32-byte size class) monopolizes one CPU inside a kernel-mode loop;
RCU grace periods starve one generation behind; fdreap close-stall
floods follow. Good boots show the same paths at 250k/60s (normal).

## What is already known (do not re-derive)
- The 16-frame rbp walk (szcap=1 boots, kernel/src/debug/syscost.rs
  note_sz32_stack) decodes to: format!(&dyn Display) -> String growth
  (reserve/finish_grow) -> __rust_realloc -> CombinedAllocator::alloc,
  with boot-stack frames at 11-12 (idle-loop context on the sampling
  CPU). The actual format! CALLER sits above the fmt machinery — one
  or two frames deeper than the current 16-frame walk reaches.
- It is NOT: syscalls (276/s total), page faults (census zero), the
  tick (alive), OOM forensics (capped at 4), fuxstuck/pathstash/
  hilogtap (switch-gated or cached).
- The Display impl in the chain is &dyn Display (erased dispatch).
  Only three kernel Display impls exist; std num/Arguments/String
  also possible.
- syscost=1 boots HEAL the wedge (probe timing) — use szcap=1 which
  reproduces it.

## Your mission
1. Reproduce: full-device boots (virtio-keyboard+tablet REQUIRED),
   szcap=1, loop until stalls>0. Image: work/gnome-x86.img in the
   MAIN repo (read-only, file.locking=off). Build kernel in THIS
   worktree (PLATFORM=x86_64 make build).
2. Extend note_sz32_stack to 24 frames (or start the walk from the
   kmalloc caller's rbp) — the fmt machinery is ~8 frames; the
   caller is at frames 17-20.
3. ALSO capture current task comm at sampling time — correlating
   WHO runs when the storm allocates narrows the family.
4. Name the caller, READ that code, find the loop's exit-condition
   bug. FIX it (reference semantics, English commits, never "Linux").
5. Verify: 5/5 full-device boots zero stalls + shell to timeout.
   Gates: x3 builds, riscv make test (kill orphan suite QEMU by
   fuser test/rootfs.img PID). Push origin/wedge-format.

## Discipline
One QEMU at a time; kill only yours; pkill FORBIDDEN. Commit small.
Final message: caller name + loop bug + fix + 5-boot verification.
