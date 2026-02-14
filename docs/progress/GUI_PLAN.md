# Rux OS 图形用户界面实现计划

## 目标

为 Rux OS 实现一个完整的图形用户界面环境，包括：
- 桌面启动器（Launcher）
- 计算器（Calculator）
- 时钟（Clock）
- 终端（Terminal）

## 当前状态

**❌ 无任何图形支持**
- 无 framebuffer 驱动
- 无输入设备驱动
- 无字体渲染
- 仅有 UART 文本控制台

## 实现阶段

### 阶段 1：基础图形输出（最小可行 GUI）

**目标**：能够在屏幕上绘制像素和文本

#### 1.1 VirtIO-GPU 驱动
- [ ] 实现 VirtIO-GPU 设备探测
- [ ] 初始化 VirtIO-GPU (2D mode)
- [ ] 获取 framebuffer 信息
- [ ] 实现 resource_create_2d
- [ ] 实现 set_scanout
- [ ] 实现资源刷新（transfer_to_host_2d）
- [ ] 创建 fb.rs 帧构

**文件结构**：
```
kernel/src/drivers/gpu/
  ├── mod.rs
  ├── virtio_gpu.rs   # VirtIO-GPU 驱动
  └── fb.rs           # Framebuffer 抽象
```

#### 1.2 基础绘图函数
- [ ] 像素绘制 (put_pixel)
- [ ] 填充矩形 (fill_rect)
- [ ] 复制矩形 (blit)
- [ ] 线条绘制 (draw_line)
- [ ] 圆形绘制 (draw_circle)

**文件**：`kernel/src/graphics/draw.rs`

#### 1.3 位图字体系统
- [ ] 8x8 ASCII 位图字体数据
- [ ] 字符渲染函数 (draw_char)
- [ ] 字符串渲染 (draw_string)
- [ ] 文本尺寸计算

**字体格式**：
```rust
const FONT_8x8: [u64; 128] = [
    0x0000000000000000, // NULL
    0x3C6660666E66663C, // 'A'
    // ... 其他字符
];
```

**文件**：`kernel/src/graphics/font.rs`

#### 1.4 双缓冲系统
- [ ] 后端缓冲区分配
- [ ] 页面翻转（swap_buffers）
- [ ] 垂直同步（可选）

**文件**：`kernel/src/graphics/double_buffer.rs`

### 阶段 2：输入设备支持

**目标**：支持键盘和鼠标输入

#### 2.1 PS/2 键盘驱动（RISC-V）
- [ ] PS/2 控制器初始化 (0x60/0x64 端口)
- [ ] 键盘扫描码读取
- [ ] 扫描码到 ASCII 转换表
- [ ] 中断处理（IRQ 1）
- [ ] Shift/Ctrl/Alt 修饰键
- [ ] 输入缓冲队列

**文件**：`kernel/src/drivers/keyboard/ps2.rs`

#### 2.2 PS/2 鼠标驱动（RISC-V）
- [ ] PS/2 鼠标初始化
- [ ] 鼠标数据包解析（3 字节）
- [ ] 中断处理（IRQ 12）
- [ ] 鼠标移动处理
- [ ] 按键处理（左/中/右）

**文件**：`kernel/src/drivers/mouse/ps2.rs`

#### 2.3 输入事件系统
- [ ] 事件队列
- [ ] 事件分发
- [ ] 回调机制

**文件**：`kernel/src/input/event.rs`

### 阶段 3：UI 框架

**目标**：实现基础窗口和控件系统

#### 3.1 窗口管理器
- [ ] 窗口结构体 (Window)
- [ ] 窗口列表管理
- [ ] 焦口层级（Z-order）
- [ ] 焦口装饰（标题栏、关闭按钮）
- [ ] 焦口移动和调整大小
- [ ] 焦口激活/去激活

**文件**：`kernel/src/gui/window.rs`

#### 3.2 基础控件
- [ ] Button（按钮）
- [ ] Label（标签）
- [ ] TextBox（文本框）
- [ ] Panel（面板）

**文件**：`kernel/src/gui/widgets/mod.rs`

#### 3.3 事件处理
- [ ] 鼠标事件到窗口映射
- [ ] 点击检测
- [ ] 焦口内控件事件路由
- [ ] 焦口管理器事件循环

**文件**：`kernel/src/gui/event.rs`

#### 3.4 布局系统
- [ ] 绝对布局
- [ ] 网格布局（可选）

**文件**：`kernel/src/gui/layout.rs`

### 阶段 4：桌面环境

**目标**：实现完整桌面和应用程序

#### 4.1 桌面环境
- [ ] 桌面背景渲染
- [ ] 任务栏（可选）
- [ ] 桌面图标管理

**文件**：`userspace/desktop/` 应用

#### 4.2 应用程序框架
- [ ] 应用程序基础结构
- [ ] 应用程序注册
- [ ] 应用程序生命周期

#### 4.3 具体应用实现

##### Launcher（启动器）
- [ ] 图标网格布局
- [ ] 应用程序列表
- [ ] 点击启动
- [ ] 图标渲染

##### Calculator（计算器）
- [ ] 按钮布局
- [ ] 表达式计算
- [ ] 数字显示
- [ ] 基础运算（+ - * /）

##### Clock（时钟）
- [ ] 数字时钟显示
- [ ] 日期显示
- [ ] 模拟时钟（可选）

##### Terminal（终端）
- [ ] 文本缓冲区
- [ ] 命令输入
- [ ] shell 集成
- [ ] ANSI 颜色支持（可选）

## 技术架构

```
┌─────────────────────────────────────┐
│         应用程序层                │
│  (Launcher, Calculator, etc.)     │
└──────────────┬──────────────────┘
               │
┌──────────────▼──────────────────┐
│         GUI 框架层               │
│  窗口管理器 | 控件 | 事件        │
└──────┬──────────────┬──────────┘
       │              │
┌──────▼──────────────▼──────────┐
│      图形和输入层               │
│  framebuffer | 字体 | 键盘/鼠标   │
└──────┬──────────────┬──────────┘
       │              │
┌──────▼──────────────▼──────────┐
│         驱动层                  │
│  VirtIO-GPU | PS/2 控制器       │
└─────────────────────────────────┘
```

## 内存布局

```
0xA0000000+ ┌─────────────────┐
            │ VirtIO-GPU MMIO │
            ├─────────────────┤
0xB0000000+ │ Framebuffer      │ (可配置大小)
            │ (16MB VRAM)     │
            └─────────────────┘
```

## QEMU 配置

运行带 VirtIO-GPU 的 QEMU：
```bash
qemu-system-riscv64 \
  -M virt \
  -cpu rv64 \
  -m 2G \
  -nographic \
  -device virtio-gpu-pci \
  -vga virtio \
  -bios none \
  -kernel rux
```

## 开发优先级

### P0（核心功能）
1. ✅ VirtIO-GPU 驱动
2. ✅ 基础像素绘制
3. ✅ 8x8 字体渲染
4. ✅ PS/2 键盘驱动

### P1（最小 GUI）
5. ✅ 窗口管理器
6. ✅ Button 控件
7. ✅ 基础事件处理
8. ✅ Launcher 应用

### P2（完整体验）
9. ⬜ Calculator 应用
10. ⬜ Terminal 应用
11. ⬜ Clock 应用
12. ⬜ PS/2 鼠标驱动

### P3（增强功能）
13. ⬜ 更多控件
14. ⬜ 双缓冲优化
15. ⬜ 桌面环境

## 预期工作量

- **阶段 1**（图形输出）：~2000 行代码
- **阶段 2**（输入设备）：~1500 行代码
- **阶段 3**（UI 框架）：~2500 行代码
- **阶段 4**（应用实现）：~2000 行代码

**总计**：~8000 行代码

## 参考资料

### VirtIO-GPU 规范
- [VirtIO GPU Device](https://docs.oasis-open.org/virtio/virtio/1.2/csprd01/virtio-v1.2-csprd01.html#x1-2800002)

### 字体数据
- [Standard 8x8 Font](https://github.com/dhepper/font8x8)
- [Terminus Font](https://terminus-font.sourceforge.net/)

### 图形编程
- [Writing a Simple Operating System](https://www.cs.bham.ac.uk/~exr/lectures/opsys/10_11/lectures/os-dev.pdf)

### PS/2 接口
- [PS/2 Keyboard/Mouse](https://wiki.osdev.org/PS/2_Keyboard)
- [OSDev Wiki](https://wiki.osdev.org/)

## 进度跟踪

- [x] 阶段 1.1：VirtIO-GPU 驱动规划
- [ ] 阶段 1.2：基础绘图函数
- [ ] 阶段 1.3：位图字体系统
- [ ] 阶段 1.4：双缓冲系统
- [ ] 阶段 2：输入设备支持
- [ ] 阶段 3：UI 框架
- [ ] 阶段 4：桌面环境

## 注意事项

1. **no_std 限制**：所有代码必须在不使用 std 的情况下工作
2. **内存管理**：注意堆分配，避免内存泄漏
3. **性能优化**：双缓冲可大幅提升性能
4. **中断安全**：中断处理程序中避免堆分配
5. **多核支持**：确保图形操作是 SMP 安全的
