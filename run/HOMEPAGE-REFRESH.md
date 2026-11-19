# HOMEPAGE-REFRESH — official numbers refresh (homepage + README)

Date: 2026-11-15. Lane: docs. No kernel code touched, no QEMUs run.

## Commits

| Side | Branch | Commit | Files |
|---|---|---|---|
| Homepage (cn+en in ONE commit) | `homepage` (worktree /home/william/rux-agents/homepage) | `167ccaa63814929978a89e9e5746e31a9fb12e1c` | 20 files, +132/−108 |
| README + structure.md | `readme-refresh` (off main @ 98702db0; worktree /home/william/rux-agents/readme-refresh) | `efe9c2416694475649a2c18b83196af06c0dbb6c` | 2 files, +35/−25 |

Not pushed (coordinator reviews). Both commit messages in English.

## Diffs summary

### Homepage — 10 page pairs, every change landed zh + en/ in the same commit

| Page (zh / en/) | Changes |
|---|---|
| index.html / en/index.html | stats bar 34万+→37.8万+ Rust lines, LTP 1084→1093; meta description 34万→37.8万; compat card LTP 1093/1869; LTP card: grew-to 1,093 + raw-1084-audit note; stability card: +boot matrix 16/16 (nums row too); perf card: GB SC 2440 / MC 7709 (best-of-runs, 113%/110% vs Linux 4-CPU anchor) + fs flush numbers + 七轮→十四轮 campaign fix; footer ~344,000→~378,000 lines |
| performance/index.html / en/ | data-as-of 2026-11-06→2026-11-15; GB verdict card + §3 totals table 7332/2274→7709/2440 with 110%/113% chips; best-of-runs convention note (user rule, recorded); per-workload split note reworded (11-04 capture keeps 7332/104% as history, adds gb-opt 7709=110%); §9 campaigns table: NEW row "Filesystem flush (fs-flush + virtio-async)" — fsync 11.55→6.18→3.04 s (~3.8× cumulative), msync 13.95→3.31 s (4.2×), sync 11.30→3.31 s, 128MB mtest06 shape, x86_64 KVM rig note; queue item best-of-runs 2297=106% → 2440=113% |
| testing/index.html / en/ | LTP card r27 raw 1084 → audited 1093 (58.5%); report table LTP row 1093 official; geekbench-report row MC 7709/7027 · SC 2440/2165 (best-of-runs) |
| testing/ltp.html / en/ | §top paragraph: raw 1,084 (58.0%) + audited official 1,093 (58.5%) with cve-2017-17052/shmat03 order-dependent-artifact note; distribution table labeled raw r27, PASS official 1093 stated; trend last bar 1084→1093 (bar heights rescaled to the 1093 max); trend caption + rigs table updated |
| testing/stability.html / en/ | §2 NEW bullet: riscv64 boot-parameter matrix 16/16 — smp{2,4,6,8} × mem{1,2,4,8}G, all cells clean (previously several panic/hang cells) |
| progress/roadmap.html / en/ | current-goal LTP 1093 official (raw 1084); 2026-11-05/06 milestone heading keeps raw 1084 + adds audited 1093 |
| progress/changelog.html / en/ | NEW 2026-11-15 entry: gb-opt GB bests, fs-flush+virtio-async numbers, LTP audit 1093, boot matrix 16/16, code recount (kernel/ 623 files / 377,650 Rust / 384,307; repo-wide 1,094 / 915,347) |
| architecture/structure.html / en/ | sub: NEW convention line (main 98702db0, 2026-11-15, git ls-files + wc -l): kernel/ 623 files · 384,307 (Rust 377,650); kernel/src 453 · 354,945; module table rebuilt from recount (fs 67,491 / arch 48,089 / syscall 39,442 / ipc 21,409 incl. binder 14,077 / net 20,669 incl. tcp 5,250, unix 4,349 / dfx renamed from debug, 18,887 / … / trace 3,475 / other 22,714); verify row now kernel/verify 28,216 outside the kernel/src total |
| security/index.html / en/ | lmbench/GB evidence row: Geekbench MC 104% → 110% |
| architecture/design.html / en/ | redesign-performance bullet: GB MC 7139 (zh; en had 7332 — pre-existing cn/en drift) → 7709 (4-CPU pinned, best-of-runs) |

### README side (branch readme-refresh, off main 98702db0)

| File | Changes |
|---|---|
| README.md | tagline ~391,000→~378,000 lines; code badge URL `~391%2C000`→`~378%2C000`; Testing table LTP **1,093/1,869** (r27 audited official; raw 1,084), Performance row GB SC 2440 / MC 7709 (best-of-runs; 113%/110% vs the 4-CPU anchor) + fs flush 128 MB numbers, NEW boot-matrix row 16/16; Numbers table "Kernel source" → 623 files, 384,307 lines (377,650 Rust) with method (kernel/, git ls-files + wc -l) |
| docs/architecture/structure.md | Overall stats 427 files/~319,000 → 623/384,307 (377,650 Rust) + counting-method paragraph recorded for comparability; module table rebuilt (kernel/src scope line added: 453 files / 354,945; verify 168/28,216 outside); test stats LTP row → 1093 official; Total row made an explicit formula (1,267 + 1,093 + 1,838 = 4,198 + 161 proofs) |

## Numbers-on-both-sides audit (grep set-comparison, zh set == en set for every value)

| Number | zh files | en files | Parity |
|---|---|---|---|
| 2440 | 4 | 4 | OK (index, performance, changelog, testing) |
| 7709 | 5 | 5 | OK (+design) |
| 113% | 3 | 3 | OK |
| 110% | 4 | 4 | OK (+security) |
| 11.55 / 6.18 / 3.04 (fsync) | 3/2/4 | 3/2/4 | OK |
| 13.95 / 3.31 / 11.30 (msync/sync) | 3/4/3 | 3/4/3 | OK |
| 1093 | 5 | 5 | OK (index, roadmap, changelog, testing/index, testing/ltp) |
| 16/16 | 3 | 3 | OK (index, stability, changelog) |
| 98702db0 | 2 | 2 | OK (structure, changelog) |
| 623 files | 2 | 2 | OK (structure, changelog) |
| 377,650 / 384,307 / 354,945 | 2/2/1 | 2/2/1 | OK |
| 915,347 + 1,094 (repo-wide) | 1 | 1 | OK (changelog) |
| 37.8万 / 378k+ / ~378,000 | 1 | 1 | OK (index hero/footer/meta) |

Remaining old values on the pages are intentional historical context, present on both sides: r27 raw 1084 (audit notes), the 2026-11-04 capture totals 7332/2274/2115 (rig notes), the 2026-11-05 changelog entry, lmbench rows.

## Badge / URL-encoded sweep

- Homepage tree (incl. .github/workflows/pages.yml, all 50 HTML pages): **zero** %2C sequences, zero shields.io badges — nothing to update.
- readme-refresh tree: %2C only in README.md lines 10–11. Line 11 code badge updated (`~378%2C000%20lines`). Line 10 "tests-4%2C200%2B" left as-is (not in the refresh data set; see note).
- No other file in either tree embeds the changed numbers in URL-encoded form.

## Verification performed

- Code-size recount re-run on main @ 98702db0 before writing: kernel/ 623 files / 377,650 Rust / 384,307 total; repo-wide 1,094 / 915,347 / 378,153 — all match the official figures exactly. Module table sums reconcile (453 files / 354,945 lines kernel/src; + verify 168/28,216 + build files = kernel/ totals).
- GB ratios check: 7709/7027 = 109.7% ≈ 110%; 2440/2165 = 112.7% ≈ 113% (same Linux anchors as before, so only the Rux column changed).
- Style: flush numbers presented as before/after absolutes with rig notes, no comparative adjectives; GB presented as percentages, no 领先/优于 added; homepage uses explicit "Linux" naming per the corrected user rule; in-repo docs (README/structure.md) avoid new reference-kernel naming.
- No image references touched.

## Notes for the coordinator

1. README "tests-4%2C200%2B cases" badge and the homepage "4200+ 测试用例" stat were NOT in the refresh data set and predate it; structure.md's Total row is now an explicit 4,198 (1,267 + 1,093 + 1,838) — the badge's 4,200+ composition was never documented. Flagging for a future decision, unchanged here.
2. zh design.html said GB MC 7139 while en/ said 7332 (pre-existing drift); both now say 7709 (best-of-runs).
3. homepage branch was already checked out at /home/william/rux-agents/homepage (clean, idle since 2026-11-12), so the worktree-add form from the task brief failed; the commit was made in that existing worktree directly on `homepage`, per "homepage branch in the homepage worktree".
