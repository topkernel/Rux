//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 双向链表实现
//!
//! 参考 Linux: include/linux/list.h
//!
//! 用途：
//! - 进程树: task_struct::children, task_struct::sibling
//! - 调度队列: rq::runqueue
//! - 设备列表: device::list
//!
//! 设计特点：
//! - 侵入式链表：list_head 直接嵌入数据结构中
//! - 通用性强：同一个链表可用于不同数据类型
//! - 内存开销小：每个节点仅 2 个指针（16 字节）

use core::ptr;

#[repr(C)]
pub struct ListHead {
    /// 下一个节点
    pub next: *mut ListHead,
    /// 前一个节点
    pub prev: *mut ListHead,
}

impl ListHead {
    /// 创建一个新的链表节点
    ///
    /// 通常用于初始化链表头
    pub const fn new() -> Self {
        Self {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }

    /// 初始化链表节点
    ///
    /// 使节点指向自己，形成一个空链表
    ///
    /// ...
    pub fn init(&mut self) {
        self.next = self;
        self.prev = self;
    }

    /// 检查链表是否为空
    ///
    /// ...
    pub fn is_empty(&self) -> bool {
        self.next == self as *const _ as *mut _
    }

    /// 在指定节点之后插入当前节点
    ///
    /// # 参数
    /// - `head`: 要插入的链表位置（在 head 之后插入）
    ///
    /// # Safety
    /// 调用者必须确保 `head` 是有效的
    ///
    /// ...
    pub unsafe fn add(&mut self, head: *mut ListHead) {
        let next = (*head).next;

        // 插入当前节点到 head 和 head->next 之间
        self.next = next;
        self.prev = head;
        (*head).next = self;
        (*next).prev = self;
    }

    /// 在链表尾部添加节点
    ///
    /// # 参数
    /// - `head`: 链表头（在 head 之前插入，即尾部）
    ///
    /// # Safety
    /// 调用者必须确保 `head` 是有效的
    ///
    /// ...
    pub unsafe fn add_tail(&mut self, head: *mut ListHead) {
        let prev = (*head).prev;

        // 插入当前节点到 head->prev 和 head 之间
        self.next = head;
        self.prev = prev;
        (*head).prev = self;
        (*prev).next = self;
    }

    /// 从链表中删除当前节点
    ///
    /// # Safety
    /// 调用者必须确保节点在链表中
    ///
    /// ...
    pub unsafe fn del(&mut self) {
        let next = self.next;
        let prev = self.prev;

        (*next).prev = prev;
        (*prev).next = next;

        // 标记为已删除（指向自己，用于调试）
        self.next = self as *mut _;
        self.prev = self as *mut _;
    }

    /// 获取包含此 ListHead 的结构体引用
    ///
    /// # 参数
    /// - `ptr`: ListHead 指针
    /// - `type`: 包含结构体类型
    /// - `member`: ListHead 在结构体中的字段名
    ///
    /// # Examples
    /// ```no_run
    /// # use crate::list::ListHead;
    /// # struct Task { children: ListHead, pid: u32 };
    /// # let list_head_ptr = &mut Task { children: ListHead::new(), pid: 0 }.children as *mut _;
    /// let task = unsafe { ListHead::entry(list_head_ptr, Task, children) };
    /// assert_eq!((*task).pid, 0);
    /// ```
    ///
    /// # Safety
    /// 调用者必须确保 `ptr` 是有效的，且指向正确的 `member`
    ///
    /// ...
    pub unsafe fn entry<T>(ptr: *mut ListHead, member: impl OffsetHelper<T>) -> *mut T {
        // 计算结构体起始地址：ptr - offset_of(member)
        let offset = member.offset();
        (ptr as *mut u8).sub(offset) as *mut T
    }

    /// 遍历链表
    ///
    /// # 参数
    /// - `head`: 链表头
    /// - `f`: 对每个节点调用的闭包
    ///
    /// # Safety
    /// 调用者必须确保 `head` 是有效的，且在遍历期间不修改链表
    ///
    /// ...
    pub unsafe fn for_each<F>(head: *mut ListHead, mut f: F)
    where
        F: FnMut(*mut ListHead),
    {
        let mut pos = (*head).next;
        let mut iterations = 0usize;
        while pos != head {
            if iterations > 1000 {
                // 防止无限循环
                use crate::console::putchar;
                const MSG: &[u8] = b"ListHead::for_each: Too many iterations, breaking\n";
                for &b in MSG {
                    putchar(b);
                }
                break;
            }
            iterations += 1;
            let next = (*pos).next;
            f(pos);
            pos = next;
        }
    }

    /// 获取第一个节点
    ///
    /// ...
    pub unsafe fn first_entry<T>(head: *mut ListHead, member: impl OffsetHelper<T>) -> Option<*mut T> {
        if (*head).next == head {
            None
        } else {
            Some(Self::entry((*head).next, member))
        }
    }
}

pub trait OffsetHelper<T> {
    fn offset(&self) -> usize;
}

#[allow(dead_code)]
#[allow(unused_macros)]
macro_rules! impl_offset_helper {
    ($type:ty, $member:ident) => {
        impl OffsetHelper<$type> for fn() -> usize {
            fn offset(&self) -> usize {
                // 使用 core::mem::offset_of! (Rust 1.77+)
                // 如果不可用，使用 unsafe 替代方案
                extern crate core;
                unsafe {
                    let dummy = core::mem::MaybeUninit::<$type>::uninit();
                    let base = dummy.as_ptr();
                    let member_ptr = core::ptr::addr_of!((*base).$member);
                    (member_ptr as usize) - (base as usize)
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_init() {
        let mut head = ListHead::new();
        head.init();
        assert!(head.is_empty());
        assert_eq!(head.next, &head as *const _ as *mut _);
        assert_eq!(head.prev, &head as *const _ as *mut _);
    }

    #[test]
    fn test_list_add() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add(&mut head);

            assert!(!head.is_empty());
            assert_eq!(head.next, &node1 as *const _ as *mut _);
            assert_eq!(head.prev, &node1 as *const _ as *mut _);
        }
    }

    #[test]
    fn test_list_add_tail() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add_tail(&mut head);

            let mut node2 = ListHead::new();
            node2.add_tail(&mut head);

            // head -> node1 -> node2 -> head
            assert_eq!(head.next, &node1 as *const _ as *mut _);
            assert_eq!(node1.next, &node2 as *const _ as *mut _);
            assert_eq!(node2.next, &head as *const _ as *mut _);
        }
    }

    #[test]
    fn test_list_del() {
        unsafe {
            let mut head = ListHead::new();
            head.init();

            let mut node1 = ListHead::new();
            node1.add(&mut head);

            assert!(!head.is_empty());

            node1.del();

            assert!(head.is_empty());
        }
    }
}
