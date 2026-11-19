# Plain-language sweep of the homepage (zh root + en/ mirrors)

Scope: ALL 86 .html pages (43 zh + 43 en mirrors; no blog/ directory exists).
Method: per glossary pattern, `grep -rn` across zh pages and en/*.html, fix each
occurrence in context (no blind sed), re-grep to zero. Language only — no
layout/CSS/factual changes; numbers re-expressed, never re-rounded.
Baseline for counts: commit 7cd1d4f22 (branch homepage before the sweep).
Sweep commits: 40c2e700a, a2269a345, 951661ecb, ff6e8f274, 409bb5f87,
a2053ea07, f4ce87d37, 227f0f221, 7b12faa9d (9 commits, 70 files,
389 lines replaced 1:1). HTML tag balance verified on all 70 modified files.

## zh conversions (glossary families)

| Pattern (baseline occurrences, zh) | Converted to | Files touched (zh side) | Left / intentional keeps |
|---|---|---|---|
| 靴 (13: 单靴/每靴/不同靴/靴间/交错靴/串口靴/双盘靴/新靴) | 单次启动 / 每次启动 / 不同次启动 / 不同次启动间方差 / 交替启动测试 3 轮取中位数 / 脚本化串口启动 / 双盘启动 / 下一次启动 | performance/{fio,lmbench,rttests}.html, architecture/scheduler.html, progress/changelog.html, testing/ci-pipeline.html | 0 left |
| 混写 M 数字 (6.3M / 126.0M / 732M / 25M / 23.2M / 15.5M / 2.81M / 1.56M / 64.4M / 33.7M / 15.9M …) | 630 万 / 1.26 亿 / 7.32 亿 / 2500 万 / 2320 万 / 1550 万 / 281 万 / 156 万 / 6440 万 / 3370 万 / 1590 万 + "样本数 N" | index, architecture/{design,memory,scheduler,filesystem,ipc}.html, debugging/{perf,tracepoints}.html, performance/index.html, performance/microbench.html, security/index.html, guides/development.html, testing/gnome-retrospective.html | fio 作业尺寸 8M/40M/48M 与 1M 块大小 = MiB 工作负载尺寸,不是计数,保留 |
| med / avg / med-of-3 (zh 中的英文缩写,29 处 med 族) | 中位数 / 平均 / 最大;med-of-3 → 3 次取中位数 | performance/{rttests,lmbench,index}.html, architecture/design.html | debugging/perf.html 的 `PBENCH ... med=... avg=...` 是程序输出行格式串,原样保留 |
| j1 / j8 (3) | 单作业 / 8 作业(en: numjobs=1 / numjobs=8) | performance/fio.html + en 镜像 | 0 left(LTP 用例名 genj1 不是黑话,保留) |
| 电池 / 电池列 (8) | 整轮测试 / 整轮测试列;电池2 → 第二轮整轮测试 | performance/{rttests,lmbench,microbench,index}.html, architecture/{ipc,scheduler}.html | 0 left |
| 两臂/双臂/Rux 臂/Linux 臂/对照臂 (11 处含代码分支臂) | 两侧内核 / Rux 侧 + Linux 侧 / 对照组;递归臂/None 臂/存活臂 → 递归分支/None 分支/存活分支 | performance/{rttests,fio}.html, security/formal.html, architecture/{memory,design}.html, debugging/tracepoints.html, testing/stress.html | 0 left |
| 带内 / 在带内 / 带 X (13 + 带 X 门禁写法) | 在基准范围内 / 允许范围 X;平价带 → 平价范围;门禁带 → 门禁基准范围;调用量带/均值带 → 调用量范围/均值范围 | index, guides/development.html, performance/{index,fio}.html, architecture/{scheduler,design}.html, debugging/perf.html, security/index.html | taxonomy.html「全部带修复与回归证据」的"带"是动词(携带),保留 |
| 同窗 (4) | 同一测量窗口(时期义的同窗口 → 同一时期) | performance/fio.html, testing/gnome-retrospective.html | 0 left |
| 银行 / 重银行 (3) | 交替启动重新测量 / 重新测量 | performance/{rttests,index}.html, debugging/perf.html (REBANK 在案 → 重新测量已记录在案) | 0 left |
| 入册 (2) | 已登记 | architecture/{memory,boot}.html | 0 left |
| 在案 (7) | 已记录在案 / 记录在案;net 线在案 → 网络工作分支已记录在案 | ipc, scheduler, design, formal, geekbench, rttests, debugging/perf | 0 left |
| 定罪 (7) | 确认为根因 / 确认问题+修复根因 / 被查出误诊(仪器语境) | debugging/{memory,panic,locks}.html, security/taxonomy.html, architecture/interrupts.html | 0 left |
| 机位 (22) | 测试平台(各页首次出现处原本就内联 q35/KVM/核数/内存配置,保留) | index, performance/{fio,lmbench,geekbench,index,rttests}.html, testing/{index,ltp}.html, progress/{changelog,roadmap}.html, architecture/scheduler.html | 0 left |
| rig(中文页内的英文,~12) | 测试平台;同 rig A/B → 同一测试平台 A/B;rig 地板 → 测试平台的物理下限 | index, performance/{fio,index,lmbench}.html, architecture/{scheduler,design,interrupts}.html, security/{index,formal,hardening}.html, debugging/perf.html, testing/index.html, testing/ci-pipeline.html(PENDING-RIG 徽标 → 待接测试平台) | 0 left |
| 地板 (5) | 入口地板 → 入口路径下限;rig 地板 → 测试平台的物理下限 | performance/{lmbench,fio,index}.html | 0 left |
| census (39) | 调度事件普查,每页首次出现加注「启动期调度事件的计数器」;census 每 boot 审判 → 每次启动的调度事件普查判定 | index, guides/development.html, performance/{index,microbench}.html, architecture/{design,scheduler,interrupts,structure,kernel-lock}.html, debugging/{perf,tracepoints}.html, security/{index,hardening}.html, testing/{index,stress,stability}.html, progress/changelog.html | 调试 tracepoints 页的 hilog_census/hilogcensus 是 trace 事件名(代码标识符),保留 |
| 走表 (1) | 页表遍历 | architecture/boot.html | 0 left |
| 在跑 (7) | 正在运行的任务 / 正在运行者 | architecture/{scheduler,design,ipc}.html | 0 left |
| 对照对 (2) | 对照组 | performance/{fio,rttests}.html | 0 left |
| 死靴 (0) | — | — | 原本就没有 |
| 线 / 车道 / 立线 / 狩猎线 / 并发 5 线 (lane 族,~8) | 工作分支 / 专门工作分支 / 已单独开出工作分支 / 专用狩猎工作分支 / 并发工作分支 5 | index, testing/{stress,ltp,ltp-full,stability,gnome-retrospective}.html, progress/changelog.html, debugging/memory.html | debugging/memory.html 保留一处 "(lane)" 括注作对照 |
| 定尺寸 / 开线 / 合入主线 (0) | — | — | 原本就没有 |
| 参考内核 (18) + 其他「参考X」指代 Linux 内核的写法 (参考实现/参考语义/参考函数/参考形态/参考窗口/参考列/同机参考 等,共 90 处「参考」) | Linux(版本注保留:「Linux(同镜像 guest 内核 7.0.0-38)」);参考的 per-rq 锁 → Linux 的 per-rq 锁;参考语义 → Linux 语义;同机参考 → 同机 Linux;按 2026-11-18 用户裁定 | index, architecture/{boot,design,filesystem,interrupts,ipc,kernel-lock,memory,riscv64,scheduler}.html, debugging/{index,locks,memory,perf,tracepoints}.html, performance/{fio,geekbench,index,lmbench,microbench,rttests}.html, security/{formal,hardening,index,taxonomy}.html, guides/development.html, testing/stress.html | zh「参考」剩 0 |

## en mirror pass

| Pattern (baseline, en) | Converted to | Files touched | Keeps |
|---|---|---|---|
| "the reference kernel" (15) + "the reference's/the reference" in the kernel sense (~80 lines of 132 "reference" hits) | "Linux" / "Linux's X" (version notes kept) | en/{index,architecture/*,debugging/*,performance/*,security/*,guides/*,testing/*} — 24 pages | Legitimate English "reference" stays: dereference, Rust reference cycles, unreferenced, quick-reference / Command reference, INV- references, literature references (MPK 20–26 cycles, eBPF ~1% CPU), "external reference point: Asterinas", symlink verb |
| "both arms" / "the Rux arm + the Linux arm" / "two-arm microbenchmark" / "the control arm" (comparison sense) | "both sides" / "the Rux side + the Linux side" / "A/B microbenchmark" / "the control side" | en/{performance/{rttests,fio,index},architecture/{interrupts,design,memory},debugging/tracepoints,testing/stress}.html | "recursion arms" (Rust match arms), verb "arms" (enables a hook), WakeArmed, "arm lock" (timer arming), "arm one event" (tracefs usage) |
| "in band" / "all in band" (11) | "within the allowed band" ("band" kept as the defined noun) | en/{index,performance/index,guides/development}.html | "the parity band", "gate bands", "release-class band" (defined nouns) stay |
| "re-bank" / "re-banked" / "REBANK" / "banked" (5) | "re-measurement" / "re-measured" / "measured and recorded" | en/{performance/{rttests,index},debugging/perf,progress/changelog,architecture/design,security/hardening}.html | "reader-bank" / "magazine bank" (per-CPU counter banks — different, standard sense) |
| "convicted" / "conviction" (12) | "confirmed as the root cause" / "established" / "proven" / "caught by" / "were also found at fault" | en/{architecture/{interrupts,kernel-lock,riscv64,design},debugging/{locks,memory,panic},security/taxonomy,progress/changelog}.html | 0 left |
| "rig floor" (2) | "the test platform's physical floor" | en/performance/{fio,index}.html | "rig"/"same rig"/"standing rig" kept (industry usage, per glossary) |
| battery shorthand ("battery column", "three batteries", "battery2", "this battery") (11) | "full-suite column", "three batches", "suite run 2", "this suite run", "full round", "full suite" | en/{performance/{rttests,lmbench,microbench,index},architecture/{ipc,scheduler},testing/gnome-retrospective}.html | none |
| "lane" bare (22 lines, incl. non-jargon senses) | "work lane" / "work-lane" on each page's first use; "hunt lanes (branch-only)" → "hunt work lanes" | en/{index,testing/{gnome-retrospective,stress,ltp,ltp-full,stability},debugging/memory,performance/rttests,guides/getting-started,progress/changelog}.html | project vocabulary kept, now self-explanatory |
| "med-of-3" (4) | "median of 3" | en/performance/lmbench.html | avg/med as table abbreviations stay (standard in English) |
| "sized M" | — | — | none found (only "zero-sized key types", legit Rust) |
| en M-notation (6.3M per boot, 25M/boot …) | unchanged | — | intentional keep: M is standard English engineering notation (glossary note) |

## Verification (final state)

- zh patterns 靴/参考/机位/带内/定罪/在案/入册/银行/同窗/臂/走表/在跑/对照对/电池/车道/census(除事件名)/每 boot/M 计数混写: **0 remaining** (only keeps listed above).
- en patterns reference kernel / arms(comparison) / in band / re-bank / convicted / rig floor / battery(suite sense) / bare lane / med-of: **0 remaining** (only keeps listed above).
- <title> and meta description tags across all 86 pages: scanned, no jargon found (index.html title/meta already plain).
- 70 modified files re-parsed with an HTML tag-balance checker: 0 issues.
- Numbers: presentation-only changes (6.3M → 630 万 etc.); no value altered anywhere.
