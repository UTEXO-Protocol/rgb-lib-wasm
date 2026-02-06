#!/bin/bash
# Скрипт для настройки переменных окружения WASI SDK

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WASI_SDK_DIR="$SCRIPT_DIR/wasi-sdk-29.0-arm64-macos"

if [ ! -d "$WASI_SDK_DIR" ]; then
    echo "❌ WASI SDK не найден в $WASI_SDK_DIR"
    echo "💡 Распакуйте архив: tar -xzf wasi-sdk-29.0-arm64-macos.tar.gz"
    exit 1
fi

if [ ! -f "$WASI_SDK_DIR/bin/clang" ]; then
    echo "❌ clang не найден в $WASI_SDK_DIR/bin/clang"
    exit 1
fi

# Set compiler for wasm32-unknown-unknown target
export CC_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/clang"
export AR_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/llvm-ar"
export CFLAGS_wasm32_unknown_unknown="--target=wasm32-wasi --sysroot=$WASI_SDK_DIR/share/wasi-sysroot"

# cc-rs also checks these variables
export TARGET_CC="$WASI_SDK_DIR/bin/clang"
export TARGET_AR="$WASI_SDK_DIR/bin/llvm-ar"

# For cc-rs to find the right compiler, we need to set it when building for wasm32-unknown-unknown
# The CC_wasm32_unknown_unknown should be enough, but if not, we can also set:
export CC="$WASI_SDK_DIR/bin/clang"
export AR="$WASI_SDK_DIR/bin/llvm-ar"

echo "✅ Переменные окружения установлены:"
echo "   CC_wasm32_unknown_unknown=$CC_wasm32_unknown_unknown"
echo "   AR_wasm32_unknown_unknown=$AR_wasm32_unknown_unknown"
echo "   CFLAGS_wasm32_unknown_unknown=$CFLAGS_wasm32_unknown_unknown"
echo ""
echo "📝 Для постоянной настройки добавьте в ~/.zshrc:"
echo "   source $SCRIPT_DIR/setup-env.sh"
echo ""
echo "🧪 Проверка компиляции:"
echo "   cargo check --target wasm32-unknown-unknown"
