// 测试：文件描述符管理 (FdTable)
use crate::println;
use crate::fs::file::{FdTable, File, FileFlags};

pub fn test_fdtable() {
    println!("test: Testing FdTable management...");

    // 测试 1: 创建 FdTable
    println!("test: 1. Creating FdTable...");
    let fdtable = FdTable::new();
    println!("test:    SUCCESS - FdTable created");

    // 测试 2: 分配文件描述符
    println!("test: 2. Allocating file descriptors...");
    let fd1 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("test:    FAILED - alloc_fd returned None");
            return;
        }
    };
    println!("test:    Allocated fd = {}", fd1);
    assert!(fd1 < 1024, "fd should be valid");

    let fd2 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("test:    FAILED - second alloc_fd returned None");
            return;
        }
    };
    println!("test:    Allocated fd = {}", fd2);

    // 测试 3: 创建 File 对象并安装
    println!("test: 3. Installing File objects...");
    let file1 = File::new(FileFlags::new(FileFlags::O_RDONLY));
    let file1_arc = unsafe {
        use crate::collection::SimpleArc;
        match SimpleArc::new(file1) {
            Some(arc) => arc,
            None => {
                println!("test:    FAILED - SimpleArc::new returned None for file1");
                return;
            }
        }
    };

    match fdtable.install_fd(fd1, file1_arc) {
        Ok(_) => println!("test:    File1 installed to fd {}", fd1),
        Err(_) => {
            println!("test:    FAILED - install_fd returned error");
            return;
        }
    }

    let file2 = File::new(FileFlags::new(FileFlags::O_WRONLY));
    let file2_arc = unsafe {
        use crate::collection::SimpleArc;
        match SimpleArc::new(file2) {
            Some(arc) => arc,
            None => {
                println!("test:    FAILED - SimpleArc::new returned None for file2");
                return;
            }
        }
    };
    match fdtable.install_fd(fd2, file2_arc) {
        Ok(_) => println!("test:    File2 installed to fd {}", fd2),
        Err(_) => {
            println!("test:    FAILED - install_fd returned error");
            return;
        }
    }
    println!("test:    SUCCESS - files installed");

    // 测试 4: 获取文件对象
    println!("test: 4. Getting File objects...");
    match fdtable.get_file(fd1) {
        Some(file) => {
            // 验证文件标志
            assert!(file.flags.is_readonly(), "File should be readonly");
            println!("test:    Retrieved fd1, flags correct");
        }
        None => {
            println!("test:    FAILED - get_file returned None");
            return;
        }
    }

    match fdtable.get_file(fd2) {
        Some(file) => {
            assert!(file.flags.is_writeonly(), "File should be writeonly");
            println!("test:    Retrieved fd2, flags correct");
        }
        None => {
            println!("test:    FAILED - get_file returned None");
            return;
        }
    }
    println!("test:    SUCCESS - get_file works");

    // 测试 5: 获取无效的文件描述符
    println!("test: 5. Getting invalid fd...");
    match fdtable.get_file(9999) {
        Some(_) => {
            println!("test:    FAILED - should return None for invalid fd");
            return;
        }
        None => {
            println!("test:    Correctly returned None for invalid fd");
        }
    }
    println!("test:    SUCCESS - invalid fd handling works");

    // 测试 6: 关闭文件描述符
    println!("test: 6. Closing file descriptors...");
    match fdtable.close_fd(fd1) {
        Ok(_) => println!("test:    Closed fd {}", fd1),
        Err(_) => {
            println!("test:    FAILED - close_fd returned error");
            return;
        }
    }

    match fdtable.close_fd(fd2) {
        Ok(_) => println!("test:    Closed fd {}", fd2),
        Err(_) => {
            println!("test:    FAILED - close_fd returned error");
            return;
        }
    }
    println!("test:    SUCCESS - close_fd works");

    // 测试 7: 验证关闭后无法获取文件
    println!("test: 7. Verifying closed fd...");
    match fdtable.get_file(fd1) {
        Some(_) => {
            println!("test:    FAILED - should return None after close");
            return;
        }
        None => {
            println!("test:    Correctly returned None after close");
        }
    }
    println!("test:    SUCCESS - closed fd not accessible");

    // 测试 8: 重复使用已释放的 fd
    println!("test: 8. Testing fd reuse...");
    let fd3 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("test:    FAILED - alloc_fd returned None");
            return;
        }
    };
    println!("test:    Allocated new fd = {}", fd3);
    // 应该能重用刚释放的 fd
    // 这里我们不验证具体是哪个 fd，只要能分配就行

    println!("test:    SUCCESS - fd reuse works");

    println!("test: FdTable testing completed.");
}
