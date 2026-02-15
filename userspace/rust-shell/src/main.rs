/*
 * Rux OS Shell - Rust std 版本
 *
 * 使用标准库功能实现
 */

use std::io::{self, Write, BufRead};
use std::process::{Command, exit};

fn print_welcome() {
    println!();
    println!("========================================");
    println!("  Rux OS Shell v0.3 (Rust std)");
    println!("========================================");
    println!("Type 'help' for available commands");
    println!();
}

fn print_help() {
    println!("Rux OS Shell v0.3");
    println!("Available commands:");
    println!("  echo <args>  - Print arguments");
    println!("  help         - Show this help message");
    println!("  exit         - Exit the shell");
    println!("  pid          - Show process ID");
    println!("  <program>    - Execute external program");
    println!();
}

fn main() {
    print_welcome();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("rux> ");
        stdout.flush().unwrap();

        let mut line = String::new();
        if stdin.lock().read_line(&mut line).is_err() {
            break;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let args: Vec<&str> = line.split_whitespace().collect();
        if args.is_empty() {
            continue;
        }

        // 内置命令
        match args[0] {
            "echo" => {
                if args.len() > 1 {
                    println!("{}", args[1..].join(" "));
                } else {
                    println!();
                }
            }
            "help" => {
                print_help();
            }
            "exit" | "quit" => {
                println!("Goodbye!");
                exit(0);
            }
            "pid" => {
                println!("PID: {}", std::process::id());
            }
            _ => {
                // 尝试执行外部程序
                let path = if args[0].starts_with('/') || args[0].starts_with('.') {
                    args[0].to_string()
                } else {
                    format!("/bin/{}", args[0])
                };

                match Command::new(&path).args(&args[1..]).spawn() {
                    Ok(mut child) => {
                        let _ = child.wait();
                    }
                    Err(e) => {
                        println!("Failed to execute {}: {}", path, e);
                    }
                }
            }
        }
    }
}
