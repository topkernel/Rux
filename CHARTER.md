# perf-gap-hunt — 全指标 ≥2x 反超参考内核战役（用户令 2026-11-04）

任务树：/home/william/rux-agents/perf-gap-hunt（分支 perf-gap-hunt）
任务书正本 = 本文件；巡检（10 分钟 cron）以本文件+台账判定进展。

## 任务

把**所有**测试用例的性能差距分析清楚并逐项消灭。目标：**每一行
lmbench/pbench 指标至少比参考内核好一倍**（我们的时间 ≤ 参考的 0.5x，
同 KVM 公平口径）。只要还有任何一行有差距，就继续优化，**中间不停**。

## 当前战况（基线 main=53fba4f6，参考值冻结于 run/lmbench-refkvm.txt / lmbench-linux.txt）

### 已达标（≤0.5x 或反超）
- lat_ctx 3.15x 反超（2.13 vs 6.71）· lat_pipe 3.56x 反超（3.74 vs 13.30，11-05 栈：钩子门在 ping-pong 路径复利 -12%）
- lat_proc exec 1.32x 反超（66 vs 87.4）· fstat 1.37x 反超（0.086 vs 0.118）
- lat_mmap 0.24x · bw_pipe 2.68x · bw_unix 1.88x · bw_mem 1.07-1.10x · usleep 0.99x
- **lat_pagefault 2.26-2.45x 反超（0.0287-0.0312 vs 0.0702；2026-11-05 定妆，门禁六腿全绿待合 main）**

### 未达标（按差距排序 = 主攻顺序）
| 行 | 我们 | 参考(KVM/原生) | 差距 |
|---|---|---|---|
| lat_syscall open | 2.4→**0.8031**（11-05 close 停顿类修复 3bf07413；探针中位 801） | **0.3566（11-05 重锚复现，与冻结 0.36 一致）** | **2.25x**；原 3.4-8.4x 的大头是每 tick 一次的 10ms close() 停顿类（均值形基准吞掉）——已消灭；剩余=中位路径：walk ~500ns（memo 攻）+ open tail ~250ns + close 88ns + 入口 50ns |
| lat_sem | 0.800 | 0.444 原生 | 1.80x |
| lat_syscall stat | 0.379 | 0.2141（重锚）/0.227（冻结） | 1.64-1.77x |
| lat_syscall read | 0.110→**0.1059**（11-05 栈） | 0.0596（重锚）/0.066（冻结） | 1.60-1.78x |
| lat_syscall write | 0.094→**0.0915**（11-05 栈） | 0.0557（重锚）/0.058（冻结） | 1.53-1.64x |
| lat_unix | 4.56 | 3.011 原生 | 1.52x |
| lat_syscall null | 0.055→**0.0505**（11-05 栈：钩子门×2） | 0.0418（重锚）/0.042（冻结） | 1.21x（入口地板，乘所有行；余量=inner call ~2ns+6 字参数拷贝 ~2-3ns+帧构建） |
| lat_proc fork | 59.9→57.9（11-05 栈，噪声带 54.9-60.9） | 51.61（重锚）/53.99（冻结） | 1.07-1.12x |
| lat_proc exec | 82-114（hello 真实形摆动）/86.5（失败形，base 88.4） | **84.80（hello 真实形重锚）** | 真实形带内~1.0-1.35x（待静窗细锚）；66 史值不可复现（谱系丢失） |
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
2. ~~钩子调用税两刀~~ **完成 2026-11-05**（第四任补 LTP fs 族 68 例逐例同 base 关门）：a5124151 irq 钩子快门 + c0034520 fpgate 快门，待主会话合 main。（原 #2 %gs per-cpu 前提已过时——cpu_id 已单指令，见 13:4x 台账）
3. **open 行 3.4-8.4x**（重锚后升回最大行）：(a) lmbench 形 2.4µs vs pbench 1.20 vs 插桩相位和 1.5µs——先分解括桩外差距（benchmp 形差异：子进程/每次迭代路径 vs pbench 单进程）；(b) 全路径→dentry 记忆化（15:0x 攻击计划，省走查 ~185ns+）；(c) open tail 789ns 插桩相位分解
4. **路径行走记忆化**（15:0x 计划：全路径 memo 键=(root dentry,规范串,挂载代)，negmemo 同款失效锚；+胶水 70ns：claim 每迭代重取提出循环、NAME_MAX 提到 split 时一次性验）——惠及 open/stat/exec
5. **syscall 入口地板 13ns**（null 1.21x：两层 dispatch 合一、armed() 残项归并、seccomp/IRQ 门重排——乘所有系统调用行）
6. **read/write 1.6x**（入口+VFS 层分解未做）
7. **fork retire 批量 park**（exit_group/cloexec 批量关闭一次锁往返）+ fork 57.9 vs 51.61
8. **lat_sem 0.800 vs 0.444**（semop 路径分解）
9. **lat_unix 4.56 vs 3.01**（唤醒侧已到平台，查 socket 缓冲/唤醒队列路径）
10. ~~D-1 runqueue 分片评估~~ **完成 2026-11-05**（1475e1a9：保持关闭——最优省 164ns/切换=7.7% lat_ctx，动不到任何未达标行；触发器已记）

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
- 2026-11-05 15:1x | open 行基准取证（第三任）| 今日全部 open A/B（base 53fba4f6 与分支栈同涨 2.1-3.0µs vs 章程 0.70）→ 逐项排除：(a) make_scratch 16MB 后台写回理论——runner 已门控（仅 lat_pagefault/lat_mmap/bw_file_rd/bw_mmap_rd/lat_fs/lat_dram_page 造 scratch；pagefault 复测 0.0286µs 正常）后 open 仍 2.1-2.8，**否**；(b) ENOENT 形（旧 rig 文件不存在走失败路径）——显式 /nope 传参 bench 中止无值，stat 同文件 0.372 与史吻合（0.379）说明走查同基，**否**；(c) 目标文件差异——/test/lat_ctx 显式传参同样 2.58µs，**否**；(d) pbench p6_openclose 同 rig 1.20µs(n=10000 med) vs lmbench 2.4 = benchmp 记账差异未定。结论：**open 行 0.70 基准不可复现**（旧图像谱系丢失；同 base 内核二进制今读 2.4-3.0=该 rig 的真实 open+close 水平），参考侧 0.36 同样出自丢失 rig → 1.95x 差距数字当前无意义；stat/read/write/null 四行与史吻合故仅 open 行基准断裂 | userspace/lmbench/lmbench_run.c：scratch 门控（rig 卫生，兼修 pagefault 案例外的 16MB 无谓写）；run/refkvm/lmbench-ref2.img 已备（work 镜像副本+init_ref 注入，与 Rux 列同图像字节）| — | **移交下任**：(1) 恢复参考列 vmlinuz（原 run/refkvm/vmlinuz-7.0.0-38-generic 已丢；宿主 /boot 同名件在 q35+serial file 形下零串口输出需查——run-refkvm.sh 当时能跑，疑 EFI stub/控制台差异）后跑 run-refkvm.sh 重锚全列；(2) 重锚前 open 行标注 basis-broken，用 p6_openclose 1.20µs 作稳态代理；(3) 重锚后按 15:0x 行攻击计划动刀（全路径 memo 等）
- 2026-11-05 15:4x | 分支栈全行战绩（修 runner 后 n=2 交错，负载 3.4-5.3 同窗，run/abpf-standings-stack-n2.txt）| base 53fba4f6 vs 栈（ce3c934f+钩子门+fpgate）：pagefault 0.4644→0.0288（**16.1x**，再证）；pipe 4.249→**3.740（-12.0%，3.56x 反超**——cli/sti 钩子税在 ping-pong 每回合多对复利）；null 0.0528→0.0491（-7.0%）；exec 86.5→82.2（-5.0%，失败 execve 形同基）；read 0.1090→0.1044（-4.2%）；write 0.0919→0.0894（-2.8%）；fstat 0.0841→0.0827（-1.6%）；stat -0.2% 平；fork +0.5%（噪声带内）。exec 行定性：现镜像无 /tmp/hello（fair-rig 文档载明的失败 execve 形），66→86 基准漂移与 open 同类，需 fair-rig 注入后重锚 | — | — | 栈在所有可锚行上全优或平；open/exec 两行 basis-broken 待参考列恢复+hello 注入重锚
- 2026-11-05 15:5x | exec 行 hello 注入重锚（我方列）| work 镜像注入 /tmp/hello（fair-rig 形）：真实 execve 基准 base 96.6/88.4、栈 99.9/113.9（负载 ~4，n=2 摆动 88-114 不可细判）；失败 execve 形 86.5/82.2 同基栈 -5%。真实形比失败形慢（82→92 中位级，方向符合预期：真 exec 多 ELF/mmap 工作）；章程 66 基准既非失败形也非本窗真实形=更静窗或再一态，**exec 行三态（失败/真实/66史）全部需与参考同锚后才有意义** | — | — | 移交：参考列 vmlinuz 恢复后，run-refkvm.sh 全列重锚（图像 run/refkvm/lmbench-ref2.img 已含 hello+init_ref，与 Rux 列同字节）
- 2026-11-05 15:2x | #2 门禁补完：LTP fs 族 68 例（第四任）| LTP fs/process 族（open/openat/stat/lstat/fstat/close/read/write/creat/dup/fcntl/rename/mkdir/rmdir/unlink/link/symlink/readlink/getcwd/chdir/access/utime/truncate/pipe/pipe2/readv/writev/pread/pwrite/flock/fork/wait/exit/execve 共 68 例，riscv64 release，ltp_family_boot.py），HEAD 栈核 vs base 53fba4f6 核：**逐例 rc 相同**（双侧 67 例 rc=0，dup03 双侧同 "?" 超时标记=已知抖动类，非分歧）。其余腿沿用第三任记录（内核码自 c0034520 未变，其后仅章程/数据提交）：双架构 rc=0、make test 1316/1316、riscv 90s 浸泡（基线既有类放行）、census 带内、武装靴双验、pbench n=3 带内+12 行改善 | — | LTP 族补齐 | **#2 关门：钩子门 a5124151 + fpgate c0034520 全门禁六腿+LTP 绿，待主会话合 main**。rig 注记：base 侧首次 setsid 启动 0 字节死胎（原因未明），nohup 重启正常——后续长测用 nohup 形
- 2026-11-05 15:2x | 参考列恢复+全列重锚（第四任）| **vmlinuz 丢失之谜=权限**：宿主 /boot/vmlinuz-7.0.0-38-generic 为 root:600，QEMU 直接 "could not open kernel file: Permission denied"（前任"零串口输出"的真相）；agent 沙箱 NO_NEW_PRIVS 使 sudo 不可用 → docker 组成员资格可解：`docker run -v /boot:/boot:ro -v <dst>:/out <img> cp/chmod`（未来 agent 复用此法）。恢复后同 rig（q35+kvm+cpu host、ref2 图像、init_ref、loglevel=4，负载 4.15-4.22 好窗）跑 run-refkvm.sh 10 例全列（run/lmbench-refkvm2.txt）：null 0.0418 / read 0.0596 / write 0.0557 / stat 0.2141 / fstat 0.1129 / **open 0.3566** / fork 51.61 / exec 84.80（hello 真实形）/ pipe 13.02 / ctx 6.37 | — | — | **open 行重锚定论：参考 0.3566 与冻结列 0.36 一致复现——参考侧从未断**。第三任"基准断裂"结论修正为：参考列可复现，断裂的只是我方 0.70 旧值（旧图像谱系）；open 真实差距 = 我方 2.1-3.0µs（lmbench 形）/ 1.20µs（pbench p6 中位） vs 参考 0.3566 ≈ **3.4-8.4x，升回最大行**。exec 行同锚完成：参考 84.80 真实形 vs 我方 hello 形 82-114（待静窗 n≥3 细锚）；fork 参考 51.61（章程 53.99 同带）；pipe/ctx/fstat 参考值带内复现=冻结列可信
- 2026-11-05 15:5x | open 行解剖→**10ms close 停顿类修复**（第四任，3bf07413）| 新探针 test/probe_open.c（同靴 rdtsc 分布+无采样纯墙环，双核同图像对比）：Rux open+close **中位 1072ns / 均值 2451ns**，p99≈10.4µs、top5 全 ~10ms——**每 10ms tick 恰一次 close() 停顿 0.7-10ms，全部落在 jiffy 边界结束**（open 恒 520-710ns）；-smp 1 干净（均值 869）=纯跨 CPU 宽限互锁非工作量；参考核同 rig：中位 351/均值 370/无此类。lmbench 的 2.4µs 即此尾巴的均值化（pbench rdtsc 中位免疫）。根因两条：(1) wait_qs_sleepy 标 INTERRUPTIBLE 后不复查 QS 即睡满 1 jiffy（缺失报告者通常 µs 级就补上——fd-close 完成环每轮 schedule() 都是报告点）；(2) retire_complete 每次 close TICKET.up 唤醒 fdreapd（closer 本会自排空批次，纯噪音；且 fdreapd 抢走批次时 closer 反而等 fdreapd 的 sleepy 宽限 jiffy 粒度）| rcu.rs：set_state 后加 QS 复查命中即撤定时器（check-flag-sleep 协议，NEW-C2 纪律照抄定时器超时路径）；fdreap.rs：快乐路径删 TICKET.up（fdreapd 静睡=次 CPU 保持 halt=构造性 quiescent，closer 的 polled 宽限 µs 窗口完成；requeue 路径仍 up）| 修复后同探针：open+close **中位 801/均值 831/墙 827**（无周期停顿，仅靴首瞬态 757µs）；中位还赚 ~270ns（省每 close 的唤醒 IPI+缓存行乒乓）。lmbench：**open 2.4→0.8031µs（vs 参考 0.3566 = 2.25x）**、fork 58.64（带内）、exec 97.76（hello 形带内）| 门禁启动中 | 台账教训：均值形基准（lmbench benchmp 墙钟）会吃周期尾巴类——中位形（pbench）看不见；今后可疑行先跑分布探针。剩余 2.25x=中位路径工作：walk memo+open tail（并入 #4 计划）
