# Makefile
.PHONY: all build-x86_64 build-aarch64 build-armv7 clean

# 定义所有 target
TARGETS := x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf

# 构建所有镜像
all: $(foreach target,$(TARGETS),build-$(target))

# 构建单个 target 镜像
build-%:
	@echo "Building FFmpeg image for $*..."
	docker build \
		--build-arg TARGET=$* \
		-t ffmpeg-cross-$*:latest \
		-f Dockerfile.ffmpeg-base \
		.

# 构建特定架构
build-x86_64: build-x86_64-unknown-linux-gnu
build-aarch64: build-aarch64-unknown-linux-gnu
build-armv7: build-armv7-unknown-linux-gnueabihf

# 清理镜像
clean:
	@for target in $(TARGETS); do \
		echo "Removing ffmpeg-cross-$$target:latest"; \
		docker rmi ffmpeg-cross-$$target:latest 2>/dev/null || true; \
	done

# 一键构建并运行 cross
build-and-run-x86: build-x86_64
	cross build --target x86_64-unknown-linux-gnu --release

build-and-run-aarch64: build-aarch64
	cross build --target aarch64-unknown-linux-gnu --release

build-and-run-armv7: build-armv7
	cross build --target armv7-unknown-linux-gnueabihf --release