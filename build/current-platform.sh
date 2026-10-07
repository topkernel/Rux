#!/bin/bash
# Print the active build platform (Linux-style: .config wins over the
# shipped default in Kernel.toml). Used by the Makefile to pick the cargo
# target/features for plain `make` — platform choice lives in the config,
# not in Makefile targets.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OVERRIDE="${PLATFORM:-}"

platform=""
if [ -f "$ROOT/build/.config" ]; then
    platform=$(sed -n 's/^platform_default_platform=//p' "$ROOT/build/.config" | head -1 | tr -d '[:space:]')
fi
if [ -z "$platform" ] && [ -f "$ROOT/Kernel.toml" ]; then
    platform=$(sed -n '/^\[platform\]/,/^\[/p' "$ROOT/Kernel.toml" \
        | sed -n 's/^default_platform[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
fi
[ -z "$platform" ] && platform="riscv64"

if [ -n "$OVERRIDE" ]; then
    if [ "$OVERRIDE" != "riscv64" ] && [ "$OVERRIDE" != "x86_64" ]; then
        echo "error: unsupported PLATFORM='$OVERRIDE' (riscv64|x86_64)" >&2
        exit 1
    fi
    platform="$OVERRIDE"
fi

case "$platform" in
    riscv64) echo "$platform" ;;
    x86_64) echo "error: x86_64 support lives on the feature/x86-64 branch" >&2; exit 1 ;;
    *) echo "error: invalid platform '$platform' in config (riscv64)" >&2; exit 1 ;;
esac
