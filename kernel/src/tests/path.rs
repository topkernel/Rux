// 测试：Path 路径解析功能
use crate::println;
use crate::fs::path::Path;

pub fn test_path() {
    println!("test: Testing Path parsing...");

    // 测试 1: 绝对路径检查
    println!("test: 1. Testing is_absolute...");
    assert!(Path::new("/usr/bin").is_absolute(), "Should be absolute");
    assert!(Path::new("/").is_absolute(), "Root should be absolute");
    assert!(!Path::new("relative/path").is_absolute(), "Should not be absolute");
    println!("test:    SUCCESS - is_absolute works");

    // 测试 2: 空路径检查
    println!("test: 2. Testing is_empty...");
    assert!(Path::new("").is_empty(), "Empty string should be empty");
    assert!(!Path::new("/").is_empty(), "Root should not be empty");
    assert!(!Path::new("path").is_empty(), "Path should not be empty");
    println!("test:    SUCCESS - is_empty works");

    // 测试 3: 父目录获取
    println!("test: 3. Testing parent...");
    let parent1 = Path::new("/usr/bin").parent();
    assert!(parent1.is_some() && parent1.unwrap().as_str() == "/usr", "Parent of /usr/bin should be /usr");
    let parent2 = Path::new("/usr").parent();
    assert!(parent2.is_some() && parent2.unwrap().as_str() == "/", "Parent of /usr should be /");
    let parent3 = Path::new("/").parent();
    assert!(parent3.is_some() && parent3.unwrap().as_str() == "/", "Parent of / should be /");
    let parent4 = Path::new("file.txt").parent();
    assert!(parent4.is_none(), "Relative path without / should have None parent");
    println!("test:    SUCCESS - parent works");

    // 测试 4: 文件名获取
    println!("test: 4. Testing file_name...");
    assert_eq!(Path::new("/usr/bin/bash").file_name(), Some("bash"), "File name should be bash");
    assert_eq!(Path::new("/usr/bin/").file_name(), None, "Trailing / should return None");
    assert_eq!(Path::new("/file.txt").file_name(), Some("file.txt"), "File name should be file.txt");
    assert_eq!(Path::new("file.txt").file_name(), Some("file.txt"), "Relative file name should work");
    assert_eq!(Path::new("").file_name(), None, "Empty path should return None");
    println!("test:    SUCCESS - file_name works");

    // 测试 5: as_str
    println!("test: 5. Testing as_str...");
    assert_eq!(Path::new("/usr/bin").as_str(), "/usr/bin", "as_str should return original");
    assert_eq!(Path::new("").as_str(), "", "Empty as_str should work");
    println!("test:    SUCCESS - as_str works");

    println!("test: Path parsing testing completed.");
}
