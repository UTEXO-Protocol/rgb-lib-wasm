# Полностью онлайн в тесте WASM — что нужно

Чтобы в браузерном тесте выйти «полностью в онлайн» (синк с блокчейном, балансы, refresh), нужно следующее.

## Включено (шаг 1)

- **rgb-lib**: добавлена фича **esplora-wasm** (без rustls): `bdk_esplora`, `bp-esplora`, `reqwest`, `rgb-ops/esplora_blocking`.
- **Целевые зависимости для wasm32**: в `[target.'cfg(target_arch = "wasm32")'.dependencies]` добавлены `bdk_esplora` (async-https), `reqwest` (json, wasm), `bp-esplora` — для wasm32 используются они вместо блокирующих/rustls.
- **Код**: все `#[cfg(feature = "esplora")]` заменены на `#[cfg(any(feature = "esplora", feature = "esplora-wasm"))]` в `lib.rs`, `utils.rs`, `wallet/online.rs`, `error.rs`.
- **bindings/wasm**: в `Cargo.toml` rgb-lib подключается с `features = ["esplora-wasm"]`.

**Проверка:** из `bindings/wasm` выполнить:
```bash
cargo check --target wasm32-unknown-unknown --no-default-features --features esplora-wasm
```
или полная сборка:
```bash
./test-now.sh
```
Если появятся ошибки (например, блокирующий Esplora API в wasm32), потребуется добавить async-путь или `#[cfg(not(target_arch = "wasm32"))]` для блокирующего кода (шаг 2–3 из чеклиста ниже).

---

## Что значит «полностью онлайн»

- **go_online** — подключить кошелёк к индексатору (Esplora URL), синхронизировать UTXO и граф транзакций.
- **refresh** — обновить статусы переводов (pending → settled и т.д.).
- **get_btc_balance** — получить BTC-баланс (после sync).
- **list_assets / get_asset_balance** — активы и их балансы.

Сейчас в WASM доступны только **офлайн**-вещи: генерация ключей, создание кошелька (in-memory), экспорт/импорт состояния. Методов `go_online`, `refresh`, `get_btc_balance` в bindings нет, а в rgb-lib они завязаны на фичу **esplora** (или electrum), которую мы для WASM не включаем.

---

## Что нужно по шагам

### 1. Включить Esplora для wasm32 в rgb-lib

- **Сейчас:** в корневом `Cargo.toml` фича `esplora` тянет `bdk_esplora` с `blocking-https-rustls`, `reqwest`, `rustls`. В WASM-сборке rgb-lib подключается **без** `esplora` (`default-features = false`, `features = []` в `bindings/wasm`), чтобы не тащить rustls/ring и блокирующий HTTP.
- **Нужно:** отдельная конфигурация для `target_arch = "wasm32"`:
  - Подключить `bdk_esplora` с фичей **async-https** (и при необходимости TLS, совместимый с WASM).
  - Подключить `bp-esplora` с асинхронным API для WASM (если есть такая опция).
  - Использовать HTTP-клиент, работающий в браузере: например `reqwest` с фичей `wasm` или аналог (gloo-net и т.п.).
  - Рантайм: `wasm-bindgen-futures` + возможно `gloo-timers` (как в bdk-wasm).

Итог: в `Cargo.toml` rgb-lib нужны условные зависимости вида `[target.'cfg(target_arch = "wasm32")'.dependencies]` для esplora/reqwest с wasm-фичами, без блокирующего rustls в том виде, в каком он сейчас используется на десктопе.

### 2. Асинхронный путь в rgb-lib для WASM

- **Сейчас:** `go_online`, `refresh`, `get_btc_balance` в rgb-lib используют **блокирующий** Esplora client (`BlockingClient`, `esplora_blocking`).
- **Нужно:** для `#[cfg(target_arch = "wasm32")]` вызывать **асинхронный** API (например `EsploraAsyncExt` у bdk_esplora) и пробрасывать async через верхний API. То есть либо:
  - ввести async-версии методов (`go_online_async`, `refresh_async`, `get_btc_balance_async`), либо
  - сделать существующие методы асинхронными там, где они обращаются к индексатору (тогда сигнатуры в rgb-lib поменяются для WASM).

Плюс в WASM не должно быть блокирующего runtime (blocking thread pool), только `wasm-bindgen-futures` и event loop браузера.

### 3. Экспорт в WASM bindings (bindings/wasm)

- **Сейчас:** из кошелька в WASM экспортируются только создание из JSON, `get_wallet_data`, `export_state`, `from_state`.
- **Нужно:**
  - Экспортировать тип **Online** (или его JS-аналог: например id + URL индексатора).
  - Экспортировать **go_online**(wallet, indexer_url, skip_consistency_check) — лучше асинхронно, т.к. в браузере sync по сети должен быть async.
  - Экспортировать **refresh**(wallet, online, asset_id, filter, skip_sync).
  - Экспортировать **get_btc_balance**(wallet, online, skip_sync).
  - При необходимости **list_assets** / **get_asset_balance** и т.д., если нужны в тесте.

Все эти методы в rgb-lib уже есть, но они за `#[cfg(feature = "esplora")]` и блокирующие; в bindings их нужно вызывать из async-обёрток и только при включённой esplora для wasm32.

### 4. Тестовая страница (полностью онлайн)

- Создать кошелёк (как сейчас: ключи + `create_wallet_data` + `Wallet::new`).
- Вызвать **go_online** с публичным URL Esplora (например `https://blockstream.info/api` для mainnet или testnet).
- Дождаться окончания (async).
- Вызвать **refresh** (и при необходимости **get_btc_balance**).
- Показать в интерфейсе: «online», баланс, список активов (если экспортировали).

Плюс: учёт CORS — публичный Esplora может не пускать запросы из браузера с другого origin; тогда нужен свой прокси или Esplora с CORS.

---

## Краткий чеклист

| Шаг | Что сделать | Где |
|-----|-------------|-----|
| 1 | Включить esplora для wasm32 (async-https, reqwest wasm, без blocking rustls) | rgb-lib `Cargo.toml` |
| 2 | Реализовать async-путь для go_online/refresh/get_btc_balance под wasm32 | rgb-lib `src/wallet/online.rs` и др. |
| 3 | Экспортировать Online, go_online, refresh, get_btc_balance в WASM | `bindings/wasm/src/wallet.rs` |
| 4 | Добавить тест «полностью онлайн» (go_online → refresh → баланс) | `examples/simple-test.html` или новый пример |

---

## Почему сейчас нельзя «просто включить» онлайн

- В rgb-lib Esplora включён через фичу **esplora** с **blocking** HTTP и **rustls**. В WASM блокирующие вызовы и текущий rustls/ring не подходят.
- Нужен отдельный путь для wasm32: **async** Esplora + HTTP-клиент для браузера + экспорт async-методов в JS (например через `#[wasm_bindgen]` и `Promise`).

После выполнения шагов 1–4 тест в браузере сможет «полностью выйти в онлайн»: go_online → refresh → показ баланса и активов.
