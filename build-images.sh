#!/bin/bash
set -e

# ==========================================================
# Universal Cross Image Builder
#
# Supports:
#   Linux GNU
#   Linux MUSL
#   Android
#   macOS
#   iOS
#   BSD
#   Windows GNU
#
# Default:
#   ./build-images.sh
#   -> x86_64-unknown-linux-gnu:latest
#
# Example:
#   ./build-images.sh aarch64-linux-android
#   ./build-images.sh x86_64-pc-windows-gnu:v1.0
#
# ==========================================================


# ==========================================================
# Default values
# ==========================================================

DEFAULT_TARGET="x86_64-unknown-linux-gnu"
DEFAULT_VERSION="latest"

INPUT="${1:-$DEFAULT_TARGET:$DEFAULT_VERSION}"

if [[ "$INPUT" == *:* ]]; then
    TARGET="${INPUT%%:*}"
    VERSION="${INPUT##*:}"
else
    TARGET="$INPUT"
    VERSION="$DEFAULT_VERSION"
fi


# ==========================================================
# Supported Targets
# ==========================================================

SUPPORTED_TARGETS=(

# =========================
# Linux GNU
# =========================

# x86_64-unknown-linux-gnu
#   - Ubuntu / Debian / CentOS / RHEL x86_64
x86_64-unknown-linux-gnu

# aarch64-unknown-linux-gnu
#   - ARM64 Linux
#   - Raspberry Pi 4/5 64-bit
#   - AWS Graviton / Ampere ARM server
aarch64-unknown-linux-gnu

# armv7-unknown-linux-gnueabihf
#   - 32-bit ARM hard-float Linux
#   - Raspberry Pi 2/3
armv7-unknown-linux-gnueabihf

# i686-unknown-linux-gnu
#   - 32-bit x86 Linux
i686-unknown-linux-gnu


# =========================
# Linux MUSL Static
# =========================

# x86_64-unknown-linux-musl
#   - Fully static x86_64 Linux binary
x86_64-unknown-linux-musl

# aarch64-unknown-linux-musl
#   - Fully static ARM64 Linux binary
aarch64-unknown-linux-musl

# armv7-unknown-linux-musleabihf
#   - Fully static ARMv7 hard-float Linux
armv7-unknown-linux-musleabihf


# =========================
# Android
# =========================

# aarch64-linux-android
#   - Android ARM64 devices
#   - Modern phones/tablets
aarch64-linux-android

# armv7-linux-androideabi
#   - Android ARMv7 legacy devices
armv7-linux-androideabi

# x86_64-linux-android
#   - Android x86_64 emulator / Intel devices
x86_64-linux-android

# i686-linux-android
#   - Android x86 legacy emulator
i686-linux-android


# =========================
# macOS
# =========================

# x86_64-apple-darwin
#   - Intel Mac
x86_64-apple-darwin

# aarch64-apple-darwin
#   - Apple Silicon Mac (M1/M2/M3)
aarch64-apple-darwin


# =========================
# iOS
# =========================

# aarch64-apple-ios
#   - Physical iPhone / iPad devices
aarch64-apple-ios

# aarch64-apple-ios-sim
#   - Apple Silicon iOS Simulator
aarch64-apple-ios-sim

# x86_64-apple-ios
#   - Intel Mac iOS Simulator
x86_64-apple-ios


# =========================
# BSD
# =========================

# x86_64-unknown-freebsd
#   - FreeBSD Intel x86_64
x86_64-unknown-freebsd

# aarch64-unknown-freebsd
#   - FreeBSD ARM64
aarch64-unknown-freebsd

# x86_64-unknown-netbsd
#   - NetBSD Intel x86_64
x86_64-unknown-netbsd

# x86_64-unknown-openbsd
#   - OpenBSD Intel x86_64
x86_64-unknown-openbsd

# x86_64-unknown-dragonfly
#   - DragonFly BSD
x86_64-unknown-dragonfly


# =========================
# Windows GNU
# =========================

# x86_64-pc-windows-gnu
#   - Windows 64-bit MinGW
x86_64-pc-windows-gnu

# i686-pc-windows-gnu
#   - Windows 32-bit MinGW
i686-pc-windows-gnu

# aarch64-pc-windows-gnullvm
#   - Windows ARM64 GNU LLVM
aarch64-pc-windows-gnullvm
)


# ==========================================================
# Validate target
# ==========================================================

VALID=false
for t in "${SUPPORTED_TARGETS[@]}"; do
    if [[ "$t" == "$TARGET" ]]; then
        VALID=true
        break
    fi
done

if [[ "$VALID" != true ]]; then
    echo "ERROR: Unsupported target: $TARGET"
    echo ""
    echo "Supported targets:"
    for t in "${SUPPORTED_TARGETS[@]}"; do
        echo "  - $t"
    done
    exit 1
fi


# ==========================================================
# Build image
# ==========================================================

IMAGE_NAME="ffmpeg-cross-$TARGET:$VERSION"

echo "=========================================================="
echo "Building Cross Native Image"
echo "----------------------------------------------------------"
echo "Target : $TARGET"
echo "Version: $VERSION"
echo "Image  : $IMAGE_NAME"
echo "=========================================================="

docker build \
    --build-arg TARGET="$TARGET" \
    -t "$IMAGE_NAME" \
    -f Dockerfile.ffmpeg-base \
    .

echo ""
echo "Done building: $IMAGE_NAME"
echo ""


# ==========================================================
# Notes
# ==========================================================

case "$TARGET" in
    *apple*)
        echo "NOTE: Apple targets require osxcross / Apple SDK integration."
        ;;
    *android*)
        echo "NOTE: Android targets require Android NDK integration."
        ;;
    *windows-gnu|*gnullvm)
        echo "NOTE: Windows GNU targets require mingw-w64 toolchain."
        ;;
esac