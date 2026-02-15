//! Rux 桌面环境
//!
//! 用户态桌面环境应用

#![no_std]
#![no_main]

extern crate alloc;

use rux_gui::{
    FramebufferDevice, FontRenderer, DoubleBuffer, MouseCursor,
    WindowManager, SimplePanel, color,
};
use core::ptr::null_mut;
use alloc::alloc::Layout;

/// 简单的 bump allocator
struct BumpAllocator {
    heap_start: usize,
    heap_size: usize,
    next: usize,
}

impl BumpAllocator {
    const fn new() -> Self {
        Self {
            heap_start: 0,
            heap_size: 0,
            next: 0,
        }
    }

    unsafe fn init(&mut self, start: usize, size: usize) {
        self.heap_start = start;
        self.heap_size = size;
        self.next = start;
    }
}

unsafe impl alloc::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();

        let aligned_next = (self.next + align - 1) & !(align - 1);
        let new_next = aligned_next + size;

        if new_next > self.heap_start + self.heap_size {
            return null_mut();
        }

        (&self.next as *const usize as *mut usize).write_volatile(new_next);
        aligned_next as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // 简单 bump allocator 不支持释放
    }
}

#[global_allocator]
static mut ALLOCATOR: BumpAllocator = BumpAllocator::new();

/// 堆大小 (16MB)
const HEAP_SIZE: usize = 16 * 1024 * 1024;

/// 堆起始地址
const HEAP_START: usize = 0x40000000;

/// 桌面环境
struct Desktop {
    fb: FramebufferDevice,
    double_buffer: DoubleBuffer,
    font: FontRenderer,
    cursor: MouseCursor,
    wm: WindowManager,
    launcher_panel: SimplePanel,
    clock_panel: SimplePanel,
    running: bool,
}

impl Desktop {
    fn new() -> Self {
        // 打开 framebuffer 设备 (使用 ioctl + mmap)
        let fb = FramebufferDevice::open()
            .expect("Failed to open framebuffer device");

        // 获取屏幕尺寸
        let screen_width = fb.width();
        let screen_height = fb.height();

        // 初始化双缓冲
        let mut double_buffer = DoubleBuffer::new();
        double_buffer.init(screen_width, screen_height, screen_width);

        // 初始化字体
        let font = FontRenderer::new_8x8();

        // 初始化光标
        let cursor = MouseCursor::new(screen_width, screen_height);

        // 初始化窗口管理器
        let mut wm = WindowManager::new();
        wm.create_window("Launcher", 10, 10, 200, 300);
        wm.create_window("Clock", 220, 10, 200, 100);

        // 创建启动器面板
        let mut launcher_panel = SimplePanel::new(10, 40, 180, 260);
        launcher_panel.add_label(10, 10, "Applications:");
        launcher_panel.add_button(10, 40, 160, 30, "Calculator");
        launcher_panel.add_button(10, 80, 160, 30, "Terminal");
        launcher_panel.add_button(10, 120, 160, 30, "File Manager");

        // 创建时钟面板
        let mut clock_panel = SimplePanel::new(220, 40, 180, 60);
        clock_panel.add_label(20, 10, "00:00:00");
        clock_panel.add_label(20, 30, "2026-02-15");

        Self {
            fb,
            double_buffer,
            font,
            cursor,
            wm,
            launcher_panel,
            clock_panel,
            running: true,
        }
    }

    fn run(&mut self) {
        while self.running {
            // 处理输入事件（需要系统调用支持）
            // self.handle_events();

            // 绘制
            self.draw();

            // 刷新屏幕
            self.double_buffer.swap_buffers(&self.fb);

            // 延迟
            for _ in 0..10000 {
                unsafe { core::arch::asm!("nop") };
            }
        }
    }

    fn draw(&self) {
        // 清空背景
        self.double_buffer.clear(color::BLUE);

        // 绘制任务栏
        let taskbar_height = 30u32;
        let screen_width = self.fb.width();
        let screen_height = self.fb.height();

        self.double_buffer.fill_rect(
            0,
            screen_height - taskbar_height,
            screen_width,
            taskbar_height,
            0xFF303030,
        );
        self.font.draw_string(
            &self.double_buffer,
            10,
            screen_height - taskbar_height + 10,
            "Rux OS Desktop",
            color::WHITE,
        );

        // 绘制窗口
        self.wm.draw_all(&self.double_buffer, &self.font);

        // 绘制面板
        self.launcher_panel.draw(&self.double_buffer, &self.font);
        self.clock_panel.draw(&self.double_buffer, &self.font);

        // 绘制光标
        self.cursor.draw(&self.double_buffer);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 初始化堆分配器
    unsafe {
        ALLOCATOR.init(HEAP_START, HEAP_SIZE);
    }

    let mut desktop = Desktop::new();
    desktop.run();

    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
