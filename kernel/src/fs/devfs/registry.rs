//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 字符设备注册表
//!
//! 管理所有注册的字符设备及其操作函数

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use crate::fs::file::FileOps;
use super::dev_t::DevNo;

/// 字符设备注册表
struct CharDeviceRegistry {
    /// 设备号 -> FileOps 映射
    devices: BTreeMap<u64, &'static FileOps>,
}

impl CharDeviceRegistry {
    const fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }
}

/// 全局字符设备注册表
static CHAR_DEVICES: Mutex<CharDeviceRegistry> = Mutex::new(CharDeviceRegistry::new());

/// 注册字符设备
///
/// # 参数
/// - devno: 设备号
/// - ops: 文件操作函数
///
/// # 返回
/// 成功返回 Ok(()), 如果设备号已被占用返回 Err(())
pub fn register_char_device(devno: DevNo, ops: &'static FileOps) -> Result<(), ()> {
    let mut registry = CHAR_DEVICES.lock();
    let key = devno.to_u64();

    if registry.devices.contains_key(&key) {
        return Err(()); // 设备号已被占用
    }

    registry.devices.insert(key, ops);
    Ok(())
}

/// 注销字符设备
pub fn unregister_char_device(devno: DevNo) {
    let mut registry = CHAR_DEVICES.lock();
    registry.devices.remove(&devno.to_u64());
}

/// 获取字符设备的操作函数
///
/// # 参数
/// - devno: 设备号
///
/// # 返回
/// 如果设备已注册，返回对应的 FileOps；否则返回 None
pub fn get_char_device_ops(devno: DevNo) -> Option<&'static FileOps> {
    let registry = CHAR_DEVICES.lock();
    registry.devices.get(&devno.to_u64()).copied()
}

/// 检查设备是否已注册
pub fn is_device_registered(devno: DevNo) -> bool {
    let registry = CHAR_DEVICES.lock();
    registry.devices.contains_key(&devno.to_u64())
}

/// 获取已注册设备数量
pub fn device_count() -> usize {
    let registry = CHAR_DEVICES.lock();
    registry.devices.len()
}
