#!/bin/bash
set -e

# ============================================================
# compress.sh (GNU-only cross-platform edition)
#
# Supported platforms:
#   Linux / Android / iOS / macOS / BSD / Windows GNU
#
# IMPORTANT:
#   鉁?GNU toolchains only
#
# Compatible with:
#   build-images.sh
#   cross.toml (GNU-only mode)
# ============================================================


# ============================================================
# Supported targets (GNU ONLY)
# ============================================================

SUPPORTED_TARGETS=(

# =========================
# Linux GNU
# =========================
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
armv7-unknown-linux-gnueabihf
i686-unknown-linux-gnu

# =========================
# Linux MUSL (static)
# =========================
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
armv7-unknown-linux-musleabihf

# =========================
# Android (GNU-like toolchain via NDK)
# =========================
aarch64-linux-android
armv7-linux-androideabi
x86_64-linux-android
i686-linux-android

# =========================
# macOS
# =========================
x86_64-apple-darwin
aarch64-apple-darwin

# =========================
# iOS
# =========================
aarch64-apple-ios
aarch64-apple-ios-sim
x86_64-apple-ios

# =========================
# BSD
# =========================
x86_64-unknown-freebsd
aarch64-unknown-freebsd
x86_64-unknown-netbsd
x86_64-unknown-openbsd
x86_64-unknown-dragonfly

# =========================
# Windows GNU ONLY
# =========================
x86_64-pc-windows-gnu
i686-pc-windows-gnu
aarch64-pc-windows-gnullvm
)


DEFAULT_TARGET="x86_64-unknown-linux-gnu"
INPUT_TARGET="${1:-$DEFAULT_TARGET}"


# ============================================================
# Colors
# ============================================================

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'


# ============================================================
# Validate target
# ============================================================

validate_target() {
    local target="$1"

    if [[ "$target" == "all" ]]; then
        return 0
    fi

    for t in "${SUPPORTED_TARGETS[@]}"; do
        if [[ "$t" == "$target" ]]; then
            return 0
        fi
    done

    echo -e "${RED}ERROR: Unsupported target: $target${NC}"
    echo ""
    echo "MSVC is NOT supported (GNU-only toolchain)."
    exit 1
}

validate_target "$INPUT_TARGET"


# ============================================================
# Build target list
# ============================================================

if [[ "$INPUT_TARGET" == "all" ]]; then
    TARGETS=("${SUPPORTED_TARGETS[@]}")
else
    TARGETS=("$INPUT_TARGET")
fi


# ============================================================
# Detect binaries
# ============================================================

detect_binary_names() {
    local binaries=()

    for target in "${TARGETS[@]}"; do
        if [ -d "target/$target/release" ]; then
            while IFS= read -r binary; do
                if [ -n "$binary" ]; then
                    binaries+=("$(basename "$binary")")
                fi
            done < <(
                find "target/$target/release" \
                    -maxdepth 1 \
                    -type f \
                    -executable \
                    ! -name "*.d" \
                    ! -name "*.so" \
                    ! -name "*.a" \
                    ! -name "*.rlib" \
                    2>/dev/null
            )
        fi
    done

    printf "%s\n" "${binaries[@]}" | sort -u
}

BINARY_NAMES=$(detect_binary_names)


# fallback Cargo metadata
if [ -z "$BINARY_NAMES" ] && [ -f Cargo.toml ]; then
    BINARY_NAMES=$(cargo metadata --no-deps --format-version=1 2>/dev/null | \
        python3 -c "
import sys, json
data=json.load(sys.stdin)
for t in data['packages'][0]['targets']:
    if 'bin' in t['kind']:
        print(t['name'])
" 2>/dev/null)
fi


if [ -z "$BINARY_NAMES" ]; then
    echo -e "${RED}No binaries found.${NC}"
    exit 1
fi


echo -e "${GREEN}馃摝 Found binaries:${NC}"
echo "$BINARY_NAMES" | while read -r name; do
    echo -e "  ${BLUE}鈥?$name${NC}"
done
echo ""


# ============================================================
# Install UPX
# ============================================================

if ! command -v upx >/dev/null 2>&1; then
    echo -e "${YELLOW}Installing UPX...${NC}"
    sudo apt-get update
    sudo apt-get install -y upx-ucl || sudo apt-get install -y upx
fi


# ============================================================
# UPX support check
# ============================================================

supports_upx() {
    local target="$1"

    case "$target" in
        # Apple binaries often problematic
        x86_64-apple-darwin|aarch64-apple-darwin)
            return 1
            ;;
        aarch64-apple-ios|aarch64-apple-ios-sim|x86_64-apple-ios)
            return 1
            ;;
        *)
            return 0
            ;;
    esac
}


# ============================================================
# Compress binary
# ============================================================

compress_binary() {
    local target="$1"
    local binary_name="$2"
    local binary_path="target/$target/release/$binary_name"

    if [ ! -f "$binary_path" ]; then
        return 1
    fi

    if ! supports_upx "$target"; then
        echo -e "${YELLOW}Skip UPX (unsupported): $target${NC}"
        return 1
    fi

    local original_size
    original_size=$(stat -c%s "$binary_path")

    mkdir -p target/backups
    cp "$binary_path" "target/backups/${binary_name}_${target}_original"

    echo -e "${GREEN}Compressing $binary_name ($target)...${NC}"

    upx --best --lzma "$binary_path" >/dev/null 2>&1 || {
        echo -e "${RED}UPX failed: $binary_name ($target)${NC}"
        return 1
    }

    local compressed_size
    compressed_size=$(stat -c%s "$binary_path")

    local saved=$((original_size - compressed_size))
    local percent=$((saved * 100 / original_size))

    echo "  Original:   $((original_size / 1024)) KB"
    echo "  Compressed: $((compressed_size / 1024)) KB"
    echo "  Saved:      $((saved / 1024)) KB ($percent%)"
    echo ""

    return 0
}


# ============================================================
# Compress all
# ============================================================

compress_all() {
    local total=0
    local compressed=0
    local failed=0

    for target in "${TARGETS[@]}"; do
        echo -e "${YELLOW}=== Target: $target ===${NC}"

        while IFS= read -r binary_name; do
            [ -z "$binary_name" ] && continue

            total=$((total + 1))

            if compress_binary "$target" "$binary_name"; then
                compressed=$((compressed + 1))
            else
                failed=$((failed + 1))
            fi
        done <<< "$BINARY_NAMES"

        echo ""
    done

    echo -e "${GREEN}馃搳 Summary:${NC}"
    echo "  Total:      $total"
    echo "  Compressed: $compressed"
    echo "  Failed:     $failed"
}


# ============================================================
# Show sizes
# ============================================================

show_sizes() {
    echo ""
    echo -e "${YELLOW}馃搹 Final sizes:${NC}"

    for target in "${TARGETS[@]}"; do
        echo -e "${BLUE}$target:${NC}"

        while IFS= read -r binary_name; do
            [ -z "$binary_name" ] && continue

            local path="target/$target/release/$binary_name"
            if [ -f "$path" ]; then
                ls -lh "$path" | awk '{print "  " $9 " (" $5 ")"}'
            fi
        done <<< "$BINARY_NAMES"

        echo ""
    done
}


# ============================================================
# Main
# ============================================================

main() {
    compress_all
    show_sizes
    echo -e "${GREEN}鉁?Done! GNU-only compression completed.${NC}"
}

main