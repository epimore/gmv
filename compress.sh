#!/bin/bash

# compress.sh - 压缩所有已构建的二进制文件（高级版）

# 配置
TARGETS=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
    "armv7-unknown-linux-gnueabihf"
)

# 手动指定要压缩的二进制（如果不自动检测）
# 取消注释并修改这里
# MANUAL_BINARIES=("gmv" "gmv-cli" "gmv-tool")

# 颜色输出
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 自动检测所有二进制名称
detect_binary_names() {
    local binaries=()

    if [ ${#MANUAL_BINARIES[@]} -gt 0 ]; then
        # 使用手动指定的列表
        printf "%s\n" "${MANUAL_BINARIES[@]}"
        return
    fi

    # 从已构建的二进制查找所有可执行文件
    for target in "${TARGETS[@]}"; do
        if [ -d "target/$target/release" ]; then
            while IFS= read -r binary; do
                if [ -n "$binary" ]; then
                    name=$(basename "$binary")
                    binaries+=("$name")
                fi
            done < <(find "target/$target/release" -maxdepth 1 -type f -executable ! -name "*.d" ! -name "*.so" ! -name "*.a" 2>/dev/null)
        fi
    done

    # 去重并输出
    printf "%s\n" "${binaries[@]}" | sort -u
}

# 获取所有二进制名称
BINARY_NAMES=$(detect_binary_names)

# 如果没有找到任何二进制，尝试从 Cargo.toml 读取
if [ -z "$BINARY_NAMES" ] && [ -f "Cargo.toml" ]; then
    # 获取所有 bin 目标
    BINARY_NAMES=$(cargo metadata --no-deps --format-version=1 2>/dev/null | \
        python3 -c "import sys, json; data=json.load(sys.stdin); [print(t['name']) for t in data['packages'][0]['targets'] if t['kind'] == ['bin']]" 2>/dev/null)

    # 如果没有 bin 目标，获取项目名称
    if [ -z "$BINARY_NAMES" ]; then
        BINARY_NAMES=$(grep -m1 '^name = ' Cargo.toml | sed 's/name = "\(.*\)"/\1/')
    fi
fi

# 如果还是没有，使用当前目录名
if [ -z "$BINARY_NAMES" ]; then
    BINARY_NAMES=$(basename $(pwd))
fi

echo -e "${GREEN}📦 Found binaries:${NC}"
echo "$BINARY_NAMES" | while read -r name; do
    echo -e "  ${BLUE}• $name${NC}"
done
echo ""

# 检查 UPX 是否安装
if ! command -v upx &> /dev/null; then
    echo -e "${YELLOW}UPX not found. Installing...${NC}"
    sudo apt-get update && sudo apt-get install -y upx
    echo -e "${GREEN}UPX installed successfully${NC}"
    echo ""
fi

# 压缩函数
compress_binary() {
    local target=$1
    local binary_name=$2
    local binary_path="target/$target/release/$binary_name"

    if [ -f "$binary_path" ]; then
        local original_size=$(stat -c%s "$binary_path")
        echo -e "${GREEN}Compressing $binary_name for $target...${NC}"

        # 备份原始文件
        mkdir -p "target/backups"
        cp "$binary_path" "target/backups/${binary_name}_${target}_original"

        # 压缩
        upx --best --lzma "$binary_path" 2>/dev/null

        local compressed_size=$(stat -c%s "$binary_path")
        local saved=$((original_size - compressed_size))
        local percent=$((saved * 100 / original_size))

        echo -e "  Original:  $((original_size / 1024)) KB"
        echo -e "  Compressed: $((compressed_size / 1024)) KB"
        echo -e "  Saved: $((saved / 1024)) KB ($percent%)"
        echo ""
        return 0
    fi
    return 1
}

# 压缩所有二进制
compress_all() {
    local compressed=0
    local total=0
    local failed=0

    for target in "${TARGETS[@]}"; do
        echo -e "${YELLOW}=== Target: $target ===${NC}"

        while IFS= read -r binary_name; do
            if [ -n "$binary_name" ]; then
                total=$((total + 1))
                if compress_binary "$target" "$binary_name"; then
                    compressed=$((compressed + 1))
                else
                    failed=$((failed + 1))
                fi
            fi
        done <<< "$BINARY_NAMES"

        echo ""
    done

    echo -e "${GREEN}📊 Summary:${NC}"
    echo -e "  Total: $total"
    echo -e "  Compressed: $compressed"
    echo -e "  Not found: $failed"
}

# 显示大小
show_sizes() {
    echo -e "\n${YELLOW}📏 Final binary sizes:${NC}"

    for target in "${TARGETS[@]}"; do
        found=false
        while IFS= read -r binary_name; do
            if [ -n "$binary_name" ]; then
                binary_path="target/$target/release/$binary_name"
                if [ -f "$binary_path" ]; then
                    found=true
                    break
                fi
            fi
        done <<< "$BINARY_NAMES"

        if [ "$found" = true ]; then
            echo -e "${BLUE}$target:${NC}"
            while IFS= read -r binary_name; do
                if [ -n "$binary_name" ]; then
                    binary_path="target/$target/release/$binary_name"
                    if [ -f "$binary_path" ]; then
                        ls -lh "$binary_path" | awk '{print "  " $9 " (" $5 ")"}'
                    fi
                fi
            done <<< "$BINARY_NAMES"
            echo ""
        fi
    done
}

# 主函数
main() {
    compress_all
    show_sizes
    echo -e "${GREEN}✨ Done!${NC}"
}

# 运行
main