//! 测试 SimpleArc 分配 - 最简化版本

use crate::println;

pub fn test_arc_alloc() {
    println!("test: Testing SimpleArc allocation (minimal)...");

    // 测试 1: 分配单个 File 对象
    println!("test: 1. Allocating single File...");
    use crate::fs::file::{File, FileFlags};
    use crate::collection::SimpleArc;

    let file1 = File::new(FileFlags::new(FileFlags::O_RDONLY));
    println!("test:    File created on stack");

    let file1_arc = unsafe {
        match SimpleArc::new(file1) {
            Some(arc) => {
                println!("test:    File1 Arc created at ptr={:#x}", arc.as_ptr() as usize);
                arc
            }
            None => {
                println!("test:    FAILED - SimpleArc::new returned None");
                return;
            }
        }
    };
    println!("test:    SUCCESS - File1 allocated");

    // 测试 2: 通过 Arc 访问数据
    println!("test: 2. Accessing data through Arc...");
    let flags1 = file1_arc.as_ref();
    println!("test:    File1 flags: {:?}", flags1.flags);
    println!("test:    SUCCESS - Data access works");

    println!("test: SimpleArc allocation test completed.");
}
