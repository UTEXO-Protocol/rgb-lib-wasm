#!/bin/bash
# Прямая сборка WASM и запуск HTTP-сервера.
# Использование: ./build-and-serve.sh [порт]
#               ./build-and-serve.sh debug [порт] — сборка с --dev для бэктрэйса при панике.
# Порт по умолчанию 8000.

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

# Зависимости
if ! command -v wasm-pack &> /dev/null; then
    echo "❌ wasm-pack не установлен. Установите: cargo install wasm-pack"
    exit 1
fi
if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
    echo "❌ target не установлен. Установите: rustup target add wasm32-unknown-unknown"
    exit 1
fi

[ -f "./setup-env.sh" ] && source ./setup-env.sh

# Прямая сборка WASM (патчи из корня Cargo.toml применяются к воркспейсу автоматически)
echo "📦 wasm-pack build --target web --out-dir pkg"
rm -rf pkg
if [ -n "$BUILD_DEBUG" ]; then
    wasm-pack build --target web --out-dir pkg --dev
else
    wasm-pack build --target web --out-dir pkg
fi
echo "✅ WASM собран"
echo ""

# Полифилл для браузера (импорт 'env' от C-кода) — убрать, когда сборка перестанет его требовать
[ -x "./patch-pkg-env.sh" ] && ./patch-pkg-env.sh || [ -f "./patch-pkg-env.sh" ] && bash ./patch-pkg-env.sh
echo ""

echo "🌐 Сервер: http://localhost:$PORT"
echo "   Тест: http://localhost:$PORT/examples/simple-test.html"
echo "   Остановка: Ctrl+C"
echo ""
exec python3 -m http.server "$PORT"
