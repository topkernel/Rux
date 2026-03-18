# Rux 内存子系统重构经验总结

**日期**: 2026-03-18
**作者**: Claude + William
**相关提交**: `8b10fbc`, `3234d82`, `f2b0bcd`, `4c3543d`, `a2d0fed`, `0c2a206`

---

## 一、重构目标

将 Rux 内核的内存管理从"自己设计的方案"迁移到"完全按照 Linux 实现"，确保：
- 100% Linux ABI 兼容
- 正确的 Sv39 虚拟内存布局
- 动态内存映射（基于实际物理内存大小）

---

## 二、遇到的核心问题

### 问题 1: VMEMMAP_START 地址无效

**现象**:
```
trap: Kernel panic - page fault at 0xffffffb800000000
```

**原因分析**:
初始实现将 `VMEMMAP_SIZE` 设为 64GB，导致：
```
VMEMMAP_START = VMALLOC_START - 64GB
            = 0xffffffc800000000 - 0x1000000000
            = 0xffffffb800000000
```

检查这个地址的 bit 38：
```
0xffffffb800000000
bit 38 = 0  ← 这是用户空间地址！
```

**Sv39 规范**:
- 有效内核地址必须有 bit 38 = 1
- bit 38 = 0 的地址是用户空间地址

**解决方案**:
按照 Linux 公式计算 `VMEMMAP_SIZE`:
```c
// Linux: arch/riscv/include/asm/pgtable.h
#define VMEMMAP_SHIFT \
    (VA_BITS - PAGE_SHIFT - 1 + STRUCT_PAGE_MAX_SHIFT)
#define VMEMMAP_SIZE BIT(VMEMMAP_SHIFT)

// 对于 Sv39:
// VMEMMAP_SHIFT = 39 - 12 - 1 + 6 = 32
// VMEMMAP_SIZE = BIT(32) = 4GB
```

修正后：
```
VMEMMAP_START = 0xffffffc800000000 - 4GB
            = 0xffffffc700000000
bit 38 = 1  ← 有效的内核地址
```

---

### 问题 2: 页表访问使用了错误的虚拟地址

**现象**:
```
trap: Unknown exception: LoadAccessFault, badaddr=0xffffffd800350008
```

**原因分析**:
在 `alloc_page_table()` 函数中，动态分配页表后直接使用物理地址：
```rust
// 错误的代码
let phys_addr = frame.start_address().as_usize() as u64;
core::ptr::write_bytes(phys_addr as *mut u8, 0, PAGE_SIZE);
```

但此时 MMU 已启用，不能直接访问物理地址。

**解决方案**:
区分两种情况：
1. **早期启动**（帧分配器未就绪）：使用静态页表，恒等映射访问
2. **正常运行**（帧分配器就绪）：使用动态分配，通过 `phys_to_virt()` 访问

```rust
unsafe fn alloc_page_table() -> Option<u64> {
    if is_frame_allocator_ready() {
        // 动态分配
        let phys_addr = alloc_kernel_page()?;
        let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
        core::ptr::write_bytes(virt_addr.bits() as *mut u8, 0, PAGE_SIZE);
        Some(phys_addr)
    } else {
        // 静态分配，恒等映射
        let idx = KERNEL_PT_NEXT.fetch_add(1, ...);
        Some(&KERNEL_PAGE_TABLES[idx] as *const PageTable as u64)
    }
}

unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    if is_frame_allocator_ready() {
        phys_to_virt(PhysAddr::new(phys_addr)).bits() as *mut PageTable
    } else {
        phys_addr as *mut PageTable  // 恒等映射
    }
}
```

---

### 问题 3: 静态页表数量不足

**现象**:
映射 8192 个 vmemmap 页面时触发 panic。

**原因分析**:
- `MAX_KERNEL_PAGE_TABLES = 256`
- 每个 4KB 页面需要 L1 和 L0 两级页表
- 8192 页面可能需要 > 256 个页表

**解决方案**:
```rust
const MAX_KERNEL_PAGE_TABLES: usize = 4096;  // 16MB
```

---

### 问题 4: VirtAddr 符号扩展错误

**现象**:
```
PANIC! attempt to add with overflow
  Location: kernel/src/arch/riscv64/mm/base.rs:1745
```

**原因分析**:
原始实现使用 `VA_MASK` 截断地址：
```rust
// 错误的实现
pub const fn new(addr: u64) -> Self {
    Self(addr & VA_MASK)  // VA_MASK = 0x7FFFFFFFFF
}
```

这会破坏内核地址的高位（bit 63-39）。

**解决方案**:
正确的 Sv39 符号扩展：
```rust
pub const fn new(addr: u64) -> Self {
    let bit38 = (addr >> 38) & 1;
    if bit38 == 1 {
        // 内核地址：扩展 bit 38 到高位
        Self(addr | 0xFFFFFFC0_00000000)
    } else {
        // 用户地址：清除高位
        Self(addr & 0x0000007F_FFFFFFFF)
    }
}
```

---

### 问题 5: vmemmap 初始化时机问题

**现象**:
在 TLB 刷新前访问 vmemmap 区域导致 page fault。

**原因分析**:
```rust
// 错误的顺序
let val = core::ptr::read_volatile(test_ptr);  // 访问
core::arch::asm!("sfence.vma zero, zero");     // 刷新

// TLB 还没有新映射，访问失败！
```

**解决方案**:
确保 TLB 刷新在访问之前：
```rust
// 正确的顺序
core::arch::asm!("sfence.vma zero, zero");     // 先刷新
let val = core::ptr::read_volatile(test_ptr);  // 再访问
```

---

## 三、正确的 Sv39 内存布局

### 虚拟地址空间划分

```
Sv39 地址空间 (39位虚拟地址):

用户空间 (bit 38 = 0):
0x00000000_00000000 - 0x0000003F_FFFFFFFF  (256GB)

内核空间 (bit 38 = 1):
0xFFFFFFC0_00000000 - 0xFFFFFFFF_FFFFFFFF  (256GB)

内核空间细分:
┌─────────────────────────────────────────┐ 0xFFFFFFFF_FFFFFFFF
│                                         │
│              (未使用)                    │
│                                         │
├─────────────────────────────────────────┤ 0xFFFFFFD8_00000000
│         PAGE_OFFSET (线性映射)           │
│         phys_to_virt(phys)              │
├─────────────────────────────────────────┤ 0xFFFFFFD0_00000000
│         VMALLOC_END                      │
├─────────────────────────────────────────┤
│         VMALLOC 区域 (64GB)              │
├─────────────────────────────────────────┤ 0xFFFFFFC8_00000000
│         VMALLOC_START                    │
├─────────────────────────────────────────┤ 0xFFFFFFC8_00000000
│         VMEMMAP_END                      │
├─────────────────────────────────────────┤
│         VMEMMAP 区域 (4GB)               │
│         pfn_to_page(pfn)                │
├─────────────────────────────────────────┤ 0xFFFFFFC7_00000000
│         VMEMMAP_START                    │
├─────────────────────────────────────────┤
│              (其他区域)                   │
└─────────────────────────────────────────┘ 0xFFFFFFC0_00000000
```

### 关键常量定义

```rust
// 按照 Linux 定义
pub const PAGE_OFFSET: usize = 0xffffffd800000000;
pub const KERN_VIRT_SIZE: usize = 128 * 1024 * 1024 * 1024;  // 128GB
pub const VMALLOC_SIZE: usize = 64 * 1024 * 1024 * 1024;     // 64GB
pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;      // 4GB

// Linux 公式
pub const VMALLOC_END: usize = PAGE_OFFSET;
pub const VMALLOC_START: usize = PAGE_OFFSET - VMALLOC_SIZE;
pub const VMEMMAP_END: usize = VMALLOC_START;
pub const VMEMMAP_START: usize = VMALLOC_START - VMEMMAP_SIZE;
```

---

## 四、关键经验教训

### 1. 不要自己发明方案

❌ **错误做法**: 自己设计内存布局，随便选一个"看起来合理"的值
```rust
// 自己想的方案
pub const VMEMMAP_SIZE: usize = 64 * 1024 * 1024 * 1024;  // 为什么是64GB？因为"感觉够用"
```

✅ **正确做法**: 严格按照 Linux 公式计算
```rust
// Linux 公式
pub const VMEMMAP_SIZE: usize = 1 << (39 - 12 - 1 + 6);  // = 4GB
```

### 2. 理解硬件规范

Sv39 不仅仅是"39位地址空间"，还有约束：
- bit 38 决定是内核还是用户空间
- 必须进行正确的符号扩展
- 不符合规范的地址会导致 page fault

### 3. 注意启动阶段的地址转换

MMU 启用后，所有内存访问都必须使用虚拟地址：
- 早期（帧分配器未就绪）：恒等映射
- 后期（帧分配器就绪）：线性映射

### 4. TLB 一致性

修改页表后必须刷新 TLB：
```rust
// 添加新页表项后
core::arch::asm!("sfence.vma zero, zero");

// 访问新映射前必须刷新！
```

### 5. 调试技巧

1. **打印 VPN 索引**: 快速定位地址属于哪个页表
2. **检查 bit 38**: 验证是否为有效内核地址
3. **打印 ROOT_PAGE_TABLE 地址**: 确认页表基址正确

---

## 五、参考资料

1. **Linux 源码**:
   - `arch/riscv/include/asm/page.h` - PAGE_OFFSET 定义
   - `arch/riscv/include/asm/pgtable.h` - 虚拟内存布局
   - `arch/riscv/mm/init.c` - 内存初始化

2. **RISC-V 规范**:
   - RISC-V Privileged Architecture - Sv39 页表格式
   - bit 38 决定地址空间

3. **本项目文件**:
   - `kernel/src/arch/riscv64/mm/base.rs` - 核心内存管理
   - `kernel/src/mm/vmemmap.rs` - vmemmap 实现
   - `kernel/src/mm/page_desc.rs` - 页描述符

---

## 六、最终结果

重构完成后，内核可以：
- ✅ 正确建立线性映射（基于实际物理内存大小）
- ✅ 正确建立 vmemmap 映射
- ✅ 成功启动并加载 shell

```
mm:               linear mapping 2048 MB             [ok]
mm:               vmemmap mapping initialized        [ok]
...
init:             loading /bin/shell                 [ok]
```
