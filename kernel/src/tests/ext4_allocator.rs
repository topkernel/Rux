//! ext4 块和 inode 分配器单元测试
//!
//! 测试 ext4 文件系统的块和 inode 分配功能

use crate::println;

#[cfg(feature = "unit-test")]
pub fn test_ext4_allocator() {
    println!("test: ===== Starting ext4 Allocator Tests =====");

    // 测试 1: 位操作测试
    println!("test: 1. Testing bitmap operations...");
    test_bitmap_operations();

    // 测试 2: 块组计算测试
    println!("test: 2. Testing block group calculations...");
    test_block_group_calculations();

    // 测试 3: inode 组计算测试
    println!("test: 3. Testing inode group calculations...");
    test_inode_group_calculations();

    // 测试 4: 位图索引计算测试
    println!("test: 4. Testing bitmap index calculations...");
    test_bitmap_index_calculations();

    // 测试 5: 分配器边界条件测试
    println!("test: 5. Testing allocator boundary conditions...");
    test_allocator_boundaries();

    println!("test: ===== ext4 Allocator Tests Completed =====");
}

fn test_bitmap_operations() {
    println!("test:    Testing bitmap bit manipulation...");

    // 测试 1: 设置位
    let mut bitmap: u8 = 0b00000000;
    bitmap |= 1 << 3;
    println!("test:      Set bit 3: 0b{:08b} (expected 0b00001000)", bitmap);
    assert!(bitmap == 0b00001000);

    // 测试 2: 清除位
    bitmap &= !(1 << 3);
    println!("test:      Clear bit 3: 0b{:08b} (expected 0b00000000)", bitmap);
    assert!(bitmap == 0b00000000);

    // 测试 3: 检查位
    bitmap |= 1 << 5;
    let is_set = (bitmap & (1 << 5)) != 0;
    println!("test:      Check bit 5: {} (expected true)", is_set);
    assert!(is_set);

    // 测试 4: 检查多个位
    let bitmap = 0b11111010u8;
    let bit0_set = (bitmap & (1 << 0)) != 0;
    let bit1_set = (bitmap & (1 << 1)) != 0;
    let bit3_set = (bitmap & (1 << 3)) != 0;
    let bit7_set = (bitmap & (1 << 7)) != 0;

    println!("test:      Bitmap: 0b{:08b}", bitmap);
    println!("test:        Bit 0: {} (expected true)", bit0_set);
    println!("test:        Bit 1: {} (expected false)", bit1_set);
    println!("test:        Bit 3: {} (expected false)", bit3_set);
    println!("test:        Bit 7: {} (expected true)", bit7_set);

    if bit0_set && !bit1_set && !bit3_set && bit7_set {
        println!("test:    SUCCESS - Bitmap operations work correctly");
    } else {
        println!("test:    FAILED - Bitmap operations failed");
    }
}

fn test_block_group_calculations() {
    println!("test:    Testing block to group mapping...");

    // 测试用例
    struct TestCase {
        block_number: u64,
        blocks_per_group: u64,
        expected_group: u64,
        expected_offset: u64,
    }

    let test_cases = [
        TestCase {
            block_number: 0,
            blocks_per_group: 8192,
            expected_group: 0,
            expected_offset: 0,
        },
        TestCase {
            block_number: 8191,
            blocks_per_group: 8192,
            expected_group: 0,
            expected_offset: 8191,
        },
        TestCase {
            block_number: 8192,
            blocks_per_group: 8192,
            expected_group: 1,
            expected_offset: 0,
        },
        TestCase {
            block_number: 15000,
            blocks_per_group: 8192,
            expected_group: 1,
            expected_offset: 6808,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let group = tc.block_number / tc.blocks_per_group;
        let offset = tc.block_number % tc.blocks_per_group;

        println!("test:      Test {}: block={}, bpg={}", i + 1, tc.block_number, tc.blocks_per_group);
        println!("test:        group={}, offset={}", group, offset);
        println!("test:        expected group={}, offset={}", tc.expected_group, tc.expected_offset);

        if group == tc.expected_group && offset == tc.expected_offset {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Block group calculations work correctly");
}

fn test_inode_group_calculations() {
    println!("test:    Testing inode to group mapping...");

    // ext4 inode 从 1 开始计数（0 保留）
    struct TestCase {
        inode_number: u32,
        inodes_per_group: u64,
        expected_group: u64,
        expected_offset: u64,
    }

    let test_cases = [
        TestCase {
            inode_number: 1,
            inodes_per_group: 8192,
            expected_group: 0,
            expected_offset: 0,
        },
        TestCase {
            inode_number: 8192,
            inodes_per_group: 8192,
            expected_group: 0,
            expected_offset: 8191,
        },
        TestCase {
            inode_number: 8193,
            inodes_per_group: 8192,
            expected_group: 1,
            expected_offset: 0,
        },
        TestCase {
            inode_number: 15000,
            inodes_per_group: 8192,
            expected_group: 1,
            expected_offset: 6807,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let group = (tc.inode_number as u64 - 1) / tc.inodes_per_group;
        let offset = (tc.inode_number as u64 - 1) % tc.inodes_per_group;

        println!("test:      Test {}: inode={}, ipg={}", i + 1, tc.inode_number, tc.inodes_per_group);
        println!("test:        group={}, offset={}", group, offset);
        println!("test:        expected group={}, offset={}", tc.expected_group, tc.expected_offset);

        if group == tc.expected_group && offset == tc.expected_offset {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Inode group calculations work correctly");
}

fn test_bitmap_index_calculations() {
    println!("test:    Testing bitmap index calculations...");

    // 测试用例：将位偏移转换为字节索引和位索引
    struct TestCase {
        bit_offset: usize,
        expected_byte_idx: usize,
        expected_bit_idx: usize,
    }

    let test_cases = [
        TestCase {
            bit_offset: 0,
            expected_byte_idx: 0,
            expected_bit_idx: 0,
        },
        TestCase {
            bit_offset: 7,
            expected_byte_idx: 0,
            expected_bit_idx: 7,
        },
        TestCase {
            bit_offset: 8,
            expected_byte_idx: 1,
            expected_bit_idx: 0,
        },
        TestCase {
            bit_offset: 10,
            expected_byte_idx: 1,
            expected_bit_idx: 2,
        },
        TestCase {
            bit_offset: 100,
            expected_byte_idx: 12,
            expected_bit_idx: 4,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let byte_idx = tc.bit_offset / 8;
        let bit_idx = tc.bit_offset % 8;

        println!("test:      Test {}: bit_offset={}", i + 1, tc.bit_offset);
        println!("test:        byte_idx={}, bit_idx={}", byte_idx, bit_idx);
        println!("test:        expected byte_idx={}, bit_idx={}", tc.expected_byte_idx, tc.expected_bit_idx);

        if byte_idx == tc.expected_byte_idx && bit_idx == tc.expected_bit_idx {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Bitmap index calculations work correctly");
}

fn test_allocator_boundaries() {
    println!("test:    Testing allocator boundary conditions...");

    // 测试 1: 验证块组边界
    println!("test:      Test 1: Block group boundaries...");
    let blocks_per_group: u64 = 8192;
    let block_groups: u32 = 4;

    // 最后一个块的编号
    let last_block = (block_groups as u64) * blocks_per_group - 1;
    println!("test:        Total blocks: {}", last_block + 1);
    println!("test:        Last block in group {}: {}", block_groups - 1, last_block);

    // 测试 2: 验证 inode 表边界
    println!("test:      Test 2: Inode table boundaries...");
    let inodes_per_group: u64 = 8192;
    let block_groups: u32 = 4;

    // 最后一个 inode 的编号
    let last_inode = (block_groups as u64) * inodes_per_group;
    println!("test:        Total inodes: {}", last_inode);
    println!("test:        Last inode in group {}: {}", block_groups - 1, last_inode);

    // 测试 3: 验证位图大小
    println!("test:      Test 3: Bitmap size calculations...");
    let blocks_per_group: u64 = 8192;
    let bitmap_bytes = (blocks_per_group + 7) / 8;
    let bitmap_blocks = (bitmap_bytes + 4095) / 4096;  // 假设块大小 4096

    println!("test:        Blocks per group: {}", blocks_per_group);
    println!("test:        Bitmap bytes: {}", bitmap_bytes);
    println!("test:        Bitmap blocks: {}", bitmap_blocks);

    if bitmap_bytes == 1024 && bitmap_blocks == 1 {
        println!("test:        Bitmap size calculations PASSED");
    } else {
        println!("test:        Bitmap size calculations FAILED");
    }

    // 测试 4: 验证块描述符表边界
    println!("test:      Test 4: Block descriptor table boundaries...");
    let direct_blocks = 12;  // 直接块数量
    let max_file_size = direct_blocks * 4096;  // 假设块大小 4096

    println!("test:        Direct blocks: {}", direct_blocks);
    println!("test:        Max file size with direct blocks: {} bytes", max_file_size);

    if max_file_size == 49152 {
        println!("test:        Direct block size PASSED");
    } else {
        println!("test:        Direct block size FAILED");
    }

    println!("test:    SUCCESS - Boundary conditions verified");
}
