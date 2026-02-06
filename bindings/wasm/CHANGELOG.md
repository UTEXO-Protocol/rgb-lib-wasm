# Changelog - In-Memory Support

## ✅ Добавлена поддержка in-memory режима для WASM

### Изменения в rgb-lib

#### `src/wallet/offline.rs`
- ✅ Добавлена проверка `is_in_memory` (когда `data_dir == ":memory:"`)
- ✅ Модифицировано создание `wallet_dir` для in-memory режима
- ✅ Пропуск файловых операций в in-memory режиме
- ✅ Использование `sqlite::memory:` для SQLite connection string
- ✅ Обработка BDK database для in-memory режима
- ✅ Минимальный no-op logger для in-memory режима
- ✅ Добавлен метод `is_in_memory()` для проверки режима

#### `src/utils.rs`
- ✅ Модифицирован `load_rgb_runtime()` для поддержки in-memory
- ✅ Использование `Stock::in_memory()` вместо файлового хранилища
- ✅ Пропуск файловых операций (lockfile, директории) в in-memory режиме

### Изменения в WASM биндингах

#### `bindings/wasm/src/wallet.rs`
- ✅ Автоматическая установка `data_dir = ":memory:"` для WASM
- ✅ Обновлены комментарии и документация

### Как использовать

```rust
// В WASM биндингах
let wallet_data = WalletData {
    data_dir: ":memory:".to_string(),  // ← In-memory режим
    // ... остальные поля
};

let wallet = Wallet::new(wallet_data)?;
```

### Что работает

- ✅ SQLite in-memory база данных
- ✅ RGB runtime in-memory (`Stock::in_memory()`)
- ✅ Создание кошелька без файловой системы
- ✅ Все операции кошелька (кроме персистентности)

### Что добавлено

- ✅ Методы `export_state()` и `from_state()` в Wallet
- ✅ WASM биндинги для экспорта/импорта состояния
- ✅ Базовая сериализация wallet_data

### Что нужно добавить

- ⚠️ Полный экспорт SQLite database dump
- ⚠️ Экспорт RGB runtime state (Stock)
- ⚠️ Экспорт BDK wallet state (changeset)
- ⚠️ Восстановление всех компонентов в `from_state()`

### Следующие шаги

1. Реализовать полную сериализацию всех компонентов (SQLite, RGB runtime, BDK)
2. Добавить тесты для экспорта/импорта состояния
3. Добавить примеры использования в JavaScript/TypeScript
4. Протестировать компиляцию в WASM
