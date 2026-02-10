//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 简单的集合类型实现
//! 绕过 alloc crate，直接使用 GlobalAlloc

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use crate::mm::allocator::HEAP_ALLOCATOR;

/// 简单的 Box 包装器
/// 在堆上分配单个值
pub struct SimpleBox<T> {
    ptr: NonNull<T>,
}

impl<T> SimpleBox<T> {
    /// 在堆上分配一个值
    pub fn new(value: T) -> Option<Self> {
        let layout = Layout::new::<T>();
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }
            *(ptr as *mut T) = value;
            Some(SimpleBox {
                ptr: NonNull::new_unchecked(ptr as *mut T),
            })
        }
    }

    /// 获取内部值的引用
    pub fn as_ref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }

    /// 获取内部值的可变引用
    pub fn as_mut(&mut self) -> &mut T {
        unsafe { self.ptr.as_mut() }
    }

    /// 消费 Box 并返回内部值
    pub fn into_inner(self) -> T {
        unsafe {
            let value = core::ptr::read(self.ptr.as_ptr());
            core::mem::forget(self);
            value
        }
    }
}

impl<T> Drop for SimpleBox<T> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::new::<T>();
            core::ptr::drop_in_place(self.ptr.as_ptr());
            GlobalAlloc::dealloc(&HEAP_ALLOCATOR, self.ptr.as_ptr() as *mut u8, layout);
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for SimpleBox<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SimpleBox({:?})", self.as_ref())
    }
}

/// 简单的 Vec 包装器
/// 动态增长的数组
pub struct SimpleVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

impl<T> SimpleVec<T> {
    /// 创建一个具有指定容量的空 Vec
    pub fn with_capacity(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }

        let layout = Layout::array::<T>(capacity).ok()?;
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }
            Some(SimpleVec {
                ptr: NonNull::new_unchecked(ptr as *mut T),
                capacity,
                len: 0,
            })
        }
    }

    /// 创建一个空的 Vec
    pub fn new() -> Option<Self> {
        Self::with_capacity(4)
    }

    /// 返回当前长度
    pub fn len(&self) -> usize {
        self.len
    }

    /// 返回当前容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 压入一个值到末尾
    pub fn push(&mut self, value: T) -> bool {
        if self.len >= self.capacity {
            // 需要扩容
            if !self.grow() {
                return false;
            }
        }

        unsafe {
            core::ptr::write(self.ptr.as_ptr().add(self.len), value);
        }
        self.len += 1;
        true
    }

    /// 弹出最后一个值
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        self.len -= 1;
        unsafe {
            Some(core::ptr::read(self.ptr.as_ptr().add(self.len)))
        }
    }

    /// 获取指定索引的值的引用
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        unsafe { Some(&*self.ptr.as_ptr().add(index)) }
    }

    /// 获取指定索引的值的可变引用
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        unsafe { Some(&mut *self.ptr.as_ptr().add(index)) }
    }

    /// 扩容（容量翻倍）
    fn grow(&mut self) -> bool {
        let new_capacity = self.capacity * 2;
        let new_layout = match Layout::array::<T>(new_capacity) {
            Ok(layout) => layout,
            Err(_) => return false,
        };

        unsafe {
            let new_ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, new_layout);
            if new_ptr.is_null() {
                return false;
            }

            // 复制旧数据
            core::ptr::copy_nonoverlapping(
                self.ptr.as_ptr(),
                new_ptr as *mut T,
                self.len,
            );

            // 释放旧内存
            let old_layout = match Layout::array::<T>(self.capacity) {
                Ok(layout) => layout,
                Err(_) => return false,
            };
            core::ptr::drop_in_place(self.ptr.as_ptr());
            GlobalAlloc::dealloc(&HEAP_ALLOCATOR, self.ptr.as_ptr() as *mut u8, old_layout);

            self.ptr = NonNull::new_unchecked(new_ptr as *mut T);
            self.capacity = new_capacity;
            true
        }
    }

    /// 返回迭代器
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            vec: self,
            index: 0,
        }
    }
}

impl<T> Drop for SimpleVec<T> {
    fn drop(&mut self) {
        unsafe {
            // 释放所有元素
            for i in 0..self.len {
                core::ptr::drop_in_place(self.ptr.as_ptr().add(i));
            }

            // 释放内存
            if self.capacity > 0 {
                let layout = Layout::array::<T>(self.capacity).ok();
                if let Some(layout) = layout {
                    GlobalAlloc::dealloc(&HEAP_ALLOCATOR, self.ptr.as_ptr() as *mut u8, layout);
                }
            }
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for SimpleVec<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// SimpleVec 的迭代器
pub struct Iter<'a, T> {
    vec: &'a SimpleVec<T>,
    index: usize,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.vec.len {
            return None;
        }
        let item = unsafe { Some(&*self.vec.ptr.as_ptr().add(self.index)) };
        self.index += 1;
        item
    }
}

/// 简单的字符串包装器
pub struct SimpleString {
    vec: SimpleVec<u8>,
}

impl SimpleString {
    /// 创建一个空字符串
    pub fn new() -> Option<Self> {
        match SimpleVec::new() {
            Some(vec) => Some(SimpleString { vec }),
            None => None,
        }
    }

    /// 从字面量创建字符串
    pub fn from_str(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        match SimpleVec::with_capacity(bytes.len()) {
            Some(mut vec) => {
                for &byte in bytes {
                    if !vec.push(byte) {
                        return None;
                    }
                }
                Some(SimpleString { vec })
            }
            None => None,
        }
    }

    /// 返回字符串切片
    pub fn as_str(&self) -> &str {
        unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                self.vec.ptr.as_ptr(),
                self.vec.len,
            ))
        }
    }

    /// 压入一个字符
    pub fn push(&mut self, ch: char) -> bool {
        let mut buf = [0u8; 4];
        ch.encode_utf8(&mut buf);
        for byte in buf {
            if !self.vec.push(byte) {
                return false;
            }
        }
        true
    }

    /// 返回长度
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    /// 检查字符串是否以指定前缀开头
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.as_str().starts_with(prefix)
    }

    /// 在指定位置分割字符串
    pub fn split_at(&self, mid: usize) -> (&str, &str) {
        self.as_str().split_at(mid)
    }

    /// 查找字符首次出现的位置
    pub fn find(&self, ch: char) -> Option<usize> {
        self.as_str().find(ch)
    }

    /// 查找字符最后一次出现的位置
    pub fn rfind(&self, ch: char) -> Option<usize> {
        self.as_str().rfind(ch)
    }

    /// 从起始位置删除指定前缀
    pub fn strip_prefix(&self, prefix: &str) -> Option<&str> {
        self.as_str().strip_prefix(prefix)
    }
}

impl core::fmt::Debug for SimpleString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "\"{}\"", self.as_str())
    }
}

/// 简单的 Arc（原子引用计数）包装器
/// 用于多所有者共享数据
use core::sync::atomic::{AtomicUsize, Ordering};

/// Arc 内部数据结构
/// 包含数据本身和引用计数
struct ArcInner<T> {
    ref_count: AtomicUsize,
    data: T,
}

/// 简单的 Arc 包装器
pub struct SimpleArc<T> {
    ptr: NonNull<ArcInner<T>>,
}

impl<T> SimpleArc<T> {
    /// 创建一个新的 Arc
    pub fn new(data: T) -> Option<Self> {
        let layout = Layout::new::<ArcInner<T>>();
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }

            let inner = ptr as *mut ArcInner<T>;
            core::ptr::write(&mut (*inner).ref_count, AtomicUsize::new(1));
            core::ptr::write(&mut (*inner).data, data);

            Some(SimpleArc {
                ptr: NonNull::new_unchecked(inner),
            })
        }
    }

    /// 获取内部数据的引用
    pub fn as_ref(&self) -> &T {
        unsafe { &self.ptr.as_ref().data }
    }

    /// 获取内部数据的裸指针
    pub fn as_ptr(&self) -> *mut T {
        unsafe { core::ptr::addr_of_mut!((*self.ptr.as_ptr()).data) }
    }

    /// 增加引用计数
    fn inc_ref(&self) {
        unsafe {
            self.ptr.as_ref().ref_count.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl<T> Clone for SimpleArc<T> {
    fn clone(&self) -> Self {
        self.inc_ref();
        SimpleArc {
            ptr: self.ptr,
        }
    }
}

impl<T> Drop for SimpleArc<T> {
    fn drop(&mut self) {
        unsafe {
            let inner = self.ptr.as_ref();
            // 减少引用计数
            if inner.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
                // 这是最后一个引用，释放数据
                core::ptr::drop_in_place(&mut (*self.ptr.as_ptr()).data);
                let layout = Layout::new::<ArcInner<T>>();
                GlobalAlloc::dealloc(
                    &HEAP_ALLOCATOR,
                    self.ptr.as_ptr() as *mut u8,
                    layout,
                );
            }
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for SimpleArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SimpleArc({:?})", self.as_ref())
    }
}

impl<T> core::ops::Deref for SimpleArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_box() {
        let box_val = SimpleBox::new(42).unwrap();
        assert_eq!(*box_val.as_ref(), 42);
        assert_eq!(box_val.into_inner(), 42);
    }

    #[test]
    fn test_simple_vec() {
        let mut vec = SimpleVec::with_capacity(4).unwrap();
        assert!(vec.push(1));
        assert!(vec.push(2));
        assert!(vec.push(3));
        assert_eq!(vec.len(), 3);
        assert_eq!(*vec.get(0).unwrap(), 1);
        assert_eq!(*vec.get(1).unwrap(), 2);
        assert_eq!(*vec.pop().unwrap(), 3);
        assert_eq!(vec.len(), 2);
    }

    #[test]
    fn test_simple_string() {
        let mut s = SimpleString::from_str("hello").unwrap();
        assert_eq!(s.as_str(), "hello");
        assert!(s.push('!'));
        assert_eq!(s.as_str(), "hello!");
        assert_eq!(s.len(), 6);
    }

    #[test]
    fn test_simple_arc() {
        let arc = SimpleArc::new(42).unwrap();
        assert_eq!(*arc.as_ref(), 42);

        // 测试克隆
        let arc2 = arc.clone();
        assert_eq!(*arc2.as_ref(), 42);

        // 两者都指向相同的数据
        assert!(core::ptr::eq(arc.as_ref(), arc2.as_ref()));
    }
}
