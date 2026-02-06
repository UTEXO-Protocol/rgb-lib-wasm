#!/bin/bash
# Сборка WASM, патч pkg, запуск HTTP-сервера для теста.
# Использование: ./build-and-serve.sh [порт]
# По умолчанию порт 8000.

set -e

PORT="${1:-8000}"

echo "🔧 build-and-serve: сборка → патч → сервер на порту $PORT"
echo ""

cd "$(dirname "$0")"

# 1) Зависимости и окружение
if ! command -v wasm-pack &> /dev/null; then
    echo "❌ wasm-pack не установлен. Установите: cargo install wasm-pack"
    exit 1
fi
if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "❌ target не установлен. Установите: rustup target add wasm32-unknown-unknown"
    exit 1
fi
if [ -f "./setup-env.sh" ]; then
    source ./setup-env.sh
fi

# 2) Проверка компиляции
echo "📦 Шаг 1: cargo check --target wasm32-unknown-unknown"
cargo check --target wasm32-unknown-unknown 2>&1 | tee /tmp/wasm-check.log
if grep -q "error: could not compile" /tmp/wasm-check.log; then
    echo "❌ Ошибки компиляции. См. вывод выше."
    exit 1
fi
echo "✅ Проверка прошла"
echo ""

# 3) Сборка WASM
echo "📦 Шаг 2: wasm-pack build --target web --out-dir pkg"
unset CC AR TARGET_CC TARGET_AR CFLAGS 2>/dev/null || true
if ! wasm-pack build --target web --out-dir pkg; then
    echo "❌ Ошибка сборки WASM"
    exit 1
fi
echo "✅ WASM собран"
echo ""

# 4) Патч
echo "📦 Шаг 3: patch-pkg-env.sh"
[ -f "./patch-pkg-env.sh" ] && chmod +x ./patch-pkg-env.sh 2>/dev/null
./patch-pkg-env.sh
echo ""

# 5) Сервер
echo "🌐 Запуск сервера: http://localhost:$PORT"
echo "   Тест: http://localhost:$PORT/examples/simple-test.html"
echo "   Остановка: Ctrl+C"
echo ""
exec python3 -m http.server "$PORT"
