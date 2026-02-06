#!/bin/bash
# Запуск локального сервера для тестирования WASM в браузере.
# Сервер должен работать из bindings/wasm (чтобы были доступны и pkg/, и examples/).

cd "$(dirname "$0")"

echo "🌐 Сервер запущен из: $(pwd)"
echo "   Откройте в браузере: http://localhost:8000/examples/simple-test.html"
echo "   (остановка: Ctrl+C)"
echo ""

python3 -m http.server 8000
