#!/bin/bash
# Build script for rgb-lib-wasm

set -e

echo "🔧 Building rgb-lib-wasm for WebAssembly..."

# Check if wasm32-unknown-unknown target is installed
if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "📦 Installing wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

# Check if wasm-pack is installed
if ! command -v wasm-pack &> /dev/null; then
    echo "⚠️  wasm-pack not found. Installing..."
    cargo install wasm-pack
fi

cd "$(dirname "$0")"

echo "🔍 Checking dependencies..."
cargo check --target wasm32-unknown-unknown

echo "📦 Building for browser/web..."
wasm-pack build --target bundler --features "esplora" --out-dir pkg-web

echo "📦 Building for Node.js..."
wasm-pack build --target nodejs --features "esplora" --out-dir pkg-node

echo "✅ Build complete!"
echo ""
echo "📁 Output directories:"
echo "   - pkg-web/  (for browsers)"
echo "   - pkg-node/ (for Node.js)"
