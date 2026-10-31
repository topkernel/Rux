# Lane: LTP r20 full scan (riscv64)
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
