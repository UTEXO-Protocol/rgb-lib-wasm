#!/bin/bash
# Set up WASI SDK environment variables

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WASI_SDK_DIR="$SCRIPT_DIR/wasi-sdk-29.0-arm64-macos"

if [ ! -d "$WASI_SDK_DIR" ]; then
    echo "❌ WASI SDK not found at $WASI_SDK_DIR"
    echo "💡 Extract archive: tar -xzf wasi-sdk-29.0-arm64-macos.tar.gz"
    exit 1
fi

if [ ! -f "$WASI_SDK_DIR/bin/clang" ]; then
    echo "❌ clang not found at $WASI_SDK_DIR/bin/clang"
    exit 1
fi

export WASI_SDK_DIR

# cc-rs passes --target=wasm32-unknown-unknown; WASI clang only understands wasm32-wasi.
# Wrapper rewrites the triple and adds sysroot.
WRAPPER="$SCRIPT_DIR/clang-wasm32-wrapper.sh"
[ -f "$WRAPPER" ] && chmod +x "$WRAPPER" 2>/dev/null || true
if [ -x "$WRAPPER" ]; then
    export CC_wasm32_unknown_unknown="$WRAPPER"
    export TARGET_CC="$WRAPPER"
    export CC="$WRAPPER"
else
    export CC_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/clang"
    export TARGET_CC="$WASI_SDK_DIR/bin/clang"
    export CC="$WASI_SDK_DIR/bin/clang"
fi
export AR_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/llvm-ar"
export TARGET_AR="$WASI_SDK_DIR/bin/llvm-ar"
export AR="$WASI_SDK_DIR/bin/llvm-ar"
export CFLAGS_wasm32_unknown_unknown="-isysroot $WASI_SDK_DIR/share/wasi-sysroot"

echo "✅ Environment set (WASI SDK + clang wrapper for wasm32):"
echo "   CC_wasm32_unknown_unknown=$CC_wasm32_unknown_unknown"
echo "   AR_wasm32_unknown_unknown=$AR_wasm32_unknown_unknown"
echo ""
echo "📝 To persist, add to ~/.zshrc:"
echo "   source $SCRIPT_DIR/setup-env.sh"
echo ""
echo "🧪 Compilation check:"
echo "   cargo check --target wasm32-unknown-unknown"
