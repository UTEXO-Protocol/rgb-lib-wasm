#!/bin/bash
# Set up WASI SDK for compiling C code to WASM

set -e

echo "🔧 Setting up WASI SDK for WASM build..."

# OS check
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "📦 macOS detected"
    
    # WASI SDK install path
    WASI_SDK_DIR="$HOME/.local/wasi-sdk"
    # Try several versions (newest to oldest)
    WASI_SDK_VERSIONS=("29.0" "28.0" "27.0" "26.0" "25.0" "24.0" "23.0" "22.0" "21.0" "20" "19" "18" "17" "16" "20.0" "19.0")
    
    # Detect architecture
    ARCH_TYPE=$(uname -m)
    if [[ "$ARCH_TYPE" == "arm64" ]]; then
        ARCH_SUFFIX="arm64-macos"
    else
        ARCH_SUFFIX="macos"
    fi
    
    # Check for WASI SDK
    if [ -d "$WASI_SDK_DIR" ] && [ -f "$WASI_SDK_DIR/bin/clang" ]; then
        echo "✅ WASI SDK already installed at $WASI_SDK_DIR"
    else
        echo "📥 Downloading WASI SDK..."
        
        mkdir -p "$HOME/.local"
        
        # Check if URL is reachable
        check_url() {
            if command -v curl &> /dev/null; then
                HTTP_CODE=$(curl -sL -o /dev/null -w "%{http_code}" "$1")
                [ "$HTTP_CODE" = "200" ]
            elif command -v wget &> /dev/null; then
                wget --spider -q "$1" 2>/dev/null
            else
                return 1
            fi
        }
        
        download_file() {
            if command -v curl &> /dev/null; then
                curl -L -f "$1" -o "$2"
            elif command -v wget &> /dev/null; then
                wget "$1" -O "$2"
            else
                return 1
            fi
        }
        
        # Try different versions and URL formats
        WASI_SDK_URL=""
        WASI_SDK_VERSION=""
        
        for version in "${WASI_SDK_VERSIONS[@]}"; do
            # Different filename patterns per version
            URL_PATTERNS=(
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-${ARCH_SUFFIX}.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-11.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-12.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-13.tar.gz"
            )
            
            for url in "${URL_PATTERNS[@]}"; do
                echo "🔍 Checking: $url"
                if check_url "$url"; then
                    WASI_SDK_URL="$url"
                    WASI_SDK_VERSION="$version"
                    echo "✅ Found working URL: $url"
                    break 2
                fi
            done
        done
        
        if [ -z "$WASI_SDK_URL" ]; then
            echo "❌ Could not find a working download URL"
            echo "💡 Try downloading manually:"
            echo "   1. Open https://github.com/WebAssembly/wasi-sdk/releases"
            echo "   2. Download the latest macOS build"
            echo "   3. Extract to $WASI_SDK_DIR"
            echo "   4. Set environment variables (see below)"
            exit 1
        fi
        
        echo "📥 Downloading from $WASI_SDK_URL..."
        if ! download_file "$WASI_SDK_URL" /tmp/wasi-sdk.tar.gz; then
            echo "❌ Download failed"
            exit 1
        fi
        
        # File size check (should be > 1MB)
        FILE_SIZE=$(stat -f%z /tmp/wasi-sdk.tar.gz 2>/dev/null || stat -c%s /tmp/wasi-sdk.tar.gz 2>/dev/null || echo "0")
        if [ "$FILE_SIZE" -lt 1048576 ]; then
            echo "❌ Downloaded file too small (possible 404)"
            rm -f /tmp/wasi-sdk.tar.gz
            echo "💡 Try manual download from https://github.com/WebAssembly/wasi-sdk/releases"
            exit 1
        fi
        
        echo "📦 Extracting..."
        if ! tar -xzf /tmp/wasi-sdk.tar.gz -C "$HOME/.local" 2>/dev/null; then
            echo "❌ Extract failed"
            rm -f /tmp/wasi-sdk.tar.gz
            exit 1
        fi
        rm /tmp/wasi-sdk.tar.gz
        
        # Find and rename extracted directory
        EXTRACTED_DIR=$(find "$HOME/.local" -maxdepth 1 -type d -name "wasi-sdk-*" | head -1)
        if [ -n "$EXTRACTED_DIR" ] && [ "$EXTRACTED_DIR" != "$WASI_SDK_DIR" ]; then
            if [ -d "$WASI_SDK_DIR" ]; then
                rm -rf "$WASI_SDK_DIR"
            fi
            mv "$EXTRACTED_DIR" "$WASI_SDK_DIR"
        fi
        
        echo "✅ WASI SDK installed"
    fi
    
    # Set environment
    if [ -d "$WASI_SDK_DIR" ] && [ -f "$WASI_SDK_DIR/bin/clang" ]; then
        export CC_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/clang"
        export AR_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/llvm-ar"
        export CFLAGS_wasm32_unknown_unknown="--target=wasm32-wasi"
        
        echo "✅ WASI SDK configured: $WASI_SDK_DIR"
        echo ""
        echo "📝 Add to ~/.zshrc or ~/.bashrc:"
        echo "export CC_wasm32_unknown_unknown=\"$WASI_SDK_DIR/bin/clang\""
        echo "export AR_wasm32_unknown_unknown=\"$WASI_SDK_DIR/bin/llvm-ar\""
        echo "export CFLAGS_wasm32_unknown_unknown=\"--target=wasm32-wasi\""
    else
        echo "❌ WASI SDK not found after install"
        echo "💡 Try manual download from https://github.com/WebAssembly/wasi-sdk/releases"
        exit 1
    fi
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "📦 Linux detected"
    echo "⚠️  On Linux download wasi-sdk manually:"
    echo "   1. Download from https://github.com/WebAssembly/wasi-sdk/releases"
    echo "   2. Extract to /opt/wasi-sdk"
    echo "   3. Set environment:"
    echo "      export CC_wasm32_unknown_unknown=/opt/wasi-sdk/bin/clang"
    echo "      export AR_wasm32_unknown_unknown=/opt/wasi-sdk/bin/llvm-ar"
    echo "      export CFLAGS_wasm32_unknown_unknown=\"--target=wasm32-wasi\""
    exit 1
else
    echo "❌ Unsupported OS: $OSTYPE"
    exit 1
fi

echo ""
echo "✅ Setup complete!"
echo ""
echo "🧪 Compilation check:"
echo "   cd bindings/wasm"
echo "   cargo check --target wasm32-unknown-unknown"
