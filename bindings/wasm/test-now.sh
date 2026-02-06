#!/bin/bash
# Скрипт для быстрого тестирования WASM сборки
# ВАЖНО: Сначала нужно исправить оставшиеся 15 ошибок компиляции!

set -e

echo "🧪 Тестирование WASM сборки rgb-lib"
echo ""

# Переход в директорию bindings/wasm
cd "$(dirname "$0")"

# Проверка наличия wasm-pack
if ! command -v wasm-pack &> /dev/null; then
    echo "❌ wasm-pack не установлен!"
    echo "   Установите: cargo install wasm-pack"
    exit 1
fi

# Проверка наличия wasm32-unknown-unknown target
if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "❌ wasm32-unknown-unknown target не установлен!"
    echo "   Установите: rustup target add wasm32-unknown-unknown"
    exit 1
fi

# Настройка окружения
echo "🔧 Настройка окружения..."
if [ -f "./setup-env.sh" ]; then
    source ./setup-env.sh
    echo "✅ Переменные окружения установлены"
else
    echo "⚠️  setup-env.sh не найден. Продолжаем без него..."
fi

echo ""
echo "📦 Шаг 1: Проверка компиляции..."
echo "   cargo check --target wasm32-unknown-unknown -p rgb-lib-wasm"
echo ""

# Запускаем cargo check только для WASM-пакета (избегаем сборки root rgb-lib с electrum → конфликт amplify)
cargo check --target wasm32-unknown-unknown -p rgb-lib-wasm 2>&1 | tee /tmp/wasm-check.log
CHECK_EXIT_CODE=${PIPESTATUS[0]}

# Проверяем наличие ошибок в выводе
ERROR_COUNT=$(grep -c "^error\[" /tmp/wasm-check.log 2>/dev/null || echo "0")
COMPILE_ERROR_COUNT=$(grep -c "error: could not compile" /tmp/wasm-check.log 2>/dev/null || echo "0")

if [ "$CHECK_EXIT_CODE" -ne 0 ] || [ "$ERROR_COUNT" -gt 0 ] || [ "$COMPILE_ERROR_COUNT" -gt 0 ]; then
    echo ""
    echo "❌ Обнаружены ошибки компиляции!"
    echo ""
    if [ "$ERROR_COUNT" -gt 0 ]; then
        echo "   Всего ошибок: $ERROR_COUNT"
    fi
    if [ "$COMPILE_ERROR_COUNT" -gt 0 ]; then
        echo "   Не скомпилировано крейтов: $COMPILE_ERROR_COUNT"
    fi
    echo ""
    echo "🔍 Основные проблемы:"
    grep -E "error: could not compile" /tmp/wasm-check.log | head -5
    echo ""
    echo "💡 Сначала нужно исправить ошибки компиляции!"
    echo "   См. PROGRESS.md для текущего статуса"
    exit 1
fi

echo ""
echo "✅ Компиляция успешна!"
echo ""
echo "📦 Шаг 2: Сборка WASM модуля..."
echo "   wasm-pack build --target web --out-dir pkg"
echo ""

# Сбрасываем CC/AR для хоста, чтобы wasm-pack при установке wasm-bindgen-cli
# не использовал WASI clang (у него нет macOS-заголовков → ring падает с TargetConditionals.h).
# CC_wasm32_unknown_unknown остаётся в окружении и используется при сборке под wasm32.
unset CC AR TARGET_CC TARGET_AR CFLAGS 2>/dev/null || true

if wasm-pack build --target web --out-dir pkg; then
    [ -f "./patch-pkg-env.sh" ] && chmod +x ./patch-pkg-env.sh 2>/dev/null; ./patch-pkg-env.sh
    echo ""
    echo "✅ WASM модуль собран успешно!"
    echo ""
    echo "📁 Результаты находятся в: bindings/wasm/pkg/"
    echo ""
    echo "🌐 Для тестирования в браузере:"
    echo "   1. Запустите сервер из папки bindings/wasm (не из pkg!):"
    echo "      cd bindings/wasm"
    echo "      python3 -m http.server 8000"
    echo "   2. Откройте в браузере: http://localhost:8000/examples/simple-test.html"
    echo ""
else
    echo ""
    echo "❌ Ошибка при сборке WASM модуля!"
    echo "   Проверьте логи выше"
    exit 1
fi
