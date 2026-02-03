//! 内核堆分配器
//!
//! 简单的链表分配器实现

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

const HEAP_START: usize = 0x8800_0000;
const HEAP_SIZE: usize = 16 * 1024 * 1024; // 16MB

/// 堆块头
#[repr(C)]
struct BlockHeader {
    size: usize,
    used: bool,
    next: Option<NonNull<BlockHeader>>,
    prev: Option<NonNull<BlockHeader>>,
}

/// 堆分配器
pub struct HeapAllocator {
    start: usize,
    size: usize,
    initialized: AtomicBool,
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"alloc: start\n";
            for &b in MSG {
                putchar(b);
            }
        }

        if !self.initialized.load(Ordering::Acquire) {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"alloc: calling init\n";
                for &b in MSG {
                    putchar(b);
                }
            }
            self.init();
        }

        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"alloc: after init check\n";
            for &b in MSG {
                putchar(b);
            }
        }

        let size = align_up(layout.size(), 8);

        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"alloc: calling find_free_block\n";
            for &b in MSG {
                putchar(b);
            }
        }

        let block = self.find_free_block(size);

        if let Some(mut block) = block {
            // 检查是否需要分割块
            let block_size = unsafe { block.as_ref().size };
            if block_size >= size + size_of::<BlockHeader>() + 8 {
                self.split_block(block.as_ptr(), size);
            }

            unsafe {
                block.as_mut().used = true;
                block.as_ref().data_addr()
            }
        } else {
            core::ptr::null_mut()
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let header = BlockHeader::from_data_ptr(ptr);
        (*header).used = false;
        self.coalesce(header);
    }
}

impl HeapAllocator {
    pub const fn new(start: usize, size: usize) -> Self {
        Self {
            start,
            size,
            initialized: AtomicBool::new(false),
        }
    }

    fn init(&self) {
        if self.initialized.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            unsafe {
                let first_block = self.start as *mut BlockHeader;
                (*first_block).size = self.size - size_of::<BlockHeader>();
                (*first_block).used = false;
                (*first_block).next = None;
                (*first_block).prev = None;
            }
        }
    }

    unsafe fn find_free_block(&self, size: usize) -> Option<NonNull<BlockHeader>> {
        let mut block = self.start as *mut BlockHeader;

        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"find_free_block: start\n";
            for &b in MSG {
                putchar(b);
            }
        }

        while !block.is_null() {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b".";
                for &b in MSG {
                    putchar(b);
                }
            }

            if !(*block).used && (*block).size >= size {
                unsafe {
                    use crate::console::putchar;
                    const MSG2: &[u8] = b"found\n";
                    for &b in MSG2 {
                        putchar(b);
                    }
                }
                return Some(NonNull::new(block)?);
            }
            block = (*block).next.map(|p| p.as_ptr()).unwrap_or(core::ptr::null_mut());
        }

        unsafe {
            use crate::console::putchar;
            const MSG3: &[u8] = b"not found\n";
            for &b in MSG3 {
                putchar(b);
            }
        }
        None
    }

    unsafe fn split_block(&self, block: *mut BlockHeader, size: usize) {
        let old_size = (*block).size;
        let new_block_size = old_size - size - size_of::<BlockHeader>();

        (*block).size = size;

        let new_block = (block as usize + size_of::<BlockHeader>() + size) as *mut BlockHeader;
        (*new_block).size = new_block_size;
        (*new_block).used = false;
        (*new_block).prev = Some(NonNull::new(block).unwrap());
        (*new_block).next = (*block).next;

        (*block).next = Some(NonNull::new(new_block).unwrap());

        if let Some(next) = (*new_block).next {
            (*next.as_ptr()).prev = Some(NonNull::new(new_block).unwrap());
        }
    }

    unsafe fn coalesce(&self, block: *mut BlockHeader) {
        // 与下一个块合并
        if let Some(next) = (*block).next {
            if !(*next.as_ref()).used {
                let next_size = (*next.as_ref()).size + size_of::<BlockHeader>();
                (*block).size += next_size;
                (*block).next = (*next.as_ref()).next;

                if let Some(next_next) = (*block).next {
                    (*next_next.as_ptr()).prev = Some(NonNull::new(block).unwrap());
                }
            }
        }

        // 与前一个块合并
        if let Some(prev) = (*block).prev {
            if !(*prev.as_ref()).used {
                let block_size = (*block).size + size_of::<BlockHeader>();
                (*prev.as_ptr()).size += block_size;
                (*prev.as_ptr()).next = (*block).next;

                if let Some(next) = (*block).next {
                    (*next.as_ptr()).prev = Some(prev);
                }
            }
        }
    }
}

impl BlockHeader {
    unsafe fn data_addr(&self) -> *mut u8 {
        (self as *const BlockHeader as usize + size_of::<BlockHeader>()) as *mut u8
    }

    unsafe fn from_data_ptr(ptr: *mut u8) -> *mut BlockHeader {
        (ptr as usize - size_of::<BlockHeader>()) as *mut BlockHeader
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

#[global_allocator]
pub static HEAP_ALLOCATOR: HeapAllocator = HeapAllocator::new(HEAP_START, HEAP_SIZE);

pub fn init_heap() {
    HEAP_ALLOCATOR.init();
}
