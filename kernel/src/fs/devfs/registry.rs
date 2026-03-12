//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Character Device Registry
//!
//! Manages all registered character devices and their operation functions

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use crate::fs::file::FileOps;
use super::dev_t::DevNo;

/// Character device registry
struct CharDeviceRegistry {
    /// Device number -> FileOps mapping
    devices: BTreeMap<u64, &'static FileOps>,
}

impl CharDeviceRegistry {
    const fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }
}

/// Global character device registry
static CHAR_DEVICES: Mutex<CharDeviceRegistry> = Mutex::new(CharDeviceRegistry::new());

/// Register character device
///
/// # Arguments
/// - devno: Device number
/// - ops: File operation functions
///
/// # Returns
/// Ok(()) on success, Err(()) if device number is already in use
pub fn register_char_device(devno: DevNo, ops: &'static FileOps) -> Result<(), ()> {
    let mut registry = CHAR_DEVICES.lock();
    let key = devno.to_u64();

    if registry.devices.contains_key(&key) {
        return Err(()); // Device number already in use
    }

    registry.devices.insert(key, ops);
    Ok(())
}

/// Unregister character device
pub fn unregister_char_device(devno: DevNo) {
    let mut registry = CHAR_DEVICES.lock();
    registry.devices.remove(&devno.to_u64());
}

/// Get character device operation functions
///
/// # Arguments
/// - devno: Device number
///
/// # Returns
/// If device is registered, returns corresponding FileOps; otherwise returns None
pub fn get_char_device_ops(devno: DevNo) -> Option<&'static FileOps> {
    let registry = CHAR_DEVICES.lock();
    registry.devices.get(&devno.to_u64()).copied()
}

/// Check if device is registered
pub fn is_device_registered(devno: DevNo) -> bool {
    let registry = CHAR_DEVICES.lock();
    registry.devices.contains_key(&devno.to_u64())
}

/// Get registered device count
pub fn device_count() -> usize {
    let registry = CHAR_DEVICES.lock();
    registry.devices.len()
}
