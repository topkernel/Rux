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

### 未达标（按差距排序 = 主攻顺序）
| 行 | 我们 | 参考(KVM/原生) | 差距 |
|---|---|---|---|
| lat_pagefault | 0.456(11-04复测) | 0.0702 KVM | 6.50x **进行中** |
| lat_syscall open | 0.70 | 0.36 | 1.95x（目标 ≤0.18） |
| lat_sem | 0.800 | 0.444 原生 | 1.80x |
| lat_syscall stat | 0.379 | 0.227 | 1.68x |
| lat_syscall read | 0.110 | 0.066 | 1.67x |
| lat_syscall write | 0.094 | 0.058 | 1.62x |
| lat_unix | 4.56 | 3.011 原生 | 1.52x |
| lat_syscall null | 0.055 | 0.042 | 1.31x（入口地板，乘所有行） |
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

1. **lat_pagefault 复测**（陈值 6.45x；若仍 >1x 分解：trap 入口+VMA 查找
   +页分配(PCP)+PTE 写+TLB；dfx=memwatch / trace 框架 kernel/src/trace/）
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
8. **D-1 runqueue 分片评估**（lock=102ns+唤醒排队——先写评估文档，不动手）

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
