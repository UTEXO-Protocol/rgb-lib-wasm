#!/bin/bash
# Скрипт для настройки WASI SDK для компиляции C кода в WASM

set -e

echo "🔧 Настройка WASI SDK для компиляции WASM..."

# Проверка ОС
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "📦 macOS обнаружен"
    
    # Путь для установки WASI SDK
    WASI_SDK_DIR="$HOME/.local/wasi-sdk"
    # Попробуем несколько версий (от новых к старым)
    WASI_SDK_VERSIONS=("29.0" "28.0" "27.0" "26.0" "25.0" "24.0" "23.0" "22.0" "21.0" "20" "19" "18" "17" "16" "20.0" "19.0")
    
    # Определение архитектуры
    ARCH_TYPE=$(uname -m)
    if [[ "$ARCH_TYPE" == "arm64" ]]; then
        ARCH_SUFFIX="arm64-macos"
    else
        ARCH_SUFFIX="macos"
    fi
    
    # Проверка наличия WASI SDK
    if [ -d "$WASI_SDK_DIR" ] && [ -f "$WASI_SDK_DIR/bin/clang" ]; then
        echo "✅ WASI SDK уже установлен в $WASI_SDK_DIR"
    else
        echo "📥 Скачивание WASI SDK..."
        
        # Создание директории
        mkdir -p "$HOME/.local"
        
        # Функция для проверки доступности URL
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
        
        # Функция для скачивания
        download_file() {
            if command -v curl &> /dev/null; then
                curl -L -f "$1" -o "$2"
            elif command -v wget &> /dev/null; then
                wget "$1" -O "$2"
            else
                return 1
            fi
        }
        
        # Пробуем разные версии и форматы URL
        WASI_SDK_URL=""
        WASI_SDK_VERSION=""
        
        for version in "${WASI_SDK_VERSIONS[@]}"; do
            # Разные форматы имен файлов для разных версий
            URL_PATTERNS=(
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-${ARCH_SUFFIX}.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-11.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-12.tar.gz"
                "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}-macos-13.tar.gz"
            )
            
            for url in "${URL_PATTERNS[@]}"; do
                echo "🔍 Проверка: $url"
                if check_url "$url"; then
                    WASI_SDK_URL="$url"
                    WASI_SDK_VERSION="$version"
                    echo "✅ Найден доступный URL: $url"
                    break 2
                fi
            done
        done
        
        if [ -z "$WASI_SDK_URL" ]; then
            echo "❌ Не удалось найти доступный URL для скачивания"
            echo "💡 Попробуйте скачать вручную:"
            echo "   1. Откройте https://github.com/WebAssembly/wasi-sdk/releases"
            echo "   2. Скачайте последнюю версию для macOS"
            echo "   3. Распакуйте в $WASI_SDK_DIR"
            echo "   4. Установите переменные окружения (см. ниже)"
            exit 1
        fi
        
        # Скачивание
        echo "📥 Скачивание с $WASI_SDK_URL..."
        if ! download_file "$WASI_SDK_URL" /tmp/wasi-sdk.tar.gz; then
            echo "❌ Ошибка при скачивании"
            exit 1
        fi
        
        # Проверка размера файла (должен быть больше 1MB)
        FILE_SIZE=$(stat -f%z /tmp/wasi-sdk.tar.gz 2>/dev/null || stat -c%s /tmp/wasi-sdk.tar.gz 2>/dev/null || echo "0")
        if [ "$FILE_SIZE" -lt 1048576 ]; then
            echo "❌ Скачанный файл слишком маленький (возможно, ошибка 404)"
            rm -f /tmp/wasi-sdk.tar.gz
            echo "💡 Попробуйте скачать вручную с https://github.com/WebAssembly/wasi-sdk/releases"
            exit 1
        fi
        
        echo "📦 Распаковка..."
        if ! tar -xzf /tmp/wasi-sdk.tar.gz -C "$HOME/.local" 2>/dev/null; then
            echo "❌ Ошибка при распаковке архива"
            rm -f /tmp/wasi-sdk.tar.gz
            exit 1
        fi
        rm /tmp/wasi-sdk.tar.gz
        
        # Поиск и переименование директории
        EXTRACTED_DIR=$(find "$HOME/.local" -maxdepth 1 -type d -name "wasi-sdk-*" | head -1)
        if [ -n "$EXTRACTED_DIR" ] && [ "$EXTRACTED_DIR" != "$WASI_SDK_DIR" ]; then
            if [ -d "$WASI_SDK_DIR" ]; then
                rm -rf "$WASI_SDK_DIR"
            fi
            mv "$EXTRACTED_DIR" "$WASI_SDK_DIR"
        fi
        
        echo "✅ WASI SDK установлен"
    fi
    
    # Настройка переменных окружения
    if [ -d "$WASI_SDK_DIR" ] && [ -f "$WASI_SDK_DIR/bin/clang" ]; then
        export CC_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/clang"
        export AR_wasm32_unknown_unknown="$WASI_SDK_DIR/bin/llvm-ar"
        export CFLAGS_wasm32_unknown_unknown="--target=wasm32-wasi"
        
        echo "✅ WASI SDK настроен: $WASI_SDK_DIR"
        echo ""
        echo "📝 Добавьте в ~/.zshrc или ~/.bashrc:"
        echo "export CC_wasm32_unknown_unknown=\"$WASI_SDK_DIR/bin/clang\""
        echo "export AR_wasm32_unknown_unknown=\"$WASI_SDK_DIR/bin/llvm-ar\""
        echo "export CFLAGS_wasm32_unknown_unknown=\"--target=wasm32-wasi\""
    else
        echo "❌ WASI SDK не найден после установки"
        echo "💡 Попробуйте скачать вручную с https://github.com/WebAssembly/wasi-sdk/releases"
        exit 1
    fi
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "📦 Linux обнаружен"
    echo "⚠️  Для Linux нужно скачать wasi-sdk вручную:"
    echo "   1. Скачайте с https://github.com/WebAssembly/wasi-sdk/releases"
    echo "   2. Распакуйте в /opt/wasi-sdk"
    echo "   3. Установите переменные окружения:"
    echo "      export CC_wasm32_unknown_unknown=/opt/wasi-sdk/bin/clang"
    echo "      export AR_wasm32_unknown_unknown=/opt/wasi-sdk/bin/llvm-ar"
    echo "      export CFLAGS_wasm32_unknown_unknown=\"--target=wasm32-wasi\""
    exit 1
else
    echo "❌ Неподдерживаемая ОС: $OSTYPE"
    exit 1
fi

echo ""
echo "✅ Настройка завершена!"
echo ""
echo "🧪 Проверка компиляции:"
echo "   cd bindings/wasm"
echo "   cargo check --target wasm32-unknown-unknown"
