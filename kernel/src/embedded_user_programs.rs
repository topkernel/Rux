//! 嵌入的用户程序
//!
/// 这个文件由 embed_user_programs.sh 自动生成
///
/// 包含预编译的用户程序 ELF 二进制文件

/// 嵌入的 hello_world 用户程序 (ELF 格式)
///
/// 注意：这个文件会占用约 6KB 内核空间
#[cfg(feature = "riscv64")]
pub static HELLO_WORLD_ELF: &[u8] = include_bytes!("../../userspace/target/riscv64gc-unknown-none-elf/release/hello_world");

/// hello_world ELF 文件大小
#[cfg(feature = "riscv64")]
pub const HELLO_WORLD_SIZE: usize = 6024;

/// 嵌入的 shell 用户程序 (ELF 格式)
///
/// Shell 会自动调用 execve 执行 hello_world
#[cfg(feature = "riscv64")]
pub static SHELL_ELF: &[u8] = include_bytes!("../../userspace/target/riscv64gc-unknown-none-elf/release/shell");

/// shell ELF 文件大小
#[cfg(feature = "riscv64")]
pub const SHELL_SIZE: usize = 6624;
