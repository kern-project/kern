#!/bin/bash
set -e

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

# 假设你把 release 传到了 GitHub Releases
DOWNLOAD_URL="https://github.com/softfault/kern/releases/download/${VERSION}/${TARBALL}"

KERN_HOME="${HOME}/.kern"
KERN_BIN="${KERN_HOME}/bin"

echo "Welcome to the Kern Programming Language Installer!"

# 1. 创建目标目录
echo "=> Creating installation directory at ${KERN_HOME}..."
mkdir -p "${KERN_HOME}"

# 2. 下载并解压工具链
echo "=> Downloading Kern ${VERSION} for ${TARGET}..."
# 本地测试时可以注释掉 curl，直接用 cp 本地的 tar.gz 来模拟
curl -L -# -o "/tmp/${TARBALL}" "${DOWNLOAD_URL}"
# 用本地刚才生成的压缩包替代
# cp "./${TARBALL}" "/tmp/${TARBALL}"

echo "=> Extracting toolchain..."
tar -xzf "/tmp/${TARBALL}" -C "/tmp"
# 把解压出来的 bin 和 lib 移动到 ~/.kern 目录
cp -r "/tmp/kern-${VERSION}-${TARGET}/"* "${KERN_HOME}/"
rm -rf "/tmp/${TARBALL}" "/tmp/kern-${VERSION}-${TARGET}"

# 3. 配置环境变量 (PATH)
echo "=> Configuring PATH..."
RC_FILE=""
if [ -n "$BASH_VERSION" ]; then
    RC_FILE="${HOME}/.bashrc"
elif [ -n "$ZSH_VERSION" ]; then
    RC_FILE="${HOME}/.zshrc"
else
    # 兜底 fallback
    RC_FILE="${HOME}/.profile"
fi

if ! grep -q "${KERN_BIN}" "${RC_FILE}"; then
    echo "" >> "${RC_FILE}"
    echo "# Kern Programming Language" >> "${RC_FILE}"
    echo "export PATH=\"${KERN_BIN}:\$PATH\"" >> "${RC_FILE}"
    echo "Added ${KERN_BIN} to your PATH in ${RC_FILE}."
    echo "Please run 'source ${RC_FILE}' or restart your terminal to apply changes."
else
    echo "${KERN_BIN} is already in your PATH."
fi

echo ""
echo "Kern ${VERSION} installed successfully!"
echo "Run 'kernc --version' to verify."