#!/bin/bash
# Сборка WASM, патч pkg, запуск HTTP-сервера для теста.
# Использование: ./build-and-serve.sh [порт]
#             или ./build-and-serve.sh debug [порт] — сборка с debug info для бэктрэйса при панике.
# По умолчанию порт 8000.

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

echo "🔧 build-and-serve: сборка → патч → сервер на порту $PORT"
[ -n "$BUILD_DEBUG" ] && echo "   Режим: debug (бэктрэйс при панике будет с именами функций)"
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

# 2) Проверка компиляции (из корня воркспейса, чтобы применялся [patch.crates-io] с bdk_core)
ROOT="$(cd .. && pwd)"
echo "📦 Шаг 1: cargo check --target wasm32-unknown-unknown (from workspace root)"
(cd "$ROOT" && cargo check -p rgb-lib-wasm --target wasm32-unknown-unknown) 2>&1 | tee /tmp/wasm-check.log
if grep -q "error: could not compile" /tmp/wasm-check.log; then
    echo "❌ Ошибки компиляции. См. вывод выше."
    exit 1
fi
echo "✅ Проверка прошла"
echo ""

# Подставить патч bdk_core в Cargo.lock (patch 0.6.2 = path; иначе в WASM попадёт crates.io и std::time panic)
if [ -d "noop-deps/bdk_core" ] && grep -q 'target_arch = "wasm32"' noop-deps/bdk_core/src/spk_client.rs 2>/dev/null; then
    echo "📦 cargo update -p bdk_core (применить [patch] bdk_core 0.6.2 → path)..."
    (cd "$ROOT" && cargo update -p bdk_core 2>&1) || true
    echo "📦 Очистка кэша для пересборки с patched bdk_core..."
    (cd "$ROOT" && cargo clean -p bdk_core -p rgb-lib-wasm 2>/dev/null || true)
fi
echo ""

# 3) Сборка WASM (запуск из bindings/wasm, где есть Cargo.toml; патч из корня уже в lock после cargo update)
echo "📦 Шаг 2: wasm-pack build --target web --out-dir pkg"
rm -rf pkg
unset CC AR TARGET_CC TARGET_AR CFLAGS 2>/dev/null || true
if [ -n "$BUILD_DEBUG" ]; then
    echo "   (профиль dev для бэктрэйса)"
    if ! wasm-pack build --target web --out-dir pkg --dev; then
        echo "❌ Ошибка сборки WASM"
        exit 1
    fi
else
    if ! wasm-pack build --target web --out-dir pkg; then
        echo "❌ Ошибка сборки WASM"
        exit 1
    fi
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
echo "   После пересборки сделайте жёсткое обновление страницы (Ctrl+Shift+R), чтобы подхватить syncWalletAsync."
echo "   Остановка: Ctrl+C"
echo ""
exec python3 -m http.server "$PORT"
