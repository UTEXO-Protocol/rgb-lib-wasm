# Патчи для сборки rgb-lib с no-op зависимостями (WASM)

Патчи — это **код в репо**, не скрипты. В `[patch.crates-io]` указаны локальные каталоги с уже пропатченными крейтами.

## async-io (Instant на wasm32)

На wasm32 `std::time::Instant::now()` паникует («time not implemented on this platform»). async-io (через sea-orm/sqlx) использует Instant. Патч подменяет его на `instant::Instant` при сборке для wasm32.

**Две версии async-io в дереве:** async-io 2.6.0 (async-std) и async-io 1.13.0 (sqlx-core). Нужны оба патча.

Один раз выполните (нужна сеть):

```bash
./bindings/wasm/noop-deps/prepare-async-io-patch.sh
./bindings/wasm/noop-deps/prepare-async-io-1.13-patch.sh
./bindings/wasm/noop-deps/prepare-sqlx-core-patch.sh
```

- `prepare-async-io-patch.sh` — async-io 2.6.0 → `async-io/`
- `prepare-async-io-1.13-patch.sh` — async-io 1.13.0 → `async-io-1.13/`
- `prepare-sqlx-core-patch.sh` — sqlx-core 0.8.6 подменяет зависимость async-io на `../async-io-1.13` → `sqlx-core/`

Патчи в корневом `Cargo.toml` уже включены.

## rustls

Пропатченный rustls лежит в `bindings/wasm/noop-deps/rustls/` (добавлены `use pki_types::{IntoOwned, ToOwned}` и замена `dns_name.as_ref().as_ref()` → `.as_bytes()`). Патч в `Cargo.toml` на него уже включён. Сборка без скриптов.

Скрипт `prepare-rustls-patch.sh` нужен только если захотите пересобрать этот каталог из новой версии rustls из реестра.

## rustls-webpki

Пропатченный rustls-webpki в `rustls-webpki/` (исправлен `ip_address.rs` для Rust &lt; 1.77). Если каталога нет — один раз запустите:

```bash
./bindings/wasm/noop-deps/prepare-rustls-webpki-patch.sh
```

После этого `cargo check -p rgb-lib` должен проходить.
