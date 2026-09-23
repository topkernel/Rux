# Rux 内核 Linux 对比与全文件检视报告（2026-09-23）

**检视目标**：全仓库 278 个源文件（含 3 个 .S 汇编）逐文件、逐函数、逐数据结构、逐行检视；以 Linux 为基准对比语义差异；重点考察 POSIX/ABI 兼容性（目标：musl 静态编译的 Linux 软件可直接运行）。

**评审约定**：每项发现的"初判"标注三类——
- `设计不一致`：有意简化/取舍，语义自洽（用户评审是否接受）
- `疑似 bug`：与 Linux 差异且引发错误行为
- `待评审`：无法单方面判定

**类别**：LINUX-DIFF（语义差异）| ABI（POSIX/ABI）| BUG | OVERFLOW | RACE | TIMING | VISIBILITY | COMMENT | LICENSE | ARCH

**统计**：（检视完成后填写）

## 分批进度
- [x] 批次 1：syscall 层（ABI 基准）
- [x] 批次 2：arch/riscv64（含 3 个 .S）
- [x] 批次 3：process（fork/exec/wait/signal）
- [x] 批次 4：mm
- [x] 批次 5：fs（vfs/ext4/pipe）
- [x] 批次 6：ipc + sync
- [x] 批次 7：net + drivers
- [x] 批次 8：sched + timer + interrupt + 其余
