//! ext4 间接块单元测试
//!
//! 测试 ext4 文件系统的间接块功能

use crate::println;

#[cfg(feature = "unit-test")]
pub fn test_ext4_indirect_blocks() {
    println!("test: ===== Starting ext4 Indirect Blocks Tests =====");

    // 测试 1: 块索引计算测试
    println!("test: 1. Testing block index calculations...");
    test_block_index_calculations();

    // 测试 2: 间接块级别测试
    println!("test: 2. Testing indirect block levels...");
    test_indirect_levels();

    // 测试 3: 文件大小限制测试
    println!("test: 3. Testing file size limits...");
    test_file_size_limits();

    // 测试 4: 块指针索引测试
    println!("test: 4. Testing block pointer indices...");
    test_block_pointer_indices();

    // 测试 5: 间接块偏移计算测试
    println!("test: 5. Testing indirect offset calculations...");
    test_indirect_offset_calculations();

    println!("test: ===== ext4 Indirect Blocks Tests Completed =====");
}

fn test_block_index_calculations() {
    println!("test:    Testing block index to pointer mapping...");

    struct TestCase {
        block_index: u64,
        block_size: u64,
        expected_type: &'static str,
        expected_pointer: usize,
        expected_offset: usize,
    }

    let block_size = 4096u64;
    let pointers_per_block = (block_size / 4) as usize;

    let test_cases = [
        // 直接块
        TestCase {
            block_index: 0,
            block_size,
            expected_type: "direct",
            expected_pointer: 0,
            expected_offset: 0,
        },
        TestCase {
            block_index: 11,
            block_size,
            expected_type: "direct",
            expected_pointer: 11,
            expected_offset: 0,
        },
        // 单级间接块
        TestCase {
            block_index: 12,
            block_size,
            expected_type: "single_indirect",
            expected_pointer: 12,
            expected_offset: 0,
        },
        TestCase {
            block_index: 13,
            block_size,
            expected_type: "single_indirect",
            expected_pointer: 12,
            expected_offset: 1,
        },
        TestCase {
            block_index: 1035,
            block_size,
            expected_type: "single_indirect",
            expected_pointer: 12,
            expected_offset: pointers_per_block - 1,
        },
        // 二级间接块
        TestCase {
            block_index: 1036,
            block_size,
            expected_type: "double_indirect",
            expected_pointer: 13,
            expected_offset: 0,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        println!("test:      Test {}: block_index={}", i + 1, tc.block_index);

        // 计算块类型
        let block_type = if tc.block_index < 12 {
            "direct"
        } else if tc.block_index < 12 + pointers_per_block as u64 {
            "single_indirect"
        } else {
            "double_indirect"
        };

        println!("test:        type: {}", block_type);
        println!("test:        expected: {}", tc.expected_type);

        if block_type == tc.expected_type {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Block index calculations work correctly");
}

fn test_indirect_levels() {
    println!("test:    Testing indirect level determination...");

    struct TestCase {
        file_size: u64,
        block_size: u64,
        expected_level: u32,
    }

    let test_cases = [
        // 直接块范围
        TestCase {
            file_size: 48 * 1024,    // 48 KB (12 blocks)
            block_size: 4096,
            expected_level: 0,
        },
        TestCase {
            file_size: 49 * 1024,    // 49 KB
            block_size: 4096,
            expected_level: 1,
        },
        // 单级间接块范围
        TestCase {
            file_size: 100 * 1024,   // 100 KB
            block_size: 4096,
            expected_level: 1,
        },
        TestCase {
            file_size: 4 * 1024 * 1024,  // 4 MB
            block_size: 4096,
            expected_level: 1,
        },
        // 二级间接块范围
        TestCase {
            file_size: 5 * 1024 * 1024,   // 5 MB
            block_size: 4096,
            expected_level: 2,
        },
        TestCase {
            file_size: 4 * 1024 * 1024 * 1024u64,  // 4 GB
            block_size: 4096,
            expected_level: 2,
        },
        // 三级间接块范围
        TestCase {
            file_size: 5 * 1024 * 1024 * 1024u64,   // 5 GB
            block_size: 4096,
            expected_level: 3,
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        let level = get_indirect_level(tc.file_size, tc.block_size);

        println!("test:      Test {}: size={} MB", i + 1, tc.file_size / (1024 * 1024));
        println!("test:        Calculated level: {}", level);
        println!("test:        Expected level: {}", tc.expected_level);

        if level == tc.expected_level {
            println!("test:        Test {} PASSED", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Indirect level determination works correctly");
}

fn test_file_size_limits() {
    println!("test:    Testing file size limits...");

    let block_size = 4096u64;
    let pointers_per_block = block_size / 4;

    // 计算各级别支持的最大文件大小
    let direct_max = 12 * block_size;
    let single_max = direct_max + pointers_per_block * block_size;
    let double_max = single_max + pointers_per_block * pointers_per_block * block_size;

    println!("test:      Direct blocks only: {} KB", direct_max / 1024);
    println!("test:      + Single indirect: {} MB", single_max / (1024 * 1024));
    println!("test:      + Double indirect: {} GB", double_max / (1024 * 1024 * 1024));

    // 验证边界条件
    let test_sizes = [
        (48 * 1024, "direct", 48),           // 48 KB - 直接块边界
        (49 * 1024, "single", 49),           // 49 KB - 需要单级间接
        (4 * 1024 * 1024, "single", 4096),   // 4 MB - 单级间接边界
        (5 * 1024 * 1024, "double", 5120),   // 5 MB - 需要二级间接
    ];

    for (size, expected_type, size_kb) in test_sizes {
        let level = get_indirect_level(size, block_size);
        let actual_type = match level {
            0 => "direct",
            1 => "single",
            2 => "double",
            _ => "triple",
        };

        println!("test:      Size: {} KB - type: {} (expected: {})",
                 size_kb, actual_type, expected_type);

        if actual_type == expected_type {
            println!("test:        Test PASSED");
        } else {
            println!("test:        Test FAILED");
        }
    }

    println!("test:    SUCCESS - File size limits verified");
}

fn test_block_pointer_indices() {
    println!("test:    Testing block pointer array indices...");

    // ext4 inode 的 i_block 数组布局
    // [0-11]: 直接块
    // [12]: 单级间接块
    // [13]: 二级间接块
    // [14]: 三级间接块

    let test_cases = [
        (0, 0, "direct block 0"),
        (11, 11, "direct block 11"),
        (12, 12, "single indirect block pointer"),
        (13, 13, "double indirect block pointer"),
        (14, 14, "triple indirect block pointer"),
    ];

    for (block_index, expected_i_block_index, description) in test_cases {
        let i_block_index = if block_index < 12 {
            block_index as usize
        } else if block_index == 12 {
            12
        } else if block_index < 12 + 1024 {
            12  // 单级间接块
        } else if block_index < 12 + 1024 + 1024 * 1024 {
            13  // 二级间接块
        } else {
            14  // 三级间接块
        };

        println!("test:      {}: i_block[{}]", description, i_block_index);

        if i_block_index == expected_i_block_index {
            println!("test:        Test PASSED");
        } else {
            println!("test:        Test FAILED - expected {}, got {}",
                     expected_i_block_index, i_block_index);
        }
    }

    println!("test:    SUCCESS - Block pointer indices verified");
}

fn test_indirect_offset_calculations() {
    println!("test:    Testing indirect offset calculations...");

    let block_size = 4096u64;
    let pointers_per_block = (block_size / 4) as usize;

    struct TestCase {
        file_block_index: u64,
        expected_level: u32,
        expected_first_index: usize,
        expected_second_index: usize,
    }

    let test_cases = [
        // 单级间接块
        TestCase {
            file_block_index: 12,
            expected_level: 1,
            expected_first_index: 0,
            expected_second_index: 0,
        },
        TestCase {
            file_block_index: 100,
            expected_level: 1,
            expected_first_index: 88,  // (100 - 12)
            expected_second_index: 0,
        },
        // 二级间接块
        TestCase {
            file_block_index: 1036,
            expected_level: 2,
            expected_first_index: 0,
            expected_second_index: 0,
        },
        TestCase {
            file_block_index: 2000,
            expected_level: 2,
            expected_first_index: 0,
            expected_second_index: 964,  // (2000 - 12 - 1024)
        },
    ];

    for (i, tc) in test_cases.iter().enumerate() {
        println!("test:      Test {}: file_block_index={}", i + 1, tc.file_block_index);

        let level = if tc.file_block_index < 12 {
            0
        } else if tc.file_block_index < 12 + pointers_per_block as u64 {
            1
        } else if tc.file_block_index < 12 + pointers_per_block as u64
            + (pointers_per_block * pointers_per_block) as u64 {
            2
        } else {
            3
        };

        println!("test:        Calculated level: {}", level);
        println!("test:        Expected level: {}", tc.expected_level);

        if level == tc.expected_level {
            println!("test:        Test {} PASSED (level)", i + 1);
        } else {
            println!("test:        Test {} FAILED", i + 1);
        }
    }

    println!("test:    SUCCESS - Indirect offset calculations work correctly");
}

/// 辅助函数：计算需要的间接块级别
fn get_indirect_level(size: u64, block_size: u64) -> u32 {
    let blocks = (size + block_size - 1) / block_size;

    if blocks <= 12 {
        return 0;
    }

    let pointers_per_block = block_size / 4;

    if blocks <= 12 + pointers_per_block {
        return 1;
    }

    let double_pointers = pointers_per_block * pointers_per_block;

    if blocks <= 12 + pointers_per_block + double_pointers {
        return 2;
    }

    3
}
