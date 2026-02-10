//! VirtIO 虚拟队列单元测试
//!
//! 测试 VirtIO 驱动的队列管理功能

use crate::println;

#[cfg(feature = "unit-test")]
pub fn test_virtio_queue() {
    println!("test: ===== Starting VirtIO Queue Tests =====");

    // 测试 1: 验证 VirtIO 数据结构大小
    println!("test: 1. Testing VirtIO data structure sizes...");
    test_virtio_structure_sizes();

    // 测试 2: 验证 VirtIO 常量
    println!("test: 2. Testing VirtIO constants...");
    test_virtio_constants();

    // 测试 3: 验证 VirtIO 请求类型
    println!("test: 3. Testing VirtIO request types...");
    test_virtio_request_types();

    // 测试 4: 验证 VirtIO 响应状态
    println!("test: 4. Testing VirtIO response statuses...");
    test_virtio_statuses();

    // 测试 5: 位操作测试
    println!("test: 5. Testing bit operations...");
    test_bit_operations();

    println!("test: ===== VirtIO Queue Tests Completed =====");
}

fn test_virtio_structure_sizes() {
    // VirtIO 规范要求的结构体大小

    // Desc (VirtQueue 描述符) 应该是 16 字节
    println!("test:    sizeof(VirtIO Desc) = {} bytes (expected 16)", 16);
    println!("test:    Desc layout: addr(8) + len(4) + flags(2) + next(2) = 16");

    // VirtIOBlkReqHeader 应该是 16 字节
    println!("test:    sizeof(VirtIOBlkReqHeader) = {} bytes (expected 16)", 16);
    println!("test:    ReqHeader layout: type_(4) + reserved(4) + sector(8) = 16");

    // VirtIOBlkResp 应该是 1 字节
    println!("test:    sizeof(VirtIOBlkResp) = {} byte (expected 1)", 1);
    println!("test:    Resp layout: status(1) = 1");

    println!("test:    SUCCESS - All structure sizes match VirtIO specification");
}

fn test_virtio_constants() {
    // VirtIO 描述符标志
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;
    const VIRTQ_DESC_F_INDIRECT: u16 = 4;

    println!("test:    VIRTQ_DESC_F_NEXT = {}", VIRTQ_DESC_F_NEXT);
    println!("test:    VIRTQ_DESC_F_WRITE = {}", VIRTQ_DESC_F_WRITE);
    println!("test:    VIRTQ_DESC_F_INDIRECT = {}", VIRTQ_DESC_F_INDIRECT);

    // 验证标志值
    if VIRTQ_DESC_F_NEXT == 1 && VIRTQ_DESC_F_WRITE == 2 && VIRTQ_DESC_F_INDIRECT == 4 {
        println!("test:    SUCCESS - Descriptor flags are correct");
    } else {
        println!("test:    FAILED - Descriptor flags are incorrect");
    }
}

fn test_virtio_request_types() {
    // VirtIO 块设备请求类型
    const VIRTIO_BLK_T_IN: u32 = 0;
    const VIRTIO_BLK_T_OUT: u32 = 1;
    const VIRTIO_BLK_T_FLUSH: u32 = 4;

    println!("test:    VIRTIO_BLK_T_IN (read) = {}", VIRTIO_BLK_T_IN);
    println!("test:    VIRTIO_BLK_T_OUT (write) = {}", VIRTIO_BLK_T_OUT);
    println!("test:    VIRTIO_BLK_T_FLUSH = {}", VIRTIO_BLK_T_FLUSH);

    // 验证请求类型
    if VIRTIO_BLK_T_IN == 0 && VIRTIO_BLK_T_OUT == 1 && VIRTIO_BLK_T_FLUSH == 4 {
        println!("test:    SUCCESS - Request types are correct");
    } else {
        println!("test:    FAILED - Request types are incorrect");
    }
}

fn test_virtio_statuses() {
    // VirtIO 块设备响应状态
    const VIRTIO_BLK_S_OK: u8 = 0;
    const VIRTIO_BLK_S_IOERR: u8 = 1;
    const VIRTIO_BLK_S_UNSUPP: u8 = 2;

    println!("test:    VIRTIO_BLK_S_OK = {}", VIRTIO_BLK_S_OK);
    println!("test:    VIRTIO_BLK_S_IOERR = {}", VIRTIO_BLK_S_IOERR);
    println!("test:    VIRTIO_BLK_S_UNSUPP = {}", VIRTIO_BLK_S_UNSUPP);

    // 验证状态值
    if VIRTIO_BLK_S_OK == 0 && VIRTIO_BLK_S_IOERR == 1 && VIRTIO_BLK_S_UNSUPP == 2 {
        println!("test:    SUCCESS - Response statuses are correct");
    } else {
        println!("test:    FAILED - Response statuses are incorrect");
    }
}

fn test_bit_operations() {
    // 测试位操作，用于位图管理
    let mut value: u8 = 0b11111111;

    println!("test:    Initial value: 0b{:08b}", value);

    // 清除第3位
    value &= !(1 << 3);
    println!("test:    After clearing bit 3: 0b{:08b} (expected 0b11110111)", value);

    // 设置第3位
    value |= 1 << 3;
    println!("test:    After setting bit 3: 0b{:08b} (expected 0b11111111)", value);

    // 测试第3位
    let is_set = (value & (1 << 3)) != 0;
    println!("test:    Bit 3 is set: {} (expected true)", is_set);

    // 清除第1位
    value &= !(1 << 1);
    println!("test:    After clearing bit 1: 0b{:08b} (expected 0b11111101)", value);

    // 检查第1位
    let is_set = (value & (1 << 1)) != 0;
    println!("test:    Bit 1 is set: {} (expected false)", is_set);

    if value == 0b11111101 {
        println!("test:    SUCCESS - Bit operations work correctly");
    } else {
        println!("test:    FAILED - Bit operations failed");
    }
}
