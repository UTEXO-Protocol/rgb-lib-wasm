#!/bin/bash
# Quick WASM build test script

set -e

echo "🧪 Testing WASM build of rgb-lib"
echo ""

cd "$(dirname "$0")"

if ! command -v wasm-pack &> /dev/null; then
    echo "❌ wasm-pack not installed!"
    echo "   Install: cargo install wasm-pack"
    exit 1
fi

if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "❌ wasm32-unknown-unknown target not installed!"
    echo "   Install: rustup target add wasm32-unknown-unknown"
    exit 1
fi

echo "🔧 Setting up environment..."
if [ -f "./setup-env.sh" ]; then
    source ./setup-env.sh
    echo "✅ Environment variables set"
else
    echo "⚠️  setup-env.sh not found. Continuing without it..."
fi

echo ""
echo "📦 Step 1: Compilation check..."
echo "   cargo check --target wasm32-unknown-unknown -p rgb-lib-wasm"
echo ""

# Run cargo check only for WASM package (avoids building root rgb-lib with electrum → amplify conflict)
cargo check --target wasm32-unknown-unknown -p rgb-lib-wasm 2>&1 | tee /tmp/wasm-check.log
CHECK_EXIT_CODE=${PIPESTATUS[0]}

ERROR_COUNT=$(grep -c "^error\[" /tmp/wasm-check.log 2>/dev/null || echo "0")
COMPILE_ERROR_COUNT=$(grep -c "error: could not compile" /tmp/wasm-check.log 2>/dev/null || echo "0")

if [ "$CHECK_EXIT_CODE" -ne 0 ] || [ "$ERROR_COUNT" -gt 0 ] || [ "$COMPILE_ERROR_COUNT" -gt 0 ]; then
    echo ""
    echo "❌ Compilation errors found!"
    echo ""
    if [ "$ERROR_COUNT" -gt 0 ]; then
        echo "   Total errors: $ERROR_COUNT"
    fi
    if [ "$COMPILE_ERROR_COUNT" -gt 0 ]; then
        echo "   Crates that failed to compile: $COMPILE_ERROR_COUNT"
    fi
    echo ""
    echo "🔍 Main issues:"
    grep -E "error: could not compile" /tmp/wasm-check.log | head -5
    echo ""
    echo "💡 Fix compilation errors first."
    echo "   See PROGRESS.md for current status"
    exit 1
fi

echo ""
echo "✅ Compilation succeeded!"
echo ""
echo "📦 Step 2: Building WASM module..."
echo "   wasm-pack build --target web --out-dir pkg"
echo ""

# Unset CC/AR for host so wasm-pack (when installing wasm-bindgen-cli) does not use WASI clang (no macOS headers → ring fails with TargetConditionals.h).
# CC_wasm32_unknown_unknown stays in environment and is used when building for wasm32.
unset CC AR TARGET_CC TARGET_AR CFLAGS 2>/dev/null || true

if wasm-pack build --target web --out-dir pkg; then
    [ -f "./patch-pkg-env.sh" ] && chmod +x ./patch-pkg-env.sh 2>/dev/null; ./patch-pkg-env.sh
    echo ""
    echo "✅ WASM module built successfully!"
    echo ""
    echo "📁 Output: bindings/wasm/pkg/"
    echo ""
    echo "🌐 To test in browser:"
    echo "   1. Start server from bindings/wasm (not from pkg!):"
    echo "      cd bindings/wasm"
    echo "      python3 -m http.server 8000"
    echo "   2. Open in browser: http://localhost:8000/examples/simple-test.html"
    echo ""
else
    echo ""
    echo "❌ WASM module build failed!"
    echo "   Check the logs above"
    exit 1
fi
