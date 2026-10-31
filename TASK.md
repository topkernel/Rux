# Perf gap-closer: KVM rig, biggest-gap-first (user directive 2026-10-31)
Mission: close EVERY interface gap vs the reference kernel, starting from the largest, all measured under KVM (the primary rig per AGENTS.md). This lane has NO directory scope limit — take whatever the gap table points at.

## The ranked gap table (KVM columns, medians µs, from perf-fs/syscall lanes' A/B on main@dd94e441)
1. p6_append 301x (rux 117.4 vs ref 0.390) — ROOT-CAUSED by the fs lane: per-write journal transaction commit in ext4_write_at_core (the known ~88µs route-1 class; the reference does delayed allocation + writeback). START HERE.
2. p6_openclose 100x (37.1 vs 0.371)
3. p6_read_cached 72x (38.8 vs 0.541)
4. p6_stat 64x (14.7 vs 0.230)
5. p6_unlink 38-40x (65-68 vs 1.703)
6. p6_mkdrmdir 34x (244 vs 7.224; was 2108x before the fs lane's 62x fix)
(already closed this sprint: timer 104x→2x-faster-than-ref, console→parity, syscall-entry 20% cut, futex wake quantization → x86 IRQ-exit preempt default-on)

## Rig
- Reference bzImage: /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage
- Battery: test/pbench_boot.py --bzimage <path> --accel kvm (KVM verdicts; the drivers' lane's runs are the template — see /home/william/rux-agents/perf-fs/run/pd-*.results)
- Every fix ships numbers: rux-base-KVM vs rux-fix-KVM medians. No numbers = not merged.
- Known suspects to check first: append→jbd2 commit-per-write (delayed alloc/journal batching); read_cached/stat→the syscall lane's remaining path-walk and dentry/inode-cache costs (their fixes landed — re-measure, the 72x may already be smaller on current main); openclose→path_lookup residue + fdtable.

## Discipline
x3 builds rc=0 + `make test` green before every push (riscv 1198+, suite grew to ~1204). Small commits, English, NEVER "Linux" (use "reference kernel"). Record in run/perf-findings.md. Push origin/perf-gaps. One QEMU at a time (ps first — other lanes run); kill only yours; pkill FORBIDDEN. Commit early — quota deaths expected; a patrol resumes you from TASK.md + git.
