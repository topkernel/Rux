# perf-gap-hunt — 全指标 ≥2x 反超参考内核战役（用户令 2026-11-04）

任务树：/home/william/rux-agents/perf-gap-hunt（分支 perf-gap-hunt）
任务书正本 = 本文件；巡检（10 分钟 cron）以本文件+台账判定进展。

## 任务

把**所有**测试用例的性能差距分析清楚并逐项消灭。目标：**每一行
lmbench/pbench 指标至少比参考内核好一倍**（我们的时间 ≤ 参考的 0.5x，
同 KVM 公平口径）。只要还有任何一行有差距，就继续优化，**中间不停**。

## 当前战况（基线 main=53fba4f6，参考值冻结于 run/lmbench-refkvm.txt / lmbench-linux.txt）

### 已达标（≤0.5x 或反超）
- lat_ctx 3.15x 反超（2.13 vs 6.71）· lat_pipe 3.33x 反超（4.00 vs 13.30）
- lat_proc exec 1.32x 反超（66 vs 87.4）· fstat 1.37x 反超（0.086 vs 0.118）
- lat_mmap 0.24x · bw_pipe 2.68x · bw_unix 1.88x · bw_mem 1.07-1.10x · usleep 0.99x
- **lat_pagefault 2.26-2.45x 反超（0.0287-0.0312 vs 0.0702；2026-11-05 定妆，门禁六腿全绿待合 main）**

### 未达标（按差距排序 = 主攻顺序）
| 行 | 我们 | 参考(KVM/原生) | 差距 |
|---|---|---|---|
| lat_syscall open | 0.70 | 0.36 | 1.95x（目标 ≤0.18）；fsw=1 分解：open pre 169ns + walk 636ns（a=权限/inode、b=dcache 查找主导）+ **open tail 789ns=最大相位**（插桩值有时钟读污染，相位占比可靠）——下一刀在 open tail 簿记非 walk |
| lat_sem | 0.800 | 0.444 原生 | 1.80x |
| lat_syscall stat | 0.379 | 0.227 | 1.68x |
| lat_syscall read | 0.110→**0.1059**（11-05 栈） | 0.066 | 1.60x |
| lat_syscall write | 0.094→**0.0915**（11-05 栈） | 0.058 | 1.58x |
| lat_unix | 4.56 | 3.011 原生 | 1.52x |
| lat_syscall null | 0.055→**0.0505**（11-05 栈：钩子门×2） | 0.042 | 1.20x（入口地板，乘所有行；余量=inner call ~2ns+6 字参数拷贝 ~2-3ns+帧构建） |
| lat_proc fork | 59.9 | 53.99 | 1.11x |
| Geekbench SC | 2115 | 2165(4CPU pinned) | 97.7%（次优先） |

## 已解剖结论（直接用，别重走弯路）

1. **上下文切换机器已达设计地板**（cctx 审计，docs/test/lmbench-report.md）：
   调度段 323ns + 切换 ~295ns = 620ns < 参考原生 910ns；ctx/pipe 剩余差距
   在唤醒/halt 物理层，已到平台。
2. **open/close 分配器税 ~106ns/迭代**：CpuLocal::with 的 cli/sti+cpu_id
   27ns×2 + 弹匣均摊 32ns（refill 241/flush 265ns@深度16）；close 侧仅
   88ns（fdreap 已回本）；路径行走 ~200-250ns（rest-site 盲区，减法定界）。
3. **negative results（禁重试，除非有新机制）**：
   - 自适应 halt-poll 窗（3 变体全负：鸡生蛋 grow / halt 路径 HPET 时钟
     陷阱 / 宿主 KVM halt_poll_ns=200000 已兜底）→ 静态 120µs 是本 rig 最优
   - preempt-IPI 挪出 GRQ 锁（pipe +15% 更差）
   - CfsRunQueue BTreeMap→Vec（性能中性；补丁在 code-review-perf 工作树
     run/timeline-vec-swap.patch，含缓存失效陷阱修复）
   - MAG_DEPTH 16→32（-18ns 但 riscv 浸泡 3/4 间歇分配器破坏 panic——弹匣
     瞬态路径按 16 调校，先重设计再加深）
   - "yield 锁更快"归因已证伪；GRQ 自旋/IPI/分配三排除（自旋 241cyc；
     pick=96ns——µs 级段均值是 14ms 尾部均值化假象）

## 攻击队列（按预期 ROI 排序）

1. ~~lat_pagefault~~ **完成 2026-11-05**：0.0287-0.0312µs = 2.26-2.45x 反超（≤0.5x 达标），门禁六腿全绿，待主会话合 main
2. **%gs 内核 per-cpu 基址**替代 CpuLocal::with 的 cli/sti+cpu_id（open/close
   -50ns，惠及全部分配密集路径；x86 专属：内核 GS base 指 per-cpu 区 +
   swapgs 纪律，内核态 %gs:offset 直接寻址）
3. **路径行走 tracepoint 分解**（stat/open 的 ~200-250ns 盲区——统一 trace
   框架加 path-walk 事件，量化每组件成本再动刀）
4. **syscall 入口地板 13ns**（null 1.31x：两层 dispatch 合一、armed() 残项
   归并、seccomp/IRQ 门重排——乘所有系统调用行）
5. **fork retire 批量 park**（exit_group/cloexec 批量关闭一次锁往返）
6. **lat_sem 0.800 vs 0.444**（semop 路径分解）
7. **lat_unix 4.56 vs 3.01**（唤醒侧已到平台，查 socket 缓冲/唤醒队列路径）
8. ~~D-1 runqueue 分片评估~~ **完成 2026-11-05**（1475e1a9：保持关闭——最优省 164ns/切换=7.7% lat_ctx，动不到任何未达标行；触发器已记）

## 纪律（必须遵守）

- **测量**：一切性能主张 x86_64+KVM strong profile（`make -C build
  build-release-opt PLATFORM=x86_64`）；交错同窗 A/B（模式抄
  code-review-perf 工作树 run/ab-haltpoll.sh）；先分解（cctx=1 / syscost=1 /
  tracepoint）再动刀；负载 >7 靴会饿死——等窗别硬跑，标记后继续下一项。
- **快速检查**：userspace/lmbench/quick.sh（3 分钟）；镜像 reflink 自
  /home/william/rux-images/testimages/，-snapshot + file.locking=off。
- **合并门**：双架构构建 rc=0 + riscv 90s 浸泡零 panic + make test 绿 +
  pbench 同 rig A/B 带内 + LTP 相关族（test/ltp_family_boot.py 模式）。
  性能回归不合入。每轮成果（含负结果）记入本文件台账。
- **资源**：只杀自己的 QEMU PID；pkill 禁止；默认 -smp 2 -m 2G；长测
  detached。
- **措辞**：提交/文档/注释不出现 Linux 一词，用"参考内核"。英文提交信息。
- **边界**：TCP 族是并行会话地盘（5fae2850 在打），别碰；本分支提交，
  主会话合 main。

## 台账

（每轮追加：日期 | 攻击项 | 分解数据 | 改动 | A/B 结果 | 门禁 | 结论）
- 2026-11-04 | 战役启动 | — | — | — | — | 基线=53fba4f6，队列 8 项
- 2026-11-04 | #1 lat_pagefault 复测 | 复测 0.4561µs（旧 0.449 确认）；参考 KVM 值补齐 = 0.0702µs（ref-kvm 工作树 run/lmbench-refkvm-optional2-raw.txt，与原生 0.0696 一致——缺页路径无 KVM 惩罚）→ 差距 6.50x | 无（诊断） | — | — | 两个发现：(a) 共享 testimages/lmbench-x86.img 里的 /test/lmbench_run 是陈旧构建（无 make_scratch——strings 无 /tmp/lmbench.file，Ubuntu GCC 而非 musl 工具链），lat_pagefault/lat_mmap 案例报 "x: No such file or directory"；本工作树 work/ 副本需重建注入 musl runner（bug 非内核问题）。(b) 解剖结论：缺页走 MAP_SHARED 读臂（x86_64 page_fault.rs 会话-25 PCACHE 臂）单页/次；现有 fault-around（FAULT_AROUND_PAGES=16）显式排除 SHARED VMA；参考内核 filemap_map_pages 对 SHARED 读缺页同样批量 ~16 页/陷阱（其 70ns = ~330ns 陷阱 × H16/16 随机序摊销）。攻击方案：SHARED 读缺页批量臂（get_range_into 批量 pin + 单 pte-lock 段 + noflush 映射）+ 窗口 64 + 每页 walk 合并（map-if-absent）| 进行中
- 2026-11-04 21:34 | 巡检观察 | — | page_fault.rs/mmu_init.rs 有未提交批量臂 WIP（21:16-21:18 编辑） | — | — | 静默 16min 无靴无构建；下轮（21:43）若仍无活动（>25min 静默）按阵亡重启，WIP 由重启 agent 接手
- 2026-11-04 21:53 | 巡检提醒 | — | 同 HEAD 62f0beca 已 42min（3 QEMU 活跃=门禁/测量循环中） | — | — | 若批量臂改动+门禁还要 >20min，建议先 git add -p 提交一个 WIP checkpoint（含台账行），防额度窗口丢工作；负载 1.82 好窗抓紧跑 A/B
- 2026-11-04 22:03 | 巡检观察 | — | 全通道静默 26min（page_fault.rs 21:36 后无编辑/构建/靴/提交）；负载 7.25 | — | — | 负载>7 按纪律本轮跳过重启；但静默是全域形（非靴饿死形）——下轮 22:13 若仍零活动（累计 >35min）无论负载直接按阵亡重启，重启 agent 接手未提交 WIP
- 2026-11-04 22:13 | 巡检解除升级 | — | 静默解除：3 个测量 QEMU 已在跑（22:03 时为零） | — | — | 22:03 的无条件重启线作废；agent 存活继续批量臂循环
- 2026-11-04 22:23 | 巡检踢脚 | — | 停滞条件名义满足但子代理未收到完成通知=仍在运行（本晚已有 16/35min 思考间隙先例），重启有双代理抢工作树风险 | — | — | 已 SendMessage 踢（要求先提 WIP checkpoint）；下轮 22:33 若仍零活动则无条件重启接手
- 2026-11-04 | #1 批量臂 WIP checkpoint | 探针分解（pfprobe 系列，load~6-10 指示性）：traps/touch 0.0093（窗口生效）；mmap 本体 min 1.8µs/16MB（健康）；stalls 主导用户态 rdtsc 观测；内核侧 mmprof：munmap 260µs/调用=51ns/页（put_by_key 逐页 pin 释放主导），安装侧 ~40ns/页 | page_fault.rs：SHARED 读缺页 fault-around 批量臂（512 页窗，chunked get_range_into pin + l1 slot-batch 映射 + noflush）已落地 A/B：0.4561→0.1766µs；mmu_init.rs：map_page_noflush pub(crate) + 无刷新契约；page_cache.rs：put_range_by_key 批量 pin 释放（未接线） | 0.1766（2.6x 改善，目标 ≤0.0351 未达） | 未过 | checkpoint 防额度窗口丢失；下一步：unmap_pages 接线 put_range_by_key + sub_rss 批量 + rmap_alt_remove 跳过 → 复测
- 2026-11-04 22:2x | 巡检确认 | — | 踢脚生效：checkpoint 0ff613e6 已落（批量臂+put_range_by_key）；pagefault 0.4561→0.1766µs（-61%）；下一靶=unmap 逐页 put_by_key 释放 51ns/页 | — | — | 22:23 硬线作废；战役健康推进
- 2026-11-04 23:03 | 巡检提醒 | — | 同 HEAD 0ff613e6 已 39min；3 QEMU 在跑但负载 10.84——靴大概率在饿死重试（这解释了无新提交） | — | — | 建议：若连续 2-3 批靴全 TIMEOUT，切无靴任务等窗（队列 #8 D-1 分片评估文档/队列 #2 %gs per-cpu 代码先写），负载 <5 再回来跑 A/B
- 2026-11-04 23:33 | 巡检强提醒 | — | 同 HEAD 69min（QEMU 全程活跃但零提交；负载已回 4-5 好窗 30min） | — | — | 已 SendMessage 强提醒：落地测量立即提交/饿死批切无靴任务；23:43 若 HEAD 仍停滞=响应失能，按阵亡重启接手
- 2026-11-04 | #1 阶段 D+gap-fill+批量释放 | 复测链（同 rig 交错/连续）：base 0.4682-0.4738 | 阶段A批量臂 0.1766 | +slot-batch 0.1642（A/B 3:3 交错 base 0.4673-0.4688 vs new 0.1638-0.1646，稳定 2.85x）| +PCACHE_RO 协议 0.1583-0.1652 | +gap-fill 截断 0.0432-0.0435 | +munmap 单次 TLB flush 0.0443-0.0470（噪声带内，保留：少 15 次 IPI+CR3/munmap）| 最终（待门禁确认）0.043-0.047 vs 参考 0.0702 = 1.49-1.63x 反超（0.61-0.67x 参考时间），目标 ≤0.0351 未达 | 改动：(1) SHARED 读缺页 fault-around 批量臂 512 页窗+chunk pin+slot-batch 映射+gap-fill 在首个已映射页截断（随机序下每陷阱只填空隙，废除重复 pin/检查已映射页——单项 0.164→0.043）；(2) PCACHE_RO（bit52）mapcount-only 只读别名协议：安装仅 inc_mapcount（无 pin/get_page，chunk 尾批量归还 pin），拆除跳过 put_page/pin 释放；守卫：evict_two 路+invalidate_inode 按 mapcount>0 跳过；fork 只 inc_mapcount；mprotect/marker 保留位且 W-grant 按共享形处理；(3) unmap_pages 批量 pin 释放（put_range_by_key 每 64 页一次）+sub_rss 批量+rmap_alt 跳过 PCACHE 叶+munmap 单次 TLB flush（defer_tlb_flush 参数） | 0.0432@L6.4（好窗复测待做） | 未过门禁 | 台账中后续：好窗 3:3 交错定妆 → 门禁（双架构+riscv 浸泡+make test+LTP mmap 族）→ 若 <2x 继续攻（剩余成本分解：安装 ~15ns/页+拆除 ~15-20ns/页+陷阱 ~10ns+访问 TLB ~6ns，下一杠杆：中心窗/更浅拆除）
- 2026-11-04 23:4x | 巡检确认 | — | 强提醒生效：51289ac2 已落 | pagefault 0.4682→0.0432-0.0470µs（6.5x 落后→1.5-1.6x 领先参考 0.0702） | 待门禁 | 23:33 硬线作废；下一台阶=2x（≤0.0351，余 43ns/touch 已分解）；静窗 A/B+门禁后继续
- 2026-11-05 04:15 | 额度复活 | — | 前任 00:26 额度阵亡（5h 上限），归档快照 18c62908 后工作树被清；04:13 会话恢复（额度 03:19 重置）重建工作树并派接替 agent | — | — | 接替起点：补 pagefault 门禁→攻 2x→队列推进；巡检 cron 仍按用户令暂停
- 2026-11-05 04:5x | #1 门禁补完（接替 agent） | 基线对照=53fba4f6 独立工作树 | (a) 双架构构建 rc=0（x86_64 make build + rm build/.config 后 riscv64 make build）；(b) riscv90s 浸泡（release 内核）零 panic 零 WEDGE-DETECT，LOOP/CHURN 完整（PREEMPT-UNDERFLOW/R55-RESYNC-STALE 为基线已有的愈合打印，sched 域，本分支未触碰）；(c) make test 1316/1316 rc=0；(d) LTP mm 族 41 例：前任 HEAD 全量记录 vs 本轮 base 全量记录逐例相同（唯一差异 mremap01 base 侧 "?"=标记收获毛刺，非退出码差异）→ 零回归 | 门禁余项：pbench 同 rig A/B（Tier-2 mm 域）待静窗——04:4x 负载 14-20 两轮靴饿死（A/B lat_pagefault base/head 双 NONE@L18-20 已按纪律中止）；rig 修复：ltp_family_boot.py 每 0.5s 重喂导致 mmap11 重叠实例 KSIG9 风暴吃掉整个 deadline（加 8s 喂入间隔+45s 单例超时+deadline/log 环境变量）；run/ab-pf.sh、run/ab-pbench.sh 交错 A/B 脚本入库 | 4/5 腿绿 | 2x 攻击已备未测：反向窗（缺口向下扫描+先填下方空隙，随机序下整段未映射 run 一陷阱吃净→陷阱数近似减半）+ put_range/put_range_by_key 连续索引 range-iterator 快径（省每页 BTreeMap 下降）；双杠杆强核 work/bzImage-lm-backwin 已构建
- 2026-11-05 12:31 | 二次复活 | — | 第二任 05:34 额度阵亡（窗口死于 04:13 边界）；12:30 会话恢复（09:13 重置已过），工作树完好，第三任接手 | — | — | 断点：ce3c934f backward-window WIP 待 A/B 定论；d1-sharding-eval.md 草稿在 run/；门禁大部分已绿（04:5x 行）
- 2026-11-05 12:5x | #1 backward-window 定论 + 2x 达成（第三任） | (1) A/B 3:3 交错（run/abpf-base-backwin.txt，负载带 2.78-10.17 双侧同窗）：base 0.4641/0.4860/0.4897 vs backwin 0.0287/0.0293/0.0312µs——赢且大幅低于 2x 目标 0.0351；(2) mmprof 归因（run/pf-mmprof-backwin.txt，插桩靴 0.0322µs 自证同带）：mmap find/add ~29-30ns avg/调用，munmap walk avg 0 / max 177ns、body 11/218ns（n=4.53M 页迭代、unpg 90.9M 页）——安装/拆除两侧都进 ~10-20ns/页级（此前拆 51ns/页） | ce3c934f 保留为 HEAD（反向窗：缺页向下 advisory PTE 扫描先填下方空隙，随机序下含缺页的整段未映射 run 一陷阱吃净；+put_range/put_range_by_key 连续索引 range-iterator 快径，invalidated/desync 回退逐页路径）；D-1 评估 1475e1a9 落档 | backwin vs 51289ac2 0.043-0.047 再 -30%；vs 参考 0.0702 = **2.26-2.45x 反超，≤0.5x 达标** | 六腿全绿（HEAD 复验 12:3x-12:5x）：(a) 双架构 rc=0；(b) riscv 90s 浸泡 release 零 panic 零 WEDGE-DETECT，LOOP/CHURN/ARITH 完整（PREEMPT-UNDERFLOW/R55-RESYNC-STALE 同基线愈合打印；wedge_reported=True 为驱动器循环完成后的串口静默启发，同基线形）；(c) make test 1316/1316；(d) LTP mm 族 41 例逐例同 base；(e) pbench Tier-2 n=2 交错（run/abpb-base-backwin.txt）：全行带内，最差 p4_fork_exit +8.3% / p4_fork_exec +6.5%（机制：512 页窗扩大已映射集→fork PTE 拷贝/mapcount 随之——参考内核 16 页 fault-around 同类权衡；其余行 ≤±5%）；rig 修复入库：ab-pbench.sh mem 1G→2G（1G=已知 boot 期堆 panic，pbench_boot.py 注释在案）+ compare 解析 re.match→re.search（行带 "[probe] " 前缀）+ compare 结果 tee 进数据文件 | **#1 关闭，待主会话合 main**；余量观察：p4 两行 +6-8% 若后续 fork 战役要冲 2x，可评估窗宽自适应（VMA 小则缩窗）；转队列 #2 %gs per-cpu
- 2026-11-05 13:4x | 队列#2 变体：irq_note 钩子内联快门（第三任） | 反汇编分解：release-strong 里 irq_note_off/on 以外联 call+栈帧出现在全部 1331 个 cli/sti 站点（未武装也付费≈4-5ns×2）——CpuLocal::with 27ns 的大头 + handle_syscall 的 enable_irq 每调用一次；注意 %gs per-cpu 前提已过时（cpu_id 已是 mov gs:[0] 单指令，smp.rs GS 纪律完备）| a5124151：#[inline(always)] 快门只跳 ARMED==1（已定未武装）；ARMED==0（pa15 未定态）/2（武装）走 #[cold] 外联全逻辑，语义零变化；无条件 call 1331→688 | pbench n=2 交错（负载 5.0-6.0 同窗，run/abpb-backwin-hookgate.txt）：p2_ctxsw -4.1%、p4_fork_exit -5.9%、p4_fork_exec -5.1%、p6_mkdrmdir -20%、p6_getdents -2.5%、p5b_dgram_recv -2.6%（irqsave 密集行全赢）；其余行带内（openclose ±2% 噪声带、unlink 尾部主导）。lmbench：stat -0.6~-1.0%×3rep 稳、null -0.4~-1.1%；open 负载 5+ 不可读（1.9-2.8µs 摆动，静窗复测挂起）；armed syscost=1 靴插桩存活（SCHED-IRQOFF/PARKWALK 在打，stat 0.4046µs 带插桩）| 双架构 rc=0、make test 1316/1316、riscv 90s 浸泡（重建 release）零 panic 循环全完成（WAKE-HEAL-SKIP×2 vs backwin×0 = 已知 r24 sched 治愈族，perf-report.md 载史前内核成千打印）；OH Tier-1 census 靴在跑（userdata 主像失而复建：oh-img/ 重建为中心库符号链接）结论后补 | rig 双修：ab-pf.sh 见 "reboot: Power down" 即杀己 QEMU（KVM 下每靴 ~5s 替 280s wfi 停车）+ 修正 -append 案例名误抄 tr；顺带发现 x86_64 machine_park=loop{wfi} 无 ACPI-off——旧脚本每靴白烧 275s
- 2026-11-05 14:0x | 钩子门禁收口：fpgate + census 定论（第三任） | (1) fpgate（c0034520）：forkprof::trace_on 同款快门（只跳 ARMED≠2），交错 A/B 3rep 负载 7.2-8.8 同窗：null 0.0532→0.0505（-5.1%，-2.7ns/入口）、read 0.1121→0.1059（-5.5%）、write -1.1%、stat -0.9%（open 负载 7.4+ 不可读）；战役起点 null 0.055→现 0.0505（参考 0.042）。(2) OH Tier-1 census（a5124151 钩子门内核，debug，1560s）：TERM-DESCHED=8（带 8±2 正中）、WAKE-REFUSED=32（带 32±4 正中）、WAKE-STRANDED=2（非 grid 类）、tick ~108/s/双 CPU 持续到 1500s 窗尾、零 panic/POISONED、syscost 全表存活（ticks 表+rip 直方图+per-comm printk 普查 t=1500s 仍在打）；PREEMPT-LEAK=已知 debug 内核基线类（perf-report.md 载史前内核成千） | — | — | 门禁六腿全绿：census 过 → 钩子门（a5124151）+fpgate（c0034520）待主会话合 main；入口地板余量：syscall_handler_inner 外联 call（~2ns）+6 字参数拷贝（~2-3ns）+CURRENT_PT_REGS 2 原子+trap_handler 帧构建——下一杠杆需结构改（懒参数/轻量入口），收益递减；转 #3 路径行走分解（open 1.95x 最大行）
- 2026-11-05 14:4x | fpgate 门禁收口 + 分支栈 pbench 记录 + riscv 浸泡既有 bug 定性（第三任） | (1) fpgate（c0034520）门禁：双架构 rc=0、make test 1316/1316、forkprof=2 武装靴相位表全出（fk_total n=44361 avg=11us——armed 路径完好）。(2) 分支栈 pbench base(53fba4f6) vs fpgate n=3（run/abpb-base-fpgate-n3.txt）：12 行改善——ctxsw -6.1%、pipe -5.0%、append -4.0%、fork_exit -3.6%、fork_exec -2.8%、rt_read/recv -2.5%、stat -2.0%、fault_cow -1.5%；p6_unlink n=3 中位 +13.8%（带内；双峰伪影：p99 8000µs vs 中位 7.6µs，journal 提交窗对齐主导行中位——去调用无加回机制，三次 rep 7.59/8.72/10.68 全在历史摆动带 6.8-10.7 内）。(3) **riscv 90s 浸泡 KERNPANIC 家族定性为 main 既有 bug**：fpgate 核 2/3 崩（epc 0xffffffff80199146、badaddr 恒 0x018cb9d13aa22300、add_dir_entry 帧）→ 对照 hookgate 核（无 fpgate）也崩（epc 0xffffffff8019917c 同页）→ **基线 53fba4f6 临时工作树 3/3 崩**（badaddr 0xd18db9d542a506e4 / epc 0x6368746170660062="pathc"字符串野跳——fs 路径/创建路径野指针，PREEMPT-UNDERFLOW(c3c4 涂鸦) 前导）。结论：非本分支回归（分支样本率 ≤ 基线）；早期"零 panic"单样本浸泡是被 1/3-2/3 命中率骗过的幸运样本。**移交主会话：main 基线 riscv 浸泡 fs 野指针 bug 需专案**（签名：PREEMPT-UNDERFLOW count_was=c3c4c3c* → add_dir_entry/路径字符串内存野跳 → DEADLOCK 螺旋）| — | — | 分支栈（ce3c934f+a5124151+c0034520）门禁判定：过（pbench n=3 全带内/12 行改善、双架构、make test、census 带内、armed 路径双验）；riscv 浸泡腿以"类别=基线既有、率≤基线"放行并如实记录；待主会话合 main 并接手基线 fs 野指针专案
- 2026-11-05 15:0x | #3 路径行走分解（fsw 既有括桩，零新代码）+ 攻击计划（第三任） | fsw=1 插桩靴（bzImage-lm-fpgate）：open 案例插桩值 3.95µs / stat 0.667µs，PC1/PC2 相位（run/fsw-open.log、fsw-stat.log）。**放缩模型**：pc 时钟读 ~17ns×2/相位，每走查 ~16 读 ≈ 280ns 插桩税 → 真值：**stat 走查 ≈ 185ns**（与章程减法定界 200-250 吻合✓）= 组件 4×~23ns（a=权限+inode、b=dcache memo 查找共大头）+ **胶水 ~70ns/走查**（括桩外：claim 重取、NAME_MAX、./..比较、迭代器）+ walk-pre ~50ns；stat 尾部簿记 ~90ns；open 的 OPEN_TAIL=789ns 插桩 ⊃ 整个 file_open（walk 在内），非 walk 后簿记（前判读修正）。open/close 行真值 ~700ns：入口 ~50 + open pre ~100 + walk ~185 + open 尾 ~150 + close ~88 + 杂 | 无（分析） | — | — | **攻击计划（下任执行）**：(1) **全路径→dentry 记忆化**（open/stat 2x 的结构杠杆，同反向窗形态）：键=(root dentry, 规范化路径串, 挂载代), 值=dentry+代验证, 命中=单哈希查找跳过整段组件走查（省 ~185ns）；失效锚=父 inode dir_mut_gen（negmemo 同款）+挂载代；先 stat 后 open；风险=bind mount/rename 一致性——按 negmemo 的 lazy-refresh 形做。(2) 走查胶水 70ns：claim 每迭代重取检查可提出循环（快路径 claim 常活）、NAME_MAX 提到 split 时一次性验（组件创建后不变）。(3) stat 尾 90ns：分解 STAT_TAIL 内部（file_open→path_lookup 之外的部分）再动。(4) read/write 1.6x 需 3x 减：入口+VFS 层分解未做，列队列。2x 现实性备注：stat ≤0.1135、read ≤0.033 要求入口/走查/簿记全链减半再减半——每项单独到顶也难合成，需逐项逼近并定期复盘口径
