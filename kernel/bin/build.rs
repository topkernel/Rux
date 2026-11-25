//! bin build script (crate-flip M3-B): the BIN package owns the
//! final kernel link, so the link-specific steps moved here from
//! kernel/build.rs verbatim (linker scripts, boot.S compilation,
//! .incbin DTB handling). The lib package's build.rs keeps the config
//! generation. TARGET/CARGO_MANIFEST_DIR are relative to THIS
//! package's dir: kernel/bin — the paths below adjust by ../../.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    emit_linker_scripts();
    compile_boot_asm();
}

/// Apply the arch linker script(s) to this bin's link, exactly once.
///
/// The -T link-args used to live in .cargo/config.toml. That is not
/// idempotent under cargo's config discovery: cargo CONCATENATES
/// [target.<triple>] rustflags arrays across every ancestor
/// .cargo/config.toml, and every Rux worktree carries that tracked file —
/// so a build run from a checkout nested inside another worktree (the lane
/// `run/<base>/` pattern) applied each script TWICE. A twice-applied
/// x86_64 linker.ld resets `. = BOOT_BASE` (0x100000) and merges a second
/// .boot_stack allocation over the first pass's real .boot contents:
/// rust-lld: "section .boot virtual address range overlaps with .boot_stack"
/// (reproduced deterministically at 912995089 and at main 0c343b043; the
/// old scripts only *happened* to tolerate it on riscv64/aarch64).
///
/// Emitting from here is single-sourced: a build script runs once per
/// build no matter how many ancestor configs cargo merged. For the
/// mixed-version nesting case (a fixed checkout inside an OLDER worktree
/// whose config still carries -T), the effective rustflags are inspected
/// via CARGO_ENCODED_RUSTFLAGS (cargo passes build scripts the merged
/// flag list) and this script defers to the already-applied -T.
fn emit_linker_scripts() {
    let target = env::var("TARGET").unwrap_or_default();

    // (repo-relative script path) per kernel target triple, in -T order.
    let scripts: &[&str] = if target.contains("x86_64-unknown-none") {
        &[
            "kernel/core/src/arch/x86_64/linker.ld",
            "kernel/core/src/arch/x86_64/linker-extra.ld",
        ]
    } else if target.contains("riscv64gc-unknown-none-elf") {
        &["kernel/core/src/arch/riscv64/linker.ld"]
    } else if target.contains("aarch64-unknown-none") {
        &["kernel/core/src/arch/aarch64/linker.ld"]
    } else {
        return; // host/verify targets: no kernel link
    };

    // Repo root (workspace root) is the parent of kernel/ (CARGO_MANIFEST_DIR).
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // bin sits INSIDE kernel/: repo root is two parents up.
    let root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("bin manifest dir has no grandparent")
        .to_path_buf();

    for script in scripts {
        let abs = root.join(script);
        assert!(
            abs.is_file(),
            "linker script missing: {} (target {})",
            abs.display(),
            target
        );
        match rustflags_script_count(script) {
            0 => println!("cargo:rustc-link-arg=-T{}", abs.display()),
            // An ancestor config (older worktree) already applies this
            // exact script; a second application re-breaks the layout.
            1 => {}
            // Two pre-fix configs merged (e.g. a shared ancestor plus an
            // older wrapping worktree): the doubled -T is already on the
            // rustc command line and cannot be removed from here. Say so
            // instead of failing with a bare rust-lld overlap error.
            n => println!(
                "cargo:warning=-T {} applied {}x by merged .cargo configs \
                 (x86_64 will fail to link: .boot/.boot_stack overlap). \
                 Build from a checkout whose .cargo/config.toml carries no \
                 link-arg=-T (post link-fix), or unset the extra config.",
                script, n
            ),
        }
    }
}

/// Compile boot.S assembly file separately and link it as the first object.
/// This ensures _start is at the kernel's load address (physical address).
/// We cannot use global_asm! because it renames sections, preventing
/// proper placement in the linker script (Linux-style VMA/LMA boot).
fn compile_boot_asm() {
    let target = env::var("TARGET").unwrap_or_default();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    if target.contains("x86_64") {
        // x86_64 boot stub: multiboot1 entry assembled with the host
        // assembler. The object must be elf64 to link into the kernel
        // image; the file itself starts in .code32 and switches to
        // .code64, so only the output format is selected here.
        let boot_asm = PathBuf::from(&manifest_dir).join("../core/src/arch/x86_64/boot.S");
        if !boot_asm.exists() {
            return;
        }
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let boot_obj = out_dir.join("boot_x86_64.o");
        if boot_obj.exists() {
            let asm_time = fs::metadata(&boot_asm).and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let obj_time = fs::metadata(&boot_obj).and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if asm_time <= obj_time {
                println!("cargo:rustc-link-arg={}", boot_obj.display());
                return;
            }
        }
        let status = std::process::Command::new("as")
            .arg("--64")
            .arg("-o").arg(&boot_obj)
            .arg(&boot_asm)
            .status()
            .expect("failed to run assembler for x86 boot.S");
        assert!(status.success(), "failed to assemble x86 boot.S");
        println!("cargo:rustc-link-arg={}", boot_obj.display());
        println!("cargo:rerun-if-changed={}", boot_asm.display());
        return;
    }

    if target.contains("aarch64") {
        // aarch64 boot: assembled with the cross toolchain like the riscv
        // branch (the object must be elf64 and land first in the link so
        // _start sits at the image head / physical load address).
        let boot_asm = PathBuf::from(&manifest_dir).join("../core/src/arch/aarch64/boot/boot.S");
        if !boot_asm.exists() {
            return;
        }
        println!("cargo:rerun-if-changed={}", boot_asm.display());

        // Embed the QEMU virt DTB (firmware-independent: QEMU's ELF -kernel
        // boot loads no DTB and zeroes the argument registers). Runs on
        // EVERY invocation so a stale boot object can never skip it.
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let dtb_src = PathBuf::from(&manifest_dir).join("../core/src/arch/aarch64/virt.dtb");
        // Fresh-clone support: the blob is a build input git does not
        // carry (*.dtb is ignored). Regenerate from the installed QEMU —
        // the dump flags must mirror the boot driver's machine config
        // (memory@ sizes track -m; the fixed MMIO bases are stable).
        if !dtb_src.exists() {
            let dump_arg = format!("dumpdtb={}", dtb_src.display());
            let ok = std::process::Command::new("qemu-system-aarch64")
                .args(["-M", "virt,gic-version=3", "-cpu", "cortex-a72", "-m", "1G", "-smp", "2"])
                .arg("-machine").arg(&dump_arg)
                .status().map(|s| s.success()).unwrap_or(false);
            assert!(
                ok && dtb_src.exists(),
                "kernel/core/src/arch/aarch64/virt.dtb missing and could not be dumped: \
                 install qemu-system-aarch64 or place the blob there"
            );
            println!("cargo:warning=generated aarch64 virt.dtb from the installed QEMU");
        }
        // The blob is .incbin'd, so a regenerated/edited virt.dtb must
        // rerun the build script — otherwise the embedded DTB goes stale
        // with zero diagnostics.
        if dtb_src.exists() {
            println!("cargo:rerun-if-changed={}", dtb_src.display());
        }
        // Generated asm lives in OUT_DIR (never the source tree): the
        // old in-tree dtb_blob.S embedded each build's absolute path,
        // so every worktree's build rewrote a tracked file — the
        // recurring merge-conflict and stray-diff generator.
        let dtb_asm = out_dir.join("dtb_blob.S");
        fs::write(&dtb_asm, format!(
            ".section .rodata\n.balign 8\n.globl aarch64_virt_dtb\naarch64_virt_dtb:\n.incbin \"{}\"\n.globl aarch64_virt_dtb_end\naarch64_virt_dtb_end:\n",
            dtb_src.display())).expect("write dtb_blob.S");
        let dtb_obj = out_dir.join("dtb_aarch64.o");
        let status = std::process::Command::new("aarch64-linux-gnu-gcc")
            .arg("-c").arg(&dtb_asm).arg("-o").arg(&dtb_obj).status()
            .expect("run assembler for dtb_blob.S");
        assert!(status.success(), "failed to assemble dtb_blob.S");
        println!("cargo:rustc-link-arg={}", dtb_obj.display());

        let boot_obj = out_dir.join("boot_aarch64.o");

        if boot_obj.exists() {
            let asm_time = fs::metadata(&boot_asm).and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let obj_time = fs::metadata(&boot_obj).and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if asm_time <= obj_time {
                println!("cargo:rustc-link-arg={}", boot_obj.display());
                return;
            }
        }

        let status = std::process::Command::new("aarch64-linux-gnu-gcc")
            .arg("-c")
            .arg("-g")
            .arg(&boot_asm)
            .arg("-o")
            .arg(&boot_obj)
            .status()
            .expect("failed to run assembler for aarch64 boot.S (is gcc-aarch64-linux-gnu installed?)");
        assert!(status.success(), "failed to assemble aarch64 boot.S");
        println!("cargo:rustc-link-arg={}", boot_obj.display());
        return;
    }

    if !target.contains("riscv64") {
        return;
    }

    let boot_asm = PathBuf::from(&manifest_dir)
        .join("../core/src/arch/riscv64/boot.S");

    if !boot_asm.exists() {
        return;
    }

    // Without this, cargo never reruns the build script for a riscv
    // boot.S-only edit (the x86 branch below declares its own): boot.o
    // stays stale while rust code relinks around it, and the mismatched
    // early page tables boot-panic past the old image window (LAYOUT-4:
    // pfault at the 18MB boundary with 20MB layout constants, batchA6).
    println!("cargo:rerun-if-changed={}", boot_asm.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let boot_obj = out_dir.join("boot.o");

    // Only recompile if boot.S is newer than boot.o
    if boot_obj.exists() {
        let asm_time = fs::metadata(&boot_asm)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let obj_time = fs::metadata(&boot_obj)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if asm_time <= obj_time {
            // Already up to date
            println!("cargo:rustc-link-arg={}", boot_obj.display());
            return;
        }
    }

    // Compile boot.S to boot.o using the system assembler
    // -c: compile only, -g: generate debug info
    let status = std::process::Command::new("riscv64-linux-gnu-gcc")
        .arg("-c")
        .arg("-g")
        .arg(&boot_asm)
        .arg("-o")
        .arg(&boot_obj)
        .status()
        .expect("Failed to execute assembler");

    if !status.success() {
        panic!("Failed to compile boot.S");
    }

    // Tell cargo to link boot.o (will be placed before Rust objects)
    println!("cargo:rustc-link-arg={}", boot_obj.display());
    // Rerun if boot.S changes
    println!("cargo:rerun-if-changed={}", boot_asm.display());
}

/// How many times the effective rustflags apply a `link-arg=-T` for
/// `script` (repo-relative, forward slashes). Understands both token
/// shapes cargo produces: "-C" + "link-arg=-T<path>" (config arrays,
/// \x1f-separated in CARGO_ENCODED_RUSTFLAGS) and the joined
/// "-Clink-arg=-T<path>" form of a hand-set RUSTFLAGS variable.
fn rustflags_script_count(script: &str) -> usize {
    let flags = env::var("CARGO_ENCODED_RUSTFLAGS")
        .or_else(|_| env::var("RUSTFLAGS"))
        .unwrap_or_default();
    let sep = if flags.contains('\u{1f}') { '\u{1f}' } else { ' ' };
    flags.split(sep)
        .filter(|tok| {
            let tok = tok.trim().trim_start_matches("-C");
            match tok.strip_prefix("link-arg=-T") {
                Some(path) => path.replace('\\', "/").ends_with(script),
                None => false,
            }
        })
        .count()
}
