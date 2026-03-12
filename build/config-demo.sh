#!/bin/bash
# Quick configuration demo - shows how the configuration system works

# Get project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo "=========================================="
echo "  Rux Kernel Configuration System Demo"
echo "=========================================="
echo ""

echo "1. Current configuration info:"
echo "----------------------------------------"
echo "Kernel name: $(grep '^name' Kernel.toml | head -1 | cut -d'"' -f2)"
echo "Version: $(grep '^version' Kernel.toml | head -1 | cut -d'"' -f2)"
echo "Target platform: $(grep '^default_platform' Kernel.toml | head -1 | cut -d'"' -f2)"
echo ""

echo "2. Modifying configuration..."
echo "----------------------------------------"

# Temporarily modify configuration
sed -i 's/^name = "Rux"/name = "RuxOS Demo"/' Kernel.toml
sed -i 's/^version = "0.1.0"/version = "0.2.0"/' Kernel.toml

echo "✓ Name changed to: RuxOS Demo"
echo "✓ Version changed to: 0.2.0"
echo ""

echo "3. Recompiling kernel..."
echo "------------------------------------------"
cargo build --target aarch64-unknown-none 2>&1 | grep -E "(Compiling|Finished)" | tail -2
echo ""

echo "4. View generated configuration code:"
echo "------------------------------------------"
echo "Kernel name constant: $(grep '^pub const KERNEL_NAME' kernel/src/config.rs | cut -d'"' -f2)"
echo "Version constant: $(grep '^pub const KERNEL_VERSION' kernel/src/config.rs | cut -d'"' -f2)"
echo ""

echo "5. Restoring original configuration..."
echo "------------------------------------------"
sed -i 's/^name = "RuxOS Demo"/name = "Rux"/' Kernel.toml
sed -i 's/^version = "0.2.0"/version = "0.1.0"/' Kernel.toml
cargo build --target aarch64-unknown-none >/dev/null 2>&1
echo "✓ Configuration restored"
echo ""

echo "=========================================="
echo "  Configuration system demo complete!"
echo "=========================================="
echo ""
echo "Usage:"
echo "  1. Edit Kernel.toml file"
echo "  2. Run: cargo build --target aarch64-unknown-none"
echo "  3. Configuration will be compiled into the kernel"
echo ""
echo "Interactive configuration:"
echo "  Run: cd build && make menuconfig"
echo ""
