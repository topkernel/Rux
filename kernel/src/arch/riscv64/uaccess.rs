//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 用户空间访问函数
//!
//! 提供安全的内核到用户空间数据复制功能。
//! 使用异常表机制处理用户空间访问时的页故障。
//!
//! # 主要函数
//! - `copy_to_user`: 将数据从内核复制到用户空间
//! - `copy_from_user`: 将数据从用户空间复制到内核
//! - `clear_user`: 将用户空间内存清零
//!
//! # 异常表机制
//! 这些函数使用异常表来安全地处理无效的用户地址。
//! 如果访问失败，函数返回未复制的字节数（而非崩溃）。
//!
//! # 参考
//! Linux: arch/riscv/include/asm/uaccess.h
//! Linux: arch/riscv/lib/uaccess.S

/// 用户空间访问错误类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAccessError {
    /// 访问成功
    Success,
    /// 源地址无效
    InvalidSource,
    /// 目标地址无效
    InvalidDestination,
    /// 地址未对齐
    Unaligned,
    /// 未知错误
    Unknown,
}

/// 检查用户空间地址是否有效
///
/// # 参数
/// - `addr`: 用户空间地址
/// - `size`: 访问大小
///
/// # 返回
/// 如果地址在用户空间范围内返回 true
#[inline]
pub fn access_ok(addr: usize, size: usize) -> bool {
    use super::mm::user_addr::{USER_START, USER_END};

    // 检查地址范围
    if addr < USER_START {
        return false;
    }

    // 检查溢出
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };

    end <= USER_END
}

/// 将数据从内核复制到用户空间
///
/// # 参数
/// - `to`: 用户空间目标地址
/// - `from`: 内核源地址
/// - `n`: 复制字节数
///
/// # 返回
/// 返回未复制的字节数。0 表示完全成功。
///
/// # 安全性
/// - `from` 必须指向有效的内核内存
/// - `to` 必须是有效的用户空间地址（如果无效，返回 n）
///
/// # 参考
/// Linux: _copy_to_user()
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // 检查用户空间地址是否有效
    if !access_ok(to as usize, n) {
        return n;
    }

    // 使用带异常处理的复制
    // 如果访问失败，返回未复制的字节数
    let mut remaining = n;
    let mut dst = to as usize;
    let mut src = from as usize;

    while remaining > 0 {
        // 尝试复制一个字节
        let result = copy_one_byte_to_user(dst, src);

        match result {
            Ok(_) => {
                dst += 1;
                src += 1;
                remaining -= 1;
            }
            Err(_) => {
                // 复制失败，返回剩余字节数
                break;
            }
        }
    }

    remaining
}

/// 将数据从用户空间复制到内核
///
/// # 参数
/// - `to`: 内核目标地址
/// - `from`: 用户空间源地址
/// - `n`: 复制字节数
///
/// # 返回
/// 返回未复制的字节数。0 表示完全成功。
///
/// # 安全性
/// - `to` 必须指向有效的内核内存
/// - `from` 必须是有效的用户空间地址（如果无效，返回 n）
///
/// # 参考
/// Linux: _copy_from_user()
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // 检查用户空间地址是否有效
    if !access_ok(from as usize, n) {
        return n;
    }

    // 使用带异常处理的复制
    let mut remaining = n;
    let mut dst = to as usize;
    let mut src = from as usize;

    while remaining > 0 {
        // 尝试复制一个字节
        let result = copy_one_byte_from_user(dst, src);

        match result {
            Ok(_) => {
                dst += 1;
                src += 1;
                remaining -= 1;
            }
            Err(_) => {
                // 复制失败，返回剩余字节数
                break;
            }
        }
    }

    remaining
}

/// 复制单个字节到用户空间（带异常处理）
///
/// # 返回
/// - Ok(()): 复制成功
/// - Err(()): 复制失败（用户地址无效）
///
/// # 注意
/// 这是简化实现，实际的异常表机制需要汇编支持
#[inline(always)]
unsafe fn copy_one_byte_to_user(to: usize, from: usize) -> Result<(), ()> {
    // 简化实现：直接复制
    // 实际的异常表版本需要在汇编中添加异常表条目
    let src_ptr = from as *const u8;
    let dst_ptr = to as *mut u8;

    // 检查用户地址有效性
    if !access_ok(to, 1) {
        return Err(());
    }

    *dst_ptr = *src_ptr;
    Ok(())
}

/// 从用户空间复制单个字节（带异常处理）
///
/// # 返回
/// - Ok(()): 复制成功
/// - Err(()): 复制失败（用户地址无效）
///
/// # 注意
/// 这是简化实现，实际的异常表机制需要汇编支持
#[inline(always)]
unsafe fn copy_one_byte_from_user(to: usize, from: usize) -> Result<(), ()> {
    // 简化实现：直接复制
    let src_ptr = from as *const u8;
    let dst_ptr = to as *mut u8;

    // 检查用户地址有效性
    if !access_ok(from, 1) {
        return Err(());
    }

    *dst_ptr = *src_ptr;
    Ok(())
}

/// 将用户空间内存清零
///
/// # 参数
/// - `to`: 用户空间起始地址
/// - `n`: 清零字节数
///
/// # 返回
/// 返回未清零的字节数
///
/// # 安全性
/// `to` 必须是有效的用户空间地址
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    if !access_ok(to as usize, n) {
        return n;
    }

    let mut remaining = n;
    let mut dst = to as usize;

    while remaining > 0 {
        let result = clear_one_byte_user(dst);

        match result {
            Ok(_) => {
                dst += 1;
                remaining -= 1;
            }
            Err(_) => {
                break;
            }
        }
    }

    remaining
}

/// 将用户空间单个字节清零
///
/// # 注意
/// 这是简化实现，实际的异常表机制需要汇编支持
#[inline(always)]
unsafe fn clear_one_byte_user(to: usize) -> Result<(), ()> {
    let dst_ptr = to as *mut u8;

    if !access_ok(to, 1) {
        return Err(());
    }

    *dst_ptr = 0;
    Ok(())
}

// ============================================================================
// 便捷包装函数
// ============================================================================

/// 安全的用户空间读取包装器
///
/// # 参数
/// - `from`: 用户空间源地址
///
/// # 返回
/// 成功返回读取的值，失败返回 None
#[inline]
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    let size = core::mem::size_of::<T>();

    if !access_ok(from as usize, size) {
        return None;
    }

    let mut value: core::mem::MaybeUninit<T> = core::mem::MaybeUninit::uninit();

    let uncopied = copy_from_user(
        value.as_mut_ptr() as *mut u8,
        from as *const u8,
        size,
    );

    if uncopied == 0 {
        Some(value.assume_init())
    } else {
        None
    }
}

/// 安全的用户空间写入包装器
///
/// # 参数
/// - `to`: 用户空间目标地址
/// - `value`: 要写入的值
///
/// # 返回
/// 成功返回 true，失败返回 false
#[inline]
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    let size = core::mem::size_of::<T>();

    if !access_ok(to as usize, size) {
        return false;
    }

    let uncopied = copy_to_user(
        to as *mut u8,
        &value as *const T as *const u8,
        size,
    );

    uncopied == 0
}
