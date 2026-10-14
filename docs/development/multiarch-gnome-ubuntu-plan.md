# Multi-arch GNOME Ubuntu Plan — x86_64 已通 / aarch64 路线

> Date: 2026-10-14
> Goal: GNOME Ubuntu 在 x86_64 与 aarch64 两个后端上达到 riscv64 的既有水位
> （GNOME Shell overview 点亮 + 2100s 稳定门禁）
> Baseline: riscv64 GNOME 已达成（FINDINGS.md：Xorg-fbdev + mutter + gnome-shell，
> bigalloc11 t=1082s 点亮、fire-r74fix-v1 2100s 零 panic）

## 1. 当前状态（本日实测）

### x86_64 — **Ubuntu 会话已跑通**

| 项 | 状态 | 证据 |
|---|---|---|
| 内核构建 | ✅ | `cargo build --target x86_64-unknown-none --features x86_64` + `build/mkbzimage.py` → bzImage 6.7MB |
| Ubuntu 22.04 amd64 rootfs | ✅ | `test/ubuntu-x86/build-img.sh` → `work/ubuntu-x86.img`（104MB，glibc 动态链 init 运行） |
| 会话 | ✅ | **`[udesk] login screen up`**（fork 修复后零 EFAULT/零 respawn，console+GUI 通道） |
| fork（glibc） | ✅ 已修复 | commit `5c32dad7`：write_child_settid 手写 SV39 三级 walk 在 x86 四级页表下全错 → 改走 arch 抽象（PageTableWalker + handle_cow_fault） |
| GNOME 镜像 | ✅ 已建成 | `work/gnome-x86.img`（3G，725 deb：Xorg-fbdev + dbus×2 + gnome-session/shell，`test/gnome-x86/build-img.sh` 免特权方案：apt download + dpkg-deb -x + mkfs -d） |
| GNOME 会话链 | 🔶 shell 存活，卡内核 scribbler | 全链已通：/bin/sh 补齐（apt 闭包不含 Essential 的 dash/bash——Xorg System() 即 xkbcomp 链）、Xorg 完整就绪（GLX/IGLX swrast）、mutter 以 X11 合成器运行、gnome-shell 存活并做 ext4 IO（dbus-launch 成功自起 session bus）。**当前唯一卡点：内核态 #UD at `signal::SigContext::zeroed+0xd7`——ELF 中该处为合法指令 `movq $0x0,0x88(%rdi)`，运行时被改写 → x86 版 text-scribbler**（riscv64 zeroed-text/R74 涂抹引擎同族；伴随 PTLEDGER DOUBLE-FREE REFUSED + FUT-STALE-TREE 诊断） |

已知限制（x86_64）：
- SMP 未上线（`single CPU (no APIC/MADT secondary)`），GNOME 门禁需补 APIC/IO-APIC/MADT 解析 + SMP 启动
- q35 3GB+ PCI hole 问题（run-x86-ub.sh 注释：MEM 固定 2G）
- PTLEDGER/FUT-STALE-TREE 诊断噪声（非致命）

### aarch64 — 骨架就位，全量移植未开始

- `kernel/src/arch/aarch64/` 模块树（12 模块 + mm 子树 9 模块），接口面参照 x86_64
- `pt_regs.rs` 为真实布局（31 GPR + sp/pc/pstate/esr/far，arm64 ABI 对齐 Linux）
- `arch/mod.rs` 已挂 `#[cfg(feature = "aarch64")]`（feature 在 Cargo.toml 已存在）
- **编译缺口基线（今日实测）：1354 个错误**。Top 连锁源：`arch::cpu::cpu_id`（43 处）、timer re-export（get_jiffies 109 处）——即少量顶层 re-export 缺失放大出绝大多数错误；真正需要新写的是后端本体
- 目标 profile：QEMU `virt` / Cortex-A72 / GICv3 / virtio-mmio / 4K granule 4 级页表

## 2. x86_64 GNOME 收尾清单（按序）

1. **[已解] Xorg xkbcomp 链**（2026-10-14）：根因是镜像缺 /bin/sh——apt 依赖闭包不含 Essential 集（dash/bash 未下载），Xorg 的 System("/bin/sh -c ...xkbcomp...") exec 失败 127。修复：PKGS 显式加 dash bash（+ 镜像内 glib-compile-schemas 预编译、machine-id、XDG_SESSION_TYPE=x11）。
2. **[当前卡点] x86 text-scribbler — 已完成最小复现与根因压缩（2026-10-15 取证轮）**：

   **复现器**：`test/gnome-x86/forkloop.c`（host gcc -static，200×fork+exec /bin/true + 100×posix_spawn）→ work/rootfs-x86/sbin/forkloop，`init=/sbin/forkloop`。结果：功能 0 失败、零 panic，但**每次 exec 的 teardown 都报** `PTLEDGER DOUBLE-FREE ppn=0x300 REFUSED` + `FUT-STALE-TREE lvl=0 pml4=0 lower=6 ppn=0x300`——与 GNOME 场景同签名（通用缺陷，非 GNOME 特有；GNOME 只是把写入频率推过了涂写阈值）。

   **取证链（STALE-DUMP 增强，已入树）**：
   - ppn 0x300 = phys 0x300000 = **内核 .text 起始页**（.text vaddr 0xffffffff80200000，VMA-LMA: phys = vaddr-base+0x100000）
   - 该"页表"的 leaf 内容 = `0x10ec8348e5894855` = **x86-64 函数序言机器码**（push rbp; mov rbp,rsp; sub rsp,0x10）——.text 原文
   - **parent PMD 槽值 = 同一段代码字节**——即 PID1 树 PMD[0] 表的 vpn2=6 槽被写入了 .text 内容，而非合法 PTE
   - 用户 VA 0xc00000(12MB)=PID1 静态 ELF 的 .data/.bss 区（demand-fill 热点）
   - 分配器陷阱（memblock<30MB、zone<floor）**零触发**：该帧从未经任何分配器发出

   **结论**：PID1 树低半区 PMD[0] 表帧与 .text 物理页发生别名（写 PMD 槽=涂写 .text 首页）。嫌疑收敛到两个函数的交叉：
   - `copy_kernel_mappings`（mm_ops.rs）低半区 leaf-only 克隆
   - `map_page_noflush`（mmu_init.rs）的 **2MB-leaf demotion 分支**（E8-MM）：对 PID1 树 PMD[0][6] 的身份 2MB leaf 首次 demand-fill 时展开为 512×4K——demotion 后的 PT 分配/链接与其后 fork 的 copy_page_table_cow 对该 PT 的再处理之间，存在把 .text 物理地址写入 PMD 槽的路径（pte2.ppn() 对被涂写槽位的解析会从代码字节产生任意 ppn，`get_page_table_virt(pte2.ppn()<<12)` 随即把"PT 写入"定向到任意物理页——自传播涂写引擎，与 riscv64 zeroed-text 同构）
   - 注意 `pte2.is_valid() && pte2.is_leaf()` 对垃圾槽值的判定天然不可靠——demotion 分支需要在读 pte2 前校验 ppn 的物理合法域

   **下一步（修复轮）**：① 在 map_page_noflush/copy_page_table_cow 的每级 pte 读取后加 ppn 物理域校验（< ram_top 且页对齐），非法即报错跳过而非继续展开；② 给 PID1 树 PMD[0][6] demotion 的 PT 帧打标验证是否被二次 demotion/copy 错误重写 PMD 槽；③ 修好后 GNOME x86 大概率直接点亮（会话链其余部分已全部验证）。
2. **SMP bring-up**：MADT 解析 + LAPIC init + INIT-SIPI-SIPI（x86_64/smp.rs 已有部分）——GNOME CPU-storm 修复（poll 事件唤醒等）是通用代码，SMP 上线即可继承
3. **q35 PCI hole**：3GB+ 内存下 BAR 落在 0xc0000000 未映射（x86 mm territory，run-x86-ub.sh 注释）
4. **2100s 门禁**（riscv64 的 fire-r74fix 同款：smp4、全链、零 KERNPANIC）
5. 参照 riscv64 的 paint-chain 修复清单逐项核对通用性：R75 buddy、R76 page-backed FileBuffer、fbdev bitfields（44c7c06f 通用）、D-Bus reply-loss 对（通用）、R82 EFAULT ordering（通用）

## 3. aarch64 bring-up 顺序（映射 x86_64 的移植路径）

估 8–12k 行新代码（x86_64 用了 11.4k）。顺序按依赖链：

| 阶段 | 内容 | 验收 |
|---|---|---|
| A1 | boot.S（QEMU virt @0x40080000 入口、EL2→EL1、关 MMU 装恒等表）+ linker.ld + boot.rs（设备树解析复用通用 cmdline.rs）+ 串口 PL011 | 串口打出 "Kernel starting..." |
| A2 | mm：4 级描述符/属性位、MAIR/TCR/TTBR、kernel 线性映射、vmemmap、get_page_table_virt、PT ledger | `mm: linear mapping [ok]`；`make test`（单核关中断模式）通过 |
| A3 | trap：VBAR_EL1 向量表 + 异常路由 + SVC 分发（x8/x0-x5 ABI）+ uaccess（PAN 开关） | mrsh 起来（smoke 15 项） |
| A4 | 进程：copy_thread（pstate=EL0）、context_switch（x19-28/sp/lr + FPSIMD 懒保存——注意 riscv64 的 ARCH-H1 FPU 只存不恢教训）、ret_from_fork | fork/exec/wait 全通；LTP 基础集 |
| A5 | 中断/定时：GICv3（dist/redist/CPU iface，drivers/intc/gicv3.rs——模块声明已存在！）+ generic timer（CNTFRQ/CNTP）+ SMP（PSCI CPU_ON）+ SGI IPI | 4 核在线、tick 稳定 |
| A6 | virtio-mmio（QEMU virt 默认无 PCI 也可用；PCI host 若走 -M virt,gic-version=3 + pcie 则另配） | vda 挂 ext4 rootfs |
| A7 | Ubuntu arm64 rootfs（ubuntu-base arm64 tarball + 同 build-img 流程，host 侧 qemu-aarch64-static 或直接 dpkg -x）+ udesk 验证 | `[udesk] login screen up` |
| A8 | GNOME arm64 镜像（同 §2 流程；mesa llvmpipe aarch64）+ 2100s 门禁 | GNOME Shell overview 点亮 |

关键映射表（x86_64 → aarch64）：
- multiboot/e820 → devicetree + FDT memory 节点
- IDT/TSS/GDT → VBAR_EL1 向量表
- CR3/invlpg → TTBR0_EL1/tlbi vmalle1（per-ASID：tlbi vae1is, asid）
- LAPIC/IOAPIC/MADT → GICv3 + PPI/SPI 路由
- SYSCALL MSR → SVC #0（无 fast-entry MSR 需求）
- PT_P/PT_U/PT_W → AttrIndx + AP 位 + PXN/UXN（软件 COW 位沿用 cow_flags 抽象）
- wp/post-boot SMP trampoline → PSCI CPU_ON（QEMU virt 默认 PSCI conduit）

## 4. 风险与依赖

1. **drivers 通用性**：virtio-pci 在 aarch64 走 PCIe host（-M virt + default）或 virtio-mmio——优先 virtio-mmio（QEMU virt 原生，无 PCI 枚举依赖）
2. **gicv3.rs 模块已被 drivers/intc/mod.rs 声明**（本日缺口报告的 `file not found for module gicv3`）——A5 时补文件即消
3. **FPU/FPSIMD**：GNOME 链（mutter/llvmpipe）重浮点——A4 的 FPSIMD 上下文切换必须真做（lazy trap 或 eager），不能重蹈 riscv64 ARCH-H1
4. **镜像构建**：本机为 amd64，arm64 rootfs 组装用 dpkg -x（无 chroot 执行）——与 x86 GNOME 镜像同方案，已验证可行
5. riscv64 的三份深度修复（trap 寄存器保存序、异常表、调度器 R7x 系列）全部在通用代码或已有 cfg 抽象——aarch64 直接继承，无需重修

## 5. 下一步（next session）

1. 等 GNOME x86 镜像出炉 → GUI 跑 paint 链，按 §2.1-2.5 收尾到 2100s 门禁
2. aarch64 A1-A2（boot + mm 线性映射 + 串口）——最小可见验收：`Kernel starting...` 打印
