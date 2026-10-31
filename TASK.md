Lane: LTP r20 full scan (riscv64)
Baseline r19 = 1,018/1,869. Work autonomously; never ask. Worktree = THIS (main@82059759; test/rootfs.img may need symlink -> /home/william/workspace/Rux/test/rootfs.img).

## Mission
1. Reconstruct deleted infra from git (main + `git branch -r | grep archived`): the LTP runner, the 1,869-case list, the per-round results layout (convention /home/william/rux-agents/ltp-r<N>/results/). test/patrol-runbook.md + test/suite.sh carry conventions. LTP source: userspace/linux-ltp/ltp-20240524.tar.xz (extract-build convention like toybox; musl SDK via `make sdk`; soft-fp overlay discipline userspace/build-softfp.sh — BUG-S005 zcb pin).
2. Build kernel (riscv64) + LTP with the pinned march.
3. Full 1,869 scan (detached/setsid; long runs survive tool timeouts). Known flake families (rerun 2x in isolation before counting new failures): accept02 getrusage04 kill02 genfmod rename/statfs.
4. Results to /home/william/rux-agents/ltp-r20/results/. Deliver r20 vs r19 delta table.
5. New real failures: triage against recent main merges (aarch64 merge b238ebd1 shared diffs: virtio device-ID table, wake floor per-arch, ELF machine checks, boot-table honesty) — compare failing syscall surface vs merge diff.
6. Small kernel fixes welcome (small commits, English, NEVER "Linux"); rerun affected family.

## Discipline
One QEMU at a time (check ps — other lanes live); -smp 2 -m 1G; kill only yours; pkill FORBIDDEN. make test stays green; x3 builds rc=0 if kernel touched. Push origin/ltp-r20.
Final message: r20 number + delta table + flake/real verdicts (2x rerun evidence) + fixes + results path.

---

# Perf review: mm

Focus: MEMORY — p3a anon fault med 0.08µs but p99 581µs (bimodal: what is the slow path paying?); P4 fork+exit 9ms / fork+exec 13ms (reference ~1ms class); TLB shootdown cost; mmap/munmap paths; copy_page_table_cow walk; vma tree operations; the allocator family (small_alloc/buddy/slab — the OH campaign named phantom preempt-count acquire sites there; the REST share, magazine discipline); demand-fill vs fault-around (aarch64 noted anon fault-around deferred; x86 batch fill exists — compare).

# Perf-review lane rules (binding)
- Mission: find where Rux is SLOW vs the reference kernel and close the gap. Target: 10x-class improvements on the lagging metrics. The user mandate: every interface metric where we lag the reference must be examined.
- GROUND TRUTH: the reference kernel bzImage is at /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage. test/pbench_boot.py --bzimage <path> boots ANY bzImage with the pbench P1-P7 battery — run the SAME battery on the reference kernel and on your rebuilt Rux kernel to get true A/B columns. This is the primary tool: measure reference, measure ours, fix, re-measure.
- Known instrumented gaps (from syscost/OH campaign, x86_64 debug): futex wake→run tick-quantized 10-108ms (reference: µs); openat avg 252µs; p1 null syscall ~4.5µs; p2 ctxsw 15.4µs (debug); p6_append ~640µs; write_fsync ~12ms; mkdrmdir ~20ms; unlink ~1ms; pipe roundtrip ~19µs; UART print 0.36-0.93ms/line; OH boot to launcher.ready 254-541s vs reference 14.4s (~26x whole-system).
- Every fix ships with NUMBERS: pbench median before/after (same boot config), or syscost per-op average before/after. No numbers = not merged.
- Correctness gate: `make test` green (riscv 1198) + x3 builds rc=0. If you touch aarch64 paths, `make test PLATFORM=aarch64` green too.
- Small commits, English messages, NEVER the word "Linux" in commits/docs — write "the reference kernel".
- Record findings/fixes/numbers in run/perf-findings.md; push origin/perf-<scope>.
- One QEMU at a time (check ps; many lanes run); kill only your own; pkill FORBIDDEN.
- Worktree: THIS one. Never write to /home/william/workspace/Rux.