# Perf gap-closer: KVM rig, biggest-gap-first (user directive 2026-10-31)
Mission: close EVERY interface gap vs the reference kernel, starting from the largest, all measured under KVM (the primary rig per AGENTS.md). This lane has NO directory scope limit — take whatever the gap table points at.

## Session log (2026-11-01, this lane)
Landed and pushed to origin/perf-gaps (all with KVM A/B numbers in run/perf-findings.md):
- G1 ext4 buffered writes / delayed allocation: p6_append 126.6 -> 5.2 us med (24x; ref 0.390, 13x left). Includes the sync(2)/syncfs/sync_file_range/shutdown dirty-flush holes and the byte-exact dirty-tail EOF revival guard. Semantics battery 11/11 (test/delalloc_check.c).
- G2 fdtable occupancy bitmap: close(2)'s epoll purge scanned all 1024 fd slots on every last-reference close (8.9 of 10.6 us). p6_openclose 21.1 -> 8.7 us (2.4x; ref 0.371), p6_read_cached 23.5 -> 10.8 us (2.2x; ref 0.541).
- G3a jbd2 keyed dirty-buffer set: registration dedup was a linear scan under the transaction lock, O(batch^2). Dedup iters 3.18M -> 64k per 16k-op metadata workload; mkdir+rmdir med 113 -> 96 us (dedicated probe), battery rows flat-to-better.

## Ranked gap table AFTER the above (KVM medians us, rux-fix vs reference)
1. p6_mkdrmdir ~32x (205 vs 7.2) — per-block metadata machinery: each op does ~6.5 breads (cache hits), ~1.6 write_inode, 2 bitmap RMWs; rmdir phases split ~evenly 11-13 us each (find+empty / entry delete / inode rewrites / free_inode / free_blocks). NOT journal commits (those batch). Next method: bracket bio::bread/write_block_from_vec/publication-record like the ocprof pattern (see G2 in findings).
2. p6_unlink ~27x (46 vs 1.70) — same machinery, single op.
3. p6_openclose ~23x (8.5 vs 0.371) — remaining: resolve_user_path String ~1.3 us, path_lookup 2.7 us hot (per-component ~0.65-0.7 us: dentry spinlocks + per-component inode_permission + global ICACHE-lock roundtrip), close 1.7 us. Depth series in run/pg3-depth.out.
4. p6_read_cached ~19x (10.4 vs 0.541) — shares the open path.
5. p6_stat ~21x (4.8 vs 0.230) — fixed cost 2.16 us + ~0.7 us/component (v_stat_d0..d3 series, test/ocprobe.c).
6. p6_append 13x (5.0 vs 0.390) — one page-cache insert + publish per write.
7. p6_write_fsync 8.8ms vs ref ~2.5ms — writeback+commit path (fs lane domain).

## Rig
- Reference bzImage: /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage
- Battery: test/pbench_boot.py --bzimage <path> --accel kvm (KVM verdicts; template run/pd-*.results in perf-fs)
- Base kernel for A/B: run/pg1-base-bzImage (main@dd94e441 build); current fix: run/pg3-fix-bzImage
- Every fix ships numbers: rux-base-KVM vs rux-fix-KVM medians. No numbers = not merged.
- Probes this lane added (in test/probes.tar.gz): delalloc_check.c (write semantics), ocprobe.c (open/close variants + depth series), metaanat.c (metadata loops + fsw dump pairing; boot with --append-extra fsw=1).
- In-kernel stage timing recipe (the ocprof pattern, used for G2): cmdline-gated rdtsc brackets with static sum/n/max buckets and a rate-limited pr_info; see G2 section of run/perf-findings.md. Add per-bio/per-buffer brackets the same way for the mkdrmdir attack.

## Discipline
x3 builds rc=0 + `make test` green before every push (riscv 1198). Small commits, English, NEVER "Linux" (use "reference kernel"). Record in run/perf-findings.md. Push origin/perf-gaps. One QEMU at a time (ps first — other lanes run); kill only yours; pkill FORBIDDEN. Commit early — quota deaths expected; a patrol resumes you from TASK.md + git.
