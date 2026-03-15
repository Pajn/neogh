#!/usr/bin/env bash

set -e

REPO="Pajn/neogh"
LUA_DIR="lua"
OS=$(uname -s)
ARCH=$(uname -m)

case "${OS}" in
    Linux*)
        TARGET="x86_64-unknown-linux-gnu"
        EXT="so"
        ;;
    Darwin*)
        if [ "${ARCH}" = "arm64" ]; then
            TARGET="aarch64-apple-darwin"
        else
            TARGET="x86_64-apple-darwin"
        fi
        EXT="dylib"
        ;;
    *)
        echo "Unsupported OS: ${OS}"
        exit 1
        ;;
esac

BINARY_NAME="neogh-${TARGET}.${EXT}"
URL="https://github.com/${REPO}/releases/download/nightly/${BINARY_NAME}"

echo "Downloading ${BINARY_NAME} from ${URL}..."

mkdir -p "${LUA_DIR}"
if command -v curl >/dev/null 2>&1; then
    curl -L -o "${LUA_DIR}/neogh.so" "${URL}"
elif command -v wget >/dev/null 2>&1; then
    wget -O "${LUA_DIR}/neogh.so" "${URL}"
else
    echo "Neither curl nor wget found. Please install one of them."
    exit 1
fi

echo "Successfully installed neogh.so"
