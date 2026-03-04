# Rux OS 图形用户界面实现计划

**最后更新**：2026-03-04
**状态**：✅ 已实现

---

## 当前状态

### ✅ 已完成功能

**图形驱动**
- ✅ VirtIO-GPU 驱动（QEMU virtio-gpu-pci）
- ✅ Framebuffer 抽象层
- ✅ 基础绘图函数（像素、矩形、线条）
- ✅ 8x16 位图字体渲染
- ✅ 双缓冲机制

**输入设备**
- ✅ VirtIO-Input 驱动
- ✅ evdev 事件设备接口
- ✅ 键盘和鼠标事件处理
- ✅ devfs 集成（/dev/input/event0）

**GUI 库 (rux_gui)**
- ✅ Window 窗口管理
- ✅ Widget 控件系统
  - ✅ Button 按钮
  - ✅ Label 标签
  - ✅ TextBox 文本框
  - ✅ Panel 面板
- ✅ 事件处理系统
- ✅ 布局管理

**应用程序**
- ✅ Desktop 桌面环境
- ✅ Calculator 计算器
- ✅ Clock 时钟
- ✅ VShell 可视化 Shell

---

## 技术架构

```
┌─────────────────────────────────────┐
│         应用程序层                  │
│  (Desktop, Calculator, Clock, etc.) │
└──────────────┬──────────────────────┘
               │
┌──────────────▼──────────────────────┐
│         rux_gui 库                   │
│  窗口管理器 | 控件 | 事件 | 布局     │
└──────┬──────────────┬────────────────┘
       │              │
┌──────▼──────────────▼────────────────┐
│      图形和输入层                     │
│  framebuffer | 字体 | evdev          │
└──────┬──────────────┬────────────────┘
       │              │
┌──────▼──────────────▼────────────────┐
│         驱动层                        │
│  VirtIO-GPU | VirtIO-Input | devfs   │
└───────────────────────────────────────┘
```

---

## 文件结构

```
kernel/src/drivers/
├── gpu/
│   ├── mod.rs           # GPU 驱动导出
│   ├── framebuffer.rs   # 帧缓冲核心
│   ├── fb_simple.rs     # 简单帧缓冲驱动
│   ├── fbdev.rs         # fbdev 设备接口
│   ├── virtio_gpu.rs    # VirtIO-GPU 驱动
│   └── virtio_cmd.rs    # GPU 命令处理
└── input/
    ├── mod.rs           # 输入设备导出
    ├── evdev.rs         # evdev 字符设备
    ├── event.rs         # 输入事件定义
    └── virtio_input.rs  # VirtIO-Input 驱动

userspace/libs/gui/
├── src/
│   ├── lib.rs           # GUI 库入口
│   ├── widget.rs        # 控件定义
│   ├── window.rs        # 窗口管理
│   ├── input.rs         # 输入处理
│   └── font.rs          # 字体数据
└── Cargo.toml

userspace/apps/
├── desktop/             # 桌面环境
├── calculator/          # 计算器
├── clock/               # 时钟
└── vshell/              # 可视化 Shell
```

---

## 运行 GUI

```bash
# 构建
make build && make user && make rootfs

# 运行 GUI
make gui

# 或手动运行
./test/run.sh gui

# 在 shell 中启动桌面
/app/desktop
```

---

## 未来改进

### 短期
- [ ] 更多字体支持
- [ ] 窗口拖动和调整大小
- [ ] 更丰富的控件（滑块、复选框等）

### 中期
- [ ] 硬件加速图形
- [ ] 多窗口 Z-order 管理
- [ ] 任务栏和系统托盘

### 长期
- [ ] 3D 图形支持
- [ ] Wayland 协议支持
- [ ] 桌面主题系统

---

## 参考资料

### VirtIO 规范
- [VirtIO GPU Device](https://docs.oasis-open.org/virtio/virtio/1.2/csprd01/virtio-v1.2-csprd01.html#x1-2800002)
- [VirtIO Input Device](https://docs.oasis-open.org/virtio/virtio/1.2/csprd01/virtio-v1.2-csprd01.html#x1-2900002)

### Linux 接口
- [evdev subsystem](https://www.kernel.org/doc/html/latest/input/input.html)
- [framebuffer API](https://www.kernel.org/doc/html/latest/fb/framebuffer.html)

---

**文档版本**：v2.0.0
**最后更新**：2026-03-04
