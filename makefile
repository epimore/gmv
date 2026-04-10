# ============================================================
# Makefile (GNU-only cross-platform build system)
#
# Windows GNU only
# aligned with compress.sh + cross.toml
# ============================================================

SHELL := /bin/bash

# ============================================================
# Targets (must match compress.sh)
# ============================================================

LINUX_TARGETS := \
	x86_64-unknown-linux-gnu \
	aarch64-unknown-linux-gnu \
	armv7-unknown-linux-gnueabihf \
	i686-unknown-linux-gnu

MUSL_TARGETS := \
	x86_64-unknown-linux-musl \
	aarch64-unknown-linux-musl \
	armv7-unknown-linux-musleabihf

ANDROID_TARGETS := \
	aarch64-linux-android \
	armv7-linux-androideabi \
	x86_64-linux-android \
	i686-linux-android

MACOS_TARGETS := \
	x86_64-apple-darwin \
	aarch64-apple-darwin

IOS_TARGETS := \
	aarch64-apple-ios \
	aarch64-apple-ios-sim \
	x86_64-apple-ios

BSD_TARGETS := \
	x86_64-unknown-freebsd \
	aarch64-unknown-freebsd \
	x86_64-unknown-netbsd \
	x86_64-unknown-openbsd \
	x86_64-unknown-dragonfly

WINDOWS_TARGETS := \
	x86_64-pc-windows-gnu \
	i686-pc-windows-gnu \
	aarch64-pc-windows-gnullvm

ALL_TARGETS := \
	$(LINUX_TARGETS) \
	$(MUSL_TARGETS) \
	$(ANDROID_TARGETS) \
	$(MACOS_TARGETS) \
	$(IOS_TARGETS) \
	$(BSD_TARGETS) \
	$(WINDOWS_TARGETS)

# ============================================================
# Default
# ============================================================

TARGET ?= x86_64-unknown-linux-gnu
CROSS ?= cross

# ============================================================
# Build (single target)
# ============================================================

build:
	$(CROSS) build --release --target $(TARGET)

# ============================================================
# Build groups
# ============================================================

linux:
	@for t in $(LINUX_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

musl:
	@for t in $(MUSL_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

android:
	@for t in $(ANDROID_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

macos:
	@for t in $(MACOS_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

ios:
	@for t in $(IOS_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

bsd:
	@for t in $(BSD_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

windows:
	@for t in $(WINDOWS_TARGETS); do \
		echo "Building $$t (GNU only)"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

# ============================================================
# Full build (ALL GNU-only targets)
# ============================================================

all:
	@for t in $(ALL_TARGETS); do \
		echo "Building $$t"; \
		$(CROSS) build --release --target $$t || exit 1; \
	done

# ============================================================
# Compression (uses your compress.sh)
# ============================================================

compress:
	@./compress.sh $(TARGET)

compress-all:
	@./compress.sh all

# ============================================================
# Clean
# ============================================================

clean:
	cargo clean
	rm -rf target/backups

# ============================================================
# CI helper
# ============================================================

ci: all compress-all
	@echo "CI build complete (GNU-only)"
