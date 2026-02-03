//! 内核堆分配器 - 简单的bump分配器实现

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 堆的起始地址
const HEAP_START: usize = 0x8800_0000;
/// 堆的大小 (16MB)
const HEAP_SIZE: usize = 16 * 1024 * 1024;

/// 简单的bump分配器
pub struct BumpAllocator {
    heap_start: AtomicUsize,
    heap_end: AtomicUsize,
    heap_top: AtomicUsize,
    initialized: AtomicBool,
}

unsafe impl Send for BumpAllocator {}
unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }

        let size = align_up(layout.size(), layout.align());
        let align = layout.align();

        // 获取当前堆顶
        let mut current_top = self.heap_top.load(Ordering::Acquire);

        // 对齐
        let aligned_top = (current_top + align - 1) & !(align - 1);
        let new_top = aligned_top + size;

        // 检查是否超出堆范围
        let heap_end = self.heap_end.load(Ordering::Acquire);
        if new_top > heap_end {
            // OOM - 暂时保留调试输出
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"alloc: OOM\n";
                for &b in MSG {
                    putchar(b);
                }
            }
            return core::ptr::null_mut();
        }

        // 尝试原子更新堆顶
        loop {
            if self.heap_top.compare_exchange_weak(current_top, new_top, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return aligned_top as *mut u8;
            }

            current_top = self.heap_top.load(Ordering::Acquire);

            // 重新计算
            let aligned_top = (current_top + align - 1) & !(align - 1);
            let new_top = aligned_top + size;

            if new_top > heap_end {
                return core::ptr::null_mut();
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump分配器不支持释放
    }
}

impl BumpAllocator {
    pub const fn new() -> Self {
        Self {
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            heap_top: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn init(&self) {
        if !self.initialized.load(Ordering::Acquire) {
            self.heap_start.store(HEAP_START, Ordering::Release);
            self.heap_end.store(HEAP_START + HEAP_SIZE, Ordering::Release);
            self.heap_top.store(HEAP_START, Ordering::Release);
            self.initialized.store(true, Ordering::Release);
        }
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// 全局堆分配器（必须存在以满足编译器要求）
/// 我们同时提供手动导出的 mangled 符号来覆盖编译器生成的 hidden 符号
#[global_allocator]
pub static HEAP_ALLOCATOR: BumpAllocator = BumpAllocator::new();

pub fn init_heap() {
    if !HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) {
        HEAP_ALLOCATOR.heap_start.store(HEAP_START, Ordering::Release);
        HEAP_ALLOCATOR.heap_end.store(HEAP_START + HEAP_SIZE, Ordering::Release);
        HEAP_ALLOCATOR.heap_top.store(HEAP_START, Ordering::Release);
        HEAP_ALLOCATOR.initialized.store(true, Ordering::Release);
    }
}
