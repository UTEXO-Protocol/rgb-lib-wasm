#!/bin/bash
# Build WASM and run HTTP server.
# Usage: ./build-and-serve.sh [port]
#        ./build-and-serve.sh debug [port] — build with --dev for panic backtrace.
# Default port 8000.

set -e

BUILD_DEBUG=""
PORT=""
for arg in "$@"; do
    if [ "$arg" = "debug" ]; then
        BUILD_DEBUG=1
    elif [ -z "$PORT" ] && [ -n "${arg##*[!0-9]*}" ]; then
        PORT="$arg"
    fi
done
PORT="${PORT:-8000}"

cd "$(dirname "$0")"

# Dependencies
if ! command -v wasm-pack &> /dev/null; then
    echo "❌ wasm-pack not installed. Install: cargo install wasm-pack"
    exit 1
fi
if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "❌ target not installed. Install: rustup target add wasm32-unknown-unknown"
    exit 1
fi

[ -f "./setup-env.sh" ] && source ./setup-env.sh

# Build WASM (patches from root Cargo.toml apply to workspace automatically)
echo "📦 wasm-pack build --target web --out-dir pkg"
rm -rf pkg
if [ -n "$BUILD_DEBUG" ]; then
    wasm-pack build --target web --out-dir pkg --dev
else
    wasm-pack build --target web --out-dir pkg
fi
echo "✅ WASM built"
echo ""

# Browser polyfill (import 'env' from C code) — remove when build no longer needs it
[ -x "./patch-pkg-env.sh" ] && ./patch-pkg-env.sh || [ -f "./patch-pkg-env.sh" ] && bash ./patch-pkg-env.sh
echo ""

echo "🌐 Server: http://localhost:$PORT"
echo "   Test: http://localhost:$PORT/examples/simple-test.html"
echo "   Stop: Ctrl+C"
echo ""
exec python3 -m http.server "$PORT"
