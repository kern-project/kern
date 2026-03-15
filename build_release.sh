#!/bin/bash
set -e # 遇到错误立即退出

VERSION="v0.4.0"
# 动态探测操作系统
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
if [ "$OS_NAME" = "linux" ]; then
    OS="linux-gnu"
elif [ "$OS_NAME" = "darwin" ]; then
    OS="apple-darwin"
else
    echo "Unsupported OS: $OS_NAME"
    exit 1
fi

# 动态探测 CPU 架构
ARCH=$(uname -m)
if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    ARCH="aarch64"
elif [ "$ARCH" = "x86_64" ] || [ "$ARCH" = "amd64" ]; then
    ARCH="x86_64"
else
    echo "Unsupported Architecture: $ARCH"
    exit 1
fi

# 拼装最终的 Target Triple
TARGET="${ARCH}-${OS}"
DIST_NAME="kern-${VERSION}-${TARGET}"
TARBALL="${DIST_NAME}.tar.gz"

echo "Building kernc release version..."
cargo build --release

echo "Creating distribution folder structure..."
rm -rf "${DIST_NAME}"
mkdir -p "${DIST_NAME}/bin"
mkdir -p "${DIST_NAME}/lib/kern"

echo "Copying artifacts..."
# 1. 拷贝编译器二进制
cp target/release/kernc "${DIST_NAME}/bin/"
# 2. 拷贝标准库源码
cp -r library/std "${DIST_NAME}/lib/kern/"
# 3. 拷贝协议和文档
cp README.md LICENSE "${DIST_NAME}/"

echo "Compressing to tarball..."
tar -czf "${DIST_NAME}.tar.gz" "${DIST_NAME}"
rm -rf "${DIST_NAME}"

echo "Success! Release artifact created: ${DIST_NAME}.tar.gz"