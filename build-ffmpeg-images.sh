#!/bin/bash

# 为不同 target 构建 FFmpeg 镜像
targets=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
    "armv7-unknown-linux-gnueabihf"
)

for target in "${targets[@]}"; do
    echo "Building FFmpeg image for $target..."

    docker build \
        --build-arg TARGET=$target \
        -t ffmpeg-cross-$target:latest \
        -f Dockerfile.ffmpeg-base \
        .

    echo "Done building for $target"
done