# DOCS-REFRESH3 — official numbers refresh round 3 (homepage + README)

Date: 2026-11-16. Lane: docs. No kernel code touched, no QEMUs run.
Conventions per run/HOMEPAGE-REFRESH.md (round 2).

## Commits

| Side | Branch | Commit | Files |
|---|---|---|---|
| Homepage (cn+en in ONE commit) | `homepage` (worktree /home/william/rux-agents/homepage) | `3606981200df91235a36df0191e576664dc81413` | 22 files, +173/−145 |
| README + structure.md | `readme-refresh3` (off main @ fe5538c4e; worktree /home/william/rux-agents/readme-refresh) | `cc764891f1712af138d839694f8476f1da19737e` | 2 files, +35/−32 |

Not pushed (coordinator reviews). Both commit messages in English.
Note: the readme worktree was sitting on `readme-refresh2` (an ancestor of
main, fully merged); round 3 moved it to a fresh `readme-refresh3` branch
off main @ fe5538c4e so the README side starts from the coordinator's
direct edits (22b5ac916, f6efede9c, 03319ac3c) instead of behind them.

## Mission data set → where it landed

1. **LTP 1,139/1,869.** README already carried the bold number
   (03319ac3c) but with the 1,133-era parenthetical; both sides now carry
   the full trajectory: r27 audited 1,093 → 2026-11-15 scan2 audited 1,122
   → +11 tail fixes (b3eb4b16e) → +3 ltp-tail2 (9330bf395) → **official
   scan3 corrected 1,138** (main @ fecf6dd9f; census PASS=1138 TCONF=434
   FAIL=233 CRASH=31 EXECFAIL=17 TIMEOUT=12 WARN=4, zero POISON/WEDGE/
   NOTRUN; 17 expected gains all in — 3 scan2 in-round + 11 tail + 3
   tail2; one real loss tgkill03) → tgkill03 fix merged (878e3d9f9) →
   **current expected 1,139 (60.9%)**. Homepage ltp.html trend gains the
   11-16 bar (1138), all bar heights rescaled to the 1138 max.
2. **24.04.** Serial login (dev-ttyS0 found, getty started, prompt
   reached — newmount2 69167992d + ttydev fecf6dd9f, on main);
   gdm.service + systemd-logind start clean, zero 90 s rides (EROFS root
   cause: namespace bind RO re-flag closed the shared root fs latch —
   gdm-hunt lane @ 3e37fd83a + DRM SETCRTC vdisplay fix b86ff11a3, on
   branch gdm-hunt at refresh time); `make ubuntu-run` boots the full
   24.04 desktop by default (fc79ac68c, user directive 2026-11-16;
   UBUNTU_IMG=work/gnome-x86.img UBUNTU_INIT=/sbin/runit override keeps
   the 22.04 GNOME chain). Published on both sides; the remaining wall
   (greeter render, convoy family) is stated honestly, not overclaimed.
3. **22.04 GNOME 5/5.** gnome-regress lane (2026-11-16, main @ 1553b75ac,
   188 commits past campaign Round B): shell up 5 s (guest), desktop
   frame 9.1 s, Super-key interactive 5/5, zero panics/OOM/SIGSEGV.
   README "What runs today" + Ubuntu line; homepage lede, stability card,
   stability page §2, roadmap §1 + timeline, changelog.
4. **Other drift caught** (README → homepage mirror, plus fixes on both):
   - **make test 1,267 → 1,328 riscv64** (2026-11-16 gate: gdm-hunt
     maketest-rv64-gate.log, 1328 PASS; zero test-content diff
     f6efede9c..main tip, so the count holds for main @ fe5538c4e;
     x86_64 1,263 / aarch64 1,119 keep their 2026-11-01 standing).
     Test files 73 → 76.
   - **Code recount (main fe5538c4e, git ls-files + wc -l)**: kernel/ 626
     files · 386,698 Rust · 393,355 total; kernel/src 456 · 363,936;
     verify unchanged 168 · 28,216. Module table rebuilt (sync 19,843 now
     ahead of mm 19,432 / dfx 19,395; binder.rs 14,144; tcp 5,250; unix
     4,372). Headline 37.8万/378k/~378,000 → 38.7万/387k/~387,000.
   - **lmbench summary**: the homepage index perf card still cited the
     retired r14-native-denominator summary (10 of 14, best 4.2×, worst
     1.61×); both surfaces now carry the 2026-11-16 standing already
     authoritative on performance/index.html — 14 of 21 paired cases
     ahead or at parity vs the same-rig Linux guest reference (pipe 4.6×,
     ctx 4.0–4.5×, sem 2.9×), 7 behind (plain-syscall band 1.2–2.5×).
     README performance row likewise.
   - **Tests-total badge/stat 4,200+ → 4,300+**, now backed by the
     documented formula 1,328 + 1,139 + 1,838 = 4,305 (structure.md Total
     row 4,238 → 4,305; homepage testing/index sentence now states the
     formula instead of the old undocumented "suite + probes + benchmarks
     + proofs" composition — closes the round-2 note).
   - **performance page data-as-of** 2026-11-15 → 2026-11-16 (the page's
     §2 authority is the 11-16 standing; trivial date drift).
   - **en roadmap §1** "Aligning debugging with the reference kernel" →
     "the Linux kernel" (user rule: homepage comparisons name Linux).

## Numbers-on-both-sides audit (grep set-comparison, zh set == en set)

| Number | zh files | en files | Parity |
|---|---|---|---|
| 1139 | 5 | 5 | OK (index, roadmap, changelog, testing/index, testing/ltp) |
| 1138 / 1,138 | 5 | 5 | OK (same set) |
| scan3 census 434/233/31/17/12/4 | 1 | 1 | OK (testing/ltp) |
| 1328 / 1,328 | 6 | 6 | OK (index, changelog, testing/index, testing/stability, guides/getting-started, guides/development) |
| 5/5 (+188, 9.1 s, 5 s) | 7/4/– | 7/4/– | OK (index, performance [pre-existing 5/5 uses], changelog, roadmap, testing/index, testing/stability, ltp-timeout [pre-existing]) |
| 386,698 / 393,355 / 626 | 2 | 2 | OK (structure, changelog) |
| 38.7万 / 387k+ / ~387,000 | 1 | 1 | OK (index hero stat + meta/footer each side) |
| 4300+ / 4,300+ | 2 | 2 | OK (index stat, testing/index) |
| 4,305 formula | 1 | 1 | OK (testing/index; structure.md formula is README-side) |
| 14,144 / 4,372 / 20,857 / 69,821 … (module table) | 1 | 1 | OK (structure) |
| pipe 4.6× / ctx 4.0–4.5× / sem 2.9× / band 1.2–2.5× | 1 | 1 | OK (index perf card; performance page pairs were already in sync from the 11-16 lane commits) |
| fe5538c4e | 2 | 2 | OK (structure, changelog) |

Remaining old values on the pages are intentional historical context,
present on both sides: the 11-16 changelog's first two bullets (1122/1133
as the intra-day trajectory), the 11-15 changelog entry (623 files @
98702db0, GB numbers), the 11-01 changelog 1267/1267, r20=1028,
ltp-full.html row indexes 1132/1133 (case-table indices, not LTP counts).

## Badge / URL-encoded sweep

- Homepage tree (all 50+ HTML pages, css/js, .github/workflows): **zero**
  %2C sequences, zero shields.io badges — nothing to update.
- readme-refresh3 tree: %2C only in README.md lines 9–11. Line 10 tests
  badge `4%2C200%2B` → `4%2C300%2B`; line 11 code badge `~378%2C000` →
  `~387%2C000`; line 9 platform badge untouched (no numbers from this
  data set). structure.md carries no URL-encoded numbers.
- No other file in either tree embeds the changed numbers URL-encoded.

## Verification performed

- LTP facts cross-checked against the ltp-scan3 lane record
  (ltp-scan3/run/LTP-SCAN3.md): official corrected 1138, census row,
  +16 = 17 gains − tgkill03, post-merge expected 1139, tgkill03 fix
  878e3d9f verified 8/8 and merged to main via defdae9a4. Trend heights
  recomputed as value/1138 (35→3.1% … 1122→98.6%, 1138→100%).
- 24.04 facts cross-checked against GDM-HUNT.md (logind+gdm `[ OK ]
  Started` with zero 90 s rides in serial-boot9; EROFS root cause; SETCRTC
  vdisplay fix) and the Makefile on main (ubuntu-run → ub2404.img, SDL
  gui mode, runit override documented in the Makefile comment).
- 22.04 5/5 cross-checked against gnome-regress/run/GNOME-REGRESS.md
  (5/5 table, shell 5 s, frame 9.1 s, Super-key 67.56–67.84%, zero fault
  classes, main @ 1553b75ac, 188 commits).
- make test 1328: PASS-line count of gdm-hunt/run/maketest-rv64-gate.log
  = 1328, "Failed: 0 / ALL TESTS PASSED"; `git diff f6efede9c..main --
  kernel/src/tests/` empty and zero added test assertions in the whole
  kernel/ diff, so the count applies to main tip. Observed run-to-run
  band on near-identical trees is 1328–1330 (gnome-regress @ 1553b75ac
  saw 1329; ltp-scan3 fix1 saw 1330) — published 1,328 (latest full gate).
- Code recount re-run on main @ fe5538c4e before writing: kernel/ 626 /
  386,698 Rust / 393,355 total; kernel/src 456 / 363,936; module sums
  reconcile (443 module files + 13 top-level = 456; percentages against
  363,936). verify 168/28,216 unchanged.
- 4,305 = 1,328 + 1,139 + 1,838; badge floor 4,300+ holds.
- Style: no comparative adjectives added (ahead/behind stated as measured
  ratios with rig notes); homepage names Linux directly in comparisons
  (also fixed one leftover "reference kernel" in en roadmap §1);
  README/structure.md avoid kernel-name phrasing per in-repo convention.
- No image references touched.

## Flags for the coordinator (not changed by this round)

1. **Trace-events count "61 events / 20 groups"** (README debugging +
   Numbers rows, homepage index observability card, structure pages,
   11-07 changelog entry) could NOT be verified and was left alone:
   static count of the EVENTS table at main tip is 69 declared events /
   17 groups (ids 1–70 minus 11), and the same greps at e83bea83b — where
   "61/20" was authored — count ~49–50/16. The claim likely came from a
   live available_events read with a different counting convention. Needs
   one booted `cat available_events | wc -l` before the next bump; dfx
   -migrate's "+10 rux events (61-70)" is merged, so the number has
   probably moved either way.
2. **make test x86_64 (1,263) / aarch64 (1,119)** are 2026-11-01 vintage;
   no recent gate log exists for either. Date-stamped where published.
3. **In-repo docs behind the surfaces** (coordinator-maintained, flagged
   only): docs/progress/roadmap.md "Current Phase" still 2026-11-14;
   docs/test/ltp-report.md / ltp-tracking.md carry no scan3 section (the
   lane record is ltp-scan3/run/LTP-SCAN3.md); docs/test/unit-test-report
   .md still says 1267 (2026-11-01); repo AGENTS.md still says make test
   "1096/1096".
4. **gdm-hunt fixes (3e37fd83a, b86ff11a3) were NOT on main at refresh
   time** — the homepage changelog cites them as lane commits ("on the
   lane pending merge", en side; zh "随车道待并入"). If they merge before
   the site push, the wording stays true; if they are dropped, strike
   those two clauses.
5. **Repo-wide totals no longer publishable as a code number**: tracked
   run/ gate logs + vendored tarballs now put repo-wide at 1,152 files /
   1,428,265 lines (wc -l). Round 2's "repo-wide 1,094 / 915,347" is not
   apples-to-apples anymore; this round publishes kernel/-scope only and
   drops repo-wide from the changelog recount bullet.
