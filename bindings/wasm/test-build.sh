#!/bin/bash
# Скрипт для тестирования сборки WASM модуля
# Запускайте вне sandbox для полного доступа к файловой системе

set -e

echo "🔧 Настройка окружения для WASM..."
cd "$(dirname "$0")"
source setup-env.sh

echo ""
echo "📦 Проверка компиляции..."
cargo check --target wasm32-unknown-unknown

echo ""
echo "✅ Компиляция успешна! Теперь можно собрать WASM модуль:"
echo ""
echo "   wasm-pack build --target web --out-dir pkg"
echo ""
echo "Или для Node.js:"
echo ""
echo "   wasm-pack build --target nodejs --out-dir pkg"
echo ""
