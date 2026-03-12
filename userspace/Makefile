# Rux User Programs Makefile
#
# Build user-space programs and generate ELF binaries

.PHONY: all clean help list hello_world

# Set environment variables to override root directory's cargo config
# Use user program's linker script instead of kernel's linker script
export CARGO_TARGET_RISCV64GC_UNKNOWN_NONE_ELF_RUSTFLAGS := \
	-Clink-arg=-T$(CURDIR)/user.ld \
	-Cforce-frame-pointers=yes \
	-Copt-level=z \
	-Clink-arg=--gc-sections

# Default target: build all user programs
all: hello_world

# List all user programs
list:
	@echo "Rux User Programs List:"
	@echo "  - hello_world: Hello World sample program"
	@echo ""
	@echo "Build commands:"
	@echo "  make           - Build all programs"
	@echo "  make hello_world - Build hello_world program"
	@echo "  make clean     - Clean build artifacts"

# Help information
help: list
	@echo ""
	@echo "Output location:"
	@echo "  target/riscv64gc-unknown-none-elf/release/<program>"

# Build hello_world
hello_world:
	cargo build --release --bin hello_world --manifest-path $(CURDIR)/Cargo.toml
	@echo "✓ Build complete: target/riscv64gc-unknown-none-elf/release/hello_world"
	@ls -lh target/riscv64gc-unknown-none-elf/release/hello_world

# Clean build artifacts
clean:
	cargo clean --manifest-path $(CURDIR)/Cargo.toml
	@echo "✓ Clean complete"

# Display build information
info:
	@echo "RISC-V target: riscv64gc-unknown-none-elf"
	@echo "Workspace members: hello_world"
	@echo "Linker script: user.ld"
	@echo ""
	cargo --version
	rustc --version
