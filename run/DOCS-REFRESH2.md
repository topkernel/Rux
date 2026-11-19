# DOCS-REFRESH2 — LTP 1133 current-expected / 1122 audited-scan citation

Date: 2026-11-16. Lane: docs (homepage worktree). No kernel code touched,
no QEMUs run. Predecessor handoff: LTP story assembled, numbers verified
against lane notes, surfaces gathered — this session executed the full
refresh on both surfaces.

## The published story (verified against lane evidence)

- **Audited-scan citation: 1122/1869 (60.0%)** — ltp-scan2 lane
  (`/home/william/rux-agents/ltp-scan2/run/LTP-SCAN2.md`), full 1,869-test
  scan on clean 1ab0019ef, x86_64 q35 + KVM, -smp 2 -m 2G, corrected-count
  protocol (every POISON/WEDGE-parked, flake-suspect and census-block face
  isolated-rerun 2x, later-run-wins merge). Census (sums to 1869, zero
  POISON/WEDGE/NOTRUN): PASS=1122 TCONF=434 FAIL=250 CRASH=30 EXECFAIL=17
  TIMEOUT=12 WARN=4. Net +29 vs the previous audited 1093 (32 gains,
  3 losses; the losses accept03/ioperm01/iopl01 were root-caused, fixed
  (0dc51136d, f9ef16b29) and merged inside the same round). Merged in main
  at 1ca4bee79 ("LTP official 1122/1869 (+29)").
- **Current-expected: 1133/1869 (60.6%)** — ltp-tail lane
  (`/home/william/rux-agents/ltp-tail/run/LTP-TAIL.md`), merged at
  **b3eb4b16e** (in main @ 9ca190d6d, verified by merge-base). +11 tests
  verified FAIL/TIMEOUT pre-fix and PASS 2/2 in isolation post-fix:
  splice06, keyctl04/05/07, request_key02/04, fsconfig01/02, fspick01,
  mount_setattr01, move_mount02. The merge message itself says
  "LTP expected 1133/1869 (+11)" — the published figure matches the
  merge commit exactly.
- Arithmetic footnote for the coordinator (flagged, NOT changed): the
  scan2 notes say expected-post-fix = 1125 (1122 + the 3 loss fixes), so
  1125 + 11 tail = 1136 > 1133. The tail lane counted its +11 off the
  1122 official base and published 1133; the task directive and the
  b3eb4b16e merge message both say 1133, so 1133 is published. If the
  next full scan lands ≥1136 the wrinkle self-resolves; otherwise a
  future count audit should reconcile the 3 rows.

## Commits (NOT pushed — coordinator reviews)

| Side | Branch | Commit | Files |
|---|---|---|---|
| Homepage (cn+en in ONE commit) | `homepage` (worktree /home/william/rux-agents/homepage) | `f0a2ab8f93e90a01916f5fd0d282bfc2a6019ba5` | 10 files, +64/−50 |
| README + structure.md | `readme-refresh2` (off main @ 9ca190d6d; worktree /home/william/rux-agents/readme-refresh) | `5ff4471490ada6a1ca44db5d27e37f497418f71b` | 2 files, +3/−3 |

Both commit messages in English. `readme-refresh2` was cut fresh off main
9ca190d6d (previous `readme-refresh` tip efe9c2416 is already in main, so
the branch carries the prior refresh forward with no cherry-picking).

## Diffs summary

### Homepage — 5 page pairs, every change landed zh + en/ in the same commit

| Page (zh / en/) | Changes |
|---|---|
| index.html / en/index.html | stats bar LTP 1093→1133; compat card nums LTP 1133/1869; LTP card paragraph rewritten (35 → audited 1,122 (+29 vs 1,093); tail-fix round +11 → current expected 1,133; dropped the r27-specific TIMEOUT breakdown parenthetical); nums row "current 1133/1869 (audited 1122 + 11 tail fixes)" |
| testing/index.html / en/ | LTP card: audited round 1122/1869 (60.0%) → current expected 1133/1869 (60.6%) after tail fixes; "ten-round trend" → round trend + isolated-rerun correction protocol wording; report table LTP row "audited 1122/1869 → expected 1133/1869 (x86+KVM) · 1028/1869 (riscv64)" |
| testing/ltp.html / en/ | top paragraph: latest audited round 2026-11-15/16 story (+29 decomposition, zero POISON/WEDGE/NOTRUN, tail +11 → 1133); distribution table → corrected census PASS 1122 / TCONF 434 / FAIL 250 / CRASH 30 / EXECFAIL 17 / TIMEOUT 12 / WARN 4 (WEDGE column dropped — class is zero); trend gains an 11th bar 1122 (all bars rescaled to the 1122 max; 1093 keeps a plain bar at 97.4%), x-label "11-15"; trend caption adds the 1122 → 1133 arc; methodology archive bullet now names both lanes (ltp-r27, ltp-scan2); rigs table row 1: 2026-11-15 audited = 1122/1869, current expected 1133/1869 after tail fixes |
| progress/roadmap.html / en/ | current-goal LTP line: r27 audited 1093 → current expected 1133/1869 (audited 1122 + 11 tail fixes); the 2026-11-05/06 milestone heading keeps its historical 1084/1093 |
| progress/changelog.html / en/ | NEW 2026-11-16 entry: audited round 1122/1869 (+29; losses fixed in-round; zero POISON/WEDGE/NOTRUN) and tail-fix round at b3eb4b16e (+11) → current expected 1133/1869 (60.6%); 2026-11-15 entry left as history |

Intentionally unchanged: testing/ltp-full.html + en/ (the explicitly
labeled "r27" per-case archive, raw chips PASS 1084 et al., dated
2026-11-05) and testing/ltp-timeout.html + en/ (the dated 2026-11-07 r27
TIMEOUT audit). The ltp.html note under the distribution table now points
to the archive page for the raw r27 distribution.

### README side (branch readme-refresh2, off main 9ca190d6d)

| File | Changes |
|---|---|
| README.md | Testing table LTP row: **1,133 / 1,869** (current expected; 2026-11-15 audited scan 1,122 + 11 tail fixes merged at b3eb4b16e) |
| docs/architecture/structure.md | Test Statistics LTP row: 1133/1869 current expected (x86_64 KVM; 2026-11-15 audited scan 1122 + 11 tail fixes) · 1028/1869 (riscv64 TCG); Total row formula → **4,238 test cases (1,267 + 1,133 + 1,838)** + 161 proofs |

## Numbers-on-both-sides audit (grep set-comparison, zh set == en set)

Aggregate across the 5 changed pairs (per-pair comparison also run; all
refresh-set tokens matched per pair):

| Number | zh total | en total | Parity |
|---|---|---|---|
| 1133 + 1,133 | 11 | 11 | OK (index 4, testing/index 2, ltp 3, roadmap 1, changelog 1 — same split both sides) |
| 1122 + 1,122 | 11 | 11 | OK (index 2, testing/index 2, ltp 5, roadmap 1, changelog 1 — same split both sides) |
| 60.6% | 3 | 3 | OK (testing/index, ltp, changelog) |
| 60.0% | 2 | 2 | OK (testing/index, ltp) |
| 434 / 250 / 30 / 17 / 12 / 4 (census) | 1 ea. | 1 ea. | OK (ltp distribution row; 17 also in the archive chips) |
| 29 / 32 / 3 (delta decomposition) | 6/7/na | 6/7/na | OK (3 is a common substring, per-pair spot-checked in context) |
| 11 (tail fixes) | matched per pair | matched per pair | OK |
| b3eb4b16e | 1 | 1 | OK (changelog) |
| 11-15 (trend label) | 10 | 10 | OK (dates elsewhere incl.) |
| 1093 + 1,093 (historical) | 8 | 8 | OK (ltp trend/caption/rigs, changelog 11-15 + 11-05/06 entries, index "previous audited 1,093") |
| 1084 + 1,084 (historical) | 3 | 3 | OK (changelog history, roadmap milestone heading) |

The only raw-substring mismatches found ('30', '17' on index.html) are
pre-existing drift OUTSIDE the refresh set: zh perf card line "p1_null
0.30µs · 战役史全表" vs en "14 paired cases · full ledger", and the zh
css/js cache-busters `?v=17`. Flagged below, not touched.

## Badge / URL-encoded sweep

- Homepage tree (all 50 HTML pages, .github/workflows/pages.yml, js, css):
  **zero** %2C sequences, zero shields.io badges — nothing to update.
- readme-refresh2 tree: %2C only in README.md lines 10–11. Line 11
  `~378%2C000 lines` untouched (code size not in this refresh). Line 10
  `tests-4%2C200%2B cases` left as-is: with the Total now 4,238 the badge
  is strictly true for the first time (the old 4,198 was slightly under).
- No other file in either tree embeds the changed numbers URL-encoded.

## Verification performed

- Census sum check: 1122+434+250+30+17+12+4 = 1869. Percentages:
  1122/1869 = 60.03% → 60.0%; 1133/1869 = 60.62% → 60.6%.
- Trend rescale check: all 11 bars = value/1122, 1 decimal; acc bar 1122
  at 100%; label sequence r1…r27, 11-15.
- b3eb4b16e and 1ca4bee79 confirmed in main @ 9ca190d6d via
  merge-base --is-ancestor; the +11 test list cross-checked between the
  task brief, the ltp-tail lane notes and the merge message.
- Style: no comparative adjectives introduced (gains/losses stated as
  counts); no new reference-kernel wording needed (LTP is not a
  comparison surface); homepage comparisons continue to name Linux
  directly where they existed (changelog GB row untouched).
- No image references touched; no kernel code touched.

## Notes for the coordinator

1. **Arithmetic footnote** (see above): 1122 + 3 in-round loss fixes + 11
   tail = 1136 vs the published 1133. Published 1133 follows the task
   directive and the b3eb4b16e merge message verbatim. Suggest the next
   full-scan lane's official count supersedes this cleanly.
2. Pre-existing zh/en drift on index.html (perf card "p1_null 0.30µs" zh
   line vs en "14 paired cases") — outside the LTP set, unchanged here,
   same class as the design.html 7139/7332 drift the previous round fixed.
3. The new trend bar is labeled "11-15" (date): the scan2 lane never
   assigned an r-number; inventing "r28" on the public page was judged
   worse than a date label.
4. ltp-full.html remains the r27 per-case archive (labeled and dated);
   a per-case page for the 1122 round would be a separate, larger task
   (scan2's corrected results file is
   /home/william/rux-agents/ltp-scan2/run/results/results-scan2-corrected.txt).
5. Neither branch is pushed. readme-refresh2 sits in the readme-refresh
   worktree; the old readme-refresh branch is fully merged into main and
   can be deleted after this lands.
