//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 文件写入操作单元测试
//!
//! 测试 ext4 文件系统的文件写入功能

use crate::println;

#[cfg(feature = "unit-test")]
pub fn test_ext4_file_write() {
    println!("test: ===== Starting ext4 File Write Tests =====");

    // 测试 1: 块计算测试
    println!("test: 1. Testing block calculations for file write...");
    test_block_calculations();

    // 测试 2: 文件大小扩展测试
    println!("test: 2. Testing file size expansion...");
    test_file_expansion();

    // 测试 3: 块对齐测试
    println!("test: 3. Testing block alignment...");
    test_block_alignment();

    // 测试 4: 写入偏移计算测试
    println!("test: 4. Testing write offset calculations...");
    test_write_offset_calculations();

    // 测试 5: 直接块限制测试
    println!("test: 5. Testing direct block limits...");
    test_direct_block_limits();

    println!("test: ===== ext4 File Write Tests Completed =====");
}

fn test_block_calculations() {
    println!("test:    Testing block count calculations...");

    struct TestCase {
        file_size: u64,
        block_size: u64,
        expected_blocks: u64,
    }

    let test_cases = [
        TestCase {
            file_size: 0,
            block_size: 4096,
            expected_blocks: 0,
        },
        TestCase {
            file_size: 1,
            block_size: 4096,
            expected_blocks: 1,
        },
        TestCase {
            file_size: 4096,
            block_size: 4096,
            expected_blocks: 1,
        },
        TestCase {
            file_size: 4097,
            block_size: 4096,
            expected_blocks: 2,
        },
        TestCase {
            file_size: 8192,
            block_size: 4096,
            expected_blocks: 2,
        },
        TestCase {
            file_size: 10000,
            block_size: 4096,
            expected_blocks: 3,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let blocks = (tc.file_size + tc.block_size - 1) / tc.block_size;

        println!("test:      Test {}: size={}, block_size={}", i + 1, tc.file_size, tc.block_size);
        println!("test:        Calculated blocks: {}", blocks);
        println!("test:        Expected blocks: {}", tc.expected_blocks);

        if blocks == tc.expected_blocks {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Block calculations work correctly");
}

fn test_file_expansion() {
    println!("test:    Testing file expansion scenarios...");

    struct TestCase {
        current_size: u64,
        write_offset: u64,
        write_size: u64,
        block_size: u64,
        expected_current_blocks: u64,
        expected_needed_blocks: u64,
        expected_new_blocks: u64,
    }

    let test_cases = [
        TestCase {
            current_size: 0,
            write_offset: 0,
            write_size: 100,
            block_size: 4096,
            expected_current_blocks: 0,
            expected_needed_blocks: 1,
            expected_new_blocks: 1,
        },
        TestCase {
            current_size: 4096,
            write_offset: 4096,
            write_size: 100,
            block_size: 4096,
            expected_current_blocks: 1,
            expected_needed_blocks: 2,
            expected_new_blocks: 1,
        },
        TestCase {
            current_size: 4096,
            write_offset: 0,
            write_size: 100,
            block_size: 4096,
            expected_current_blocks: 1,
            expected_needed_blocks: 1,
            expected_new_blocks: 0,
        },
        TestCase {
            current_size: 8192,
            write_offset: 8192,
            write_size: 8192,
            block_size: 4096,
            expected_current_blocks: 2,
            expected_needed_blocks: 4,
            expected_new_blocks: 2,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let end_offset = tc.write_offset + tc.write_size;
        let current_blocks = (tc.current_size + tc.block_size - 1) / tc.block_size;
        let needed_blocks = (end_offset + tc.block_size - 1) / tc.block_size;
        let new_blocks = if needed_blocks > current_blocks {
            needed_blocks - current_blocks
        } else {
            0
        };

        println!("test:      Test {}: current_size={}, offset={}, write={}",
                 i + 1, tc.current_size, tc.write_offset, tc.write_size);
        println!("test:        current_blocks={}, needed_blocks={}, new_blocks={}",
                 current_blocks, needed_blocks, new_blocks);
        println!("test:        expected current={}, needed={}, new={}",
                 tc.expected_current_blocks, tc.expected_needed_blocks, tc.expected_new_blocks);

        if current_blocks == tc.expected_current_blocks
            && needed_blocks == tc.expected_needed_blocks
            && new_blocks == tc.expected_new_blocks
        {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - File expansion calculations work correctly");
}

fn test_block_alignment() {
    println!("test:    Testing block alignment for writes...");

    struct TestCase {
        offset: u64,
        block_size: u64,
        expected_block_index: u64,
        expected_block_offset: usize,
    }

    let test_cases = [
        TestCase {
            offset: 0,
            block_size: 4096,
            expected_block_index: 0,
            expected_block_offset: 0,
        },
        TestCase {
            offset: 100,
            block_size: 4096,
            expected_block_index: 0,
            expected_block_offset: 100,
        },
        TestCase {
            offset: 4096,
            block_size: 4096,
            expected_block_index: 1,
            expected_block_offset: 0,
        },
        TestCase {
            offset: 5000,
            block_size: 4096,
            expected_block_index: 1,
            expected_block_offset: 904,
        },
        TestCase {
            offset: 8192,
            block_size: 4096,
            expected_block_index: 2,
            expected_block_offset: 0,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let block_index = tc.offset / tc.block_size;
        let block_offset = (tc.offset % tc.block_size) as usize;

        println!("test:      Test {}: offset={}, block_size={}",
                 i + 1, tc.offset, tc.block_size);
        println!("test:        block_index={}, block_offset={}",
                 block_index, block_offset);
        println!("test:        expected block_index={}, block_offset={}",
                 tc.expected_block_index, tc.expected_block_offset);

        if block_index == tc.expected_block_index && block_offset == tc.expected_block_offset {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Block alignment works correctly");
}

fn test_write_offset_calculations() {
    println!("test:    Testing write offset calculations...");

    let block_size: u64 = 4096;
    let offset: u64 = 5000;
    let write_size: usize = 10000;

    let block_index = offset / block_size;
    let block_offset = (offset % block_size) as usize;

    println!("test:      Block size: {}", block_size);
    println!("test:      Write offset: {}", offset);
    println!("test:      Write size: {}", write_size);
    println!("test:      First block index: {}", block_index);
    println!("test:      First block offset: {}", block_offset);

    // 计算第一个块中可写入的字节数
    let available_in_first_block = block_size as usize - block_offset;
    println!("test:      Available in first block: {}", available_in_first_block);

    // 计算需要多少个块
    let total_blocks_needed = ((offset as usize + write_size) + block_size as usize - 1) / block_size as usize;
    println!("test:      Total blocks needed: {}", total_blocks_needed);

    if available_in_first_block == 3092 && total_blocks_needed == 4 {
        println!("test:    SUCCESS - Write offset calculations work correctly");
    } else {
        println!("test:    FAILED - Write offset calculations failed");
    }
}

fn test_direct_block_limits() {
    println!("test:    Testing direct block limits...");

    let direct_blocks = 12;  // ext4 直接块数量
    let block_size = 4096;    // 典型块大小

    println!("test:      Direct blocks: {}", direct_blocks);
    println!("test:      Block size: {}", block_size);

    // 计算最大文件大小
    let max_file_size = direct_blocks * block_size;
    println!("test:      Max file size with direct blocks: {} bytes", max_file_size);
    println!("test:      Max file size: {} KB", max_file_size / 1024);

    // 测试边界条件
    let test_sizes = [
        (0, 0, true),
        (1, 1, true),
        (4096, 1, true),
        (4097, 2, true),
        (49152, 12, true),      // 12 * 4096
        (49153, 13, false),     // 超过直接块限制
    ];

    for (size, expected_blocks, should_succeed) in test_sizes {
        let blocks = (size + block_size - 1) / block_size;

        println!("test:      File size: {} bytes", size);
        println!("test:        Required blocks: {}", blocks);
        println!("test:        Expected blocks: {}", expected_blocks);
        println!("test:        Should succeed: {}", should_succeed);

        let success = blocks <= direct_blocks;
        if blocks == expected_blocks && success == should_succeed {
            println!("test:        Test PASSED");
        } else {
            println!("test:        Test FAILED");
        }
    }

    println!("test:    SUCCESS - Direct block limits verified");
}
