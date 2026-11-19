# Table-format sweep (homepage lane, 2026-11-18)

Mission: mobile readability — performance tables keep only 项/Job | Rux | Linux(同机) | 比值;
notes content moves to a compact footnote paragraph under the table. Bilingual same-commit.
No numbers or honest caveats deleted; everything relocated.

## Comparison tables (now exactly 4 header cells each)

| Page (zh + en) | Change | Columns removed | Footnotes added |
|---|---|---|---|
| performance/fio.html | ratio-cell parentheticals capped ≤15 zh chars / ≤4 en words | none (already 4 cols) | fresh-write mechanism (async unwritten conversion + delalloc), buffered-write physical floor (cold-frame RFO), flush-wall pending SMP8 re-measure (fsync rebuilt), numjobs=1 vs 8 basis |
| performance/rttests.html | 说明/Notes column dropped → 项/Rux/Linux/比值 | 说明 (Notes) | clean-pass basis, load-wave 2.2–2.4 ms shared, FIFO path 104→46→14–19, residual adjudication, pi_stress OWNER_DIED / D-26 / 374 µs detail, hackbench 3-run medians |
| performance/lmbench.html | both tables 4 cols; headers 用例/Rux/Linux(同机)/比值 | 优化方向 (dangling empty column, behind table) | image-runner-fix recovery note (bw_file_rd, lat_pagefault), lat_syscall null mechanism (fdget+getppid O(1)+PT_REGS+fdreap+NOHZ); 参考内核→Linux wording fix |
| performance/geekbench.html | totals + both per-workload tables 4 cols | 类/Class column merged into 项/Item ("整数 · 文本压缩") | none needed (existing AES-XTS footnotes kept) |
| performance/microbench.html | flush-wall table already 4 cols; Tier-1 band table cells stripped to bands | none | Tier-1 parentheticals (8/32 exact reads, arc 104–124/s, p7_sleep1ms 15.8 µs, if0 v3 official, 6–9 ms current, one-shot 594–599 ms gone); en sync-basis caveat added |

## Spirit trims (non-comparison tables, structure kept)

| Page (zh + en) | Table | Trimmed into cells→footnote |
|---|---|---|
| performance/index.html | merge-gate tier table | Tier 1 requirement cell → bands only; footnote: census = boot-time counters, per-op list, link to microbench page |
| performance/index.html | six guarded domains | scheduler row → numbers only; footnote: probe basis, 70–84 ns KVM tax, call-site IF=0 basis n=6.3M, quiet-load futex |
| performance/index.html | landed-lever table (§5) | fs-flush result cell, r5/r7 mechanism, r10 result, r12–14 mechanism shortened; footnote carries mtest06 shape, ~3.8× cumulative, mmap2/fsx PASS list, path_memo detail, File-pool refutation, six-leg gate |
| guides/development.html | gate tier + domain tables | same treatment as index gate tables (glosses → footnotes) |

## Verification

- `<th>` count per comparison table: fio 4, rttests 4, lmbench 2×4, geekbench 3×4, microbench 4 (+2-col band table) — all 项+3 data.
- zh comparison cells: no sentence-style cell >25 chars remains (job/parameter identifiers and number lists excepted).
- Measurement-basis labels survive: 干净轮 / 交替测试 3 轮取中位数 / 测试平台的物理下限 / 单作业 vs 8 作业.
- Load-wave caveat = one footnote sentence (rttests zh+en). vdso/poison and different-boot caveats: not present on these pages (nothing to collapse).
- Tag balance checked on all 14 edited files: OK.

Commits: e7be953 (fio), a2d9541 (rttests), 3d9b378 (lmbench), 4b74e06 (geekbench),
86dec6f (microbench), 5fa7287 (perf index), 16537c3 (dev guide).
